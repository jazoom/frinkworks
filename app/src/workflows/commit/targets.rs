use crate::execution::{DirectoryAccess, DirectoryGrant, DirectoryGrantId};
use crate::workflows::artefacts::{CandidatePayload, candidate::CandidateRevisionArtefact};
use crate::workflows::run::WorkflowRun;

use super::CommitError;

pub(crate) fn target_grant(
    run: &WorkflowRun,
    id: DirectoryGrantId,
) -> Result<DirectoryGrant, CommitError> {
    let grant = run
        .reviewed_directories()
        .into_iter()
        .find(|grant| grant.id == id)
        .ok_or(CommitError::Authority)?;
    grant.revalidate().map_err(|_| CommitError::Authority)?;
    Ok(grant)
}

pub(crate) fn destination_revision<'a>(
    grant: &DirectoryGrant,
    payload: &'a CandidatePayload,
) -> Result<&'a CandidateRevisionArtefact, CommitError> {
    if grant.access != DirectoryAccess::ReviewBeforeApply {
        return Err(CommitError::Authority);
    }
    match payload {
        CandidatePayload::Revision(revision) => Ok(revision),
        CandidatePayload::Set(set) => set
            .roots
            .iter()
            .find(|root| {
                root.grant_id == grant.id
                    && root.alias == grant.alias
                    && root.identity == grant.identity
            })
            .map(|root| &root.candidate)
            .ok_or(CommitError::Authority),
    }
}

pub(crate) fn destination_pair<'a>(
    grant: &DirectoryGrant,
    initial: &'a CandidatePayload,
    target: &'a CandidatePayload,
) -> Result<(&'a CandidateRevisionArtefact, &'a CandidateRevisionArtefact), CommitError> {
    let before = destination_revision(grant, initial)?;
    let after = destination_revision(grant, target)?;
    if before.repository != after.repository
        || before.git_admin != after.git_admin
        || before.exclusions != after.exclusions
    {
        return Err(CommitError::Preflight);
    }
    Ok((before, after))
}

// Detection selects only changed, approved roots. It never expands a grant to its parent repository.
pub(crate) fn commit_targets(
    run: &WorkflowRun,
    initial: &CandidatePayload,
    target: &CandidatePayload,
) -> Result<Vec<DirectoryGrant>, CommitError> {
    let grants = run.reviewed_directories();
    let ids = match (initial, target) {
        (CandidatePayload::Set(before), CandidatePayload::Set(after)) => {
            if before.roots.len() != after.roots.len()
                || before.roots.iter().zip(&after.roots).any(|(a, b)| {
                    a.grant_id != b.grant_id || a.alias != b.alias || a.identity != b.identity
                })
            {
                return Err(CommitError::Authority);
            }
            before
                .roots
                .iter()
                .map(|root| root.grant_id)
                .collect::<Vec<_>>()
        }
        (CandidatePayload::Revision(_), CandidatePayload::Revision(_)) if grants.len() == 1 => {
            vec![grants[0].id]
        }
        _ => return Err(CommitError::Authority),
    };
    let mut targets = Vec::new();
    for id in ids {
        let grant = grants
            .iter()
            .find(|grant| grant.id == id)
            .ok_or(CommitError::Authority)?;
        let (before, after) = destination_pair(grant, initial, target)?;
        if before.candidate_hash == after.candidate_hash {
            continue;
        }
        grant.revalidate().map_err(|_| CommitError::Authority)?;
        if before.repository.is_none()
            || before.git_admin.is_none()
            || grant.host_path.join(".jj").exists()
        {
            return Err(CommitError::Repository);
        }
        crate::workflows::artefacts::inspect_supported_worktree(&grant.host_path)
            .map_err(|_| CommitError::Repository)?;
        targets.push(grant.clone());
    }
    Ok(targets)
}

#[cfg(test)]
mod tests;
