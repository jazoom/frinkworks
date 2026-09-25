use crate::{
    execution::{DirectoryAccess, ExecutionSettings, ToolLocation},
    state::AppState,
};

use super::{
    AttemptId, WorkflowRun,
    artefacts::{CandidateCapture, CandidateDiff, CandidatePayload, ObjectHash},
    definition::StepDefinition,
    executor::WorkflowJob,
    run::TransitionError,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectChanges {
    before: String,
    after: Option<String>,
}

impl DirectChanges {
    pub(crate) fn valid(&self) -> bool {
        ObjectHash::parse(&self.before).is_some()
            && self
                .after
                .as_ref()
                .is_none_or(|hash| ObjectHash::parse(hash).is_some())
    }

    pub(crate) fn diff(&self, state: &AppState) -> Option<CandidateDiff> {
        let load = |hash: &str| {
            let bytes = state
                .workflow_artefacts
                .get(&ObjectHash::parse(hash)?)
                .ok()?;
            CandidatePayload::from_manifest_bytes(&bytes)
        };
        CandidateDiff::from_payloads(load(&self.before)?, load(self.after.as_ref()?)?).ok()
    }
}

fn directories(settings: &ExecutionSettings) -> Vec<crate::execution::DirectoryGrant> {
    settings
        .directories
        .iter()
        .filter(|grant| {
            settings.location == ToolLocation::Host || grant.access == DirectoryAccess::DirectWrite
        })
        .cloned()
        .collect()
}

fn capture(
    state: &AppState,
    run: &WorkflowRun,
    step: &StepDefinition,
) -> Result<Option<String>, &'static str> {
    let Some(settings) = run.phase_settings(&step.key) else {
        return Ok(None);
    };
    let directories = directories(settings);
    if directories.is_empty() {
        return Ok(None);
    }
    let candidate = CandidateCapture::capture_set(
        &directories,
        state.local_data.root(),
        &state.workflow_artefacts,
    )
    .map_err(|_| "Frinkworks cannot capture the named directories for direct-write evidence.")?;
    let bytes = candidate
        .manifest_bytes()
        .map_err(|_| "Frinkworks cannot encode the direct-write evidence.")?;
    state
        .workflow_artefacts
        .publish(&bytes)
        .map(|hash| Some(hash.as_str()))
        .map_err(|_| "Frinkworks cannot store the direct-write evidence.")
}

pub(super) fn before(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt: AttemptId,
) -> Result<(), &'static str> {
    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or("The run is unavailable.")?;
    // A continuation retains the original baseline, not the host state after its earlier writes.
    if run.attempts.iter().any(|record| {
        record.id == attempt && record.continuation.is_some() && record.direct_changes.is_some()
    }) {
        return state
            .workflow_runs
            .mutate(&job.run_id, |run| {
                let changes = run
                    .attempts
                    .iter_mut()
                    .find(|record| record.id == attempt)
                    .and_then(|record| record.direct_changes.as_mut())
                    .ok_or(TransitionError::Invalid)?;
                changes.after = None;
                Ok(())
            })
            .map(|_| ())
            .map_err(|_| "Frinkworks cannot retain the direct-write baseline.");
    }
    let Some(before) = capture(state, &run, step)? else {
        return Ok(());
    };
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            let record = run
                .attempts
                .iter_mut()
                .find(|record| record.id == attempt && record.direct_changes.is_none())
                .ok_or(TransitionError::Invalid)?;
            record.direct_changes = Some(DirectChanges {
                before,
                after: None,
            });
            Ok(())
        })
        .map(|_| ())
        .map_err(|_| "Frinkworks cannot store the direct-write baseline.")
}

pub(super) fn after(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt: AttemptId,
) -> Result<(), &'static str> {
    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or("The run is unavailable.")?;
    let Some(original) = run
        .attempts
        .iter()
        .find(|record| record.id == attempt)
        .and_then(|record| record.direct_changes.as_ref())
    else {
        return Ok(());
    };
    if original.after.is_some() {
        return Err("The final direct-write snapshot is already recorded.");
    }
    let after =
        capture(state, &run, step)?.ok_or("The direct-write directories are unavailable.")?;
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            let record = run
                .attempts
                .iter_mut()
                .find(|record| {
                    record.id == attempt && record.direct_changes.as_ref() == Some(original)
                })
                .ok_or(TransitionError::Invalid)?;
            record
                .direct_changes
                .as_mut()
                .ok_or(TransitionError::Invalid)?
                .after = Some(after);
            Ok(())
        })
        .map(|_| ())
        .map_err(|_| "Frinkworks cannot store the observed direct writes.")
}

#[cfg(test)]
mod tests;
