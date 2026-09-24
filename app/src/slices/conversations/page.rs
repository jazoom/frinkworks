pub(crate) mod model_picker;
mod presets;
use presets::{PresetPreviewView, PresetSettingView, setup_rows};

use askama::Template;
use model_picker::ModelPicker;

#[cfg(test)]
mod tests;

use crate::{
    agents::AgentRecord,
    conversations::{
        ConversationId, ConversationMessage, ConversationModelConfiguration, ConversationRecord,
        MessageId, MessageRole, MessageStatus,
    },
    environments::{EnvironmentCatalogue, EnvironmentId, EnvironmentSnapshotRepository},
    models::models_dev::ModelsDevCatalogue,
    providers::ModelSelection,
    sessions::{JobSnapshot, JobStatus},
    vault::ProviderVault,
    workflows::WorkflowRun,
};

pub(super) const CATALOGUE_TITLE: &str = "Conversations | Power Plant";

pub(super) struct ConversationListItem {
    pub(super) title: String,
    pub(super) href: String,
    pub(super) status: &'static str,
    pub(super) dot: &'static str,
    pub(super) meta: String,
}

pub(super) struct DirectoryView {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) alias: String,
    pub(super) host_path: String,
    pub(super) guest_path: String,
    pub(super) form_value: String,
    pub(super) available: bool,
    pub(super) sensitive: bool,
    pub(super) pending_approval: bool,
    pub(super) transient: bool,
    pub(super) review_before_apply: bool,
    pub(super) direct_write: bool,
    pub(super) access_label: &'static str,
    pub(super) exclusions: Vec<String>,
}

#[derive(Clone)]
pub(super) struct CandidateChangeView {
    pub(super) path: String,
    pub(super) name: String,
    pub(super) directory: String,
    pub(super) status: &'static str,
    pub(super) preview: String,
    pub(super) additions: usize,
    pub(super) removals: usize,
    pub(super) has_counts: bool,
}

#[derive(Clone)]
pub(super) struct PendingCodeGateView {
    pub(super) run_id: String,
    pub(super) gate_id: String,
    pub(super) revision: String,
    pub(super) candidate: String,
    pub(super) diff_base: String,
    pub(super) diff_href: String,
    pub(super) review_href: String,
    pub(super) ordinary: bool,
    pub(super) commit_on_approval: bool,
    pub(super) application_destination: String,
    pub(super) can_request_revision: bool,
    pub(super) quick_task: bool,
    pub(super) exclusions: Vec<String>,
    pub(super) changes: Vec<CandidateChangeView>,
    pub(super) total_changes: usize,
    pub(super) changes_truncated: bool,
}

#[derive(Template)]
#[template(path = "conversations/templates/index.html")]
pub(super) struct CatalogueView {
    pub(super) conversations: Vec<ConversationListItem>,
    pub(super) directories: Vec<HistoryDirectoryOption>,
    pub(super) filter: String,
    pub(super) query: String,
    pub(super) error: &'static str,
    pub(super) back_href: String,
    pub(super) back_label: &'static str,
    pub(super) has_more: bool,
    pub(super) next_href: String,
    pub(super) newest_href: String,
    pub(super) paged: bool,
}

pub(super) struct HistoryDirectoryOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) selected: bool,
    available: bool,
}

// The identifier breaks timestamp ties without mutable row offsets.
pub(super) const CATALOGUE_PAGE_SIZE: usize = 50;

impl CatalogueView {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_records(
        state: &crate::state::AppState,
        records: &[crate::conversations::ConversationMetadata],
        filter: &str,
        query: &str,
        cursor: Option<(u64, crate::conversations::ConversationId)>,
        error: &'static str,
        back_href: String,
        back_label: &'static str,
    ) -> Self {
        let needle = query.trim().to_lowercase();
        let mut ordered: Vec<_> = records
            .iter()
            .filter(|record| {
                (filter.is_empty()
                    || history_grants(record).any(|grant| history_directory_key(grant) == filter))
                    && (needle.is_empty() || record.title.to_lowercase().contains(&needle))
            })
            .filter(|record| cursor.is_none_or(|cursor| catalogue_after(record, cursor)))
            .collect();
        // Filters apply before the page bound so every match is reachable.
        ordered.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        let has_more = ordered.len() > CATALOGUE_PAGE_SIZE;
        ordered.truncate(CATALOGUE_PAGE_SIZE);
        let next_href = ordered
            .last()
            .filter(|_| has_more)
            .map(|record| {
                catalogue_href(
                    filter,
                    query,
                    &back_href,
                    Some(&format!("{}-{}", record.updated_at_ms, record.id.as_hex())),
                )
            })
            .unwrap_or_default();
        let newest_href = if cursor.is_some() {
            catalogue_href(filter, query, &back_href, None)
        } else {
            String::new()
        };
        let conversations: Vec<_> = ordered
            .into_iter()
            .map(|record| {
                let status = super::recent::conversation_status(state, record);
                ConversationListItem {
                    href: format!("/conversations/{}", record.id.as_hex()),
                    title: record.title.clone(),
                    status,
                    dot: super::recent::status_dot(status),
                    meta: super::recent::conversation_meta(state, record),
                }
            })
            .collect();
        let mut directories = std::collections::BTreeMap::new();
        for grant in records.iter().flat_map(history_grants) {
            let id = history_directory_key(grant);
            let available = grant.is_available();
            let option = HistoryDirectoryOption {
                selected: filter == id,
                id: id.clone(),
                name: format!(
                    "{}{}",
                    grant.host_path.display(),
                    if available { "" } else { " — Unavailable" }
                ),
                available,
            };
            let existing = directories.entry(id).or_insert(option);
            if available && !existing.available {
                existing.name = grant.host_path.display().to_string();
                existing.available = true;
            }
        }
        let mut directories: Vec<_> = directories.into_values().collect();
        directories.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            conversations,
            directories,
            filter: filter.to_owned(),
            query: query.trim().to_owned(),
            error,
            back_href,
            back_label,
            has_more,
            next_href,
            newest_href,
            paged: cursor.is_some(),
        }
    }
}

fn catalogue_after(
    record: &crate::conversations::ConversationMetadata,
    cursor: (u64, crate::conversations::ConversationId),
) -> bool {
    match record.updated_at_ms.cmp(&cursor.0) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => record.id > cursor.1,
    }
}

fn catalogue_href(filter: &str, query: &str, back_href: &str, cursor: Option<&str>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if !filter.is_empty() {
        serializer.append_pair("directory", filter);
    }
    let query = query.trim();
    if !query.is_empty() {
        serializer.append_pair("q", query);
    }
    let back = back_href.trim_start_matches("/conversations/");
    if !back.is_empty() {
        serializer.append_pair("conversation", back);
    }
    if let Some(cursor) = cursor {
        serializer.append_pair("cursor", cursor);
    }
    let encoded = serializer.finish();
    if encoded.is_empty() {
        "/conversations".to_owned()
    } else {
        format!("/conversations?{encoded}")
    }
}

pub(super) fn history_grants(
    record: &crate::conversations::ConversationMetadata,
) -> impl Iterator<Item = &crate::execution::DirectoryGrant> {
    record
        .model
        .iter()
        .flat_map(|model| &model.settings.directories)
}

pub(super) fn history_directory_key(grant: &crate::execution::DirectoryGrant) -> String {
    format!(
        "{:016x}-{:016x}",
        grant.identity.device, grant.identity.inode
    )
}

#[derive(Template)]
#[template(
    path = "conversations/templates/candidate_review.html",
    block = "candidate_review_page"
)]
pub(super) struct CandidateReviewView {
    pub(super) run_id: String,
    pub(super) source_title: String,
    pub(super) candidate_id: String,
    pub(super) diff_base_id: String,
    pub(super) candidate_hash: String,
    pub(super) diff_base_hash: String,
    pub(super) preview: String,
    pub(super) instructions_summary: String,
    pub(super) brief: String,
    pub(super) reviewer_summary: String,
    pub(super) model_picker: ModelPicker,
    pub(super) reviewer_agents: Vec<PresetOption>,
    pub(super) error: &'static str,
}

#[derive(Template)]
#[template(
    path = "conversations/templates/candidate_review.html",
    block = "candidate_review_detail"
)]
pub(super) struct CandidateReviewContents<'a> {
    pub(super) run_id: &'a str,
    pub(super) source_title: &'a str,
    pub(super) candidate_id: &'a str,
    pub(super) diff_base_id: &'a str,
    pub(super) candidate_hash: &'a str,
    pub(super) diff_base_hash: &'a str,
    pub(super) preview: &'a str,
    pub(super) instructions_summary: &'a str,
    pub(super) brief: &'a str,
    pub(super) reviewer_summary: &'a str,
    pub(super) model_picker: &'a ModelPicker,
    pub(super) reviewer_agents: &'a [PresetOption],
    pub(super) error: &'static str,
}

impl CandidateReviewView {
    pub(super) fn contents(&self) -> CandidateReviewContents<'_> {
        CandidateReviewContents {
            run_id: &self.run_id,
            source_title: &self.source_title,
            candidate_id: &self.candidate_id,
            diff_base_id: &self.diff_base_id,
            candidate_hash: &self.candidate_hash,
            diff_base_hash: &self.diff_base_hash,
            preview: &self.preview,
            instructions_summary: &self.instructions_summary,
            brief: &self.brief,
            reviewer_summary: &self.reviewer_summary,
            model_picker: &self.model_picker,
            reviewer_agents: &self.reviewer_agents,
            error: self.error,
        }
    }
}

pub(super) struct MessageView {
    pub(super) id: String,
    pub(super) user: bool,
    pub(super) is_command: bool,
    pub(super) role_label: String,
    /// True for `!!`, which excludes the command from model context.
    pub(super) excluded: bool,
    pub(super) html: String,
    /// The original skill command, retained for display beside the expanded
    /// message. A plain or assistant entry leaves this absent.
    pub(super) command: Option<String>,
    pub(super) copy: Option<CopyResponseView>,
    pub(super) status: &'static str,
    pub(super) error: String,
    pub(super) streaming: bool,
    pub(super) forkable: bool,
    pub(super) fork_href: String,
    pub(super) revisable: bool,
    pub(super) revise_href: String,
}

/// The frozen replacement prompt that the composer restores. The client binds
/// the source, parent and expected active leaf so a stale form cannot append
/// to a branch that another mutation already changed.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct RevisionDraft {
    pub(super) source: String,
    pub(super) parent: String,
    pub(super) active_leaf: String,
    pub(super) revision: String,
    pub(super) text: String,
}

impl RevisionDraft {
    pub(super) fn from_source(
        source: crate::conversations::RevisionSource,
        revision: u32,
        active_leaf: Option<MessageId>,
    ) -> Self {
        Self {
            source: source.id.as_hex(),
            parent: source
                .parent
                .map(|parent| parent.as_hex())
                .unwrap_or_default(),
            active_leaf: active_leaf.map(|leaf| leaf.as_hex()).unwrap_or_default(),
            revision: revision.to_string(),
            text: source.text,
        }
    }
}

pub(super) struct CopyResponseView {
    /// Raw response Markdown. The template escapes it before it reaches HTML.
    pub(super) source: String,
    /// HTML-escaped source length. Askama escapes five characters as five-byte
    /// numeric entities, so the transcript budget counts the real payload.
    pub(super) escaped_bytes: usize,
}

pub(super) struct CandidateReviewLinkView {
    pub(super) title: String,
    pub(super) href: String,
    pub(super) run_href: String,
    pub(super) candidate_hash: String,
    pub(super) diff_base_hash: String,
}

pub(super) struct ApplyOutcomeView {
    pub(super) directory: String,
    pub(super) path: String,
    pub(super) outcome: &'static str,
}

pub(super) struct WorkflowProgressView {
    pub(super) run_href: String,
    pub(super) name: String,
    pub(super) state: &'static str,
    pub(super) current_step: String,
    pub(super) result: &'static str,
    pub(super) task_progress: String,

