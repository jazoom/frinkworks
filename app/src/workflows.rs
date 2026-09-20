#[cfg(test)]
pub(super) mod tests;

pub(crate) mod apply;
pub(crate) mod artefacts;
pub(crate) mod capabilities;
mod catalogue;
pub(crate) mod commands;
mod commit;
pub(crate) use commit::CommitTransactionState;
pub(crate) mod definition;
pub(crate) mod direct;
pub(crate) mod evidence;
mod execution;
mod executor;
pub(crate) mod gates;
pub(crate) mod handoff;
mod id;
pub(crate) mod input_context;
mod quick;
mod resolve;
pub(crate) mod run;

pub(crate) mod seeds;
mod store;
pub(crate) mod summary;
pub(crate) mod workspace;

pub(crate) use apply::ApplyJournals;
pub(crate) use artefacts::WorkflowArtefactRepository;
pub(crate) use catalogue::{
    CatalogueError, ResolveWorkflowError, WorkflowCatalogue, WorkflowRecord, WorkflowSelection,
};
pub(crate) use commit::CommitJournals;
pub(crate) use evidence::{AttemptEvidenceContext, WorkflowEvidenceStore};
pub(crate) use execution::{ExecutionGuard, WorkflowExecution};
pub(crate) use executor::{
    WorkflowContinuationRegistry, WorkflowJob, execute_run, interrupt_provider_continuations,
    interrupt_session_continuations, recover_apply_transactions, recover_commit_transactions,
    settle_cancelled_job, settle_terminal_job, validate_phase_selection,
};
pub(crate) use id::{ArtefactId, AttemptId, GateId, RunId, WorkflowId};
pub(crate) use quick::{HOST_UNCHANGED, alpine_git_id, pin_agent_work};
#[cfg(test)]
pub(crate) use quick::{pin_project_free_quick_task_with_directories, tests::pin_quick_task};
pub(crate) use resolve::{
    preview_environments, resolve_environments, validate_replacement_environment,
};
pub(crate) use run::{PhaseModelSelection, PinnedPreset, RunKind, RunSource, WorkflowRun, now_ms};
pub(crate) use store::{RunSummary, StoreError, WorkflowRunStore};
