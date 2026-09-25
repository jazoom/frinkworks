#[cfg(test)]
mod tests;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::execution::{CanonicalDirectoryIdentity, DirectoryGrantId};
use crate::storage::{self, PersistError};

use super::artefacts::{
    ArtefactHash, ArtefactReference, CandidateHash, ObjectHash, TypedPayload,
    WorkflowArtefactRepository, artefact_hash_for, parse_typed_payload,
};
use super::definition::{ArtefactKind, StepDefinition, SystemCommandId};
use super::id::{AttemptId, RunId};
use super::run::{AttemptArtefactInput, WorkflowRun};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplyTransactionState {
    Prepared,
    Applying { completed: usize, path: String },
    Applied { completed: usize },
    Verified,
    Recovered,
    RecoveryUncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApplyRoot {
    pub(crate) grant_id: DirectoryGrantId,
    pub(crate) alias: String,
    pub(crate) host_path: PathBuf,
    pub(crate) identity: CanonicalDirectoryIdentity,
    pub(crate) baseline_candidate: CandidateHash,
    pub(crate) candidate_hash: CandidateHash,
    pub(crate) exclusions: Vec<String>,
    pub(crate) outcome: ApplyRootOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApplyRootOutcome {
    Pending,
    Unchanged,
    Applied,
    Conflicted,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApplyTransaction {
    pub(crate) state: ApplyTransactionState,
    pub(crate) roots: Vec<ApplyRoot>,
    pub(crate) baseline: ArtefactReference,
    pub(crate) candidate: ArtefactReference,
    pub(crate) approval: ArtefactReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApprovedApplication {
    pub(crate) baseline_reference: ArtefactReference,
    pub(crate) baseline: super::artefacts::candidate::CandidatePayload,
    pub(crate) candidate_reference: ArtefactReference,
    pub(crate) candidate: super::artefacts::candidate::CandidatePayload,
    pub(crate) approval: ArtefactReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApplyExecutionError {
    Assurance,
    Authority,
    Conflict,
    Integrity,
    Write,
    Operational,
}

impl ApplyExecutionError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Assurance => "That approval does not match these prepared changes.",
            Self::Authority => "Directory access changed before file application.",
            Self::Conflict => "The host directory changed before file application.",
            Self::Integrity => "The stored candidate failed an integrity check.",
            Self::Write => "Frinkworks could not apply the prepared files.",
            Self::Operational => "Frinkworks could not store the file application transaction.",
        }
    }
}

pub(crate) fn require_approval(
    run: &WorkflowRun,
    step: &StepDefinition,
    inputs: &[AttemptArtefactInput],
    store: &WorkflowArtefactRepository,
) -> Result<ApprovedApplication, ApplyExecutionError> {
    let candidate = inputs
        .iter()
        .find(|input| input.artefact.kind == ArtefactKind::CandidateRevision)
        .ok_or(ApplyExecutionError::Assurance)?;
    let super::run::RunSource::Captured { source } = &run.source else {
        return Err(ApplyExecutionError::Assurance);
    };
    if source.accepted != candidate.artefact
        || source.observed
            != (super::run::ObservedCandidate::Exact {
                artefact: candidate.artefact.clone(),
            })
    {
        return Err(ApplyExecutionError::Assurance);
    }
    require_bound_approval(run, step, inputs, store)
}

// Recovery can follow output publication, which advances the accepted source reference.
pub(crate) fn require_bound_approval(
    run: &WorkflowRun,
    step: &StepDefinition,
    inputs: &[AttemptArtefactInput],
    store: &WorkflowArtefactRepository,
) -> Result<ApprovedApplication, ApplyExecutionError> {
    let super::definition::StepAction::SystemCommand(action) = &step.action else {
        return Err(ApplyExecutionError::Assurance);
    };
    let input_kinds: Vec<_> = inputs.iter().map(|input| input.artefact.kind).collect();
    let output_kinds: Vec<_> = action
        .required_outputs
        .iter()
        .map(|output| output.kind)
        .collect();
    if action.command != SystemCommandId::ApplyChanges
        || !action
            .command
            .contract()
            .accepts(&input_kinds, &output_kinds)
    {
        return Err(ApplyExecutionError::Assurance);
    }
    let candidate_input = inputs
        .iter()
        .find(|input| input.artefact.kind == ArtefactKind::CandidateRevision)
        .ok_or(ApplyExecutionError::Assurance)?;
    let approval_input = inputs
        .iter()
        .find(|input| input.artefact.kind == ArtefactKind::HumanDecision)
        .ok_or(ApplyExecutionError::Assurance)?;
    let super::run::RunSource::Captured { source } = &run.source else {
        return Err(ApplyExecutionError::Assurance);
    };
    let baseline_record = run
        .artefact(&source.initial.id)
        .filter(|record| record.artefact_hash == source.initial.artefact_hash)
        .ok_or(ApplyExecutionError::Assurance)?;
    let candidate_record = run
        .artefact(&candidate_input.artefact.id)
        .filter(|record| record.artefact_hash == candidate_input.artefact.artefact_hash)
        .ok_or(ApplyExecutionError::Assurance)?;
    let approval_record = run
        .artefact(&approval_input.artefact.id)
        .filter(|record| record.artefact_hash == approval_input.artefact.artefact_hash)
        .ok_or(ApplyExecutionError::Assurance)?;
    let load_candidate = |record: &super::artefacts::ArtefactRecord| {
        let bytes = store
            .get(&record.object_hash)
            .map_err(|_| ApplyExecutionError::Integrity)?;
        if ObjectHash::of(&bytes) != record.object_hash
            || artefact_hash_for(record.kind, super::artefacts::CANDIDATE_SCHEMA, &bytes)
                != record.artefact_hash
        {
            return Err(ApplyExecutionError::Integrity);
        }
        super::artefacts::candidate::CandidatePayload::from_manifest_bytes(&bytes)
            .ok_or(ApplyExecutionError::Integrity)
    };
    let baseline = load_candidate(baseline_record)?;
    let candidate = load_candidate(candidate_record)?;
    if !candidate_payloads_match(&baseline, &candidate) {
        return Err(ApplyExecutionError::Assurance);
    }
    let decision_bytes = store
        .get(&approval_record.object_hash)
        .map_err(|_| ApplyExecutionError::Integrity)?;
    if ObjectHash::of(&decision_bytes) != approval_record.object_hash {
        return Err(ApplyExecutionError::Integrity);
    }
    let TypedPayload::HumanDecision(decision) =
        parse_typed_payload(ArtefactKind::HumanDecision, &decision_bytes)
            .map_err(|_| ApplyExecutionError::Integrity)?
    else {
        return Err(ApplyExecutionError::Assurance);
    };
    if artefact_hash_for(
        ArtefactKind::HumanDecision,
        decision.format_version,
        &decision_bytes,
    ) != approval_record.artefact_hash
        || decision.decision != super::gates::HumanDecisionKind::Approved
        || super::gates::hashes(&decision)
            != Some((candidate.candidate_hash(), baseline.candidate_hash()))
    {
        return Err(ApplyExecutionError::Assurance);
    }
    let super::artefacts::ArtefactProducer::HumanGate {
        gate_id,
        step,
        output,
    } = &approval_record.provenance.producer
    else {
        return Err(ApplyExecutionError::Assurance);
    };
    let gate = run
        .gates
        .iter()
        .find(|gate| gate.id == *gate_id)
        .ok_or(ApplyExecutionError::Assurance)?;
    if gate.state != super::gates::HumanGateState::Approved
        || gate.decision.as_ref() != Some(&approval_input.artefact)
        || gate.candidate != candidate_input.artefact
        || gate.diff_base != source.initial
        || gate.step != *step
        || gate.output != *output
        || !approval_record
            .provenance
            .inputs
            .iter()
            .any(|input| input == &candidate_input.artefact)
    {
        return Err(ApplyExecutionError::Assurance);
    }
    Ok(ApprovedApplication {
        baseline_reference: source.initial.clone(),
        baseline,
        candidate_reference: candidate_input.artefact.clone(),
        candidate,
        approval: approval_input.artefact.clone(),
    })
}

fn candidate_payloads_match(
    baseline: &super::artefacts::candidate::CandidatePayload,
    candidate: &super::artefacts::candidate::CandidatePayload,
) -> bool {
    match (baseline, candidate) {
        (
            super::artefacts::candidate::CandidatePayload::Revision(left),
            super::artefacts::candidate::CandidatePayload::Revision(right),
        ) => {
            left.ordinary
                && right.ordinary
                && left.repository == right.repository
                && left.git_admin == right.git_admin
                && left.exclusions == right.exclusions
        }
        (
            super::artefacts::candidate::CandidatePayload::Set(left),
            super::artefacts::candidate::CandidatePayload::Set(right),
        ) => {
            left.roots.len() == right.roots.len()
                && left.roots.iter().zip(&right.roots).all(|(left, right)| {
                    left.grant_id == right.grant_id
                        && left.alias == right.alias
                        && left.identity == right.identity
                        && left.candidate.repository == right.candidate.repository
                        && left.candidate.git_admin == right.candidate.git_admin
                        && left.candidate.exclusions == right.candidate.exclusions
                })
        }
        _ => false,
    }
}

fn apply_failure_outcome(error: super::artefacts::apply::ApplyError) -> ApplyRootOutcome {
    use super::artefacts::apply::ApplyError;
    match error {
        ApplyError::Conflict | ApplyError::Drift => ApplyRootOutcome::Conflicted,
        _ => ApplyRootOutcome::Uncertain,
    }
}

fn map_file_apply_error(error: super::artefacts::apply::ApplyError) -> ApplyExecutionError {
    use super::artefacts::apply::ApplyError;
    match error {
        ApplyError::Conflict | ApplyError::Drift => ApplyExecutionError::Conflict,
        ApplyError::Integrity => ApplyExecutionError::Integrity,
        ApplyError::Escape | ApplyError::Unsupported | ApplyError::Write => {
            ApplyExecutionError::Write
        }
    }
}

fn root_outcome_can_advance(left: ApplyRootOutcome, right: ApplyRootOutcome) -> bool {
    left == right
        || matches!(
            (left, right),
            (ApplyRootOutcome::Pending, ApplyRootOutcome::Unchanged)
                | (ApplyRootOutcome::Pending, ApplyRootOutcome::Applied)
                | (ApplyRootOutcome::Pending, ApplyRootOutcome::Conflicted)
                | (ApplyRootOutcome::Pending, ApplyRootOutcome::Uncertain)
                | (ApplyRootOutcome::Applied, ApplyRootOutcome::Uncertain)
                | (ApplyRootOutcome::Applied, ApplyRootOutcome::Unchanged)
                | (ApplyRootOutcome::Unchanged, ApplyRootOutcome::Uncertain)
                | (ApplyRootOutcome::Unchanged, ApplyRootOutcome::Conflicted)
                | (
                    ApplyRootOutcome::Conflicted | ApplyRootOutcome::Uncertain,
                    ApplyRootOutcome::Applied
                        | ApplyRootOutcome::Unchanged
                        | ApplyRootOutcome::Uncertain
                )
        )
}

impl ApplyTransaction {
    pub(crate) fn apply_roots(
        &mut self,
        baseline: &super::artefacts::candidate::CandidateSetArtefact,
        candidate: &super::artefacts::candidate::CandidateSetArtefact,
        store: &WorkflowArtefactRepository,
        journal: &ApplyJournal,
        mut persist: impl FnMut(&Self) -> Result<(), ApplyExecutionError>,
    ) -> Result<(), ApplyExecutionError> {
        use super::artefacts::{
            CandidateApply,
            apply::{ApplyError, CandidateApplicationBinding, changed_paths},
        };
        let child_hash = |value: &super::artefacts::candidate::CandidateRevisionArtefact| {
            let bytes = value
                .manifest_bytes()
                .map_err(|_| ApplyExecutionError::Integrity)?;
            Ok(artefact_hash_for(
                super::definition::ArtefactKind::CandidateRevision,
                value.format_version,
                &bytes,
            ))
        };
        // Every root must pass preflight before any root can change.
        for index in 0..self.roots.len() {
            let root = &self.roots[index];
            let before = &baseline.roots[index].candidate;
            let after = &candidate.roots[index].candidate;
            if let Err(error) = CandidateApply::preflight_bound(
                &root.host_path,
                before,
                child_hash(before)?,
                after,
                child_hash(after)?,
                &root.exclusions,
                store,
            ) {
                self.roots[index].outcome = apply_failure_outcome(error);
                self.state = ApplyTransactionState::Recovered;
                persist(self)?;
                return Err(map_file_apply_error(error));
            }
        }
        let mut completed = 0usize;
        for index in 0..self.roots.len() {
            if self.roots[index].outcome == ApplyRootOutcome::Unchanged {
                continue;
            }
            let root = self.roots[index].clone();
            let before = &baseline.roots[index].candidate;
            let after = &candidate.roots[index].candidate;
            let result = CandidateApply::apply_journalled(
                &root.host_path,
                before,
                after,
                CandidateApplicationBinding {
                    initial_hash: child_hash(before)?,
                    target_hash: child_hash(after)?,
                    exclusions: &root.exclusions,
                },
                store,
                |root_completed, path, applied| {
                    let qualified = format!("{}/{}", root.alias, path);
                    journal
                        .record_progress(completed + root_completed, &qualified, applied)
                        .map_err(|_| ApplyError::Write)?;
                    self.state = if applied {
                        ApplyTransactionState::Applied {
                            completed: completed + root_completed,
                        }
                    } else {
                        ApplyTransactionState::Applying {
                            completed: completed + root_completed,
                            path: qualified,
                        }
                    };
                    persist(self).map_err(|_| ApplyError::Write)
                },
            );
            if let Err(error) = result {
                self.roots[index].outcome = apply_failure_outcome(error);
                self.state = ApplyTransactionState::RecoveryUncertain;
                persist(self)?;
                return Err(map_file_apply_error(error));
            }
            completed += changed_paths(before, after).len();
            self.roots[index].outcome = ApplyRootOutcome::Applied;
            persist(self)?;
        }
        self.state = ApplyTransactionState::Verified;
        persist(self)
    }

    pub(crate) fn can_advance_to(&self, next: &Self) -> bool {
        if self.roots.len() != next.roots.len()
            || self.roots.iter().zip(&next.roots).any(|(left, right)| {
                left.grant_id != right.grant_id
                    || left.alias != right.alias
                    || left.host_path != right.host_path
                    || left.identity != right.identity
                    || left.baseline_candidate != right.baseline_candidate
                    || left.candidate_hash != right.candidate_hash
                    || left.exclusions != right.exclusions
                    || !root_outcome_can_advance(left.outcome, right.outcome)
            })
            || self.baseline != next.baseline
            || self.candidate != next.candidate
            || self.approval != next.approval
        {
            return false;
        }
        if self.state == next.state {
            return true;
        }
        match (&self.state, &next.state) {
            (
                ApplyTransactionState::Prepared,
                ApplyTransactionState::Applying { completed: 0, .. },
            ) => true,
            (
                ApplyTransactionState::Applying { completed, .. },
                ApplyTransactionState::Applied { completed: next },
            ) => next == &(completed + 1),
            (
                ApplyTransactionState::Applied { completed },
                ApplyTransactionState::Applying {
                    completed: next, ..
                },
            ) => completed == next,
            (ApplyTransactionState::Applying { .. }, ApplyTransactionState::Verified)
            | (ApplyTransactionState::Prepared, ApplyTransactionState::Verified)
            | (ApplyTransactionState::Applied { .. }, ApplyTransactionState::Verified)
            | (ApplyTransactionState::Verified, ApplyTransactionState::Verified)
            | (ApplyTransactionState::RecoveryUncertain, ApplyTransactionState::Verified)
            | (_, ApplyTransactionState::Recovered) => true,
            (_, ApplyTransactionState::RecoveryUncertain) => true,
            _ => false,
        }
    }

    pub(crate) fn recover_roots(
        &mut self,
        baseline: &super::artefacts::candidate::CandidateSetArtefact,
        candidate: &super::artefacts::candidate::CandidateSetArtefact,
        started_paths: &[String],
        store: &WorkflowArtefactRepository,
    ) {
        let mut all_targets = true;
        for ((root, before), after) in self
            .roots
            .iter_mut()
            .zip(&baseline.roots)
            .zip(&candidate.roots)
        {
            let grant = crate::execution::DirectoryGrant {
                id: root.grant_id,
                host_path: root.host_path.clone(),
                identity: root.identity,
                alias: root.alias.clone(),
                access: crate::execution::DirectoryAccess::ReviewBeforeApply,
            };
            let live = grant.revalidate().ok().and_then(|()| {
                super::artefacts::CandidateCapture::capture_directory(
                    &root.host_path,
                    &root.exclusions,
                    store,
                )
                .ok()
            });
            let Some(live) = live else {
                root.outcome = ApplyRootOutcome::Uncertain;
                all_targets = false;
                continue;
            };
            if live == before.candidate {
                root.outcome = ApplyRootOutcome::Unchanged;
                all_targets &= before.candidate == after.candidate;
            } else if live == after.candidate {
                let started =
                    super::artefacts::apply::changed_paths(&before.candidate, &after.candidate)
                        .iter()
                        .all(|path| started_paths.contains(&format!("{}/{}", root.alias, path)));
                root.outcome = if started {
                    ApplyRootOutcome::Applied
                } else {
                    ApplyRootOutcome::Uncertain
                };
                all_targets &= started;
            } else {
                let prefix = format!("{}/", root.alias);
                let root_started = started_paths
                    .iter()
                    .filter_map(|path| path.strip_prefix(&prefix).map(str::to_owned))
                    .collect::<Vec<_>>();
                root.outcome = if super::artefacts::CandidateApply::recover_to_initial(
                    &root.host_path,
                    &before.candidate,
                    &after.candidate,
                    &root.exclusions,
                    &root_started,
                    store,
                )
                .is_ok()
                {
                    ApplyRootOutcome::Unchanged
                } else {
                    ApplyRootOutcome::Uncertain
                };
                all_targets = false;
            }
        }
        self.state = if all_targets {
            ApplyTransactionState::Verified
        } else if self
            .roots
            .iter()
            .any(|root| root.outcome == ApplyRootOutcome::Uncertain)
        {
            ApplyTransactionState::RecoveryUncertain
        } else {
            // A known partial result releases recovery, but never completes the run.
            ApplyTransactionState::Recovered
        };
    }

    pub(crate) fn is_verified(&self) -> bool {
        self.state == ApplyTransactionState::Verified
    }

    pub(crate) fn is_settled(&self) -> bool {
        matches!(
            self.state,
            ApplyTransactionState::Verified | ApplyTransactionState::Recovered
        )
    }
}

pub(crate) struct ApplyJournals {
    root: PathBuf,
    #[cfg(test)]
    _hold: Option<tempfile::TempDir>,
}

pub(crate) struct ApplyJournal {
    root: PathBuf,
}

impl ApplyJournals {
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        let hold = tempfile::tempdir().expect("application journals");
        Self {
            root: hold.path().to_path_buf(),
            _hold: Some(hold),
        }
    }

    pub(crate) fn open(root: PathBuf) -> Result<Self, PersistError> {
        storage::ensure_private_dir(&root)?;
        Ok(Self {
            root,
            #[cfg(test)]
            _hold: None,
        })
    }

    pub(crate) fn create(
        &self,
        run: RunId,
        attempt: AttemptId,
        transaction: &ApplyTransaction,
        baseline_manifest: &[u8],
        baseline_hash: ArtefactHash,
    ) -> Result<ApplyJournal, PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        storage::ensure_private_dir(&run_dir)?;
        let root = storage::confined_child(&run_dir, &attempt.as_hex())?;
        if root.exists() {
            return Err(PersistError);
        }
        storage::ensure_private_dir(&root)?;
        storage::write_private(&root.join("baseline.json"), baseline_manifest)?;
        storage::write_private(
            &root.join("baseline.hash"),
            baseline_hash.as_str().as_bytes(),
        )?;
        storage::write_private(&root.join("binding.json"), &binding_bytes(transaction)?)?;
        sync_dir(&root)?;
        Ok(ApplyJournal { root })
    }

    pub(crate) fn load(
        &self,
        run: RunId,
        attempt: AttemptId,
    ) -> Result<ApplyJournal, PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        let root = storage::confined_child(&run_dir, &attempt.as_hex())?;
        if !root.is_dir() {
            return Err(PersistError);
        }
        Ok(ApplyJournal { root })
    }

    pub(crate) fn remove(&self, run: RunId, attempt: AttemptId) -> Result<(), PersistError> {
        let run_dir = storage::confined_child(&self.root, &run.as_hex())?;
        let path = storage::confined_child(&run_dir, &attempt.as_hex())?;
        if path.exists() {
            storage::remove_tree_nofollow(&path).map_err(|_| PersistError)?;
        }
        Ok(())
    }
}