    pub(super) conversation_id: String,
    pub(super) apply_run_id: String,
    pub(super) apply_attempt_id: String,
    pub(super) apply_state: &'static str,
    pub(super) apply_outcomes: Vec<ApplyOutcomeView>,
    pub(super) apply_resolve_href: String,
    pub(super) apply_partial: bool,
    pub(super) apply_uncertain: bool,
    pub(super) apply_complete: bool,
    pub(super) settlement_eligible: bool,
    pub(super) run_terminal: bool,
}

pub(super) struct ModelSources<'a> {
    pub(super) vault: &'a ProviderVault,
    pub(super) preferences: &'a crate::preferences::Preferences,
    pub(super) models: &'a ModelsDevCatalogue,
    pub(super) environments: &'a EnvironmentCatalogue,
    pub(super) environment_snapshots: &'a EnvironmentSnapshotRepository,
    pub(super) presets: &'a [crate::presets::PresetRecord],
}

pub(super) struct PresetOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) selected: bool,
}

pub(super) struct ProviderOption {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

#[derive(Clone)]
pub(super) struct NetworkOption {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) selected: bool,
}

pub(super) struct HostCommandView {
    pub(super) request: String,
    pub(super) job: String,
    pub(super) revision: String,
    pub(super) command: String,
    pub(super) command_input: String,
    pub(super) directory: String,
    pub(super) explanation: String,
    pub(super) approval_policy: String,
}

pub(super) struct PendingCommandView {
    pub(super) location: String,
    pub(super) cleanup: bool,
}

pub(super) struct ExecutionSwitchView {
    pub(super) requested_location: String,
    pub(super) requested_host_approval: String,
    pub(super) requested_environment: String,
    pub(super) directory_access: String,
    pub(super) requested_network: String,
    pub(super) requested_network_domains: String,
    pub(super) rows: Vec<ExecutionChangeRow>,
    pub(super) access_lines: Vec<String>,
    pub(super) host_effects_remain: bool,
    pub(super) needs_new_consent: bool,
    pub(super) active_job: bool,
    pub(super) job_id: String,
    pub(super) gate: Option<EnvironmentSwitchGateView>,
}

pub(super) struct ExecutionChangeRow {
    pub(super) label: String,
    pub(super) current: String,
    pub(super) requested: String,
    pub(super) changed: bool,
}

pub(super) struct EnvironmentSwitchGateView {
    pub(super) run_id: String,
    pub(super) gate_id: String,
    pub(super) revision: String,
    pub(super) candidate: String,
    pub(super) review_href: String,
}

use crate::slices::execution_settings::page::{
    EnvironmentOption, ToolOption, environment_options, tool_options,
};

pub(super) struct SubmittedSettingsFields<'a> {
    pub(super) provider: &'a str,
    pub(super) model: &'a str,
    pub(super) thinking: &'a str,
    pub(super) instructions: String,
    pub(super) tools: Vec<String>,
    pub(super) location: &'a str,
    pub(super) host_approval: &'a str,
    pub(super) environment: &'a str,
    pub(super) network: &'a str,
    pub(super) network_domains: &'a str,
}

pub(super) enum ConversationPageState {
    New { message: String },
    Saved(Box<SavedConversationState>),
}

#[derive(Template)]
#[template(path = "conversations/templates/detail.html", blocks = ["conversation_detail"])]
pub(super) struct ConversationDetailView {
    pub(super) heading: String,
    pub(super) document_title: String,
    pub(super) title: String,
    pub(super) state: ConversationPageState,
    pub(super) error: &'static str,
    pub(super) notice: &'static str,
    show_thinking: bool,
    thinking_visibility_error: Option<&'static str>,
    pub(super) messages: Vec<MessageView>,
    pub(super) companion_html: String,
    pub(super) companion_kind: &'static str,
    pub(super) companion_title: String,

    pub(super) omitted_messages: usize,
    window_entries: Vec<(String, String)>,
    /// The inspection leaf query suffix. Omitted-entry links keep the
    /// inspected path instead of jumping to the active leaf.
    pub(super) inspection_leaf_query: String,
    pub(super) earlier_href: String,
    pub(super) later_href: String,
    pub(super) latest_href: String,
    pub(super) historical: bool,
    pub(super) window_nav: bool,
    pub(super) history_status: String,
    pub(super) model_picker: ModelPicker,
    pub(super) presets: Vec<PresetOption>,
    pub(super) preset_name: String,
    pub(super) preset_source: String,
    preset_preview: Option<PresetPreviewView>,
    preset_setup: Vec<PresetSettingView>,
    pub(super) preset_save_open: bool,
    pub(super) directories: Vec<DirectoryView>,
    pub(super) data_root: String,
    pub(super) consent_path: String,
    pub(super) consent_request: String,
    pub(super) pending_directory: String,
    pub(super) consent_existing: bool,
    pub(super) consent_reviewed: bool,
    pub(super) consent_direct: bool,
    pub(super) consent_sensitive: bool,
    pub(super) draft_nonce: String,
    pub(super) prepared_run: String,
    pub(super) fork_source: String,
    pub(super) handoff_settings: String,
    pub(super) handoff_approved: bool,
    pub(super) prepared_recovery: Option<super::handoff::recovery::RecoveryView>,
    pub(super) draft_preset_reference: String,
    pub(super) consent_reference: String,
    pub(super) instructions: String,
    instruction_sources: Option<InstructionSourcesView>,
    instruction_sources_error: bool,
    pub(super) tool_options: Vec<ToolOption>,
    pub(super) environment_options: Vec<EnvironmentOption>,
    pub(super) environment_summary: String,
    pub(super) execution_switch: Option<ExecutionSwitchView>,
    pub(super) network_options: Vec<NetworkOption>,
    pub(super) network_restricted: bool,
    pub(super) network_domains: String,
    pub(super) network_summary: String,
    pub(super) location_host: bool,
    pub(super) host_summary: String,
    pub(super) host_user: String,
    pub(super) host_elevated: bool,
    pub(super) host_approval_automatic: bool,
    pub(super) host_pending_approval: bool,
    pub(super) host_consent_request: String,
    pub(super) pending_host_command: Option<HostCommandView>,
    /// A direct command that is still running. It reports the selected
    /// location and any unfinished sandbox cleanup.
    pub(super) pending_command: Option<PendingCommandView>,
    pub(super) pending_question: Option<super::questions::QuestionView>,
    pub(super) continuation: Option<super::continuation::ContinuationView>,
    pub(super) observe_active: bool,
    pub(super) settings_open: bool,
    pub(super) directories_open: bool,
    pub(super) model_available: bool,
    pub(super) job_active: bool,
    pub(super) retry: Option<RetryView>,
    pub(super) context: Option<ContextView>,
    pub(super) can_compact: bool,
    pub(super) compaction: Option<super::compaction::CompactionView>,
    summary_usage: Option<UsagePanelView>,
    pub(super) queue: super::queue::QueueView,
    /// Session-staged images for this composer scope. The composer submits
    /// their references and never the bytes.
    pub(super) attachments: super::attachments::AttachmentsView,
    /// A new draft that continues copied context. It is a context alternative,
    /// not a file rollback and not a permission transfer.
    pub(super) fork: Option<ForkNotice>,
    /// A pending replacement for an earlier user prompt. It holds no durable
    /// mutation until Send.
    pub(super) revision: Option<RevisionDraft>,
}

pub(super) struct ForkNotice {
    pub(super) source_title: String,
    pub(super) entries: usize,
    pub(super) candidate_review: bool,
}

pub(super) struct ContextView {
    pub(super) label: String,
    /// Catalogue capacity is unknown, so the limit is an operational fallback.
    pub(super) unknown_capacity: bool,
    /// The count came from an approximation instead of a model tokenizer.
    pub(super) approximate: bool,
    /// Runtime instructions or tools were unavailable for this estimate.
    pub(super) partial: bool,
    /// A provider-reported count replaced tokenisation for this request.
    pub(super) measured: bool,
    pub(super) compacting: bool,
}

pub(super) struct InstructionSourcesView {
    paths: Vec<String>,
    href: String,
    truncated: bool,
}

pub(super) struct RetryView {
    pub(super) message: String,
}

pub(super) struct SavedConversationState {
    pub(super) id: String,
    pub(super) revision: String,
    pub(super) job_id: String,
    pub(super) cursor: u64,
    pub(super) pending_gate: Option<PendingCodeGateView>,

    pub(super) source_candidate_review: Option<CandidateReviewLinkView>,
    pub(super) linked_candidate_reviews: Vec<CandidateReviewLinkView>,
    pub(super) workflow_progress: Option<WorkflowProgressView>,
    pub(super) direct_results: Vec<crate::slices::workflow_runs::DirectChangesView>,
}
impl ConversationDetailView {
    pub(super) fn from_new(
        state: &crate::state::AppState,
        session: crate::sessions::SessionId,
        form: super::new::NewForm,
        error: &'static str,
    ) -> Self {
        let fork = state
            .forks
            .get(session, &form.draft_nonce)
            .map(|snapshot| ForkNotice {
                source_title: snapshot.source_title,
                entries: snapshot.messages.len(),
                candidate_review: snapshot.candidate_review,
            });
        let handoff_settings = super::handoff::transfer::draft_run(state, session, &form)
            .ok()
            .flatten()
            .map(|run| super::handoff::transfer::settings_text(&run))
            .unwrap_or_default();
        let preset_setup = setup_rows(
            super::new::settings_snapshot(state, session, &form)
                .ok()
                .flatten()
                .as_ref()
                .map(|model| &model.settings),
            &state.environments,
        );
        let selected_tools = form.tool_values();
        let host_identity = crate::execution::HostIdentity::current();
        let location_host = form.location == crate::execution::ToolLocation::Host.as_str();
        let host_approval_automatic =
            crate::execution::HostApprovalPolicy::parse(if form.host_approval.trim().is_empty() {
                "ask-each-time"
            } else {
                form.host_approval.trim()
            })
            .is_some_and(crate::execution::HostApprovalPolicy::automatic);
        let host_pending_approval = super::new::settings_snapshot(state, session, &form)
            .ok()
            .flatten()
            .is_some_and(|model| {
                model.settings.host_tools()
                    && !state.access_consent.authorised_host_draft(
                        &form.consent_reference,
                        session,
                        &form.consent_nonce(),
                        &model.settings,
                    )
            });
        let host_consent_request = form.host_consent_request.clone();
        let draft_directories = form.directories().unwrap_or_default();
        let mut directories = directory_views(&draft_directories);
        for (view, grant) in directories.iter_mut().zip(&draft_directories) {
            view.sensitive = crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            );
            view.exclusions = crate::workflows::workspace::reviewed_capture_exclusions(
                &grant.host_path,
                state.local_data.root(),
            );
            view.pending_approval = grant.requires_access_consent(state.local_data.root())
                && !state.access_consent.authorised_draft(
                    &form.consent_reference,
                    session,
                    &form.consent_nonce(),
                    &draft_directories,
                    grant,
                );
        }
        let consent_path = form
            .pending_directory()
            .map(|grant| grant.host_path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let consent_existing = form.consent_existing == "true";
        let consent_sensitive = form.pending_directory().is_some_and(|grant| {
            crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            )
        });
        let consent_direct = form
            .pending_directory()
            .is_some_and(|grant| grant.access == crate::execution::DirectoryAccess::DirectWrite);
        let consent_reviewed = form.pending_directory().is_some_and(|grant| {
            grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply
        });
        if !consent_existing && let Some(grant) = form.pending_directory() {
            let mut view = directory_view(&grant);
            view.sensitive = true;
            view.pending_approval = true;
            view.transient = true;
            directories.push(view);
        }
        Self {
            show_thinking: state.preferences.show_thinking(),
            thinking_visibility_error: None,
            heading: "New conversation".to_owned(),
            document_title: "New conversation | Power Plant".to_owned(),
            title: form.title,
            model_picker: ModelPicker::new(
                &state.vault,
                &state.preferences,
                &state.models_dev,
                &form.provider,
                &form.model,
                &form.thinking,
            ),
            presets: state
                .presets
                .list()
                .into_iter()
                .map(|preset| PresetOption {
                    id: preset.id.as_hex(),
                    name: preset.name,
                    description: preset_summary(&preset.settings),
                    selected: preset.id.as_hex() == form.preset,
                })
                .collect(),
            preset_source: String::new(),
            preset_name: if form.preset_name.trim().is_empty() {
                draft_directories
                    .first()
                    .and_then(|grant| grant.host_path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled preset".to_owned())
            } else {
                form.preset_name.clone()
            },
            preset_preview: None,
            preset_setup,
            preset_save_open: false,
            state: ConversationPageState::New {
                message: form.message,
            },
            error,
            notice: "",
            messages: Vec::new(),
            companion_html: String::new(),
            companion_kind: "activity",
            companion_title: String::new(),

            omitted_messages: 0,
            window_entries: Vec::new(),
            inspection_leaf_query: String::new(),
            earlier_href: String::new(),
            later_href: String::new(),
            latest_href: String::new(),
            historical: false,
            window_nav: false,
            history_status: String::new(),
            directories,
            data_root: state.local_data.root().to_string_lossy().into_owned(),
            consent_path,
            consent_request: form.consent_request,
            pending_directory: form.pending_directory,
            consent_existing,
            consent_direct,
            consent_sensitive,
            consent_reviewed,
            draft_nonce: form.draft_nonce.clone(),
            prepared_run: form.prepared_run,
            fork_source: form.fork_source,
            handoff_settings,
            handoff_approved: form.handoff_approval == "continue-prepared",
            prepared_recovery: None,
            draft_preset_reference: form.preset_preview,
            consent_reference: form.consent_reference,
            instructions: form.instructions.clone(),
            instruction_sources: None,
            instruction_sources_error: false,
            tool_options: tool_options(&selected_tools),
            environment_options: environment_options(
                &state.environments,
                &state.environment_snapshots,
                EnvironmentId::parse(&form.environment),
            ),
            environment_summary: environment_summary(
                &state.environments,
                EnvironmentId::parse(&form.environment),
            ),
            execution_switch: None,
            network_options: network_options(&form.network),
            network_restricted: network_restricted(&form.network),
            network_domains: form.network_domains,
            network_summary: network_summary_from_form(&form.network),
            location_host,
            host_summary: host_access_summary(location_host, host_approval_automatic),
            host_user: format!(
                "{} (uid {}, effective uid {})",
                host_identity.username, host_identity.uid, host_identity.euid
            ),
            host_elevated: host_identity.elevated(),
            host_approval_automatic,
            host_pending_approval,
            host_consent_request,
            pending_host_command: None,
            pending_command: None,
            pending_question: None,
            continuation: None,
            observe_active: false,
            settings_open: false,
            directories_open: false,
            model_available: !form.model.is_empty(),
            job_active: false,
            retry: None,
            context: None,
            can_compact: false,
            compaction: None,
            summary_usage: None,
            queue: super::queue::QueueView::from_queue(
                &crate::conversations::ConversationQueue::default(),
                false,
                false,
            ),
            attachments: super::attachments::page::view(
                state,
                session,
                &super::attachments::page::draft_scope(&form.draft_nonce),
            ),
            fork,
            revision: None,
        }
    }

