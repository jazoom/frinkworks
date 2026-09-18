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

use super::id::ConversationId;

mod handoff;

const CATALOGUE_VERSION: u32 = 1;
const CATALOGUE_FILE: &str = "catalogue.json";
const MAXIMUM_CATALOGUE_BYTES: usize = 8 * 1024 * 1024;
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageStatus {
    Complete,
    Pending,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationMessage {
    pub(crate) role: MessageRole,
    pub(crate) text: String,
    pub(crate) activity: Vec<crate::providers::AssistantActivity>,
    pub(crate) status: MessageStatus,
    pub(crate) error: Option<String>,
    pub(crate) request: Option<JobId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConversationError {
    Random,
    Persist,
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
            Self::Corrupt => "The conversation catalogue is unreadable.",
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
    path: Option<PathBuf>,
    inner: Mutex<BTreeMap<ConversationId, ConversationRecord>>,
    title_updates: tokio::sync::broadcast::Sender<()>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CatalogueFile {
    version: u32,
    conversations: Vec<ConversationFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ConversationFile {
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
    created_at_ms: u64,
    updated_at_ms: u64,
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
    role: MessageRole,
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    activity: Vec<crate::providers::AssistantActivity>,
    status: MessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(deserialize_with = "crate::storage::required_option")]
    request: Option<String>,
}

impl ConversationStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, ConversationError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| ConversationError::Persist)?;
        let path = crate::storage::confined_child(&dir, CATALOGUE_FILE)
            .map_err(|_| ConversationError::Persist)?;
        let mut conversations = load_path(&path)?;
        if interrupt_recovered_requests(&mut conversations) {
            persist(Some(&path), &conversations)?;
        }
        Ok(Self {
            path: Some(path),
            inner: Mutex::new(conversations),
            title_updates: tokio::sync::broadcast::channel(16).0,
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
            created_at_ms: now,
            updated_at_ms: now,
        };
        conversations.insert(id, record.clone());
        if let Err(error) = persist(self.path.as_deref(), &conversations) {
            conversations.remove(&id);
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
            created_at_ms: now,
            updated_at_ms: now,
        };
        let previous = conversations.clone();
        if let Some(source) = source {
            let mut updated = source.clone();
            updated.revision = source
                .revision
                .checked_add(1)
                .ok_or(ConversationError::Revision)?;
            updated.updated_at_ms = now.max(source.updated_at_ms);
            updated.candidate_reviews.push(review_link);
            conversations.insert(source.id, updated);
        }
        conversations.insert(id, review.clone());
        if let Err(error) = persist(self.path.as_deref(), &conversations) {
            *conversations = previous;
            return Err(error);
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
        let current = records.get(id).cloned().ok_or(ConversationError::Missing)?;
        if current.revision != revision {
            return Err(ConversationError::Conflict);
        }
        let mut updated = current.clone();
        updated.title = title;
        records.insert(*id, updated);
        if let Err(error) = persist(self.path.as_deref(), &records) {
            records.insert(*id, current);
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
                role: MessageRole::User,
                text,
                activity: Vec::new(),
                status: MessageStatus::Complete,
                error: None,
                request: None,
            });
            current.messages.push(ConversationMessage {
                role: MessageRole::Assistant,
                text: String::new(),
                activity: Vec::new(),
                status: MessageStatus::Pending,
                error: None,
                request: Some(request),
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
        if reply.text.len() > MAXIMUM_REPLY_BYTES
            || reply.text.contains('\0')
            || !valid_activity(&reply.activity, &reply.text)
        {
            return Err(ConversationError::Message);
        }
        // Transcript progress must not invalidate revision-bound execution controls.
        self.update(id, 0, false, |current| {
            let message = active_assistant(current, request)?;
            message.text = reply.text;
            message.activity = reply.activity;
            Ok(())
        })
        .map(|_| ())
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
        if !valid_activity(&reply.activity, &reply.text)
            || !valid_message_error(status, error.as_deref())
            || !matches!(
                status,
                MessageStatus::Complete | MessageStatus::Interrupted | MessageStatus::Failed
            )
            || reply.text.len() > MAXIMUM_REPLY_BYTES
            || reply.text.contains('\0')
        {
            return Err(ConversationError::Message);
        }
        self.replace(id, 0, |current| {
            let message = active_assistant(current, request)?;
            message.text = reply.text;
            message.activity = reply.activity;
            message.status = status;
            message.error = error;
            current.active_job = None;
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn delete(
        &self,
        id: &ConversationId,
        expected_revision: u32,
    ) -> Result<(), ConversationError> {
        let mut conversations = self.lock();
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
        if let Err(error) = persist(self.path.as_deref(), &conversations) {
            conversations.insert(*id, current);
            return Err(error);
        }
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
        if let Err(error) = persist(self.path.as_deref(), &conversations) {
            conversations.insert(*id, current);
            return Err(error);
        }
        Ok(updated)
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<ConversationId, ConversationRecord>> {
        self.inner
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

fn load_path(
    path: &Path,
) -> Result<BTreeMap<ConversationId, ConversationRecord>, ConversationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(ConversationError::Corrupt),
    }
    let bytes = crate::storage::read_private_bounded(path, MAXIMUM_CATALOGUE_BYTES)
        .map_err(|_| ConversationError::Corrupt)?;
    let file: CatalogueFile =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Corrupt)?;
    state_from_file(file)
}

fn state_from_file(
    file: CatalogueFile,
) -> Result<BTreeMap<ConversationId, ConversationRecord>, ConversationError> {
    if file.version != CATALOGUE_VERSION || file.conversations.len() > MAXIMUM_CONVERSATIONS {
        return Err(ConversationError::Corrupt);
    }
    let mut conversations = BTreeMap::new();
    for file in file.conversations {
        let record = record_from_file(file)?;
        if conversations.insert(record.id, record).is_some() {
            return Err(ConversationError::Corrupt);
        }
    }
    Ok(conversations)
}

fn record_from_file(file: ConversationFile) -> Result<ConversationRecord, ConversationError> {
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
        messages,
        active_job,
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
    let limit = match file.role {
        MessageRole::User => MAXIMUM_MESSAGE_BYTES,
        MessageRole::Assistant => MAXIMUM_REPLY_BYTES,
    };
    if file.text.len() > limit
        || file.text.contains('\0')
        || !valid_message_error(file.status, file.error.as_deref())
        || !valid_activity(&file.activity, &file.text)
        || (file.role == MessageRole::User && !file.activity.is_empty())
    {
        return Err(ConversationError::Corrupt);
    }
    let request = file.request.as_deref().and_then(JobId::parse);
    if file.request.is_some() != request.is_some()
        || matches!(file.role, MessageRole::User) != request.is_none()
        || (file.role == MessageRole::User
            && (file.status != MessageStatus::Complete
                || normalise_message(&file.text).as_ref() != Ok(&file.text)))
    {
        return Err(ConversationError::Corrupt);
    }
    Ok(ConversationMessage {
        role: file.role,
        text: file.text,
        activity: file.activity,
        status: file.status,
        error: file.error,
        request,
    })
}

fn valid_activity(activity: &[crate::providers::AssistantActivity], text: &str) -> bool {
    use crate::providers::AssistantActivity;
    if activity.is_empty() {
        return true;
    }
    if activity.len() > 256 {
        return false;
    }
    let mut bytes = 0usize;
    let mut response = String::new();
    for item in activity {
        let parts: Vec<&str> = match item {
            AssistantActivity::Response(value) => {
                response.push_str(value);
                vec![value]
            }
            AssistantActivity::Thinking(value) => vec![value],
            AssistantActivity::Tool(tool) => vec![&tool.label, &tool.output],
            AssistantActivity::ToolCall { id, name, result } => {
                if id.len() > 512 || name.len() > 512 {
                    return false;
                }
                let mut parts = vec![id.as_str(), name.as_str()];
                if let Some(tool) = result {
                    parts.extend([tool.label.as_str(), tool.output.as_str()]);
                }
                parts
            }
        };
        for part in parts {
            bytes = bytes.saturating_add(part.len());
            if part.contains('\0') || bytes > 256 * 1024 {
                return false;
            }
        }
    }
    response == text
}

fn valid_message_error(status: MessageStatus, error: Option<&str>) -> bool {
    error.is_none_or(|text| {
        status == MessageStatus::Failed
            && !text.trim().is_empty()
            && text.len() <= crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES
            && !text.chars().any(char::is_control)
    })
}

fn persist(
    path: Option<&Path>,
    conversations: &BTreeMap<ConversationId, ConversationRecord>,
) -> Result<(), ConversationError> {
    let records = conversations.values().map(record_to_file).collect();
    let mut catalogue = CatalogueFile {
        version: CATALOGUE_VERSION,
        conversations: records,
    };
    let bytes = serde_json::to_vec_pretty(&catalogue).map_err(|_| ConversationError::Persist)?;
    // Reserve complete payloads against empty pending messages. Partial output must not
    // change the reservation through JSON escaping or pretty-print indentation.
    let mut reserved = 0usize;
    for record in &mut catalogue.conversations {
        if record.active_job.is_some() {
            if let Some(message) = record.messages.last_mut() {
                message.text.clear();
                message.activity.clear();
            }
            reserved += 6
                * (MAXIMUM_REPLY_BYTES
                    + 256 * 1024
                    + crate::providers::MAXIMUM_PROVIDER_DETAIL_BYTES)
                + 256 * 512;
        }
    }
    let reserved_bytes = if reserved == 0 {
        bytes.len()
    } else {
        serde_json::to_vec_pretty(&catalogue)
            .map_err(|_| ConversationError::Persist)?
            .len()
            .saturating_add(reserved)
    };
    if bytes.len().max(reserved_bytes) > MAXIMUM_CATALOGUE_BYTES {
        return Err(ConversationError::Full);
    }
    let Some(path) = path else {
        return Ok(());
    };
    let dir = path.parent().ok_or(ConversationError::Persist)?;
    crate::storage::ensure_private_dir(dir).map_err(|_| ConversationError::Persist)?;
    crate::storage::write_private(path, &bytes).map_err(|_| ConversationError::Persist)
}

fn record_to_file(record: &ConversationRecord) -> ConversationFile {
    ConversationFile {
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
                role: message.role,
                text: message.text.clone(),
                activity: message.activity.clone(),
                status: message.status,
                error: message.error.clone(),
                request: message.request.map(|request| request.as_hex()),
            })
            .collect(),
        active_job: record.active_job.map(|request| request.as_hex()),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
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
