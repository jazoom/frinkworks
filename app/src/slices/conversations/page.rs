pub(crate) mod model_picker;

use askama::Template;
use model_picker::ModelPicker;

#[cfg(test)]
mod tests;

use crate::{
    agents::AgentRecord,
    conversations::{
        ConversationMessage, ConversationModelConfiguration, ConversationRecord,
        MAXIMUM_PROJECT_ASSOCIATIONS, MessageRole, MessageStatus, PlanDocument, PlanSource,
    },
    environments::{EnvironmentCatalogue, EnvironmentId, EnvironmentSnapshotRepository},
    models::models_dev::ModelsDevCatalogue,
    projects::ProjectRecord,
    providers::ModelSelection,
    sessions::{JobSnapshot, JobStatus},
    vault::ProviderVault,
    workflows::WorkflowRun,
};

pub(super) const CATALOGUE_TITLE: &str = "Conversations | Power Plant";

#[derive(Template)]
#[template(source = "{{ message }}", ext = "html")]
pub(super) struct ModelSelectionStatus<'a> {
    pub(super) message: &'a str,
}

pub(super) struct ConversationListItem {
    pub(super) title: String,
    pub(super) href: String,
    pub(super) status: &'static str,
    pub(super) dot: &'static str,
    pub(super) meta: String,
}

pub(super) struct CatalogueProjectOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) selected: bool,
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

pub(super) struct ProjectContextView {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) alias: String,
    pub(super) status: &'static str,
    pub(super) access_granted: bool,
    pub(super) writable: bool,
    pub(super) execution_target: bool,
    pub(super) secondary_context: bool,
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
    pub(super) application_destination: String,
    pub(super) can_request_revision: bool,
    pub(super) quick_task: bool,
    pub(super) exclusions: Vec<String>,
    pub(super) changes: Vec<CandidateChangeView>,
    pub(super) total_changes: usize,
    pub(super) changes_truncated: bool,
}

#[derive(Template)]
#[template(
    path = "conversations/templates/index.html",
    block = "conversation_catalogue"
)]
pub(super) struct CatalogueView {
    pub(super) conversations: Vec<ConversationListItem>,
    pub(super) directories: Vec<HistoryDirectoryOption>,
    pub(super) filter: String,
    pub(super) query: String,
    pub(super) error: &'static str,
    pub(super) back_href: String,
    pub(super) back_label: &'static str,
}

pub(super) struct HistoryDirectoryOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) selected: bool,
    available: bool,
}

impl CatalogueView {
    pub(super) fn from_records(
        state: &crate::state::AppState,
        records: &[ConversationRecord],
        filter: &str,
        query: &str,
        error: &'static str,
        back_href: String,
        back_label: &'static str,
    ) -> Self {
        let needle = query.trim().to_lowercase();
        let mut ordered: Vec<_> = records.iter().collect();
        ordered.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        let conversations: Vec<_> = ordered
            .into_iter()
            .filter(|record| {
                (filter.is_empty()
                    || history_grants(record).any(|grant| history_directory_key(grant) == filter))
                    && (needle.is_empty() || record.title.to_lowercase().contains(&needle))
            })
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
        }
    }
}

pub(super) fn history_grants(
    record: &ConversationRecord,
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

pub(super) struct ReviewProjectOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) access: &'static str,
    pub(super) selected: bool,
}

#[derive(Template)]
#[template(
    path = "conversations/templates/index.html",
    block = "plan_review_page"
)]
pub(super) struct PlanReviewView {
    pub(super) document_title: String,
    pub(super) source_title: String,
    pub(super) source_id: String,
    pub(super) source_revision: String,
    pub(super) document_id: String,
    pub(super) document_revision: u32,
    pub(super) content_hash: String,
    pub(super) content_html: String,
    pub(super) brief: String,
    pub(super) reviewer_summary: String,
    pub(super) model_picker: ModelPicker,
    pub(super) presets: Vec<PresetOption>,
    pub(super) read_only_projects: Vec<ReviewProjectOption>,
    pub(super) error: &'static str,
}

#[derive(Template)]
#[template(
    path = "conversations/templates/index.html",
    block = "plan_review_detail"
)]
pub(super) struct PlanReviewContents<'a> {
    pub(super) source_title: &'a str,
    pub(super) source_id: &'a str,
    pub(super) source_revision: &'a str,
    pub(super) document_id: &'a str,
    pub(super) document_revision: u32,
    pub(super) content_hash: &'a str,
    pub(super) content_html: &'a str,
    pub(super) brief: &'a str,
    pub(super) reviewer_summary: &'a str,
    pub(super) model_picker: &'a ModelPicker,
    pub(super) presets: &'a [PresetOption],
    pub(super) read_only_projects: &'a [ReviewProjectOption],
    pub(super) error: &'static str,
}

impl PlanReviewView {
    pub(super) fn contents(&self) -> PlanReviewContents<'_> {
        PlanReviewContents {
            source_title: &self.source_title,
            source_id: &self.source_id,
            source_revision: &self.source_revision,
            document_id: &self.document_id,
            document_revision: self.document_revision,
            content_hash: &self.content_hash,
            content_html: &self.content_html,
            brief: &self.brief,
            reviewer_summary: &self.reviewer_summary,
            model_picker: &self.model_picker,
            presets: &self.presets,
            read_only_projects: &self.read_only_projects,
            error: self.error,
        }
    }
}

#[derive(Template)]
#[template(
    path = "conversations/templates/index.html",
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
    pub(super) presets: Vec<PresetOption>,
    pub(super) error: &'static str,
}

#[derive(Template)]
#[template(
    path = "conversations/templates/index.html",
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
    pub(super) presets: &'a [PresetOption],
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
            presets: &self.presets,
            error: self.error,
        }
    }
}

pub(super) struct MessageView {
    pub(super) index: usize,
    pub(super) user: bool,
    pub(super) html: String,
    pub(super) status: &'static str,
    pub(super) error: String,
    pub(super) streaming: bool,
    pub(super) saveable_plan: bool,
    pub(super) plan_request_href: String,
}

pub(super) struct PlanDocumentView {
    pub(super) title: String,
    pub(super) kind: String,
    pub(super) task_list: bool,
    pub(super) revision: u32,
    pub(super) provenance: String,
    pub(super) open_href: String,
    pub(super) remove_href: String,
}

