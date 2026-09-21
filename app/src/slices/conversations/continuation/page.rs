use crate::conversations::ContinuationCheckpoint;

pub(crate) struct ContinuationView {
    pub(crate) checkpoint: String,
    pub(crate) reason: String,
    pub(crate) model_requests: String,
    pub(crate) tool_dispatches: String,
    pub(crate) elapsed: String,
    pub(crate) run: String,
    pub(crate) attempt: String,
    pub(crate) step: String,
    pub(crate) needs_run_consent: bool,
    pub(crate) settings_summary: String,
}

impl ContinuationView {
    pub(crate) fn from_checkpoint(checkpoint: &ContinuationCheckpoint) -> Self {
        Self {
            checkpoint: checkpoint.id.as_hex(),
            reason: format!("The {} paused this work.", checkpoint.budget.reason.label()),
            model_requests: format!(
                "{} of {}",
                checkpoint.budget.model_requests, checkpoint.budget.model_request_limit
            ),
            tool_dispatches: format!(
                "{} of {}",
                checkpoint.budget.tool_dispatches, checkpoint.budget.tool_dispatch_limit
            ),
            elapsed: format!(
                "{} of {} minutes",
                checkpoint.budget.elapsed_ms / 60_000,
                (checkpoint.budget.elapsed_limit_ms / 60_000).max(1)
            ),
            run: checkpoint.run.map(|id| id.as_hex()).unwrap_or_default(),
            attempt: checkpoint.attempt.map(|id| id.as_hex()).unwrap_or_default(),
            step: checkpoint.step.clone().unwrap_or_default(),
            needs_run_consent: checkpoint.run.is_some(),
            settings_summary: String::new(),
        }
    }
}