    fn draft_access_note(&self) -> &'static str {
        if self
            .directories
            .iter()
            .any(|directory| !directory.available)
        {
            "A directory is unavailable. Open Setup to review your directories."
        } else if self.location_host {
            if self.host_pending_approval {
                "Host execution requires approval. Host tools have unrestricted access."
            } else {
                "Host tools have unrestricted access. File changes take effect immediately."
            }
        } else if self
            .directories
            .iter()
            .any(|directory| directory.pending_approval)
        {
            "Directory access requires approval. Open Setup to review the requested access."
        } else if self
            .directories
            .iter()
            .any(|directory| directory.direct_write)
        {
            "Direct write changes files immediately. Discard cannot undo those changes."
        } else if self
            .directories
            .iter()
            .any(|directory| directory.review_before_apply)
        {
            "Review before apply keeps changes isolated until you choose to apply them."
        } else {
            "Your directories are read-only. The agent can inspect files without changes."
        }
    }

    fn host_start_directory(&self) -> String {
        self.directories.first().map_or_else(
            || {
                crate::execution::command_directory(&[])
                    .display()
                    .to_string()
            },
            |directory| directory.host_path.clone(),
        )
    }

    fn fresh_draft_href(&self) -> String {
        self.saved().map_or_else(
            || "/conversations/new".to_owned(),
            |saved| format!("/conversations/new?source={}", saved.id),
        )
    }

    pub(super) fn saved(&self) -> Option<&SavedConversationState> {
        match &self.state {
            ConversationPageState::New { .. } => None,
            ConversationPageState::Saved(saved) => Some(saved),
        }
    }

    fn is_new(&self) -> bool {
        self.saved().is_none()
    }

    fn subtitle_directory(&self) -> &str {
        self.directories
            .first()
            .map_or("No directory", |directory| directory.name.as_str())
    }

    fn work_non_idle(&self) -> bool {
        self.saved().is_some_and(|saved| {
            self.job_active
                || self.pending_host_command.is_some()
                || self.pending_command.is_some()
                || self.pending_question.is_some()
                || self.continuation.is_some()
                || saved.pending_gate.is_some()
                || saved.workflow_progress.is_some()
        })
    }

    fn needs_review(&self) -> bool {
        self.saved().is_some_and(|saved| {
            saved.pending_gate.is_some()
                || self.pending_host_command.is_some()
                || self.prepared_recovery.is_some()
        })
    }

    fn draft_message(&self) -> &str {
        if let Some(revision) = &self.revision {
            return &revision.text;
        }
        match &self.state {
            ConversationPageState::New { message, .. } => message,
            ConversationPageState::Saved(_) => "",
        }
    }

    fn model_form(&self) -> &'static str {
        if self.is_new() {
            "conversation-composer"
        } else {
            "conversation-settings-form"
        }
    }

    fn selection_form(&self) -> &'static str {
        if self.is_new() {
            "conversation-composer"
        } else {
            "conversation-model-form"
        }
    }

    fn composer_action(&self) -> String {
        self.saved().map_or_else(
            || "/conversations/new".to_owned(),
            |saved| {
                // A replacement prompt is a branch mutation, never a queue
                // entry. The command route validates the bound revision.
                if self.queue.follow_up && self.revision.is_none() {
                    format!("/conversations/{}/queue", saved.id)
                } else {
                    format!("/conversations/{}/messages", saved.id)
                }
            },
        )
    }

    /// Leave a replacement draft without a durable mutation. The link keeps
    /// the inspected transcript path so cancellation never jumps to new work.
    fn cancel_revision_href(&self) -> String {
        let Some(saved) = self.saved() else {
            return "/conversations/new".to_owned();
        };
        let leaf = self.inspection_leaf_query.trim_start_matches('&');
        if leaf.is_empty() {
            format!("/conversations/{}", saved.id)
        } else {
            format!("/conversations/{}?{}", saved.id, leaf)
        }
    }

    fn follow_up_submit_label(&self) -> &'static str {
        if self.queue.follow_up && self.revision.is_none() {
            "Queue"
        } else {
            "Send message"
        }
    }

    fn follow_up_submit_value(&self) -> &'static str {
        if self.queue.follow_up && self.revision.is_none() {
            "queue"
        } else {
            "send"
        }
    }

    // Queue entry does not grant authority to pass an execution gate.
    fn composer_disabled(&self) -> bool {
        if self.is_new() {
            return false;
        }
        !self.model_available && !self.location_host
    }

    /// Staged images need a model with known image-input support. The server
    /// rejects the request anyway; this note explains the mismatch before the
    /// user submits. An unknown capability is not treated as supported.
    fn image_compatibility_note(&self) -> &'static str {
        if self.attachments.attachments.is_empty() || self.model_picker.model.is_empty() {
            return "";
        }
        match self.model_picker.selected_image_input {
            Some(true) => "",
            Some(false) => {
                "The selected model does not accept images. Choose a model with image input."
            }
            None => {
                "Power Plant cannot confirm image input for the selected model. Choose a model with image input."
            }
        }
    }

    fn transcript_empty(&self) -> bool {
        self.messages.is_empty()
            && self.saved().is_none_or(|saved| {
                saved.workflow_progress.is_none()
                    && saved.pending_gate.is_none()
                    && saved.source_candidate_review.is_none()
            })
    }

    #[cfg(test)]
    pub(super) fn from_record(
        record: &ConversationRecord,
        sources: ModelSources<'_>,
        agents: &[AgentRecord],
        job: Option<&JobSnapshot>,
        title: &str,
        error: &'static str,
        transcript: Option<&crate::conversations::TranscriptWindow>,
    ) -> Self {
        Self::from_record_with_gate(
            record,
            sources,
            agents,
            job,
            title,
            error,
            None,
            None,
            Vec::new(),
            super::attachments::AttachmentsView::empty(String::new()),
            transcript,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_record_with_gate(
        record: &ConversationRecord,
        sources: ModelSources<'_>,
        agents: &[AgentRecord],
        job: Option<&JobSnapshot>,
        title: &str,
        error: &'static str,
        pending_gate: Option<PendingCodeGateView>,

        source_candidate_review: Option<CandidateReviewLinkView>,
        linked_candidate_reviews: Vec<CandidateReviewLinkView>,
        attachments: super::attachments::AttachmentsView,
        transcript: Option<&crate::conversations::TranscriptWindow>,
        inspection_leaf: Option<MessageId>,
    ) -> Self {
        let fallback = sources
            .preferences
            .desk_providers(sources.vault)
            .into_iter()
            .find(|provider| provider.selected)
            .and_then(|mut connection| {
                connection.model = sources
                    .models
                    .preferred_model(connection.kind, &connection.model)?;
                crate::workflows::alpine_git_id(sources.environments)
                    .ok()
                    .map(|environment| {
                        ConversationModelConfiguration::direct(
                            ModelSelection {
                                provider: connection.kind,
                                thinking: sources.models.effective_effort(
                                    connection.kind,
                                    &connection.model,
                                    connection.thinking.as_ref(),
                                ),
                                model: connection.model,
                            },
                            environment,
                        )
                    })
            });
        let configuration = record.model.as_ref().or(fallback.as_ref());
        let selection = configuration.map(|configuration| &configuration.settings.model);
        let selected_environment = configuration.map(|model| model.settings.environment);
        let network_options = network_options(record.network.as_str());
        let network_domains = record.network.domains().join("\n");
        let _ = agents;
        let effective_network = &record.network;
        let network_summary = network_summary_from_form(effective_network.as_str());
        let model_picker = ModelPicker::new(
            sources.vault,
            sources.preferences,
            sources.models,
            selection.map_or("", |selection| selection.provider.as_str()),
            selection.map_or("", |selection| selection.model.as_str()),
            selection
                .and_then(|selection| selection.thinking.as_ref())
                .map_or("", |effort| effort.as_str()),
        );
        let presets = sources
            .presets
            .iter()
            .map(|record| PresetOption {
                id: record.id.as_hex(),
                name: record.name.clone(),
                description: preset_summary(&record.settings),
                selected: configuration
                    .and_then(|configuration| configuration.preset.as_ref())
                    .is_some_and(|preset| preset.id == record.id),
            })
            .collect();
        let (job_id, cursor, job_active, observe_active) = match job {
            Some(job) if job.status == JobStatus::Running => {
                (job.id.as_hex(), job.latest_seq, true, true)
            }
            Some(job)
                if matches!(
                    job.status,
                    JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion
                ) =>
            {
                (job.id.as_hex(), job.latest_seq, true, false)
            }
            _ => (String::new(), 0, false, false),
        };
        let retry = job.and_then(|job| {
            job.retry.as_ref().map(|retry| RetryView {
                message: retry_status_message(retry.attempt, retry.delay),
            })
        });
        let location_host = configuration
            .is_some_and(|model| model.settings.location == crate::execution::ToolLocation::Host);
        let host_approval_automatic =
            configuration.is_some_and(|model| model.settings.host_approval.automatic());
        let host_identity = crate::execution::HostIdentity::current();
        // The escaped model catalogue shares the transcript envelope.
        let message_budget =
            (672_usize * 1024).saturating_sub(ammonia::clean_text(&model_picker.catalogue).len());
        let (mut messages, rendered) = match transcript {
            Some(window) => message_views(record, message_budget, window.messages.len()),
            None => message_views(record, message_budget, 64),
        };
        if let Some(job) = job
            && !job.output.is_empty()
            && let Some(message) = record.messages.iter().rev().find(|message| {
                message.role == MessageRole::Assistant
                    && message.request == Some(job.id)
                    && message.status == MessageStatus::Pending
            })
        {
            let anchor = message.response.unwrap_or(message.id);
            let committed: Vec<&ConversationMessage> = record
                .messages
                .iter()
                .filter(|message| {
                    message.role == MessageRole::Assistant
                        && message.response.unwrap_or(message.id) == anchor
                        && message.status != MessageStatus::Pending
                })
                .collect();
            if let Some(view) = messages
                .iter_mut()
                .find(|view| view.id == reply_id(&record.id, anchor))
            {
                *view = response_view(
                    &record.id,
                    anchor,
                    &committed,
                    Some((&job.output, job.status == JobStatus::Running)),
                );
            }
        }
        if job.is_some_and(|job| job.status == JobStatus::AwaitingQuestion) {
            for message in messages.iter_mut().filter(|message| message.streaming) {
                message.status = "Awaiting your answer";
                message.streaming = false;
            }
        } else if pending_gate.is_some()
            || job.is_some_and(|job| job.status == JobStatus::AwaitingDecision)
        {
            for message in messages.iter_mut().filter(|message| message.streaming) {
                message.status = "Awaiting your decision";
                message.streaming = false;
            }
        }
        if retry.is_some() {
            for message in messages.iter_mut().filter(|message| message.streaming) {
                message.status = "Retrying the provider";
            }
        }
        let retained = transcript.map_or(record.messages.len(), |window| window.total);
        let omitted_messages = retained.saturating_sub(rendered);
        let latest_href = format!("/conversations/{}", record.id.as_hex());
        let leaf_query = inspection_leaf
            .map(|leaf| format!("&leaf={}", leaf.as_hex()))
            .unwrap_or_default();
        let historical = transcript.is_some_and(|window| !window.live);
        let window_nav =
            historical || transcript.is_some_and(|window| window.has_before || window.has_after);
        let (earlier_href, later_href) = match transcript {
            Some(window) => (
                window
                    .has_before
                    .then(|| {
                        window
                            .before_anchor
                            .map(|id| format!("{latest_href}?before={}{leaf_query}", id.as_hex()))
                    })
                    .flatten()
                    .unwrap_or_default(),
                window
                    .has_after
                    .then(|| {
                        window
                            .after_anchor
                            .map(|id| format!("{latest_href}?after={}{leaf_query}", id.as_hex()))
                    })
                    .flatten()
                    .unwrap_or_default(),
            ),
            None => (String::new(), String::new()),
        };
        // A historical window observes status only. The pending reply belongs
        // to the live view, which the user reaches through Latest.
        let history_status =
            if historical && job.is_some_and(|job| job.status == JobStatus::Running) {
                "Work is in progress. New output will appear at the latest view.".to_owned()
            } else {
                String::new()
            };
        Self {
            show_thinking: sources.preferences.show_thinking(),
            thinking_visibility_error: None,
            heading: record.title.clone(),
            document_title: format!("{} | Power Plant", record.title),
            title: title.to_owned(),
            error,
            messages,
            companion_html: String::new(),
            companion_kind: "activity",
            companion_title: String::new(),
            notice: "",

            omitted_messages,
            window_entries: transcript.map_or_else(Vec::new, |window| {
                window
                    .messages
                    .iter()
                    .map(|message| (message.id.as_hex(), message_id(&record.id, message)))
                    .collect()
            }),
            inspection_leaf_query: leaf_query.clone(),
            earlier_href,
            later_href,
            latest_href,
            historical,
            window_nav,
            history_status,
            model_available: selection.is_some(),
            model_picker,
            presets,
            preset_source: configuration
                .and_then(|c| {
                    c.preset.as_ref().map(|source| {
                        let status = sources.presets.iter().find(|p| p.id == source.id).map_or(
                            "Source unavailable; independent local settings",
                            |p| {
                                if p.revision != source.revision {
                                    "Source changed; independent local settings"
                                } else if p.settings != c.settings {
                                    "Locally customised"
                                } else {
                                    "Independent copy"
                                }
                            },
                        );
                        format!(
                            "{} · {} · source revision {}",
                            source.name, status, source.revision
                        )
                    })
                })
                .unwrap_or_default(),
            preset_name: configuration
                .map(|configuration| crate::presets::suggested_name(&configuration.settings))
                .unwrap_or_else(|| "Untitled preset".to_owned()),
            preset_preview: None,
            preset_setup: setup_rows(
                configuration.map(|model| &model.settings),
                sources.environments,
            ),
            preset_save_open: false,
            directories: configuration
                .map(|configuration| directory_views(&configuration.settings.directories))
                .unwrap_or_default(),
            data_root: String::new(),
            consent_path: String::new(),
            consent_request: String::new(),
            pending_directory: String::new(),
            consent_existing: false,
            consent_reviewed: false,
            consent_direct: false,
            consent_sensitive: false,
            draft_nonce: String::new(),
            prepared_run: String::new(),
            fork_source: String::new(),
            handoff_settings: String::new(),
            handoff_approved: false,
            prepared_recovery: None,
            draft_preset_reference: String::new(),
            consent_reference: String::new(),
            instructions: configuration
                .map(|configuration| configuration.settings.instructions.clone())
                .unwrap_or_default(),
            instruction_sources: None,
            instruction_sources_error: false,
            tool_options: tool_options(
                &configuration
                    .map(|configuration| {
                        configuration
                            .settings
                            .tools
                            .iter()
                            .map(|tool| tool.as_str().to_owned())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            ),
            environment_options: environment_options(
                sources.environments,
                sources.environment_snapshots,
                selected_environment,
            ),
            environment_summary: environment_summary(sources.environments, selected_environment),
            execution_switch: None,
            network_options: network_options.clone(),
            network_restricted: effective_network.as_str() == "restricted",
            network_domains: network_domains.clone(),
            network_summary,
            location_host,
            host_summary: host_access_summary(location_host, host_approval_automatic),
            host_user: format!(
                "{} (uid {}, effective uid {})",
                host_identity.username, host_identity.uid, host_identity.euid
            ),
            host_elevated: host_identity.elevated(),
            host_approval_automatic,
            host_pending_approval: false,
            host_consent_request: String::new(),
            pending_host_command: None,
            pending_command: None,
            pending_question: None,
            continuation: record
                .continuation
                .as_ref()
                .filter(|_| !job_active)
                .map(super::continuation::ContinuationView::from_checkpoint),
            observe_active,
            settings_open: false,
            directories_open: false,
            job_active,
            retry,
            context: if transcript.is_some_and(|window| window.total != record.messages.len())
                && job.is_none_or(|job| job.context.is_none())
            {
                None
            } else {
                context_view(record, sources.models, job)
            },
            can_compact: !job_active
                && pending_gate.is_none()
                && record.continuation.is_none()
                && (transcript.is_some_and(|window| window.total > record.messages.len())
                    || crate::conversations::compaction::has_boundary(
                        &record.messages,
                        record.compaction.as_ref(),
                    )),
            summary_usage: usage_panel(
                &record.summary_requests,
                &format!("/conversations/{}/context/", record.id.as_hex()),
            ),
            compaction: record
                .compaction
                .as_ref()
                .map(super::compaction::CompactionView::from_record),
            queue: super::queue::QueueView::from_queue(
                &record.queue,
                job_active || pending_gate.is_some() || record.continuation.is_some(),
                job.is_some_and(|job| job.status == JobStatus::Running),
            ),
            attachments,
            fork: None,
            revision: None,
            state: ConversationPageState::Saved(Box::new(SavedConversationState {
                id: record.id.as_hex(),
                revision: record.revision.to_string(),
                job_id,
                cursor,
                pending_gate,

                source_candidate_review,
                linked_candidate_reviews,
                workflow_progress: None,
                direct_results: Vec::new(),
            })),
        }
    }

    pub(super) fn with_instruction_sources(
        mut self,
        state: &crate::state::AppState,
        record: &ConversationRecord,
    ) -> Self {
        match state.conversations.latest_reply_request(&record.id) {
            Ok(request) => {
                self.instruction_sources = request.map(|request| {
                    let paths: Vec<_> = request
                        .sources
                        .iter()
                        .filter(|source| source.kind == crate::execution::ResourceKind::Instruction)
                        .map(|source| source.path.clone())
                        .collect();
                    InstructionSourcesView {
                        truncated: paths.len() > 8,
                        paths: paths.into_iter().take(8).collect(),
                        href: format!(
                            "/conversations/{}/context/{}",
                            record.id,
                            request.id.as_hex()
                        ),
                    }
                });
            }
            Err(_) => self.instruction_sources_error = true,
        }
        self
    }

    pub(super) fn with_revision(mut self, draft: RevisionDraft) -> Self {
        self.revision = Some(draft);
        self
    }

    pub(super) fn with_access_status(
        mut self,
        state: &crate::state::AppState,
        session: crate::sessions::SessionId,
        record: &ConversationRecord,
    ) -> Self {
        if let Some(pause) = &mut self.continuation
            && let Some(run) = record
                .continuation
                .as_ref()
                .and_then(|checkpoint| checkpoint.run)
                .and_then(|id| state.workflow_runs.get(&id))
        {
            pause.settings_summary = super::handoff::transfer::settings_text(&run);
        }
        self.data_root = state.local_data.root().to_string_lossy().into_owned();
        if let Some(configuration) = &record.model {
            for (view, grant) in self
                .directories
                .iter_mut()
                .zip(&configuration.settings.directories)
            {
                view.sensitive = crate::execution::authority::sensitive_directory(
                    &grant.host_path,
                    state.local_data.root(),
                );
                view.exclusions = crate::workflows::workspace::reviewed_capture_exclusions(
                    &grant.host_path,
                    state.local_data.root(),
                );
                view.pending_approval = grant.requires_access_consent(state.local_data.root())
                    && !state.access_consent.authorised_conversation(
                        session,
                        record.id,
                        &configuration.settings,
                        grant,
                    )
                    && !state.conversations.directory_approved(
                        &record.id,
                        &configuration.settings,
                        grant,
                    );
            }
            self.location_host =
                configuration.settings.location == crate::execution::ToolLocation::Host;
            self.host_approval_automatic = configuration.settings.host_approval.automatic();
            self.host_summary =
                host_access_summary(self.location_host, self.host_approval_automatic);
            self.host_pending_approval = configuration.settings.host_tools()
                && !state.access_consent.authorised_host_conversation(
                    session,
                    record.id,
                    &configuration.settings,
                );
        }
        self
    }

    pub(super) fn with_host_consent_request(mut self, request: String) -> Self {
        self.host_consent_request = request;
        self.settings_open = true;
        self
    }

    pub(super) fn with_pending_host_command(
        mut self,
        command: Option<crate::execution::HostCommandRequest>,
        approval_policy: &str,
    ) -> Self {
        self.pending_host_command = command.map(|command| HostCommandView {
            request: command.token,
            job: command.job.as_hex(),
            revision: command.execution_revision.to_string(),
            command_input: serde_json::to_string(&command.command).unwrap_or_default(),
            command: command.command,
            directory: command.directory.display().to_string(),
            explanation: if command.explanation.is_empty() {
                "The model did not explain this command.".to_owned()
            } else {
                command.explanation
            },
            approval_policy: approval_policy.to_owned(),
        });
        self
    }

    pub(super) fn with_pending_command(mut self, command: Option<PendingCommandView>) -> Self {
        self.pending_command = command;
        self
    }

    pub(super) fn with_pending_question(
        mut self,
        question: Option<crate::conversations::PendingQuestion>,
    ) -> Self {
        self.pending_question = question.map(super::questions::QuestionView::from_question);
        self
    }

    pub(super) fn with_pending_directory(
        mut self,
        state: &crate::state::AppState,
        grant: crate::execution::DirectoryGrant,
        request: String,
        existing: bool,
    ) -> Self {
        if existing {
            if let Some(view) = self
                .directories
                .iter_mut()
                .find(|view| view.id == grant.id.as_hex())
            {
                view.sensitive = crate::execution::authority::sensitive_directory(
                    &grant.host_path,
                    state.local_data.root(),
                );
                view.pending_approval = true;
                view.review_before_apply =
                    grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply;
                view.direct_write = grant.access == crate::execution::DirectoryAccess::DirectWrite;
                view.access_label =
                    crate::slices::execution_settings::page::directory_access_label(grant.access);
                view.form_value = grant.form_value();
                view.exclusions = crate::workflows::workspace::reviewed_capture_exclusions(
                    &grant.host_path,
                    state.local_data.root(),
                );
            }
        } else {
            let mut view = directory_view(&grant);
            view.sensitive = crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            );
            view.pending_approval = true;
            view.transient = true;
            self.directories.push(view);
        }
        self.data_root = state.local_data.root().to_string_lossy().into_owned();
        self.consent_path = grant.host_path.to_string_lossy().into_owned();
        self.consent_request = request;
        self.pending_directory = grant.form_value();
        self.consent_existing = existing;
        self.consent_reviewed =
            grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply;
        self.consent_direct = grant.access == crate::execution::DirectoryAccess::DirectWrite;
        self.consent_sensitive = crate::execution::authority::sensitive_directory(
            &grant.host_path,
            state.local_data.root(),
        );
        self
    }

    pub(super) fn open_settings(mut self) -> Self {
        self.settings_open = true;
        self
    }

    pub(super) fn open_directories(mut self) -> Self {
        self.directories_open = true;
        self
    }

    pub(super) fn with_settings_fields(
        mut self,
        state: &crate::state::AppState,
        fields: SubmittedSettingsFields<'_>,
    ) -> Self {
        self.model_picker = ModelPicker::new(
            &state.vault,
            &state.preferences,
            &state.models_dev,
            fields.provider,
            fields.model,
            fields.thinking,
        );
        self.instructions = fields.instructions;
        self.tool_options = tool_options(&fields.tools);
        let selected_environment = EnvironmentId::parse(fields.environment);
        self.environment_options = environment_options(
            &state.environments,
            &state.environment_snapshots,
            selected_environment,
        );
        if self.saved().is_none() {
            self.environment_summary =
                environment_summary(&state.environments, selected_environment);
        }
        self.network_options = network_options(fields.network);
        self.network_restricted = network_restricted(fields.network);
        self.network_domains = fields.network_domains.to_owned();
        if self.saved().is_none() {
            self.network_summary = network_summary_from_form(fields.network);
            self.location_host = fields.location == crate::execution::ToolLocation::Host.as_str();
            self.host_approval_automatic = crate::execution::HostApprovalPolicy::parse(
                if fields.host_approval.trim().is_empty() {
                    "ask-each-time"
                } else {
                    fields.host_approval.trim()
                },
            )
            .is_some_and(crate::execution::HostApprovalPolicy::automatic);
        }
        self.settings_open = true;
        self
    }

    pub(super) fn with_execution_switch(
        mut self,
        state: &crate::state::AppState,
        current: &crate::execution::ExecutionSettings,
        replacement: &crate::execution::ExecutionSettings,
        directory_access: &str,
        gate: Option<&PendingCodeGateView>,
    ) -> Self {
        let location = replacement.location;
        let host_approval = replacement.host_approval;
        let environment = replacement.environment;
        let current_environment = state.environments.get(&current.environment).map_or_else(
            || "Current environment unavailable".to_owned(),
            |item| item.name,
        );
        let requested_environment_name = state.environments.get(&environment).map_or_else(
            || "Requested environment unavailable".to_owned(),
            |item| item.name,
        );
        let switch_gate = gate.map(|gate| EnvironmentSwitchGateView {
            run_id: gate.run_id.clone(),
            gate_id: gate.gate_id.clone(),
            revision: gate.revision.clone(),
            candidate: gate.candidate.clone(),
            review_href: gate.diff_href.clone(),
        });
        self.execution_switch = Some(ExecutionSwitchView {
            requested_location: location.as_str().to_owned(),
            requested_host_approval: host_approval.as_str().to_owned(),
            requested_environment: environment.as_hex(),
            directory_access: directory_access.to_owned(),
            requested_network: replacement.network.as_str().to_owned(),
            requested_network_domains: replacement.network.domains().join("\n"),
            rows: execution_change_rows(
                current,
                replacement,
                current_environment,
                requested_environment_name,
            ),
            access_lines: execution_access_lines(state, current, replacement, location),
            host_effects_remain: current.location == crate::execution::ToolLocation::Host
                || current
                    .directories
                    .iter()
                    .any(|grant| grant.access == crate::execution::DirectoryAccess::DirectWrite),
            needs_new_consent: execution_switch_needs_consent(state, replacement, location),
            active_job: self.job_active,
            job_id: self
                .saved()
                .map_or_else(String::new, |saved| saved.job_id.clone()),
            gate: switch_gate,
        });
        self.settings_open = true;
        self
    }

    pub(super) fn with_prepared_recovery(
        mut self,
        state: &crate::state::AppState,
        session: crate::sessions::SessionId,
        record: &ConversationRecord,
    ) -> Self {
        self.prepared_recovery =
            super::handoff::recovery::RecoveryView::for_conversation(state, session, record);
        self
    }

    pub(super) fn with_direct_results(
        mut self,
        state: &crate::state::AppState,
        conversation: crate::conversations::ConversationId,
    ) -> Self {
        if let ConversationPageState::Saved(saved) = &mut self.state
            && let Some(run) = state.workflow_runs.for_conversation(&conversation).first()
        {
            saved.direct_results = run
                .attempts
                .iter()
                .rev()
                .filter(|attempt| attempt.direct_changes.is_some())
                .take(4)
                .map(|attempt| {
                    crate::slices::workflow_runs::DirectChangesView::new(run, attempt, state, 0)
                })
                .collect();
        }
        self
    }

    pub(super) fn with_queue_return(
        mut self,
        item: crate::conversations::QueueItemId,
        message: &'static str,
    ) -> Self {
        self.queue = self.queue.with_return(item, message);
        self
    }

    pub(super) fn with_workflow_progress(
        mut self,
        workflow_progress: Option<WorkflowProgressView>,
    ) -> Self {
        if let ConversationPageState::Saved(saved) = &mut self.state {
            saved.workflow_progress = workflow_progress;
        }
        self
    }

    pub(super) fn contents(&self) -> impl Template + '_ {
        self.as_conversation_detail()
    }
}

fn backend_label(location: crate::execution::ToolLocation) -> &'static str {
    match location {
        crate::execution::ToolLocation::Sandbox => "Sandbox",
        crate::execution::ToolLocation::Host => "This computer",
    }
}

