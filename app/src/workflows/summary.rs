pub(crate) fn required_inputs(_definition: &WorkflowDefinition) -> &'static str {
    "Brief · Conversation settings"
}
use super::commands::SystemCommandId;
use super::definition::{
    CandidateAuthority, OutputKind, StepAction, StepDefinition, WorkflowDefinition,
};

pub(crate) struct ProcessPhase {
    pub(crate) position: usize,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) purpose: String,
    pub(crate) effects: String,
    pub(crate) context: String,
    pub(crate) approval: String,
    pub(crate) group: String,
    pub(crate) group_start: bool,
    pub(crate) group_detail: String,
}

pub(crate) enum ProcessAction {
    Model(CandidateAuthority),
    Command(SystemCommandId),
    Approval,
    PlanApproval,
    Invalid,
}

impl ProcessPhase {
    pub(crate) fn new(position: usize, name: String, action: ProcessAction) -> Self {
        let (kind, purpose, effects, context) = match action {
            ProcessAction::Model(access) => (
                "Model phase",
                match access {
                    CandidateAuthority::ReadOnly => {
                        "A model inspects the candidate and produces the declared outputs."
                            .to_owned()
                    }
                    CandidateAuthority::Edit => {
                        "A model works on the candidate and proposes changes.".to_owned()
                    }
                },
                match access {
                    CandidateAuthority::ReadOnly => {
                        "Reads the candidate. Does not change project files."
                    }
                    CandidateAuthority::Edit => {
                        "Can edit an isolated candidate. Does not change the project yet."
                    }
                },
                "Each attempt starts with fresh model context: the brief, declared artefacts and authorised root project instructions. Conversation and earlier worker transcripts stay excluded.",
            ),
            ProcessAction::Command(command) => (
                "System action",
                format!("Frinkworks runs {}.", command.label()),
                match command {
                    SystemCommandId::RepositoryStatus => {
                        "Reads repository status. Does not change project files."
                    }
                    SystemCommandId::ApplyChanges | SystemCommandId::CommitCandidate => {
                        command.consequence()
                    }
                },
                "The current candidate and validated artefacts continue to the next phase. No model request occurs.",
            ),
            ProcessAction::Approval => (
                "Approval stop",
                "A person reviews the exact candidate and records a decision.".to_owned(),
                "Changes no project files by itself.",
                "A safe pause. The candidate remains available for inspection.",
            ),
            ProcessAction::PlanApproval => (
                "Plan checkpoint",
                "A person reviews the exact plan and records acceptance or requested changes."
                    .to_owned(),
                "Changes no project files and does not approve code.",
                "A safe pause. The exact plan remains available for inspection.",
            ),
            ProcessAction::Invalid => (
                "Incomplete phase",
                "This phase needs a valid action.".to_owned(),
                "Unknown until the action is valid.",
                "Unknown until the action is valid.",
            ),
        };
        Self {
            position,
            name,
            kind: kind.to_owned(),
            purpose,
            effects: effects.to_owned(),
            context: context.to_owned(),
            approval: if matches!(action, ProcessAction::Approval) {
                "Approval stop. The exact candidate needs a human decision."
            } else if matches!(action, ProcessAction::PlanApproval) {
                "Plan checkpoint. The exact plan needs human acceptance."
            } else {
                "No approval stop."
            }
            .to_owned(),
            group: String::new(),
            group_start: false,
            group_detail: String::new(),
        }
    }

    pub(crate) fn annotate_direct(&mut self, paths: &str) {
        if paths.is_empty() {
            return;
        }
        self.purpose =
            "A model works in authorised directories and produces the declared outputs.".to_owned();
        self.effects = format!(
            "Immediate host writes: {paths}. Candidate approval covers reviewed roots only. Direct changes remain after failure, discard or cancellation."
        );
    }

    pub(crate) fn annotate_review(&mut self, independent_review: bool, review_and_fix: bool) {
        if independent_review {
            self.kind = "Independent review".to_owned();
            self.purpose = "A separate reviewer inspects this exact candidate. It does not change the candidate.".to_owned();
            self.effects = "Reads the candidate. Does not change project files.".to_owned();
        } else if review_and_fix {
            self.kind = "Review and fix".to_owned();
            self.purpose = "A reviewer inspects the candidate and can fix safe issues.".to_owned();
            self.effects =
                "Can edit an isolated candidate. Does not change the project yet.".to_owned();
        }
    }
}

pub(crate) fn revision_summary(human: bool, target: &str, attempt_limit: &str) -> String {
    let prefix = if human {
        "Approval stop. Request changes returns to"
    } else {
        "Review route. A changes-requested verdict returns to"
    };
    format!("{prefix} {target}. Maximum {attempt_limit} attempts, including the first attempt.")
}

