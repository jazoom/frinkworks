use crate::sandbox::{GUEST_PROJECT, GuestExec};
use crate::workflows::artefacts::candidate::{
    CandidateEntryKind, CandidateRevisionArtefact, GitObjectFormat, GitObjectId,
};
use crate::workflows::artefacts::{
    ArtefactProducer, ArtefactRecord, ArtefactReference, CandidatePayload, ReviewVerdict,
    TypedPayload, WorkflowArtefactRepository, artefact_hash_for, parse_typed_payload,
};
use crate::workflows::definition::{ArtefactKind, CommitPolicy};
use crate::workflows::run::{AttemptArtefactInput, FailureCategory, WorkflowRun};

mod journal;

pub(crate) use journal::{CommitJournal, CommitJournals};

const AUTHOR_NAME: &str = "Power Plant";
const AUTHOR_EMAIL: &str = "powerplant@localhost";
const COMMIT_MESSAGE: &str = "Apply Power Plant workflow candidate";
const TEMP_INDEX_GUEST_PREFIX: &str = "/project/.git/powerplant-commit-index-";
pub(crate) const NON_APPROVED_MESSAGE: &str = "The review did not approve this candidate.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommitTransactionState {
    Prepared,
    WorktreeApplied,
    ReferenceUpdated { commit: String },
    Verified { commit: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitTransaction {
    pub(crate) state: CommitTransactionState,
    pub(crate) candidate: ArtefactReference,
    pub(crate) reviews: Vec<ArtefactReference>,
    pub(crate) approval: Option<ArtefactReference>,
    pub(crate) expected_reference: String,
    pub(crate) old_object: Option<String>,
    pub(crate) target_tree: Option<String>,
    pub(crate) expected_commit: Option<String>,
    pub(crate) timestamp: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitResult {
    pub(crate) commit: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitError {
    Assurance,
    Authority,
    Preflight,
    Command,
    Apply,
    Operational,
}

impl CommitError {
    pub(crate) fn category(self) -> FailureCategory {
        match self {
            Self::Assurance => FailureCategory::Assurance,
            Self::Authority => FailureCategory::Authority,
            Self::Preflight | Self::Operational => FailureCategory::Operational,
            Self::Command | Self::Apply => FailureCategory::Commit,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Assurance => NON_APPROVED_MESSAGE,
            Self::Authority => "Project access changed before that commit.",
            Self::Preflight => "The project changed before that commit.",
            Self::Command => "Power Plant could not create the Git commit.",
            Self::Apply => "Power Plant could not apply the candidate.",
            Self::Operational => "Power Plant could not store the workflow run. Try again.",
        }
    }
}

pub(crate) fn require_commit_approval(
    run: &WorkflowRun,
    commit_step: &crate::workflows::definition::StepDefinition,
    inputs: &[AttemptArtefactInput],
    store: &WorkflowArtefactRepository,
) -> Result<(ArtefactRecord, Vec<ArtefactRecord>, CandidatePayload), CommitError> {
    match run.pinned.definition.commit_policy() {
        CommitPolicy::NoCommit => return Err(CommitError::Assurance),
        CommitPolicy::HumanApproval
            if !inputs
                .iter()
                .any(|input| input.artefact.kind == ArtefactKind::HumanDecision) =>
        {
            return Err(CommitError::Assurance);
        }
        CommitPolicy::AutomaticAfterReview
            if !inputs
                .iter()
                .any(|input| input.artefact.kind == ArtefactKind::ReviewReport)
                || inputs
                    .iter()
                    .any(|input| input.artefact.kind == ArtefactKind::HumanDecision) =>
        {
            return Err(CommitError::Assurance);
        }
        CommitPolicy::HumanApproval | CommitPolicy::AutomaticAfterReview => {}
    }
    if inputs.iter().any(|input| {
        !matches!(
            input.artefact.kind,
            ArtefactKind::CandidateRevision
                | ArtefactKind::ReviewReport
                | ArtefactKind::HumanDecision
        )
    }) {
        return Err(CommitError::Assurance);
    }
    let mut candidates = inputs
        .iter()
        .filter(|input| input.artefact.kind == ArtefactKind::CandidateRevision);
    let candidate_input = candidates.next().ok_or(CommitError::Assurance)?;
    if candidates.next().is_some() {
        return Err(CommitError::Assurance);
    }
    let review_inputs: Vec<_> = inputs
        .iter()
        .filter(|input| input.artefact.kind == ArtefactKind::ReviewReport)
        .collect();
    let decision_inputs: Vec<_> = inputs
        .iter()
        .filter(|input| input.artefact.kind == ArtefactKind::HumanDecision)
        .collect();
    if decision_inputs.len() > 1 || (review_inputs.is_empty() && decision_inputs.is_empty()) {
        return Err(CommitError::Assurance);
    }
    let candidate_record = run
        .artefact(&candidate_input.artefact.id)
        .cloned()
        .ok_or(CommitError::Assurance)?;
    let review_records: Vec<_> = review_inputs
        .iter()
        .map(|input| {
            run.artefact(&input.artefact.id)
                .cloned()
                .ok_or(CommitError::Assurance)
        })
        .collect::<Result<_, _>>()?;
    if candidate_record.artefact_hash != candidate_input.artefact.artefact_hash {
        return Err(CommitError::Assurance);
    }
    for (review_input, review_record) in review_inputs.iter().zip(&review_records) {
        if review_record.artefact_hash != review_input.artefact.artefact_hash
            || review_record.provenance.run_id != run.id
            || !review_record
                .provenance
                .inputs
                .iter()
                .any(|input| input == &candidate_input.artefact)
        {
            return Err(CommitError::Assurance);
        }
    }
    let crate::workflows::RunSource::Captured { source } = &run.source else {
        return Err(CommitError::Assurance);
    };
    let crate::workflows::run::ObservedCandidate::Exact { artefact } = &source.observed else {
        return Err(CommitError::Assurance);
    };
    if artefact.id != candidate_record.id || source.accepted.id != candidate_record.id {
        return Err(CommitError::Assurance);
    }
    let candidate_bytes = store
        .get(&candidate_record.object_hash)
        .map_err(|_| CommitError::Assurance)?;
    if crate::workflows::artefacts::ObjectHash::of(&candidate_bytes) != candidate_record.object_hash
    {
        return Err(CommitError::Assurance);
    }
    let candidate =
        CandidatePayload::from_manifest_bytes(&candidate_bytes).ok_or(CommitError::Assurance)?;
    let artefact_hash = artefact_hash_for(
        ArtefactKind::CandidateRevision,
        crate::workflows::artefacts::CANDIDATE_SCHEMA,
        &candidate_bytes,
    );
    if artefact_hash != candidate_record.artefact_hash {
        return Err(CommitError::Assurance);
    }
    for (review_input, review_record) in review_inputs.iter().zip(&review_records) {
        let review_bytes = store
            .get(&review_record.object_hash)
            .map_err(|_| CommitError::Assurance)?;
        if crate::workflows::artefacts::ObjectHash::of(&review_bytes) != review_record.object_hash {
            return Err(CommitError::Assurance);
        }
        let TypedPayload::Review(report) =
            parse_typed_payload(ArtefactKind::ReviewReport, &review_bytes)
                .map_err(|_| CommitError::Assurance)?
        else {
            return Err(CommitError::Assurance);
        };
        let bound = crate::workflows::artefacts::CandidateHash::parse(&report.candidate)
            .ok_or(CommitError::Assurance)?;
        if report.verdict != ReviewVerdict::Approved || bound != candidate.candidate_hash() {
            return Err(CommitError::Assurance);
        }
        let declared_source = commit_step
            .inputs
            .iter()
            .find(|input| input.key == review_input.key)
            .map(|input| &input.source);
        match (&review_record.provenance.producer, declared_source) {
            (
                ArtefactProducer::StepAttempt {
                    step,
                    output: Some(output),
                    ..
                },
                Some(crate::workflows::definition::ArtefactSource::StepOutput {
                    step: declared_step,
                    output: declared_output,
                }),
            ) if step == declared_step && output == declared_output => {}
            _ => return Err(CommitError::Assurance),
        }
    }
    if let Some(input) = decision_inputs.first() {
        let record = run
            .artefact(&input.artefact.id)
            .ok_or(CommitError::Assurance)?;
        if record.artefact_hash != input.artefact.artefact_hash
            || record.provenance.run_id != run.id
            || !record
                .provenance
                .inputs
                .iter()
                .any(|item| item == &candidate_input.artefact)
        {
            return Err(CommitError::Assurance);
        }
        let bytes = store
            .get(&record.object_hash)
            .map_err(|_| CommitError::Assurance)?;
        if crate::workflows::artefacts::ObjectHash::of(&bytes) != record.object_hash {
            return Err(CommitError::Assurance);
        }
        let TypedPayload::HumanDecision(decision) =
            parse_typed_payload(ArtefactKind::HumanDecision, &bytes)
                .map_err(|_| CommitError::Assurance)?
        else {
            return Err(CommitError::Assurance);
        };
        if artefact_hash_for(ArtefactKind::HumanDecision, decision.format_version, &bytes)
            != record.artefact_hash
        {
            return Err(CommitError::Assurance);
        }
        let base = run
            .artefact(&source.initial.id)
            .and_then(ArtefactRecord::candidate_hash)
            .ok_or(CommitError::Assurance)?;
        let (bound, diff_base) =
            crate::workflows::gates::hashes(&decision).ok_or(CommitError::Assurance)?;
        let ArtefactProducer::HumanGate {
            gate_id,
            step,
            output,
        } = &record.provenance.producer
        else {
            return Err(CommitError::Assurance);
        };
        let gate = run
            .gates
            .iter()
            .find(|gate| gate.id == *gate_id)
            .ok_or(CommitError::Assurance)?;
        let declared_source = commit_step
            .inputs
            .iter()
            .find(|declared| declared.key == input.key)
            .map(|declared| &declared.source);
        let named_gate_output = matches!(
            declared_source,
            Some(crate::workflows::definition::ArtefactSource::StepOutput {
                step: declared_step,
                output: declared_output,
            }) if declared_step == step && declared_output == output
        );
        if decision.decision != crate::workflows::gates::HumanDecisionKind::Approved
            || gate.state != crate::workflows::gates::HumanGateState::Approved
            || gate.closed_at_ms.is_none()
            || gate.decision.as_ref() != Some(&input.artefact)
            || gate.step != *step
            || gate.output != *output
            || bound != candidate.candidate_hash()
            || diff_base != base
            || !named_gate_output
        {
            return Err(CommitError::Assurance);
        }
    }
    Ok((candidate_record, review_records, candidate))
}

pub(crate) fn destination_revision<'a>(
    run: &WorkflowRun,
    payload: &'a CandidatePayload,
) -> Result<&'a CandidateRevisionArtefact, CommitError> {
    match payload {
        CandidatePayload::Revision(revision) => Ok(revision),
        CandidatePayload::Set(set) => {
            let settings = run.directory_settings().ok_or(CommitError::Authority)?;
            let grant = settings
                .git_destination_grant()
                .ok_or(CommitError::Authority)?;
            if grant.access != crate::execution::DirectoryAccess::ReviewBeforeApply {
                return Err(CommitError::Authority);
            }
            set.roots
                .iter()
                .find(|root| {
                    root.grant_id == grant.id
                        && root.alias == grant.alias
                        && root.identity == grant.identity
                })
                .map(|root| &root.candidate)
                .ok_or(CommitError::Authority)
        }
    }
}

pub(crate) fn destination_pair<'a>(
    run: &WorkflowRun,
    initial: &'a CandidatePayload,
    target: &'a CandidatePayload,
) -> Result<(&'a CandidateRevisionArtefact, &'a CandidateRevisionArtefact), CommitError> {
    let before = destination_revision(run, initial)?;
    let after = destination_revision(run, target)?;
    // A single-repository transaction cannot apply changes in other roots or
    // remove excluded tracked paths through its replacement index.
    if !before.exclusions.is_empty() || !after.exclusions.is_empty() {
        return Err(CommitError::Preflight);
    }
    match (initial, target) {
        (CandidatePayload::Set(initial), CandidatePayload::Set(target)) => {
            let destination = run
                .directory_settings()
                .and_then(|settings| settings.git_destination)
                .ok_or(CommitError::Authority)?;
            if initial.roots.len() != target.roots.len()
                || initial
                    .roots
                    .iter()
                    .zip(&target.roots)
                    .any(|(left, right)| {
                        left.grant_id != right.grant_id
                            || left.alias != right.alias
                            || left.identity != right.identity
                            || (left.grant_id != destination && left != right)
                    })
            {
                return Err(CommitError::Preflight);
            }
        }
        (CandidatePayload::Revision(_), CandidatePayload::Revision(_)) => {}
        _ => return Err(CommitError::Preflight),
    }
    Ok((before, after))
}

