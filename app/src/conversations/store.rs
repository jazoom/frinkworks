use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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

const CATALOGUE_VERSION: u32 = 1;
const FILE_SUFFIX: &str = ".json";
const MAXIMUM_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_STORE_BYTES: usize = 64 * 1024 * 1024;
const OUTPUT_CHECKPOINT_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_CONVERSATIONS: usize = 128;
pub(crate) const MAXIMUM_TITLE_BYTES: usize = 120;
pub(crate) const MAXIMUM_MESSAGES: usize = 512;
pub(crate) const MAXIMUM_MESSAGE_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_REPLY_BYTES: usize = 128 * 1024;
const MAXIMUM_LINKED_REVIEWS: usize = 32;
const MAXIMUM_REVIEW_BRIEF_BYTES: usize = MAXIMUM_MESSAGE_BYTES;

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
    pub(crate) messages: Vec<ConversationMessage>,
    pub(crate) active_job: Option<JobId>,
    pub(crate) continuation: Option<super::history::ContinuationCheckpoint>,
    pub(crate) queue: super::queue::ConversationQueue,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
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
    Unsettled,
    Corrupt,
    Full,
    Missing,
    Conflict,
    Revision,
    Title,
    Message,
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
            Self::Persist => "Power Plant could not store the conversation. Try again.",
            Self::Unsettled => {
                "Power Plant could not confirm that local history was stored. Restart before you continue."
            }
            Self::Corrupt => "Stored conversation history is unreadable.",
            Self::Full => {
                "Delete an inactive conversation to free local history space. If this discussion reached its message limit, start another conversation."
            }
            Self::Missing => "That conversation is not in the catalogue.",
            Self::Conflict => "That conversation changed in another tab. Reload it.",
            Self::Revision => "Power Plant cannot update this conversation again.",
            Self::Title => "Enter a title of 1 to 120 bytes without control characters.",
            Self::Message => "Enter a message within the conversation limit.",
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
    // The directory holds one private record file per conversation.
    path: Option<PathBuf>,
    inner: Mutex<BTreeMap<ConversationId, ConversationRecord>>,
    // Unsynchronised transcript bytes since the last durable checkpoint.
    pending: Mutex<BTreeMap<ConversationId, usize>>,
    uncertain: Mutex<std::collections::BTreeSet<ConversationId>>,
    title_updates: tokio::sync::broadcast::Sender<()>,
    questions: QuestionWaiters,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ConversationFile {
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
    messages: Vec<MessageFile>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    active_job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<ContinuationFile>,
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
struct ArtefactRefFile {
    id: String,
    kind: String,
    artefact_hash: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct MessageFile {
    id: String,
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
}

impl ConversationStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, ConversationError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| ConversationError::Persist)?;
        let mut conversations = load_dir(&dir)?;
        if interrupt_recovered_requests(&mut conversations) {
            persist_map(Some(&dir), &conversations)?;
        }
        Ok(Self {
            path: Some(dir),
            inner: Mutex::new(conversations),
            pending: Mutex::new(BTreeMap::new()),
            uncertain: Mutex::new(std::collections::BTreeSet::new()),
            title_updates: tokio::sync::broadcast::channel(16).0,
            questions: QuestionWaiters::new(),
        })
    }

    pub(crate) fn list(&self) -> Vec<ConversationRecord> {
        self.lock().values().cloned().collect()
    }

    pub(crate) fn get(&self, id: &ConversationId) -> Option<ConversationRecord> {
        self.lock().get(id).cloned()
    }

    pub(crate) fn create_saved(
        &self,
        id: ConversationId,
        title: Option<String>,
        model: Option<ConversationModelConfiguration>,
        directory_approvals: Vec<DirectoryApproval>,
    ) -> Result<ConversationRecord, ConversationError> {
        if directory_approvals.len() > crate::execution::MAXIMUM_DIRECTORY_GRANTS
            || directory_approvals
                .iter()
                .enumerate()
                .any(|(index, approval)| {
                    directory_approvals[..index].contains(approval)
                        || !model.as_ref().is_some_and(|model| {
                            model
                                .settings
                                .directories
                                .iter()
                                .any(|grant| approval.matches(&model.settings, grant))
                        })
                })
        {
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
        let mut conversations = self.lock();
        check_capacity(&conversations)?;
        if conversations.contains_key(&id) {
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
            messages: Vec::new(),
            active_job: None,
            continuation: None,
            queue: super::queue::ConversationQueue::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        conversations.insert(id, record.clone());
        if let Err(error) = self.persist_one(&record) {
            if error != ConversationError::Unsettled {
                conversations.remove(&id);
            }
            return Err(error);
        }
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
        let mut conversations = self.lock();
        let source = source_conversation
            .map(|(id, revision)| {
                self.require_durable(&id)?;
                let source = conversations
                    .get(&id)
                    .cloned()
                    .ok_or(ConversationError::Missing)?;
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
        check_capacity(&conversations)?;
        let id = unused_identifier(&conversations)?;
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
            messages: Vec::new(),
            active_job: None,
            continuation: None,
            queue: super::queue::ConversationQueue::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut updated_source = None;
        if let Some(source) = source {
            let mut updated = source.clone();
            updated.revision = source
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            updated.updated_at_ms = now.max(source.updated_at_ms);
            updated.candidate_reviews.push(review_link);
            updated_source = Some(updated);
        }
        let review_result = self.persist_one(&review);
        if review_result.is_ok() || review_result == Err(ConversationError::Unsettled) {
            conversations.insert(id, review.clone());
        }
        review_result?;
        if let Some(source) = updated_source {
            let result = self.persist_one(&source);
            if result.is_ok() || result == Err(ConversationError::Unsettled) {
                conversations.insert(source.id, source);
            }
            result?;
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
        let mut records = self.lock();
        self.require_durable(id)?;
        let current = records.get(id).cloned().ok_or(ConversationError::Missing)?;
        if current.revision != revision {
            return Err(ConversationError::Conflict);
        }
        let mut updated = current.clone();
        updated.title = title;
        records.insert(*id, updated.clone());
        if let Err(error) = self.persist_one(&updated) {
            if error != ConversationError::Unsettled {
                records.insert(*id, current);
            }
            return Err(error);
        }
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
        self.lock().get(id).is_some_and(|record| {
            record
                .directory_approvals
                .iter()
                .any(|approval| approval.matches(settings, grant))
        })
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
            if current.messages.len() > MAXIMUM_MESSAGES.saturating_sub(2) {
                return Err(ConversationError::Full);
            }
            if let Some(model) = model {
                current.model = Some(model);
            }
            current.messages.push(ConversationMessage {
                id: user_id,
                role: MessageRole::User,
                text,
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
            });
            current.messages.push(ConversationMessage {
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
            });
            current.active_job = Some(request);
            Ok(())
        })
    }

    // Transcript progress stays in memory. A checkpoint writes the record at an
    // explicit boundary; a replaced-but-unsynced write blocks advancement.
    pub(crate) fn append_output(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: impl Into<crate::providers::AssistantReply>,
    ) -> Result<(), ConversationError> {
        let reply = reply.into();
        validate_reply(&reply)?;
        self.update_output(id, request, reply, false)
    }

    pub(crate) fn checkpoint_output(
        &self,
        id: &ConversationId,
        request: JobId,
        reply: impl Into<crate::providers::AssistantReply>,
    ) -> Result<(), ConversationError> {
        let reply = reply.into();
        validate_reply(&reply)?;
        self.update_output(id, request, reply, true)
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
            if record.messages.len() >= MAXIMUM_MESSAGES {
                return Err(ConversationError::Full);
            }
            let active = active_assistant(record, request)?;
            active.text = committed.text.clone();
            active.activity = committed.activity.clone();
            active.continuation = committed.continuation.clone();
            active.completion = committed.completion;
            let failed = ConversationMessage {
                id: MessageId::generate().map_err(|_| ConversationError::Random)?,
                role: MessageRole::Assistant,
                text: failed.text,
                activity: failed.activity,
                continuation: Vec::new(),
                status: MessageStatus::Failed,
                error: Some(error),
                request: Some(JobId::generate().map_err(|_| ConversationError::Random)?),
                completion: Some(crate::providers::CompletionReason::Unknown),
            };
            // Failed attempts stay local and never split a completed tool exchange.
            record.messages.insert(record.messages.len() - 1, failed);
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
        checkpoint: bool,
    ) -> Result<(), ConversationError> {
        let bytes = reply_size(&reply);
        let mut conversations = self.lock();
        self.require_durable(id)?;
        let Some(current) = conversations.get(id).cloned() else {
            return Err(ConversationError::Missing);
        };
        let mut updated = current.clone();
        {
            let message = active_assistant(&mut updated, request)?;
            message.text = reply.text;
            message.activity = reply.activity;
            message.continuation = reply.continuation;
            message.completion = reply.completion;
        }
        super::history::validate_exchange(&updated.messages)
            .map_err(|_| ConversationError::Message)?;
        updated.updated_at_ms = now_ms().max(current.updated_at_ms);
        conversations.insert(*id, updated.clone());
        let due = {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            let entry = pending.entry(*id).or_insert(0);
            let previous = crate::providers::AssistantReply {
                text: current
                    .messages
                    .last()
                    .map_or_else(String::new, |message| message.text.clone()),
                activity: current
                    .messages
                    .last()
                    .map_or_else(Vec::new, |message| message.activity.clone()),
                continuation: current
                    .messages
                    .last()
                    .map_or_else(Vec::new, |message| message.continuation.clone()),
                ..Default::default()
            };
            *entry = entry.saturating_add(bytes.abs_diff(reply_size(&previous)));
            let due = checkpoint || *entry >= OUTPUT_CHECKPOINT_BYTES;
            if due {
                pending.remove(id);
            }
            due
        };
        if !due {
            return Ok(());
        }
        match self.persist_one(&updated) {
            Ok(()) => Ok(()),
            Err(ConversationError::Unsettled) => Err(ConversationError::Unsettled),
            Err(error) => {
                conversations.insert(*id, current);
                Err(error)
            }
        }
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
            message.text = reply.text;
            message.activity = reply.activity;
            message.continuation = reply.continuation;
            message.status = status;
            message.error = error;
            message.completion = reply.completion;
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
                message.text = reply.text;
                message.activity = reply.activity;
                message.continuation = reply.continuation;
                message.status = MessageStatus::Complete;
                message.error = None;
                message.completion = reply.completion;
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
            if current.messages.len() >= MAXIMUM_MESSAGES {
                return Err(ConversationError::Full);
            }
            current.messages.push(ConversationMessage {
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
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
            if current.messages.len() > MAXIMUM_MESSAGES.saturating_sub(2) {
                return Err(ConversationError::Full);
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
                id: user_id,
                role: MessageRole::User,
                text: item.text,
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
            });
            current.messages.push(ConversationMessage {
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
            });
            current.active_job = Some(request);
            Ok(())
        })
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
            if current.messages.len() > MAXIMUM_MESSAGES.saturating_sub(2) {
                return Err(ConversationError::Full);
            }
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
                message.text = reply.text.clone();
                message.activity = reply.activity.clone();
                message.continuation = reply.continuation.clone();
                message.completion = reply.completion;
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
                id: user_id,
                role: MessageRole::User,
                text: item.text.clone(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
                completion: None,
            });
            current.messages.push(ConversationMessage {
                id: assistant_id,
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                continuation: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
                completion: None,
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
        let mut conversations = self.lock();
        self.require_durable(id)?;
        self.questions.invalidate_conversation(*id);
        let Some(current) = conversations.get(id).cloned() else {
            return Err(ConversationError::Missing);
        };
        if current.revision != expected_revision {
            return Err(ConversationError::Conflict);
        }
        if current.active_job.is_some() {
            return Err(ConversationError::Active);
        }
        conversations.remove(id);
        if let Some(path) = self.path.as_deref() {
            let file = record_file(path, *id);
            if let Err(_error) = crate::storage::remove_private(&file) {
                conversations.insert(*id, current);
                return Err(ConversationError::Persist);
            }
        }
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        Ok(())
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
        let mut conversations = self.lock();
        let Some(current) = conversations.get(id).cloned() else {
            return Err(ConversationError::Missing);
        };
        if expected_revision != 0 && current.revision != expected_revision {
            return Err(ConversationError::Conflict);
        }
        self.require_durable(id)?;
        let mut updated = current.clone();
        edit(&mut updated)?;
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
        conversations.insert(*id, updated.clone());
        match self.persist_one(&updated) {
            Ok(()) => Ok(updated),
            Err(ConversationError::Unsettled) => Err(ConversationError::Unsettled),
            Err(error) => {
                conversations.insert(*id, current);
                Err(error)
            }
        }
    }

    fn persist_one(&self, record: &ConversationRecord) -> Result<(), ConversationError> {
        let result = persist_record(self.path.as_deref(), record);
        if result == Err(ConversationError::Unsettled) {
            self.uncertain
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(record.id);
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

    fn lock(&self) -> MutexGuard<'_, BTreeMap<ConversationId, ConversationRecord>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for ConversationStore {
    // A clean shutdown flushes unsynchronised transcript progress. A crash keeps
    // the last explicit checkpoint only.
    fn drop(&mut self) {
        let Some(dir) = self.path.as_deref() else {
            return;
        };
        let pending: Vec<ConversationId> = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .copied()
            .collect();
        if pending.is_empty() {
            return;
        }
        let conversations = self.lock();
        for id in pending {
            if self.require_durable(&id).is_ok()
                && let Some(record) = conversations.get(&id)
            {
                let _ = persist_record(Some(dir), record);
            }
        }
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

fn interrupt_recovered_requests(
    conversations: &mut BTreeMap<ConversationId, ConversationRecord>,
) -> bool {
    let mut changed = false;
    for record in conversations.values_mut() {
        let Some(request) = record.active_job else {
            continue;
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
        changed = true;
    }
    changed
}

fn check_capacity(
    conversations: &BTreeMap<ConversationId, ConversationRecord>,
) -> Result<(), ConversationError> {
    if conversations.len() < MAXIMUM_CONVERSATIONS {
        Ok(())
    } else {
        Err(ConversationError::Full)
    }
}

fn unused_identifier(
    conversations: &BTreeMap<ConversationId, ConversationRecord>,
) -> Result<ConversationId, ConversationError> {
    for _ in 0..16 {
        let id = ConversationId::generate().map_err(|_| ConversationError::Random)?;
        if !conversations.contains_key(&id) {
            return Ok(id);
        }
    }
    Err(ConversationError::Random)
}

fn load_dir(dir: &Path) -> Result<BTreeMap<ConversationId, ConversationRecord>, ConversationError> {
    let mut conversations = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(conversations),
        Err(_) => return Err(ConversationError::Corrupt),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ConversationError::Corrupt)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(FILE_SUFFIX) else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        let Some(id) = ConversationId::parse(stem) else {
            return Err(ConversationError::Corrupt);
        };
        let metadata = entry.metadata().map_err(|_| ConversationError::Corrupt)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ConversationError::Corrupt);
        }
        files.push((id, entry.path()));
    }
    if files.len() > MAXIMUM_CONVERSATIONS {
        return Err(ConversationError::Corrupt);
    }
    let mut total = 0usize;
    for (id, path) in files {
        if conversations.contains_key(&id) {
            return Err(ConversationError::Corrupt);
        }
        let bytes = crate::storage::read_private_bounded(&path, MAXIMUM_RECORD_BYTES)
            .map_err(|_| ConversationError::Corrupt)?;
        total = total.saturating_add(bytes.len());
        if total > MAXIMUM_STORE_BYTES {
            return Err(ConversationError::Corrupt);
        }
        let file: ConversationFile =
            serde_json::from_slice(&bytes).map_err(|_| ConversationError::Corrupt)?;
        let record = record_from_file(file)?;
        if record.id != id {
            return Err(ConversationError::Corrupt);
        }
        conversations.insert(id, record);
    }
    Ok(conversations)
}

fn record_from_file(file: ConversationFile) -> Result<ConversationRecord, ConversationError> {
    if file.version != CATALOGUE_VERSION {
        return Err(ConversationError::Corrupt);
    }
    let id = ConversationId::parse(&file.id).ok_or(ConversationError::Corrupt)?;
    if file.revision == 0
        || file.updated_at_ms < file.created_at_ms
        || file.messages.len() > MAXIMUM_MESSAGES
    {
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
    let mut directory_approvals = Vec::with_capacity(file.directory_approvals.len());
    for approval in file.directory_approvals {
        let settings_digest: [u8; 32] =
            crate::hex::decode(&approval.settings_digest).ok_or(ConversationError::Corrupt)?;
        let candidate = DirectoryApproval {
            settings_digest,
            root: approval.host_path,
            device: approval.device,
            inode: approval.inode,
            access: approval.access,
        };
        if directory_approvals.len() >= crate::execution::MAXIMUM_DIRECTORY_GRANTS
            || directory_approvals.contains(&candidate)
        {
            return Err(ConversationError::Corrupt);
        }
        directory_approvals.push(candidate);
    }
    let messages: Result<Vec<_>, _> = file.messages.into_iter().map(message_from_file).collect();
    let messages = messages?;
    if messages
        .iter()
        .enumerate()
        .any(|(index, message)| messages[..index].iter().any(|other| other.id == message.id))
    {
        return Err(ConversationError::Corrupt);
    }
    let active_job = file.active_job.as_deref().and_then(JobId::parse);
    if file.active_job.is_some() && active_job.is_none() {
        return Err(ConversationError::Corrupt);
    }
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
    let queue = queue_from_file(file.queue_revision, file.queue)?;
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
        messages: messages.clone(),
        active_job,
        continuation: file
            .continuation
            .map(|file| continuation_from_file(file, &messages))
            .transpose()?,
        queue,
        created_at_ms: file.created_at_ms,
        updated_at_ms: file.updated_at_ms,
    })
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

fn message_from_file(file: MessageFile) -> Result<ConversationMessage, ConversationError> {
    let id = MessageId::parse(&file.id).ok_or(ConversationError::Corrupt)?;
    let limit = match file.role {
        MessageRole::User => MAXIMUM_MESSAGE_BYTES,
        MessageRole::Assistant => MAXIMUM_REPLY_BYTES,
    };
    if file.text.len() > limit
        || file.text.contains('\0')
        || !valid_message_error(file.status, file.error.as_deref())
        || !valid_activity(&file.activity, &file.text)
        || !valid_continuation(&file.continuation)
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
        role: file.role,
        text: file.text,
        activity: file.activity,
        continuation: file.continuation,
        status: file.status,
        error: file.error,
        request,
        completion: file.completion,
    })
}

fn validate_reply(reply: &crate::providers::AssistantReply) -> Result<(), ConversationError> {
    if reply.text.len() > MAXIMUM_REPLY_BYTES
        || reply.text.contains('\0')
        || !valid_activity(&reply.activity, &reply.text)
        || !valid_continuation(&reply.continuation)
    {
        return Err(ConversationError::Message);
    }
    Ok(())
}

fn reply_size(reply: &crate::providers::AssistantReply) -> usize {
    reply.text.len().saturating_add(
        reply
            .activity
            .iter()
            .map(|activity| match activity {
                crate::providers::AssistantActivity::Response(text)
                | crate::providers::AssistantActivity::Thinking(text) => text.len(),
                crate::providers::AssistantActivity::Tool(tool) => tool.output.len(),
                crate::providers::AssistantActivity::ToolCall {
                    id, name, result, ..
                } => id
                    .len()
                    .saturating_add(name.len())
                    .saturating_add(result.as_ref().map_or(0, |tool| tool.output.len())),
            })
            .sum::<usize>(),
    )
}

fn record_file(dir: &Path, id: ConversationId) -> PathBuf {
    dir.join(format!("{}{FILE_SUFFIX}", id.as_hex()))
}

fn persist_record(
    dir: Option<&Path>,
    record: &ConversationRecord,
) -> Result<(), ConversationError> {
    let file = record_to_file(record);
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| ConversationError::Persist)?;
    let reserved = if record.active_job.is_some() {
        let mut reserved_file = record_to_file(record);
        if let Some(message) = reserved_file.messages.last_mut() {
            message.text.clear();
            message.activity.clear();
            message.continuation.clear();
        }
        serde_json::to_vec_pretty(&reserved_file)
            .map_err(|_| ConversationError::Persist)?
            .len()
            .saturating_add(
                6 * (MAXIMUM_REPLY_BYTES
                    + crate::conversations::history::MAXIMUM_ACTIVITY_BYTES
                    + crate::conversations::history::MAXIMUM_CONTINUATION_BYTES
                    + crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES),
            )
    } else {
        bytes.len()
    };
    if bytes.len().max(reserved) > MAXIMUM_RECORD_BYTES {
        return Err(ConversationError::Full);
    }
    let Some(dir) = dir else {
        return Ok(());
    };
    crate::storage::ensure_private_dir(dir).map_err(|_| ConversationError::Persist)?;
    let file = record_file(dir, record.id);
    let mut total = bytes.len().max(reserved);
    for entry in fs::read_dir(dir).map_err(|_| ConversationError::Persist)? {
        let entry = entry.map_err(|_| ConversationError::Persist)?;
        if entry.path() == file
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let size = entry
            .metadata()
            .map_err(|_| ConversationError::Persist)?
            .len();
        total = total.saturating_add(usize::try_from(size).unwrap_or(usize::MAX));
        if total > MAXIMUM_STORE_BYTES {
            return Err(ConversationError::Full);
        }
    }
    match crate::storage::write_private_outcome(&file, &bytes) {
        Ok(()) => Ok(()),
        Err(crate::storage::PrivateWriteError::Unchanged) => Err(ConversationError::Persist),
        Err(crate::storage::PrivateWriteError::Replaced) => Err(ConversationError::Unsettled),
    }
}

