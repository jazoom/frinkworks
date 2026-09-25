#[cfg(test)]
mod tests;

use super::*;
use crate::workflows::commit::{
    CommitError, CommitRoot, CommitTransaction, CommitTransactionState,
};

pub(super) async fn isolate(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::super::run::AttemptArtefactInput],
    capabilities: &super::super::capabilities::AttemptCapabilities,
    target: &LoadedCandidate,
) -> IsolatedRun {
    let drafts = Arc::new(std::sync::Mutex::new(
        crate::workflows::artefacts::output::OutputDrafts::default(),
    ));
    let transaction = match prepare(state, job, step, attempt_id, inputs, target) {
        Ok(transaction) => transaction,
        Err(error) => {
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: error.category(),
                    error: Some(error.message().to_owned()),
                },
                cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
                drafts,
                captured: None,
            };
        }
    };
    if transaction.roots.is_empty() {
        return IsolatedRun::Finished {
            outcome: StepOutcome::Completed,
            cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
            drafts,
            captured: Some(target.artefact.clone()),
        };
    }
    let workspace = match state
        .workflow_workspaces
        .create_attempt(job.run_id, attempt_id)
    {
        Ok(workspace) => workspace,
        Err(error) => {
            return IsolatedRun::Finished {
                outcome: StepOutcome::Failed {
                    category: FailureCategory::Operational,
                    error: Some("Frinkworks cannot create the commit workspace.".to_owned()),
                },
                cleanup: if error.orphaned {
                    crate::workflows::run::AttemptCleanupRecord::Orphaned {
                        sandbox: false,
                        workspace: true,
                        journal: true,
                    }
                } else {
                    crate::workflows::run::AttemptCleanupRecord::Complete
                },
                drafts,
                captured: None,
            };
        }
    };
    let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt_id);
    if let Err(error) =
        start_attempt_sandbox(state, job, step, capabilities, &workspace, sandbox.clone()).await
    {
        let outcome = StepOutcome::Failed {
            category: FailureCategory::Operational,
            error: Some(error.to_owned()),
        };
        let (outcome, cleanup) =
            cleanup_after_start_failure(state, attempt_id, sandbox, workspace, outcome).await;
        return IsolatedRun::Finished {
            outcome,
            cleanup,
            drafts,
            captured: None,
        };
    }
    let mut outcome = StepOutcome::Completed;
    for root in transaction.roots {
        if let Err(error) =
            execute_commit_root(state, job, attempt_id, &root, target, &sandbox).await
        {
            outcome = if job.job.cancel_requested() {
                StepOutcome::Cancelled
            } else {
                StepOutcome::Failed {
                    category: error.category(),
                    error: Some(format!(
                        "{}: {} Earlier repository commits remain.",
                        root.grant.host_path.display(),
                        error.message()
                    )),
                }
            };
            break;
        }
    }
    let stopped = sandbox.stop().await.is_ok();
    let captured = if stopped {
        state
            .workflow_runs
            .get(&job.run_id)
            .and_then(|run| capture_commit_candidate(state, &run, &target.artefact).ok())
    } else {
        None
    };
    if matches!(outcome, StepOutcome::Completed) && captured.is_none() {
        outcome = StepOutcome::Failed {
            category: FailureCategory::Commit,
            error: Some("Frinkworks cannot capture the committed files.".to_owned()),
        };
    }
    let sandbox_gone = stopped && sandbox.remove().await.is_ok();
    if sandbox_gone {
        state.sandboxes.drop_attempt(attempt_id);
    } else {
        state.sandboxes.expose_orphan(sandbox.name().to_owned());
    }
    let workspace_gone = sandbox_gone && workspace.destroy().is_ok();
    let cleanup = if workspace_gone {
        crate::workflows::run::AttemptCleanupRecord::Complete
    } else {
        crate::workflows::run::AttemptCleanupRecord::Orphaned {
            sandbox: !sandbox_gone,
            workspace: !workspace_gone,
            journal: true,
        }
    };
    IsolatedRun::Finished {
        outcome: fail_for_orphan(outcome, &cleanup),
        cleanup,
        drafts,
        captured,
    }
}