pub(crate) fn capture_revision(
    project: &std::path::Path,
    template: &CandidateRevisionArtefact,
    store: &WorkflowArtefactRepository,
) -> Result<CandidateRevisionArtefact, CommitError> {
    let captured = if template.ordinary {
        crate::workflows::artefacts::CandidateCapture::capture_directory(
            project,
            &template.exclusions,
            store,
        )
    } else {
        crate::workflows::artefacts::CandidateCapture::capture_host(project, store)
    };
    captured.map_err(|_| CommitError::Preflight)
}

pub(crate) fn require_unchanged_project(
    project: &std::path::Path,
    initial: &CandidateRevisionArtefact,
    target: &CandidateRevisionArtefact,
    store: &WorkflowArtefactRepository,
) -> Result<(), CommitError> {
    let live = capture_revision(project, initial, store)?;
    if live != *initial
        || target.repository != initial.repository
        || target.git_admin != initial.git_admin
    {
        return Err(CommitError::Preflight);
    }
    Ok(())
}

impl CommitTransactionState {
    pub(crate) fn encode(&self) -> String {
        match self {
            Self::Prepared => "prepared".to_owned(),
            Self::WorktreeApplied => "worktree-applied".to_owned(),
            Self::ReferenceUpdated { commit } => format!("reference-updated:{commit}"),
            Self::Verified { commit } => format!("verified:{commit}"),
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "worktree-applied" => Some(Self::WorktreeApplied),
            value if value.starts_with("reference-updated:") => Some(Self::ReferenceUpdated {
                commit: value.strip_prefix("reference-updated:")?.to_owned(),
            }),
            value if value.starts_with("verified:") => Some(Self::Verified {
                commit: value.strip_prefix("verified:")?.to_owned(),
            }),
            _ => None,
        }
    }
}

