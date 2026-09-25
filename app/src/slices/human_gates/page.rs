use crate::workflows::{WorkflowRun, gates::HumanGateRecord};
use askama::Template;

pub(super) const TITLE: &str = "Plan review | Frinkworks";

#[derive(Template)]
#[template(path = "human_gates/templates/detail.html")]
pub(super) struct GatePage {
    pub(super) run_id: String,
    pub(super) gate_id: String,
    pub(super) gate_name: String,
    pub(super) plan: String,
    pub(super) text: String,
    pub(super) revision: u64,
    pub(super) awaiting: bool,
    pub(super) needs_recovery: bool,
    pub(super) can_request_revision: bool,
    pub(super) back_href: String,
}

impl GatePage {
    pub(super) fn new(run: &WorkflowRun, gate: &HumanGateRecord, text: String) -> Self {
        Self {
            run_id: run.id.as_hex(),
            gate_id: gate.id.as_hex(),
            gate_name: run
                .pinned
                .definition
                .step(&gate.step)
                .map(|step| step.name.clone())
                .unwrap_or_else(|| "Review the plan".to_owned()),
            plan: gate.candidate.artefact_hash.as_str(),
            text: crate::markdown::render(&text),
            revision: run.decision_revision(gate).get(),
            awaiting: gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision,
            needs_recovery: false,
            can_request_revision: run.human_revision_policy(&gate.step).is_some(),
            back_href: run
                .conversation_id
                .map(|id| format!("/conversations/{id}"))
                .unwrap_or_else(|| format!("/runs/{}", run.id)),
        }
    }
}
