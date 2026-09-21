use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::agents::{AgentRecord, ToolId};
use crate::workflows::artefacts::{ArtefactHash, ArtefactReference};
use crate::workflows::{ArtefactId, RunId};

use crate::providers::ModelSelection;
use crate::sessions::JobId;

use super::history::{
    ConversationMessage, MessageRole, MessageStatus, settle_interrupted_questions, valid_activity,
    valid_continuation, valid_message_error,
};
use super::id::{ConversationId, MessageId};
use super::questions::QuestionWaiters;

mod handoff;
mod sqlite;

use sqlite::Database;

const CATALOGUE_VERSION: u32 = 1;
pub(crate) const MAXIMUM_TITLE_BYTES: usize = 120;
pub(crate) const MAXIMUM_MESSAGE_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_REPLY_BYTES: usize = 128 * 1024;
const MAXIMUM_LINKED_REVIEWS: usize = 32;
const MAXIMUM_REVIEW_BRIEF_BYTES: usize = MAXIMUM_MESSAGE_BYTES;
/// One transport window of retained history. It bounds the browser and the
/// response payload, never the retained records themselves.
pub(crate) const TRANSCRIPT_WINDOW: usize = 64;
/// One bounded page of the read-only conversation tree.
pub(crate) const TREE_PAGE: usize = 64;

/// A validated position inside the append order of one conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptCursor {
    Before(MessageId),
    After(MessageId),
    Around(MessageId),
}

impl TranscriptCursor {
    pub(crate) fn message(self) -> MessageId {
        match self {
            Self::Before(id) | Self::After(id) | Self::Around(id) => id,
        }
    }
}

/// One bounded window of retained entries. Messages keep presentation order.
/// The anchors and existence flags follow the immutable append order, so a
/// byte-budget omission in the view never changes cursor progress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TranscriptWindow {
    pub(crate) messages: Vec<ConversationMessage>,
    pub(crate) has_before: bool,
    pub(crate) has_after: bool,
    /// The entry that an Earlier link from this window selects before.
    pub(crate) before_anchor: Option<MessageId>,
    /// The entry that a Later link from this window selects after.
    pub(crate) after_anchor: Option<MessageId>,
    /// Total retained entries in append order.
    pub(crate) total: usize,
    /// True when the window is the live tail rather than a browsed history page.
    pub(crate) live: bool,
}

/// Tree text excludes tool bodies. Tool details use the transcript route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TreeEntry {
    pub(crate) id: MessageId,
    pub(crate) parent: Option<MessageId>,
    pub(crate) role: MessageRole,
    pub(crate) status: MessageStatus,
    pub(crate) text: String,
    pub(crate) on_active_path: bool,
    /// Immutable append order. It orders tree pages and breaks ties.
    pub(crate) sequence: i64,
}

