use serde::{Deserialize, Serialize};

use crate::agents::{AccessMode, AgentId, ToolId};
use crate::conversations::ConversationId;
use crate::environments::snapshot::{OciManifestDigest, RecordedIntegrity, SnapshotArtifactKey};
use crate::environments::{
    EnvironmentId, EnvironmentRecipeVersion, PreparationId, PreparedSnapshot, SnapshotDigest,
};
use crate::providers::{ModelSelection, ProviderKind, ThinkingEffort};

use super::apply::{ApplyRoot, ApplyRootOutcome, ApplyTransaction, ApplyTransactionState};
use super::artefacts::{ArtefactRecord, ArtefactReference};
use super::capabilities::{
    AttemptCapabilities, CapabilityDirectory, DirectoryRole, NetworkCapability,
    PrimarySourceLocation,
};
use super::commit::{CommitRoot, CommitTransaction, CommitTransactionState};
use super::definition::{
    DefinitionFile, DefinitionVersion, InputKey, OutputKey, PinnedWorkflowDefinition, StepAction,
    StepDefinition, StepKey, WorkflowDefinition,
};
use super::gates::{GateRevision, HumanGateRecord, HumanGateState};
use super::id::{AttemptId, GateId, RunId, WorkflowId};
use super::input_context::AttemptContextPacket;
use super::resolve::{ResolvedEnvironment, ResolvedEnvironmentSet, ResolvedStepEnvironment};