impl ApplyJournal {
    pub(crate) fn make_sure_binding(
        &self,
        transaction: &ApplyTransaction,
    ) -> Result<(), PersistError> {
        let stored = std::fs::read(self.root.join("binding.json")).map_err(|_| PersistError)?;
        if stored != binding_bytes(transaction)? {
            return Err(PersistError);
        }
        Ok(())
    }

    pub(crate) fn make_sure_baseline(
        &self,
        manifest: &[u8],
        hash: ArtefactHash,
    ) -> Result<(), PersistError> {
        let stored = std::fs::read(self.root.join("baseline.json")).map_err(|_| PersistError)?;
        let stored_hash =
            std::fs::read(self.root.join("baseline.hash")).map_err(|_| PersistError)?;
        if stored != manifest || stored_hash != hash.as_str().as_bytes() {
            return Err(PersistError);
        }
        Ok(())
    }

    pub(crate) fn started_paths(&self, operations: &[String]) -> Result<Vec<String>, PersistError> {
        let progress = self.root.join("progress.log");
        let file = match std::fs::File::open(progress) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(PersistError),
        };
        let limit = operations
            .iter()
            .map(|path| 2 * path.len() + 80)
            .sum::<usize>();
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| PersistError)?;
        if bytes.len() > limit {
            return Err(PersistError);
        }
        let mut remaining = bytes.as_slice();
        let mut started = Vec::new();
        for (index, path) in operations.iter().enumerate() {
            if remaining.is_empty() {
                return Ok(started);
            }
            let next = format!("{index}\tnext\t{path}\n");
            remaining = remaining
                .strip_prefix(next.as_bytes())
                .ok_or(PersistError)?;
            started.push(path.clone());
            if remaining.is_empty() {
                return Ok(started);
            }
            let applied = format!("{}\tapplied\t{path}\n", index + 1);
            remaining = remaining
                .strip_prefix(applied.as_bytes())
                .ok_or(PersistError)?;
        }
        if !remaining.is_empty() {
            return Err(PersistError);
        }
        Ok(started)
    }

    pub(crate) fn record_progress(
        &self,
        completed: usize,
        path: &str,
        applied: bool,
    ) -> Result<(), PersistError> {
        if path.is_empty() || path.contains('\n') {
            return Err(PersistError);
        }
        let progress = self.root.join("progress.log");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(progress)
            .map_err(|_| PersistError)?;
        writeln!(
            file,
            "{completed}\t{}\t{path}",
            if applied { "applied" } else { "next" }
        )
        .map_err(|_| PersistError)?;
        file.sync_all().map_err(|_| PersistError)?;
        sync_dir(&self.root)
    }
}

