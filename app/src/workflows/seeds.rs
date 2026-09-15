use crate::agents::ToolId;
use crate::environments::EnvironmentId;

use super::commands::SystemCommandId;
use super::definition::{
    ASSISTANT_REPLY, AgentAuthority, AgentStep, ArtefactKind, ArtefactSource, CandidateAuthority,
    HumanGateStep, HumanRevisionPolicy, InputKey, OutputKey, OutputKind, RequiredInput,
    RequiredOutput, ReviewPolicy, RoleDefinition, RoleKey, StepAction, StepDefinition,
    StepEnvironment, StepKey, SystemCommandStep, WorkflowDefinition, candidate_revision_output,
    initial_candidate_input,
};
pub(crate) const PLAN_A_CHANGE_V1: &str = "plan-a-change-v1";
pub(crate) const REVIEW_CURRENT_CODE_V1: &str = "review-current-code-v1";
pub(crate) const IMPLEMENT_WITH_APPROVAL_V1: &str = "implement-with-approval-v1";
pub(crate) const IMPLEMENT_AND_REVIEW_V1: &str = "implement-and-review-v1";
pub(crate) const PLAN_THEN_IMPLEMENT_V1: &str = "plan-then-implement-v1";

#[cfg(test)]
pub(crate) const ONE_AGENT_V1: &str = "one-agent-v1";
#[cfg(test)]
pub(crate) const SEQUENTIAL_TEAM_V1: &str = "sequential-team-v1";

