use crate::environments::{
    EnvironmentCatalogue, EnvironmentId, EnvironmentRecipeVersion, EnvironmentSnapshotRepository,
    PreparationId, PreparedSnapshot, ReadyPointer, ReadyPointerError, SnapshotAvailability,
    SnapshotDigest,
};

use super::definition::{StepDefinition, StepKey, WorkflowDefinition};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedEnvironmentSet {
    pub(crate) environments: Vec<ResolvedEnvironment>,
    pub(crate) steps: Vec<ResolvedStepEnvironment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedEnvironment {
    pub(crate) environment_id: EnvironmentId,
    pub(crate) name: String,
    pub(crate) preparation_id: PreparationId,
    pub(crate) recipe_version: EnvironmentRecipeVersion,
    pub(crate) snapshot: PreparedSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedStepEnvironment {
    pub(crate) step: StepKey,
    pub(crate) environment_id: EnvironmentId,
    pub(crate) preparation_id: PreparationId,
    pub(crate) snapshot_digest: SnapshotDigest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolveEnvironmentError {
    Missing,
    NotReady,
    Unavailable,
    Changed,
}

impl ResolveEnvironmentError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Missing => "That environment is no longer in the catalogue.",
            Self::NotReady => "That environment is not ready.",
            Self::Unavailable => "That environment snapshot is unavailable.",
            Self::Changed => "That environment changed. Try again.",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnvironmentPreview {
    pub(crate) environments: Vec<PreviewEnvironment>,
    pub(crate) steps: Vec<PreviewStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewEnvironment {
    pub(crate) environment_id: EnvironmentId,
    pub(crate) name: String,
    pub(crate) preparation_ordinal: u64,
    pub(crate) snapshot_short: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewStep {
    pub(crate) step: String,
    pub(crate) environment_name: String,
    pub(crate) preparation_ordinal: u64,
    pub(crate) snapshot_short: String,
}

pub(crate) fn step_environment_ids(
    definition: &WorkflowDefinition,
) -> Vec<(StepKey, EnvironmentId)> {
    definition
        .steps()
        .iter()
        .filter(|step| step.is_sandbox_backed())
        .map(|step| (step.key.clone(), definition.effective_environment(step)))
        .collect()
}

pub(crate) async fn preview_environments(
    definition: &WorkflowDefinition,
    catalogue: &EnvironmentCatalogue,
    snapshots: &EnvironmentSnapshotRepository,
) -> Result<EnvironmentPreview, ResolveEnvironmentError> {
    let bindings = step_environment_ids(definition);
    let mut unique = Vec::new();
    for (_, id) in &bindings {
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    let mut pointers = Vec::new();
    for id in unique {
        pointers.push(copy_ready(catalogue, &id)?);
    }
    for pointer in &pointers {
        if snapshots.inspect(&pointer.snapshot).await != SnapshotAvailability::Available {
            return Err(ResolveEnvironmentError::Unavailable);
        }
    }
    let mut environments = Vec::new();
    for pointer in &pointers {
        let ordinal = catalogue
            .preparation(&pointer.preparation_id)
            .map(|record| record.ordinal)
            .unwrap_or(0);
        environments.push(PreviewEnvironment {
            environment_id: pointer.environment_id,
            name: pointer.name.clone(),
            preparation_ordinal: ordinal,
            snapshot_short: pointer.snapshot.snapshot_digest.short_hex(),
        });
    }
    let mut steps = Vec::new();
    for (step, environment_id) in bindings {
        let pointer = pointers
            .iter()
            .find(|pointer| pointer.environment_id == environment_id)
            .ok_or(ResolveEnvironmentError::Missing)?;
        let name = definition
            .step(&step)
            .map(|item: &StepDefinition| item.name.clone())
            .unwrap_or_else(|| step.as_str().to_owned());
        let ordinal = catalogue
            .preparation(&pointer.preparation_id)
            .map(|record| record.ordinal)
            .unwrap_or(0);
        steps.push(PreviewStep {
            step: name,
            environment_name: pointer.name.clone(),
            preparation_ordinal: ordinal,
            snapshot_short: pointer.snapshot.snapshot_digest.short_hex(),
        });
    }
    Ok(EnvironmentPreview {
        environments,
        steps,
    })
}

pub(crate) async fn resolve_environments(
    definition: &WorkflowDefinition,
    catalogue: &EnvironmentCatalogue,
    snapshots: &EnvironmentSnapshotRepository,
) -> Result<ResolvedEnvironmentSet, ResolveEnvironmentError> {
    let bindings = step_environment_ids(definition);
    let mut unique = Vec::new();
    for (_, id) in &bindings {
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    let mut pointers = Vec::new();
    for id in unique {
        pointers.push(copy_ready(catalogue, &id)?);
    }
    for pointer in &pointers {
        snapshots
            .verify(&pointer.snapshot)
            .await
            .map_err(|_| ResolveEnvironmentError::Unavailable)?;
    }
    for pointer in &pointers {
        if !catalogue.ready_pointer_matches(pointer) {
            return Err(ResolveEnvironmentError::Changed);
        }
    }
    let environments = pointers
        .iter()
        .map(|pointer| ResolvedEnvironment {
            environment_id: pointer.environment_id,
            name: pointer.name.clone(),
            preparation_id: pointer.preparation_id,
            recipe_version: pointer.recipe_version,
            snapshot: pointer.snapshot.clone(),
        })
        .collect();
    let mut steps = Vec::new();
    for (step, environment_id) in bindings {
        let pointer = pointers
            .iter()
            .find(|pointer| pointer.environment_id == environment_id)
            .ok_or(ResolveEnvironmentError::Missing)?;
        steps.push(ResolvedStepEnvironment {
            step,
            environment_id,
            preparation_id: pointer.preparation_id,
            snapshot_digest: pointer.snapshot.snapshot_digest.clone(),
        });
    }
    Ok(ResolvedEnvironmentSet {
        environments,
        steps,
    })
}

pub(crate) async fn validate_replacement_environment(
    catalogue: &EnvironmentCatalogue,
    snapshots: &EnvironmentSnapshotRepository,
    id: EnvironmentId,
) -> Result<(), ResolveEnvironmentError> {
    let pointer = copy_ready(catalogue, &id)?;
    snapshots
        .verify(&pointer.snapshot)
        .await
        .map_err(|_| ResolveEnvironmentError::Unavailable)?;
    if !catalogue.ready_pointer_matches(&pointer) {
        return Err(ResolveEnvironmentError::Changed);
    }
    Ok(())
}

fn copy_ready(
    catalogue: &EnvironmentCatalogue,
    id: &EnvironmentId,
) -> Result<ReadyPointer, ResolveEnvironmentError> {
    catalogue
        .copy_ready_pointer(id)
        .map_err(|error| match error {
            ReadyPointerError::Missing => ResolveEnvironmentError::Missing,
            ReadyPointerError::NotReady => ResolveEnvironmentError::NotReady,
        })
}

#[cfg(test)]
pub(in crate::workflows) mod tests;

pub(crate) fn alpine_git_id(
    catalogue: &crate::environments::EnvironmentCatalogue,
) -> Result<crate::environments::EnvironmentId, ResolveEnvironmentError> {
    catalogue
        .seed_id(crate::environments::seeds::ALPINE_GIT_V1)
        .ok_or(ResolveEnvironmentError::Missing)
}