fn execution_change_rows(
    current: &crate::execution::ExecutionSettings,
    replacement: &crate::execution::ExecutionSettings,
    current_environment: String,
    requested_environment: String,
) -> Vec<ExecutionChangeRow> {
    let mut rows = Vec::new();
    let mut row = |label: String, current: String, requested: String| {
        rows.push(ExecutionChangeRow {
            label,
            changed: current != requested,
            current,
            requested,
        });
    };
    row(
        "Location".into(),
        backend_label(current.location).into(),
        backend_label(replacement.location).into(),
    );
    row(
        "Environment".into(),
        current_environment,
        requested_environment,
    );
    for (before, after) in current.directories.iter().zip(&replacement.directories) {
        let access = crate::slices::execution_settings::page::directory_access_label;
        row(
            format!("{} · sandbox access", before.alias),
            access(before.access).into(),
            access(after.access).into(),
        );
    }
    let network = |settings: &crate::execution::ExecutionSettings| {
        let label = network_summary_from_form(settings.network.as_str());
        if settings.network.domains().is_empty() {
            label
        } else {
            format!("{label}: {}", settings.network.domains().join(", "))
        }
    };
    row(
        "Sandbox network".into(),
        network(current),
        network(replacement),
    );
    if current.host_tools() || replacement.host_tools() {
        let approval = crate::slices::execution_settings::page::host_approval_label;
        row(
            "Host commands".into(),
            approval(current.host_approval).into(),
            approval(replacement.host_approval).into(),
        );
    }
    rows
}