fn binding_bytes(transaction: &ApplyTransaction) -> Result<Vec<u8>, PersistError> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct Binding<'a> {
        roots: Vec<BindingRoot<'a>>,
        baseline_id: String,
        baseline: String,
        candidate_id: String,
        candidate: String,
        approval_id: String,
        approval: String,
    }
    #[derive(serde::Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct BindingRoot<'a> {
        grant_id: String,
        alias: &'a str,
        host_path: &'a Path,
        device: u64,
        inode: u64,
        baseline_candidate: String,
        candidate_hash: String,
        exclusions: &'a [String],
    }
    serde_json::to_vec(&Binding {
        roots: transaction
            .roots
            .iter()
            .map(|root| BindingRoot {
                grant_id: root.grant_id.as_hex(),
                alias: &root.alias,
                host_path: &root.host_path,
                device: root.identity.device,
                inode: root.identity.inode,
                baseline_candidate: root.baseline_candidate.as_str(),
                candidate_hash: root.candidate_hash.as_str(),
                exclusions: &root.exclusions,
            })
            .collect(),
        baseline_id: transaction.baseline.id.as_hex(),
        baseline: transaction.baseline.artefact_hash.as_str(),
        candidate_id: transaction.candidate.id.as_hex(),
        candidate: transaction.candidate.artefact_hash.as_str(),
        approval_id: transaction.approval.id.as_hex(),
        approval: transaction.approval.artefact_hash.as_str(),
    })
    .map_err(|_| PersistError)
}

fn sync_dir(path: &Path) -> Result<(), PersistError> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| PersistError)
}