pub(super) struct ConversationLinkView {
    pub(super) title: String,
    pub(super) plan_title: String,
    pub(super) href: String,
    pub(super) plan_href: String,
    pub(super) plan_revision: u32,
    pub(super) content_hash: String,
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
    pub(super) loop_id: String,
    pub(super) command_token: String,
    pub(super) can_pause: bool,
    pub(super) can_continue: bool,
    pub(super) can_retry: bool,
    pub(super) can_stop: bool,
    pub(super) pause_requested: bool,
    pub(super) awaiting_gate: bool,
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
    pub(super) projects: &'a [ProjectRecord],
    pub(super) documents: &'a [PlanDocument],
    pub(super) presets: &'a [crate::presets::PresetRecord],
}

pub(super) struct PresetOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) selected: bool,
}

pub(super) struct PresetPreviewView {
    pub(super) token: String,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) instructions: String,
    pub(super) environment: String,
    pub(super) tools: String,
    pub(super) network: String,
    pub(super) location: String,
    pub(super) directories: Vec<String>,
}

pub(super) struct ProviderOption {
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) model: String,
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

pub(super) struct ExecutionSwitchView {
    pub(super) requested_location: String,
    pub(super) requested_host_approval: String,
    pub(super) requested_environment: String,
    pub(super) directory_access: String,
    pub(super) current_backend: &'static str,
    pub(super) requested_backend: &'static str,
    pub(super) current_approval: &'static str,
    pub(super) requested_approval: &'static str,
    pub(super) current_environment: String,
    pub(super) requested_environment_name: String,
    pub(super) environment_changes: bool,
    pub(super) backend_changes: bool,
    pub(super) approval_changes: bool,
    pub(super) access_lines: Vec<String>,
    pub(super) host_effects_remain: bool,
    pub(super) needs_new_consent: bool,
    pub(super) active_job: bool,
    pub(super) job_id: String,
    pub(super) gate: Option<EnvironmentSwitchGateView>,
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
    New { project: String, message: String },
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
    show_thinking: bool,
    thinking_visibility_error: Option<&'static str>,
    pub(super) messages: Vec<MessageView>,
    pub(super) companion_html: String,
    pub(super) companion_kind: &'static str,
    pub(super) companion_title: String,
    pub(super) plan_actions_omitted: bool,
    pub(super) omitted_messages: usize,
    model_picker: ModelPicker,
    pub(super) presets: Vec<PresetOption>,
    pub(super) preset_name: String,
    pub(super) preset_source: String,
    pub(super) preset_preview: Option<PresetPreviewView>,
    pub(super) attached_projects: Vec<ProjectContextView>,
    pub(super) attachable_projects: Vec<CatalogueProjectOption>,
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
    pub(super) draft_preset_reference: String,
    pub(super) consent_reference: String,
    pub(super) instructions: String,
    pub(super) tool_options: Vec<ToolOption>,
    pub(super) environment_options: Vec<EnvironmentOption>,
    pub(super) environment_summary: String,
    pub(super) execution_switch: Option<ExecutionSwitchView>,
    pub(super) network_options: Vec<NetworkOption>,
    pub(super) network_restricted: bool,
    pub(super) network_domains: String,
    pub(super) network_summary: String,
    pub(super) network_detail: String,
    pub(super) location_host: bool,
    pub(super) host_summary: String,
    pub(super) host_identity: String,
    pub(super) host_elevated: bool,
    pub(super) host_approval_automatic: bool,
    pub(super) host_pending_approval: bool,
    pub(super) host_consent_request: String,
    pub(super) pending_host_command: Option<HostCommandView>,
    pub(super) observe_active: bool,
    pub(super) settings_open: bool,
    pub(super) directories_open: bool,
    pub(super) model_available: bool,
    pub(super) job_active: bool,
    pub(super) session_busy: bool,
}

