use super::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, ArtefactKind, ArtefactSource, HumanGateStep,
    HumanRevisionPolicy, InputKey, ModelStepSettings, OutputKey, OutputKind, RequiredInput,
    RequiredOutput, ReviewPolicy, RoleDefinition, RoleKey, StepAccess, StepAction, StepDefinition,
    StepEnvironment, StepKey, WorkflowDefinition,
};
use crate::{agents::ToolId, environments::EnvironmentId};

pub(crate) const PLAN_A_CHANGE_V1: &str = "plan-a-change-v1";
pub(crate) const REVIEW_CURRENT_CODE_V1: &str = "review-current-code-v1";
pub(crate) const IMPLEMENT_A_CHANGE_V1: &str = "implement-a-change-v1";
pub(crate) const IMPLEMENT_AND_REVIEW_V1: &str = "implement-and-review-v1";
pub(crate) const PLAN_THEN_IMPLEMENT_V1: &str = "plan-then-implement-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SeedKey(String);

#[derive(Clone, Debug)]
pub(crate) struct WorkflowSeed {
    pub(crate) key: SeedKey,
    pub(crate) definition: WorkflowDefinition,
}

impl SeedKey {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let key = value.trim();
        if key.is_empty()
            || key.len() > 32
            || !key.starts_with(|c: char| c.is_ascii_alphabetic())
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        Some(Self(key.to_owned()))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn production_seeds(environment: EnvironmentId) -> Vec<WorkflowSeed> {
    [
        (PLAN_A_CHANGE_V1, plan_a_change_definition(environment)),
        (
            REVIEW_CURRENT_CODE_V1,
            review_current_code_definition(environment),
        ),
        (
            IMPLEMENT_A_CHANGE_V1,
            implement_a_change_definition(environment),
        ),
        (
            IMPLEMENT_AND_REVIEW_V1,
            implement_and_review_definition(environment),
        ),
        (
            PLAN_THEN_IMPLEMENT_V1,
            plan_then_implement_definition(environment),
        ),
    ]
    .into_iter()
    .map(|(key, definition)| WorkflowSeed {
        key: SeedKey::parse(key).expect("seed key"),
        definition,
    })
    .collect()
}

pub(crate) fn plan_a_change_definition(environment: EnvironmentId) -> WorkflowDefinition {
    definition(
        "Plan a change",
        environment,
        vec![role(
            "planner",
            "Planner",
            "Inspect the authorised directories and produce a plan. Do not change files.",
        )],
        vec![agent_step(
            "planner",
            "Plan the change",
            "planner",
            StepAccess::Read,
            vec![],
            vec![output("plan", OutputKind::Plan)],
        )],
    )
}

pub(crate) fn review_current_code_definition(environment: EnvironmentId) -> WorkflowDefinition {
    definition(
        "Review current code",
        environment,
        vec![role(
            "reviewer",
            "Reviewer",
            "Review the current files for correctness, security and regressions. Do not change files.",
        )],
        vec![agent_step(
            "reviewer",
            "Review the current code",
            "reviewer",
            StepAccess::Read,
            vec![],
            vec![output("review", OutputKind::ReviewReport)],
        )],
    )
}

pub(crate) fn implement_a_change_definition(environment: EnvironmentId) -> WorkflowDefinition {
    definition(
        "Implement a change",
        environment,
        vec![implementer()],
        vec![agent_step(
            "implementer",
            "Implement the change",
            "implementer",
            StepAccess::Write,
            vec![],
            vec![],
        )],
    )
}

pub(crate) fn implement_and_review_definition(environment: EnvironmentId) -> WorkflowDefinition {
    let work = agent_step(
        "implementer",
        "Implement the change",
        "implementer",
        StepAccess::Write,
        vec![RequiredInput {
            key: InputKey::parse("review-feedback").expect("input"),
            kind: ArtefactKind::ReviewReport,
            source: ArtefactSource::StepOutput {
                step: StepKey::parse("reviewer").expect("step"),
                output: OutputKey::parse("review").expect("output"),
            },
        }],
        vec![],
    );
    let mut review = agent_step(
        "reviewer",
        "Review the change",
        "reviewer",
        StepAccess::Read,
        vec![],
        vec![output("review", OutputKind::ReviewReport)],
    );
    review.review = Some(ReviewPolicy {
        report_output: OutputKey::parse("review").expect("output"),
        revision_target: StepKey::parse("implementer").expect("step"),
        attempt_limit: 3,
    });
    definition(
        "Implement and review",
        environment,
        vec![
            implementer(),
            role(
                "reviewer",
                "Reviewer",
                "Review the current files against the task. Report defects or approve the result. Do not change files.",
            ),
        ],
        vec![work, review],
    )
}

pub(crate) fn plan_then_implement_definition(environment: EnvironmentId) -> WorkflowDefinition {
    let plan_input = || RequiredInput {
        key: InputKey::parse("plan").expect("input"),
        kind: ArtefactKind::Plan,
        source: ArtefactSource::RunCurrentPlan,
    };
    let planner = agent_step(
        "planner",
        "Plan the change",
        "planner",
        StepAccess::Read,
        vec![plan_input()],
        vec![output("plan", OutputKind::Plan)],
    );
    let approval = StepDefinition {
        key: StepKey::parse("plan-approval").expect("step"),
        name: "Review the plan".to_owned(),
        inputs: vec![plan_input()],
        action: StepAction::HumanGate(HumanGateStep {
            required_output: output("plan-decision", OutputKind::PlanDecision),
            revision: Some(HumanRevisionPolicy {
                revision_target: StepKey::parse("planner").expect("step"),
                attempt_limit: 3,
            }),
        }),
        review: None,
    };
    let implementer_step = agent_step(
        "implementer",
        "Implement the plan",
        "implementer",
        StepAccess::Write,
        vec![
            plan_input(),
            RequiredInput {
                key: InputKey::parse("plan-decision").expect("input"),
                kind: ArtefactKind::PlanDecision,
                source: ArtefactSource::StepOutput {
                    step: StepKey::parse("plan-approval").expect("step"),
                    output: OutputKey::parse("plan-decision").expect("output"),
                },
            },
        ],
        vec![],
    );
    definition(
        "Plan then implement",
        environment,
        vec![
            role(
                "planner",
                "Planner",
                "Produce a plan from the task and current files. Use revision feedback when supplied. Do not change files.",
            ),
            implementer(),
        ],
        vec![planner, approval, implementer_step],
    )
}

fn implementer() -> RoleDefinition {
    role(
        "implementer",
        "Implementer",
        "Complete the requested change in the authorised directories. Writes take effect immediately. Use prior review feedback when supplied.",
    )
}

fn role(key: &str, name: &str, instructions: &str) -> RoleDefinition {
    RoleDefinition::new(
        RoleKey::parse(key).expect("role"),
        name.to_owned(),
        String::new(),
        instructions.to_owned(),
    )
    .expect("role definition")
}

fn output(key: &str, kind: OutputKind) -> RequiredOutput {
    RequiredOutput {
        key: OutputKey::parse(key).expect("output"),
        kind,
    }
}

fn agent_step(
    key: &str,
    name: &str,
    role: &str,
    access: StepAccess,
    inputs: Vec<RequiredInput>,
    mut outputs: Vec<RequiredOutput>,
) -> StepDefinition {
    outputs.insert(0, output(ASSISTANT_REPLY, OutputKind::AssistantReply));
    StepDefinition {
        key: StepKey::parse(key).expect("step"),
        name: name.to_owned(),
        inputs,
        action: StepAction::Agent(AgentStep {
            role: RoleKey::parse(role).expect("role"),
            environment: StepEnvironment::WorkflowDefault,
            directory_access: access,
            authority: AgentAuthority::new(
                if access == StepAccess::Read {
                    vec![ToolId::List, ToolId::Read, ToolId::Run]
                } else {
                    ToolId::ALL.to_vec()
                },
                vec![],
            )
            .expect("authority"),
            required_outputs: outputs,
            settings: ModelStepSettings::SameAsRunDefaults,
        }),
        review: None,
    }
}

fn definition(
    name: &str,
    environment: EnvironmentId,
    roles: Vec<RoleDefinition>,
    steps: Vec<StepDefinition>,
) -> WorkflowDefinition {
    WorkflowDefinition::from_parts(name.to_owned(), environment, roles, steps)
        .expect("bundled workflow")
}

#[cfg(test)]
pub(crate) mod tests;