impl CommitTransaction {
    pub(crate) fn can_advance_to(&self, next: &Self) -> bool {
        if self.candidate != next.candidate
            || self.reviews != next.reviews
            || self.approval != next.approval
            || self.expected_reference != next.expected_reference
            || self.old_object != next.old_object
            || self.timestamp != next.timestamp
        {
            return false;
        }
        let valid_state = match (&self.state, &next.state) {
            (CommitTransactionState::Prepared, CommitTransactionState::Prepared)
            | (CommitTransactionState::Prepared, CommitTransactionState::WorktreeApplied) => true,
            (
                CommitTransactionState::WorktreeApplied,
                CommitTransactionState::ReferenceUpdated { commit },
            ) => next.expected_commit.as_deref() == Some(commit),
            (
                CommitTransactionState::ReferenceUpdated { commit: current },
                CommitTransactionState::Verified {
                    commit: next_commit,
                },
            ) => current == next_commit && next.expected_commit.as_deref() == Some(next_commit),
            (
                CommitTransactionState::Verified { commit: current },
                CommitTransactionState::Verified {
                    commit: next_commit,
                },
            ) => current == next_commit,
            _ => false,
        };
        valid_state
            && self
                .target_tree
                .as_ref()
                .is_none_or(|tree| next.target_tree.as_ref() == Some(tree))
            && self
                .expected_commit
                .as_ref()
                .is_none_or(|commit| next.expected_commit.as_ref() == Some(commit))
    }

