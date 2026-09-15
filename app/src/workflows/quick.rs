use crate::agents::{AccessMode, ToolId};
use crate::environments::seeds::ALPINE_GIT_V1;
use crate::environments::{EnvironmentCatalogue, EnvironmentId};

use super::commands::SystemCommandId;
use super::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, ArtefactKind, ArtefactSource, CandidateAuthority,
    DefinitionError, GuestDirectoryAccess, HumanGateStep, HumanRevisionPolicy, InputKey, OutputKey,
    OutputKind, PinnedWorkflowDefinition, RequiredInput, RequiredOutput, RoleDefinition, RoleKey,
    StepAction, StepDefinition, StepEnvironment, StepKey, SystemCommandStep, WorkflowDefinition,
    candidate_revision_output,
};
use super::resolve::ResolveEnvironmentError;

pub(crate) const QUICK_TASK_NAME: &str = "Agent work";
pub(crate) const HOST_UNCHANGED: &str =
    "The host project is unchanged. Review the candidate before you apply it.";
const ROLE_KEY: &str = "agent";
const AGENT_STEP_KEY: &str = "work";
const GATE_STEP_KEY: &str = "gate";
const DECISION_OUTPUT_KEY: &str = "decision";

pub(crate) fn pin_agent_work(
    settings: &crate::execution::ExecutionSettings,
) -> Result<PinnedWorkflowDefinition, DefinitionError> {
    let reviewed = settings.location == crate::execution::ToolLocation::Sandbox
        && settings
            .directories
            .iter()
            .any(|grant| grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply);
    let pinned = pin_project_free_quick_task_with_directories(
        &crate::tools::advertised(&settings.tools, settings.location),
        &settings.instructions,
        settings.environment,
        settings
            .directories
            .iter()
            .map(|grant| GuestDirectoryAccess {
                alias: grant.alias.clone(),
                access: AccessMode::ReadOnly,
            })
            .collect(),
        reviewed,
    )?;
    let definition = pinned.definition.with_conversation_settings(settings)?;
    Ok(PinnedWorkflowDefinition::pin(None, definition))
}

pub(crate) fn pin_project_free_quick_task_with_directories(
    tools: &[ToolId],
    instructions: &str,
    environment: EnvironmentId,
    directories: Vec<GuestDirectoryAccess>,
    reviewed: bool,
) -> Result<PinnedWorkflowDefinition, DefinitionError> {
    let role = RoleDefinition::new(
        RoleKey::parse(ROLE_KEY).expect("quick task role"),
        "Agent".to_owned(),
        String::new(),
        instructions.to_owned(),
    )?;
    let work = StepDefinition {
        key: StepKey::parse(AGENT_STEP_KEY).expect("quick task step"),
        name: if reviewed {
            "Prepare changes"
        } else {
            "Use tools"
        }
        .to_owned(),
        inputs: reviewed
            .then(|| RequiredInput {
                key: InputKey::parse("candidate").expect("quick candidate"),
                kind: ArtefactKind::CandidateRevision,
                source: ArtefactSource::RunCurrentCandidate,
            })
            .into_iter()
            .collect(),
        action: StepAction::Agent(AgentStep {
            role: RoleKey::parse(ROLE_KEY).expect("quick task role"),
            environment: StepEnvironment::WorkflowDefault,
            candidate_authority: if reviewed {
                CandidateAuthority::Edit
            } else {
                CandidateAuthority::ReadOnly
            },
            authority: AgentAuthority::new(tools.to_vec(), directories)?,
            settings: super::definition::ModelStepSettings::SameAsRunDefaults,
            required_outputs: if reviewed {
                vec![assistant_output(), candidate_revision_output()]
            } else {
                vec![assistant_output()]
            },
        }),
        review: None,
    };
    let mut steps = vec![work];
    if reviewed {
        // Revision retains the rejected candidate and original baseline through the reservation.
        steps.push(gate_step());
        steps.push(apply_step());
    }
    let definition =
        WorkflowDefinition::from_parts(QUICK_TASK_NAME.to_owned(), environment, vec![role], steps)?;
    Ok(PinnedWorkflowDefinition::pin(None, definition))
}

pub(crate) fn alpine_git_id(
    catalogue: &EnvironmentCatalogue,
) -> Result<EnvironmentId, ResolveEnvironmentError> {
    catalogue
        .seed_id(ALPINE_GIT_V1)
        .ok_or(ResolveEnvironmentError::Missing)
}

pub(super) fn is_expected_gate_step(step: &StepDefinition) -> bool {
    step.key.as_str() == GATE_STEP_KEY
        && step.inputs.len() == 1
        && step.inputs[0].key.as_str() == "candidate"
        && step.inputs[0].kind == ArtefactKind::CandidateRevision
        && matches!(
            &step.inputs[0].source,
            ArtefactSource::StepOutput { step, output }
                if step.as_str() == AGENT_STEP_KEY && output.as_str() == "candidate"
        )
        && matches!(
            &step.action,
            StepAction::HumanGate(action)
                if action.required_output.key.as_str() == DECISION_OUTPUT_KEY
                    && action.required_output.kind == OutputKind::HumanDecision
        )
        && step.review.is_none()
}

fn gate_step() -> StepDefinition {
    StepDefinition {
        key: StepKey::parse(GATE_STEP_KEY).expect("quick task gate"),
        name: "Review changes".to_owned(),
        inputs: vec![step_output(
            "candidate",
            ArtefactKind::CandidateRevision,
            AGENT_STEP_KEY,
            "candidate",
        )],
        action: StepAction::HumanGate(HumanGateStep {
            required_output: RequiredOutput {
                key: OutputKey::parse(DECISION_OUTPUT_KEY).expect("quick task decision"),
                kind: OutputKind::HumanDecision,
            },
            revision: Some(HumanRevisionPolicy {
                revision_target: StepKey::parse(AGENT_STEP_KEY).expect("quick task work"),
                attempt_limit: 3,
            }),
        }),
        review: None,
    }
}

fn apply_step() -> StepDefinition {
    StepDefinition {
        key: StepKey::parse("apply").expect("quick task apply"),
        name: "Apply changes".to_owned(),
        inputs: vec![
            step_output(
                "candidate",
                ArtefactKind::CandidateRevision,
                AGENT_STEP_KEY,
                "candidate",
            ),
            step_output(
                "decision",
                ArtefactKind::HumanDecision,
                GATE_STEP_KEY,
                DECISION_OUTPUT_KEY,
            ),
        ],
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::ApplyChanges,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: vec![RequiredOutput {
                key: OutputKey::parse("applied-candidate").expect("quick task applied"),
                kind: OutputKind::CandidateRevision,
            }],
        }),
        review: None,
    }
}

fn assistant_output() -> RequiredOutput {
    RequiredOutput {
        key: OutputKey::parse(ASSISTANT_REPLY).expect("assistant output"),
        kind: OutputKind::AssistantReply,
    }
}

fn step_output(key: &str, kind: ArtefactKind, step: &str, output: &str) -> RequiredInput {
    RequiredInput {
        key: InputKey::parse(key).expect("quick task input"),
        kind,
        source: ArtefactSource::StepOutput {
            step: StepKey::parse(step).expect("quick task source step"),
            output: OutputKey::parse(output).expect("quick task source output"),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests;