fn prepare(
    state: &AppState,
    job: &WorkflowJob,
    step: &StepDefinition,
    attempt_id: AttemptId,
    inputs: &[super::super::run::AttemptArtefactInput],
    target: &LoadedCandidate,
) -> Result<CommitTransaction, CommitError> {
    let run = state
        .workflow_runs
        .get(&job.run_id)
        .ok_or(CommitError::Operational)?;
    confirm_run_authority(state, job).map_err(|_| CommitError::Authority)?;
    let (_, _, approved) = crate::workflows::commit::require_commit_approval(
        &run,
        step,
        inputs,
        &state.workflow_artefacts,
    )?;
    if approved != target.artefact {
        return Err(CommitError::Assurance);
    }
    let crate::workflows::RunSource::Captured { source } = &run.source else {
        return Err(CommitError::Assurance);
    };
    let initial = load_candidate_payload_reference(state, &run, &source.initial)
        .map_err(|_| CommitError::Operational)?;
    let targets = crate::workflows::commit::commit_targets(&run, &initial, &target.artefact)?;
    if capture_commit_candidate(state, &run, &initial).map_err(|_| CommitError::Preflight)?
        != initial
    {
        return Err(CommitError::Preflight);
    }
    let candidate = inputs
        .iter()
        .find(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
        .ok_or(CommitError::Assurance)?
        .artefact
        .clone();
    let mut transaction = CommitTransaction {
        candidate,
        reviews: inputs
            .iter()
            .filter(|input| {
                input.artefact.kind == crate::workflows::definition::ArtefactKind::ReviewReport
            })
            .map(|input| input.artefact.clone())
            .collect(),
        approval: inputs
            .iter()
            .find(|input| {
                input.artefact.kind == crate::workflows::definition::ArtefactKind::HumanDecision
            })
            .map(|input| input.artefact.clone()),
        roots: Vec::new(),
    };
    // Validate every destination before the first host write. A later conflict still cannot undo earlier commits.
    for grant in targets {
        let (before, after) =
            crate::workflows::commit::destination_pair(&grant, &initial, &target.artefact)?;
        crate::workflows::commit::require_unchanged_project(
            &grant.host_path,
            before,
            after,
            &state.workflow_artefacts,
        )?;
        require_clean_repository(&grant.host_path)?;
        let reference = current_reference(&grant.host_path)?;
        let old = before
            .repository
            .as_ref()
            .ok_or(CommitError::Repository)?
            .head
            .as_ref()
            .map(|head| head.0.clone());
        transaction.roots.push(CommitRoot {
            grant,
            state: CommitTransactionState::Prepared,
            expected_reference: reference,
            old_object: old,
            target_tree: None,
            expected_commit: None,
            timestamp: crate::workflows::commit::utc_timestamp(now_ms()),
        });
    }
    persist_transaction(state, job.run_id, attempt_id, transaction.clone())?;
    Ok(transaction)
}

pub(super) fn recover(state: &AppState) -> Result<(), &'static str> {
    const ERROR: &str = "Frinkworks cannot recover a repository commit. Earlier commits remain.";
    for run in state.workflow_runs.active_runs() {
        let Some(attempt_id) = run.active_attempt() else {
            continue;
        };
        let attempt = run
            .attempts
            .iter()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or(ERROR)?;
        let Some(transaction) = &attempt.commit_transaction else {
            continue;
        };
        let crate::workflows::RunSource::Captured { source } = &run.source else {
            return Err(ERROR);
        };
        let initial = load_candidate_payload_reference(state, &run, &source.initial)?;
        let target = load_candidate_payload_reference(state, &run, &transaction.candidate)?;
        let targets =
            crate::workflows::commit::commit_targets(&run, &initial, &target).map_err(|_| ERROR)?;
        if targets.len() != transaction.roots.len()
            || targets
                .iter()
                .zip(&transaction.roots)
                .any(|(grant, root)| *grant != root.grant)
        {
            return Err(ERROR);
        }
        for root in &transaction.roots {
            let recovered = recover_root(state, &run, attempt_id, root, &initial, &target)
                .map_err(|error| error.message())?;
            persist_commit_root(state, run.id, attempt_id, recovered)
                .map_err(|_| "Frinkworks cannot record the recovered repository.")?;
            let temporary = root
                .grant
                .host_path
                .join(".git")
                .join(format!("frinkworks-commit-index-{}", attempt_id.as_hex()));
            match std::fs::remove_file(temporary) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(ERROR),
            }
        }
        let run = state.workflow_runs.get(&run.id).ok_or(ERROR)?;
        let transaction = run
            .attempts
            .iter()
            .find(|attempt| attempt.id == attempt_id)
            .and_then(|attempt| attempt.commit_transaction.as_ref())
            .ok_or(ERROR)?;
        if transaction.needs_recovery() {
            return Err(ERROR);
        }
        if transaction.is_verified() {
            let captured = capture_commit_candidate(state, &run, &target)?;
            if captured.candidate_hash() != target.candidate_hash() {
                return Err("The recovered files differ from the approved candidate.");
            }
            publish_recovered_commit(state, &run, attempt_id, &captured)?;
            remove_commit_journal(state, run.id, attempt_id)?;
            state
                .workflow_runs
                .mutate(&run.id, |run| {
                    run.record_cleanup(
                        attempt_id,
                        crate::workflows::run::AttemptCleanupRecord::Complete,
                    )?;
                    run.complete_attempt(attempt_id, now_ms())
                })
                .map_err(|_| "Frinkworks cannot complete the recovered commit step.")?;
        } else {
            remove_commit_journal(state, run.id, attempt_id)?;
        }
    }
    Ok(())
}