const SEED_KEY_BYTES: usize = 32;

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
        if key.is_empty() || key.len() > SEED_KEY_BYTES {
            return None;
        }
        let mut characters = key.chars();
        let first = characters.next()?;
        if !first.is_ascii_alphabetic() {
            return None;
        }
        if !characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }) {
            return None;
        }
        Some(Self(key.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn production_seeds(default_environment: EnvironmentId) -> Vec<WorkflowSeed> {
    [
        (
            PLAN_A_CHANGE_V1,
            plan_a_change_definition(default_environment),
        ),
        (
            REVIEW_CURRENT_CODE_V1,
            review_current_code_definition(default_environment),
        ),
        (
            IMPLEMENT_WITH_APPROVAL_V1,
            implement_with_approval_definition(default_environment),
        ),
        (
            IMPLEMENT_AND_REVIEW_V1,
            implement_and_review_definition(default_environment),
        ),
        (
            PLAN_THEN_IMPLEMENT_V1,
            plan_then_implement_definition(default_environment),
        ),
    ]
    .into_iter()
    .map(|(key, definition)| WorkflowSeed {
        key: SeedKey::parse(key).expect("workflow seed key"),
        definition,
    })
    .collect()
}

pub(crate) fn plan_a_change_definition(default_environment: EnvironmentId) -> WorkflowDefinition {
    let roles = vec![role(
        "planner",
        "Planner",
        "Produces a plan from the brief and authorised directory context.",
        "Produce a plan from the brief. Inspect authorised directories when available. Do not change files.",
    )];
    let planner = agent_step(
        "planner",
        "Plan the change",
        "planner",
        CandidateAuthority::ReadOnly,
        review_tools(),
        Vec::new(),
        vec![assistant_output(), output("plan", OutputKind::Plan)],
        None,
    );
    definition("Plan a change", default_environment, roles, vec![planner])
}

pub(crate) fn review_current_code_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![role(
        "reviewer",
        "Reviewer",
        "Checks the pinned directory contents for correctness, security and regressions.",
        "Inspect the pinned directory contents and produce a review. Do not change the candidate set.",
    )];
    let reviewer = agent_step(
        "reviewer",
        "Review the current code",
        "reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![initial_candidate_input()],
        vec![assistant_output(), review_output()],
        None,
    );
    definition(
        "Review current code",
        default_environment,
        roles,
        vec![reviewer],
    )
}

pub(crate) fn implement_with_approval_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![role(
        "implementer",
        "Implementer",
        "Applies the requested change to an isolated candidate.",
        "Implement the task in the candidate. Submit the complete candidate for approval.",
    )];
    let implementer = agent_step(
        "implementer",
        "Implement the change",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![current_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let approval = human_approval_step("implementer", None);
    let commit = apply_with_approval("implementer", None, "approval");
    definition(
        "Implement with approval",
        default_environment,
        roles,
        vec![implementer, approval, commit],
    )
}

pub(crate) fn plan_then_implement_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![
        role(
            "planner",
            "Planner",
            "Inspects the project and prepares the first plan.",
            "Inspect the project and produce a plan. Do not change the candidate.",
        ),
        role(
            "plan-reviewer",
            "Plan reviewer",
            "Reviews and corrects the exact plan before acceptance.",
            "Review the exact plan. Correct it when needed and explain the result. Do not change the candidate.",
        ),
        role(
            "implementer",
            "Implementer",
            "Applies the accepted plan to an isolated candidate.",
            "Apply the accepted plan. Submit the complete candidate for review.",
        ),
        role(
            "reviewer",
            "Reviewer",
            "Checks the candidate for correctness, security and regressions.",
            "Review this exact candidate. Do not change it. Submit a structured review.",
        ),
    ];
    let planner = agent_step(
        "planner",
        "Prepare the plan",
        "planner",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![initial_candidate_input()],
        vec![assistant_output(), output("plan", OutputKind::Plan)],
        None,
    );
    let plan_review = agent_step(
        "plan-review",
        "Review and correct the plan",
        "plan-reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![
            initial_candidate_input(),
            RequiredInput {
                key: InputKey::parse("plan").expect("plan input"),
                kind: ArtefactKind::Plan,
                source: ArtefactSource::RunCurrentPlan,
            },
        ],
        vec![assistant_output(), output("plan", OutputKind::Plan)],
        None,
    );
    let plan_checkpoint = plan_checkpoint_step("plan-review", "plan");
    let implementer = agent_step(
        "implementer",
        "Implement the accepted plan",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![
            current_candidate_input(),
            input("plan", ArtefactKind::Plan, "plan-review", "plan"),
            input(
                "plan-decision",
                ArtefactKind::PlanDecision,
                "plan-acceptance",
                "plan-decision",
            ),
        ],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let reviewer = agent_step(
        "reviewer",
        "Review the implementation",
        "reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![current_candidate_input()],
        vec![assistant_output(), review_output()],
        None,
    );
    let approval = human_approval_step("implementer", Some("reviewer"));
    let commit = apply_with_approval("implementer", Some("reviewer"), "approval");
    definition(
        "Plan then implement",
        default_environment,
        roles,
        vec![
            planner,
            plan_review,
            plan_checkpoint,
            implementer,
            reviewer,
            approval,
            commit,
        ],
    )
}

pub(crate) fn implement_and_review_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![
        role(
            "implementer",
            "Implementer",
            "Applies the requested change to an isolated candidate.",
            "Implement the task in the candidate. Submit the complete candidate for review.",
        ),
        role(
            "reviewer",
            "Reviewer",
            "Checks the candidate for correctness, security and regressions.",
            "Review this exact candidate. Do not change it. Submit a structured review.",
        ),
    ];
    let implementer = agent_step(
        "implementer",
        "Implement the change",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![current_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let reviewer = agent_step(
        "reviewer",
        "Review the change",
        "reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![current_candidate_input()],
        vec![assistant_output(), review_output()],
        None,
    );
    let approval = human_approval_step("implementer", Some("reviewer"));
    let commit = apply_with_approval("implementer", Some("reviewer"), "approval");
    definition(
        "Implement and review",
        default_environment,
        roles,
        vec![implementer, reviewer, approval, commit],
    )
}

#[cfg(test)]
pub(crate) fn one_agent_definition(default_environment: EnvironmentId) -> WorkflowDefinition {
    let role = role("coding-agent", "Coding agent", "", "");
    let step = agent_step(
        "work-on-task",
        "Work on task",
        "coding-agent",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![initial_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    definition("One agent", default_environment, vec![role], vec![step])
}

#[cfg(test)]
pub(crate) fn sequential_team_definition(default_environment: EnvironmentId) -> WorkflowDefinition {
    let roles = vec![
        role(
            "planner",
            "Planner",
            "Analyses the task, repository, constraints, and implementation sequence.",
            "Submit a plan that describes the implementation sequence. Do not implement the change.",
        ),
        role(
            "implementer",
            "Implementer",
            "Applies the accepted plan and produces the complete candidate.",
            "Apply the accepted plan. Submit the complete candidate.",
        ),
        role(
            "reviewer",
            "Reviewer",
            "Checks correctness, security, regressions, scope, and plan compliance.",
            "Assess the accepted plan and the candidate. Submit a review report for this exact candidate.",
        ),
    ];
    let planner = agent_step(
        "planner",
        "Planner",
        "planner",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![initial_candidate_input()],
        vec![assistant_output(), output("plan", OutputKind::Plan)],
        None,
    );
    let implementer = agent_step(
        "implementer",
        "Implementer",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![
            initial_candidate_input(),
            input("plan", ArtefactKind::Plan, "planner", "plan"),
        ],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let reviewer = agent_step(
        "reviewer",
        "Reviewer",
        "reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![
            input(
                "candidate",
                ArtefactKind::CandidateRevision,
                "implementer",
                "candidate",
            ),
            input("plan", ArtefactKind::Plan, "planner", "plan"),
        ],
        vec![assistant_output(), review_output()],
        None,
    );
    let commit = commit_step("implementer", "reviewer");
    definition(
        "Sequential team",
        default_environment,
        roles,
        vec![planner, implementer, reviewer, commit],
    )
}

#[cfg(test)]
pub(crate) fn review_with_fixes_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![
        role(
            "implementer",
            "Implementer",
            "Implements the requested change.",
            "Implement the task and submit the complete candidate.",
        ),
        role(
            "fixing-reviewer",
            "Fixing reviewer",
            "Reviews the candidate and fixes safe issues.",
            "Fix every safe issue that you find. Submit a structured verdict for your output candidate.",
        ),
        role(
            "independent-reviewer",
            "Independent reviewer",
            "Assesses the fixed candidate without trust in the prior verdict.",
            "Review the fixed candidate independently. Treat the prior review as context only.",
        ),
    ];
    let implementer = agent_step(
        "implementer",
        "Implementer",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![initial_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let fixing = agent_step(
        "fixing-reviewer",
        "Fixing reviewer",
        "fixing-reviewer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![input(
            "candidate",
            ArtefactKind::CandidateRevision,
            "implementer",
            "candidate",
        )],
        vec![
            assistant_output(),
            candidate_revision_output(),
            review_output(),
        ],
        None,
    );
    let independent = agent_step(
        "independent-reviewer",
        "Independent reviewer",
        "independent-reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![
            input(
                "candidate",
                ArtefactKind::CandidateRevision,
                "fixing-reviewer",
                "candidate",
            ),
            input(
                "prior-review",
                ArtefactKind::ReviewReport,
                "fixing-reviewer",
                "review",
            ),
        ],
        vec![assistant_output(), review_output()],
        None,
    );
    definition(
        "Review with fixes",
        default_environment,
        roles,
        vec![
            implementer,
            fixing,
            independent,
            commit_step("fixing-reviewer", "independent-reviewer"),
        ],
    )
}

#[cfg(test)]
pub(crate) fn review_until_approved_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![
        role(
            "implementer",
            "Implementer",
            "Implements requested changes.",
            "Implement the task and address each review.",
        ),
        role(
            "reviewer",
            "Reviewer",
            "Checks correctness and regressions.",
            "Submit a structured review verdict for this candidate.",
        ),
    ];
    let implementer = agent_step(
        "implementer",
        "Implementer",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![current_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let reviewer = agent_step(
        "reviewer",
        "Reviewer",
        "reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![current_candidate_input()],
        vec![assistant_output(), review_output()],
        Some(review_policy("review", "implementer", 3)),
    );
    definition(
        "Review until approved",
        default_environment,
        roles,
        vec![
            implementer,
            reviewer,
            commit_step_current(&[("review", "reviewer")]),
        ],
    )
}

#[cfg(test)]
pub(crate) fn correctness_security_definition(
    default_environment: EnvironmentId,
) -> WorkflowDefinition {
    let roles = vec![
        role(
            "implementer",
            "Implementer",
            "Implements requested changes.",
            "Implement the task and address each review.",
        ),
        role(
            "correctness-reviewer",
            "Correctness reviewer",
            "Checks behaviour and regressions.",
            "Submit a structured correctness verdict.",
        ),
        role(
            "security-reviewer",
            "Security reviewer",
            "Checks security boundaries and unsafe input.",
            "Submit a structured security verdict.",
        ),
    ];
    let implementer = agent_step(
        "implementer",
        "Implementer",
        "implementer",
        CandidateAuthority::Edit,
        ToolId::ALL.to_vec(),
        vec![current_candidate_input()],
        vec![assistant_output(), candidate_revision_output()],
        None,
    );
    let correctness = agent_step(
        "correctness-review",
        "Correctness review",
        "correctness-reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![current_candidate_input()],
        vec![assistant_output(), review_output()],
        Some(review_policy("review", "implementer", 3)),
    );
    let security = agent_step(
        "security-review",
        "Security review",
        "security-reviewer",
        CandidateAuthority::ReadOnly,
        review_tools(),
        vec![
            current_candidate_input(),
            input(
                "correctness-review",
                ArtefactKind::ReviewReport,
                "correctness-review",
                "review",
            ),
        ],
        vec![assistant_output(), review_output()],
        Some(review_policy("review", "implementer", 3)),
    );
    definition(
        "Correctness and security review",
        default_environment,
        roles,
        vec![
            implementer,
            correctness,
            security,
            commit_step_current(&[
                ("correctness-review", "correctness-review"),
                ("security-review", "security-review"),
            ]),
        ],
    )
}

fn current_candidate_input() -> RequiredInput {
    RequiredInput {
        key: InputKey::parse("candidate").expect("input"),
        kind: ArtefactKind::CandidateRevision,
        source: ArtefactSource::RunCurrentCandidate,
    }
}

#[cfg(test)]
fn review_policy(output: &str, revision: &str, limit: u8) -> ReviewPolicy {
    ReviewPolicy {
        report_output: OutputKey::parse(output).expect("output"),
        revision_target: StepKey::parse(revision).expect("revision"),
        attempt_limit: limit,
    }
}

#[cfg(test)]
fn commit_step_current(reviews: &[(&str, &str)]) -> StepDefinition {
    let mut inputs = vec![current_candidate_input()];
    inputs.extend(
        reviews
            .iter()
            .map(|(key, step)| input(key, ArtefactKind::ReviewReport, step, "review")),
    );
    StepDefinition {
        key: StepKey::parse("commit").expect("step"),
        name: "Commit".to_owned(),
        inputs,
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::CommitCandidate,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: vec![output("committed-candidate", OutputKind::CandidateRevision)],
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
        .expect("seed definition")
}

fn role(key: &str, name: &str, expertise: &str, prompt: &str) -> RoleDefinition {
    RoleDefinition::new(
        RoleKey::parse(key).expect("role"),
        name.to_owned(),
        expertise.to_owned(),
        prompt.to_owned(),
    )
    .expect("role definition")
}

#[allow(clippy::too_many_arguments)]
fn agent_step(
    key: &str,
    name: &str,
    role_key: &str,
    candidate_authority: CandidateAuthority,
    tools: Vec<ToolId>,
    inputs: Vec<RequiredInput>,
    outputs: Vec<RequiredOutput>,
    review: Option<ReviewPolicy>,
) -> StepDefinition {
    StepDefinition {
        key: StepKey::parse(key).expect("step"),
        name: name.to_owned(),
        inputs,
        action: StepAction::Agent(AgentStep {
            role: RoleKey::parse(role_key).expect("role"),
            environment: StepEnvironment::WorkflowDefault,
            candidate_authority,
            authority: AgentAuthority::new(tools, Vec::new()).expect("authority"),
            required_outputs: outputs,
            settings: super::definition::ModelStepSettings::SameAsRunDefaults,
        }),
        review,
    }
}

#[cfg(test)]
fn commit_step(candidate_step: &str, review_step: &str) -> StepDefinition {
    StepDefinition {
        key: StepKey::parse("commit").expect("step"),
        name: "Commit".to_owned(),
        inputs: vec![
            input(
                "candidate",
                ArtefactKind::CandidateRevision,
                candidate_step,
                "candidate",
            ),
            input("review", ArtefactKind::ReviewReport, review_step, "review"),
        ],
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::CommitCandidate,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: vec![output("committed-candidate", OutputKind::CandidateRevision)],
        }),
        review: None,
    }
}

fn plan_checkpoint_step(plan_step: &str, plan_output: &str) -> StepDefinition {
    StepDefinition {
        key: StepKey::parse("plan-acceptance").expect("plan checkpoint"),
        name: "Accept the plan".to_owned(),
        inputs: vec![input("plan", ArtefactKind::Plan, plan_step, plan_output)],
        action: StepAction::HumanGate(HumanGateStep {
            required_output: output("plan-decision", OutputKind::PlanDecision),
            revision: Some(HumanRevisionPolicy {
                revision_target: StepKey::parse(plan_step).expect("plan revision target"),
                attempt_limit: 3,
            }),
        }),
        review: None,
    }
}

fn human_approval_step(candidate_step: &str, review_step: Option<&str>) -> StepDefinition {
    let mut inputs = vec![input(
        "candidate",
        ArtefactKind::CandidateRevision,
        candidate_step,
        "candidate",
    )];
    if let Some(review_step) = review_step {
        inputs.push(input(
            "review",
            ArtefactKind::ReviewReport,
            review_step,
            "review",
        ));
    }
    StepDefinition {
        key: StepKey::parse("approval").expect("approval step"),
        name: "Approve changes".to_owned(),
        inputs,
        action: StepAction::HumanGate(HumanGateStep {
            required_output: output("decision", OutputKind::HumanDecision),
            revision: Some(HumanRevisionPolicy {
                revision_target: StepKey::parse(candidate_step).expect("revision target"),
                attempt_limit: 3,
            }),
        }),
        review: None,
    }
}

fn apply_with_approval(
    candidate_step: &str,
    review_step: Option<&str>,
    approval_step: &str,
) -> StepDefinition {
    let mut step = commit_with_approval(candidate_step, review_step, approval_step);
    step.key = StepKey::parse("apply").expect("step");
    step.name = "Apply changes".to_owned();
    if let StepAction::SystemCommand(action) = &mut step.action {
        action.command = SystemCommandId::ApplyChanges;
        action.required_outputs = vec![output("applied-candidate", OutputKind::CandidateRevision)];
    }
    step
}

fn commit_with_approval(
    candidate_step: &str,
    review_step: Option<&str>,
    approval_step: &str,
) -> StepDefinition {
    let mut inputs = vec![input(
        "candidate",
        ArtefactKind::CandidateRevision,
        candidate_step,
        "candidate",
    )];
    if let Some(review_step) = review_step {
        inputs.push(input(
            "review",
            ArtefactKind::ReviewReport,
            review_step,
            "review",
        ));
    }
    inputs.push(input(
        "decision",
        ArtefactKind::HumanDecision,
        approval_step,
        "decision",
    ));
    StepDefinition {
        key: StepKey::parse("commit").expect("step"),
        name: "Commit".to_owned(),
        inputs,
        action: StepAction::SystemCommand(SystemCommandStep {
            command: SystemCommandId::CommitCandidate,
            environment: StepEnvironment::WorkflowDefault,
            required_outputs: vec![output("committed-candidate", OutputKind::CandidateRevision)],
        }),
        review: None,
    }
}

fn input(key: &str, kind: ArtefactKind, step: &str, output_key: &str) -> RequiredInput {
    RequiredInput {
        key: InputKey::parse(key).expect("input"),
        kind,
        source: ArtefactSource::StepOutput {
            step: StepKey::parse(step).expect("source step"),
            output: OutputKey::parse(output_key).expect("source output"),
        },
    }
}

fn output(key: &str, kind: OutputKind) -> RequiredOutput {
    RequiredOutput {
        key: OutputKey::parse(key).expect("output"),
        kind,
    }
}

fn assistant_output() -> RequiredOutput {
    output(ASSISTANT_REPLY, OutputKind::AssistantReply)
}
fn review_output() -> RequiredOutput {
    output("review", OutputKind::ReviewReport)
}
fn review_tools() -> Vec<ToolId> {
    vec![ToolId::List, ToolId::Read, ToolId::Run]
}

#[cfg(test)]
mod tests;
