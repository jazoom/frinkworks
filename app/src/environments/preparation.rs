use super::id::{EnvironmentId, PreparationId};
use super::recipe::EnvironmentRecipeVersion;
use super::snapshot::PreparedSnapshot;
use crate::storage::LogState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparationState {
    Queued,
    Preparing,
    Ready,
    Failed,
    Interrupted,
    Cancelled,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PreparationPhase {
    Waiting,
    CreatingGuest,
    RunningSetup,
    StoppingGuest,
    CreatingSnapshot,
    VerifyingSnapshot,
    RemovingGuest,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureCategory {
    RuntimeUnavailable,
    GuestCreate,
    SetupExit,
    SetupTimeout,
    GuestStop,
    SnapshotCreate,
    SnapshotIntegrity,
    SnapshotRemove,
    GuestRemove,
    CataloguePersist,
    ProcessRestarted,
    EnvironmentDeleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparationFailure {
    pub(crate) category: FailureCategory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparationLogRecord {
    pub(crate) captured_bytes: u64,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparationRecord {
    pub(crate) id: PreparationId,
    pub(crate) environment_id: EnvironmentId,
    pub(crate) ordinal: u64,
    pub(crate) environment_revision: u64,
    pub(crate) recipe_version: EnvironmentRecipeVersion,
    pub(crate) state: PreparationState,
    pub(crate) phase: PreparationPhase,
    pub(crate) requested_at_ms: u64,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) log: PreparationLogRecord,
    pub(crate) failure: Option<PreparationFailure>,
    pub(crate) snapshot: Option<PreparedSnapshot>,
}

impl PreparationState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "preparing" => Some(Self::Preparing),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            "interrupted" => Some(Self::Interrupted),
            "cancelled" => Some(Self::Cancelled),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Preparing)
    }
}

impl PreparationPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::CreatingGuest => "creating-guest",
            Self::RunningSetup => "running-setup",
            Self::StoppingGuest => "stopping-guest",
            Self::CreatingSnapshot => "creating-snapshot",
            Self::VerifyingSnapshot => "verifying-snapshot",
            Self::RemovingGuest => "removing-guest",
            Self::Finished => "finished",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "waiting" => Some(Self::Waiting),
            "creating-guest" => Some(Self::CreatingGuest),
            "running-setup" => Some(Self::RunningSetup),
            "stopping-guest" => Some(Self::StoppingGuest),
            "creating-snapshot" => Some(Self::CreatingSnapshot),
            "verifying-snapshot" => Some(Self::VerifyingSnapshot),
            "removing-guest" => Some(Self::RemovingGuest),
            "finished" => Some(Self::Finished),
            _ => None,
        }
    }

    pub(crate) fn log_line(self) -> &'static str {
        match self {
            Self::Waiting => "waiting\n",
            Self::CreatingGuest => "creating guest\n",
            Self::RunningSetup => "running setup\n",
            Self::StoppingGuest => "stopping guest\n",
            Self::CreatingSnapshot => "creating snapshot\n",
            Self::VerifyingSnapshot => "verifying snapshot\n",
            Self::RemovingGuest => "removing guest\n",
            Self::Finished => "finished\n",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::CreatingGuest => "Creating guest",
            Self::RunningSetup => "Running setup",
            Self::StoppingGuest => "Stopping guest",
            Self::CreatingSnapshot => "Creating snapshot",
            Self::VerifyingSnapshot => "Verifying snapshot",
            Self::RemovingGuest => "Removing guest",
            Self::Finished => "Finished",
        }
    }
}