pub(super) struct SavedConversationState {
    pub(super) id: String,
    pub(super) revision: String,
    pub(super) project_limit_reached: bool,
    pub(super) job_id: String,
    pub(super) cursor: u64,
    pub(super) pending_gate: Option<PendingCodeGateView>,
    pub(super) plans: Vec<PlanDocumentView>,
    pub(super) source_review: Option<ConversationLinkView>,
    pub(super) linked_reviews: Vec<ConversationLinkView>,
    pub(super) source_candidate_review: Option<CandidateReviewLinkView>,
    pub(super) linked_candidate_reviews: Vec<CandidateReviewLinkView>,
    pub(super) workflow_progress: Option<WorkflowProgressView>,
}
impl ConversationDetailView {
    pub(super) fn from_new(
        state: &crate::state::AppState,
        session: crate::sessions::SessionId,
        form: super::new::NewForm,
        error: &'static str,
    ) -> Self {
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
            view.pending_approval = (view.sensitive
                || grant.access != crate::execution::DirectoryAccess::ReadOnly)
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
            attachable_projects: state
                .projects
                .list()
                .into_iter()
                .map(|project| CatalogueProjectOption {
                    id: project.id.as_hex(),
                    name: project.name,
                    selected: project.id.as_hex() == form.project,
                })
                .collect(),
            state: ConversationPageState::New {
                project: form.project,
                message: form.message,
            },
            error,
            messages: Vec::new(),
            companion_html: String::new(),
            companion_kind: "plan",
            companion_title: String::new(),
            plan_actions_omitted: false,
            omitted_messages: 0,
            attached_projects: Vec::new(),
            directories,
            data_root: state.local_data.root().to_string_lossy().into_owned(),
            consent_path,
            consent_request: form.consent_request,
            pending_directory: form.pending_directory,
            consent_existing,
            consent_direct,
            consent_sensitive,
            consent_reviewed,
            draft_nonce: form.draft_nonce,
            draft_preset_reference: form.preset_preview,
            consent_reference: form.consent_reference,
            instructions: form.instructions.clone(),
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
            network_detail: String::new(),
            location_host,
            host_summary: host_access_summary(location_host, host_approval_automatic),
            host_identity: host_identity.authority_summary(),
            host_elevated: host_identity.elevated(),
            host_approval_automatic,
            host_pending_approval,
            host_consent_request,
            pending_host_command: None,
            observe_active: false,
            settings_open: false,
            directories_open: false,
            model_available: !form.model.is_empty(),
            job_active: false,
            session_busy: false,
        }
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
                || saved.pending_gate.is_some()
                || saved.workflow_progress.is_some()
        })
    }

    fn needs_review(&self) -> bool {
        self.saved().is_some_and(|saved| {
            saved.pending_gate.is_some() || self.pending_host_command.is_some()
        })
    }

    fn draft_project(&self) -> &str {
        match &self.state {
            ConversationPageState::New { project, .. } => project,
            ConversationPageState::Saved(_) => "",
        }
    }

    fn draft_message(&self) -> &str {
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

    fn composer_action(&self) -> String {
        self.saved().map_or_else(
            || "/conversations/new".to_owned(),
            |saved| format!("/conversations/{}/messages", saved.id),
        )
    }

    // The composer stays locked while a candidate or host command awaits a
    // decision. The editor maxlength in the template is not the persisted
    // message bound in the conversation store.
    fn composer_disabled(&self) -> bool {
        if self.is_new() {
            return false;
        }
        self.job_active || self.session_busy || !self.model_available || self.needs_review()
    }

    fn transcript_empty(&self) -> bool {
        self.messages.is_empty()
            && self.saved().is_none_or(|saved| {
                saved.workflow_progress.is_none()
                    && saved.pending_gate.is_none()
                    && saved.source_review.is_none()
                    && saved.source_candidate_review.is_none()
            })
    }

    #[cfg(test)]
    pub(super) fn from_record(
        record: &ConversationRecord,
        sources: ModelSources<'_>,
        agents: &[AgentRecord],
        job: Option<&JobSnapshot>,
        session_busy: bool,
        title: &str,
        error: &'static str,
    ) -> Self {
        Self::from_record_with_gate(
            record,
            sources,
            agents,
            job,
            session_busy,
            title,
            error,
            None,
            None,
            Vec::new(),
            None,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_record_with_gate(
        record: &ConversationRecord,
        sources: ModelSources<'_>,
        agents: &[AgentRecord],
        job: Option<&JobSnapshot>,
        session_busy: bool,
        title: &str,
        error: &'static str,
        pending_gate: Option<PendingCodeGateView>,
        source_review: Option<ConversationLinkView>,
        linked_reviews: Vec<ConversationLinkView>,
        source_candidate_review: Option<CandidateReviewLinkView>,
        linked_candidate_reviews: Vec<CandidateReviewLinkView>,
    ) -> Self {
        let fallback = sources
            .preferences
            .desk_providers(sources.vault)
            .into_iter()
            .find(|provider| provider.selected)
            .and_then(|connection| {
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
        let network_detail = String::new();
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
        let plans: Vec<_> = sources
            .documents
            .iter()
            .filter(|document| document.associated_conversation == Some(record.id))
            .filter(|document| !matches!(&document.revisions[0].source, PlanSource::Action { plan: Some(plan), .. } if sources.documents.iter().any(|parent| parent.id == plan.document_id)))
            .map(plan_document_view)
            .collect();
        let attached_projects = record
            .projects
            .iter()
            .map(|id| {
                let access = record
                    .grants
                    .iter()
                    .find(|grant| grant.project_id == *id)
                    .map(|grant| grant.access);
                let access_granted = access.is_some();
                let writable = access == Some(crate::agents::AccessMode::ReadWrite);
                let execution_target = record.execution_target == Some(*id);
                let secondary_context = access_granted && !execution_target;
                let alias = if secondary_context {
                    crate::conversations::secondary_alias(*id)
                } else {
                    String::new()
                };
                match sources.projects.iter().find(|project| project.id == *id) {
                    Some(project) if project.host_path_is_available() => ProjectContextView {
                        id: id.as_hex(),
                        name: project.name.clone(),
                        alias: alias.clone(),
                        status: if execution_target && writable {
                            "Writable access granted. This is the execution target. Tools: List, Read, Run and Write in the candidate workspace. Network: None by default."
                        } else if secondary_context {
                            "Read-only context. Tools: List, Read and Run. Writes are blocked. Network: None by default."
                        } else if access_granted {
                            "Read-only access granted. This is the execution target. Tools: List, Read and Run. Network: None by default."
                        } else {
                            "Context reference only. File access is not granted."
                        },
                        access_granted,
                        writable,
                        execution_target,
                        secondary_context,
                    },
                    Some(project) => ProjectContextView {
                        id: id.as_hex(),
                        name: project.name.clone(),
                        alias: String::new(),
                        status: "Project unavailable. It remains a context reference without file access.",
                        access_granted: false,
                        writable: false,
                        execution_target: false,
                        secondary_context: false,
                    },
                    None => ProjectContextView {
                        id: id.as_hex(),
                        name: "Project record unavailable".to_owned(),
                        alias: String::new(),
                        status: "This context reference has no file access.",
                        access_granted: false,
                        writable: false,
                        execution_target: false,
                        secondary_context: false,
                    },
                }
            })
            .collect();
        let mut attachable_projects: Vec<_> = sources
            .projects
            .iter()
            .filter(|project| !record.projects.contains(&project.id))
            .map(|project| CatalogueProjectOption {
                id: project.id.as_hex(),
                name: project.name.clone(),
                selected: false,
            })
            .collect();
        attachable_projects.sort_by(|left, right| left.name.cmp(&right.name));
        let (job_id, cursor, job_active, observe_active) = match job {
            Some(job) if job.status == JobStatus::Running => {
                (job.id.as_hex(), job.latest_seq, true, true)
            }
            Some(job) if job.status == JobStatus::AwaitingDecision => {
                (job.id.as_hex(), job.latest_seq, true, false)
            }
            _ => (String::new(), 0, false, false),
        };
        let location_host = configuration
            .is_some_and(|model| model.settings.location == crate::execution::ToolLocation::Host);
        let host_approval_automatic =
            configuration.is_some_and(|model| model.settings.host_approval.automatic());
        let host_identity = crate::execution::HostIdentity::current();
        // Plan controls and the escaped model catalogue share the transcript envelope.
        let message_budget = (672_usize * 1024)
            .saturating_sub(plans.len() * 3072)
            .saturating_sub(ammonia::clean_text(&model_picker.catalogue).len());
        let messages = visible_messages(record, message_budget);
        let omitted_messages = record.messages.len() - messages.len();
        Self {
            show_thinking: sources.preferences.show_thinking(),
            thinking_visibility_error: None,
            heading: record.title.clone(),
            document_title: format!("{} | Power Plant", record.title),
            title: title.to_owned(),
            error,
            messages,
            companion_html: String::new(),
            companion_kind: "plan",
            companion_title: String::new(),
            plan_actions_omitted: false,
            omitted_messages,
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
            attached_projects,
            attachable_projects,
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
            draft_preset_reference: String::new(),
            consent_reference: String::new(),
            instructions: configuration
                .map(|configuration| configuration.settings.instructions.clone())
                .unwrap_or_default(),
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
            network_detail,
            location_host,
            host_summary: host_access_summary(location_host, host_approval_automatic),
            host_identity: host_identity.authority_summary(),
            host_elevated: host_identity.elevated(),
            host_approval_automatic,
            host_pending_approval: false,
            host_consent_request: String::new(),
            pending_host_command: None,
            observe_active,
            settings_open: false,
            directories_open: false,
            job_active,
            session_busy,
            state: ConversationPageState::Saved(Box::new(SavedConversationState {
                id: record.id.as_hex(),
                revision: record.revision.to_string(),
                project_limit_reached: record.projects.len() >= MAXIMUM_PROJECT_ASSOCIATIONS,
                job_id,
                cursor,
                pending_gate,
                plans,
                source_review,
                linked_reviews,
                source_candidate_review,
                linked_candidate_reviews,
                workflow_progress: None,
            })),
        }
    }

    pub(super) fn with_preset_preview(mut self, preview: PresetPreviewView) -> Self {
        self.preset_preview = Some(preview);
        self.settings_open = true;
        self
    }

    pub(super) fn with_access_status(
        mut self,
        state: &crate::state::AppState,
        session: crate::sessions::SessionId,
        record: &ConversationRecord,
    ) -> Self {
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
                view.pending_approval = (view.sensitive
                    || grant.access != crate::execution::DirectoryAccess::ReadOnly)
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
            current_backend: backend_label(current.location),
            requested_backend: backend_label(location),
            current_approval: crate::slices::execution_settings::page::host_approval_label(
                current.host_approval,
            ),
            requested_approval: crate::slices::execution_settings::page::host_approval_label(
                host_approval,
            ),
            current_environment,
            requested_environment_name,
            environment_changes: current.environment != environment,
            backend_changes: current.location != location,
            approval_changes: current.host_approval != host_approval,
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
    current.directories.iter().any(|grant| {
        grant.access != crate::execution::DirectoryAccess::ReadOnly
            || crate::execution::authority::sensitive_directory(
                &grant.host_path,
                state.local_data.root(),
            )
    })
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
        loop_id: String::new(),
        command_token: String::new(),
        can_pause: false,
        can_continue: false,
        can_retry: false,
        can_stop: false,
        pause_requested: false,
        awaiting_gate: false,
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

pub(super) fn loop_progress(
    record: &crate::workflows::TaskLoop,
    awaiting_gate: bool,
) -> WorkflowProgressView {
    loop_progress_with_child(record, awaiting_gate, None)
}

pub(super) fn loop_progress_with_child(
    record: &crate::workflows::TaskLoop,
    awaiting_gate: bool,
    child: Option<&WorkflowRun>,
) -> WorkflowProgressView {
    let (apply_run_id, apply_attempt_id, apply_state, apply_resolve_href) = child
        .map(apply_attempt_presentation)
        .unwrap_or((String::new(), String::new(), "", String::new()));
    // An uncertain child blocks continuation and retry until recovery finishes.
    let child_uncertain = child.is_some_and(|run| run.apply_is_uncertain());
    WorkflowProgressView {
        run_href: format!("/runs/loops/{}", record.id.as_hex()),
        name: record.pinned.definition.name().to_owned(),
        state: record.state.as_label(),
        current_step: record.progress_label(),
        result: loop_result_label(&record.state),
        task_progress: record.progress_label(),
        loop_id: record.id.as_hex(),
        command_token: record.command_token(),
        can_pause: matches!(
            record.state,
            crate::workflows::task_loop::TaskLoopState::Active { .. }
                | crate::workflows::task_loop::TaskLoopState::AwaitingChild { .. }
        ),
        can_continue: record.allows_continue() && !child_uncertain,
        can_retry: record.allows_retry() && !child_uncertain,
        can_stop: !record.state.is_terminal(),
        pause_requested: record.pause_requested(),
        awaiting_gate,
        conversation_id: record.conversation_id.as_hex(),
        apply_outcomes: child.map(apply_outcomes).unwrap_or_default(),
        apply_run_id,
        apply_attempt_id,
        apply_state,
        apply_resolve_href,
        apply_partial: child.is_some_and(|run| run.apply_is_known_partial()),
        apply_uncertain: child_uncertain,
        apply_complete: child.is_some_and(|run| run.apply_is_complete()),
        settlement_eligible: false,
        run_terminal: record.state.is_terminal(),
    }
}

fn loop_result_label(state: &crate::workflows::task_loop::TaskLoopState) -> &'static str {
    match state {
        crate::workflows::task_loop::TaskLoopState::Completed => {
            "Open the parent run for each task's file application, commit or host execution result."
        }
        crate::workflows::task_loop::TaskLoopState::Blocked => {
            "The task loop is unavailable until reconciliation finishes. Earlier file changes and host command effects remain."
        }
        crate::workflows::task_loop::TaskLoopState::Cancelled
        | crate::workflows::task_loop::TaskLoopState::Stopped => {
            "The task loop stopped. Earlier file changes and host command effects remain. Stop does not reverse them."
        }
        crate::workflows::task_loop::TaskLoopState::Interrupted
        | crate::workflows::task_loop::TaskLoopState::Failed => {
            "The current task stopped. Retry starts a fresh attempt from the recorded task base. It does not resume the earlier transcript."
        }
        crate::workflows::task_loop::TaskLoopState::AwaitingChild { .. } => {
            "A task waits for a human decision. The conversation stays reserved."
        }
        crate::workflows::task_loop::TaskLoopState::PauseRequested { .. } => {
            "A pause waits until the current task settles. Pause is not approval."
        }
        crate::workflows::task_loop::TaskLoopState::Paused => {
            "The loop is paused after a completed task. Continue starts the next pending task."
        }
        _ => "Each task uses a fresh worker context. Earlier worker transcripts stay excluded.",
    }
}

fn activity_source_lines(
    state: &crate::state::AppState,
    conversation_id: &crate::conversations::ConversationId,
) -> (String, String, String) {
    let latest_loop = state
        .task_loops
        .for_conversation(conversation_id)
        .into_iter()
        .next();
    let latest_run = state
        .workflow_runs
        .for_conversation(conversation_id)
        .into_iter()
        .next();
    let (brief, task, environments) = match (latest_loop, latest_run) {
        (Some(parent), run)
            if run
                .as_ref()
                .is_none_or(|run| parent.created_at_ms >= run.created_at_ms) =>
        {
            let child = parent
                .occupied_child()
                .and_then(|child| state.workflow_runs.get(&child));
            (
                parent.launch_brief.clone(),
                format!(
                    "Task list revision {} · {} {}",
                    parent.task_list.revision,
                    parent.task_list.content_hash.as_str(),
                    parent.progress_label()
                ),
                child.map_or(parent.environments.clone(), |run| run.environments.clone()),
            )
        }
        (_, Some(run)) => {
            let task = run
                .task_selection
                .as_ref()
                .map_or_else(String::new, |selection| {
                    format!(
                        "Task {} from task-list revision {}",
                        selection.index, selection.revision
                    )
                });
            (run.launch_brief.clone(), task, run.environments.clone())
        }
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
        revision: gate.revision.get().to_string(),
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

// The transcript leaves envelope space for controls and retains stable message indices.
fn visible_messages(record: &ConversationRecord, byte_budget: usize) -> Vec<MessageView> {
    let mut messages = Vec::new();
    let mut bytes = 0;
    for (index, message) in record.messages.iter().enumerate().rev() {
        let mut view = message_view(index, message);
        if view.saveable_plan {
            view.plan_request_href = format!(
                "/conversations/{}/plans/request?mode=from&message_index={}",
                record.id.as_hex(),
                index
            );
        }
        bytes += view.html.len() + 2048;
        if bytes > byte_budget || messages.len() >= 64 {
            break;
        }
        messages.push(view);
    }
    messages.reverse();
    messages
}

fn message_view(index: usize, message: &ConversationMessage) -> MessageView {
    let user = message.role == MessageRole::User;
    MessageView {
        index,
        user,
        html: if user {
            format!(
                "<p class=\"whitespace-pre-wrap\">{}</p>",
                ammonia::clean_text(&message.text)
            )
        } else {
            reply_html(&message.text)
        },
        status: match message.status {
            MessageStatus::Complete => "",
            MessageStatus::Pending => "Replying",
            MessageStatus::Interrupted => "Interrupted",
            MessageStatus::Failed => "Failed",
        },
        error: message_error(message),
        streaming: message.status == MessageStatus::Pending,
        saveable_plan: !user
            && message.status == MessageStatus::Complete
            && !message.text.trim().is_empty(),
        plan_request_href: String::new(),
    }
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

pub(super) fn plan_document_view(document: &PlanDocument) -> PlanDocumentView {
    let revision = document.current();
    PlanDocumentView {
        title: document.title.clone(),
        kind: document.kind.label().to_owned(),
        task_list: document.kind == crate::conversations::DocumentKind::TaskList,
        revision: revision.revision,
        provenance: source_label(&revision.source),
        open_href: format!(
            "/plans/{}?revision={}",
            document.id.as_hex(),
            revision.revision
        ),
        remove_href: format!(
            "/conversations/{}/plans/{}/remove",
            document
                .associated_conversation
                .expect("associated plan document")
                .as_hex(),
            document.id.as_hex()
        ),
    }
}

#[derive(Template)]
#[template(path = "conversations/templates/plans.html", block = "plans_list")]
pub(super) struct PlansListContents<'a> {
    pub(super) revision: &'a str,
    pub(super) plans: &'a [PlanDocumentView],
    pub(super) error: &'a str,
    pub(super) job_locked: bool,
    pub(super) create_href: &'a str,
    pub(super) paste_href: &'a str,
}

#[derive(Template)]
#[template(path = "conversations/templates/plans.html", block = "plan_request")]
pub(super) struct PlanRequestContents<'a> {
    pub(super) revision: &'a str,
    pub(super) mode: &'a str,
    pub(super) title: &'a str,
    pub(super) request: &'a str,
    pub(super) markdown: &'a str,
    pub(super) message_index: &'a str,
    pub(super) show_source: bool,
    pub(super) source_heading: &'a str,
    pub(super) source_text: &'a str,
    pub(super) error: &'a str,
    pub(super) back_href: &'a str,
    pub(super) form_action: &'a str,
    pub(super) task_action: &'a str,
    pub(super) submit_label: &'a str,
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

#[derive(Template)]
#[template(path = "conversations/templates/plan_action.html")]
struct PlanActionView {
    task_list: bool,
    deferred: bool,
    label: &'static str,
    title: String,
    html: String,
    href: String,
    provenance: String,
    revision: u32,
    hash: String,
}

impl ConversationDetailView {
    pub(super) fn with_companion(mut self, html: String, kind: &'static str) -> Self {
        let budget = (672_usize * 1024)
            .saturating_sub(html.len())
            .saturating_sub(self.saved().map_or(0, |saved| saved.plans.len()) * 3072)
            .saturating_sub(ammonia::clean_text(&self.model_picker.catalogue).len());
        let mut bytes = 0;
        let count = self
            .messages
            .iter()
            .rev()
            .take_while(|message| {
                bytes += message.html.len() + 2048;
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

    pub(super) fn command_locked(&self) -> bool {
        self.job_active || self.session_busy
    }

    /// The Plans list occupies the companion beside the transcript. Its
    /// header and footer stay outside the content scroll area.
    pub(super) fn render_plans_list(&self, error: &str) -> Result<String, askama::Error> {
        use askama::Template;
        let saved = self.saved().expect("saved plans conversation");
        PlansListContents {
            revision: &saved.revision,
            plans: &saved.plans,
            error,
            job_locked: self.command_locked(),
            create_href: &format!("/conversations/{}/plans/request?mode=create", saved.id),
            paste_href: &format!("/conversations/{}/plans/request?mode=paste", saved.id),
        }
        .render()
    }

    /// A plan request companion keeps the exact source message and needs an
    /// explicit submission. Cancellation returns to Plans with no model call.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_plan_request(
        &self,
        mode: &str,
        title: &str,
        request: &str,
        markdown: &str,
        message_index: &str,
        source_heading: &str,
        source_text: &str,
        error: &str,
    ) -> Result<String, askama::Error> {
        use askama::Template;
        let saved = self.saved().expect("saved plan request conversation");
        let (form_action, submit_label) = match mode {
            "paste" => (
                format!("/conversations/{}/plans/text", saved.id),
                "Add plan",
            ),
            "from" => (
                format!("/conversations/{}/plans/from-message", saved.id),
                "Create plan",
            ),
            _ => (
                format!("/conversations/{}/plans/request", saved.id),
                "Create plan",
            ),
        };
        PlanRequestContents {
            revision: &saved.revision,
            mode,
            title,
            request,
            markdown,
            message_index,
            show_source: !source_text.is_empty(),
            source_heading,
            source_text,
            error,
            back_href: &format!("/conversations/{}/plans", saved.id),
            form_action: &form_action,
            task_action: &format!("/conversations/{}/tasks", saved.id),
            submit_label,
        }
        .render()
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

    pub(super) fn with_plan_actions(
        mut self,
        state: &crate::state::AppState,
        record: &ConversationRecord,
    ) -> Self {
        let mut actions = Vec::new();
        for document in state.documents.action_documents(record.id) {
            for revision in &document.revisions {
                let PlanSource::Action {
                    conversation_id,
                    message_index,
                    title,
                    assistant,
                    previous,
                    plan,
                } = &revision.source
                else {
                    continue;
                };
                if *conversation_id != record.id
                    || !self
                        .messages
                        .iter()
                        .any(|message| message.index == *message_index as usize)
                {
                    continue;
                }
                let Ok(content) = state.documents.content(&document, revision.revision) else {
                    continue;
                };
                // Ordinary actions render their complete original contents.
                // The transcript budget below defers oversized actions to
                // their pinned revision instead of excerpting them.
                let html = reply_html(&content);
                let href = format!("/plans/{}?revision={}", document.id, revision.revision);
                let view = PlanActionView {
                    deferred: false,
                    task_list: document.kind == crate::conversations::DocumentKind::TaskList,
                    label: if plan.is_some() {
                        "Created a task breakdown"
                    } else if previous.is_some() {
                        "Revised the plan"
                    } else if *assistant {
                        "Created a plan"
                    } else {
                        "Added a plan"
                    },
                    title: title.clone(),
                    html,
                    href,
                    provenance: source_label(&revision.source),
                    revision: revision.revision,
                    hash: revision.content_hash.as_str(),
                };
                actions.push((
                    revision.created_at_ms,
                    *message_index as usize + usize::from(!assistant),
                    *assistant,
                    view,
                ));
            }
        }
        for document in state.documents.list_for_conversation(record.id) {
            if document.kind != crate::conversations::DocumentKind::TaskList {
                continue;
            }
            let Some(revision) = document.revisions.first() else {
                continue;
            };
            let anchor = match &revision.source {
                PlanSource::SubmittedText {
                    conversation_id,
                    message_count,
                    ..
                } if *conversation_id == record.id => *message_count as usize,
                PlanSource::ConversationMessage {
                    conversation_id,
                    message_index,
                    ..
                } if *conversation_id == record.id => {
                    let index = *message_index as usize;
                    if !self.messages.iter().any(|message| message.index == index) {
                        continue;
                    }
                    index + 1
                }
                _ => continue,
            };
            let Ok(content) = state.documents.content(&document, revision.revision) else {
                continue;
            };
            // Standalone task saves keep their SubmittedText or
            // ConversationMessage provenance. The transcript action only
            // presents that immutable revision without a plan association.
            let view = PlanActionView {
                deferred: false,
                task_list: true,
                label: "Added tasks",
                title: document.revision_title(revision.revision).to_owned(),
                html: reply_html(&content),
                href: format!("/plans/{}?revision={}", document.id, revision.revision),
                provenance: source_label(&revision.source),
                revision: revision.revision,
                hash: revision.content_hash.as_str(),
            };
            actions.push((revision.created_at_ms, anchor, false, view));
        }
        actions.sort_by_key(|(time, _, _, _)| *time);
        let mut budget = 128 * 1024usize;
        let mut selected = Vec::new();
        for (_, index, assistant, view) in actions.into_iter().rev() {
            let Ok(full) = view.render() else {
                continue;
            };
            if full.len() <= budget {
                budget -= full.len();
                selected.push((index, assistant, full));
            } else {
                // Bounded delivery keeps the transcript within the Hypergraft
                // envelope. Deferred contents stay available at the pinned
                // revision link inside the fallback. Fallbacks that no longer
                // fit are dropped so the transcript retains its bound.
                let mut fallback = view;
                fallback.deferred = true;
                fallback.html = String::new();
                let Ok(html) = fallback.render() else {
                    continue;
                };
                if html.len() <= budget {
                    budget -= html.len();
                    selected.push((index, assistant, html));
                }
                self.plan_actions_omitted = true;
            }
        }
        for (action_index, (index, assistant, html)) in selected.into_iter().rev().enumerate() {
            if !assistant {
                // The saved message boundary keeps actions before later replies.
                let position = self
                    .messages
                    .iter()
                    .position(|message| {
                        message.index >= index && message.index < record.messages.len()
                    })
                    .unwrap_or(self.messages.len());
                self.messages.insert(
                    position,
                    MessageView {
                        index: record.messages.len() + action_index,
                        user: true,
                        html,
                        status: "",
                        error: String::new(),
                        streaming: false,
                        saveable_plan: false,
                        plan_request_href: String::new(),
                    },
                );
                continue;
            }
            if let Some(message) = self
                .messages
                .iter_mut()
                .find(|message| message.index == index)
            {
                message.html.push_str(&html);
            }
        }
        self
    }
}

fn source_label(source: &PlanSource) -> String {
    match source {
        PlanSource::ConversationMessage { message_index, .. } => {
            format!("Assistant message {}", message_index + 1)
        }
        PlanSource::Action {
            assistant,
            previous,
            plan,
            ..
        } => {
            let author = if !*assistant {
                "User action"
            } else if plan.is_some() {
                "Assistant · create_task_breakdown"
            } else if previous.is_some() {
                "Assistant · revise_plan"
            } else {
                "Assistant · create_plan"
            };
            if let Some(plan) = plan {
                format!(
                    "{author} · Source plan {} revision {}",
                    plan.document_id, plan.revision
                )
            } else if let Some(previous) = previous {
                format!("{author} · Revision of {}", previous.revision)
            } else {
                author.to_owned()
            }
        }
        PlanSource::SubmittedText { .. } => "Submitted document text".to_owned(),
        PlanSource::DirectoryFile { path, .. } => format!("Imported file: {path}"),
        PlanSource::Correction { previous } => {
            format!("Correction of revision {}", previous.revision)
        }
    }
}

pub(super) struct TaskListItemView {
    pub(super) index: u32,
    pub(super) checked: bool,
    pub(super) markdown: String,
    pub(super) run_href: String,
}

pub(super) struct PlanPageRevision {
    pub(super) revision: u32,
    pub(super) provenance: String,
    pub(super) open_href: String,
    pub(super) export_href: String,
}

#[derive(Template)]
#[template(path = "conversations/templates/plan.html", block = "plan_page")]
pub(super) struct PlanDocumentPage {
    pub(super) conversation_revision: u32,
    pub(super) action_href: String,
    pub(super) implementation_href: String,
    pub(super) breakdowns: Vec<PlanDocumentView>,
    pub(super) outdated: bool,
    pub(super) review_href: String,
    pub(super) document_title: String,
    pub(super) title: String,
    pub(super) document_id: String,
    pub(super) document_revision: u32,
    pub(super) current_revision: u32,
    pub(super) provenance: String,
    pub(super) content_hash: String,
    pub(super) content: String,
    pub(super) content_html: String,
    pub(super) revisions: Vec<PlanPageRevision>,
    pub(super) back_href: String,
    pub(super) plans_href: String,
    pub(super) associated: bool,
    pub(super) task_count: usize,
    pub(super) eligible_task_count: usize,
    pub(super) tasks: Vec<TaskListItemView>,
    pub(super) section: usize,
    pub(super) sections: usize,
    pub(super) section_prev: String,
    pub(super) section_next: String,
    pub(super) error: &'static str,
}

#[derive(Template)]
#[template(path = "conversations/templates/plan.html", block = "plan_detail")]
pub(super) struct PlanDocumentContents<'a> {
    pub(super) conversation_revision: u32,
    pub(super) action_href: &'a str,
    pub(super) implementation_href: &'a str,
    pub(super) breakdowns: &'a [PlanDocumentView],
    pub(super) outdated: bool,
    pub(super) review_href: &'a str,
    pub(super) title: &'a str,
    pub(super) document_id: &'a str,
    pub(super) document_revision: u32,
    pub(super) current_revision: u32,
    pub(super) provenance: &'a str,
    pub(super) content_hash: &'a str,
    pub(super) content: &'a str,
    pub(super) content_html: &'a str,
    pub(super) revisions: &'a [PlanPageRevision],
    pub(super) back_href: &'a str,
    pub(super) plans_href: &'a str,
    pub(super) associated: bool,
    pub(super) task_count: usize,
    pub(super) eligible_task_count: usize,
    pub(super) tasks: &'a [TaskListItemView],
    pub(super) section: usize,
    pub(super) sections: usize,
    pub(super) section_prev: &'a str,
    pub(super) section_next: &'a str,
    pub(super) error: &'static str,
}

impl PlanDocumentPage {
    pub(super) fn from_document(
        document: &PlanDocument,
        revision: u32,
        content: String,
        section: usize,
        error: &'static str,
    ) -> Self {
        let selected = document
            .revision(revision)
            .unwrap_or_else(|| document.current());
        let revisions = document
            .revisions
            .iter()
            .rev()
            .map(|item| PlanPageRevision {
                revision: item.revision,
                provenance: source_label(&item.source),
                open_href: format!("/plans/{}?revision={}", document.id.as_hex(), item.revision),
                export_href: format!(
                    "/plans/{}/export?revision={}",
                    document.id.as_hex(),
                    item.revision
                ),
            })
            .collect();
        let associated = document.associated_conversation.is_some();
        let task_list = (document.kind == crate::conversations::DocumentKind::TaskList)
            .then(|| crate::workflows::task_list::parse(&content).ok())
            .flatten();
        let task_count = task_list.as_ref().map_or(0, |list| list.tasks.len());
        let eligible_task_count = task_list
            .as_ref()
            .map_or(0, |list| list.eligible_tasks().count());
        let back_href = document.associated_conversation.map_or_else(
            || "/conversations".to_owned(),
            |id| format!("/conversations/{id}"),
        );
        let plans_href = document.associated_conversation.map_or_else(
            || "/conversations".to_owned(),
            |id| format!("/conversations/{id}/plans"),
        );
        // Sections share the full Markdown parser context, including reference links.
        // An oversized block uses escaped text rather than partial Markdown.
        // Task display demotes the saved heading so the page keeps one h1.
        // The stored revision and the pinned export retain the exact source.
        let display_preamble = task_list
            .as_ref()
            .map(|list| demote_task_heading(&list.preamble));
        let preview_source = display_preamble.as_deref().unwrap_or(content.as_str());
        let (chunks, plain) = plan_sections(preview_source);
        let sections = chunks.len().max(1);
        let section = section.min(sections.saturating_sub(1));
        let section_text = chunks.get(section).cloned().unwrap_or_default();
        let section_href = |index: usize| {
            if index == 0 {
                format!(
                    "/plans/{}?revision={}",
                    document.id.as_hex(),
                    selected.revision
                )
            } else {
                format!(
                    "/plans/{}?revision={}&section={}",
                    document.id.as_hex(),
                    selected.revision,
                    index
                )
            }
        };
        let content_html = if plain {
            plain_html(&section_text)
        } else {
            let start: usize = chunks.iter().take(section).map(String::len).sum();
            let end = start + section_text.len();
            let events = crate::markdown::parser(preview_source)
                .into_offset_iter()
                .filter(|(_, range)| range.start >= start && range.end <= end)
                .map(|(event, _)| event);
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, events);
            bounded_reply_html(ammonia::clean(&html), &section_text)
        };
        Self {
            conversation_revision: 0,
            implementation_href: String::new(),
            breakdowns: Vec::new(),
            outdated: false,
            action_href: document.associated_conversation.map_or_else(String::new, |id| {
                if document.kind == crate::conversations::DocumentKind::TaskList {
                    format!("/conversations/{id}/workflow?task_document={}&task_revision={}&task_hash={}", document.id, selected.revision, selected.content_hash.as_str())
                } else {
                    format!("/conversations/{id}/plans/{}/tasks", document.id)
                }
            }),
            review_href: document.associated_conversation.map_or_else(String::new, |id| format!("/conversations/{id}/plans/{}/review?revision={}", document.id, selected.revision)),
            document_title: format!(
                "{} | {} | Power Plant",
                document.title,
                document.kind.label()
            ),
            title: document.revision_title(selected.revision).to_owned(),
            document_id: document.id.as_hex(),
            document_revision: selected.revision,
            current_revision: document.current_revision(),
            provenance: source_label(&selected.source),
            content_hash: selected.content_hash.as_str(),
            content_html,
            content,
            revisions,
            back_href,
            plans_href,
            associated,
            task_count,
            eligible_task_count,
            section,
            sections,
            section_prev: if section > 0 {
                section_href(section - 1)
            } else {
                String::new()
            },
            section_next: if section + 1 < sections {
                section_href(section + 1)
            } else {
                String::new()
            },
            tasks: task_list.map_or_else(Vec::new, |list| {
                list.tasks
                    .into_iter()
                    .map(|task| TaskListItemView {
                        index: task.index,
                        checked: task.checked,
                        markdown: task.markdown,
                        run_href: document.associated_conversation.map_or_else(String::new, |conversation| format!(
                            "/conversations/{conversation}/workflow?task_document={}&task_revision={}&task_hash={}&task_index={}",
                            document.id.as_hex(), selected.revision, selected.content_hash.as_str(), task.index
                        )),
                    })
                    .collect()
            }),
            error,
        }
    }

    pub(super) fn with_context(
        mut self,
        state: &crate::state::AppState,
        document: &PlanDocument,
    ) -> Self {
        let selected = document
            .revision(self.document_revision)
            .expect("selected revision");
        self.implementation_href = super::workflow::implementation_href(state, document, selected);
        if let Some(owner) = document.associated_conversation {
            self.conversation_revision = state
                .conversations
                .get(&owner)
                .map_or(0, |record| record.revision);
            self.breakdowns = state.documents.list_for_conversation(owner).iter().filter(|child| {
                matches!(&child.revisions[0].source, PlanSource::Action { plan: Some(plan), .. } if plan.document_id == document.id)
            }).map(|child| {
                let mut view = plan_document_view(child);
                if let PlanSource::Action { plan: Some(plan), .. } = &child.revisions[0].source {
                    view.provenance = format!("Source plan revision {}{}", plan.revision, if plan.revision != document.current_revision() { " · Outdated for a new run" } else { "" });
                }
                view
            }).collect();
        }
        if let PlanSource::Action {
            plan: Some(plan), ..
        } = &document.revisions[0].source
        {
            self.outdated = state.documents.get(&plan.document_id).is_none_or(|parent| {
                parent.current_revision() != plan.revision
                    || parent.associated_conversation.is_none()
            });
        }
        self
    }

    pub(super) fn contents(&self) -> PlanDocumentContents<'_> {
        PlanDocumentContents {
            conversation_revision: self.conversation_revision,
            action_href: &self.action_href,
            implementation_href: &self.implementation_href,
            breakdowns: &self.breakdowns,
            outdated: self.outdated,
            review_href: &self.review_href,
            title: &self.title,
            document_id: &self.document_id,
            document_revision: self.document_revision,
            current_revision: self.current_revision,
            provenance: &self.provenance,
            content_hash: &self.content_hash,
            content: &self.content,
            content_html: &self.content_html,
            revisions: &self.revisions,
            back_href: &self.back_href,
            plans_href: &self.plans_href,
            associated: self.associated,
            task_count: self.task_count,
            eligible_task_count: self.eligible_task_count,
            tasks: &self.tasks,
            section: self.section,
            sections: self.sections,
            section_prev: &self.section_prev,
            section_next: &self.section_next,
            error: self.error,
        }
    }
}

pub(super) const PLAN_SECTION_CHARS: usize = 24_000;

pub(super) fn demote_task_heading(preamble: &str) -> String {
    // Demote every level-one heading so the page keeps one h1.
    let mut demoted = String::with_capacity(preamble.len() + preamble.lines().count());
    for line in preamble.split_inclusive('\n') {
        if line.starts_with("# ") && !line[2..].trim().is_empty() {
            demoted.push('#');
            demoted.push_str(line);
        } else {
            demoted.push_str(line);
        }
    }
    demoted
}

pub(super) fn split_plan_sections(content: &str) -> Vec<String> {
    plan_sections(content).0
}

fn plan_sections(content: &str) -> (Vec<String>, bool) {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut end = 0;
    let mut depth = 0usize;
    for (event, range) in crate::markdown::parser(content).into_offset_iter() {
        match event {
            pulldown_cmark::Event::Start(_) => depth += 1,
            pulldown_cmark::Event::End(_) => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        if range.end - end > PLAN_SECTION_CHARS {
            return (split_plain_sections(content), true);
        }
        if range.end - start > PLAN_SECTION_CHARS {
            sections.push(content[start..end].to_owned());
            start = end;
        }
        end = range.end;
    }
    if content.len() - start > PLAN_SECTION_CHARS {
        return (split_plain_sections(content), true);
    }
    sections.push(content[start..].to_owned());
    (sections, false)
}

fn split_plain_sections(content: &str) -> Vec<String> {
    if content.len() <= PLAN_SECTION_CHARS {
        return vec![content.to_owned()];
    }
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in content.split_inclusive('\n') {
        // Escaped continuation preserves exact bytes and UTF-8 boundaries.
        if line.len() > PLAN_SECTION_CHARS {
            if !current.is_empty() {
                sections.push(std::mem::take(&mut current));
            }
            let mut rest = line;
            while rest.len() > PLAN_SECTION_CHARS {
                let mut take = PLAN_SECTION_CHARS;
                while take > 0 && !rest.is_char_boundary(take) {
                    take -= 1;
                }
                if take == 0 {
                    take = rest.chars().next().map_or(1, |cell| cell.len_utf8());
                }
                sections.push(rest[..take].to_owned());
                rest = &rest[take..];
            }
            current.push_str(rest);
            continue;
        }
        if !current.is_empty() && current.len() + line.len() > PLAN_SECTION_CHARS {
            sections.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        sections.push(current);
    }
    if sections.is_empty() {
        vec![content.to_owned()]
    } else {
        sections
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
}