    pub(crate) fn verified_commit(&self) -> Option<&str> {
        match &self.state {
            CommitTransactionState::Verified { commit } => Some(commit),
            _ => None,
        }
    }
}

pub(crate) fn temporary_index_guest(attempt: crate::workflows::AttemptId) -> String {
    format!("{TEMP_INDEX_GUEST_PREFIX}{}", attempt.as_hex())
}

pub(crate) fn git_env(timestamp: &str) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        ("GIT_OPTIONAL_LOCKS".to_owned(), "0".to_owned()),
        ("GIT_AUTHOR_NAME".to_owned(), AUTHOR_NAME.to_owned()),
        ("GIT_AUTHOR_EMAIL".to_owned(), AUTHOR_EMAIL.to_owned()),
        ("GIT_COMMITTER_NAME".to_owned(), AUTHOR_NAME.to_owned()),
        ("GIT_COMMITTER_EMAIL".to_owned(), AUTHOR_EMAIL.to_owned()),
        ("GIT_AUTHOR_DATE".to_owned(), timestamp.to_owned()),
        ("GIT_COMMITTER_DATE".to_owned(), timestamp.to_owned()),
    ]
}

pub(crate) fn git_command(args: Vec<String>, stdin: Option<Vec<u8>>, timestamp: &str) -> GuestExec {
    let mut exec = GuestExec::command("git", args)
        .in_dir(GUEST_PROJECT)
        .with_env(git_env(timestamp));
    if let Some(stdin) = stdin {
        exec = exec.with_stdin(stdin);
    }
    exec
}