impl FailureCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeUnavailable => "runtime-unavailable",
            Self::GuestCreate => "guest-create",
            Self::SetupExit => "setup-exit",
            Self::SetupTimeout => "setup-timeout",
            Self::GuestStop => "guest-stop",
            Self::SnapshotCreate => "snapshot-create",
            Self::SnapshotIntegrity => "snapshot-integrity",
            Self::SnapshotRemove => "snapshot-remove",
            Self::GuestRemove => "guest-remove",
            Self::CataloguePersist => "catalogue-persist",
            Self::ProcessRestarted => "process-restarted",
            Self::EnvironmentDeleted => "environment-deleted",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "runtime-unavailable" => Some(Self::RuntimeUnavailable),
            "guest-create" => Some(Self::GuestCreate),
            "setup-exit" => Some(Self::SetupExit),
            "setup-timeout" => Some(Self::SetupTimeout),
            "guest-stop" => Some(Self::GuestStop),
            "snapshot-create" => Some(Self::SnapshotCreate),
            "snapshot-integrity" => Some(Self::SnapshotIntegrity),
            "snapshot-remove" => Some(Self::SnapshotRemove),
            "guest-remove" => Some(Self::GuestRemove),
            "catalogue-persist" => Some(Self::CataloguePersist),
            "process-restarted" => Some(Self::ProcessRestarted),
            "environment-deleted" => Some(Self::EnvironmentDeleted),
            _ => None,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::RuntimeUnavailable => {
                "Frinkworks cannot find the microsandbox runtime. Install it, then try again."
            }
            Self::GuestCreate => "Frinkworks could not create the preparation guest. Try again.",
            Self::SetupExit => "The setup script exited with a non-zero code.",
            Self::SetupTimeout => "The setup script reached the time limit.",
            Self::GuestStop => "Frinkworks could not stop the preparation guest. Try again.",
            Self::SnapshotCreate => "Frinkworks could not create the snapshot. Try again.",
            Self::SnapshotIntegrity => "The snapshot failed integrity verification.",
            Self::SnapshotRemove => {
                "Frinkworks could not remove an unpublished snapshot. Try again."
            }
            Self::GuestRemove => "Frinkworks could not remove the preparation guest. Try again.",
            Self::CataloguePersist => {
                "Frinkworks could not store the preparation result. Try again."
            }
            Self::ProcessRestarted => "Preparation stopped because Frinkworks restarted.",
            Self::EnvironmentDeleted => "Preparation stopped because the environment was deleted.",
        }
    }
}

impl PreparationFailure {
    pub(crate) fn new(category: FailureCategory) -> Self {
        Self { category }
    }
}

impl PreparationLogRecord {
    pub(crate) fn empty() -> Self {
        Self {
            captured_bytes: 0,
            truncated: false,
        }
    }

    pub(crate) fn from_state(state: LogState) -> Self {
        Self {
            captured_bytes: state.captured_bytes,
            truncated: state.truncated,
        }
    }
}

impl PreparationRecord {
    pub(crate) fn queued(
        id: PreparationId,
        environment_id: EnvironmentId,
        ordinal: u64,
        environment_revision: u64,
        recipe_version: EnvironmentRecipeVersion,
        requested_at_ms: u64,
    ) -> Self {
        Self {
            id,
            environment_id,
            ordinal,
            environment_revision,
            recipe_version,
            state: PreparationState::Queued,
            phase: PreparationPhase::Waiting,
            requested_at_ms,
            started_at_ms: None,
            finished_at_ms: None,
            log: PreparationLogRecord::empty(),
            failure: None,
            snapshot: None,
        }
    }

    pub(crate) fn validate_combination(&self) -> bool {
        if self.ordinal == 0 {
            return false;
        }
        let timestamps_ok = match (self.started_at_ms, self.finished_at_ms) {
            (None, None) => true,
            (Some(started), None) => started >= self.requested_at_ms,
            (Some(started), Some(finished)) => {
                started >= self.requested_at_ms && finished >= started
            }
            (None, Some(finished)) => finished >= self.requested_at_ms,
        };
        if !timestamps_ok {
            return false;
        }
        match self.state {
            PreparationState::Queued => {
                self.phase == PreparationPhase::Waiting
                    && self.started_at_ms.is_none()
                    && self.finished_at_ms.is_none()
                    && self.failure.is_none()
                    && self.snapshot.is_none()
            }
            PreparationState::Preparing => {
                self.phase > PreparationPhase::Waiting
                    && self.phase < PreparationPhase::Finished
                    && self.started_at_ms.is_some()
                    && self.finished_at_ms.is_none()
                    && self.failure.is_none()
                    && self.snapshot.is_none()
            }
            PreparationState::Ready => {
                self.phase == PreparationPhase::Finished
                    && self.started_at_ms.is_some()
                    && self.finished_at_ms.is_some()
                    && self.failure.is_none()
                    && self.snapshot.is_some()
            }
            PreparationState::Failed | PreparationState::Interrupted => {
                self.phase != PreparationPhase::Waiting
                    && self.started_at_ms.is_some()
                    && self.finished_at_ms.is_some()
                    && self.failure.is_some()
                    && self.snapshot.is_none()
            }
            PreparationState::Cancelled | PreparationState::Superseded => {
                self.finished_at_ms.is_some() && self.failure.is_none() && self.snapshot.is_none()
            }
        }
    }
}

#[cfg(test)]
mod tests;