fn execution_access_lines(
    state: &crate::state::AppState,
    current: &crate::execution::ExecutionSettings,
    replacement: &crate::execution::ExecutionSettings,
    requested: crate::execution::ToolLocation,
) -> Vec<String> {
    if replacement.directories.is_empty() {
        return vec![if requested == crate::execution::ToolLocation::Host {
            "No work locations. Commands start in Power Plant's current directory. These paths do not confine host access.".to_owned()
        } else {
            "No host directory access. Tools use private scratch storage at /workspace.".to_owned()
        }];
    }
    replacement
        .directories
        .iter()
        .map(|grant| {
            let label = crate::slices::execution_settings::page::directory_access_label;
            let access = current.directories.iter().find(|old| old.id == grant.id)
                .filter(|old| old.access != grant.access)
                .map_or_else(|| label(grant.access).to_owned(), |old| format!("{} to {}", label(old.access), label(grant.access)));
            let effects = match grant.access {
                crate::execution::DirectoryAccess::DirectWrite => " Immediate host writes need no candidate approval. These writes can alter or corrupt live configuration and execution evidence.",
                crate::execution::DirectoryAccess::ReviewBeforeApply => " Tools use an isolated copy. File application needs approval.",
                crate::execution::DirectoryAccess::ReadOnly => "",
            };
            let sensitive = crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            );
            let path = grant.host_path.display();
            if requested == crate::execution::ToolLocation::Host {
                if sensitive {
                    format!(
                        "{path} · Work location. Sandbox strategy: {access}. This path contains sensitive Power Plant data."
                    )
                } else {
                    format!("{path} · Work location. Sandbox strategy: {access}.")
                }
            } else if sensitive {
                format!("{path} · {access}.{effects} This path can expose credentials and private conversations, even with Network off.")
            } else {
                format!("{path} · {access}.{effects}")
            }
        })
        .collect()
}

fn execution_switch_needs_consent(
    state: &crate::state::AppState,
    current: &crate::execution::ExecutionSettings,
    location: crate::execution::ToolLocation,
) -> bool {
    if location == crate::execution::ToolLocation::Host {
        return true;
    }
    current
        .directories
        .iter()
        .any(|grant| grant.requires_access_consent(state.local_data.root()))
}

fn host_access_summary(location_host: bool, automatic: bool) -> String {
    if !location_host {
        String::new()
    } else {
        format!(
            "Unrestricted host access · {}",
            crate::slices::execution_settings::page::host_approval_label(if automatic {
                crate::execution::HostApprovalPolicy::Automatic
            } else {
                crate::execution::HostApprovalPolicy::AskEachTime
            })
        )
    }
}

fn preset_summary(settings: &crate::execution::ExecutionSettings) -> String {
    let directory_count = settings.directories.len();
    format!(
        "{} · {} · {} tool{} · {} director{}",
        settings.model.provider.label(),
        settings.model.model,
        settings.tools.len(),
        if settings.tools.len() == 1 { "" } else { "s" },
        directory_count,
        if directory_count == 1 { "y" } else { "ies" },
    )
}

fn directory_views(grants: &[crate::execution::DirectoryGrant]) -> Vec<DirectoryView> {
    let mut views = grants.iter().map(directory_view).collect::<Vec<_>>();
    let names = views
        .iter()
        .map(|view| view.name.clone())
        .collect::<Vec<_>>();
    for (index, view) in views.iter_mut().enumerate() {
        if names
            .iter()
            .enumerate()
            .any(|(other, name)| other != index && name == &names[index])
        {
            view.name = format!("{} ({})", view.name, view.alias);
        }
    }
    views
}