pub(crate) fn plumbing_prefix() -> Vec<String> {
    vec![
        "--no-optional-locks".to_owned(),
        "-c".to_owned(),
        "core.hooksPath=/dev/null".to_owned(),
        "-c".to_owned(),
        "commit.gpgsign=false".to_owned(),
        "-c".to_owned(),
        "core.useReplaceRefs=false".to_owned(),
    ]
}

pub(crate) fn hash_object_command(bytes: Vec<u8>, timestamp: &str) -> GuestExec {
    let mut args = plumbing_prefix();
    args.extend([
        "hash-object".to_owned(),
        "-w".to_owned(),
        "--stdin".to_owned(),
    ]);
    git_command(args, Some(bytes), timestamp)
}

fn with_index(mut exec: GuestExec, index: &str) -> GuestExec {
    exec.env
        .push(("GIT_INDEX_FILE".to_owned(), index.to_owned()));
    exec
}

pub(crate) fn read_tree_empty_command(index: &str, timestamp: &str) -> GuestExec {
    let mut args = plumbing_prefix();
    args.extend(["read-tree".to_owned(), "--empty".to_owned()]);
    with_index(git_command(args, None, timestamp), index)
}

pub(crate) fn write_tree_command(index: &str, timestamp: &str) -> GuestExec {
    let mut args = plumbing_prefix();
    args.push("write-tree".to_owned());
    with_index(git_command(args, None, timestamp), index)
}

pub(crate) fn commit_tree_command(tree: &str, parent: Option<&str>, timestamp: &str) -> GuestExec {
    let mut args = plumbing_prefix();
    args.push("commit-tree".to_owned());
    args.push(tree.to_owned());
    if let Some(parent) = parent {
        args.push("-p".to_owned());
        args.push(parent.to_owned());
    }
    args.push("-m".to_owned());
    args.push(COMMIT_MESSAGE.to_owned());
    git_command(args, None, timestamp)
}

pub(crate) fn update_ref_command(
    reference: &str,
    new: &str,
    old: Option<&str>,
    timestamp: &str,
) -> GuestExec {
    let mut args = plumbing_prefix();
    args.push("update-ref".to_owned());
    args.push(reference.to_owned());
    args.push(new.to_owned());
    if let Some(old) = old {
        args.push(old.to_owned());
    }
    git_command(args, None, timestamp)
}

pub(crate) fn index_info_command(info: Vec<u8>, index: &str, timestamp: &str) -> GuestExec {
    let mut args = plumbing_prefix();
    args.extend([
        "update-index".to_owned(),
        "-z".to_owned(),
        "--index-info".to_owned(),
    ]);
    with_index(git_command(args, Some(info), timestamp), index)
}

pub(crate) fn index_info_record(
    entry: &crate::workflows::artefacts::candidate::CandidateEntry,
    blob: &str,
) -> Result<Vec<u8>, CommitError> {
    let (mode, object) = match &entry.kind {
        CandidateEntryKind::Regular {
            executable: false, ..
        } => ("100644", blob.to_owned()),
        CandidateEntryKind::Regular {
            executable: true, ..
        } => ("100755", blob.to_owned()),
        CandidateEntryKind::Symlink { .. } => ("120000", blob.to_owned()),
        CandidateEntryKind::Directory { .. } => return Err(CommitError::Command),
        CandidateEntryKind::Gitlink { commit } => ("160000", commit.0.clone()),
    };
    let mut record = format!("{mode} {object}\t{} ", entry.path).into_bytes();
    if record.len() > crate::workflows::artefacts::candidate::MAXIMUM_PATH_BYTES + 160 {
        return Err(CommitError::Command);
    }
    Ok(std::mem::take(&mut record))
}

pub(crate) fn utc_timestamp(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{seconds} +0000")
}

pub(crate) fn parse_object_id(
    raw: &str,
    format: GitObjectFormat,
) -> Result<GitObjectId, CommitError> {
    let value = raw.trim();
    let expected = match format {
        GitObjectFormat::Sha1 => 40,
        GitObjectFormat::Sha256 => 64,
    };
    if value.len() != expected
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(CommitError::Command);
    }
    Ok(GitObjectId(value.to_owned()))
}

#[cfg(test)]
mod tests;