fn recover_root(
    state: &AppState,
    run: &crate::workflows::WorkflowRun,
    attempt_id: AttemptId,
    root: &CommitRoot,
    initial_payload: &crate::workflows::artefacts::CandidatePayload,
    target_payload: &crate::workflows::artefacts::CandidatePayload,
) -> Result<CommitRoot, CommitError> {
    let grant = crate::workflows::commit::target_grant(run, root.grant.id)?;
    if grant != root.grant {
        return Err(CommitError::Authority);
    }
    let mut recovered = root.clone();
    if matches!(root.state, CommitTransactionState::Restored) {
        return Ok(recovered);
    }
    // No host file or reference write precedes the durable expected commit.
    if matches!(root.state, CommitTransactionState::Prepared) && root.expected_commit.is_none() {
        recovered.state = CommitTransactionState::Restored;
        return Ok(recovered);
    }
    let (initial, target) =
        crate::workflows::commit::destination_pair(&grant, initial_payload, target_payload)?;
    let project = &grant.host_path;
    if current_reference(project)? != root.expected_reference {
        return Err(CommitError::Preflight);
    }
    let head = current_head(project).map_err(|_| CommitError::Preflight)?;
    let live =
        crate::workflows::commit::capture_revision(project, initial, &state.workflow_artefacts)?;
    if head == root.old_object && root.verified_commit().is_none() {
        if live == *initial {
            recovered.state = CommitTransactionState::Restored;
            return Ok(recovered);
        }
        if live.candidate_hash != target.candidate_hash
            || live.repository != initial.repository
            || live.git_admin != initial.git_admin
        {
            return Err(CommitError::Preflight);
        }
        let journal = state
            .commit_journals
            .load_for_directory(run.id, attempt_id, grant.id)
            .map_err(|_| CommitError::Operational)?;
        restore_before_reference(state, project, initial, target, &journal)?;
        recovered.state = CommitTransactionState::Restored;
        return Ok(recovered);
    }
    let commit = root
        .expected_commit
        .as_ref()
        .ok_or(CommitError::Preflight)?;
    if head.as_ref() != Some(commit) || live.candidate_hash != target.candidate_hash {
        return Err(CommitError::Preflight);
    }
    if root.verified_commit().is_none() {
        let journal = state
            .commit_journals
            .load_for_directory(run.id, attempt_id, grant.id)
            .map_err(|_| CommitError::Operational)?;
        let original = journal
            .read_index_backup("original.index")
            .map_err(|_| CommitError::Operational)?;
        let target_index = journal
            .read_index_backup("target.index")
            .map_err(|_| CommitError::Operational)?;
        let index = project.join(".git/index");
        let live_index = match std::fs::read(&index) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err(CommitError::Operational),
        };
        if live_index != original && live_index != target_index {
            return Err(CommitError::Preflight);
        }
        crate::storage::write_private(&index, &target_index)
            .map_err(|_| CommitError::Operational)?;
    }
    recovered.state = CommitTransactionState::Verified {
        commit: commit.clone(),
    };
    Ok(recovered)
}

pub(super) fn require_clean_repository(project: &std::path::Path) -> Result<(), CommitError> {
    if !git_host_text(
        project,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err(CommitError::Dirty);
    }
    Ok(())
}