fn directory_view(grant: &crate::execution::DirectoryGrant) -> DirectoryView {
    DirectoryView {
        id: grant.id.as_hex(),
        name: grant
            .host_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| grant.host_path.to_string_lossy().into_owned()),
        alias: grant.alias.clone(),
        host_path: grant.host_path.to_string_lossy().into_owned(),
        guest_path: grant.guest_path(),
        form_value: grant.form_value(),
        available: grant.is_available(),
        sensitive: false,
        pending_approval: false,
        transient: false,
        review_before_apply: grant.access == crate::execution::DirectoryAccess::ReviewBeforeApply,
        direct_write: grant.access == crate::execution::DirectoryAccess::DirectWrite,
        access_label: crate::slices::execution_settings::page::directory_access_label(grant.access),
        exclusions: Vec::new(),
    }
}

fn environment_summary(
    catalogue: &EnvironmentCatalogue,
    selected: Option<EnvironmentId>,
) -> String {
    let Some(selected) = selected else {
        return "Choose environment".to_owned();
    };
    let Some(record) = catalogue.get(&selected) else {
        return "Environment unavailable".to_owned();
    };
    record.name
}

fn network_options(selected: &str) -> Vec<NetworkOption> {
    vec![
        NetworkOption {
            value: "none",
            label: "Off",
            selected: selected.is_empty() || selected == "none",
        },
        NetworkOption {
            value: "restricted",
            label: "Restricted domains",
            selected: selected == "restricted",
        },
        NetworkOption {
            value: "public",
            label: "Public internet",
            selected: selected == "public",
        },
    ]
}

fn network_restricted(selected: &str) -> bool {
    selected == "restricted"
}

fn network_summary_from_form(network: &str) -> String {
    match network {
        "restricted" => "Restricted domains".to_owned(),
        "public" => "Public internet".to_owned(),
        _ => "Off".to_owned(),
    }
}