fn persist_map(
    dir: Option<&Path>,
    conversations: &BTreeMap<ConversationId, ConversationRecord>,
) -> Result<(), ConversationError> {
    let mut total = 0usize;
    for record in conversations.values() {
        if dir.is_some() {
            let bytes = serde_json::to_vec_pretty(&record_to_file(record))
                .map_err(|_| ConversationError::Persist)?;
            total = total.saturating_add(bytes.len());
        }
        persist_record(dir, record)?;
    }
    if total > MAXIMUM_STORE_BYTES {
        return Err(ConversationError::Full);
    }
    Ok(())
}

fn record_to_file(record: &ConversationRecord) -> ConversationFile {
    ConversationFile {
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
        messages: record
            .messages
            .iter()
            .map(|message| MessageFile {
                id: message.id.as_hex(),
                role: message.role,
                text: message.text.clone(),
                activity: message.activity.clone(),
                continuation: message.continuation.clone(),
                status: message.status,
                error: message.error.clone(),
                request: message.request.map(|request| request.as_hex()),
                completion: message.completion,
            })
            .collect(),
        active_job: record.active_job.map(|request| request.as_hex()),
        continuation: record.continuation.as_ref().map(continuation_to_file),
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
    if !checkpoint.valid(messages) {
        return Err(ConversationError::Corrupt);
    }
    Ok(checkpoint)
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