pub(crate) fn process_overview(definition: &WorkflowDefinition) -> Vec<ProcessPhase> {
    let phases: Vec<_> = definition
        .steps()
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let action = match &step.action {
                StepAction::Agent(agent) => ProcessAction::Model(agent.candidate_authority),
                StepAction::SystemCommand(command) => ProcessAction::Command(command.command),
                StepAction::HumanGate(action) if action.is_plan_checkpoint() => {
                    ProcessAction::PlanApproval
                }
                StepAction::HumanGate(_) => ProcessAction::Approval,
            };
            let mut phase = ProcessPhase::new(index + 1, step.name.clone(), action);
            phase.annotate_review(
                is_independent_review(definition.steps(), index),
                is_review_and_fix(step),
            );
            phase.approval = match &step.action {
                StepAction::HumanGate(action) if action.is_plan_checkpoint() => {
                    match &action.revision {
                        Some(policy) => format_plan_revision(
                            definition,
                            &policy.revision_target,
                            policy.attempt_limit,
                        ),
                        None => {
                            "Plan checkpoint. The exact plan needs human acceptance.".to_owned()
                        }
                    }
                }
                StepAction::HumanGate(action) => match &action.revision {
                    Some(policy) => format_revision(
                        true,
                        definition,
                        &policy.revision_target,
                        policy.attempt_limit,
                    ),
                    None => "Approval stop. The exact candidate needs a human decision.".to_owned(),
                },
                _ => match &step.review {
                    Some(policy) => format_revision(
                        false,
                        definition,
                        &policy.revision_target,
                        policy.attempt_limit,
                    ),
                    None => "No approval stop.".to_owned(),
                },
            };
            if let StepAction::Agent(action) = &step.action
                && let super::definition::ModelStepSettings::Override(settings) = &action.settings
                && let Some(directories) = &settings.directories
            {
                let direct: Vec<_> = directories
                    .iter()
                    .filter(|grant| grant.access == crate::execution::DirectoryAccess::DirectWrite)
                    .map(|grant| grant.host_path.display().to_string())
                    .collect();
                phase.annotate_direct(&direct.join(", "));
            }
            phase
        })
        .collect();

    phases
}

fn produces_review(step: &StepDefinition) -> bool {
    step.required_outputs()
        .iter()
        .any(|output| output.kind == OutputKind::ReviewReport)
}

fn is_independent_review(steps: &[StepDefinition], index: usize) -> bool {
    let Some(step) = steps.get(index) else {
        return false;
    };
    let StepAction::Agent(agent) = &step.action else {
        return false;
    };
    agent.candidate_authority == CandidateAuthority::ReadOnly
        && produces_review(step)
        && steps[..index]
            .iter()
            .any(StepDefinition::writes_primary_source)
}

fn is_review_and_fix(step: &StepDefinition) -> bool {
    let StepAction::Agent(agent) = &step.action else {
        return false;
    };
    agent.candidate_authority == CandidateAuthority::Edit && produces_review(step)
}

fn format_plan_revision(
    definition: &WorkflowDefinition,
    target: &super::definition::StepKey,
    attempt_limit: u8,
) -> String {
    let target = definition
        .step(target)
        .map(|step| step.name.as_str())
        .unwrap_or("the earlier planning phase");
    format!(
        "Plan checkpoint. Request changes returns to {target}. Maximum {attempt_limit} attempts, including the first attempt."
    )
}

fn format_revision(
    human: bool,
    definition: &WorkflowDefinition,
    target: &super::definition::StepKey,
    attempt_limit: u8,
) -> String {
    let target = definition
        .step(target)
        .map(|step| step.name.as_str())
        .unwrap_or("the earlier implementation");
    revision_summary(human, target, &attempt_limit.to_string())
}

pub(crate) fn process_summary(definition: &WorkflowDefinition) -> String {
    definition
        .steps()
        .iter()
        .map(|step| {
            let action = match &step.action {
                StepAction::Agent(_) => "model phase",
                StepAction::SystemCommand(_) => "system action",
                StepAction::HumanGate(action) if action.is_plan_checkpoint() => "plan checkpoint",
                StepAction::HumanGate(_) => "human approval",
            };
            format!("{} ({action})", step.name)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn approval_stops(definition: &WorkflowDefinition) -> String {
    let stops: Vec<_> = definition
        .steps()
        .iter()
        .filter_map(|step| {
            let StepAction::HumanGate(action) = &step.action else {
                return None;
            };
            let target = action.revision.as_ref().map(|policy| {
                definition
                    .step(&policy.revision_target)
                    .map(|target| target.name.as_str())
                    .unwrap_or("the earlier phase")
            });
            Some((
                action.is_plan_checkpoint(),
                target
                    .map(|target| format!("{} with request changes to {target}", step.name))
                    .unwrap_or_else(|| step.name.to_owned()),
            ))
        })
        .collect();
    if !stops.is_empty() {
        let plan: Vec<_> = stops
            .iter()
            .filter(|(is_plan, _)| *is_plan)
            .map(|(_, stop)| stop.clone())
            .collect();
        let code: Vec<_> = stops
            .iter()
            .filter(|(is_plan, _)| !*is_plan)
            .map(|(_, stop)| stop.clone())
            .collect();
        match (plan.is_empty(), code.is_empty()) {
            (false, false) => format!(
                "Plan acceptance at {}; human approval at {}",
                plan.join(", "),
                code.join(", ")
            ),
            (false, true) => format!("Plan acceptance at {}", plan.join(", ")),
            (true, false) => format!("Human approval at {}", code.join(", ")),
            (true, true) => unreachable!(),
        }
    } else {
        match definition.commit_policy() {
            crate::workflows::definition::CommitPolicy::AutomaticAfterReview => {
                "Automatic commit after approved review".to_owned()
            }
            _ => "No approval stop".to_owned(),
        }
    }
}
