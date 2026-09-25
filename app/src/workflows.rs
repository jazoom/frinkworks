#[cfg(test)]
pub(super) mod tests;

pub(crate) mod artefacts;
pub(crate) mod capabilities;
mod catalogue;
pub(crate) mod commands;
pub(crate) mod definition;
pub(crate) mod evidence;
mod execution;
mod executor;
pub(crate) mod gates;
pub(crate) mod handoff;
mod id;
pub(crate) mod input_context;
mod resolve;
pub(crate) mod run;

pub(crate) mod seeds;
mod store;
pub(crate) mod summary;
pub(crate) mod workspace;

pub(crate) use artefacts::WorkflowArtefactRepository;
pub(crate) use catalogue::{
    CatalogueError, ResolveWorkflowError, WorkflowCatalogue, WorkflowRecord, WorkflowSelection,
};
pub(crate) use evidence::{AttemptEvidenceContext, WorkflowEvidenceStore};
pub(crate) use execution::{ExecutionGuard, WorkflowExecution};
pub(crate) use executor::{
    WorkflowContinuationRegistry, WorkflowJob, execute_run, interrupt_provider_continuations,
    interrupt_session_continuations, settle_cancelled_job, settle_terminal_job,
    validate_phase_selection,
};
pub(crate) use id::{ArtefactId, AttemptId, GateId, RunId, WorkflowId};
pub(crate) use resolve::{
    alpine_git_id, preview_environments, resolve_environments, validate_replacement_environment,
};
#[cfg(test)]
pub(crate) use run::RunKind;
pub(crate) use run::{PhaseModelSelection, PinnedPreset, WorkflowRun, now_ms};
pub(crate) use store::{RunSummary, StoreError, WorkflowRunStore};