/// One bounded page of tree entries. `partial` reports that a bounded search
/// stopped at its work budget instead of an exhaustive match set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TreeWindow {
    pub(crate) entries: Vec<TreeEntry>,
    pub(crate) active_leaf: Option<MessageId>,
    pub(crate) has_more: bool,
    pub(crate) next_cursor: Option<i64>,
    pub(crate) total: usize,
    pub(crate) partial: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateReviewLink {
    pub(crate) conversation_id: Option<ConversationId>,
    pub(crate) run_id: RunId,
    pub(crate) candidate: ArtefactReference,
    pub(crate) diff_base: ArtefactReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateReviewContext {
    pub(crate) source: CandidateReviewLink,
    pub(crate) task_brief: String,
}

pub(crate) struct CandidateReviewCreation {
    pub(crate) source_conversation: Option<(ConversationId, u32)>,
    pub(crate) title: String,
    pub(crate) model: ConversationModelConfiguration,
    pub(crate) run_id: RunId,
    pub(crate) candidate: ArtefactReference,
    pub(crate) diff_base: ArtefactReference,
    pub(crate) task_brief: String,
    pub(crate) source_at_safe_gate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationRecord {
    pub(crate) id: ConversationId,
    pub(crate) revision: u32,
    pub(crate) title: String,
    pub(crate) title_pending: bool,
    pub(crate) network: crate::agents::NetworkAccess,
    pub(crate) model: Option<ConversationModelConfiguration>,
    // Approvals survive restarts, but access settings changes revoke them.
    // The digest covers execution access, not model selection or instructions.
    pub(crate) directory_approvals: Vec<DirectoryApproval>,

    pub(crate) source_candidate_review: Option<CandidateReviewLink>,
    pub(crate) candidate_reviews: Vec<CandidateReviewLink>,
    pub(crate) candidate_review_context: Option<CandidateReviewContext>,
    /// A fork records its source boundary. It holds no mutable alias.
    pub(crate) forked_from: Option<super::forks::ForkProvenance>,
    /// The active path in root-first order. It is the only projection that
    /// appends and the only path a provider request includes. Off-path
    /// entries stay in the store and are read through the tree.
    pub(crate) messages: Vec<ConversationMessage>,
    pub(crate) active_job: Option<JobId>,
    pub(crate) continuation: Option<super::history::ContinuationCheckpoint>,
    pub(crate) compaction: Option<super::compaction::CompactionRecord>,
    pub(crate) summary_requests: Vec<super::history::RequestUsage>,
    pub(crate) queue: super::queue::ConversationQueue,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

/// Catalogue metadata without message content. List and status views use this
/// projection so a metadata change never loads every retained transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationMetadata {
    pub(crate) id: ConversationId,
    pub(crate) revision: u32,
    pub(crate) title: String,
    pub(crate) title_pending: bool,
    pub(crate) network: crate::agents::NetworkAccess,
    pub(crate) model: Option<ConversationModelConfiguration>,
    pub(crate) active_job: Option<JobId>,
    pub(crate) continuation: bool,
    /// The tail of the active path. It identifies the retained entry that the
    /// next append extends and the transcript shows as the live branch.
    pub(crate) active_leaf: Option<MessageId>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) last_message_status: Option<MessageStatus>,
}

impl ConversationRecord {
    #[cfg(test)]
    pub(crate) fn metadata(&self) -> ConversationMetadata {
        ConversationMetadata {
            id: self.id,
            revision: self.revision,
            title: self.title.clone(),
            title_pending: self.title_pending,
            network: self.network.clone(),
            model: self.model.clone(),
            active_job: self.active_job,
            continuation: self.continuation.is_some(),
            active_leaf: self.messages.last().map(|message| message.id),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            last_message_status: self.messages.last().map(|message| message.status),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationModelConfiguration {
    pub(crate) settings: crate::execution::ExecutionSettings,
    pub(crate) preset: Option<AppliedPreset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppliedPreset {
    pub(crate) id: crate::presets::PresetId,
    pub(crate) revision: u32,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryApproval {
    pub(crate) settings_digest: [u8; 32],
    pub(crate) root: PathBuf,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) access: crate::execution::DirectoryAccess,
}

impl DirectoryApproval {
    pub(crate) fn for_grant(
        settings: &crate::execution::ExecutionSettings,
        grant: &crate::execution::DirectoryGrant,
    ) -> Self {
        Self {
            settings_digest: crate::execution::settings_digest(settings),
            root: grant.host_path.clone(),
            device: grant.identity.device,
            inode: grant.identity.inode,
            access: grant.access,
        }
    }

    pub(crate) fn matches(
        &self,
        settings: &crate::execution::ExecutionSettings,
        grant: &crate::execution::DirectoryGrant,
    ) -> bool {
        self.settings_digest == crate::execution::settings_digest(settings)
            && self.root == grant.host_path
            && self.device == grant.identity.device
            && self.inode == grant.identity.inode
            && self.access == grant.access
    }
}

impl ConversationModelConfiguration {
    pub(crate) fn direct(
        selection: ModelSelection,
        environment: crate::environments::EnvironmentId,
    ) -> Self {
        Self {
            settings: crate::execution::ExecutionSettings::new(
                selection,
                String::new(),
                Vec::new(),
                environment,
            )
            .expect("empty conversation settings are valid"),
            preset: None,
        }
    }

    pub(crate) fn from_preset(record: &crate::presets::PresetRecord) -> Self {
        Self {
            settings: record.settings.clone(),
            preset: Some(AppliedPreset {
                id: record.id,
                revision: record.revision,
                name: record.name.clone(),
            }),
        }
    }

    pub(crate) fn from_agent_snapshot(
        record: &AgentRecord,
        selection: ModelSelection,
        environment: crate::environments::EnvironmentId,
    ) -> Self {
        Self {
            settings: crate::execution::ExecutionSettings::new(
                selection,
                record.instructions.clone(),
                record.tools.clone(),
                environment,
            )
            .and_then(|settings| settings.with_network(record.network.clone()))
            .expect("stored agent settings are valid"),
            preset: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConversationError {
    Random,
    Persist,
    Busy,
    Unsettled,
    Corrupt,
    Full,
    Missing,
    Conflict,
    Revision,
    Title,
    Message,
    Entry,
    Active,
    Selection,
    Network,
    Directories,
    Review,
}

impl ConversationError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Power Plant could not create a conversation identifier. Try again.",
            Self::Persist => "Power Plant cannot store the conversation.",
            Self::Busy => "The conversation database is busy. The request did not commit.",
            Self::Unsettled => {
                "The local history commit has an uncertain outcome. Restart before you continue."
            }
            Self::Corrupt => "Stored conversation history is unreadable.",
            Self::Full => "The disk has no space for more conversation data.",
            Self::Missing => "That conversation is not in the catalogue.",
            Self::Conflict => "That conversation changed in another tab. Reload it.",
            Self::Revision => "Power Plant cannot update this conversation again.",
            Self::Title => "Enter a title of 1 to 120 bytes without control characters.",
            Self::Message => "Enter a message within the conversation limit.",
            Self::Entry => "That entry is not part of this conversation.",
            Self::Active => "This conversation has an active request. Wait for it to finish.",
            Self::Selection => "Choose an available model before you send a message.",
            Self::Network => "Choose valid network access for this conversation.",
            Self::Directories => "Choose valid non-overlapping directories for this conversation.",
            Self::Review => "That plan review hand-off is no longer available.",
        }
    }
}

impl std::fmt::Display for ConversationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ConversationError {}

pub(crate) struct ConversationStore {
    // This lock also protects ownership validation through the authoritative run commit.
    database: Mutex<Database>,
    // A commit I/O failure can leave the outcome unknown. Block new work until restart.
    uncertain: Mutex<std::collections::BTreeSet<ConversationId>>,
    title_updates: tokio::sync::broadcast::Sender<()>,
    questions: QuestionWaiters,
}

/// The durable, message-free projection of one conversation. Messages and
/// summary requests live in their own tables.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct MetadataFile {
    version: u32,
    id: String,
    revision: u32,
    title: String,
    #[serde(default)]
    title_pending: bool,
    network: String,
    network_domains: Vec<String>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    model: Option<ConversationModelFile>,
    directory_approvals: Vec<DirectoryApprovalFile>,

    #[serde(deserialize_with = "crate::storage::required_option")]
    source_candidate_review: Option<CandidateReviewLinkFile>,
    candidate_reviews: Vec<CandidateReviewLinkFile>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    candidate_review_context: Option<CandidateReviewContextFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forked_from: Option<ForkProvenanceFile>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    active_job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<ContinuationFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionFile>,
    /// The retained entry at the end of the active path. A load walks its
    /// parent chain to rebuild that path without reading every branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_leaf: Option<String>,
    #[serde(default)]
    queue_revision: u32,
    #[serde(default)]
    queue: Vec<QueueItemFile>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ContinuationFile {
    id: String,
    boundary: String,
    pinned: ConversationModelFile,
    budget: BudgetFile,
    #[serde(deserialize_with = "crate::storage::required_option")]
    run: Option<String>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    attempt: Option<String>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    step: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    drafts: Vec<PausedDraftFile>,
    created_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CompactionFile {
    covered_through: String,
    retained_from: String,
    text: String,
    request: super::history::RequestUsage,
    created_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct BudgetFile {
    model_requests: u32,
    model_request_limit: u32,
    tool_dispatches: u32,
    tool_dispatch_limit: u32,
    elapsed_ms: u64,
    elapsed_limit_ms: u64,
    reason: String,
}

#[derive(Deserialize, Serialize)]
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
struct QueueItemFile {
    id: String,
    text: String,
    delivery: String,
    settings_digest: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    job: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ConversationModelFile {
    selection: ModelSelection,
    instructions: String,
    tools: Vec<String>,
    environment: String,
    directories: Vec<DirectoryGrantFile>,
    location: String,
    host_approval: String,
    #[serde(deserialize_with = "crate::storage::required_option")]
    preset: Option<AppliedPresetFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct DirectoryGrantFile {
    id: String,
    host_path: PathBuf,
    device: u64,
    inode: u64,
    alias: String,
    access: crate::execution::DirectoryAccess,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct DirectoryApprovalFile {
    settings_digest: String,
    host_path: PathBuf,
    device: u64,
    inode: u64,
    access: crate::execution::DirectoryAccess,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct AppliedPresetFile {
    id: String,
    revision: u32,
    name: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CandidateReviewLinkFile {
    conversation: Option<String>,
    run: String,
    candidate: ArtefactRefFile,
    diff_base: ArtefactRefFile,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CandidateReviewContextFile {
    source: CandidateReviewLinkFile,
    task_brief: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ForkProvenanceFile {
    source: String,
    source_revision: u32,
    boundary: String,
    #[serde(default)]
    candidate_review: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ArtefactRefFile {
    id: String,
    kind: String,
    artefact_hash: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct MessageFile {
    id: String,
    /// The immutable parent identity inside the conversation. Absent means the
    /// entry starts a path. The store validates it before use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    role: MessageRole,
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    activity: Vec<crate::providers::AssistantActivity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    continuation: Vec<super::history::ContinuationMetadata>,
    status: MessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<crate::providers::CompletionReason>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requests: Vec<super::history::RequestUsage>,
}

impl ConversationStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, ConversationError> {
        let database = Database::open(&dir)?;
        let store = Self {
            database: Mutex::new(database),
            uncertain: Mutex::new(std::collections::BTreeSet::new()),
            title_updates: tokio::sync::broadcast::channel(16).0,
            questions: QuestionWaiters::new(),
        };
        store.interrupt_requests()?;
        Ok(store)
    }

    pub(crate) fn metadata(&self) -> Vec<ConversationMetadata> {
        self.try_metadata().unwrap_or_default()
    }

    pub(crate) fn try_metadata(&self) -> Result<Vec<ConversationMetadata>, ConversationError> {
        self.database().metadata_all()
    }

    pub(crate) fn metadata_for(&self, id: &ConversationId) -> Option<ConversationMetadata> {
        self.database().metadata(id).ok().flatten()
    }

    pub(crate) fn contains(&self, id: &ConversationId) -> bool {
        self.database().contains(id).unwrap_or(true)
    }

    pub(crate) fn get(&self, id: &ConversationId) -> Option<ConversationRecord> {
        self.database().load(id).ok().flatten()
    }

    /// Load metadata and one bounded transcript window without reading every
    /// retained message body. `Err(Entry)` reports a foreign cursor entry.
    pub(crate) fn transcript_window(
        &self,
        id: &ConversationId,
        cursor: Option<TranscriptCursor>,
        leaf: Option<MessageId>,
    ) -> Result<Option<(ConversationRecord, TranscriptWindow)>, ConversationError> {
        let database = self.database();
        let Some(shell) = database.load_shell(id)? else {
            return Ok(None);
        };
        let window = database.transcript_window(id, cursor, leaf, TRANSCRIPT_WINDOW)?;
        let mut shell = shell;
        shell.messages = window.messages.clone();
        Ok(Some((shell, window)))
    }

    /// Load one bounded page of retained entries for the read-only tree. It
    /// never loads a full record or an active path.
    pub(crate) fn tree_window(
        &self,
        id: &ConversationId,
        after: Option<i64>,
        search: Option<&str>,
        parent: Option<MessageId>,
    ) -> Result<Option<TreeWindow>, ConversationError> {
        let database = self.database();
        if database.load_shell(id)?.is_none() {
            return Ok(None);
        }
        database.tree_window(id, after, search, parent).map(Some)
    }

    pub(crate) fn create_saved(
        &self,
        id: ConversationId,
        title: Option<String>,
        model: Option<ConversationModelConfiguration>,
        directory_approvals: Vec<DirectoryApproval>,
    ) -> Result<ConversationRecord, ConversationError> {
        if !valid_directory_approvals(&model, &directory_approvals) {
            return Err(ConversationError::Directories);
        }
        let title_pending = title.is_none();
        let title = match title {
            Some(title) => normalise_title(&title)?,
            None => "New conversation".to_owned(),
        };
        let network = model
            .as_ref()
            .map(|model| model.settings.network.clone())
            .unwrap_or_default();
        let mut database = self.database();
        if database.contains(&id)? {
            return Err(ConversationError::Conflict);
        }
        let now = now_ms();
        let record = ConversationRecord {
            id,
            revision: 1,
            title,
            title_pending,
            network,
            model,
            directory_approvals,

            source_candidate_review: None,
            candidate_reviews: Vec::new(),
            candidate_review_context: None,
            forked_from: None,
            summary_requests: Vec::new(),
            messages: Vec::new(),
            active_job: None,
            continuation: None,
            compaction: None,
            queue: super::queue::ConversationQueue::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.persist(&mut database, None, &record)?;
        Ok(record)
    }

    /// Create the destination conversation for a fork on its first send. The
    /// caller supplies copied entries with source identities and destination
    /// output references. No directory approval or runtime consent is copied.
    pub(crate) fn create_fork(
        &self,
        id: ConversationId,
        title: Option<String>,
        model: Option<ConversationModelConfiguration>,
        directory_approvals: Vec<DirectoryApproval>,
        messages: Vec<ConversationMessage>,
        snapshot: &super::forks::ForkSnapshot,
    ) -> Result<ConversationRecord, ConversationError> {
        if !valid_directory_approvals(&model, &directory_approvals) {
            return Err(ConversationError::Directories);
        }
        let title_pending = title.is_none();
        let title = match title {
            Some(title) => normalise_title(&title)?,
            None => "New conversation".to_owned(),
        };
        super::history::validate_exchange(&messages).map_err(|_| ConversationError::Corrupt)?;
        let network = model
            .as_ref()
            .map(|model| model.settings.network.clone())
            .unwrap_or_default();
        let mut database = self.database();
        if database.contains(&id)? {
            return Err(ConversationError::Conflict);
        }
        let now = now_ms();
        let mut record = ConversationRecord {
            id,
            revision: 1,
            title,
            title_pending,
            network,
            model,
            directory_approvals,

            source_candidate_review: snapshot
                .review_context
                .as_ref()
                .map(|context| context.source.clone()),
            candidate_reviews: Vec::new(),
            candidate_review_context: snapshot.review_context.clone(),
            forked_from: Some(super::forks::ForkProvenance {
                source: snapshot.source,
                source_revision: snapshot.source_revision,
                boundary: snapshot.boundary,
                candidate_review: snapshot.candidate_review,
            }),
            summary_requests: Vec::new(),
            messages,
            active_job: None,
            continuation: None,
            compaction: None,
            queue: super::queue::ConversationQueue::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        project_active_path(&mut record);
        self.persist(&mut database, None, &record)?;
        Ok(record)
    }

    pub(crate) fn create_candidate_review(
        &self,
        creation: CandidateReviewCreation,
    ) -> Result<ConversationRecord, ConversationError> {
        let CandidateReviewCreation {
            source_conversation,
            title,
            model,
            run_id,
            candidate,
            diff_base,
            task_brief,
            source_at_safe_gate,
        } = creation;
        let title = normalise_title(&title)?;
        let task_brief = normalise_message(&task_brief)?;
        if task_brief.len() > MAXIMUM_REVIEW_BRIEF_BYTES
            || candidate.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
            || diff_base.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
        {
            return Err(ConversationError::Review);
        }
        let mut database = self.database();
        let source = source_conversation
            .map(|(id, revision)| {
                self.require_durable(&id)?;
                let source = database.load(&id)?.ok_or(ConversationError::Missing)?;
                if source.revision != revision {
                    return Err(ConversationError::Conflict);
                }
                if source.active_job.is_some() && !source_at_safe_gate {
                    return Err(ConversationError::Active);
                }
                if source.candidate_reviews.len() >= MAXIMUM_LINKED_REVIEWS {
                    return Err(ConversationError::Review);
                }
                Ok(source)
            })
            .transpose()?;
        let id = unused_identifier(&database)?;
        let now = now_ms();
        let source_link = CandidateReviewLink {
            conversation_id: source.as_ref().map(|source| source.id),
            run_id,
            candidate: candidate.clone(),
            diff_base: diff_base.clone(),
        };
        let review_link = CandidateReviewLink {
            conversation_id: Some(id),
            run_id,
            candidate,
            diff_base,
        };
        let review = ConversationRecord {
            id,
            revision: 1,
            title,
            title_pending: false,
            network: crate::agents::NetworkAccess::None,
            model: Some(model),
            directory_approvals: Vec::new(),

            source_candidate_review: Some(source_link.clone()),
            candidate_reviews: Vec::new(),
            candidate_review_context: Some(CandidateReviewContext {
                source: source_link,
                task_brief,
            }),
            forked_from: None,
            summary_requests: Vec::new(),
            messages: Vec::new(),
            active_job: None,
            continuation: None,
            compaction: None,
            queue: super::queue::ConversationQueue::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut updated_source = None;
        if let Some(source) = source.as_ref() {
            let mut updated = source.clone();
            updated.revision = source
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            updated.updated_at_ms = now.max(source.updated_at_ms);
            updated.candidate_reviews.push(review_link);
            updated_source = Some(updated);
        }
        match (source.as_ref(), updated_source.as_ref()) {
            (Some(previous), Some(updated)) => {
                self.persist_pair(&mut database, None, &review, Some(previous), updated)?;
            }
            _ => self.persist(&mut database, None, &review)?,
        }
        Ok(review)
    }

    pub(crate) fn rename(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        title: String,
    ) -> Result<ConversationRecord, ConversationError> {
        let title = normalise_title(&title)?;
        self.replace(id, expected_revision, |current| {
            current.title = title;
            current.title_pending = false;
            Ok(())
        })
    }

    pub(crate) fn subscribe_titles(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.title_updates.subscribe()
    }

    pub(crate) fn claim_title(&self, id: &ConversationId) -> Option<ConversationRecord> {
        self.replace(id, 0, |record| {
            if !record.title_pending {
                return Err(ConversationError::Conflict);
            }
            super::titles::exchange(record).ok_or(ConversationError::Conflict)?;
            record.title_pending = false;
            Ok(())
        })
        .ok()
    }

    // An automatic title is presentation-only. It must not invalidate open command forms.
    // A manual rename still changes the revision and therefore wins this comparison.
    pub(crate) fn save_automatic_title(
        &self,
        id: &ConversationId,
        revision: u32,
        title: String,
    ) -> Result<(), ConversationError> {
        let title = normalise_title(&title)?;
        let mut database = self.database();
        self.require_durable(id)?;
        self.commit_result(*id, database.save_automatic_title(id, revision, &title))?;
        let _ = self.title_updates.send(());
        Ok(())
    }

    pub(crate) fn set_network(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        network: crate::agents::NetworkAccess,
    ) -> Result<ConversationRecord, ConversationError> {
        let network = network.validate().map_err(|_| ConversationError::Network)?;
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            current.network = network.clone();
            if let Some(model) = &mut current.model {
                model.settings.network = network;
            }
            Ok(())
        })
    }

    pub(crate) fn select_model(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        selection: ModelSelection,
        environment: crate::environments::EnvironmentId,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            if let Some(model) = &mut current.model {
                model.settings.model = selection;
            } else {
                let mut model = ConversationModelConfiguration::direct(selection, environment);
                model.settings.network = current.network.clone();
                current.model = Some(model);
            }
            Ok(())
        })
    }

    pub(crate) fn apply_preset(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        preset: &crate::presets::PresetRecord,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            current.directory_approvals.clear();
            current.network = preset.settings.network.clone();
            current.model = Some(ConversationModelConfiguration::from_preset(preset));
            Ok(())
        })
    }

    pub(crate) fn update_execution_settings(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        settings: crate::execution::ExecutionSettings,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            current.network = settings.network.clone();
            let preset = current
                .model
                .as_ref()
                .and_then(|model| model.preset.clone());
            current.model = Some(ConversationModelConfiguration { settings, preset });
            Ok(())
        })
    }

    pub(crate) fn add_directory(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        grant: crate::execution::DirectoryGrant,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            let model = current.model.as_mut().ok_or(ConversationError::Selection)?;
            let mut directories = model.settings.directories.clone();
            directories.push(grant.clone());
            model.settings = model
                .settings
                .clone()
                .with_directories(directories)
                .ok_or(ConversationError::Directories)?;
            Ok(())
        })
    }

    pub(crate) fn update_directory(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        grant: crate::execution::DirectoryGrant,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            let model = current.model.as_mut().ok_or(ConversationError::Selection)?;
            let index = model
                .settings
                .directories
                .iter()
                .position(|stored| stored.id == grant.id)
                .ok_or(ConversationError::Directories)?;
            let mut directories = model.settings.directories.clone();
            directories[index] = grant.clone();
            model.settings = model
                .settings
                .clone()
                .with_directories(directories)
                .ok_or(ConversationError::Directories)?;
            Ok(())
        })
    }

    pub(crate) fn record_directory_approval(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        approval: DirectoryApproval,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            current.directory_approvals.retain(|stored| {
                stored.settings_digest == approval.settings_digest && stored != &approval
            });
            if current.directory_approvals.len() >= crate::execution::MAXIMUM_DIRECTORY_GRANTS {
                return Err(ConversationError::Directories);
            }
            current.directory_approvals.push(approval);
            Ok(())
        })
    }

    pub(crate) fn directory_approved(
        &self,
        id: &ConversationId,
        settings: &crate::execution::ExecutionSettings,
        grant: &crate::execution::DirectoryGrant,
    ) -> bool {
        self.database()
            .directory_approved(id, settings, grant)
            .unwrap_or(false)
    }

    pub(crate) fn remove_directory(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        grant_id: crate::execution::DirectoryGrantId,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            let model = current.model.as_mut().ok_or(ConversationError::Selection)?;
            let index = model
                .settings
                .directories
                .iter()
                .position(|grant| grant.id == grant_id)
                .ok_or(ConversationError::Directories)?;
            model.settings.directories.remove(index);
            Ok(())
        })
    }

    pub(crate) fn begin_message_with_model(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        model: Option<ConversationModelConfiguration>,
        request: JobId,
        text: String,
    ) -> Result<ConversationRecord, ConversationError> {
        let text = normalise_message(&text)?;
        let user_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        let assistant_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            if let Some(model) = model {
                current.model = Some(model);
            }
            current.messages.push(ConversationMessage {
                parent: None,
                id: user_id,
                role: MessageRole::User,
                text,
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
                requests: Vec::new(),
            });
            current.messages.push(ConversationMessage {
                parent: None,
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
                requests: Vec::new(),
            });
            current.active_job = Some(request);
            Ok(())
        })
    }

    pub(crate) fn append_output(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: impl Into<crate::providers::AssistantReply>,
    ) -> Result<(), ConversationError> {
        let reply = reply.into();
        validate_reply(&reply)?;
        self.update_output(id, request, reply)
    }

    pub(crate) fn checkpoint_output(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: impl Into<crate::providers::AssistantReply>,
    ) -> Result<(), ConversationError> {
        let reply = reply.into();
        validate_reply(&reply)?;
        self.update_output(id, request, reply)
    }

    pub(crate) fn record_provider_failure(
        &self,
        id: &ConversationId,
        request: JobId,
        failed: crate::providers::AssistantReply,
        committed: &crate::providers::AssistantReply,
        error: String,
    ) -> Result<(), ConversationError> {
        validate_reply(&failed)?;
        validate_reply(committed)?;
        if !valid_message_error(MessageStatus::Failed, Some(&error)) {
            return Err(ConversationError::Message);
        }
        self.update(id, 0, false, |record| {
            let active = active_assistant(record, request)?;
            apply_reply(active, committed);
            let mut pending = active.clone();
            pending.id = MessageId::generate().map_err(|_| ConversationError::Random)?;
            let failed = ConversationMessage {
                parent: active.parent,
                id: active.id,
                role: MessageRole::Assistant,
                text: failed.text,
                activity: failed.activity,
                continuation: Vec::new(),
                status: MessageStatus::Failed,
                error: Some(error),
                request: Some(JobId::generate().map_err(|_| ConversationError::Random)?),
                completion: Some(crate::providers::CompletionReason::Unknown),
                requests: failed.usage,
            };
            // Retain the original identity and parent. The retry extends it.
            *active = failed;
            record.messages.push(pending);
            super::history::validate_exchange(&record.messages)
                .map_err(|_| ConversationError::Message)?;
            Ok(())
        })
        .map(|_| ())
    }

    fn update_output(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: crate::providers::AssistantReply,
    ) -> Result<(), ConversationError> {
        let mut database = self.database();
        self.require_durable(id)?;
        self.commit_result(*id, database.checkpoint_output(id, request, reply))
    }

    pub(crate) fn settle_message(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: impl Into<crate::providers::AssistantReply>,
        status: MessageStatus,
        error: Option<String>,
    ) -> Result<(), ConversationError> {
        let reply = reply.into();
        validate_reply(&reply)?;
        if !valid_message_error(status, error.as_deref())
            || !matches!(
                status,
                MessageStatus::Complete | MessageStatus::Interrupted | MessageStatus::Failed
            )
        {
            return Err(ConversationError::Message);
        }
        self.replace(id, 0, |current| {
            let message = active_assistant(current, request)?;
            apply_owned_reply(message, reply);
            message.status = status;
            message.error = error;
            current.active_job = None;
            current.continuation = None;
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn pause_for_budget(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: crate::providers::AssistantReply,
        mut checkpoint: super::history::ContinuationCheckpoint,
    ) -> Result<ConversationRecord, ConversationError> {
        validate_reply(&reply)?;
        if !checkpoint.budget.valid() {
            return Err(ConversationError::Message);
        }
        self.replace(id, 0, |current| {
            if current.active_job != Some(request) {
                return Err(ConversationError::Conflict);
            }
            let empty_pending = {
                let message = active_assistant(current, request)?;
                reply.is_empty() && message.text.is_empty() && message.activity.is_empty()
            };
            if empty_pending {
                current.messages.pop();
            } else {
                let message = active_assistant(current, request)?;
                apply_owned_reply(message, reply);
                message.status = MessageStatus::Complete;
                message.error = None;
            }
            current.active_job = None;
            let boundary = current
                .messages
                .iter()
                .rev()
                .find(|message| {
                    message.role == MessageRole::Assistant
                        && message.status == MessageStatus::Complete
                })
                .map(|message| message.id)
                .ok_or(ConversationError::Message)?;
            checkpoint.boundary = boundary;
            if !checkpoint.valid(&current.messages) {
                return Err(ConversationError::Message);
            }
            current.continuation = Some(checkpoint);
            Ok(())
        })
    }

    pub(crate) fn record_summary_request(
        &self,
        id: &ConversationId,
        job: JobId,
        request: &super::history::RequestUsage,
    ) -> Result<(), ConversationError> {
        let mut database = self.database();
        self.require_durable(id)?;
        self.commit_result(*id, database.record_summary_request(id, job, request))
    }

    pub(crate) fn begin_compaction(
        &self,
        id: &ConversationId,
        revision: u32,
        job: JobId,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, revision, |current| {
            if current.active_job.is_some() || current.continuation.is_some() {
                return Err(ConversationError::Active);
            }
            current.messages.push(ConversationMessage {
                parent: None,
                id: MessageId::generate().map_err(|_| ConversationError::Random)?,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(job),
                completion: None,
                requests: Vec::new(),
            });
            current.active_job = Some(job);
            Ok(())
        })
    }

    pub(crate) fn finish_compaction(
        &self,
        id: &ConversationId,
        job: JobId,
        error: Option<&str>,
    ) -> Result<ConversationRecord, ConversationError> {
        self.update(id, 0, false, |current| {
            if current.active_job != Some(job) {
                return Err(ConversationError::Conflict);
            }
            if let Some(error) = error {
                let message = current
                    .messages
                    .last_mut()
                    .ok_or(ConversationError::Message)?;
                message.status = MessageStatus::Failed;
                message.error = Some(error.to_owned());
            } else {
                current.messages.pop();
            }
            current.active_job = None;
            Ok(())
        })
    }

    pub(crate) fn record_job_compaction(
        &self,
        id: &ConversationId,
        request: JobId,
        compaction: super::compaction::CompactionRecord,
    ) -> Result<ConversationRecord, ConversationError> {
        self.update(id, 0, false, |current| {
            if current.active_job != Some(request) {
                return Err(ConversationError::Conflict);
            }
            commit_compaction(current, compaction)
        })
    }

    pub(crate) fn claim_continuation(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        checkpoint: super::id::CheckpointId,
        request: JobId,
    ) -> Result<ConversationRecord, ConversationError> {
        let assistant_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            let Some(stored) = current.continuation.as_ref() else {
                return Err(ConversationError::Conflict);
            };
            if stored.id != checkpoint {
                return Err(ConversationError::Conflict);
            }
            current.messages.push(ConversationMessage {
                parent: None,
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
                requests: Vec::new(),
            });
            current.active_job = Some(request);
            Ok(())
        })
    }

    pub(crate) fn clear_continuation(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        checkpoint: super::id::CheckpointId,
        runs: &crate::workflows::WorkflowRunStore,
    ) -> Result<ConversationRecord, ConversationError> {
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            let stored = current.continuation.as_ref().ok_or(ConversationError::Conflict)?;
            if stored.id != checkpoint {
                return Err(ConversationError::Conflict);
            }
            if let Some(run_id) = stored.run {
                // The conversation lock keeps the displayed revision stable through cancellation.
                // If the second record write fails, the terminal run prevents execution from this checkpoint.
                runs.mutate(&run_id, |run| {
                    if run.conversation_id != Some(*id)
                        || run.pending_handoff.is_some()
                        || !run.attempts.iter().any(|attempt| Some(attempt.id) == stored.attempt && attempt.continuation == Some(checkpoint))
                    {
                        return Err(crate::workflows::run::TransitionError::Invalid);
                    }
                    if run.is_terminal() {
                        return Ok(());
                    }
                    if !matches!(run.state, crate::workflows::run::RunState::Paused { checkpoint: expected, .. } if expected == checkpoint) {
                        return Err(crate::workflows::run::TransitionError::Invalid);
                    }
                    run.cancel(crate::workflows::now_ms())
                }).map_err(|error| match error {
                    crate::workflows::StoreError::Conflict | crate::workflows::StoreError::Missing => ConversationError::Conflict,
                    _ => ConversationError::Persist,
                })?;
            }
            current.continuation = None;
            Ok(())
        })
    }

    pub(crate) fn enqueue(
        &self,
        id: &ConversationId,
        expected_queue_revision: u32,
        text: String,
        delivery: super::queue::QueueDelivery,
        job: Option<JobId>,
    ) -> Result<ConversationRecord, ConversationError> {
        let text = normalise_message(&text)?;
        let item_id =
            super::queue::QueueItemId::generate().map_err(|_| ConversationError::Random)?;
        self.update(id, 0, false, |current| {
            if current.queue.revision != expected_queue_revision {
                return Err(ConversationError::Conflict);
            }
            if current.queue.items.len() >= super::queue::MAXIMUM_QUEUE_ITEMS {
                return Err(ConversationError::Full);
            }
            let aggregate = current
                .queue
                .items
                .iter()
                .map(|item| item.text.len())
                .fold(0usize, usize::saturating_add);
            if aggregate.saturating_add(text.len()) > super::queue::MAXIMUM_QUEUE_BYTES {
                return Err(ConversationError::Full);
            }
            let settings_digest = current
                .model
                .as_ref()
                .map(|model| super::queue::launch_digest(&model.settings))
                .ok_or(ConversationError::Selection)?;
            if delivery == super::queue::QueueDelivery::Steering
                && (job.is_none() || job != current.active_job)
            {
                return Err(ConversationError::Conflict);
            }
            current.queue.items.push(super::queue::QueueItem {
                id: item_id,
                text,
                delivery,
                settings_digest,
                job,
            });
            current.queue.revision = current
                .queue
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            if !super::queue::valid_queue(&current.queue) {
                return Err(ConversationError::Message);
            }
            Ok(())
        })
    }

    pub(crate) fn remove_queue_item(
        &self,
        id: &ConversationId,
        expected_queue_revision: u32,
        item_id: super::queue::QueueItemId,
    ) -> Result<(ConversationRecord, super::queue::QueueItem), ConversationError> {
        let mut removed = None;
        let record = self.update(id, 0, false, |current| {
            if current.queue.revision != expected_queue_revision {
                return Err(ConversationError::Conflict);
            }
            let index = current
                .queue
                .items
                .iter()
                .position(|item| item.id == item_id)
                .ok_or(ConversationError::Conflict)?;
            removed = Some(current.queue.items.remove(index));
            current.queue.revision = current
                .queue
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            Ok(())
        })?;
        Ok((record, removed.expect("removed queued item")))
    }

    pub(crate) fn begin_follow_up(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        expected_queue_revision: u32,
        item_id: super::queue::QueueItemId,
        request: JobId,
        model: Option<ConversationModelConfiguration>,
    ) -> Result<ConversationRecord, ConversationError> {
        let user_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        let assistant_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        self.replace(id, expected_revision, |current| {
            if current.active_job.is_some() {
                return Err(ConversationError::Active);
            }
            if current.queue.revision != expected_queue_revision {
                return Err(ConversationError::Conflict);
            }
            let index = current
                .queue
                .items
                .iter()
                .position(|item| {
                    item.id == item_id && item.delivery == super::queue::QueueDelivery::FollowUp
                })
                .ok_or(ConversationError::Conflict)?;
            if model.as_ref() != current.model.as_ref() {
                return Err(ConversationError::Conflict);
            }
            let settings_digest = current
                .model
                .as_ref()
                .map(|model| super::queue::launch_digest(&model.settings))
                .ok_or(ConversationError::Selection)?;
            if current.queue.items[index].settings_digest != settings_digest {
                return Err(ConversationError::Conflict);
            }
            let item = current.queue.items.remove(index);
            current.queue.revision = current
                .queue
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            current.messages.push(ConversationMessage {
                parent: None,
                id: user_id,
                role: MessageRole::User,
                text: item.text,
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
                requests: Vec::new(),
            });
            current.messages.push(ConversationMessage {
                parent: None,
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
                requests: Vec::new(),
            });
            current.active_job = Some(request);
            Ok(())
        })
    }

    pub(crate) fn settle_tool_batch(
        &self,
        id: &ConversationId,
        job: JobId,
        reply: &crate::providers::AssistantReply,
    ) -> Result<(), ConversationError> {
        validate_reply(reply)?;
        self.update(id, 0, false, |current| {
            let message = active_assistant(current, job)?;
            apply_reply(message, reply);
            message.status = MessageStatus::Complete;
            super::history::project(&current.messages, None)
                .map_err(|_| ConversationError::Unsettled)?;
            current.messages.push(ConversationMessage {
                parent: None,
                id: MessageId::generate().map_err(|_| ConversationError::Random)?,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(job),
                completion: None,
                requests: Vec::new(),
            });
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn deliver_steering(
        &self,
        id: &ConversationId,
        request: JobId,
        item_id: super::queue::QueueItemId,
        reply: crate::providers::AssistantReply,
    ) -> Result<String, ConversationError> {
        validate_reply(&reply)?;
        let user_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        let assistant_id = MessageId::generate().map_err(|_| ConversationError::Random)?;
        let mut delivered = None;
        self.update(id, 0, false, |current| {
            let index = current
                .queue
                .items
                .iter()
                .position(|item| {
                    item.id == item_id
                        && item.delivery == super::queue::QueueDelivery::Steering
                        && item.job == Some(request)
                })
                .ok_or(ConversationError::Conflict)?;
            {
                let message = active_assistant(current, request)?;
                apply_reply(message, &reply);
                message.status = MessageStatus::Complete;
                message.error = None;
            }
            let item = current.queue.items.remove(index);
            current.queue.revision = current
                .queue
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            current.messages.push(ConversationMessage {
                parent: None,
                id: user_id,
                role: MessageRole::User,
                text: item.text.clone(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
                requests: Vec::new(),
            });
            current.messages.push(ConversationMessage {
                parent: None,
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
                requests: Vec::new(),
            });
            super::history::validate_exchange(&current.messages)
                .map_err(|_| ConversationError::Message)?;
            if !super::queue::valid_queue(&current.queue) {
                return Err(ConversationError::Message);
            }
            delivered = Some(item.text);
            Ok(())
        })?;
        Ok(delivered.expect("delivered steering text"))
    }

    pub(crate) fn submit_question(
        &self,
        question: super::questions::PendingQuestion,
    ) -> Result<(), super::questions::QuestionError> {
        self.questions.submit(question)
    }

    pub(crate) fn pending_question(
        &self,
        conversation: ConversationId,
        job: JobId,
    ) -> Option<super::questions::PendingQuestion> {
        self.questions.pending_for(conversation, job)
    }

    pub(crate) fn answer_question(
        &self,
        conversation: ConversationId,
        job: &crate::sessions::Job,
        tool_call: &str,
        answer: super::questions::QuestionAnswer,
    ) -> Result<(), super::questions::QuestionError> {
        self.questions.decide(conversation, job, tool_call, answer)
    }

    pub(crate) async fn wait_question(
        &self,
        conversation: ConversationId,
        job: &crate::sessions::Job,
        tool_call: &str,
    ) -> Result<super::questions::QuestionAnswer, super::questions::QuestionError> {
        self.questions.wait(conversation, job, tool_call).await
    }

    pub(crate) fn invalidate_questions_for_job(&self, job: JobId) {
        self.questions.invalidate_job(job);
    }

    pub(crate) fn invalidate_questions_for_conversation(&self, conversation: ConversationId) {
        self.questions.invalidate_conversation(conversation);
    }

    pub(crate) fn retain_question_sessions(
        &self,
        live: impl Fn(&crate::sessions::SessionId) -> bool,
    ) {
        self.questions.retain_sessions(live);
    }

    pub(crate) fn delete(
        &self,
        id: &ConversationId,
        expected_revision: u32,
    ) -> Result<(), ConversationError> {
        let mut database = self.database();
        self.require_durable(id)?;
        self.questions.invalidate_conversation(*id);
        let Some(current) = database.load(id)? else {
            return Err(ConversationError::Missing);
        };
        if current.revision != expected_revision {
            return Err(ConversationError::Conflict);
        }
        if current.active_job.is_some() {
            return Err(ConversationError::Active);
        }
        self.commit_result(*id, database.remove(id))
    }

    fn replace(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        edit: impl FnOnce(&mut ConversationRecord) -> Result<(), ConversationError>,
    ) -> Result<ConversationRecord, ConversationError> {
        self.update(id, expected_revision, true, edit)
    }

    fn update(
        &self,
        id: &ConversationId,
        expected_revision: u32,
        advance_revision: bool,
        edit: impl FnOnce(&mut ConversationRecord) -> Result<(), ConversationError>,
    ) -> Result<ConversationRecord, ConversationError> {
        let mut database = self.database();
        let Some(current) = database.load(id)? else {
            return Err(ConversationError::Missing);
        };
        if expected_revision != 0 && current.revision != expected_revision {
            return Err(ConversationError::Conflict);
        }
        self.require_durable(id)?;
        let mut updated = current.clone();
        edit(&mut updated)?;
        project_active_path(&mut updated);
        // Reverting settings must not resurrect consent from an earlier configuration.
        let access_digest = |record: &ConversationRecord| {
            record
                .model
                .as_ref()
                .map(|model| crate::execution::settings_digest(&model.settings))
        };
        if access_digest(&current) != access_digest(&updated) {
            updated.directory_approvals.clear();
        }
        if advance_revision {
            updated.revision = current
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
        }
        updated.updated_at_ms = now_ms().max(current.updated_at_ms);
        self.persist(&mut database, Some(&current), &updated)?;
        Ok(updated)
    }

    /// Commit one conversation. An uncertain replacement blocks later work.
    fn persist(
        &self,
        database: &mut Database,
        previous: Option<&ConversationRecord>,
        record: &ConversationRecord,
    ) -> Result<(), ConversationError> {
        self.commit_result(record.id, database.save(previous, record))
    }

    /// Commit two projections in one transaction for an ownership transfer.
    fn persist_pair(
        &self,
        database: &mut Database,
        first: Option<&ConversationRecord>,
        first_record: &ConversationRecord,
        second: Option<&ConversationRecord>,
        second_record: &ConversationRecord,
    ) -> Result<(), ConversationError> {
        let result = database.save_pair(first, first_record, second, second_record);
        let result = self.commit_result(first_record.id, result);
        self.commit_result(second_record.id, result)
    }

    fn commit_result(
        &self,
        id: ConversationId,
        result: Result<(), ConversationError>,
    ) -> Result<(), ConversationError> {
        if result == Err(ConversationError::Unsettled) {
            self.uncertain
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id);
        }
        result
    }

    fn require_durable(&self, id: &ConversationId) -> Result<(), ConversationError> {
        if self
            .uncertain
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(id)
        {
            Err(ConversationError::Unsettled)
        } else {
            Ok(())
        }
    }

    fn database(&self) -> MutexGuard<'_, Database> {
        self.database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn active_assistant(
    record: &mut ConversationRecord,
    request: JobId,
) -> Result<&mut ConversationMessage, ConversationError> {
    if record.active_job != Some(request) {
        return Err(ConversationError::Conflict);
    }
    record
        .messages
        .iter_mut()
        .rev()
        .find(|message| {
            message.request == Some(request) && message.status == MessageStatus::Pending
        })
        .ok_or(ConversationError::Conflict)
}

/// Settle one recovered request. The caller persists only a changed record.
fn interrupt_recovered_request(record: &mut ConversationRecord) -> bool {
    let Some(request) = record.active_job else {
        return false;
    };
    if let Some(message) = record
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.request == Some(request))
    {
        settle_interrupted_questions(message);
        message.status = MessageStatus::Interrupted;
    }
    record.active_job = None;
    record.revision = record.revision.saturating_add(1);
    record.updated_at_ms = now_ms().max(record.updated_at_ms);
    true
}

/// Approvals must name an exact grant in the submitted settings. This accepts
/// fresh consent; it never copies source approvals.
fn valid_directory_approvals(
    model: &Option<ConversationModelConfiguration>,
    directory_approvals: &[DirectoryApproval],
) -> bool {
    directory_approvals.len() <= crate::execution::MAXIMUM_DIRECTORY_GRANTS
        && directory_approvals
            .iter()
            .enumerate()
            .all(|(index, approval)| {
                !directory_approvals[..index].contains(approval)
                    && model.as_ref().is_some_and(|model| {
                        model
                            .settings
                            .directories
                            .iter()
                            .any(|grant| approval.matches(&model.settings, grant))
                    })
            })
}

fn unused_identifier(database: &Database) -> Result<ConversationId, ConversationError> {
    for _ in 0..16 {
        let id = ConversationId::generate().map_err(|_| ConversationError::Random)?;
        if !database.contains(&id)? {
            return Ok(id);
        }
    }
    Err(ConversationError::Random)
}

/// Bind every active-path entry to its predecessor. Appends extend the active
/// path, so vector order is the authoritative parent chain.
fn project_active_path(record: &mut ConversationRecord) {
    let mut parent = None;
    for message in &mut record.messages {
        message.parent = parent;
        parent = Some(message.id);
    }
}

// Only read-only transcript shells skip cross-message validation.
fn record_from_parts(
    file: MetadataFile,
    message_files: Vec<MessageFile>,
    summary_requests: Vec<super::history::RequestUsage>,
    validate_messages: bool,
) -> Result<ConversationRecord, ConversationError> {
    if file.version != CATALOGUE_VERSION {
        return Err(ConversationError::Corrupt);
    }
    let id = ConversationId::parse(&file.id).ok_or(ConversationError::Corrupt)?;
    if file.revision == 0 || file.updated_at_ms < file.created_at_ms {
        return Err(ConversationError::Corrupt);
    }
    let title = normalise_title(&file.title).map_err(|_| ConversationError::Corrupt)?;
    if title != file.title {
        return Err(ConversationError::Corrupt);
    }
    let network = parse_stored_network(&file.network, &file.network_domains)?;
    let mut model = file.model.map(model_from_file).transpose()?;
    if let Some(model) = &mut model {
        model.settings.network = network.clone();
    }
    let source_candidate_review = file
        .source_candidate_review
        .map(candidate_review_link_from_file)
        .transpose()?;
    let candidate_reviews = file
        .candidate_reviews
        .into_iter()
        .map(candidate_review_link_from_file)
        .collect::<Result<Vec<_>, _>>()?;
    if candidate_reviews.len() > MAXIMUM_LINKED_REVIEWS
        || candidate_reviews.iter().enumerate().any(|(index, link)| {
            candidate_reviews[..index]
                .iter()
                .any(|previous| previous == link)
        })
    {
        return Err(ConversationError::Corrupt);
    }
    let candidate_review_context = file
        .candidate_review_context
        .map(candidate_review_context_from_file)
        .transpose()?;
    if candidate_review_context
        .as_ref()
        .is_some_and(|context| source_candidate_review.as_ref() != Some(&context.source))
    {
        return Err(ConversationError::Corrupt);
    }
    let directory_approvals = directory_approvals_from_file(file.directory_approvals)?;
    let messages: Result<Vec<_>, _> = message_files.into_iter().map(message_from_file).collect();
    let messages = messages?;
    let mut seen = std::collections::BTreeSet::new();
    if messages.iter().any(|message| !seen.insert(message.id)) {
        return Err(ConversationError::Corrupt);
    }
    if !messages.is_empty() {
        // The active path is a chain. A missing link, a foreign parent or a
        // cycle leaves a path whose first entry still names a parent.
        let stored_leaf = match file.active_leaf.as_deref() {
            Some(value) => Some(MessageId::parse(value).ok_or(ConversationError::Corrupt)?),
            None => None,
        };
        let chain = messages.iter().enumerate().all(|(index, message)| {
            message.parent == index.checked_sub(1).map(|previous| messages[previous].id)
        });
        if !chain || stored_leaf != messages.last().map(|message| message.id) {
            return Err(ConversationError::Corrupt);
        }
    }
    let active_job = file.active_job.as_deref().and_then(JobId::parse);
    if file.active_job.is_some() && active_job.is_none() {
        return Err(ConversationError::Corrupt);
    }
    if validate_messages {
        let pending: Vec<_> = messages
            .iter()
            .filter(|message| message.status == MessageStatus::Pending)
            .collect();
        if match active_job {
            Some(request) => {
                pending.len() != 1
                    || pending[0].request != Some(request)
                    || messages.last() != pending.first().copied()
            }
            None => !pending.is_empty(),
        } {
            return Err(ConversationError::Corrupt);
        }
    }
    let queue = queue_from_file(file.queue_revision, file.queue)?;
    let forked_from = file
        .forked_from
        .map(fork_provenance_from_file)
        .transpose()?;
    let continuation = file
        .continuation
        .map(|file| continuation_from_file(file, &messages, validate_messages))
        .transpose()?;
    let compaction = file
        .compaction
        .map(|file| compaction_from_file(file, &messages, validate_messages))
        .transpose()?;
    Ok(ConversationRecord {
        id,
        revision: file.revision,
        title,
        title_pending: file.title_pending,
        network,
        model,
        directory_approvals,

        source_candidate_review,
        candidate_reviews,
        candidate_review_context,
        forked_from,
        messages,
        active_job,
        continuation,
        summary_requests: {
            let mut seen = std::collections::BTreeSet::new();
            if !summary_requests
                .iter()
                .all(|request| request.valid() && seen.insert(request.id))
            {
                return Err(ConversationError::Corrupt);
            }
            summary_requests
        },
        compaction,
        queue,
        created_at_ms: file.created_at_ms,
        updated_at_ms: file.updated_at_ms,
    })
}

fn directory_approvals_from_file(
    files: Vec<DirectoryApprovalFile>,
) -> Result<Vec<DirectoryApproval>, ConversationError> {
    if files.len() > crate::execution::MAXIMUM_DIRECTORY_GRANTS {
        return Err(ConversationError::Corrupt);
    }
    let mut approvals = Vec::with_capacity(files.len());
    for file in files {
        let approval = DirectoryApproval {
            settings_digest: crate::hex::decode(&file.settings_digest)
                .ok_or(ConversationError::Corrupt)?,
            root: file.host_path,
            device: file.device,
            inode: file.inode,
            access: file.access,
        };
        if approvals.contains(&approval) {
            return Err(ConversationError::Corrupt);
        }
        approvals.push(approval);
    }
    Ok(approvals)
}

fn model_from_file(
    file: ConversationModelFile,
) -> Result<ConversationModelConfiguration, ConversationError> {
    let selection = ModelSelection::new(
        file.selection.provider,
        file.selection.model.clone(),
        file.selection.thinking.clone(),
    )
    .filter(|selection| selection == &file.selection)
    .ok_or(ConversationError::Corrupt)?;
    let tools = file
        .tools
        .iter()
        .map(|name| ToolId::parse(name).ok_or(ConversationError::Corrupt))
        .collect::<Result<Vec<_>, _>>()?;
    let directories = file
        .directories
        .into_iter()
        .map(|grant| {
            Some(crate::execution::DirectoryGrant {
                id: crate::execution::DirectoryGrantId::parse(&grant.id)?,
                host_path: grant.host_path,
                identity: crate::execution::CanonicalDirectoryIdentity {
                    device: grant.device,
                    inode: grant.inode,
                },
                alias: grant.alias,
                access: grant.access,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(ConversationError::Corrupt)?;
    let environment = crate::environments::EnvironmentId::parse(&file.environment)
        .ok_or(ConversationError::Corrupt)?;
    let location =
        crate::execution::ToolLocation::parse(&file.location).ok_or(ConversationError::Corrupt)?;
    let host_approval = crate::execution::HostApprovalPolicy::parse(&file.host_approval)
        .ok_or(ConversationError::Corrupt)?;
    let settings =
        crate::execution::ExecutionSettings::new(selection, file.instructions, tools, environment)
            .and_then(|settings| settings.with_directories(directories))
            .map(|settings| {
                settings
                    .with_location(location)
                    .with_host_approval(host_approval)
            })
            .ok_or(ConversationError::Corrupt)?;
    let preset = match file.preset {
        Some(preset)
            if preset.revision > 0
                && !preset.name.trim().is_empty()
                && preset.name.len() <= crate::agents::MAXIMUM_NAME_BYTES
                && !preset.name.chars().any(char::is_control) =>
        {
            Some(AppliedPreset {
                id: crate::presets::PresetId::parse(&preset.id)
                    .ok_or(ConversationError::Corrupt)?,
                revision: preset.revision,
                name: preset.name,
            })
        }
        None => None,
        Some(_) => return Err(ConversationError::Corrupt),
    };
    Ok(ConversationModelConfiguration { settings, preset })
}

fn model_to_file(model: &ConversationModelConfiguration) -> ConversationModelFile {
    ConversationModelFile {
        selection: model.settings.model.clone(),
        instructions: model.settings.instructions.clone(),
        tools: model
            .settings
            .tools
            .iter()
            .map(|tool| tool.as_str().to_owned())
            .collect(),
        environment: model.settings.environment.as_hex(),
        directories: model
            .settings
            .directories
            .iter()
            .map(|grant| DirectoryGrantFile {
                id: grant.id.as_hex(),
                host_path: grant.host_path.clone(),
                device: grant.identity.device,
                inode: grant.identity.inode,
                alias: grant.alias.clone(),
                access: grant.access,
            })
            .collect(),
        location: model.settings.location.as_str().to_owned(),
        host_approval: model.settings.host_approval.as_str().to_owned(),
        preset: model.preset.as_ref().map(|preset| AppliedPresetFile {
            id: preset.id.as_hex(),
            revision: preset.revision,
            name: preset.name.clone(),
        }),
    }
}

fn artefact_ref_from_file(file: ArtefactRefFile) -> Result<ArtefactReference, ConversationError> {
    Ok(ArtefactReference {
        id: ArtefactId::parse(&file.id).ok_or(ConversationError::Corrupt)?,
        kind: crate::workflows::definition::ArtefactKind::parse(&file.kind)
            .ok_or(ConversationError::Corrupt)?,
        artefact_hash: ArtefactHash::parse(&file.artefact_hash)
            .ok_or(ConversationError::Corrupt)?,
    })
}

fn artefact_ref_to_file(reference: &ArtefactReference) -> ArtefactRefFile {
    ArtefactRefFile {
        id: reference.id.as_hex(),
        kind: reference.kind.as_str().to_owned(),
        artefact_hash: reference.artefact_hash.as_str(),
    }
}

fn candidate_review_link_from_file(
    file: CandidateReviewLinkFile,
) -> Result<CandidateReviewLink, ConversationError> {
    let candidate = artefact_ref_from_file(file.candidate)?;
    let diff_base = artefact_ref_from_file(file.diff_base)?;
    if candidate.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
        || diff_base.kind != crate::workflows::definition::ArtefactKind::CandidateRevision
    {
        return Err(ConversationError::Corrupt);
    }
    Ok(CandidateReviewLink {
        conversation_id: match file.conversation {
            Some(value) => Some(ConversationId::parse(&value).ok_or(ConversationError::Corrupt)?),
            None => None,
        },
        run_id: RunId::parse(&file.run).ok_or(ConversationError::Corrupt)?,
        candidate,
        diff_base,
    })
}

fn candidate_review_link_to_file(link: &CandidateReviewLink) -> CandidateReviewLinkFile {
    CandidateReviewLinkFile {
        conversation: link.conversation_id.map(|id| id.as_hex()),
        run: link.run_id.as_hex(),
        candidate: artefact_ref_to_file(&link.candidate),
        diff_base: artefact_ref_to_file(&link.diff_base),
    }
}

fn candidate_review_context_from_file(
    file: CandidateReviewContextFile,
) -> Result<CandidateReviewContext, ConversationError> {
    let task_brief = normalise_message(&file.task_brief)?;
    if task_brief.len() > MAXIMUM_REVIEW_BRIEF_BYTES {
        return Err(ConversationError::Corrupt);
    }
    Ok(CandidateReviewContext {
        source: candidate_review_link_from_file(file.source)?,
        task_brief,
    })
}

fn candidate_review_context_to_file(
    context: &CandidateReviewContext,
) -> CandidateReviewContextFile {
    CandidateReviewContextFile {
        source: candidate_review_link_to_file(&context.source),
        task_brief: context.task_brief.clone(),
    }
}

fn fork_provenance_from_file(
    file: ForkProvenanceFile,
) -> Result<super::forks::ForkProvenance, ConversationError> {
    let source = ConversationId::parse(&file.source).ok_or(ConversationError::Corrupt)?;
    let boundary = MessageId::parse(&file.boundary).ok_or(ConversationError::Corrupt)?;
    if file.source_revision == 0 {
        return Err(ConversationError::Corrupt);
    }
    Ok(super::forks::ForkProvenance {
        source,
        source_revision: file.source_revision,
        boundary,
        candidate_review: file.candidate_review,
    })
}

fn message_from_file(file: MessageFile) -> Result<ConversationMessage, ConversationError> {
    let id = MessageId::parse(&file.id).ok_or(ConversationError::Corrupt)?;
    let parent = match file.parent {
        Some(value) => Some(MessageId::parse(&value).ok_or(ConversationError::Corrupt)?),
        None => None,
    };
    if parent == Some(id) {
        return Err(ConversationError::Corrupt);
    }
    let limit = match file.role {
        MessageRole::User => MAXIMUM_MESSAGE_BYTES,
        MessageRole::Assistant => MAXIMUM_REPLY_BYTES,
    };
    if file.text.len() > limit
        || file.text.contains('\0')
        || !valid_message_error(file.status, file.error.as_deref())
        || !valid_activity(&file.activity, &file.text)
        || !valid_continuation(&file.continuation)
        || !super::history::valid_requests(&file.requests, file.role)
        || (file.role == MessageRole::User
            && (!file.activity.is_empty() || !file.continuation.is_empty()))
    {
        return Err(ConversationError::Corrupt);
    }
    let request = file.request.as_deref().and_then(JobId::parse);
    if file.request.is_some() != request.is_some()
        || matches!(file.role, MessageRole::User) != request.is_none()
        || (file.role == MessageRole::User
            && (file.status != MessageStatus::Complete
                || file.completion.is_some()
                || normalise_message(&file.text).as_ref() != Ok(&file.text)))
    {
        return Err(ConversationError::Corrupt);
    }
    Ok(ConversationMessage {
        id,
        parent,
        role: file.role,
        text: file.text,
        activity: file.activity,
        continuation: file.continuation,
        status: file.status,
        error: file.error,
        request,
        completion: file.completion,
        requests: file.requests,
    })
}

fn validate_reply(reply: &crate::providers::AssistantReply) -> Result<(), ConversationError> {
    if reply.text.len() > MAXIMUM_REPLY_BYTES
        || reply.text.contains('\0')
        || !valid_activity(&reply.activity, &reply.text)
        || !valid_continuation(&reply.continuation)
        || !super::history::valid_requests(&reply.usage, MessageRole::Assistant)
    {
        return Err(ConversationError::Message);
    }
    Ok(())
}

fn apply_reply(message: &mut ConversationMessage, reply: &crate::providers::AssistantReply) {
    message.text = reply.text.clone();
    message.activity = reply.activity.clone();
    message.continuation = reply.continuation.clone();
    message.completion = reply.completion;
    message.requests = reply.usage.clone();
}

fn apply_owned_reply(message: &mut ConversationMessage, reply: crate::providers::AssistantReply) {
    message.text = reply.text;
    message.activity = reply.activity;
    message.continuation = reply.continuation;
    message.completion = reply.completion;
    message.requests = reply.usage;
}

fn message_to_file(message: &ConversationMessage) -> MessageFile {
    MessageFile {
        id: message.id.as_hex(),
        parent: message.parent.map(|parent| parent.as_hex()),
        role: message.role,
        text: message.text.clone(),
        activity: message.activity.clone(),
        continuation: message.continuation.clone(),
        status: message.status,
        error: message.error.clone(),
        request: message.request.map(|request| request.as_hex()),
        completion: message.completion,
        requests: message.requests.clone(),
    }
}

fn metadata_to_file(record: &ConversationRecord) -> MetadataFile {
    MetadataFile {
        version: CATALOGUE_VERSION,
        id: record.id.as_hex(),
        revision: record.revision,
        title: record.title.clone(),
        title_pending: record.title_pending,
        network: record.network.as_str().to_owned(),
        network_domains: record.network.domains().to_vec(),
        model: record.model.as_ref().map(model_to_file),
        directory_approvals: record
            .directory_approvals
            .iter()
            .map(|approval| DirectoryApprovalFile {
                settings_digest: crate::hex::encode(&approval.settings_digest),
                host_path: approval.root.clone(),
                device: approval.device,
                inode: approval.inode,
                access: approval.access,
            })
            .collect(),

        source_candidate_review: record
            .source_candidate_review
            .as_ref()
            .map(candidate_review_link_to_file),
        candidate_reviews: record
            .candidate_reviews
            .iter()
            .map(candidate_review_link_to_file)
            .collect(),
        candidate_review_context: record
            .candidate_review_context
            .as_ref()
            .map(candidate_review_context_to_file),
        forked_from: record
            .forked_from
            .as_ref()
            .map(|provenance| ForkProvenanceFile {
                source: provenance.source.as_hex(),
                source_revision: provenance.source_revision,
                boundary: provenance.boundary.as_hex(),
                candidate_review: provenance.candidate_review,
            }),
        active_job: record.active_job.map(|request| request.as_hex()),
        continuation: record.continuation.as_ref().map(continuation_to_file),
        compaction: record.compaction.as_ref().map(compaction_to_file),
        active_leaf: record.messages.last().map(|message| message.id.as_hex()),
        queue_revision: record.queue.revision,
        queue: record.queue.items.iter().map(queue_item_to_file).collect(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

fn continuation_to_file(checkpoint: &super::history::ContinuationCheckpoint) -> ContinuationFile {
    ContinuationFile {
        id: checkpoint.id.as_hex(),
        boundary: checkpoint.boundary.as_hex(),
        pinned: model_to_file(&ConversationModelConfiguration {
            settings: checkpoint.pinned.clone(),
            preset: None,
        }),
        budget: BudgetFile {
            model_requests: checkpoint.budget.model_requests,
            model_request_limit: checkpoint.budget.model_request_limit,
            tool_dispatches: checkpoint.budget.tool_dispatches,
            tool_dispatch_limit: checkpoint.budget.tool_dispatch_limit,
            elapsed_ms: checkpoint.budget.elapsed_ms,
            elapsed_limit_ms: checkpoint.budget.elapsed_limit_ms,
            reason: checkpoint.budget.reason.as_str().to_owned(),
        },
        run: checkpoint.run.map(|id| id.as_hex()),
        attempt: checkpoint.attempt.map(|id| id.as_hex()),
        step: checkpoint.step.clone(),
        drafts: checkpoint
            .drafts
            .iter()
            .map(|draft| PausedDraftFile {
                key: draft.key.clone(),
                kind: draft.kind.clone(),
                markdown: draft.markdown.clone(),
                verdict: draft.verdict.clone(),
                outcome: draft.outcome.clone(),
            })
            .collect(),
        created_at_ms: checkpoint.created_at_ms,
    }
}

fn continuation_from_file(
    file: ContinuationFile,
    messages: &[ConversationMessage],
    validate_messages: bool,
) -> Result<super::history::ContinuationCheckpoint, ConversationError> {
    let pinned = model_from_file(file.pinned)?;
    let checkpoint = super::history::ContinuationCheckpoint {
        id: super::id::CheckpointId::parse(&file.id).ok_or(ConversationError::Corrupt)?,
        boundary: MessageId::parse(&file.boundary).ok_or(ConversationError::Corrupt)?,
        pinned: pinned.settings,
        budget: crate::execution::BudgetSnapshot {
            model_requests: file.budget.model_requests,
            model_request_limit: file.budget.model_request_limit,
            tool_dispatches: file.budget.tool_dispatches,
            tool_dispatch_limit: file.budget.tool_dispatch_limit,
            elapsed_ms: file.budget.elapsed_ms,
            elapsed_limit_ms: file.budget.elapsed_limit_ms,
            reason: crate::execution::BudgetReason::parse(&file.budget.reason)
                .ok_or(ConversationError::Corrupt)?,
        },
        run: match file.run {
            Some(value) => Some(RunId::parse(&value).ok_or(ConversationError::Corrupt)?),
            None => None,
        },
        attempt: match file.attempt {
            Some(value) => {
                Some(crate::workflows::AttemptId::parse(&value).ok_or(ConversationError::Corrupt)?)
            }
            None => None,
        },
        step: file.step,
        drafts: file
            .drafts
            .into_iter()
            .map(|draft| super::history::PausedOutputDraft {
                key: draft.key,
                kind: draft.kind,
                markdown: draft.markdown,
                verdict: draft.verdict,
                outcome: draft.outcome,
            })
            .collect(),
        created_at_ms: file.created_at_ms,
    };
    if validate_messages && !checkpoint.valid(messages) {
        return Err(ConversationError::Corrupt);
    }
    Ok(checkpoint)
}

fn commit_compaction(
    current: &mut ConversationRecord,
    compaction: super::compaction::CompactionRecord,
) -> Result<(), ConversationError> {
    if current.continuation.is_some() {
        return Err(ConversationError::Active);
    }
    if !compaction.valid(&current.messages) {
        return Err(ConversationError::Message);
    }
    current.compaction = Some(compaction);
    Ok(())
}

fn compaction_to_file(record: &super::compaction::CompactionRecord) -> CompactionFile {
    CompactionFile {
        covered_through: record.covered_through.as_hex(),
        retained_from: record.retained_from.as_hex(),
        text: record.text.clone(),
        request: record.request.clone(),
        created_at_ms: record.created_at_ms,
    }
}

fn compaction_from_file(
    file: CompactionFile,
    messages: &[ConversationMessage],
    validate_messages: bool,
) -> Result<super::compaction::CompactionRecord, ConversationError> {
    let record = super::compaction::CompactionRecord {
        covered_through: MessageId::parse(&file.covered_through)
            .ok_or(ConversationError::Corrupt)?,
        retained_from: MessageId::parse(&file.retained_from).ok_or(ConversationError::Corrupt)?,
        text: file.text,
        request: file.request,
        created_at_ms: file.created_at_ms,
    };
    if validate_messages && !record.valid(messages) {
        return Err(ConversationError::Corrupt);
    }
    Ok(record)
}

fn queue_from_file(
    revision: u32,
    items: Vec<QueueItemFile>,
) -> Result<super::queue::ConversationQueue, ConversationError> {
    let revision = match (revision, items.is_empty()) {
        (0, true) => 1,
        (0, false) => return Err(ConversationError::Corrupt),
        (revision, _) => revision,
    };
    let mut queue = super::queue::ConversationQueue {
        revision,
        items: Vec::with_capacity(items.len()),
    };
    for item in items {
        let id = super::queue::QueueItemId::parse(&item.id).ok_or(ConversationError::Corrupt)?;
        let delivery =
            super::queue::QueueDelivery::parse(&item.delivery).ok_or(ConversationError::Corrupt)?;
        let settings_digest: [u8; 32] =
            crate::hex::decode(&item.settings_digest).ok_or(ConversationError::Corrupt)?;
        let job = match item.job.as_deref() {
            None => None,
            Some(value) => Some(JobId::parse(value).ok_or(ConversationError::Corrupt)?),
        };
        queue.items.push(super::queue::QueueItem {
            id,
            text: item.text,
            delivery,
            settings_digest,
            job,
        });
    }
    if !super::queue::valid_queue(&queue) {
        return Err(ConversationError::Corrupt);
    }
    Ok(queue)
}

fn queue_item_to_file(item: &super::queue::QueueItem) -> QueueItemFile {
    QueueItemFile {
        id: item.id.as_hex(),
        text: item.text.clone(),
        delivery: item.delivery.as_str().to_owned(),
        settings_digest: crate::hex::encode(&item.settings_digest),
        job: item.job.map(|job| job.as_hex()),
    }
}

fn parse_stored_network(
    mode: &str,
    domains: &[String],
) -> Result<crate::agents::NetworkAccess, ConversationError> {
    match mode {
        "none" if domains.is_empty() => Ok(crate::agents::NetworkAccess::None),
        "restricted" => crate::agents::NetworkAccess::parse_form(mode, &domains.join("\n"))
            .map_err(|_| ConversationError::Corrupt),
        "public" if domains.is_empty() => Ok(crate::agents::NetworkAccess::Public),
        _ => Err(ConversationError::Corrupt),
    }
}

pub(crate) fn normalise_title(raw: &str) -> Result<String, ConversationError> {
    let title = raw.trim();
    if title.is_empty() || title.len() > MAXIMUM_TITLE_BYTES || title.chars().any(char::is_control)
    {
        return Err(ConversationError::Title);
    }
    Ok(title.to_owned())
}

pub(crate) fn normalise_message(raw: &str) -> Result<String, ConversationError> {
    let text = raw.trim();
    if text.is_empty()
        || text.len() > MAXIMUM_MESSAGE_BYTES
        || text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ConversationError::Message);
    }
    Ok(text.to_owned())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