pub(super) fn apply_outcomes(run: &WorkflowRun) -> Vec<ApplyOutcomeView> {
    run.latest_apply_attempt()
        .and_then(|attempt| {
            attempt
                .apply_transaction
                .as_ref()
                .map(|transaction| (attempt, transaction))
        })
        .map(|(_, transaction)| {
            transaction
                .roots
                .iter()
                .map(|root| ApplyOutcomeView {
                    directory: root.alias.clone(),
                    path: root.host_path.display().to_string(),
                    outcome: match root.outcome {
                        crate::workflows::apply::ApplyRootOutcome::Pending => "Pending",
                        crate::workflows::apply::ApplyRootOutcome::Unchanged => "Unchanged",
                        crate::workflows::apply::ApplyRootOutcome::Applied => "Applied",
                        crate::workflows::apply::ApplyRootOutcome::Conflicted => "Conflicted",
                        crate::workflows::apply::ApplyRootOutcome::Uncertain => "Uncertain",
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn apply_attempt_presentation(run: &WorkflowRun) -> (String, String, &'static str, String) {
    match run.latest_apply_attempt() {
        Some(attempt) => {
            let state = attempt
                .apply_transaction
                .as_ref()
                .map(|transaction| match transaction.state {
                    crate::workflows::apply::ApplyTransactionState::Prepared => "prepared",
                    crate::workflows::apply::ApplyTransactionState::Applying { .. } => "applying",
                    crate::workflows::apply::ApplyTransactionState::Applied { .. } => "applied",
                    crate::workflows::apply::ApplyTransactionState::Verified => "verified",
                    crate::workflows::apply::ApplyTransactionState::Recovered => "recovered",
                    crate::workflows::apply::ApplyTransactionState::RecoveryUncertain => {
                        "recovery-uncertain"
                    }
                })
                .unwrap_or("");
            (
                run.id.as_hex(),
                attempt.id.as_hex(),
                state,
                format!(
                    "/runs/{}/attempts/{}/changes",
                    run.id.as_hex(),
                    attempt.id.as_hex()
                ),
            )
        }
        None => (String::new(), String::new(), "", String::new()),
    }
}

pub(super) fn workflow_progress(run: &WorkflowRun) -> WorkflowProgressView {
    let (apply_run_id, apply_attempt_id, apply_state, apply_resolve_href) =
        apply_attempt_presentation(run);
    WorkflowProgressView {
        run_href: format!("/runs/{}", run.id.as_hex()),
        name: run.pinned.definition.name().to_owned(),
        state: run.state.as_label(),
        current_step: run
            .current_step_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "Finished".to_owned()),
        result: workflow_result_label(&run.state),
        task_progress: String::new(),

        conversation_id: run
            .conversation_id
            .map(|id| id.as_hex())
            .unwrap_or_default(),
        apply_outcomes: apply_outcomes(run),
        apply_run_id,
        apply_attempt_id,
        apply_state,
        apply_resolve_href,
        apply_partial: run.apply_is_known_partial(),
        apply_uncertain: run.apply_is_uncertain(),
        apply_complete: run.apply_is_complete(),
        settlement_eligible: run.partial_settlement_eligible() && run.conversation_id.is_some(),
        run_terminal: run.is_terminal(),
    }
}

fn activity_source_lines(
    state: &crate::state::AppState,
    conversation_id: &crate::conversations::ConversationId,
) -> (String, String, String) {
    let latest_run = state
        .workflow_runs
        .for_conversation(conversation_id)
        .into_iter()
        .next();
    let (brief, task, environments) = match latest_run {
        Some(run) => (
            run.launch_brief.clone(),
            String::new(),
            run.environments.clone(),
        ),
        _ => return (String::new(), String::new(), String::new()),
    };
    let environment_line = if environments.environments.is_empty() {
        "Host steps need no sandbox environment.".to_owned()
    } else {
        environments
            .environments
            .iter()
            .map(|environment| environment.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    (brief, task, environment_line)
}

fn workflow_result_label(state: &crate::workflows::run::RunState) -> &'static str {
    match state {
        crate::workflows::run::RunState::Completed => {
            "The terminal result and detailed worker evidence stay in the run record."
        }
        crate::workflows::run::RunState::Failed
        | crate::workflows::run::RunState::Escalated { .. } => {
            "The run stopped. Open the run record for its terminal result and retained evidence."
        }
        crate::workflows::run::RunState::Cancelled => {
            "The run was cancelled. Direct writes and host command effects remain. Earlier evidence stays in the run record."
        }
        _ => "Worker activity stays in the run record and does not enter this conversation.",
    }
}

pub(super) fn pending_code_gate(
    run: &WorkflowRun,
    store: &crate::workflows::WorkflowArtefactRepository,
    application_destination: String,
) -> Option<PendingCodeGateView> {
    let gate = run
        .gates
        .iter()
        .rev()
        .find(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)?;
    let diff = crate::workflows::artefacts::CandidateDiff::load(
        run,
        &gate.diff_base,
        &gate.candidate,
        store,
    )
    .ok()?;
    let (total_changes, changes) = diff.manifest_page(0, 16).ok()?;
    let changes_truncated = total_changes > changes.len();
    let mut preview_budget = 128 * 1024;
    Some(PendingCodeGateView {
        run_id: run.id.as_hex(),
        gate_id: gate.id.as_hex(),
        revision: run.decision_revision(gate).get().to_string(),
        candidate: diff.target.as_str().to_owned(),
        diff_base: diff.base.as_str().to_owned(),
        diff_href: format!("/runs/{}/gates/{}", run.id.as_hex(), gate.id.as_hex()),
        review_href: format!(
            "/conversations/candidate-review?run={}&candidate={}&diff_base={}",
            run.id.as_hex(),
            gate.candidate.id.as_hex(),
            gate.diff_base.id.as_hex()
        ),
        ordinary: diff.ordinary(),
        commit_on_approval: crate::slices::human_gates::approval_commits(run, gate),
        can_request_revision: run.human_revision_policy(&gate.step).is_some(),
        quick_task: run.kind == crate::workflows::RunKind::QuickTask,
        application_destination,
        exclusions: diff.exclusions().to_vec(),
        total_changes,
        changes_truncated,
        changes: changes
            .into_iter()
            .enumerate()
            .map(|(index, change)| {
                // Counts derive from the complete stored diff, not the bounded
                // preview below. Binary or oversized changes omit counts.
                let full = diff.change(index, store).ok();
                let (additions, removals, has_counts) = full
                    .as_ref()
                    .and_then(|change| change.text.as_ref())
                    .map(|fragments| {
                        let text: String = fragments
                            .iter()
                            .map(|fragment| fragment.text.as_str())
                            .collect();
                        let mut additions = 0;
                        let mut removals = 0;
                        for line in text.lines() {
                            if line.starts_with('+') && !line.starts_with("+++") {
                                additions += 1;
                            } else if line.starts_with('-') && !line.starts_with("---") {
                                removals += 1;
                            }
                        }
                        (additions, removals, true)
                    })
                    .unwrap_or((0, 0, false));
                let preview = full
                    .and_then(|change| {
                        let text: String = change
                            .text?
                            .into_iter()
                            .map(|fragment| fragment.text)
                            .collect();
                        let bytes = crate::markdown::escape_plain(&text).len();
                        if bytes > preview_budget {
                            return None;
                        }
                        preview_budget -= bytes;
                        Some(text)
                    })
                    .unwrap_or_else(|| {
                        "Open the full candidate diff for binary content or a larger preview."
                            .to_owned()
                    });
                let name = candidate_file_name(&change.path);
                CandidateChangeView {
                    path: change.path,
                    name,
                    directory: change.directory,
                    status: change.status,
                    preview,
                    additions,
                    removals,
                    has_counts,
                }
            })
            .collect(),
    })
}

fn candidate_file_name(path: &str) -> String {
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

// The transcript uses the durable message identity so projection never depends on position.
#[cfg(test)]
fn visible_messages(record: &ConversationRecord, byte_budget: usize) -> Vec<MessageView> {
    message_views(record, byte_budget, 64).0
}

/// One prefix of the transcript ordered newest first. Consecutive assistant
/// phases that share a logical-response anchor render as one entry with one
/// stable identity, so a live patch never targets an absent phase. The second
/// value counts the original entries represented by the returned views.
fn message_views(
    record: &ConversationRecord,
    byte_budget: usize,
    limit: usize,
) -> (Vec<MessageView>, usize) {
    let messages = &record.messages;
    let mut views: Vec<MessageView> = Vec::new();
    let mut bytes = 0usize;
    let mut rendered = 0usize;
    let mut end = messages.len();
    while end > 0 {
        let last = &messages[end - 1];
        let (start, grouped) = if last.role == MessageRole::Assistant {
            let anchor = last.response.unwrap_or(last.id);
            let mut start = end - 1;
            while start > 0 {
                let previous = &messages[start - 1];
                if previous.role == MessageRole::Assistant
                    && previous.response.unwrap_or(previous.id) == anchor
                {
                    start -= 1;
                } else {
                    break;
                }
            }
            (start, true)
        } else {
            (end - 1, false)
        };
        let view = if grouped {
            let phases: Vec<&ConversationMessage> = messages[start..end].iter().collect();
            let mut view = response_view(
                &record.id,
                phases[0].response.unwrap_or(phases[0].id),
                &phases,
                None,
            );
            let boundary = end - 1;
            if crate::conversations::forks::forkable(messages, boundary) {
                view.forkable = true;
                view.fork_href = format!(
                    "/conversations/{}/fork?message={}",
                    record.id.as_hex(),
                    messages[boundary].id.as_hex()
                );
            }
            view
        } else {
            message_view_in(&record.id, messages, start)
        };
        let cost = view.html.len()
            + view
                .command
                .as_ref()
                .map_or(0, |command| ammonia::clean_text(command).len())
            + view.copy.as_ref().map_or(0, |copy| copy.escaped_bytes)
            + 2048;
        if !views.is_empty() && (bytes + cost > byte_budget || views.len() >= limit) {
            break;
        }
        bytes += cost;
        rendered += end - start;
        views.push(view);
        end = start;
    }
    views.reverse();
    let rendered = rendered.min(messages.len());
    (views, rendered)
}

pub(super) fn message_id(conversation: &ConversationId, message: &ConversationMessage) -> String {
    reply_id(conversation, message.id)
}

pub(super) fn reply_id(conversation: &ConversationId, id: MessageId) -> String {
    format!(
        "conversation-{}-message-{}",
        conversation.as_hex(),
        id.as_hex()
    )
}

#[cfg(test)]
pub(super) fn message_view(
    conversation: &ConversationId,
    message: &ConversationMessage,
) -> MessageView {
    message_view_in(conversation, std::slice::from_ref(message), 0)
}

pub(super) fn message_view_in(
    conversation: &ConversationId,
    messages: &[ConversationMessage],
    index: usize,
) -> MessageView {
    let message = &messages[index];
    let user = message.role == MessageRole::User;
    let command_entry = message.role == MessageRole::Command;
    let streaming = message.status == MessageStatus::Pending;
    MessageView {
        id: message_id(conversation, message),
        user,
        is_command: command_entry,
        role_label: match message.role {
            MessageRole::User => "You",
            MessageRole::Assistant => "Power Plant",
            MessageRole::Command => "Command",
        }
        .to_owned(),
        excluded: command_entry
            && message
                .command
                .as_ref()
                .is_some_and(|entry| !entry.included),
        command: message.input.as_ref().map(|input| input.typed.clone()),
        // Copy belongs to a settled logical response. An intermediate phase,
        // an active phase, a user entry and a command entry never carry it.
        copy: if user || command_entry || streaming || !message.final_phase {
            None
        } else {
            let anchor = message.response.unwrap_or(message.id);
            copy_response_view(&crate::conversations::history::logical_response_text(
                messages, anchor,
            ))
        },
        html: if command_entry {
            command_entry_html(
                conversation,
                &message_id(conversation, message),
                message,
                streaming,
            )
        } else if user {
            let mut html = format!(
                "<p class=\"whitespace-pre-wrap\">{}</p>",
                ammonia::clean_text(&message.text)
            );
            for reference in &message.attachments {
                html.push_str(&format!(
                    "<a href=\"/conversations/{conversation}/attachments/{id}\" target=\"_blank\" rel=\"noopener\"><img src=\"/conversations/{conversation}/attachments/{id}\" alt=\"Attached image\" loading=\"lazy\" class=\"max-h-64 max-w-full object-contain\"></a>",
                    id = reference.id,
                ));
            }
            html
        } else {
            activity_html(
                conversation,
                &message_id(conversation, message),
                &message.text,
                &message.activity,
                &[],
                &message.requests,
                streaming,
                message
                    .completion
                    .is_some_and(crate::providers::CompletionReason::incomplete),
            )
        },
        status: if command_entry {
            match message.status {
                MessageStatus::Complete => "",
                MessageStatus::Pending => "Running",
                MessageStatus::Interrupted => "Interrupted",
                MessageStatus::Failed => "Failed",
            }
        } else {
            match message.status {
                MessageStatus::Complete => "",
                MessageStatus::Pending => reply_status(&message.activity),
                MessageStatus::Interrupted => "Interrupted",
                MessageStatus::Failed => message
                    .completion
                    .and_then(crate::providers::CompletionReason::status_label)
                    .unwrap_or("Failed"),
            }
        },
        error: message_error(message),
        streaming: message.status == MessageStatus::Pending,
        forkable: false,
        fork_href: String::new(),
        revisable: message.role == MessageRole::User,
        revise_href: if message.role == MessageRole::User {
            format!(
                "/conversations/{}?revise={}",
                conversation.as_hex(),
                message.id.as_hex()
            )
        } else {
            String::new()
        },
    }
}

/// One view for a whole logical response. Committed phases and an optional
/// live reply concatenate in order under the anchor identity, so a phase
/// transition never targets an absent element and never duplicates text.
pub(super) fn response_view(
    conversation: &ConversationId,
    anchor: MessageId,
    phases: &[&ConversationMessage],
    live: Option<(&crate::providers::AssistantReply, bool)>,
) -> MessageView {
    let id = reply_id(conversation, anchor);
    let mut html = String::new();
    for message in phases {
        let phase_id = message_id(conversation, message);
        let content = activity_html(
            conversation,
            &message_id(conversation, message),
            &message.text,
            &message.activity,
            &[],
            &message.requests,
            false,
            message
                .completion
                .is_some_and(crate::providers::CompletionReason::incomplete),
        );
        // A long response still uses a bounded transport window. Each omitted
        // phase remains available through its canonical entry presentation.
        let content = if phases.len() > 1 && html.len() + content.len() > 192 * 1024 {
            format!(
                "<a data-graft href=\"/conversations/{}?around={}&amp;leaf={}\">View response phase</a>",
                conversation.as_hex(),
                message.id.as_hex(),
                message.id.as_hex(),
            )
        } else {
            content
        };
        if message.id != anchor {
            html.push_str(&format!("<div id=\"{phase_id}\">{content}</div>"));
        } else {
            html.push_str(&content);
        }
    }
    let mut streaming = false;
    let mut status = "";
    let mut error = String::new();
    if let Some((reply, live_streaming)) = live {
        // A distinct live id keeps inner streaming anchors unique when the
        // committed first phase shares the logical-response anchor.
        let live_id = format!("{id}-live");
        // The job snapshot can precede the phase commit. Request identities
        // distinguish that stale snapshot from a fresh pending phase.
        let committed = reply.usage.last().is_some_and(|request| {
            phases
                .iter()
                .any(|phase| phase.requests.iter().any(|stored| stored.id == request.id))
        });
        if !committed {
            html.push_str(&activity_html(
                conversation,
                &live_id,
                &reply.text,
                &reply.activity,
                &reply.progress,
                &reply.usage,
                live_streaming,
                reply
                    .completion
                    .is_some_and(crate::providers::CompletionReason::incomplete),
            ));
        }
        streaming = live_streaming;
        status = if live_streaming {
            reply_status(&reply.activity)
        } else {
            reply
                .completion
                .and_then(crate::providers::CompletionReason::status_label)
                .unwrap_or("")
        };
    } else if let Some(last) = phases.last() {
        streaming = last.status == MessageStatus::Pending;
        status = match last.status {
            MessageStatus::Complete => "",
            MessageStatus::Pending => reply_status(&last.activity),
            MessageStatus::Interrupted => "Interrupted",
            MessageStatus::Failed => last
                .completion
                .and_then(crate::providers::CompletionReason::status_label)
                .unwrap_or("Failed"),
        };
        error = message_error(last);
    }
    let settled = phases.last().filter(|message| {
        message.final_phase
            && live.is_none()
            && phases.first().is_some_and(|first| first.id == anchor)
    });
    let copy = settled.and_then(|_| {
        let text = phases
            .iter()
            .map(|message| crate::conversations::history::response_text(message))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (html_escaped_len(&text) <= 256 * 1024)
            .then(|| copy_response_view(&text))
            .flatten()
    });
    MessageView {
        id,
        user: false,
        is_command: false,
        role_label: "Power Plant".to_owned(),
        excluded: false,
        command: None,
        copy,
        html,
        status,
        error,
        streaming,
        forkable: false,
        fork_href: String::new(),
        revisable: false,
        revise_href: String::new(),
    }
}

fn retry_status_message(attempt: u32, delay: std::time::Duration) -> String {
    let wait = delay.as_secs();
    if wait == 0 {
        format!("Retrying the provider (attempt {attempt}). Stop remains available.")
    } else {
        format!("Retrying the provider in {wait}s (attempt {attempt}). Stop remains available.")
    }
}

fn reply_status(activity: &[crate::providers::AssistantActivity]) -> &'static str {
    use crate::providers::AssistantActivity;
    match activity.last() {
        Some(AssistantActivity::Thinking(_)) => "Thinking",
        Some(AssistantActivity::ToolCall { result: None, .. }) => "Tool call in progress",
        Some(_) => "Replying",
        None => "Waiting for model",
    }
}

#[derive(Template)]
#[template(path = "conversations/templates/message_content.html")]
struct MessageContent<'a> {
    id: &'a str,
    output_base: &'a str,
    blocks: Vec<MessageBlock>,
    usage: Option<UsagePanelView>,
}

struct UsagePanelView {
    requests: Vec<RequestUsageView>,
    subtotal: String,
}

struct RequestUsageView {
    model: String,
    detail: String,
    cost: String,
    context_href: String,
}

struct MessageBlock {
    kind: &'static str,
    html: String,
    label: String,
    output: String,
    active: bool,
    command: Option<CommandView>,
    progress: Vec<ProgressChunkView>,
}

struct CommandView {
    chunks: Vec<CommandChunkView>,
    status: String,
    error: bool,
    reference: Option<String>,
}

struct CommandChunkView {
    stream: &'static str,
    stderr: bool,
    text: String,
}

struct ProgressChunkView {
    stream: &'static str,
    stderr: bool,
    text: String,
}

fn command_view(command: &crate::execution::CommandResult) -> CommandView {
    CommandView {
        chunks: command
            .chunks
            .iter()
            .map(|chunk| CommandChunkView {
                stream: chunk.stream.label(),
                stderr: chunk.stream.is_stderr(),
                text: chunk.text.clone(),
            })
            .collect(),
        status: command.status_text(),
        error: command.is_error(),
        reference: command.retained_reference().map(str::to_owned),
    }
}

/// Render one direct command entry. A settled command shows its output with
/// the retained-output link. A pending command shows the running state.
fn command_entry_html(
    conversation: &ConversationId,
    id: &str,
    message: &ConversationMessage,
    streaming: bool,
) -> String {
    let output_base = format!("/conversations/{}/output/", conversation.as_hex());
    let context_base = format!("/conversations/{}/context/", conversation.as_hex());
    let block = match message
        .command
        .as_ref()
        .and_then(|entry| entry.output.as_ref())
    {
        Some(output) => MessageBlock {
            kind: "tool",
            html: String::new(),
            label: "Command output".to_owned(),
            output: String::new(),
            active: false,
            command: Some(command_view(output)),
            progress: Vec::new(),
        },
        None => MessageBlock {
            kind: "progress",
            html: String::new(),
            label: String::new(),
            output: String::new(),
            active: streaming,
            command: None,
            progress: Vec::new(),
        },
    };
    let output = MessageContent {
        id,
        output_base: &output_base,
        blocks: vec![block],
        usage: usage_panel(&[], &context_base),
    }
    .render()
    .expect("command content template");
    let directory = message
        .command
        .as_ref()
        .map_or("", |entry| entry.directory.as_str());
    format!(
        "<pre class=\"whitespace-pre-wrap\"><code>$ {}</code></pre><p class=\"text-quiet text-xs\">Directory: {}</p>{output}",
        ammonia::clean_text(&message.text),
        ammonia::clean_text(directory),
    )
}

#[allow(clippy::too_many_arguments)]
fn activity_html(
    conversation: &ConversationId,
    id: &str,
    text: &str,
    activity: &[crate::providers::AssistantActivity],
    progress: &[crate::providers::ToolProgress],
    requests: &[crate::conversations::RequestUsage],
    streaming: bool,
    incomplete: bool,
) -> String {
    use crate::providers::AssistantActivity;
    let output_base = format!("/conversations/{}/output/", conversation.as_hex());
    let context_base = format!("/conversations/{}/context/", conversation.as_hex());
    let progress_blocks: Vec<MessageBlock> = progress
        .iter()
        .map(|progress| MessageBlock {
            kind: "progress",
            html: String::new(),
            label: String::new(),
            output: String::new(),
            active: true,
            command: None,
            progress: vec![ProgressChunkView {
                stream: progress.stream.label(),
                stderr: progress.stream.is_stderr(),
                text: progress.text.clone(),
            }],
        })
        .collect();
    let blocks = if activity.is_empty() {
        vec![MessageBlock {
            kind: "response",
            html: reply_html(text),
            label: String::new(),
            output: String::new(),
            active: streaming,
            command: None,
            progress: Vec::new(),
        }]
    } else {
        activity
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let active = streaming && index + 1 == activity.len();
                let mut block = MessageBlock {
                    kind: "",
                    html: String::new(),
                    label: String::new(),
                    output: String::new(),
                    active,
                    command: None,
                    progress: Vec::new(),
                };
                match item {
                    AssistantActivity::Response(text) => {
                        block.kind = "response";
                        block.html = reply_html(text);
                    }
                    AssistantActivity::Thinking(text) => {
                        block.kind = "thinking";
                        block.html = reply_html(text);
                    }
                    AssistantActivity::Tool(tool)
                    | AssistantActivity::ToolCall {
                        result: Some(tool), ..
                    } => {
                        block.kind = "tool";
                        block.label = tool.label.clone();
                        block.output = tool.output.clone();
                        block.command = tool.command.as_ref().map(command_view);
                        block.active = false;
                    }
                    AssistantActivity::ToolCall {
                        name, result: None, ..
                    } => {
                        block.kind = "tool-call";
                        block.label = name.clone();
                        block.active = streaming;
                    }
                }
                block
            })
            .collect()
    };
    let mut blocks = blocks;
    blocks.extend(progress_blocks);
    if incomplete && !streaming {
        blocks.push(MessageBlock {
            kind: "incomplete",
            html: String::new(),
            label: String::new(),
            output: String::new(),
            active: false,
            command: None,
            progress: Vec::new(),
        });
    }
    MessageContent {
        id,
        output_base: &output_base,
        blocks,
        usage: usage_panel(requests, &context_base),
    }
    .render()
    .expect("message content template")
}

fn context_view(
    record: &ConversationRecord,
    models: &ModelsDevCatalogue,
    job: Option<&JobSnapshot>,
) -> Option<ContextView> {
    if let Some(estimate) = job.and_then(|job| job.context) {
        return Some(ContextView {
            label: estimate.label(),
            unknown_capacity: estimate.unknown_capacity(),
            approximate: estimate.approximate(),
            partial: false,
            measured: estimate.measured,
            compacting: job.is_some_and(|job| job.compacting),
        });
    }
    let model = record.model.as_ref()?;
    let selection = &model.settings.model;
    let turns = crate::conversations::compaction::project(
        &record.messages,
        Some(selection),
        record.compaction.as_ref(),
    )
    .ok()?;
    let tools = crate::tools::definitions_for(&model.settings.tools, model.settings.location);
    let request = crate::execution::ContextRequest {
        preamble: &model.settings.instructions,
        tools: &tools,
        turns: &turns,
        extra: &[],
        provider: selection.provider,
        model: &selection.model,
        output_limit: models.output_limit(selection.provider, &selection.model),
    };
    let estimate = crate::execution::context::inspect(
        request,
        models.context_limit(selection.provider, &selection.model),
    )
    .ok()?;
    Some(ContextView {
        label: estimate.label(),
        unknown_capacity: estimate.unknown_capacity(),
        approximate: estimate.approximate(),
        // Idle estimates omit the runtime project instructions and discovered
        // tools, so they are partial and not dispatch-equivalent.
        partial: true,
        measured: false,
        compacting: job.is_some_and(|job| job.compacting),
    })
}

fn usage_panel(
    requests: &[crate::conversations::RequestUsage],
    context_base: &str,
) -> Option<UsagePanelView> {
    if requests.is_empty() {
        return None;
    }
    let total = crate::conversations::history::total_cost(requests);
    Some(UsagePanelView {
        requests: requests
            .iter()
            .map(|request| request_usage_view(request, context_base))
            .collect(),
        subtotal: cost_label(&total),
    })
}

fn request_usage_view(
    request: &crate::conversations::RequestUsage,
    context_base: &str,
) -> RequestUsageView {
    RequestUsageView {
        model: format!(
            "{} · {}",
            request.usage.provider.label(),
            request.usage.model
        ),
        detail: token_detail(&request.usage),
        cost: request_cost_label(request),
        context_href: format!("{context_base}{}", request.id.as_hex()),
    }
}

fn token_detail(usage: &crate::providers::ModelUsage) -> String {
    let mut parts = Vec::new();
    if let Some(value) = usage.input_tokens {
        parts.push(format!("{} input", format_count(value)));
    }
    if let Some(value) = usage.output_tokens {
        parts.push(format!("{} output", format_count(value)));
    }
    if let Some(value) = usage.cache_read_tokens {
        parts.push(format!("{} cache read", format_count(value)));
    }
    if let Some(value) = usage.cache_creation_tokens {
        parts.push(format!("{} cache write", format_count(value)));
    }
    if parts.is_empty() {
        "Usage unknown".to_owned()
    } else {
        parts.join(", ")
    }
}

fn request_cost_label(request: &crate::conversations::RequestUsage) -> String {
    if request.auth == crate::providers::AuthMethod::Plan {
        return "Cost unknown for plan authentication".to_owned();
    }
    cost_label(&crate::conversations::history::request_cost(request))
}

fn cost_label(coverage: &crate::conversations::history::CostCoverage) -> String {
    match (coverage.known_micros, coverage.incomplete) {
        (None, false) => String::new(),
        (None, true) => "Cost unknown".to_owned(),
        (Some(micros), false) => format!("Estimated cost {}", format_usd(micros)),
        (Some(micros), true) => {
            format!(
                "Known subtotal {} (estimate, incomplete)",
                format_usd(micros)
            )
        }
    }
}

fn format_usd(micros: u64) -> String {
    let whole = micros / 1_000_000;
    let frac = micros % 1_000_000;
    if frac == 0 {
        format!("${whole}.00")
    } else {
        let mut frac_str = format!("{frac:06}");
        while frac_str.ends_with('0') && frac_str.len() > 2 {
            frac_str.pop();
        }
        format!("${whole}.{frac_str}")
    }
}

fn format_count(value: u64) -> String {
    let raw = value.to_string();
    let mut formatted = String::new();
    for (index, character) in raw.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted.chars().rev().collect()
}

fn copy_response_view(text: &str) -> Option<CopyResponseView> {
    if text.trim().is_empty() {
        return None;
    }
    Some(CopyResponseView {
        source: text.to_owned(),
        escaped_bytes: html_escaped_len(text),
    })
}

// Matches the Askama HTML escaping used by the message body template.
fn html_escaped_len(text: &str) -> usize {
    text.len()
        + 4 * text
            .bytes()
            .filter(|byte| matches!(byte, b'"' | b'&' | b'\'' | b'<' | b'>'))
            .count()
}

pub(super) fn message_error(message: &ConversationMessage) -> String {
    if message.status == MessageStatus::Failed {
        message.error.clone().unwrap_or_else(|| {
            "The reply failed. No error details are available for this message.".to_owned()
        })
    } else {
        String::new()
    }
}

#[derive(Template)]
#[template(path = "conversations/templates/activity.html", block = "activity")]
pub(super) struct ActivityContents<'a> {
    pub(super) has_run: bool,
    pub(super) name: &'a str,
    pub(super) state: &'a str,
    pub(super) current_step: &'a str,
    pub(super) task_progress: &'a str,
    pub(super) result: &'a str,
    pub(super) run_href: &'a str,
    pub(super) back_href: String,
    pub(super) launch_brief: String,
    pub(super) task_line: String,
    pub(super) environment_line: String,
    pub(super) needs_recovery: bool,
    pub(super) apply_outcomes: &'a [ApplyOutcomeView],
    pub(super) apply_partial: bool,
    pub(super) apply_uncertain: bool,
    pub(super) apply_complete: bool,
    pub(super) apply_resolve_href: &'a str,
}

impl ConversationDetailView {
    pub(super) fn omitted_entries(&self) -> Vec<&(String, String)> {
        self.window_entries
            .iter()
            .filter(|(_, id)| !self.messages.iter().any(|message| message.id == *id))
            .collect()
    }

    pub(super) fn with_companion(mut self, html: String, kind: &'static str) -> Self {
        let budget = (672_usize * 1024)
            .saturating_sub(html.len())
            .saturating_sub(ammonia::clean_text(&self.model_picker.catalogue).len());
        let mut bytes = 0;
        let count = self
            .messages
            .iter()
            .rev()
            .take_while(|message| {
                bytes += message.html.len()
                    + message
                        .command
                        .as_ref()
                        .map_or(0, |command| ammonia::clean_text(command).len())
                    + message.copy.as_ref().map_or(0, |copy| copy.escaped_bytes)
                    + 2048;
                bytes <= budget
            })
            .count();
        let removed = self.messages.len() - count;
        self.messages.drain(..removed);
        self.omitted_messages += removed;
        self.companion_html = html;
        self.companion_kind = kind;
        self.companion_title = String::new();
        self
    }

    pub(super) fn with_companion_titled(
        self,
        html: String,
        kind: &'static str,
        title: &str,
    ) -> Self {
        let mut titled = self.with_companion(html, kind);
        titled.companion_title = title.to_owned();
        titled
    }

    pub(super) fn render_activity(
        &self,
        state: &crate::state::AppState,
    ) -> Result<String, askama::Error> {
        use askama::Template;
        let saved = self.saved().expect("saved activity conversation");
        let back_href = format!("/conversations/{}", saved.id);
        let Some(progress) = saved.workflow_progress.as_ref() else {
            return ActivityContents {
                has_run: false,
                name: "",
                state: "",
                current_step: "",
                task_progress: "",
                result: "",
                run_href: "",
                back_href,
                launch_brief: String::new(),
                task_line: String::new(),
                environment_line: String::new(),
                needs_recovery: false,
                apply_outcomes: &[],
                apply_partial: false,
                apply_uncertain: false,
                apply_complete: false,
                apply_resolve_href: "",
            }
            .render();
        };
        let conversation_id = crate::conversations::ConversationId::parse(&saved.id);
        let (launch_brief, task_line, environment_line) = conversation_id
            .map(|id| activity_source_lines(state, &id))
            .unwrap_or_default();
        ActivityContents {
            has_run: true,
            name: &progress.name,
            state: progress.state,
            current_step: &progress.current_step,
            task_progress: &progress.task_progress,
            result: progress.result,
            run_href: &progress.run_href,
            back_href,
            launch_brief,
            task_line,
            environment_line,
            needs_recovery: matches!(progress.state, "Failed" | "Blocked" | "Interrupted"),
            apply_outcomes: &progress.apply_outcomes,
            apply_partial: progress.apply_partial,
            apply_uncertain: progress.apply_uncertain,
            apply_complete: progress.apply_complete,
            apply_resolve_href: &progress.apply_resolve_href,
        }
        .render()
    }
}

fn plain_html(text: &str) -> String {
    format!("<pre>{}</pre>", ammonia::clean_text(text))
}

pub(super) fn reply_html(text: &str) -> String {
    bounded_reply_html(crate::markdown::render(text), text)
}

fn bounded_reply_html(html: String, text: &str) -> String {
    if html.len() > 768 * 1024 || html.matches('<').count() > 64 {
        // Dense markup uses plain text so one message cannot exhaust the browser node bound.
        plain_html(text)
    } else {
        html
    }
}

#[derive(Template)]
#[template(path = "conversations/templates/message_article.html")]
pub(super) struct MessageArticle<'a> {
    pub(super) message: &'a MessageView,
}

#[derive(Template)]
#[template(path = "conversations/templates/message_body.html")]
pub(super) struct MessageBody<'a> {
    pub(super) message: &'a MessageView,
}

#[derive(Template)]
#[template(path = "conversations/templates/observe.html")]
pub(super) struct ConversationObserveContents<'a> {
    pub(super) id: &'a str,
    pub(super) job_id: &'a str,
    pub(super) cursor: u64,
    pub(super) active: bool,
    pub(super) historical: bool,
}

#[derive(Template)]
#[template(path = "conversations/templates/history_status.html")]
pub(super) struct HistoryStatusContents<'a> {
    pub(super) history_status: &'a str,
    pub(super) latest_href: &'a str,
}