pub(crate) const RUN_RECORD_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowRun {
    pub(crate) id: RunId,
    pub(crate) created_at_ms: u64,
    pub(crate) conversation_id: Option<ConversationId>,
    pub(crate) ownership_history: Vec<ConversationId>,
    pub(crate) pending_handoff: Option<super::handoff::PendingHandoff>,
    pub(crate) launch_brief: String,

    pub(crate) kind: RunKind,
    pub(crate) agent_id: Option<AgentId>,
    pub(crate) phase_models: Vec<PhaseModelSelection>,
    pub(crate) pinned: PinnedWorkflowDefinition,
    pub(crate) environments: ResolvedEnvironmentSet,
    pub(crate) state: RunState,
    pub(crate) source: RunSource,
    pub(crate) artefacts: Vec<ArtefactRecord>,
    pub(crate) attempts: Vec<AttemptRecord>,
    pub(crate) gates: Vec<HumanGateRecord>,
    pub(crate) revision_reservation: Option<RevisionReservation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunKind {
    Configured,
    QuickTask,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PhaseModelSelection {
    pub(crate) step: StepKey,
    pub(crate) selection: ModelSelection,
    pub(crate) instructions: String,
    pub(crate) preset: Option<PinnedPreset>,
    pub(crate) settings: Option<crate::execution::ExecutionSettings>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PinnedPreset {
    pub(crate) id: crate::presets::PresetId,
    pub(crate) revision: u32,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevisionReservation {
    pub(crate) attempt: AttemptId,
    pub(crate) gate: GateId,
    pub(crate) target: StepKey,
    pub(crate) decision: ArtefactReference,
    pub(crate) candidate: ArtefactReference,
    pub(crate) diff_base: ArtefactReference,
    pub(crate) feedback: String,
    pub(crate) started: bool,
}

impl RunKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::QuickTask => "quick-task",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "configured" => Some(Self::Configured),
            "quick-task" => Some(Self::QuickTask),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RunState {
    InitialisingSource,
    Ready {
        step: StepKey,
    },
    Active {
        step: StepKey,
        attempt: AttemptId,
    },
    Paused {
        step: StepKey,
        attempt: AttemptId,
        checkpoint: crate::conversations::CheckpointId,
    },
    AwaitingHuman {
        step: StepKey,
        gate: GateId,
    },
    RevisionRequested {
        step: StepKey,
        decision: ArtefactReference,
    },
    Escalated {
        step: StepKey,
        report: ArtefactReference,
        reason: EscalationReason,
    },
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RunSource {
    None,
    Pending,
    Captured { source: RunSourceState },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunSourceState {
    pub(crate) initial: ArtefactReference,
    pub(crate) accepted: ArtefactReference,
    pub(crate) observed: ObservedCandidate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ObservedCandidate {
    Exact { artefact: ArtefactReference },
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptArtefactInput {
    pub(crate) key: InputKey,
    pub(crate) artefact: ArtefactReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptArtefactOutput {
    pub(crate) key: OutputKey,
    pub(crate) artefact: ArtefactReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttemptState {
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EscalationReason {
    Blocked,
    AttemptLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReviewRoute {
    Approved,
    RevisionRequested,
    BlockedEscalation,
    AttemptLimitEscalation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptRecord {
    pub(crate) id: AttemptId,
    pub(crate) step: StepKey,
    pub(crate) ordinal: u32,
    pub(crate) action_kind: ActionKind,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) state: AttemptState,
    pub(crate) result: Option<AttemptResult>,
    pub(crate) review_route: Option<ReviewRoute>,
    pub(crate) inputs: Vec<AttemptArtefactInput>,
    pub(crate) outputs: Vec<AttemptArtefactOutput>,
    pub(crate) capabilities: AttemptCapabilities,
    pub(crate) sandbox: AttemptSandboxRecord,
    pub(crate) initial_context: Option<AttemptContextPacket>,
    pub(crate) cleanup: AttemptCleanupRecord,
    pub(crate) apply_transaction: Option<ApplyTransaction>,
    pub(crate) commit_transaction: Option<CommitTransaction>,
    pub(crate) direct_changes: Option<super::direct::DirectChanges>,
    pub(crate) continuation: Option<crate::conversations::CheckpointId>,
    pub(crate) paused_candidate: Option<crate::workflows::artefacts::ObjectHash>,
    pub(crate) paused_drafts: Vec<crate::conversations::PausedOutputDraft>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Agent,
    SystemCommand,
    HumanGate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttemptResult {
    Completed { outputs: Vec<String> },
    Failed { category: FailureCategory },
    Cancelled,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureCategory {
    Provider,
    Tool,
    Authority,
    Command,
    Operational,
    Definition,
    Cleanup,
    Assurance,
    Apply,
    Commit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptSandboxRecord {
    pub(crate) kind: AttemptSandboxKind,
    pub(crate) snapshot_digest: SnapshotDigest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttemptSandboxKind {
    IsolatedAttempt,
    FileApplication,
    HostExecution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum AttemptCleanupRecord {
    Pending,
    Complete,
    Orphaned {
        sandbox: bool,
        workspace: bool,
        journal: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransitionError {
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunRecordError {
    Corrupt,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct RunFile {
    record_version: u32,
    id: String,
    created_at_ms: u64,
    #[serde(deserialize_with = "crate::storage::required_option")]
    conversation_id: Option<String>,
    ownership_history: Vec<String>,
    pending_handoff: Option<super::handoff::PendingHandoff>,
    launch_brief: String,

    kind: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    agent_id: Option<String>,
    phase_models: Vec<PhaseModelFile>,
    workflow_id: Option<String>,
    version: String,
    definition: DefinitionFile,
    environments: ResolvedEnvironmentSetFile,
    state: RunStateFile,
    source: RunSourceFile,
    artefacts: Vec<ArtefactFile>,
    attempts: Vec<AttemptFile>,
    gates: Vec<HumanGateFile>,
    #[serde(default)]
    revision_reservation: Option<RevisionReservationFile>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum RunStateFile {
    InitialisingSource,
    Ready {
        step: String,
    },
    Active {
        step: String,
        attempt: String,
    },
    Paused {
        step: String,
        attempt: String,
        checkpoint: String,
    },
    AwaitingHuman {
        step: String,
        gate: String,
    },
    RevisionRequested {
        step: String,
        decision: ArtefactRefFile,
    },
    Escalated {
        step: String,
        report: ArtefactRefFile,
        reason: String,
    },
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PhaseModelFile {
    step: String,
    provider: String,
    model: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    thinking: Option<String>,
    instructions: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    preset: Option<PinnedPresetFile>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    settings: Option<crate::execution::ExecutionSettingsFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PinnedPresetFile {
    id: String,
    revision: u32,
    name: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct HumanGateFile {
    id: String,
    step: String,
    sequence: u32,
    revision: u64,
    opened_at_ms: u64,
    closed_at_ms: Option<u64>,
    candidate: ArtefactRefFile,
    diff_base: ArtefactRefFile,
    state: String,
    decision: Option<ArtefactRefFile>,
    output: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RevisionReservationFile {
    attempt: String,
    gate: String,
    target: String,
    decision: ArtefactRefFile,
    candidate: ArtefactRefFile,
    diff_base: ArtefactRefFile,
    feedback: String,
    started: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct AttemptFile {
    id: String,
    step: String,
    ordinal: u32,
    action_kind: String,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
    state: String,
    result: Option<AttemptResultFile>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    review_route: Option<String>,
    inputs: Vec<AttemptInputFile>,
    outputs: Vec<AttemptOutputFile>,
    capabilities: AttemptCapabilitiesFile,
    sandbox: AttemptSandboxFile,
    initial_context: Option<AttemptContextPacket>,
    cleanup: AttemptCleanupFile,
    #[serde(deserialize_with = "crate::storage::required_option")]
    apply_transaction: Option<ApplyTransactionFile>,
    commit_transaction: Option<CommitTransactionFile>,
    direct_changes: Option<super::direct::DirectChanges>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paused_candidate: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    paused_drafts: Vec<PausedDraftFile>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PausedDraftFile {
    key: String,
    kind: String,
    markdown: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ApplyTransactionFile {
    state: String,
    completed: Option<usize>,
    path: Option<String>,
    roots: Vec<ApplyRootFile>,
    baseline: ArtefactRefFile,
    candidate: ArtefactRefFile,
    approval: ArtefactRefFile,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ApplyRootFile {
    grant_id: String,
    alias: String,
    host_path: std::path::PathBuf,
    device: u64,
    inode: u64,
    baseline_candidate: String,
    candidate_hash: String,
    exclusions: Vec<String>,
    outcome: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommitTransactionFile {
    candidate: ArtefactRefFile,
    reviews: Vec<ArtefactRefFile>,
    approval: Option<ArtefactRefFile>,
    roots: Vec<CommitRootFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommitRootFile {
    grant_id: String,
    alias: String,
    host_path: std::path::PathBuf,
    identity: crate::execution::CanonicalDirectoryIdentity,
    state: String,
    expected_reference: String,
    old_object: Option<String>,
    target_tree: Option<String>,
    expected_commit: Option<String>,
    timestamp: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct AttemptCapabilitiesFile {
    schema: u32,
    agent_revision: u32,
    tools: Vec<String>,
    directories: Vec<CapabilityDirectoryFile>,
    git_admin: String,
    source_location: String,
    network: String,
    network_domains: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CapabilityDirectoryFile {
    alias: String,
    guest_path: String,
    access: String,
    role: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AttemptSandboxFile {
    kind: String,
    snapshot_digest: String,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum AttemptCleanupFile {
    Pending,
    Complete,
    Orphaned {
        sandbox: bool,
        workspace: bool,
        journal: bool,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AttemptInputFile {
    key: String,
    artefact: ArtefactRefFile,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AttemptOutputFile {
    key: String,
    artefact: ArtefactRefFile,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArtefactRefFile {
    id: String,
    kind: String,
    artefact_hash: String,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
enum RunSourceFile {
    None,
    Pending,
    Captured { source: RunSourceStateFile },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct RunSourceStateFile {
    initial: ArtefactRefFile,
    accepted: ArtefactRefFile,
    observed: ObservedCandidateFile,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum ObservedCandidateFile {
    Exact { artefact: ArtefactRefFile },
    Unknown,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArtefactFile {
    id: String,
    kind: String,
    artefact_hash: String,
    object_hash: String,
    payload_bytes: u64,
    created_at_ms: u64,
    provenance: ProvenanceFile,
    summary: SummaryFile,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ProvenanceFile {
    run_id: String,
    producer: ProducerFile,
    inputs: Vec<ArtefactRefFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "producer", rename_all = "kebab-case")]
enum ProducerFile {
    RunSourceCapture,
    StepAttempt {
        attempt_id: String,
        step: String,
        output: Option<String>,
        disposition: String,
    },
    HumanGate {
        gate_id: String,
        step: String,
        output: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum SummaryFile {
    Plan {
        markdown_bytes: u64,
    },
    Review {
        candidate: String,
        verdict: String,
    },
    Test {
        candidate: String,
        outcome: String,
    },
    Candidate {
        candidate: String,
        entries: u64,
        bytes: u64,
        disposition: String,
    },
    HumanDecision {
        candidate: String,
        diff_base: String,
        decision: String,
    },
    PlanDecision {
        plan: String,
        decision: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum AttemptResultFile {
    Completed { outputs: Vec<String> },
    Failed { category: String },
    Cancelled,
    Interrupted,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ResolvedEnvironmentSetFile {
    environments: Vec<ResolvedEnvironmentFile>,
    steps: Vec<ResolvedStepEnvironmentFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolvedEnvironmentFile {
    environment_id: String,
    name: String,
    preparation_id: String,
    recipe_version: String,
    snapshot: PinnedSnapshotFile,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolvedStepEnvironmentFile {
    step: String,
    environment_id: String,
    preparation_id: String,
    snapshot_digest: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct PinnedSnapshotFile {
    artifact_key: String,
    snapshot_digest: String,
    image_reference: String,
    image_manifest_digest: String,
    upper_integrity: PinnedIntegrityFile,
    upper_size_bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct PinnedIntegrityFile {
    algorithm: String,
    value: String,
}

impl WorkflowRun {
    #[cfg(test)]
    pub(crate) fn create(
        id: RunId,
        created_at_ms: u64,
        agent_id: Option<AgentId>,
        kind: RunKind,
        pinned: PinnedWorkflowDefinition,
        environments: ResolvedEnvironmentSet,
    ) -> Self {
        let step = pinned.definition.first_step().clone();
        Self {
            id,
            created_at_ms,
            conversation_id: None,
            ownership_history: Vec::new(),
            pending_handoff: None,
            launch_brief: String::new(),

            kind,
            agent_id,
            phase_models: Vec::new(),
            pinned,
            environments,
            state: RunState::Ready { step },
            source: RunSource::Pending,
            artefacts: Vec::new(),
            attempts: Vec::new(),
            gates: Vec::new(),
            revision_reservation: None,
        }
    }

    pub(crate) fn directory_settings(&self) -> Option<crate::execution::ExecutionSettings> {
        crate::execution::ExecutionSettings::combined(
            self.phase_models
                .iter()
                .filter_map(|phase| phase.settings.as_ref()),
        )
    }

    pub(crate) fn phase_settings(
        &self,
        step: &crate::workflows::definition::StepKey,
    ) -> Option<&crate::execution::ExecutionSettings> {
        self.phase_models
            .iter()
            .find(|phase| phase.step == *step)
            .and_then(|phase| phase.settings.as_ref())
    }

    pub(crate) fn has_direct_writes(&self) -> bool {
        self.phase_models
            .iter()
            .filter_map(|phase| phase.settings.as_ref())
            .filter(|settings| settings.location == crate::execution::ToolLocation::Sandbox)
            .any(|settings| {
                settings
                    .directories
                    .iter()
                    .any(|grant| grant.access == crate::execution::DirectoryAccess::DirectWrite)
            })
    }

    pub(crate) fn reviewed_directories(&self) -> Vec<crate::execution::DirectoryGrant> {
        let mut grants = Vec::new();
        for phase in &self.phase_models {
            let Some(settings) = phase.settings.as_ref() else {
                continue;
            };
            for grant in &settings.directories {
                if grant.access != crate::execution::DirectoryAccess::ReviewBeforeApply {
                    continue;
                }
                if grants
                    .iter()
                    .any(|item: &crate::execution::DirectoryGrant| item.identity == grant.identity)
                {
                    continue;
                }
                grants.push(grant.clone());
            }
        }
        grants
    }

    pub(crate) fn create_source_free_for_conversation(
        id: RunId,
        created_at_ms: u64,
        conversation_id: ConversationId,
        pinned: PinnedWorkflowDefinition,
        environments: ResolvedEnvironmentSet,
        phase_models: Vec<PhaseModelSelection>,
    ) -> Self {
        let step = pinned.definition.first_step().clone();
        let captures_source = pinned.definition.steps().iter().any(|step| {
            step.inputs.iter().any(|input| {
                input.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
            })
        });
        Self {
            id,
            created_at_ms,
            conversation_id: Some(conversation_id),
            ownership_history: Vec::new(),
            pending_handoff: None,
            launch_brief: String::new(),

            kind: RunKind::QuickTask,
            agent_id: None,
            phase_models,
            pinned,
            environments,
            state: RunState::Ready { step },
            source: if captures_source {
                RunSource::Pending
            } else {
                RunSource::None
            },
            artefacts: Vec::new(),
            attempts: Vec::new(),
            gates: Vec::new(),
            revision_reservation: None,
        }
    }

    pub(crate) fn record_initial_candidate(
        &mut self,
        record: crate::workflows::artefacts::ArtefactRecord,
    ) -> Result<(), TransitionError> {
        if !matches!(self.source, RunSource::Pending) {
            return Err(TransitionError::Invalid);
        }
        if record.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
            || record.provenance.run_id != self.id
            || self.artefacts.iter().any(|item| item.id == record.id)
        {
            return Err(TransitionError::Invalid);
        }
        let reference = ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        };
        self.artefacts.push(record);
        self.source = RunSource::Captured {
            source: RunSourceState {
                initial: reference.clone(),
                accepted: reference.clone(),
                observed: ObservedCandidate::Exact {
                    artefact: reference,
                },
            },
        };
        Ok(())
    }

    pub(crate) fn phase_model(&self, step: &StepKey) -> Option<&PhaseModelSelection> {
        self.phase_models
            .iter()
            .find(|selection| selection.step == *step)
    }

    pub(crate) fn model_phases(&self) -> impl Iterator<Item = &PhaseModelSelection> {
        self.phase_models.iter()
    }

    pub(crate) fn artefact(
        &self,
        id: &crate::workflows::id::ArtefactId,
    ) -> Option<&crate::workflows::artefacts::ArtefactRecord> {
        self.artefacts.iter().find(|record| record.id == *id)
    }

    pub(crate) fn observed_candidate_hash(
        &self,
    ) -> Option<crate::workflows::artefacts::CandidateHash> {
        let RunSource::Captured { source } = &self.source else {
            return None;
        };
        let ObservedCandidate::Exact { artefact } = &source.observed else {
            return None;
        };
        self.artefact(&artefact.id)
            .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
    }

    pub(crate) fn completed_without_changes(&self) -> bool {
        self.state == RunState::Completed
            && self
                .pinned
                .definition
                .steps()
                .iter()
                .any(|step| unchanged_task_gate(self, &step.key))
    }

    pub(crate) fn complete_unchanged_task(&mut self) -> Result<(), TransitionError> {
        let RunState::Ready { step } = &self.state else {
            return Err(TransitionError::Invalid);
        };
        if !unchanged_task_gate(self, step) {
            return Err(TransitionError::Invalid);
        }
        self.state = RunState::Completed;
        Ok(())
    }

    pub(crate) fn open_gate(
        &mut self,
        gate_id: GateId,
        candidate: ArtefactReference,
        diff_base: ArtefactReference,
        at_ms: u64,
    ) -> Result<HumanGateRecord, TransitionError> {
        if !self.accepts_time(at_ms) || self.gates.iter().any(|gate| gate.id == gate_id) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Ready { step } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        let definition = self
            .pinned
            .definition
            .step(&step)
            .ok_or(TransitionError::Invalid)?;
        let StepAction::HumanGate(action) = &definition.action else {
            return Err(TransitionError::Invalid);
        };
        if action.is_plan_checkpoint()
            || candidate.kind != super::definition::ArtefactKind::CandidateRevision
            || diff_base.kind != super::definition::ArtefactKind::CandidateRevision
        {
            return Err(TransitionError::Invalid);
        }
        let sequence = u32::try_from(self.gates.len() + 1).map_err(|_| TransitionError::Invalid)?;
        let revision = GateRevision::new(u64::from(sequence)).ok_or(TransitionError::Invalid)?;
        let gate = HumanGateRecord {
            id: gate_id,
            step: step.clone(),
            sequence,
            revision,
            opened_at_ms: at_ms,
            closed_at_ms: None,
            candidate,
            diff_base,
            state: HumanGateState::AwaitingDecision,
            decision: None,
            output: action.required_output.key.clone(),
        };
        self.gates.push(gate.clone());
        self.state = RunState::AwaitingHuman {
            step,
            gate: gate_id,
        };
        Ok(gate)
    }

    pub(crate) fn open_plan_gate(
        &mut self,
        gate_id: GateId,
        plan: ArtefactReference,
        at_ms: u64,
    ) -> Result<HumanGateRecord, TransitionError> {
        if !self.accepts_time(at_ms) || self.gates.iter().any(|gate| gate.id == gate_id) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Ready { step } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        let definition = self
            .pinned
            .definition
            .step(&step)
            .ok_or(TransitionError::Invalid)?;
        let StepAction::HumanGate(action) = &definition.action else {
            return Err(TransitionError::Invalid);
        };
        if !action.is_plan_checkpoint()
            || plan.kind != super::definition::ArtefactKind::Plan
            || definition
                .inputs
                .iter()
                .filter(|input| input.kind == super::definition::ArtefactKind::Plan)
                .count()
                != 1
            || self
                .artefact(&plan.id)
                .is_none_or(|record| !artefact_matches_reference(record, &plan))
        {
            return Err(TransitionError::Invalid);
        }
        let sequence = u32::try_from(self.gates.len() + 1).map_err(|_| TransitionError::Invalid)?;
        let revision = GateRevision::new(u64::from(sequence)).ok_or(TransitionError::Invalid)?;
        let gate = HumanGateRecord {
            id: gate_id,
            step: step.clone(),
            sequence,
            revision,
            opened_at_ms: at_ms,
            closed_at_ms: None,
            candidate: plan.clone(),
            diff_base: plan,
            state: HumanGateState::AwaitingDecision,
            decision: None,
            output: action.required_output.key.clone(),
        };
        self.gates.push(gate.clone());
        self.state = RunState::AwaitingHuman {
            step,
            gate: gate_id,
        };
        Ok(gate)
    }

    pub(crate) fn human_revision_policy(
        &self,
        step: &StepKey,
    ) -> Option<&crate::workflows::definition::HumanRevisionPolicy> {
        match &self.pinned.definition.step(step)?.action {
            StepAction::HumanGate(action) => action.revision.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn human_revision_count(&self, step: &StepKey) -> usize {
        self.gates
            .iter()
            .filter(|gate| gate.step == *step && gate.state == HumanGateState::RevisionRequested)
            .count()
    }

    pub(crate) fn current_plan(&self) -> Option<ArtefactReference> {
        // A revision starts from the rejected plan, not a later unrelated plan output.
        if let Some(reservation) = &self.revision_reservation
            && reservation.candidate.kind == super::definition::ArtefactKind::Plan
        {
            return Some(reservation.candidate.clone());
        }
        self.artefacts.iter().rev().find_map(|record| {
            (record.kind == super::definition::ArtefactKind::Plan).then_some(ArtefactReference {
                id: record.id,
                kind: record.kind,
                artefact_hash: record.artefact_hash,
            })
        })
    }

    pub(crate) fn revision_feedback(&self, step: &StepKey) -> Option<&str> {
        self.revision_reservation
            .as_ref()
            .filter(|reservation| reservation.target == *step)
            .map(|reservation| reservation.feedback.as_str())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decide_gate(
        &mut self,
        gate_id: GateId,
        revision: GateRevision,
        record: ArtefactRecord,
        kind: super::gates::HumanDecisionKind,
        feedback: Option<String>,
        reserved_attempt: Option<AttemptId>,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms)
            || record.provenance.run_id != self.id
            || self.artefacts.iter().any(|item| item.id == record.id)
        {
            return Err(TransitionError::Invalid);
        }
        let RunState::AwaitingHuman { step, gate } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if gate != gate_id || record.kind != super::definition::ArtefactKind::HumanDecision {
            return Err(TransitionError::Invalid);
        }
        let gate_record = self
            .gates
            .iter()
            .find(|item| item.id == gate_id)
            .ok_or(TransitionError::Invalid)?;
        if gate_record.state != HumanGateState::AwaitingDecision || gate_record.revision != revision
        {
            return Err(TransitionError::Invalid);
        }
        let gate_definition = self
            .pinned
            .definition
            .step(&step)
            .ok_or(TransitionError::Invalid)?;
        if matches!(
            &gate_definition.action,
            StepAction::HumanGate(action) if action.is_plan_checkpoint()
        ) {
            return Err(TransitionError::Invalid);
        }
        let reference = ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        };
        let next_state = match kind {
            super::gates::HumanDecisionKind::Approved => {
                if feedback.is_some() || reserved_attempt.is_some() {
                    return Err(TransitionError::Invalid);
                }
                let definition = self
                    .pinned
                    .definition
                    .step(&step)
                    .ok_or(TransitionError::Invalid)?;
                if definition.review.is_some() {
                    return Err(TransitionError::Invalid);
                }
                advanced_state(&self.pinned.definition, &step)?
            }
            super::gates::HumanDecisionKind::RevisionRequested => {
                let (revision_target, attempt_limit) = {
                    let policy = self
                        .human_revision_policy(&step)
                        .ok_or(TransitionError::Invalid)?;
                    (policy.revision_target.clone(), policy.attempt_limit)
                };
                let attempt = reserved_attempt.ok_or(TransitionError::Invalid)?;
                let feedback = feedback
                    .filter(|feedback| !feedback.is_empty())
                    .ok_or(TransitionError::Invalid)?;
                if self.revision_reservation.is_some() {
                    return Err(TransitionError::Invalid);
                }
                if self.human_revision_count(&step) >= usize::from(attempt_limit.saturating_sub(1))
                {
                    RunState::Escalated {
                        step,
                        report: reference.clone(),
                        reason: EscalationReason::AttemptLimit,
                    }
                } else {
                    self.revision_reservation = Some(RevisionReservation {
                        attempt,
                        gate: gate_id,
                        target: revision_target.clone(),
                        decision: reference.clone(),
                        candidate: gate_record.candidate.clone(),
                        diff_base: gate_record.diff_base.clone(),
                        feedback,
                        started: false,
                    });
                    RunState::Ready {
                        step: revision_target,
                    }
                }
            }
        };
        let gate = self
            .gates
            .iter_mut()
            .find(|item| item.id == gate_id)
            .ok_or(TransitionError::Invalid)?;
        gate.closed_at_ms = Some(at_ms);
        gate.decision = Some(reference.clone());
        gate.state = match kind {
            super::gates::HumanDecisionKind::Approved => HumanGateState::Approved,
            super::gates::HumanDecisionKind::RevisionRequested => HumanGateState::RevisionRequested,
        };
        self.artefacts.push(record);
        self.state = next_state;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decide_plan_gate(
        &mut self,
        gate_id: GateId,
        revision: GateRevision,
        record: ArtefactRecord,
        kind: super::gates::PlanDecisionKind,
        feedback: Option<String>,
        reserved_attempt: Option<AttemptId>,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms)
            || record.provenance.run_id != self.id
            || self.artefacts.iter().any(|item| item.id == record.id)
        {
            return Err(TransitionError::Invalid);
        }
        let RunState::AwaitingHuman { step, gate } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if gate != gate_id || record.kind != super::definition::ArtefactKind::PlanDecision {
            return Err(TransitionError::Invalid);
        }
        let gate_record = self
            .gates
            .iter()
            .find(|item| item.id == gate_id)
            .ok_or(TransitionError::Invalid)?;
        if gate_record.state != HumanGateState::AwaitingDecision
            || gate_record.revision != revision
            || gate_record.candidate.kind != super::definition::ArtefactKind::Plan
            || gate_record.diff_base != gate_record.candidate
            || !matches!(
                &self.pinned.definition.step(&step).map(|step| &step.action),
                Some(StepAction::HumanGate(action)) if action.is_plan_checkpoint()
            )
            || !matches!(
                &record.summary,
                crate::workflows::artefacts::ArtefactSummary::PlanDecision {
                    plan,
                    decision,
                } if *plan == gate_record.candidate.artefact_hash && *decision == kind
            )
            || !matches!(
                &record.provenance.producer,
                crate::workflows::artefacts::ArtefactProducer::HumanGate {
                    gate_id,
                    step,
                    output,
                } if *gate_id == gate_record.id
                    && *step == gate_record.step
                    && *output == gate_record.output
            )
            || !record
                .provenance
                .inputs
                .iter()
                .any(|input| input == &gate_record.candidate)
        {
            return Err(TransitionError::Invalid);
        }
        let reference = ArtefactReference {
            id: record.id,
            kind: record.kind,
            artefact_hash: record.artefact_hash,
        };
        let next_state = match kind {
            super::gates::PlanDecisionKind::Accepted => {
                if feedback.is_some() || reserved_attempt.is_some() {
                    return Err(TransitionError::Invalid);
                }
                advanced_state(&self.pinned.definition, &step)?
            }
            super::gates::PlanDecisionKind::RevisionRequested => {
                let (revision_target, attempt_limit) = {
                    let policy = self
                        .human_revision_policy(&step)
                        .ok_or(TransitionError::Invalid)?;
                    (policy.revision_target.clone(), policy.attempt_limit)
                };
                let attempt = reserved_attempt.ok_or(TransitionError::Invalid)?;
                let feedback = feedback
                    .filter(|feedback| !feedback.is_empty())
                    .ok_or(TransitionError::Invalid)?;
                if self.revision_reservation.is_some() {
                    return Err(TransitionError::Invalid);
                }
                if self.human_revision_count(&step) >= usize::from(attempt_limit.saturating_sub(1))
                {
                    RunState::Escalated {
                        step,
                        report: reference.clone(),
                        reason: EscalationReason::AttemptLimit,
                    }
                } else {
                    self.revision_reservation = Some(RevisionReservation {
                        attempt,
                        gate: gate_id,
                        target: revision_target.clone(),
                        decision: reference.clone(),
                        candidate: gate_record.candidate.clone(),
                        diff_base: gate_record.diff_base.clone(),
                        feedback,
                        started: false,
                    });
                    RunState::Ready {
                        step: revision_target,
                    }
                }
            }
        };
        let gate = self
            .gates
            .iter_mut()
            .find(|item| item.id == gate_id)
            .ok_or(TransitionError::Invalid)?;
        gate.closed_at_ms = Some(at_ms);
        gate.decision = Some(reference.clone());
        gate.state = match kind {
            super::gates::PlanDecisionKind::Accepted => HumanGateState::Approved,
            super::gates::PlanDecisionKind::RevisionRequested => HumanGateState::RevisionRequested,
        };
        self.artefacts.push(record);
        self.state = next_state;
        Ok(())
    }

    pub(crate) fn cancel_gate(
        &mut self,
        gate_id: GateId,
        revision: GateRevision,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::AwaitingHuman { gate, .. } = self.state else {
            return Err(TransitionError::Invalid);
        };
        if gate != gate_id {
            return Err(TransitionError::Invalid);
        }
        let record = self
            .gates
            .iter_mut()
            .find(|item| item.id == gate_id)
            .ok_or(TransitionError::Invalid)?;
        if record.state != HumanGateState::AwaitingDecision || record.revision != revision {
            return Err(TransitionError::Invalid);
        }
        record.state = HumanGateState::Cancelled;
        record.closed_at_ms = Some(at_ms);
        self.state = RunState::Cancelled;
        Ok(())
    }

    pub(crate) fn start_attempt(
        &mut self,
        attempt_id: AttemptId,
        inputs: Vec<AttemptArtefactInput>,
        capabilities: AttemptCapabilities,
        sandbox: AttemptSandboxRecord,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Ready { step } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if self.attempts.iter().any(|attempt| attempt.id == attempt_id) {
            return Err(TransitionError::Invalid);
        }
        if self
            .attempts
            .iter()
            .any(|attempt| attempt.state == AttemptState::Active)
        {
            return Err(TransitionError::Invalid);
        }
        let Some(definition_step) = self.pinned.definition.step(&step) else {
            return Err(TransitionError::Invalid);
        };
        if let Some(reservation) = &self.revision_reservation
            && (reservation.started
                || reservation.attempt != attempt_id
                || reservation.target != step
                || inputs
                    .iter()
                    .find(|input| input.artefact.kind == reservation.candidate.kind)
                    .is_none_or(|input| input.artefact != reservation.candidate))
        {
            return Err(TransitionError::Invalid);
        }
        if !capabilities_match_step(self, &capabilities, definition_step) {
            return Err(TransitionError::Invalid);
        }
        let host_step = matches!(
            &definition_step.action,
            StepAction::Agent(action) if action.host_tools()
        );
        let apply_step = matches!(
            &definition_step.action,
            StepAction::SystemCommand(action)
                if action.command == crate::workflows::commands::SystemCommandId::ApplyChanges
        );
        let expected_kind = if host_step {
            AttemptSandboxKind::HostExecution
        } else if apply_step {
            AttemptSandboxKind::FileApplication
        } else {
            AttemptSandboxKind::IsolatedAttempt
        };
        if sandbox.kind != expected_kind {
            return Err(TransitionError::Invalid);
        }
        if !host_step {
            let expected = self
                .environments
                .steps
                .iter()
                .find(|item| item.step == step)
                .map(|item| &item.snapshot_digest);
            if expected != Some(&sandbox.snapshot_digest) {
                return Err(TransitionError::Invalid);
            }
        }
        if let Some(reservation) = &mut self.revision_reservation {
            reservation.started = true;
        }
        let ordinal = next_ordinal(&self.attempts, &step);
        self.attempts.push(AttemptRecord {
            id: attempt_id,
            step: step.clone(),
            ordinal,
            action_kind: ActionKind::from_action(&definition_step.action),
            started_at_ms: at_ms,
            finished_at_ms: None,
            state: AttemptState::Active,
            result: None,
            review_route: None,
            inputs,
            outputs: Vec::new(),
            capabilities,
            sandbox,
            initial_context: None,
            cleanup: AttemptCleanupRecord::Pending,
            apply_transaction: None,
            commit_transaction: None,
            direct_changes: None,
            continuation: None,
            paused_candidate: None,
            paused_drafts: Vec::new(),
        });
        self.state = RunState::Active {
            step,
            attempt: attempt_id,
        };
        Ok(())
    }

    pub(crate) fn record_initial_context(
        &mut self,
        attempt_id: AttemptId,
        context: AttemptContextPacket,
    ) -> Result<(), TransitionError> {
        if !context.validate() {
            return Err(TransitionError::Invalid);
        }
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id && attempt.state == AttemptState::Active)
        else {
            return Err(TransitionError::Invalid);
        };
        if !context_matches_attempt(&context, attempt) {
            return Err(TransitionError::Invalid);
        }
        if let Some(current) = &attempt.initial_context {
            return if current == &context {
                Ok(())
            } else {
                Err(TransitionError::Invalid)
            };
        }
        attempt.initial_context = Some(context);
        Ok(())
    }

    pub(crate) fn record_apply_transaction(
        &mut self,
        attempt_id: AttemptId,
        transaction: ApplyTransaction,
    ) -> Result<(), TransitionError> {
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id && attempt.state == AttemptState::Active)
        else {
            return Err(TransitionError::Invalid);
        };
        if let Some(current) = &attempt.apply_transaction
            && !current.can_advance_to(&transaction)
        {
            return Err(TransitionError::Invalid);
        }
        attempt.apply_transaction = Some(transaction);
        Ok(())
    }

    pub(crate) fn record_commit_transaction(
        &mut self,
        attempt_id: AttemptId,
        transaction: CommitTransaction,
    ) -> Result<(), TransitionError> {
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id && attempt.state == AttemptState::Active)
        else {
            return Err(TransitionError::Invalid);
        };
        if let Some(current) = &attempt.commit_transaction
            && !current.can_advance_to(&transaction)
        {
            return Err(TransitionError::Invalid);
        }
        attempt.commit_transaction = Some(transaction);
        Ok(())
    }

    pub(crate) fn record_cleanup(
        &mut self,
        attempt_id: AttemptId,
        cleanup: AttemptCleanupRecord,
    ) -> Result<(), TransitionError> {
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        else {
            return Err(TransitionError::Invalid);
        };
        if attempt.cleanup == cleanup && !matches!(cleanup, AttemptCleanupRecord::Pending) {
            return Ok(());
        }
        if !matches!(attempt.cleanup, AttemptCleanupRecord::Pending)
            || matches!(cleanup, AttemptCleanupRecord::Pending)
        {
            return Err(TransitionError::Invalid);
        }
        attempt.cleanup = cleanup;
        Ok(())
    }

    pub(crate) fn record_attempt_outputs(
        &mut self,
        attempt_id: AttemptId,
        artefacts: Vec<crate::workflows::artefacts::ArtefactRecord>,
        outputs: Vec<AttemptArtefactOutput>,
        accepted: Option<ArtefactReference>,
        observed: ObservedCandidate,
    ) -> Result<(), TransitionError> {
        if artefacts.len()
            > crate::workflows::artefacts::MAXIMUM_ARTEFACTS.saturating_sub(self.artefacts.len())
        {
            return Err(TransitionError::Invalid);
        }
        for (index, record) in artefacts.iter().enumerate() {
            if record.provenance.run_id != self.id
                || self.artefacts.iter().any(|item| item.id == record.id)
                || artefacts[..index].iter().any(|item| item.id == record.id)
            {
                return Err(TransitionError::Invalid);
            }
        }
        let reference_exists = |reference: &ArtefactReference| {
            self.artefacts
                .iter()
                .chain(&artefacts)
                .any(|record| artefact_matches_reference(record, reference))
        };
        if outputs
            .iter()
            .any(|output| !reference_exists(&output.artefact))
            || accepted
                .as_ref()
                .is_some_and(|item| !reference_exists(item))
            || matches!(&observed, ObservedCandidate::Exact { artefact } if !reference_exists(artefact))
        {
            return Err(TransitionError::Invalid);
        }
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|item| item.id == attempt_id && item.state == AttemptState::Active)
        else {
            return Err(TransitionError::Invalid);
        };
        attempt.outputs = outputs;
        self.artefacts.extend(artefacts);
        if let RunSource::Captured { source } = &mut self.source {
            source.observed = observed;
            if let Some(accepted) = accepted {
                source.accepted = accepted;
            }
        }
        Ok(())
    }

    pub(crate) fn complete_attempt(
        &mut self,
        attempt_id: AttemptId,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Active { step, attempt } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if attempt != attempt_id {
            return Err(TransitionError::Invalid);
        }
        let (review, expected_outputs, ready) = {
            let definition_step = self
                .pinned
                .definition
                .step(&step)
                .ok_or(TransitionError::Invalid)?;
            let Some(attempt) = self.attempts.iter().find(|item| item.id == attempt_id) else {
                return Err(TransitionError::Invalid);
            };
            (
                definition_step.review.clone(),
                required_output_keys(&definition_step.action),
                attempt.cleanup == AttemptCleanupRecord::Complete
                    && durable_outputs_match(attempt, definition_step),
            )
        };
        if !ready {
            return Err(TransitionError::Invalid);
        }
        let (review_route, next_state) = match review {
            None => (None, advanced_state(&self.pinned.definition, &step)?),
            Some(policy) => {
                let attempt = self
                    .attempts
                    .iter()
                    .find(|item| item.id == attempt_id)
                    .ok_or(TransitionError::Invalid)?;
                let (route, next_state) = review_consumption(
                    &self.pinned.definition,
                    &step,
                    &policy,
                    attempt,
                    &self.artefacts,
                )?;
                (Some(route), next_state)
            }
        };
        self.finish_attempt(
            attempt_id,
            at_ms,
            AttemptState::Completed,
            AttemptResult::Completed {
                outputs: expected_outputs,
            },
            review_route,
        )?;
        if self
            .revision_reservation
            .as_ref()
            .is_some_and(|reservation| reservation.attempt == attempt_id)
        {
            self.revision_reservation = None;
        }
        self.state = next_state;
        Ok(())
    }

    pub(crate) fn pause_attempt(
        &mut self,
        attempt_id: AttemptId,
        checkpoint: crate::conversations::CheckpointId,
        drafts: Vec<crate::conversations::PausedOutputDraft>,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Active { step, attempt } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if attempt != attempt_id {
            return Err(TransitionError::Invalid);
        }
        let Some(record) = self.attempts.iter_mut().find(|item| item.id == attempt_id) else {
            return Err(TransitionError::Invalid);
        };
        if record.state != AttemptState::Active
            || record.result.is_some()
            || record.cleanup != AttemptCleanupRecord::Complete
            || record.action_kind != ActionKind::Agent
        {
            return Err(TransitionError::Invalid);
        }
        record.state = AttemptState::Paused;
        record.continuation = Some(checkpoint);
        record.paused_drafts = drafts;
        self.state = RunState::Paused {
            step,
            attempt: attempt_id,
            checkpoint,
        };
        Ok(())
    }

    pub(crate) fn resume_paused(
        &mut self,
        attempt_id: AttemptId,
        checkpoint: crate::conversations::CheckpointId,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Paused {
            step,
            attempt,
            checkpoint: expected,
        } = self.state.clone()
        else {
            return Err(TransitionError::Invalid);
        };
        if attempt != attempt_id || expected != checkpoint {
            return Err(TransitionError::Invalid);
        }
        let Some(record) = self.attempts.iter_mut().find(|item| item.id == attempt_id) else {
            return Err(TransitionError::Invalid);
        };
        if record.state != AttemptState::Paused
            || record.continuation != Some(checkpoint)
            || record.cleanup != AttemptCleanupRecord::Complete
        {
            return Err(TransitionError::Invalid);
        }
        record.state = AttemptState::Active;
        record.cleanup = AttemptCleanupRecord::Pending;
        self.state = RunState::Active {
            step,
            attempt: attempt_id,
        };
        Ok(())
    }

    pub(crate) fn fail_before_attempt(&mut self, at_ms: u64) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        match self.state {
            RunState::Ready { .. } | RunState::InitialisingSource => {
                self.state = RunState::Failed;
                Ok(())
            }
            _ => Err(TransitionError::Invalid),
        }
    }

    pub(crate) fn fail_attempt(
        &mut self,
        attempt_id: AttemptId,
        category: FailureCategory,
        at_ms: u64,
    ) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        let RunState::Active { attempt, .. } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        if attempt != attempt_id {
            return Err(TransitionError::Invalid);
        }
        let Some(record) = self.attempts.iter().find(|item| item.id == attempt_id) else {
            return Err(TransitionError::Invalid);
        };
        if matches!(record.cleanup, AttemptCleanupRecord::Pending) {
            return Err(TransitionError::Invalid);
        }
        self.finish_attempt(
            attempt_id,
            at_ms,
            AttemptState::Failed,
            AttemptResult::Failed { category },
            None,
        )?;
        self.revision_reservation = None;
        self.state = RunState::Failed;
        Ok(())
    }

    pub(crate) fn cancel(&mut self, at_ms: u64) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        match self.state.clone() {
            RunState::Ready { .. } => {
                self.state = RunState::Cancelled;
                Ok(())
            }
            RunState::Active { attempt, .. } | RunState::Paused { attempt, .. } => {
                let Some(record) = self.attempts.iter().find(|item| item.id == attempt) else {
                    return Err(TransitionError::Invalid);
                };
                if matches!(record.cleanup, AttemptCleanupRecord::Pending) {
                    return Err(TransitionError::Invalid);
                }
                self.finish_attempt(
                    attempt,
                    at_ms,
                    AttemptState::Cancelled,
                    AttemptResult::Cancelled,
                    None,
                )?;
                self.revision_reservation = None;
                self.state = RunState::Cancelled;
                Ok(())
            }
            RunState::InitialisingSource => {
                self.state = RunState::Cancelled;
                Ok(())
            }
            RunState::AwaitingHuman { gate, .. } => {
                let revision = self
                    .gates
                    .iter()
                    .find(|item| item.id == gate)
                    .map(|item| item.revision)
                    .ok_or(TransitionError::Invalid)?;
                self.cancel_gate(gate, revision, at_ms)
            }
            RunState::Completed
            | RunState::RevisionRequested { .. }
            | RunState::Escalated { .. }
            | RunState::Failed
            | RunState::Cancelled
            | RunState::Interrupted => Err(TransitionError::Invalid),
        }
    }

    pub(crate) fn interrupt(&mut self, at_ms: u64) -> Result<(), TransitionError> {
        if !self.accepts_time(at_ms) {
            return Err(TransitionError::Invalid);
        }
        if matches!(self.state, RunState::InitialisingSource)
            || matches!(self.state, RunState::Ready { .. }) && self.revision_reservation.is_some()
        {
            self.revision_reservation = None;
            self.state = RunState::Interrupted;
            return Ok(());
        }
        if let RunState::AwaitingHuman { gate, .. } = self.state.clone() {
            let record = self
                .gates
                .iter_mut()
                .find(|item| item.id == gate)
                .ok_or(TransitionError::Invalid)?;
            if record.state != HumanGateState::AwaitingDecision {
                return Err(TransitionError::Invalid);
            }
            record.state = HumanGateState::Interrupted;
            record.closed_at_ms = Some(at_ms);
            self.state = RunState::Interrupted;
            return Ok(());
        }
        let RunState::Active { attempt, .. } = self.state.clone() else {
            return Err(TransitionError::Invalid);
        };
        self.finish_attempt(
            attempt,
            at_ms,
            AttemptState::Interrupted,
            AttemptResult::Interrupted,
            None,
        )?;
        self.revision_reservation = None;
        self.state = RunState::Interrupted;
        Ok(())
    }

    pub(crate) fn ready_step(&self) -> Option<&StepKey> {
        match &self.state {
            RunState::Ready { step } => Some(step),
            _ => None,
        }
    }

    pub(crate) fn active_attempt(&self) -> Option<AttemptId> {
        match self.state {
            RunState::Active { attempt, .. } | RunState::Paused { attempt, .. } => Some(attempt),
            _ => None,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.state,
            RunState::Active { .. } | RunState::AwaitingHuman { .. }
        ) || matches!(self.state, RunState::Ready { .. }) && self.revision_reservation.is_some()
    }

    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            RunState::Completed
                | RunState::RevisionRequested { .. }
                | RunState::Escalated { .. }
                | RunState::Failed
                | RunState::Cancelled
                | RunState::Interrupted
        )
    }

    /// The latest attempt that carries a file-application transaction, if any.
    /// Current work reads per-directory outcomes from this authoritative record.
    pub(crate) fn latest_apply_attempt(&self) -> Option<&AttemptRecord> {
        self.attempts
            .iter()
            .rev()
            .find(|attempt| attempt.apply_transaction.is_some())
    }

    /// Uncertain recovery blocks settlement, retry and continuation.
    pub(crate) fn apply_is_uncertain(&self) -> bool {
        self.attempts.iter().any(|attempt| {
            let Some(transaction) = attempt.apply_transaction.as_ref() else {
                return false;
            };
            transaction.state == ApplyTransactionState::RecoveryUncertain
                || !transaction.is_settled()
                || transaction.roots.iter().any(|root| {
                    matches!(
                        root.outcome,
                        ApplyRootOutcome::Pending | ApplyRootOutcome::Uncertain
                    )
                })
                || attempt.cleanup != AttemptCleanupRecord::Complete
        })
    }

    /// A known partial outcome shows the latest recovered transaction
    /// with every stored transaction settled and cleanup complete.
    pub(crate) fn apply_is_known_partial(&self) -> bool {
        let Some(latest) = self.latest_apply_attempt() else {
            return false;
        };
        let Some(transaction) = latest.apply_transaction.as_ref() else {
            return false;
        };
        transaction.state == ApplyTransactionState::Recovered && !self.apply_is_uncertain()
    }

    /// Successful completion shows the latest verified transaction
    /// over applied roots with every stored transaction settled.
    pub(crate) fn apply_is_complete(&self) -> bool {
        let Some(latest) = self.latest_apply_attempt() else {
            return false;
        };
        let Some(transaction) = latest.apply_transaction.as_ref() else {
            return false;
        };
        transaction.is_verified()
            && transaction.roots.iter().all(|root| {
                matches!(
                    root.outcome,
                    ApplyRootOutcome::Applied | ApplyRootOutcome::Unchanged
                )
            })
            && !self.apply_is_uncertain()
    }

    /// Settlement keeps applied files and ends ownership without another write.
    pub(crate) fn partial_settlement_eligible(&self) -> bool {
        !self.is_terminal() && self.apply_is_known_partial()
    }

    /// End a known partial task through cancellation, never completion.
    pub(crate) fn settle_known_partial(&mut self, at_ms: u64) -> Result<(), TransitionError> {
        if !self.partial_settlement_eligible() {
            return Err(TransitionError::Invalid);
        }
        self.cancel(at_ms)
    }

    pub(crate) fn current_step_name(&self) -> Option<&str> {
        let key = match &self.state {
            RunState::Ready { step }
            | RunState::Active { step, .. }
            | RunState::Paused { step, .. }
            | RunState::AwaitingHuman { step, .. }
            | RunState::RevisionRequested { step, .. }
            | RunState::Escalated { step, .. } => step,
            _ => self.attempts.last().map(|attempt| &attempt.step)?,
        };
        self.pinned
            .definition
            .step(key)
            .map(|step| step.name.as_str())
    }

    fn accepts_time(&self, at_ms: u64) -> bool {
        at_ms >= self.latest_fact_ms()
    }

    fn latest_fact_ms(&self) -> u64 {
        let mut latest = self.created_at_ms;
        for attempt in &self.attempts {
            latest = latest.max(attempt.started_at_ms);
            if let Some(finished) = attempt.finished_at_ms {
                latest = latest.max(finished);
            }
        }
        for gate in &self.gates {
            latest = latest.max(gate.opened_at_ms);
            if let Some(closed) = gate.closed_at_ms {
                latest = latest.max(closed);
            }
        }
        latest
    }

    fn finish_attempt(
        &mut self,
        attempt_id: AttemptId,
        at_ms: u64,
        state: AttemptState,
        result: AttemptResult,
        review_route: Option<ReviewRoute>,
    ) -> Result<(), TransitionError> {
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        else {
            return Err(TransitionError::Invalid);
        };
        if !matches!(attempt.state, AttemptState::Active | AttemptState::Paused)
            || attempt.result.is_some()
            || attempt.review_route.is_some()
        {
            return Err(TransitionError::Invalid);
        }
        attempt.state = state;
        attempt.finished_at_ms = Some(at_ms);
        attempt.result = Some(result);
        attempt.review_route = review_route;
        Ok(())
    }

    pub(super) fn to_file(&self) -> RunFile {
        RunFile {
            record_version: RUN_RECORD_VERSION,
            id: self.id.as_hex(),
            created_at_ms: self.created_at_ms,
            conversation_id: self
                .conversation_id
                .map(|conversation| conversation.as_hex()),
            ownership_history: self
                .ownership_history
                .iter()
                .map(|id| id.as_hex())
                .collect(),
            pending_handoff: self.pending_handoff.clone(),
            launch_brief: self.launch_brief.clone(),

            kind: self.kind.as_str().to_owned(),
            agent_id: self.agent_id.map(|id| id.as_hex()),
            phase_models: self.phase_models.iter().map(phase_model_to_file).collect(),
            workflow_id: self.pinned.workflow_id.map(|id| id.as_hex()),
            version: self.pinned.version.as_hex(),
            definition: self.pinned.definition.to_file(),
            environments: environment_set_to_file(&self.environments),
            state: RunStateFile::from_state(&self.state),
            source: source_to_file(&self.source),
            artefacts: self.artefacts.iter().map(artefact_to_file).collect(),
            attempts: self.attempts.iter().map(AttemptRecord::to_file).collect(),
            gates: self.gates.iter().map(gate_to_file).collect(),
            revision_reservation: self
                .revision_reservation
                .as_ref()
                .map(revision_reservation_to_file),
        }
    }

    pub(super) fn from_file(file: RunFile) -> Result<Self, RunRecordError> {
        if file.record_version != RUN_RECORD_VERSION || file.ownership_history.len() > 64 {
            return Err(RunRecordError::Corrupt);
        }
        let id = RunId::parse(&file.id).ok_or(RunRecordError::Corrupt)?;
        let conversation_id = match file.conversation_id.as_deref() {
            Some(value) => Some(ConversationId::parse(value).ok_or(RunRecordError::Corrupt)?),
            None => None,
        };
        let kind = RunKind::parse(&file.kind).ok_or(RunRecordError::Corrupt)?;
        let agent_id = match file.agent_id.as_deref() {
            Some(value) => Some(AgentId::parse(value).ok_or(RunRecordError::Corrupt)?),
            None => None,
        };
        let phase_models = file
            .phase_models
            .into_iter()
            .map(phase_model_from_file)
            .collect::<Result<Vec<_>, _>>()?;
        let workflow_id = match file.workflow_id {
            Some(value) => Some(WorkflowId::parse(&value).ok_or(RunRecordError::Corrupt)?),
            None => None,
        };
        let stored_version =
            DefinitionVersion::parse(&file.version).ok_or(RunRecordError::Corrupt)?;
        let definition =
            WorkflowDefinition::from_file(file.definition).map_err(|_| RunRecordError::Corrupt)?;
        let version = definition.version();
        if version != stored_version {
            return Err(RunRecordError::Corrupt);
        }
        let state = RunState::from_file(file.state)?;
        let attempts = file
            .attempts
            .into_iter()
            .map(AttemptRecord::from_file)
            .collect::<Result<Vec<_>, _>>()?;
        let gates = file
            .gates
            .into_iter()
            .map(gate_from_file)
            .collect::<Result<Vec<_>, _>>()?;
        let revision_reservation = file
            .revision_reservation
            .map(revision_reservation_from_file)
            .transpose()?;
        let environments = environment_set_from_file(file.environments)?;
        let source = source_from_file(file.source)?;
        let artefacts = file
            .artefacts
            .into_iter()
            .map(artefact_from_file)
            .collect::<Result<Vec<_>, _>>()?;
        let run = Self {
            id,
            created_at_ms: file.created_at_ms,
            conversation_id,
            ownership_history: file
                .ownership_history
                .iter()
                .map(|id| ConversationId::parse(id).ok_or(RunRecordError::Corrupt))
                .collect::<Result<Vec<_>, _>>()?,
            pending_handoff: file.pending_handoff,
            launch_brief: file.launch_brief,
            kind,
            agent_id,
            phase_models,
            pinned: PinnedWorkflowDefinition {
                workflow_id,
                version,
                definition,
            },
            environments,
            state,
            source,
            artefacts,
            attempts,
            gates,
            revision_reservation,
        };
        if run.ownership_history.iter().enumerate().any(|(index, id)| {
            Some(*id) == run.conversation_id || run.ownership_history[..index].contains(id)
        }) || run.pending_handoff.as_ref().is_some_and(|pending| {
            !pending.valid()
                || !run.recoverable_gate()
                || run
                    .ownership_history
                    .last()
                    .map(|id| id.as_hex())
                    .as_deref()
                    != Some(&pending.source)
        }) {
            return Err(RunRecordError::Corrupt);
        }
        run.validate_loaded()?;
        Ok(run)
    }

    fn validate_loaded(&self) -> Result<(), RunRecordError> {
        let fact_count = self
            .attempts
            .len()
            .checked_add(self.gates.len())
            .ok_or(RunRecordError::Corrupt)?;
        let source_free = matches!(self.source, RunSource::None);
        let reviewed_directory =
            matches!(self.source, RunSource::Pending | RunSource::Captured { .. })
                && self.conversation_id.is_some()
                && self.agent_id.is_none()
                && self.pinned.definition.steps().iter().any(|step| {
                    step.inputs.iter().any(|input| {
                        input.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
                    })
                });
        let desk = self.conversation_id.is_none() && self.agent_id.is_some();
        if (!source_free && !reviewed_directory && !desk)
            || (self.conversation_id.is_none() && self.agent_id.is_none())
            || (source_free
                && (self.agent_id.is_some()
                    || self.pinned.definition.steps().iter().any(|step| {
                        step.writes_primary_source()
                            || matches!(&step.action, StepAction::SystemCommand(_))
                            || step.inputs.iter().any(|input| {
                                input.kind
                                    == crate::workflows::definition::ArtefactKind::CandidateRevision
                            })
                            || step.required_outputs().iter().any(|output| {
                                output.kind
                                    == crate::workflows::definition::OutputKind::CandidateRevision
                            })
                    })
                    || self.attempts.iter().any(|attempt| {
                        attempt.capabilities.source_location
                            != PrimarySourceLocation::PrivateWorkspace
                    })))
            || (!source_free
                && self.attempts.iter().any(|attempt| {
                    attempt.capabilities.source_location == PrimarySourceLocation::PrivateWorkspace
                }))
            || fact_count > self.pinned.definition.attempt_bound()
            || self.artefacts.len() > crate::workflows::artefacts::MAXIMUM_ARTEFACTS
        {
            return Err(RunRecordError::Corrupt);
        }
        validate_artefacts(self)?;
        validate_attempts(self)?;
        validate_gates(self)?;
        validate_review_routes(self)?;
        validate_revision_reservation(self)?;
        validate_phase_models(self)?;
        validate_state_facts(self)?;
        validate_source(self)?;
        validate_environments(self)
    }
}

impl RunState {
    pub(crate) fn as_label(&self) -> &'static str {
        match self {
            Self::InitialisingSource => "Source capture",
            Self::Ready { .. } => "Ready",
            Self::Active { .. } => "Active",
            Self::Paused { .. } => "Paused",
            Self::AwaitingHuman { .. } => "Awaiting decision",
            Self::RevisionRequested { .. } => "Revision requested",
            Self::Escalated { reason, .. } => match reason {
                EscalationReason::Blocked => "Escalated (blocked)",
                EscalationReason::AttemptLimit => "Escalated (attempt limit)",
            },
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Interrupted => "Interrupted",
        }
    }

    fn from_file(file: RunStateFile) -> Result<Self, RunRecordError> {
        Ok(match file {
            RunStateFile::InitialisingSource => Self::InitialisingSource,
            RunStateFile::Ready { step } => Self::Ready {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
            },
            RunStateFile::Active { step, attempt } => Self::Active {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
                attempt: AttemptId::parse(&attempt).ok_or(RunRecordError::Corrupt)?,
            },
            RunStateFile::Paused {
                step,
                attempt,
                checkpoint,
            } => Self::Paused {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
                attempt: AttemptId::parse(&attempt).ok_or(RunRecordError::Corrupt)?,
                checkpoint: crate::conversations::CheckpointId::parse(&checkpoint)
                    .ok_or(RunRecordError::Corrupt)?,
            },
            RunStateFile::AwaitingHuman { step, gate } => Self::AwaitingHuman {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
                gate: GateId::parse(&gate).ok_or(RunRecordError::Corrupt)?,
            },
            RunStateFile::RevisionRequested { step, decision } => Self::RevisionRequested {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
                decision: ref_from_file(decision)?,
            },
            RunStateFile::Escalated {
                step,
                report,
                reason,
            } => Self::Escalated {
                step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
                report: ref_from_file(report)?,
                reason: EscalationReason::parse(&reason).ok_or(RunRecordError::Corrupt)?,
            },
            RunStateFile::Completed => Self::Completed,
            RunStateFile::Failed => Self::Failed,
            RunStateFile::Cancelled => Self::Cancelled,
            RunStateFile::Interrupted => Self::Interrupted,
        })
    }
}

impl RunStateFile {
    fn from_state(state: &RunState) -> Self {
        match state {
            RunState::InitialisingSource => Self::InitialisingSource,
            RunState::Ready { step } => Self::Ready {
                step: step.as_str().to_owned(),
            },
            RunState::Active { step, attempt } => Self::Active {
                step: step.as_str().to_owned(),
                attempt: attempt.as_hex(),
            },
            RunState::Paused {
                step,
                attempt,
                checkpoint,
            } => Self::Paused {
                step: step.as_str().to_owned(),
                attempt: attempt.as_hex(),
                checkpoint: checkpoint.as_hex(),
            },
            RunState::AwaitingHuman { step, gate } => Self::AwaitingHuman {
                step: step.as_str().to_owned(),
                gate: gate.as_hex(),
            },
            RunState::RevisionRequested { step, decision } => Self::RevisionRequested {
                step: step.as_str().to_owned(),
                decision: ref_to_file(decision),
            },
            RunState::Escalated {
                step,
                report,
                reason,
            } => Self::Escalated {
                step: step.as_str().to_owned(),
                report: ref_to_file(report),
                reason: reason.as_str().to_owned(),
            },
            RunState::Completed => Self::Completed,
            RunState::Failed => Self::Failed,
            RunState::Cancelled => Self::Cancelled,
            RunState::Interrupted => Self::Interrupted,
        }
    }
}

impl AttemptRecord {
    fn to_file(&self) -> AttemptFile {
        AttemptFile {
            id: self.id.as_hex(),
            step: self.step.as_str().to_owned(),
            ordinal: self.ordinal,
            action_kind: self.action_kind.as_str().to_owned(),
            started_at_ms: self.started_at_ms,
            finished_at_ms: self.finished_at_ms,
            state: self.state.as_str().to_owned(),
            result: self.result.as_ref().map(AttemptResult::to_file),
            review_route: self
                .review_route
                .map(ReviewRoute::as_str)
                .map(str::to_owned),
            inputs: self
                .inputs
                .iter()
                .map(|input| AttemptInputFile {
                    key: input.key.as_str().to_owned(),
                    artefact: ref_to_file(&input.artefact),
                })
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|output| AttemptOutputFile {
                    key: output.key.as_str().to_owned(),
                    artefact: ref_to_file(&output.artefact),
                })
                .collect(),
            capabilities: capabilities_to_file(&self.capabilities),
            sandbox: AttemptSandboxFile {
                kind: self.sandbox.kind.as_str().to_owned(),
                snapshot_digest: self.sandbox.snapshot_digest.as_str().to_owned(),
            },
            initial_context: self.initial_context.clone(),
            cleanup: cleanup_to_file(&self.cleanup),
            apply_transaction: self
                .apply_transaction
                .as_ref()
                .map(apply_transaction_to_file),
            commit_transaction: self
                .commit_transaction
                .as_ref()
                .map(commit_transaction_to_file),
            direct_changes: self.direct_changes.clone(),
            continuation: self.continuation.map(|id| id.as_hex()),
            paused_candidate: self.paused_candidate.map(|hash| hash.as_str()),
            paused_drafts: self
                .paused_drafts
                .iter()
                .map(|draft| PausedDraftFile {
                    key: draft.key.clone(),
                    kind: draft.kind.clone(),
                    markdown: draft.markdown.clone(),
                    verdict: draft.verdict.clone(),
                    outcome: draft.outcome.clone(),
                })
                .collect(),
        }
    }

    fn from_file(file: AttemptFile) -> Result<Self, RunRecordError> {
        if file.ordinal == 0
            || file
                .direct_changes
                .as_ref()
                .is_some_and(|changes| !changes.valid())
        {
            return Err(RunRecordError::Corrupt);
        }
        Ok(Self {
            id: AttemptId::parse(&file.id).ok_or(RunRecordError::Corrupt)?,
            step: StepKey::parse(&file.step).map_err(|_| RunRecordError::Corrupt)?,
            ordinal: file.ordinal,
            action_kind: ActionKind::parse(&file.action_kind).ok_or(RunRecordError::Corrupt)?,
            started_at_ms: file.started_at_ms,
            finished_at_ms: file.finished_at_ms,
            state: AttemptState::parse(&file.state).ok_or(RunRecordError::Corrupt)?,
            result: file.result.map(AttemptResult::from_file).transpose()?,
            review_route: match file.review_route {
                Some(value) => Some(ReviewRoute::parse(&value).ok_or(RunRecordError::Corrupt)?),
                None => None,
            },
            inputs: file
                .inputs
                .into_iter()
                .map(input_from_file)
                .collect::<Result<Vec<_>, _>>()?,
            outputs: file
                .outputs
                .into_iter()
                .map(output_from_file)
                .collect::<Result<Vec<_>, _>>()?,
            capabilities: capabilities_from_file(file.capabilities)?,
            sandbox: AttemptSandboxRecord {
                kind: AttemptSandboxKind::parse(&file.sandbox.kind)
                    .ok_or(RunRecordError::Corrupt)?,
                snapshot_digest: SnapshotDigest::parse(&file.sandbox.snapshot_digest)
                    .ok_or(RunRecordError::Corrupt)?,
            },
            initial_context: file.initial_context,
            cleanup: cleanup_from_file(file.cleanup)?,
            apply_transaction: file
                .apply_transaction
                .map(apply_transaction_from_file)
                .transpose()?,
            commit_transaction: file
                .commit_transaction
                .map(commit_transaction_from_file)
                .transpose()?,
            direct_changes: file.direct_changes,
            continuation: match file.continuation {
                Some(value) => Some(
                    crate::conversations::CheckpointId::parse(&value)
                        .ok_or(RunRecordError::Corrupt)?,
                ),
                None => None,
            },
            paused_candidate: file
                .paused_candidate
                .map(|value| {
                    crate::workflows::artefacts::ObjectHash::parse(&value)
                        .ok_or(RunRecordError::Corrupt)
                })
                .transpose()?,
            paused_drafts: file
                .paused_drafts
                .into_iter()
                .map(|draft| crate::conversations::PausedOutputDraft {
                    key: draft.key,
                    kind: draft.kind,
                    markdown: draft.markdown,
                    verdict: draft.verdict,
                    outcome: draft.outcome,
                })
                .collect(),
        })
    }
}

impl AttemptState {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Interrupted => "Interrupted",
        }
    }
}

impl AttemptResult {
    fn to_file(&self) -> AttemptResultFile {
        match self {
            Self::Completed { outputs } => AttemptResultFile::Completed {
                outputs: outputs.clone(),
            },
            Self::Failed { category } => AttemptResultFile::Failed {
                category: category.as_str().to_owned(),
            },
            Self::Cancelled => AttemptResultFile::Cancelled,
            Self::Interrupted => AttemptResultFile::Interrupted,
        }
    }

    fn from_file(file: AttemptResultFile) -> Result<Self, RunRecordError> {
        Ok(match file {
            AttemptResultFile::Completed { outputs } => Self::Completed { outputs },
            AttemptResultFile::Failed { category } => Self::Failed {
                category: FailureCategory::parse(&category).ok_or(RunRecordError::Corrupt)?,
            },
            AttemptResultFile::Cancelled => Self::Cancelled,
            AttemptResultFile::Interrupted => Self::Interrupted,
        })
    }

    pub(crate) fn as_label(&self) -> String {
        match self {
            Self::Completed { .. } => "Completed".to_owned(),
            Self::Failed { category } => format!("Failed ({})", category.as_label()),
            Self::Cancelled => "Cancelled".to_owned(),
            Self::Interrupted => "Interrupted".to_owned(),
        }
    }
}

impl FailureCategory {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "provider" => Some(Self::Provider),
            "tool" => Some(Self::Tool),
            "authority" => Some(Self::Authority),
            "command" => Some(Self::Command),
            "operational" => Some(Self::Operational),
            "definition" => Some(Self::Definition),
            "cleanup" => Some(Self::Cleanup),
            "assurance" => Some(Self::Assurance),
            "apply" => Some(Self::Apply),
            "commit" => Some(Self::Commit),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Tool => "tool",
            Self::Authority => "authority",
            Self::Command => "command",
            Self::Operational => "operational",
            Self::Definition => "definition",
            Self::Cleanup => "cleanup",
            Self::Assurance => "assurance",
            Self::Apply => "apply",
            Self::Commit => "commit",
        }
    }

    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Tool => "tool",
            Self::Authority => "authority",
            Self::Command => "command",
            Self::Operational => "operational",
            Self::Definition => "definition",
            Self::Cleanup => "cleanup",
            Self::Assurance => "assurance",
            Self::Apply => "file application",
            Self::Commit => "commit",
        }
    }
}

impl ActionKind {
    fn from_action(action: &StepAction) -> Self {
        match action {
            StepAction::Agent(_) => Self::Agent,
            StepAction::SystemCommand(_) => Self::SystemCommand,
            StepAction::HumanGate(_) => Self::HumanGate,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::Agent),
            "system-command" => Some(Self::SystemCommand),
            "human-gate" => Some(Self::HumanGate),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::SystemCommand => "system-command",
            Self::HumanGate => "human-gate",
        }
    }

    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::SystemCommand => "System command",
            Self::HumanGate => "Human gate",
        }
    }
}

impl EscalationReason {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "blocked" => Some(Self::Blocked),
            "attempt-limit" => Some(Self::AttemptLimit),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::AttemptLimit => "attempt-limit",
        }
    }
}

impl ReviewRoute {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "approved" => Some(Self::Approved),
            "revision-requested" => Some(Self::RevisionRequested),
            "blocked-escalation" => Some(Self::BlockedEscalation),
            "attempt-limit-escalation" => Some(Self::AttemptLimitEscalation),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::RevisionRequested => "revision-requested",
            Self::BlockedEscalation => "blocked-escalation",
            Self::AttemptLimitEscalation => "attempt-limit-escalation",
        }
    }

    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Approved => "Approved route",
            Self::RevisionRequested => "Revision route",
            Self::BlockedEscalation => "Blocked escalation",
            Self::AttemptLimitEscalation => "Attempt-limit escalation",
        }
    }
}

fn validate_phase_models(run: &WorkflowRun) -> Result<(), RunRecordError> {
    if (run.kind == RunKind::Configured && run.conversation_id.is_some()
        || !run.phase_models.is_empty())
        && run.phase_models.len()
            != run
                .pinned
                .definition
                .steps()
                .iter()
                .filter(|step| matches!(step.action, StepAction::Agent(_)))
                .count()
    {
        return Err(RunRecordError::Corrupt);
    }
    let mut seen = Vec::new();
    for selection in &run.phase_models {
        if !seen.iter().all(|step: &StepKey| step != &selection.step)
            || (selection.preset.is_some() && selection.settings.is_none())
            || (run.kind == RunKind::Configured && selection.settings.is_none())
            || selection.settings.as_ref().is_some_and(|settings| {
                settings.model != selection.selection
                    || settings.instructions != selection.instructions
                    || crate::execution::validate_directories(&settings.directories).is_err()
                    || run
                        .pinned
                        .definition
                        .step(&selection.step)
                        .is_none_or(|step| {
                            settings.environment
                                != run.pinned.definition.effective_environment(step)
                        })
            })
            || !matches!(
                run.pinned.definition.step(&selection.step),
                Some(StepDefinition {
                    action: StepAction::Agent(_),
                    ..
                })
            )
        {
            return Err(RunRecordError::Corrupt);
        }
        seen.push(selection.step.clone());
    }
    Ok(())
}

fn validate_gates(run: &WorkflowRun) -> Result<(), RunRecordError> {
    let mut open_gates = 0usize;
    for (index, gate) in run.gates.iter().enumerate() {
        if run.gates[..index]
            .iter()
            .any(|earlier| earlier.id == gate.id)
            || run.gates[..index]
                .iter()
                .any(|earlier| earlier.revision == gate.revision)
            || gate.sequence != u32::try_from(index + 1).map_err(|_| RunRecordError::Corrupt)?
            || gate.revision.get() != u64::from(gate.sequence)
            || gate.opened_at_ms < run.created_at_ms
        {
            return Err(RunRecordError::Corrupt);
        }
        let step = run
            .pinned
            .definition
            .step(&gate.step)
            .ok_or(RunRecordError::Corrupt)?;
        let StepAction::HumanGate(action) = &step.action else {
            return Err(RunRecordError::Corrupt);
        };
        let plan_gate = action.is_plan_checkpoint();
        if plan_gate {
            if gate.candidate.kind != crate::workflows::definition::ArtefactKind::Plan
                || gate.diff_base != gate.candidate
            {
                return Err(RunRecordError::Corrupt);
            }
        } else if gate.candidate.kind
            != crate::workflows::definition::ArtefactKind::CandidateRevision
            || gate.diff_base.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
        {
            return Err(RunRecordError::Corrupt);
        }
        let candidate = referenced_artefact(run, &gate.candidate)?;
        let diff_base = referenced_artefact(run, &gate.diff_base)?;
        if let Some(previous) = index.checked_sub(1).map(|previous| &run.gates[previous]) {
            let previous_closed = previous.closed_at_ms.ok_or(RunRecordError::Corrupt)?;
            if gate.opened_at_ms < previous_closed {
                return Err(RunRecordError::Corrupt);
            }
        }
        if gate.state == HumanGateState::AwaitingDecision {
            open_gates += 1;
            if index + 1 != run.gates.len() {
                return Err(RunRecordError::Corrupt);
            }
        }
        if gate.output != action.required_output.key {
            return Err(RunRecordError::Corrupt);
        }
        match gate.state {
            HumanGateState::AwaitingDecision => {
                if gate.closed_at_ms.is_some() || gate.decision.is_some() {
                    return Err(RunRecordError::Corrupt);
                }
            }
            HumanGateState::Approved | HumanGateState::RevisionRequested => {
                let decision = gate.decision.as_ref().ok_or(RunRecordError::Corrupt)?;
                let record = referenced_artefact(run, decision)?;
                let crate::workflows::artefacts::ArtefactProducer::HumanGate {
                    gate_id,
                    step,
                    output,
                } = &record.provenance.producer
                else {
                    return Err(RunRecordError::Corrupt);
                };
                if *gate_id != gate.id
                    || *step != gate.step
                    || *output != gate.output
                    || gate.closed_at_ms.is_none()
                {
                    return Err(RunRecordError::Corrupt);
                }
                if plan_gate {
                    let expected = if gate.state == HumanGateState::Approved {
                        super::gates::PlanDecisionKind::Accepted
                    } else {
                        super::gates::PlanDecisionKind::RevisionRequested
                    };
                    if decision.kind != crate::workflows::definition::ArtefactKind::PlanDecision
                        || !matches!(
                            &record.summary,
                            crate::workflows::artefacts::ArtefactSummary::PlanDecision {
                                plan,
                                decision,
                            } if *decision == expected
                                && *plan == candidate.artefact_hash
                                && record
                                    .provenance
                                    .inputs
                                    .iter()
                                    .any(|input| input == &gate.candidate)
                        )
                    {
                        return Err(RunRecordError::Corrupt);
                    }
                } else {
                    let expected = if gate.state == HumanGateState::Approved {
                        super::gates::HumanDecisionKind::Approved
                    } else {
                        super::gates::HumanDecisionKind::RevisionRequested
                    };
                    if decision.kind != crate::workflows::definition::ArtefactKind::HumanDecision
                        || !matches!(
                            &record.summary,
                            crate::workflows::artefacts::ArtefactSummary::HumanDecision {
                                candidate: decision_candidate,
                                diff_base: decision_diff_base,
                                decision,
                            } if *decision == expected
                                && Some(*decision_candidate) == candidate.candidate_hash()
                                && Some(*decision_diff_base) == diff_base.candidate_hash()
                        )
                    {
                        return Err(RunRecordError::Corrupt);
                    }
                }
            }
            HumanGateState::Cancelled | HumanGateState::Interrupted => {
                if gate.closed_at_ms.is_none() || gate.decision.is_some() {
                    return Err(RunRecordError::Corrupt);
                }
            }
        }
        if let Some(closed) = gate.closed_at_ms
            && closed < gate.opened_at_ms
        {
            return Err(RunRecordError::Corrupt);
        }
    }
    if open_gates > 1 {
        return Err(RunRecordError::Corrupt);
    }
    Ok(())
}

fn validate_environments(run: &WorkflowRun) -> Result<(), RunRecordError> {
    let set = &run.environments;
    let mut seen = Vec::new();
    for environment in &set.environments {
        if seen.contains(&environment.environment_id) {
            return Err(RunRecordError::Corrupt);
        }
        seen.push(environment.environment_id);
    }
    let mut bound_steps = Vec::new();
    for binding in &set.steps {
        if bound_steps.contains(&binding.step) {
            return Err(RunRecordError::Corrupt);
        }
        let stored = set
            .environments
            .iter()
            .find(|environment| environment.environment_id == binding.environment_id)
            .ok_or(RunRecordError::Corrupt)?;
        if stored.preparation_id != binding.preparation_id
            || stored.snapshot.snapshot_digest != binding.snapshot_digest
        {
            return Err(RunRecordError::Corrupt);
        }
        let step = run
            .pinned
            .definition
            .step(&binding.step)
            .ok_or(RunRecordError::Corrupt)?;
        if !step.is_sandbox_backed() {
            return Err(RunRecordError::Corrupt);
        }
        bound_steps.push(binding.step.clone());
    }
    for step in run.pinned.definition.steps() {
        if step.is_sandbox_backed() && !bound_steps.iter().any(|key| key == &step.key) {
            return Err(RunRecordError::Corrupt);
        }
    }
    Ok(())
}

fn phase_model_to_file(selection: &PhaseModelSelection) -> PhaseModelFile {
    PhaseModelFile {
        step: selection.step.as_str().to_owned(),
        provider: selection.selection.provider.as_str().to_owned(),
        model: selection.selection.model.clone(),
        thinking: selection
            .selection
            .thinking
            .as_ref()
            .map(|effort| effort.as_str().to_owned()),
        instructions: selection.instructions.clone(),
        preset: selection.preset.as_ref().map(|preset| PinnedPresetFile {
            id: preset.id.as_hex(),
            revision: preset.revision,
            name: preset.name.clone(),
        }),
        settings: selection
            .settings
            .as_ref()
            .map(crate::execution::ExecutionSettings::to_file),
    }
}

fn phase_model_from_file(file: PhaseModelFile) -> Result<PhaseModelSelection, RunRecordError> {
    let provider = ProviderKind::parse(&file.provider).ok_or(RunRecordError::Corrupt)?;
    let thinking = file
        .thinking
        .map(|value| ThinkingEffort::new(value).ok_or(RunRecordError::Corrupt))
        .transpose()?;
    let selection =
        ModelSelection::new(provider, file.model, thinking).ok_or(RunRecordError::Corrupt)?;
    if file.instructions.len() > crate::agents::MAXIMUM_INSTRUCTION_BYTES
        || file
            .instructions
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(RunRecordError::Corrupt);
    }
    let preset = file
        .preset
        .map(|preset| {
            Ok(PinnedPreset {
                id: crate::presets::PresetId::parse(&preset.id).ok_or(RunRecordError::Corrupt)?,
                revision: preset.revision,
                name: preset.name,
            })
        })
        .transpose()?;
    Ok(PhaseModelSelection {
        step: StepKey::parse(&file.step).map_err(|_| RunRecordError::Corrupt)?,
        selection,
        instructions: file.instructions,
        preset,
        settings: match file.settings {
            Some(settings) => Some(
                crate::execution::ExecutionSettings::from_file(settings)
                    .ok_or(RunRecordError::Corrupt)?,
            ),
            None => None,
        },
    })
}

pub(crate) fn environment_set_to_file(set: &ResolvedEnvironmentSet) -> ResolvedEnvironmentSetFile {
    ResolvedEnvironmentSetFile {
        environments: set
            .environments
            .iter()
            .map(|environment| ResolvedEnvironmentFile {
                environment_id: environment.environment_id.as_hex(),
                name: environment.name.clone(),
                preparation_id: environment.preparation_id.as_hex(),
                recipe_version: environment.recipe_version.as_hex(),
                snapshot: PinnedSnapshotFile {
                    artifact_key: environment.snapshot.artifact_key.as_str().to_owned(),
                    snapshot_digest: environment.snapshot.snapshot_digest.as_str().to_owned(),
                    image_reference: environment.snapshot.image_reference.clone(),
                    image_manifest_digest: environment
                        .snapshot
                        .image_manifest_digest
                        .as_str()
                        .to_owned(),
                    upper_integrity: PinnedIntegrityFile {
                        algorithm: environment.snapshot.upper_integrity.algorithm.clone(),
                        value: environment.snapshot.upper_integrity.value.clone(),
                    },
                    upper_size_bytes: environment.snapshot.upper_size_bytes,
                },
            })
            .collect(),
        steps: set
            .steps
            .iter()
            .map(|binding| ResolvedStepEnvironmentFile {
                step: binding.step.as_str().to_owned(),
                environment_id: binding.environment_id.as_hex(),
                preparation_id: binding.preparation_id.as_hex(),
                snapshot_digest: binding.snapshot_digest.as_str().to_owned(),
            })
            .collect(),
    }
}

pub(crate) fn environment_set_from_file(
    file: ResolvedEnvironmentSetFile,
) -> Result<ResolvedEnvironmentSet, RunRecordError> {
    Ok(ResolvedEnvironmentSet {
        environments: file
            .environments
            .into_iter()
            .map(|environment| {
                Ok(ResolvedEnvironment {
                    environment_id: EnvironmentId::parse(&environment.environment_id)
                        .ok_or(RunRecordError::Corrupt)?,
                    name: environment.name,
                    preparation_id: PreparationId::parse(&environment.preparation_id)
                        .ok_or(RunRecordError::Corrupt)?,
                    recipe_version: EnvironmentRecipeVersion::parse(&environment.recipe_version)
                        .ok_or(RunRecordError::Corrupt)?,
                    snapshot: PreparedSnapshot {
                        artifact_key: SnapshotArtifactKey::parse(
                            &environment.snapshot.artifact_key,
                        )
                        .ok_or(RunRecordError::Corrupt)?,
                        snapshot_digest: SnapshotDigest::parse(
                            &environment.snapshot.snapshot_digest,
                        )
                        .ok_or(RunRecordError::Corrupt)?,
                        image_reference: environment.snapshot.image_reference,
                        image_manifest_digest: OciManifestDigest::parse(
                            &environment.snapshot.image_manifest_digest,
                        )
                        .ok_or(RunRecordError::Corrupt)?,
                        upper_integrity: RecordedIntegrity {
                            algorithm: environment.snapshot.upper_integrity.algorithm,
                            value: environment.snapshot.upper_integrity.value,
                        },
                        upper_size_bytes: environment.snapshot.upper_size_bytes,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        steps: file
            .steps
            .into_iter()
            .map(|binding| {
                Ok(ResolvedStepEnvironment {
                    step: StepKey::parse(&binding.step).map_err(|_| RunRecordError::Corrupt)?,
                    environment_id: EnvironmentId::parse(&binding.environment_id)
                        .ok_or(RunRecordError::Corrupt)?,
                    preparation_id: PreparationId::parse(&binding.preparation_id)
                        .ok_or(RunRecordError::Corrupt)?,
                    snapshot_digest: SnapshotDigest::parse(&binding.snapshot_digest)
                        .ok_or(RunRecordError::Corrupt)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn ref_to_file(reference: &ArtefactReference) -> ArtefactRefFile {
    ArtefactRefFile {
        id: reference.id.as_hex(),
        kind: reference.kind.as_str().to_owned(),
        artefact_hash: reference.artefact_hash.as_str(),
    }
}

fn ref_from_file(file: ArtefactRefFile) -> Result<ArtefactReference, RunRecordError> {
    Ok(ArtefactReference {
        id: super::id::ArtefactId::parse(&file.id).ok_or(RunRecordError::Corrupt)?,
        kind: crate::workflows::definition::ArtefactKind::parse(&file.kind)
            .ok_or(RunRecordError::Corrupt)?,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::parse(&file.artefact_hash)
            .ok_or(RunRecordError::Corrupt)?,
    })
}

fn gate_to_file(gate: &HumanGateRecord) -> HumanGateFile {
    HumanGateFile {
        id: gate.id.as_hex(),
        step: gate.step.as_str().to_owned(),
        sequence: gate.sequence,
        revision: gate.revision.get(),
        opened_at_ms: gate.opened_at_ms,
        closed_at_ms: gate.closed_at_ms,
        candidate: ref_to_file(&gate.candidate),
        diff_base: ref_to_file(&gate.diff_base),
        state: match gate.state {
            HumanGateState::AwaitingDecision => "awaiting-decision",
            HumanGateState::Approved => "approved",
            HumanGateState::RevisionRequested => "revision-requested",
            HumanGateState::Cancelled => "cancelled",
            HumanGateState::Interrupted => "interrupted",
        }
        .to_owned(),
        decision: gate.decision.as_ref().map(ref_to_file),
        output: gate.output.as_str().to_owned(),
    }
}

fn revision_reservation_to_file(reservation: &RevisionReservation) -> RevisionReservationFile {
    RevisionReservationFile {
        attempt: reservation.attempt.as_hex(),
        gate: reservation.gate.as_hex(),
        target: reservation.target.as_str().to_owned(),
        decision: ref_to_file(&reservation.decision),
        candidate: ref_to_file(&reservation.candidate),
        diff_base: ref_to_file(&reservation.diff_base),
        feedback: reservation.feedback.clone(),
        started: reservation.started,
    }
}

fn revision_reservation_from_file(
    file: RevisionReservationFile,
) -> Result<RevisionReservation, RunRecordError> {
    Ok(RevisionReservation {
        attempt: AttemptId::parse(&file.attempt).ok_or(RunRecordError::Corrupt)?,
        gate: GateId::parse(&file.gate).ok_or(RunRecordError::Corrupt)?,
        target: StepKey::parse(&file.target).map_err(|_| RunRecordError::Corrupt)?,
        decision: ref_from_file(file.decision)?,
        candidate: ref_from_file(file.candidate)?,
        diff_base: ref_from_file(file.diff_base)?,
        feedback: file.feedback,
        started: file.started,
    })
}

fn gate_from_file(file: HumanGateFile) -> Result<HumanGateRecord, RunRecordError> {
    Ok(HumanGateRecord {
        id: GateId::parse(&file.id).ok_or(RunRecordError::Corrupt)?,
        step: StepKey::parse(&file.step).map_err(|_| RunRecordError::Corrupt)?,
        sequence: file.sequence,
        revision: GateRevision::new(file.revision).ok_or(RunRecordError::Corrupt)?,
        opened_at_ms: file.opened_at_ms,
        closed_at_ms: file.closed_at_ms,
        candidate: ref_from_file(file.candidate)?,
        diff_base: ref_from_file(file.diff_base)?,
        state: match file.state.as_str() {
            "awaiting-decision" => HumanGateState::AwaitingDecision,
            "approved" => HumanGateState::Approved,
            "revision-requested" => HumanGateState::RevisionRequested,
            "cancelled" => HumanGateState::Cancelled,
            "interrupted" => HumanGateState::Interrupted,
            _ => return Err(RunRecordError::Corrupt),
        },
        decision: file.decision.map(ref_from_file).transpose()?,
        output: OutputKey::parse(&file.output).map_err(|_| RunRecordError::Corrupt)?,
    })
}

fn input_from_file(file: AttemptInputFile) -> Result<AttemptArtefactInput, RunRecordError> {
    Ok(AttemptArtefactInput {
        key: InputKey::parse(&file.key).map_err(|_| RunRecordError::Corrupt)?,
        artefact: ref_from_file(file.artefact)?,
    })
}

fn output_from_file(file: AttemptOutputFile) -> Result<AttemptArtefactOutput, RunRecordError> {
    Ok(AttemptArtefactOutput {
        key: OutputKey::parse(&file.key).map_err(|_| RunRecordError::Corrupt)?,
        artefact: ref_from_file(file.artefact)?,
    })
}

fn source_to_file(source: &RunSource) -> RunSourceFile {
    match source {
        RunSource::None => RunSourceFile::None,
        RunSource::Pending => RunSourceFile::Pending,
        RunSource::Captured { source } => RunSourceFile::Captured {
            source: RunSourceStateFile {
                initial: ref_to_file(&source.initial),
                accepted: ref_to_file(&source.accepted),
                observed: match &source.observed {
                    ObservedCandidate::Exact { artefact } => ObservedCandidateFile::Exact {
                        artefact: ref_to_file(artefact),
                    },
                    ObservedCandidate::Unknown => ObservedCandidateFile::Unknown,
                },
            },
        },
    }
}

fn source_from_file(file: RunSourceFile) -> Result<RunSource, RunRecordError> {
    Ok(match file {
        RunSourceFile::None => RunSource::None,
        RunSourceFile::Pending => RunSource::Pending,
        RunSourceFile::Captured { source } => RunSource::Captured {
            source: RunSourceState {
                initial: ref_from_file(source.initial)?,
                accepted: ref_from_file(source.accepted)?,
                observed: match source.observed {
                    ObservedCandidateFile::Exact { artefact } => ObservedCandidate::Exact {
                        artefact: ref_from_file(artefact)?,
                    },
                    ObservedCandidateFile::Unknown => ObservedCandidate::Unknown,
                },
            },
        },
    })
}

fn artefact_to_file(record: &ArtefactRecord) -> ArtefactFile {
    ArtefactFile {
        id: record.id.as_hex(),
        kind: record.kind.as_str().to_owned(),
        artefact_hash: record.artefact_hash.as_str(),
        object_hash: record.object_hash.as_str(),
        payload_bytes: record.payload_bytes,
        created_at_ms: record.created_at_ms,
        provenance: ProvenanceFile {
            run_id: record.provenance.run_id.as_hex(),
            producer: producer_to_file(&record.provenance.producer),
            inputs: record.provenance.inputs.iter().map(ref_to_file).collect(),
        },
        summary: summary_to_file(&record.summary),
    }
}

fn artefact_from_file(file: ArtefactFile) -> Result<ArtefactRecord, RunRecordError> {
    use crate::workflows::artefacts::ArtefactProvenance;
    Ok(ArtefactRecord {
        id: super::id::ArtefactId::parse(&file.id).ok_or(RunRecordError::Corrupt)?,
        kind: crate::workflows::definition::ArtefactKind::parse(&file.kind)
            .ok_or(RunRecordError::Corrupt)?,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::parse(&file.artefact_hash)
            .ok_or(RunRecordError::Corrupt)?,
        object_hash: crate::workflows::artefacts::ObjectHash::parse(&file.object_hash)
            .ok_or(RunRecordError::Corrupt)?,
        payload_bytes: file.payload_bytes,
        created_at_ms: file.created_at_ms,
        provenance: ArtefactProvenance {
            run_id: RunId::parse(&file.provenance.run_id).ok_or(RunRecordError::Corrupt)?,
            producer: producer_from_file(file.provenance.producer)?,
            inputs: file
                .provenance
                .inputs
                .into_iter()
                .map(ref_from_file)
                .collect::<Result<Vec<_>, _>>()?,
        },
        summary: summary_from_file(file.summary)?,
    })
}

fn producer_to_file(producer: &crate::workflows::artefacts::ArtefactProducer) -> ProducerFile {
    use crate::workflows::artefacts::ArtefactProducer;
    match producer {
        ArtefactProducer::RunSourceCapture => ProducerFile::RunSourceCapture,

        ArtefactProducer::StepAttempt {
            attempt_id,
            step,
            output,
            disposition,
        } => ProducerFile::StepAttempt {
            attempt_id: attempt_id.as_hex(),
            step: step.as_str().to_owned(),
            output: output.as_ref().map(|item| item.as_str().to_owned()),
            disposition: disposition.as_str().to_owned(),
        },
        ArtefactProducer::HumanGate {
            gate_id,
            step,
            output,
        } => ProducerFile::HumanGate {
            gate_id: gate_id.as_hex(),
            step: step.as_str().to_owned(),
            output: output.as_str().to_owned(),
        },
    }
}

fn producer_from_file(
    file: ProducerFile,
) -> Result<crate::workflows::artefacts::ArtefactProducer, RunRecordError> {
    use crate::workflows::artefacts::{ArtefactProducer, ProductionDisposition};
    Ok(match file {
        ProducerFile::RunSourceCapture => ArtefactProducer::RunSourceCapture,

        ProducerFile::StepAttempt {
            attempt_id,
            step,
            output,
            disposition,
        } => ArtefactProducer::StepAttempt {
            attempt_id: AttemptId::parse(&attempt_id).ok_or(RunRecordError::Corrupt)?,
            step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
            output: output
                .map(|value| OutputKey::parse(&value).map_err(|_| RunRecordError::Corrupt))
                .transpose()?,
            disposition: ProductionDisposition::parse(&disposition)
                .ok_or(RunRecordError::Corrupt)?,
        },
        ProducerFile::HumanGate {
            gate_id,
            step,
            output,
        } => ArtefactProducer::HumanGate {
            gate_id: GateId::parse(&gate_id).ok_or(RunRecordError::Corrupt)?,
            step: StepKey::parse(&step).map_err(|_| RunRecordError::Corrupt)?,
            output: OutputKey::parse(&output).map_err(|_| RunRecordError::Corrupt)?,
        },
    })
}

fn summary_to_file(summary: &crate::workflows::artefacts::ArtefactSummary) -> SummaryFile {
    use crate::workflows::artefacts::ArtefactSummary;
    match summary {
        ArtefactSummary::Plan { markdown_bytes } => SummaryFile::Plan {
            markdown_bytes: *markdown_bytes,
        },
        ArtefactSummary::Review { candidate, verdict } => SummaryFile::Review {
            candidate: candidate.as_str(),
            verdict: match verdict {
                crate::workflows::artefacts::ReviewVerdict::Approved => "approved".to_owned(),
                crate::workflows::artefacts::ReviewVerdict::RevisionRequired => {
                    "revision-required".to_owned()
                }
                crate::workflows::artefacts::ReviewVerdict::Blocked => "blocked".to_owned(),
            },
        },
        ArtefactSummary::Test { candidate, outcome } => SummaryFile::Test {
            candidate: candidate.as_str(),
            outcome: match outcome {
                crate::workflows::artefacts::TestOutcome::Passed => "passed".to_owned(),
                crate::workflows::artefacts::TestOutcome::Failed => "failed".to_owned(),
                crate::workflows::artefacts::TestOutcome::NotRun => "not-run".to_owned(),
            },
        },
        ArtefactSummary::Candidate {
            candidate,
            entries,
            bytes,
            disposition,
        } => SummaryFile::Candidate {
            candidate: candidate.as_str(),
            entries: *entries,
            bytes: *bytes,
            disposition: disposition.as_str().to_owned(),
        },
        ArtefactSummary::HumanDecision {
            candidate,
            diff_base,
            decision,
        } => SummaryFile::HumanDecision {
            candidate: candidate.as_str(),
            diff_base: diff_base.as_str(),
            decision: match decision {
                super::gates::HumanDecisionKind::Approved => "approved",
                super::gates::HumanDecisionKind::RevisionRequested => "revision-requested",
            }
            .to_owned(),
        },
        ArtefactSummary::PlanDecision { plan, decision } => SummaryFile::PlanDecision {
            plan: plan.as_str(),
            decision: match decision {
                super::gates::PlanDecisionKind::Accepted => "accepted",
                super::gates::PlanDecisionKind::RevisionRequested => "revision-requested",
            }
            .to_owned(),
        },
    }
}

fn summary_from_file(
    file: SummaryFile,
) -> Result<crate::workflows::artefacts::ArtefactSummary, RunRecordError> {
    use crate::workflows::artefacts::{ArtefactSummary, ProductionDisposition};
    Ok(match file {
        SummaryFile::Plan { markdown_bytes } => ArtefactSummary::Plan { markdown_bytes },
        SummaryFile::Review { candidate, verdict } => ArtefactSummary::Review {
            candidate: crate::workflows::artefacts::CandidateHash::parse(&candidate)
                .ok_or(RunRecordError::Corrupt)?,
            verdict: match verdict.as_str() {
                "approved" => crate::workflows::artefacts::ReviewVerdict::Approved,
                "revision-required" => crate::workflows::artefacts::ReviewVerdict::RevisionRequired,
                "blocked" => crate::workflows::artefacts::ReviewVerdict::Blocked,
                _ => return Err(RunRecordError::Corrupt),
            },
        },
        SummaryFile::Test { candidate, outcome } => ArtefactSummary::Test {
            candidate: crate::workflows::artefacts::CandidateHash::parse(&candidate)
                .ok_or(RunRecordError::Corrupt)?,
            outcome: match outcome.as_str() {
                "passed" => crate::workflows::artefacts::TestOutcome::Passed,
                "failed" => crate::workflows::artefacts::TestOutcome::Failed,
                "not-run" => crate::workflows::artefacts::TestOutcome::NotRun,
                _ => return Err(RunRecordError::Corrupt),
            },
        },
        SummaryFile::Candidate {
            candidate,
            entries,
            bytes,
            disposition,
        } => ArtefactSummary::Candidate {
            candidate: crate::workflows::artefacts::CandidateHash::parse(&candidate)
                .ok_or(RunRecordError::Corrupt)?,
            entries,
            bytes,
            disposition: ProductionDisposition::parse(&disposition)
                .ok_or(RunRecordError::Corrupt)?,
        },
        SummaryFile::HumanDecision {
            candidate,
            diff_base,
            decision,
        } => ArtefactSummary::HumanDecision {
            candidate: crate::workflows::artefacts::CandidateHash::parse(&candidate)
                .ok_or(RunRecordError::Corrupt)?,
            diff_base: crate::workflows::artefacts::CandidateHash::parse(&diff_base)
                .ok_or(RunRecordError::Corrupt)?,
            decision: match decision.as_str() {
                "approved" => super::gates::HumanDecisionKind::Approved,
                "revision-requested" => super::gates::HumanDecisionKind::RevisionRequested,
                _ => return Err(RunRecordError::Corrupt),
            },
        },
        SummaryFile::PlanDecision { plan, decision } => ArtefactSummary::PlanDecision {
            plan: crate::workflows::artefacts::ArtefactHash::parse(&plan)
                .ok_or(RunRecordError::Corrupt)?,
            decision: match decision.as_str() {
                "accepted" => super::gates::PlanDecisionKind::Accepted,
                "revision-requested" => super::gates::PlanDecisionKind::RevisionRequested,
                _ => return Err(RunRecordError::Corrupt),
            },
        },
    })
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn unchanged_task_gate(run: &WorkflowRun, step: &StepKey) -> bool {
    let Some(definition) = run.pinned.definition.step(step) else {
        return false;
    };
    let quick_task = run.kind == RunKind::QuickTask
        && run.pinned.workflow_id.is_none()
        && super::quick::is_expected_gate_step(definition);
    if !quick_task {
        return false;
    }
    let RunSource::Captured { source } = &run.source else {
        return false;
    };
    let ObservedCandidate::Exact { artefact: observed } = &source.observed else {
        return false;
    };
    let Some(candidate) = gate_input_candidate(run, definition) else {
        return false;
    };
    if observed != &candidate {
        return false;
    }
    let initial = run
        .artefact(&source.initial.id)
        .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash);
    let candidate = run
        .artefact(&candidate.id)
        .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash);
    initial.is_some() && candidate == initial
}

fn gate_input_candidate(run: &WorkflowRun, step: &StepDefinition) -> Option<ArtefactReference> {
    let input = step.inputs.first()?;
    Some(match &input.source {
        crate::workflows::definition::ArtefactSource::RunInitialCandidate => {
            let RunSource::Captured { source } = &run.source else {
                return None;
            };
            source.initial.clone()
        }
        crate::workflows::definition::ArtefactSource::RunCurrentCandidate => {
            let RunSource::Captured { source } = &run.source else {
                return None;
            };
            source.accepted.clone()
        }
        crate::workflows::definition::ArtefactSource::RunCurrentPlan => return None,

        crate::workflows::definition::ArtefactSource::StepOutput {
            step: source_step,
            output,
        } => run
            .attempts
            .iter()
            .rev()
            .find(|attempt| {
                attempt.step == *source_step
                    && matches!(attempt.result, Some(AttemptResult::Completed { .. }))
            })?
            .outputs
            .iter()
            .find(|item| item.key == *output)?
            .artefact
            .clone(),
    })
}

fn advanced_state(
    definition: &WorkflowDefinition,
    key: &StepKey,
) -> Result<RunState, TransitionError> {
    if definition.step_position(key).is_none() {
        return Err(TransitionError::Invalid);
    }
    Ok(match definition.next_step(key) {
        Some(next) => RunState::Ready { step: next.clone() },
        None => RunState::Completed,
    })
}

fn review_consumption(
    definition: &WorkflowDefinition,
    step: &StepKey,
    policy: &crate::workflows::definition::ReviewPolicy,
    attempt: &AttemptRecord,
    artefacts: &[ArtefactRecord],
) -> Result<(ReviewRoute, RunState), TransitionError> {
    let report = attempt
        .outputs
        .iter()
        .find(|output| output.key == policy.report_output)
        .map(|output| output.artefact.clone())
        .ok_or(TransitionError::Invalid)?;
    let verdict = artefacts
        .iter()
        .find(|artefact| artefact_matches_reference(artefact, &report))
        .and_then(|artefact| match artefact.summary {
            crate::workflows::artefacts::ArtefactSummary::Review { verdict, .. } => Some(verdict),
            _ => None,
        })
        .ok_or(TransitionError::Invalid)?;
    match verdict {
        crate::workflows::artefacts::ReviewVerdict::Approved => {
            Ok((ReviewRoute::Approved, advanced_state(definition, step)?))
        }
        crate::workflows::artefacts::ReviewVerdict::RevisionRequired
            if attempt.ordinal < u32::from(policy.attempt_limit) =>
        {
            Ok((
                ReviewRoute::RevisionRequested,
                RunState::Ready {
                    step: policy.revision_target.clone(),
                },
            ))
        }
        crate::workflows::artefacts::ReviewVerdict::RevisionRequired => Ok((
            ReviewRoute::AttemptLimitEscalation,
            RunState::Escalated {
                step: step.clone(),
                report,
                reason: EscalationReason::AttemptLimit,
            },
        )),
        crate::workflows::artefacts::ReviewVerdict::Blocked => Ok((
            ReviewRoute::BlockedEscalation,
            RunState::Escalated {
                step: step.clone(),
                report,
                reason: EscalationReason::Blocked,
            },
        )),
    }
}

fn required_output_keys(action: &StepAction) -> Vec<String> {
    let outputs = match action {
        StepAction::Agent(action) => &action.required_outputs,
        StepAction::SystemCommand(action) => &action.required_outputs,
        StepAction::HumanGate(action) => std::slice::from_ref(&action.required_output),
    };
    outputs
        .iter()
        .map(|output| output.key.as_str().to_owned())
        .collect()
}

fn next_ordinal(attempts: &[AttemptRecord], step: &StepKey) -> u32 {
    attempts
        .iter()
        .filter(|attempt| attempt.step == *step)
        .count() as u32
        + 1
}

fn apply_transaction_to_file(transaction: &ApplyTransaction) -> ApplyTransactionFile {
    let (state, completed, path) = match &transaction.state {
        ApplyTransactionState::Prepared => ("prepared", None, None),
        ApplyTransactionState::Applying { completed, path } => {
            ("applying", Some(*completed), Some(path.clone()))
        }
        ApplyTransactionState::Applied { completed } => ("applied", Some(*completed), None),
        ApplyTransactionState::Verified => ("verified", None, None),
        ApplyTransactionState::Recovered => ("recovered", None, None),
        ApplyTransactionState::RecoveryUncertain => ("recovery-uncertain", None, None),
    };
    ApplyTransactionFile {
        state: state.to_owned(),
        completed,
        path,
        roots: transaction
            .roots
            .iter()
            .map(|root| ApplyRootFile {
                grant_id: root.grant_id.as_hex(),
                alias: root.alias.clone(),
                host_path: root.host_path.clone(),
                device: root.identity.device,
                inode: root.identity.inode,
                baseline_candidate: root.baseline_candidate.as_str(),
                candidate_hash: root.candidate_hash.as_str(),
                exclusions: root.exclusions.clone(),
                outcome: match root.outcome {
                    ApplyRootOutcome::Pending => "pending",
                    ApplyRootOutcome::Unchanged => "unchanged",
                    ApplyRootOutcome::Applied => "applied",
                    ApplyRootOutcome::Conflicted => "conflicted",
                    ApplyRootOutcome::Uncertain => "uncertain",
                }
                .to_owned(),
            })
            .collect(),
        baseline: ref_to_file(&transaction.baseline),
        candidate: ref_to_file(&transaction.candidate),
        approval: ref_to_file(&transaction.approval),
    }
}

fn apply_transaction_from_file(
    file: ApplyTransactionFile,
) -> Result<ApplyTransaction, RunRecordError> {
    let state = match (file.state.as_str(), file.completed, file.path) {
        ("prepared", None, None) => ApplyTransactionState::Prepared,
        ("applying", Some(completed), Some(path)) if !path.is_empty() => {
            ApplyTransactionState::Applying { completed, path }
        }
        ("applied", Some(completed), None) => ApplyTransactionState::Applied { completed },
        ("verified", None, None) => ApplyTransactionState::Verified,
        ("recovered", None, None) => ApplyTransactionState::Recovered,
        ("recovery-uncertain", None, None) => ApplyTransactionState::RecoveryUncertain,
        _ => return Err(RunRecordError::Corrupt),
    };
    Ok(ApplyTransaction {
        state,
        roots: file
            .roots
            .into_iter()
            .map(|root| {
                Ok(ApplyRoot {
                    grant_id: crate::execution::DirectoryGrantId::parse(&root.grant_id)
                        .ok_or(RunRecordError::Corrupt)?,
                    alias: root.alias,
                    host_path: root.host_path,
                    identity: crate::execution::CanonicalDirectoryIdentity {
                        device: root.device,
                        inode: root.inode,
                    },
                    baseline_candidate: crate::workflows::artefacts::CandidateHash::parse(
                        &root.baseline_candidate,
                    )
                    .ok_or(RunRecordError::Corrupt)?,
                    candidate_hash: crate::workflows::artefacts::CandidateHash::parse(
                        &root.candidate_hash,
                    )
                    .ok_or(RunRecordError::Corrupt)?,
                    exclusions: root.exclusions,
                    outcome: match root.outcome.as_str() {
                        "pending" => ApplyRootOutcome::Pending,
                        "unchanged" => ApplyRootOutcome::Unchanged,
                        "applied" => ApplyRootOutcome::Applied,
                        "conflicted" => ApplyRootOutcome::Conflicted,
                        "uncertain" => ApplyRootOutcome::Uncertain,
                        _ => return Err(RunRecordError::Corrupt),
                    },
                })
            })
            .collect::<Result<Vec<_>, RunRecordError>>()?,
        baseline: ref_from_file(file.baseline)?,
        candidate: ref_from_file(file.candidate)?,
        approval: ref_from_file(file.approval)?,
    })
}

fn commit_transaction_to_file(transaction: &CommitTransaction) -> CommitTransactionFile {
    CommitTransactionFile {
        candidate: ref_to_file(&transaction.candidate),
        reviews: transaction.reviews.iter().map(ref_to_file).collect(),
        approval: transaction.approval.as_ref().map(ref_to_file),
        roots: transaction
            .roots
            .iter()
            .map(|root| CommitRootFile {
                grant_id: root.grant.id.as_hex(),
                alias: root.grant.alias.clone(),
                host_path: root.grant.host_path.clone(),
                identity: root.grant.identity,
                state: root.state.encode(),
                expected_reference: root.expected_reference.clone(),
                old_object: root.old_object.clone(),
                target_tree: root.target_tree.clone(),
                expected_commit: root.expected_commit.clone(),
                timestamp: root.timestamp.clone(),
            })
            .collect(),
    }
}

fn commit_transaction_from_file(
    file: CommitTransactionFile,
) -> Result<CommitTransaction, RunRecordError> {
    Ok(CommitTransaction {
        candidate: ref_from_file(file.candidate)?,
        reviews: file
            .reviews
            .into_iter()
            .map(ref_from_file)
            .collect::<Result<_, _>>()?,
        approval: file.approval.map(ref_from_file).transpose()?,
        roots: file
            .roots
            .into_iter()
            .map(|root| {
                Ok(CommitRoot {
                    grant: crate::execution::DirectoryGrant {
                        id: crate::execution::DirectoryGrantId::parse(&root.grant_id)
                            .ok_or(RunRecordError::Corrupt)?,
                        alias: root.alias,
                        host_path: root.host_path,
                        identity: root.identity,
                        access: crate::execution::DirectoryAccess::ReviewBeforeApply,
                    },
                    state: CommitTransactionState::parse(&root.state)
                        .ok_or(RunRecordError::Corrupt)?,
                    expected_reference: root.expected_reference,
                    old_object: root.old_object,
                    target_tree: root.target_tree,
                    expected_commit: root.expected_commit,
                    timestamp: root.timestamp,
                })
            })
            .collect::<Result<_, RunRecordError>>()?,
    })
}

fn validate_attempt_references(
    run: &WorkflowRun,
    attempt: &AttemptRecord,
    step: &StepDefinition,
) -> Result<(), RunRecordError> {
    for (index, input) in attempt.inputs.iter().enumerate() {
        if attempt.inputs[..index]
            .iter()
            .any(|earlier| earlier.key == input.key)
            || !step
                .inputs
                .iter()
                .any(|declared| declared.key == input.key && declared.kind == input.artefact.kind)
        {
            return Err(RunRecordError::Corrupt);
        }
        referenced_artefact(run, &input.artefact)?;
    }
    for (index, output) in attempt.outputs.iter().enumerate() {
        if attempt.outputs[..index]
            .iter()
            .any(|earlier| earlier.key == output.key)
            || !step.required_outputs().iter().any(|declared| {
                declared.key == output.key
                    && declared.kind.as_artefact_kind() == Some(output.artefact.kind)
            })
        {
            return Err(RunRecordError::Corrupt);
        }
        let record = referenced_artefact(run, &output.artefact)?;
        if !matches!(
            &record.provenance.producer,
            crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                attempt_id,
                step: producer_step,
                output: Some(producer_output),
                disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            } if *attempt_id == attempt.id
                && *producer_step == attempt.step
                && *producer_output == output.key
        ) {
            return Err(RunRecordError::Corrupt);
        }
    }
    Ok(())
}

fn validate_attempt_result(
    attempt: &AttemptRecord,
    action: &StepDefinition,
) -> Result<(), RunRecordError> {
    crate::workflows::artefacts::output::OutputDrafts::default()
        .restore(action.required_outputs(), &attempt.paused_drafts)
        .map_err(|_| RunRecordError::Corrupt)?;
    let valid_finish = attempt
        .finished_at_ms
        .is_some_and(|finished| finished >= attempt.started_at_ms);
    match (&attempt.state, &attempt.finished_at_ms, &attempt.result) {
        (AttemptState::Active, None, None) => Ok(()),
        (AttemptState::Paused, None, None)
            if attempt.continuation.is_some()
                && attempt.cleanup == AttemptCleanupRecord::Complete
                && attempt.action_kind == ActionKind::Agent =>
        {
            Ok(())
        }
        (AttemptState::Completed, Some(_), Some(AttemptResult::Completed { outputs }))
            if valid_finish
                && *outputs == required_output_keys(&action.action)
                && durable_outputs_match(attempt, action) =>
        {
            Ok(())
        }
        (AttemptState::Failed, Some(_), Some(AttemptResult::Failed { .. })) if valid_finish => {
            Ok(())
        }
        (AttemptState::Cancelled, Some(_), Some(AttemptResult::Cancelled)) if valid_finish => {
            Ok(())
        }
        (AttemptState::Interrupted, Some(_), Some(AttemptResult::Interrupted)) if valid_finish => {
            Ok(())
        }
        _ => Err(RunRecordError::Corrupt),
    }
}

fn durable_outputs_match(attempt: &AttemptRecord, step: &StepDefinition) -> bool {
    let fixes_or_commits = step
        .required_outputs()
        .iter()
        .any(|output| output.kind == crate::workflows::definition::OutputKind::ReviewReport)
        || matches!(
            &step.action,
            StepAction::SystemCommand(action)
                if action.command == crate::workflows::commands::SystemCommandId::CommitCandidate
        );
    if !fixes_or_commits {
        return true;
    }
    let durable: Vec<_> = step
        .required_outputs()
        .iter()
        .filter_map(|output| {
            output
                .kind
                .as_artefact_kind()
                .map(|kind| (&output.key, kind))
        })
        .collect();
    attempt.outputs.len() == durable.len()
        && durable.iter().all(|(key, kind)| {
            attempt
                .outputs
                .iter()
                .any(|output| output.key == **key && output.artefact.kind == *kind)
        })
}

fn validate_report_binding(
    attempt: &AttemptRecord,
    step: &StepDefinition,
    run: &WorkflowRun,
) -> Result<(), RunRecordError> {
    let StepAction::Agent(action) = &step.action else {
        return Ok(());
    };
    let reports: Vec<_> = attempt
        .outputs
        .iter()
        .filter(|output| {
            output.artefact.kind == crate::workflows::definition::ArtefactKind::ReviewReport
        })
        .collect();
    if reports.is_empty() {
        return Ok(());
    }
    let candidate_reference = match action.candidate_authority {
        crate::workflows::definition::CandidateAuthority::ReadOnly => attempt
            .inputs
            .iter()
            .find(|input| {
                input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
            })
            .map(|input| &input.artefact),
        crate::workflows::definition::CandidateAuthority::Edit => attempt
            .outputs
            .iter()
            .find(|output| {
                output.artefact.kind
                    == crate::workflows::definition::ArtefactKind::CandidateRevision
            })
            .map(|output| &output.artefact),
    }
    .ok_or(RunRecordError::Corrupt)?;
    let candidate_record = run
        .artefact(&candidate_reference.id)
        .ok_or(RunRecordError::Corrupt)?;
    let candidate = candidate_record
        .candidate_hash()
        .ok_or(RunRecordError::Corrupt)?;
    if candidate_record.provenance.run_id != run.id {
        return Err(RunRecordError::Corrupt);
    }
    if action.candidate_authority == crate::workflows::definition::CandidateAuthority::Edit
        && !required_output_from_attempt(candidate_record, attempt)
    {
        return Err(RunRecordError::Corrupt);
    }
    for report in reports {
        let record = run
            .artefact(&report.artefact.id)
            .ok_or(RunRecordError::Corrupt)?;
        let bound = record.candidate_hash().ok_or(RunRecordError::Corrupt)?;
        if bound != candidate
            || record.provenance.run_id != run.id
            || !required_output_from_attempt(record, attempt)
            || !record
                .provenance
                .inputs
                .iter()
                .any(|input| input == candidate_reference)
        {
            return Err(RunRecordError::Corrupt);
        }
    }
    Ok(())
}

fn required_output_from_attempt(
    record: &crate::workflows::artefacts::ArtefactRecord,
    attempt: &AttemptRecord,
) -> bool {
    matches!(
        &record.provenance.producer,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id,
            step,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
            ..
        } if *attempt_id == attempt.id && *step == attempt.step
    )
}

fn validate_attempt_isolation(
    attempt: &AttemptRecord,
    step: &StepDefinition,
    run: &WorkflowRun,
) -> Result<(), RunRecordError> {
    let file_application = matches!(
        &step.action,
        StepAction::SystemCommand(action)
            if action.command == crate::workflows::commands::SystemCommandId::ApplyChanges
    );
    let host_execution = matches!(
        &step.action,
        StepAction::Agent(action) if action.host_tools()
    );
    if (file_application && attempt.sandbox.kind != AttemptSandboxKind::FileApplication)
        || (host_execution && attempt.sandbox.kind != AttemptSandboxKind::HostExecution)
        || (!file_application
            && !host_execution
            && attempt.sandbox.kind != AttemptSandboxKind::IsolatedAttempt)
    {
        return Err(RunRecordError::Corrupt);
    }
    if !host_execution {
        let Some(binding) = run
            .environments
            .steps
            .iter()
            .find(|item| item.step == attempt.step)
        else {
            return Err(RunRecordError::Corrupt);
        };
        if attempt.sandbox.snapshot_digest != binding.snapshot_digest {
            return Err(RunRecordError::Corrupt);
        }
    }
    let apply = matches!(
        &step.action,
        StepAction::SystemCommand(action)
            if action.command == crate::workflows::commands::SystemCommandId::ApplyChanges
    );
    let commit = matches!(
        &step.action,
        StepAction::SystemCommand(action)
            if action.command == crate::workflows::commands::SystemCommandId::CommitCandidate
    );
    if apply {
        if attempt.capabilities.git_admin != AccessMode::ReadOnly
            || attempt.capabilities.source_location != PrimarySourceLocation::UserProject
            || attempt.commit_transaction.is_some()
            || attempt
                .apply_transaction
                .as_ref()
                .is_some_and(|transaction| !valid_apply_transaction(run, attempt, transaction))
            || (attempt.state == AttemptState::Completed
                && !attempt
                    .apply_transaction
                    .as_ref()
                    .is_some_and(ApplyTransaction::is_verified))
        {
            return Err(RunRecordError::Corrupt);
        }
    } else if commit {
        if attempt.capabilities.git_admin != AccessMode::ReadWrite
            || attempt.capabilities.source_location != PrimarySourceLocation::UserProject
        {
            return Err(RunRecordError::Corrupt);
        }
        if let Some(transaction) = &attempt.commit_transaction
            && !valid_commit_transaction(attempt, transaction)
        {
            return Err(RunRecordError::Corrupt);
        }
        if let Some(transaction) = &attempt.commit_transaction {
            if transaction.roots.is_empty() {
                let RunSource::Captured { source } = &run.source else {
                    return Err(RunRecordError::Corrupt);
                };
                let unchanged = matches!(
                    (run.artefact(&source.initial.id).map(|record| &record.summary),
                     run.artefact(&transaction.candidate.id).map(|record| &record.summary)),
                    (Some(crate::workflows::artefacts::ArtefactSummary::Candidate { candidate: before, .. }),
                     Some(crate::workflows::artefacts::ArtefactSummary::Candidate { candidate: after, .. })) if before == after
                );
                if !unchanged {
                    return Err(RunRecordError::Corrupt);
                }
            }
            let grants = run.reviewed_directories();
            if transaction.roots.iter().any(|root| {
                !grants.contains(&root.grant)
                    || !attempt
                        .capabilities
                        .directories
                        .iter()
                        .any(|directory| directory.alias == root.grant.alias)
            }) {
                return Err(RunRecordError::Corrupt);
            }
        }
        if attempt.state == AttemptState::Completed
            && !attempt
                .commit_transaction
                .as_ref()
                .is_some_and(CommitTransaction::is_verified)
        {
            return Err(RunRecordError::Corrupt);
        }
    } else if attempt.apply_transaction.is_some()
        || attempt.commit_transaction.is_some()
        || attempt.capabilities.git_admin != AccessMode::ReadOnly
        || !matches!(
            attempt.capabilities.source_location,
            PrimarySourceLocation::AttemptWorkspace | PrimarySourceLocation::PrivateWorkspace
        )
    {
        return Err(RunRecordError::Corrupt);
    }
    match attempt.action_kind {
        ActionKind::SystemCommand => {
            if attempt.capabilities.network != NetworkCapability::None
                || !attempt.capabilities.tools.is_empty()
            {
                return Err(RunRecordError::Corrupt);
            }
        }
        ActionKind::Agent => {}
        ActionKind::HumanGate => return Err(RunRecordError::Corrupt),
    }
    if !capabilities_match_step(run, &attempt.capabilities, step) {
        return Err(RunRecordError::Corrupt);
    }
    match (&attempt.state, &attempt.cleanup) {
        (AttemptState::Completed | AttemptState::Paused, AttemptCleanupRecord::Complete) => {}
        (
            AttemptState::Active,
            AttemptCleanupRecord::Pending
            | AttemptCleanupRecord::Complete
            | AttemptCleanupRecord::Orphaned { .. },
        ) => {}
        (
            AttemptState::Interrupted,
            AttemptCleanupRecord::Pending
            | AttemptCleanupRecord::Complete
            | AttemptCleanupRecord::Orphaned { .. },
        ) => {}
        (
            AttemptState::Failed | AttemptState::Cancelled,
            AttemptCleanupRecord::Complete | AttemptCleanupRecord::Orphaned { .. },
        ) => {}
        _ => return Err(RunRecordError::Corrupt),
    }
    Ok(())
}

fn valid_apply_transaction(
    run: &WorkflowRun,
    attempt: &AttemptRecord,
    transaction: &ApplyTransaction,
) -> bool {
    let candidate = attempt.inputs.iter().find(|input| {
        input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    });
    let approval = attempt.inputs.iter().find(|input| {
        input.artefact.kind == crate::workflows::definition::ArtefactKind::HumanDecision
    });
    let source = match &run.source {
        RunSource::Captured { source } => source,
        RunSource::None | RunSource::Pending => return false,
    };
    let baseline_record = run.artefact(&transaction.baseline.id);
    let candidate_record = run.artefact(&transaction.candidate.id);
    let approval_record = run.artefact(&transaction.approval.id);
    let summaries_match = match (
        baseline_record.map(|record| &record.summary),
        candidate_record.map(|record| &record.summary),
        approval_record.map(|record| &record.summary),
    ) {
        (
            Some(crate::workflows::artefacts::ArtefactSummary::Candidate {
                candidate: baseline,
                ..
            }),
            Some(crate::workflows::artefacts::ArtefactSummary::Candidate { candidate, .. }),
            Some(crate::workflows::artefacts::ArtefactSummary::HumanDecision {
                candidate: approved,
                diff_base,
                decision: crate::workflows::gates::HumanDecisionKind::Approved,
            }),
        ) => candidate == approved && baseline == diff_base,
        _ => false,
    };
    if candidate.map(|input| &input.artefact) != Some(&transaction.candidate)
        || approval.map(|input| &input.artefact) != Some(&transaction.approval)
        || source.initial != transaction.baseline
        // Later outputs can advance accepted and observed references. The transaction retains its exact approved inputs.
        || !summaries_match
        || transaction.baseline.kind
            != crate::workflows::definition::ArtefactKind::CandidateRevision
        || transaction.roots.is_empty()
        || transaction.roots.iter().enumerate().any(|(index, root)| {
            root.alias.is_empty()
                || root.host_path.as_os_str().is_empty()
                || !root.host_path.is_absolute()
                || root.exclusions.windows(2).any(|pair| pair[0] >= pair[1])
                || transaction.roots[..index].iter().any(|previous| {
                    previous.grant_id == root.grant_id || previous.alias == root.alias
                })
        })
    {
        return false;
    }
    match &transaction.state {
        ApplyTransactionState::Prepared => transaction.roots.iter().all(|root| {
            matches!(
                root.outcome,
                ApplyRootOutcome::Pending | ApplyRootOutcome::Unchanged
            )
        }),
        ApplyTransactionState::Verified => transaction.roots.iter().all(|root| {
            matches!(
                root.outcome,
                ApplyRootOutcome::Applied | ApplyRootOutcome::Unchanged
            )
        }),
        ApplyTransactionState::Recovered => true,
        ApplyTransactionState::RecoveryUncertain => transaction.roots.iter().any(|root| {
            matches!(
                root.outcome,
                ApplyRootOutcome::Conflicted | ApplyRootOutcome::Uncertain
            )
        }),
        ApplyTransactionState::Applying { path, .. } => !path.is_empty(),
        ApplyTransactionState::Applied { .. } => true,
    }
}

fn valid_commit_transaction(attempt: &AttemptRecord, transaction: &CommitTransaction) -> bool {
    let expected_reviews: Vec<_> = attempt
        .inputs
        .iter()
        .filter(|input| {
            input.artefact.kind == crate::workflows::definition::ArtefactKind::ReviewReport
        })
        .map(|input| &input.artefact)
        .collect();
    let expected_approval = attempt.inputs.iter().find_map(|input| {
        (input.artefact.kind == crate::workflows::definition::ArtefactKind::HumanDecision)
            .then_some(&input.artefact)
    });
    if transaction.candidate.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
        || (expected_reviews.is_empty() && expected_approval.is_none())
        || transaction
            .reviews
            .iter()
            .any(|review| review.kind != crate::workflows::definition::ArtefactKind::ReviewReport)
        || transaction.approval.as_ref() != expected_approval
        || !attempt
            .inputs
            .iter()
            .any(|input| input.artefact == transaction.candidate)
        || transaction.reviews.len() != expected_reviews.len()
        || transaction
            .reviews
            .iter()
            .zip(expected_reviews)
            .any(|(stored, expected)| stored != expected)
        || transaction.roots.len() > crate::execution::MAXIMUM_DIRECTORY_GRANTS
        || transaction.roots.iter().enumerate().any(|(index, root)| {
            transaction.roots[..index].iter().any(|previous| {
                previous.grant.id == root.grant.id || previous.grant.identity == root.grant.identity
            }) || !valid_commit_root(root)
        })
    {
        return false;
    }
    true
}

fn valid_commit_root(root: &CommitRoot) -> bool {
    if root.grant.access != crate::execution::DirectoryAccess::ReviewBeforeApply
        || !root.grant.host_path.is_absolute()
        || root.grant.alias.is_empty()
        || root.expected_reference.is_empty()
        || root.timestamp.split_once(' ').is_none()
        || [&root.old_object, &root.target_tree, &root.expected_commit]
            .into_iter()
            .any(|object| {
                object
                    .as_deref()
                    .is_some_and(|object| !valid_git_object_id(object))
            })
    {
        return false;
    }
    match &root.state {
        CommitTransactionState::Prepared | CommitTransactionState::Restored => true,
        CommitTransactionState::WorktreeApplied => {
            root.target_tree.is_some() && root.expected_commit.is_some()
        }
        CommitTransactionState::ReferenceUpdated { commit }
        | CommitTransactionState::Verified { commit } => {
            root.target_tree.is_some()
                && root.expected_commit.as_deref() == Some(commit)
                && valid_git_object_id(commit)
        }
    }
}

fn valid_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn capabilities_match_step(
    run: &WorkflowRun,
    capabilities: &AttemptCapabilities,
    step: &StepDefinition,
) -> bool {
    if capabilities.schema != crate::workflows::capabilities::CAPABILITY_SCHEMA
        || capabilities.agent_revision == 0
    {
        return false;
    }
    if let Some(settings) = run.directory_settings() {
        return crate::execution::ProjectFreeAuthority::from_snapshot(
            capabilities.agent_revision,
            run.phase_settings(&step.key).unwrap_or(&settings),
        )
        .ok()
        .and_then(|authority| AttemptCapabilities::derive_project_free(step, &authority).ok())
        .is_some_and(|expected| expected == *capabilities);
    }
    match &step.action {
        StepAction::Agent(action) => {
            let primary = capabilities
                .directories
                .iter()
                .filter(|directory| directory.role == DirectoryRole::PrimarySource);
            let primary: Vec<_> = primary.collect();
            let secondary: Vec<_> = capabilities
                .directories
                .iter()
                .filter(|directory| directory.role == DirectoryRole::SecondaryContext)
                .collect();
            let private_workspace = capabilities.source_location
                == PrimarySourceLocation::PrivateWorkspace
                && action.candidate_authority
                    == crate::workflows::definition::CandidateAuthority::ReadOnly
                && primary.len() == 1
                && if action.authority.directories.is_empty() {
                    primary[0].guest_path == crate::execution::GUEST_WORKSPACE
                        && primary[0].access == AccessMode::ReadWrite
                        && secondary.is_empty()
                } else {
                    primary[0].access == AccessMode::ReadOnly
                        && valid_guest_path(&primary[0].guest_path)
                        && action
                            .authority
                            .directories
                            .iter()
                            .any(|item| item.alias == primary[0].alias)
                        && capabilities.directories.len() == action.authority.directories.len()
                };
            let candidate_workspace = capabilities.source_location
                == PrimarySourceLocation::AttemptWorkspace
                && primary.len() == 1
                && primary[0].access == action.candidate_authority.access();
            capabilities.git_admin == AccessMode::ReadOnly
                && (private_workspace || candidate_workspace)
                && capabilities
                    .tools
                    .iter()
                    .all(|tool| action.authority.tools.contains(tool))
                && (private_workspace || secondary.len() == action.authority.directories.len())
                && secondary.iter().all(|directory| {
                    valid_guest_path(&directory.guest_path)
                        && directory.access == AccessMode::ReadOnly
                        && action
                            .authority
                            .directories
                            .iter()
                            .any(|item| item.alias == directory.alias)
                })
        }
        StepAction::SystemCommand(action) => {
            let apply = action.command == crate::workflows::commands::SystemCommandId::ApplyChanges;
            let commit =
                action.command == crate::workflows::commands::SystemCommandId::CommitCandidate;
            capabilities.tools.is_empty()
                && capabilities.network == NetworkCapability::None
                && capabilities.directories.iter().all(|directory| {
                    valid_guest_path(&directory.guest_path)
                        && if apply || commit {
                            directory.access == AccessMode::ReadWrite
                        } else {
                            directory.access == AccessMode::ReadOnly
                        }
                })
                && if commit {
                    capabilities.git_admin == AccessMode::ReadWrite
                        && capabilities.source_location == PrimarySourceLocation::UserProject
                } else if apply {
                    capabilities.git_admin == AccessMode::ReadOnly
                        && capabilities.source_location == PrimarySourceLocation::UserProject
                } else {
                    capabilities.git_admin == AccessMode::ReadOnly
                        && capabilities.source_location == PrimarySourceLocation::AttemptWorkspace
                }
        }
        StepAction::HumanGate(_) => false,
    }
}

fn valid_guest_path(path: &str) -> bool {
    (path == crate::agents::GUEST_PROJECT
        || path == crate::execution::GUEST_WORKSPACE
        || path.starts_with("/access/"))
        && !path.contains('\\')
        && !path.contains(':')
        && !path.contains("..")
        && !path.contains("workflow-workspaces")
}

fn capabilities_to_file(capabilities: &AttemptCapabilities) -> AttemptCapabilitiesFile {
    AttemptCapabilitiesFile {
        schema: capabilities.schema,
        agent_revision: capabilities.agent_revision,
        tools: capabilities
            .tools
            .iter()
            .map(|tool| tool.as_str().to_owned())
            .collect(),
        directories: capabilities
            .directories
            .iter()
            .map(|directory| CapabilityDirectoryFile {
                alias: directory.alias.clone(),
                guest_path: directory.guest_path.clone(),
                access: directory.access.as_str().to_owned(),
                role: directory.role.as_str().to_owned(),
            })
            .collect(),
        git_admin: capabilities.git_admin.as_str().to_owned(),
        source_location: capabilities.source_location.as_str().to_owned(),
        network: capabilities.network.as_str().to_owned(),
        network_domains: capabilities.network.domains().to_vec(),
    }
}

fn capabilities_from_file(
    file: AttemptCapabilitiesFile,
) -> Result<AttemptCapabilities, RunRecordError> {
    if file.schema != crate::workflows::capabilities::CAPABILITY_SCHEMA || file.agent_revision == 0
    {
        return Err(RunRecordError::Corrupt);
    }
    let git_admin = AccessMode::parse(&file.git_admin).ok_or(RunRecordError::Corrupt)?;
    let source_location =
        PrimarySourceLocation::parse(&file.source_location).ok_or(RunRecordError::Corrupt)?;
    match (git_admin, source_location) {
        (AccessMode::ReadOnly, PrimarySourceLocation::AttemptWorkspace)
        | (AccessMode::ReadOnly, PrimarySourceLocation::PrivateWorkspace)
        | (AccessMode::ReadOnly, PrimarySourceLocation::UserProject)
        | (AccessMode::ReadWrite, PrimarySourceLocation::UserProject) => {}
        _ => return Err(RunRecordError::Corrupt),
    }
    let mut tools = Vec::new();
    for name in file.tools {
        let tool = ToolId::parse(&name).ok_or(RunRecordError::Corrupt)?;
        if tools.contains(&tool) {
            return Err(RunRecordError::Corrupt);
        }
        tools.push(tool);
    }
    let mut directories = Vec::new();
    for directory in file.directories {
        if !valid_guest_path(&directory.guest_path) {
            return Err(RunRecordError::Corrupt);
        }
        directories.push(CapabilityDirectory {
            alias: directory.alias,
            guest_path: directory.guest_path,
            access: AccessMode::parse(&directory.access).ok_or(RunRecordError::Corrupt)?,
            role: DirectoryRole::parse(&directory.role).ok_or(RunRecordError::Corrupt)?,
        });
    }
    let network = NetworkCapability::parse(&file.network, file.network_domains)
        .ok_or(RunRecordError::Corrupt)?;
    Ok(AttemptCapabilities {
        schema: file.schema,
        agent_revision: file.agent_revision,
        tools,
        directories,
        source_location,
        git_admin,
        network,
    })
}

fn cleanup_to_file(cleanup: &AttemptCleanupRecord) -> AttemptCleanupFile {
    match cleanup {
        AttemptCleanupRecord::Pending => AttemptCleanupFile::Pending,
        AttemptCleanupRecord::Complete => AttemptCleanupFile::Complete,
        AttemptCleanupRecord::Orphaned {
            sandbox,
            workspace,
            journal,
        } => AttemptCleanupFile::Orphaned {
            sandbox: *sandbox,
            workspace: *workspace,
            journal: *journal,
        },
    }
}

fn cleanup_from_file(file: AttemptCleanupFile) -> Result<AttemptCleanupRecord, RunRecordError> {
    Ok(match file {
        AttemptCleanupFile::Pending => AttemptCleanupRecord::Pending,
        AttemptCleanupFile::Complete => AttemptCleanupRecord::Complete,
        AttemptCleanupFile::Orphaned {
            sandbox,
            workspace,
            journal,
        } => AttemptCleanupRecord::Orphaned {
            sandbox,
            workspace,
            journal,
        },
    })
}

impl AttemptSandboxKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::IsolatedAttempt => "isolated-attempt",
            Self::FileApplication => "file-application",
            Self::HostExecution => "host-execution",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "isolated-attempt" => Some(Self::IsolatedAttempt),
            "file-application" => Some(Self::FileApplication),
            "host-execution" => Some(Self::HostExecution),
            _ => None,
        }
    }
}

fn context_matches_attempt(context: &AttemptContextPacket, attempt: &AttemptRecord) -> bool {
    if attempt.action_kind != ActionKind::Agent {
        return false;
    }
    let candidate = attempt.inputs.iter().find(|input| {
        input.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    });
    match (candidate, context.project_instructions.candidate.as_ref()) {
        (None, None) => true,
        (Some(input), Some(recorded)) => {
            input.artefact.id.as_hex() == recorded.id
                && input.artefact.kind.as_str() == recorded.kind
                && input.artefact.artefact_hash.as_str() == recorded.artefact_hash
        }
        _ => false,
    }
}

fn validate_attempts(run: &WorkflowRun) -> Result<(), RunRecordError> {
    let mut active = 0usize;
    for (index, attempt) in run.attempts.iter().enumerate() {
        if run.attempts[..index]
            .iter()
            .any(|earlier| earlier.id == attempt.id)
        {
            return Err(RunRecordError::Corrupt);
        }
        if let Some(previous) = index.checked_sub(1).map(|previous| &run.attempts[previous]) {
            let previous_finished = previous.finished_at_ms.ok_or(RunRecordError::Corrupt)?;
            if attempt.started_at_ms < previous_finished {
                return Err(RunRecordError::Corrupt);
            }
        }
        let step = run
            .pinned
            .definition
            .step(&attempt.step)
            .ok_or(RunRecordError::Corrupt)?;
        if attempt.action_kind != ActionKind::from_action(&step.action)
            || attempt.ordinal != next_ordinal(&run.attempts[..index], &attempt.step)
            || attempt.started_at_ms < run.created_at_ms
        {
            return Err(RunRecordError::Corrupt);
        }
        if attempt.state == AttemptState::Active {
            active += 1;
            if index + 1 != run.attempts.len() {
                return Err(RunRecordError::Corrupt);
            }
        }
        validate_attempt_references(run, attempt, step)?;
        if let Some(context) = &attempt.initial_context
            && (!context.validate() || !context_matches_attempt(context, attempt))
        {
            return Err(RunRecordError::Corrupt);
        }
        validate_attempt_result(attempt, step)?;
        validate_attempt_isolation(attempt, step, run)?;
        validate_report_binding(attempt, step, run)?;
    }
    if active > 1 {
        return Err(RunRecordError::Corrupt);
    }
    Ok(())
}

fn validate_revision_reservation(run: &WorkflowRun) -> Result<(), RunRecordError> {
    let Some(reservation) = &run.revision_reservation else {
        return Ok(());
    };
    let gate = run
        .gates
        .iter()
        .find(|gate| gate.id == reservation.gate)
        .ok_or(RunRecordError::Corrupt)?;
    let policy = run
        .human_revision_policy(&gate.step)
        .ok_or(RunRecordError::Corrupt)?;
    let plan_gate = gate.candidate.kind == crate::workflows::definition::ArtefactKind::Plan;
    if gate.state != HumanGateState::RevisionRequested
        || gate.decision.as_ref() != Some(&reservation.decision)
        || gate.candidate != reservation.candidate
        || gate.diff_base != reservation.diff_base
        || (plan_gate && gate.diff_base != gate.candidate)
        || reservation.feedback.is_empty()
        || reservation.feedback.len() > super::gates::MAXIMUM_REVISION_NOTE_BYTES
        || reservation.target != policy.revision_target
        || (!reservation.started
            && !matches!(
                run.state,
                RunState::Ready { ref step } if step == &reservation.target
            ))
        || (reservation.started
            && !matches!(
                run.state,
                RunState::Active { ref step, attempt } if step == &reservation.target && attempt == reservation.attempt
            ))
    {
        return Err(RunRecordError::Corrupt);
    }
    if plan_gate {
        let Some(step) = run.pinned.definition.step(&gate.step) else {
            return Err(RunRecordError::Corrupt);
        };
        let StepAction::HumanGate(action) = &step.action else {
            return Err(RunRecordError::Corrupt);
        };
        if !action.is_plan_checkpoint() {
            return Err(RunRecordError::Corrupt);
        }
    }
    Ok(())
}

fn validate_review_routes(run: &WorkflowRun) -> Result<(), RunRecordError> {
    for attempt in &run.attempts {
        let step = run
            .pinned
            .definition
            .step(&attempt.step)
            .ok_or(RunRecordError::Corrupt)?;
        match (&step.review, attempt.state, attempt.review_route) {
            (None, _, None) => {}
            (Some(_), AttemptState::Completed, Some(route)) => {
                let policy = step.review.as_ref().ok_or(RunRecordError::Corrupt)?;
                let (expected, _) = review_consumption(
                    &run.pinned.definition,
                    &attempt.step,
                    policy,
                    attempt,
                    &run.artefacts,
                )
                .map_err(|_| RunRecordError::Corrupt)?;
                if expected != route {
                    return Err(RunRecordError::Corrupt);
                }
            }
            (Some(_), AttemptState::Completed, None)
            | (None, _, Some(_))
            | (Some(_), _, Some(_)) => return Err(RunRecordError::Corrupt),
            (Some(_), _, None) => {}
        }
    }
    Ok(())
}

fn validate_state_facts(run: &WorkflowRun) -> Result<(), RunRecordError> {
    let mut expected = RunState::Ready {
        step: run.pinned.definition.first_step().clone(),
    };
    let mut attempt_index = 0usize;
    let mut gate_index = 0usize;
    let mut previous_at = run.created_at_ms;

    while attempt_index < run.attempts.len() || gate_index < run.gates.len() {
        let RunState::Ready {
            step: expected_step,
        } = &expected
        else {
            return Err(RunRecordError::Corrupt);
        };
        let attempt = run.attempts.get(attempt_index);
        let gate = run.gates.get(gate_index);
        let take_attempt = match (attempt, gate) {
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(attempt), Some(gate)) => match attempt.started_at_ms.cmp(&gate.opened_at_ms) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => !matches!(
                    run.pinned
                        .definition
                        .step(expected_step)
                        .ok_or(RunRecordError::Corrupt)?
                        .action,
                    StepAction::HumanGate(_)
                ),
            },
            (None, None) => break,
        };

        if take_attempt {
            let attempt = attempt.ok_or(RunRecordError::Corrupt)?;
            if attempt.step != *expected_step || attempt.started_at_ms < previous_at {
                return Err(RunRecordError::Corrupt);
            }
            previous_at = attempt.finished_at_ms.unwrap_or(attempt.started_at_ms);
            expected = match attempt.state {
                AttemptState::Active => RunState::Active {
                    step: attempt.step.clone(),
                    attempt: attempt.id,
                },
                AttemptState::Paused => RunState::Paused {
                    step: attempt.step.clone(),
                    attempt: attempt.id,
                    checkpoint: attempt.continuation.ok_or(RunRecordError::Corrupt)?,
                },
                _ => predicted_from_attempt(run, attempt)?,
            };
            attempt_index += 1;
        } else {
            let gate = gate.ok_or(RunRecordError::Corrupt)?;
            if gate.step != *expected_step || gate.opened_at_ms < previous_at {
                return Err(RunRecordError::Corrupt);
            }
            previous_at = gate.closed_at_ms.unwrap_or(gate.opened_at_ms);
            expected = match gate.state {
                HumanGateState::AwaitingDecision => RunState::AwaitingHuman {
                    step: gate.step.clone(),
                    gate: gate.id,
                },
                _ => predicted_from_gate(run, gate)?,
            };
            gate_index += 1;
        }
    }

    if run.state == expected
        || matches!(expected, RunState::Ready { .. })
            && matches!(
                run.state,
                RunState::Failed | RunState::Cancelled | RunState::Interrupted
            )
        || matches!(
            (&expected, &run.state),
            (
                RunState::Ready { .. },
                RunState::Escalated {
                    reason: EscalationReason::AttemptLimit,
                    report,
                    ..
                }
            ) if run.gates.last().is_some_and(|gate| {
                gate.state == HumanGateState::RevisionRequested
                    && gate.decision.as_ref() == Some(report)
            })
        )
    {
        return Ok(());
    }
    if let RunState::Ready { step } = &expected
        && run.state == RunState::Completed
        && unchanged_task_gate(run, step)
    {
        return Ok(());
    }
    if run.attempts.is_empty()
        && run.gates.is_empty()
        && matches!(run.source, RunSource::Pending)
        && matches!(
            run.state,
            RunState::InitialisingSource | RunState::Interrupted
        )
    {
        return Ok(());
    }
    Err(RunRecordError::Corrupt)
}

fn predicted_from_attempt(
    run: &WorkflowRun,
    attempt: &AttemptRecord,
) -> Result<RunState, RunRecordError> {
    match attempt.state {
        AttemptState::Completed => {
            let step = run
                .pinned
                .definition
                .step(&attempt.step)
                .ok_or(RunRecordError::Corrupt)?;
            match (&step.review, attempt.review_route) {
                (None, None) => advanced_state(&run.pinned.definition, &attempt.step)
                    .map_err(|_| RunRecordError::Corrupt),
                (Some(_), Some(route)) => {
                    let policy = step.review.as_ref().ok_or(RunRecordError::Corrupt)?;
                    let (expected, next) = review_consumption(
                        &run.pinned.definition,
                        &attempt.step,
                        policy,
                        attempt,
                        &run.artefacts,
                    )
                    .map_err(|_| RunRecordError::Corrupt)?;
                    if expected != route {
                        return Err(RunRecordError::Corrupt);
                    }
                    Ok(next)
                }
                _ => Err(RunRecordError::Corrupt),
            }
        }
        AttemptState::Failed => Ok(RunState::Failed),
        AttemptState::Cancelled => Ok(RunState::Cancelled),
        AttemptState::Interrupted => Ok(RunState::Interrupted),
        AttemptState::Active | AttemptState::Paused => Err(RunRecordError::Corrupt),
    }
}

fn predicted_from_gate(
    run: &WorkflowRun,
    gate: &HumanGateRecord,
) -> Result<RunState, RunRecordError> {
    match gate.state {
        HumanGateState::Approved => {
            let step = run
                .pinned
                .definition
                .step(&gate.step)
                .ok_or(RunRecordError::Corrupt)?;
            if step.review.is_some() {
                return Err(RunRecordError::Corrupt);
            }
            advanced_state(&run.pinned.definition, &gate.step).map_err(|_| RunRecordError::Corrupt)
        }
        HumanGateState::RevisionRequested => {
            let step = run
                .pinned
                .definition
                .step(&gate.step)
                .ok_or(RunRecordError::Corrupt)?;
            let StepAction::HumanGate(action) = &step.action else {
                return Err(RunRecordError::Corrupt);
            };
            let target = action
                .revision
                .as_ref()
                .ok_or(RunRecordError::Corrupt)?
                .revision_target
                .clone();
            Ok(RunState::Ready { step: target })
        }
        HumanGateState::Cancelled => Ok(RunState::Cancelled),
        HumanGateState::Interrupted => Ok(RunState::Interrupted),
        HumanGateState::AwaitingDecision => Err(RunRecordError::Corrupt),
    }
}

fn validate_source(run: &WorkflowRun) -> Result<(), RunRecordError> {
    match &run.source {
        RunSource::None if run.agent_id.is_none() => Ok(()),
        RunSource::None => Err(RunRecordError::Corrupt),
        RunSource::Pending => Ok(()),
        RunSource::Captured { source } => {
            validate_source_ref(run, &source.initial)?;
            validate_source_ref(run, &source.accepted)?;
            match &source.observed {
                ObservedCandidate::Exact { artefact } => validate_source_ref(run, artefact),
                ObservedCandidate::Unknown => Ok(()),
            }
        }
    }
}

fn validate_source_ref(
    run: &WorkflowRun,
    reference: &ArtefactReference,
) -> Result<(), RunRecordError> {
    if reference.kind != crate::workflows::definition::ArtefactKind::CandidateRevision {
        return Err(RunRecordError::Corrupt);
    }
    referenced_artefact(run, reference)?;
    Ok(())
}

fn referenced_artefact<'a>(
    run: &'a WorkflowRun,
    reference: &ArtefactReference,
) -> Result<&'a ArtefactRecord, RunRecordError> {
    run.artefacts
        .iter()
        .find(|record| artefact_matches_reference(record, reference))
        .ok_or(RunRecordError::Corrupt)
}

fn artefact_matches_reference(record: &ArtefactRecord, reference: &ArtefactReference) -> bool {
    record.id == reference.id
        && record.kind == reference.kind
        && record.artefact_hash == reference.artefact_hash
}

fn validate_artefacts(run: &WorkflowRun) -> Result<(), RunRecordError> {
    for (index, record) in run.artefacts.iter().enumerate() {
        if run.artefacts[..index]
            .iter()
            .any(|earlier| earlier.id == record.id)
            || record.provenance.run_id != run.id
            || record.created_at_ms < run.created_at_ms
            || !artefact_kind_matches_summary(record)
        {
            return Err(RunRecordError::Corrupt);
        }
        for input in &record.provenance.inputs {
            if !run.artefacts[..index]
                .iter()
                .any(|stored| artefact_matches_reference(stored, input))
            {
                return Err(RunRecordError::Corrupt);
            }
        }
        match &record.provenance.producer {
            crate::workflows::artefacts::ArtefactProducer::RunSourceCapture => {
                let RunSource::Captured { source } = &run.source else {
                    return Err(RunRecordError::Corrupt);
                };
                if record.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
                    || !record.provenance.inputs.is_empty()
                    || !artefact_matches_reference(record, &source.initial)
                {
                    return Err(RunRecordError::Corrupt);
                }
            }

            crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                attempt_id,
                step,
                output,
                disposition,
            } => {
                let attempt = run
                    .attempts
                    .iter()
                    .find(|attempt| attempt.id == *attempt_id)
                    .ok_or(RunRecordError::Corrupt)?;
                if attempt.step != *step
                    || matches!(
                        &record.summary,
                        crate::workflows::artefacts::ArtefactSummary::Candidate {
                            disposition: summary_disposition,
                            ..
                        } if summary_disposition != disposition
                    )
                {
                    return Err(RunRecordError::Corrupt);
                }
                match disposition {
                    crate::workflows::artefacts::ProductionDisposition::RequiredOutput => {
                        let output = output.as_ref().ok_or(RunRecordError::Corrupt)?;
                        if !attempt.outputs.iter().any(|attempt_output| {
                            attempt_output.key == *output
                                && artefact_matches_reference(record, &attempt_output.artefact)
                        }) {
                            return Err(RunRecordError::Corrupt);
                        }
                    }
                    crate::workflows::artefacts::ProductionDisposition::ObservedAfterFailure
                    | crate::workflows::artefacts::ProductionDisposition::SourceDrift => {
                        if output.is_some()
                            || record.kind
                                != crate::workflows::definition::ArtefactKind::CandidateRevision
                        {
                            return Err(RunRecordError::Corrupt);
                        }
                    }
                }
            }
            crate::workflows::artefacts::ArtefactProducer::HumanGate {
                gate_id,
                step,
                output,
            } => {
                let gate = run
                    .gates
                    .iter()
                    .find(|gate| gate.id == *gate_id)
                    .ok_or(RunRecordError::Corrupt)?;
                let plan_gate = run
                    .pinned
                    .definition
                    .step(&gate.step)
                    .and_then(|step| match &step.action {
                        StepAction::HumanGate(action) => Some(action.is_plan_checkpoint()),
                        _ => None,
                    })
                    .ok_or(RunRecordError::Corrupt)?;
                if gate.step != *step
                    || gate.output != *output
                    || (plan_gate
                        && record.kind != crate::workflows::definition::ArtefactKind::PlanDecision)
                    || (!plan_gate
                        && record.kind != crate::workflows::definition::ArtefactKind::HumanDecision)
                    || gate
                        .decision
                        .as_ref()
                        .is_none_or(|decision| !artefact_matches_reference(record, decision))
                {
                    return Err(RunRecordError::Corrupt);
                }
            }
        }
    }
    Ok(())
}

fn artefact_kind_matches_summary(record: &ArtefactRecord) -> bool {
    matches!(
        (&record.kind, &record.summary),
        (
            crate::workflows::definition::ArtefactKind::Plan,
            crate::workflows::artefacts::ArtefactSummary::Plan { .. },
        ) | (
            crate::workflows::definition::ArtefactKind::CandidateRevision,
            crate::workflows::artefacts::ArtefactSummary::Candidate { .. },
        ) | (
            crate::workflows::definition::ArtefactKind::ReviewReport,
            crate::workflows::artefacts::ArtefactSummary::Review { .. },
        ) | (
            crate::workflows::definition::ArtefactKind::TestReport,
            crate::workflows::artefacts::ArtefactSummary::Test { .. },
        ) | (
            crate::workflows::definition::ArtefactKind::HumanDecision,
            crate::workflows::artefacts::ArtefactSummary::HumanDecision { .. },
        ) | (
            crate::workflows::definition::ArtefactKind::PlanDecision,
            crate::workflows::artefacts::ArtefactSummary::PlanDecision { .. },
        ),
    )
}

#[cfg(test)]
mod tests;
