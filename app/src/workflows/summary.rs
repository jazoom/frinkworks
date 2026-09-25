use super::commands::SystemCommandId;
use super::definition::{OutputKind, StepAccess, StepAction, StepDefinition, WorkflowDefinition};

pub(crate) fn required_inputs(_definition: &WorkflowDefinition) -> &'static str {
    "Brief · Conversation settings"
}

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
    Model(StepAccess),
    Command(SystemCommandId),
    PlanApproval,
    Invalid,
}

impl ProcessPhase {
    pub(crate) fn new(position: usize, name: String, action: ProcessAction) -> Self {
        let (kind, purpose, effects, context) = match action {
            ProcessAction::Model(access) => (
                "Model phase",
                "A model uses the authorised directories and produces the declared outputs."
                    .to_owned(),
                match access {
                    StepAccess::Read => "Read mounts prevent changes to the original files.",
                    StepAccess::Write => {
                        "Writes change original files immediately within the directory permissions."
                    }
                },
                "Each attempt receives the brief and declared artefacts. Earlier worker transcripts stay excluded.",
            ),
            ProcessAction::Command(command) => (
                "System action",
                format!("Frinkworks runs {}.", command.label()),
                command.consequence(),
                "No model request occurs.",
            ),
            ProcessAction::PlanApproval => (
                "Plan checkpoint",
                "A person accepts the exact plan or requests changes.".to_owned(),
                "The decision does not approve files or change directory permissions.",
                "The workflow pauses with the plan available for inspection.",
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
            approval: if matches!(action, ProcessAction::PlanApproval) {
                "The plan needs human acceptance."
            } else {
                "No plan approval stop."
            }
            .to_owned(),
            group: String::new(),
            group_start: false,
            group_detail: String::new(),
        }
    }

    pub(crate) fn annotate_direct(&mut self, paths: &str) {
        if !paths.is_empty() {
            self.effects = format!(
                "Write access changes original files immediately: {paths}. Failure or cancellation does not undo writes."
            );
        }
    }

    pub(crate) fn annotate_review(&mut self, independent_review: bool, review_and_fix: bool) {
        if independent_review {
            self.kind = "Independent review".to_owned();
            self.purpose =
                "A separate reviewer inspects the live files without changes.".to_owned();
        } else if review_and_fix {
            self.kind = "Review and fix".to_owned();
            self.purpose =
                "A reviewer inspects the live files and can change files with Write access."
                    .to_owned();
        }
    }
}

pub(crate) fn revision_summary(human: bool, target: &str, attempt_limit: &str) -> String {
    let prefix = if human {
        "Plan changes return to"
    } else {
        "A revision-required verdict returns to"
    };
    format!(
        "{prefix} {target}. The limit is {attempt_limit} attempts, with the first attempt included."
    )
}

pub(crate) fn process_overview(definition: &WorkflowDefinition) -> Vec<ProcessPhase> {
    definition
        .steps()
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let action = match &step.action {
                StepAction::Agent(action) => ProcessAction::Model(action.directory_access),
                StepAction::SystemCommand(action) => ProcessAction::Command(action.command),
                StepAction::HumanGate(_) => ProcessAction::PlanApproval,
            };
            let mut phase = ProcessPhase::new(index + 1, step.name.clone(), action);
            let review = step
                .required_outputs()
                .iter()
                .any(|output| output.kind == OutputKind::ReviewReport);
            if let StepAction::Agent(action) = &step.action {
                phase.annotate_review(
                    review
                        && action.directory_access == StepAccess::Read
                        && definition.steps()[..index]
                            .iter()
                            .any(StepDefinition::writes_primary_source),
                    review && action.directory_access == StepAccess::Write,
                );
                if action.directory_access == StepAccess::Write
                    && let super::definition::ModelStepSettings::Override(settings) =
                        &action.settings
                    && let Some(directories) = &settings.directories
                {
                    let paths = directories
                        .iter()
                        .filter(|grant| grant.access == crate::execution::DirectoryAccess::Write)
                        .map(|grant| grant.host_path.display().to_string())
                        .collect::<Vec<_>>();
                    phase.annotate_direct(&paths.join(", "));
                }
            }
            let route = match &step.action {
                StepAction::HumanGate(action) => action
                    .revision
                    .as_ref()
                    .map(|policy| (true, &policy.revision_target, policy.attempt_limit)),
                _ => step
                    .review
                    .as_ref()
                    .map(|policy| (false, &policy.revision_target, policy.attempt_limit)),
            };
            if let Some((human, target, limit)) = route {
                let target = definition
                    .step(target)
                    .map(|step| step.name.as_str())
                    .unwrap_or("the earlier phase");
                phase.approval = revision_summary(human, target, &limit.to_string());
            }
            phase
        })
        .collect()
}

pub(crate) fn process_summary(definition: &WorkflowDefinition) -> String {
    definition
        .steps()
        .iter()
        .map(|step| {
            let action = match &step.action {
                StepAction::Agent(_) => "model phase",
                StepAction::SystemCommand(_) => "system action",
                StepAction::HumanGate(_) => "plan checkpoint",
            };
            format!("{} ({action})", step.name)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn approval_stops(definition: &WorkflowDefinition) -> String {
    let stops = definition
        .steps()
        .iter()
        .filter(|step| matches!(step.action, StepAction::HumanGate(_)))
        .map(|step| step.name.as_str())
        .collect::<Vec<_>>();
    if stops.is_empty() {
        "No plan approval stop".to_owned()
    } else {
        format!("Plan acceptance at {}", stops.join(", "))
    }
}
