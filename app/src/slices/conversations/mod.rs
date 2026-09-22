mod activity;
mod attachments;
mod commands;
mod compaction;
mod context;
mod continuation;
mod directories;
mod files;
mod forks;
mod handoff;
mod job;
pub(crate) use job::history_with_review;
mod model_favourites;
mod new;
mod output;
mod questions;
mod queue;

pub(crate) mod recent;
mod title;
mod tool_approval;
pub(super) use title::live_router;
mod page;
pub(crate) mod settings;

mod tree;

mod workflow;

#[cfg(test)]
mod tests;

use axum::{
    Form, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    response::Response,
    routing::{get, post},
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    agents::AgentId,
    conversations::{
        CandidateReviewCreation, CandidateReviewLink, ConversationError, ConversationId,
        ConversationModelConfiguration, ConversationRecord, MessageId, RevisionRequest,
        TranscriptCursor,
    },
    environments::EnvironmentId,
    error::{AppError, AppResult},
    providers::{ModelSelection, ProviderKind, ThinkingEffort},
    responses,
    sessions::{JobId, RequiredSession},
    state::AppState,
    workflows::{self, WorkflowRun},
};

use self::page::model_picker::ModelPicker;
use self::page::{
    CandidateReviewLinkView, CandidateReviewView, CatalogueView, ConversationDetailView,
    ModelSources, PresetOption, RevisionDraft,
};

const REVISION_MESSAGE: &str = "Reload the conversation and try again.";

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(catalogue).post(create))
        .route("/conversations/new", get(new::show).post(new::save))
        .route(
            "/conversations/new/attachments",
            post(attachments::upload_new)
                .layer(DefaultBodyLimit::max(attachments::UPLOAD_BODY_LIMIT)),
        )
        .route(
            "/conversations/new/attachments/{attachment_id}/remove",
            post(attachments::remove_new),
        )
        .route(
            "/conversations/new/attachments/{attachment_id}",
            get(attachments::serve_new),
        )
        .route("/conversations/new/files", get(files::lookup_new))
        .route("/conversations/new/commands", get(commands::lookup_new))
        .route(
            "/conversations/{conversation_id}/commands",
            get(commands::lookup_saved),
        )
        .route(
            "/conversations/{conversation_id}/files",
            get(files::lookup_saved),
        )
        .route(
            "/conversations/{conversation_id}/attachments",
            post(attachments::upload_saved)
                .layer(DefaultBodyLimit::max(attachments::UPLOAD_BODY_LIMIT)),
        )
        .route(
            "/conversations/{conversation_id}/attachments/{attachment_id}/remove",
            post(attachments::remove_saved),
        )
        .route(
            "/conversations/{conversation_id}/attachments/{attachment_id}",
            get(attachments::serve_saved),
        )
        .route(
            "/conversations/models/favourite",
            post(model_favourites::toggle),
        )
        .route(
            "/conversations/new/settings/presets/save",
            post(settings::save_draft_preset),
        )
        .route(
            "/conversations/new/settings/presets/preview",
            post(settings::preview_draft_preset),
        )
        .route(
            "/conversations/new/settings/presets/apply",
            post(settings::apply_draft_preset),
        )
        .route(
            "/conversations/new/directories/pick",
            post(directories::pick_new),
        )
        .route(
            "/conversations/new/directories/consent",
            post(directories::approve_new),
        )
        .route(
            "/conversations/new/directories/{grant_id}/consent",
            post(directories::request_new),
        )
        .route(
            "/conversations/new/directories/{grant_id}/access",
            post(directories::update_new),
        )
        .route(
            "/conversations/new/directories/{grant_id}/remove",
            post(directories::remove_new),
        )
        .route("/conversations/{conversation_id}", get(detail))
        .route(
            "/conversations/{conversation_id}/output/{reference}",
            get(output::show),
        )
        .route(
            "/conversations/{conversation_id}/context/{request_id}",
            get(context::show),
        )
        .route(
            "/conversations/{conversation_id}/activity",
            get(activity::show),
        )
        .route("/conversations/{conversation_id}/tree", get(tree::show))
        .route(
            "/conversations/{conversation_id}/tree/continue",
            post(tree::continue_here),
        )
        .route(
            "/conversations/{conversation_id}/tree/revise",
            get(tree::revise),
        )
        .route(
            "/conversations/{conversation_id}/workflow",
            get(workflow::show).post(workflow::launch),
        )
        .route(
            "/conversations/{conversation_id}/fork",
            get(forks::show).post(forks::create),
        )
        .route(
            "/conversations/{conversation_id}/handoff",
            get(handoff::show),
        )
        .route(
            "/conversations/{conversation_id}/handoff/generate",
            post(handoff::generate),
        )
        .route(
            "/conversations/{conversation_id}/handoff/prepare",
            post(handoff::prepare),
        )
        .route(
            "/conversations/{conversation_id}/prepared/restore",
            post(handoff::recovery::restore),
        )
        .route(
            "/conversations/candidate-review",
            get(candidate_review).post(create_candidate_review),
        )
        .route(
            "/conversations/{conversation_id}/messages",
            post(send_message),
        )
        .route(
            "/conversations/{conversation_id}/queue",
            post(queue::enqueue),
        )
        .route(
            "/conversations/{conversation_id}/queue/{item_id}/remove",
            post(queue::remove),
        )
        .route(
            "/conversations/{conversation_id}/queue/{item_id}/editor",
            post(queue::return_to_editor),
        )
        .route(
            "/conversations/{conversation_id}/cancel",
            post(cancel_message),
        )
        .route(
            "/conversations/{conversation_id}/runs/{run_id}/settle-partial",
            post(settle_partial),
        )
        .route(
            "/conversations/{conversation_id}/continue",
            post(continuation::resume),
        )
        .route(
            "/conversations/{conversation_id}/continue/end",
            post(continuation::end_pause),
        )
        .route(
            "/conversations/{conversation_id}/compact",
            post(compaction::compact),
        )
        .route(
            "/conversations/{conversation_id}/questions/answer",
            post(questions::answer),
        )
        .route(
            "/conversations/{conversation_id}/questions/cancel",
            post(questions::cancel),
        )
        .route(
            "/conversations/{conversation_id}/host-command/approve",
            post(tool_approval::approve),
        )
        .route(
            "/conversations/{conversation_id}/host-command/reject",
            post(tool_approval::reject),
        )
        .route(
            "/conversations/{conversation_id}/settings",
            post(settings::update),
        )
        .route(
            "/conversations/{conversation_id}/settings/defaults",
            post(settings::save_defaults),
        )
        .route(
            "/conversations/{conversation_id}/settings/host-consent",
            post(settings::request_host),
        )
        .route(
            "/conversations/{conversation_id}/settings/host-consent/approve",
            post(settings::approve_host),
        )
        .route(
            "/conversations/new/settings/host-consent",
            post(settings::request_host_draft),
        )
        .route(
            "/conversations/new/settings/host-consent/approve",
            post(settings::approve_host_draft),
        )
        .route(
            "/conversations/{conversation_id}/settings/environment",
            post(settings::preview_environment_switch),
        )
        .route(
            "/conversations/{conversation_id}/settings/presets/save",
            post(settings::save_preset),
        )
        .route(
            "/conversations/{conversation_id}/settings/presets/preview",
            post(settings::preview_preset),
        )
        .route(
            "/conversations/{conversation_id}/settings/presets/apply",
            post(settings::apply_preset),
        )
        .route(
            "/conversations/{conversation_id}/settings/environment/stop-and-switch",
            post(settings::stop_and_switch_environment),
        )
        .route(
            "/conversations/{conversation_id}/directories/pick",
            post(directories::pick_saved),
        )
        .route(
            "/conversations/{conversation_id}/directories/{grant_id}/consent",
            post(directories::request_saved),
        )
        .route(
            "/conversations/{conversation_id}/directories/consent",
            post(directories::approve_saved),
        )
        .route(
            "/conversations/{conversation_id}/directories/{grant_id}/access",
            post(directories::update_saved),
        )
        .route(
            "/conversations/{conversation_id}/directories/{grant_id}/remove",
            post(directories::remove_saved),
        )
        .route("/conversations/{conversation_id}/model", post(select_model))
        .route(
            "/conversations/{conversation_id}/network",
            post(set_network),
        )
        .route(
            "/conversations/{conversation_id}/rename",
            post(rename_conversation),
        )
        .route(
            "/conversations/{conversation_id}/delete",
            post(delete_conversation),
        )
}

#[derive(Deserialize)]
struct RenameForm {
    title: String,
    revision: String,
}

#[derive(Deserialize)]
struct RevisionForm {
    revision: String,
}

#[derive(Deserialize)]
struct MessageForm {
    revision: String,
    message: String,
    /// Frozen skill preview binding. An unpreviewed command sends both empty.
    #[serde(default, rename = "command_source")]
    command_source: String,
    #[serde(default, rename = "command_hash")]
    command_hash: String,
    #[serde(default)]
    revise_source: String,
    #[serde(default)]
    revise_parent: String,
    #[serde(default)]
    revise_active_leaf: String,
    #[serde(default, rename = "attachment_0")]
    attachment_0: String,
    #[serde(default, rename = "attachment_1")]
    attachment_1: String,
    #[serde(default, rename = "attachment_2")]
    attachment_2: String,
    #[serde(default, rename = "attachment_3")]
    attachment_3: String,
    #[serde(default, rename = "attachment_4")]
    attachment_4: String,
    #[serde(default, rename = "attachment_5")]
    attachment_5: String,
    #[serde(default, rename = "attachment_6")]
    attachment_6: String,
    #[serde(default, rename = "attachment_7")]
    attachment_7: String,
}

impl MessageForm {
    fn attachment_ids(&self) -> Result<Vec<crate::conversations::AttachmentId>, &'static str> {
        let mut ids = Vec::new();
        for value in [
            &self.attachment_0,
            &self.attachment_1,
            &self.attachment_2,
            &self.attachment_3,
            &self.attachment_4,
            &self.attachment_5,
            &self.attachment_6,
            &self.attachment_7,
        ] {
            if value.trim().is_empty() {
                continue;
            }
            let id = crate::conversations::attachments::parse_attachment_id(value)
                .ok_or("That staged image is not valid. Add it again.")?;
            if ids.contains(&id) {
                return Err("That staged image is not valid. Add it again.");
            }
            ids.push(id);
        }
        Ok(ids)
    }
}

impl MessageForm {
    /// Parse an optional replacement prompt. Every revision field must be
    /// present together; the store still validates each value against its own
    /// retained path before it appends anything.
    fn revision_request(&self) -> Result<Option<RevisionRequest>, &'static str> {
        let source = self.revise_source.trim();
        let parent = self.revise_parent.trim();
        let active_leaf = self.revise_active_leaf.trim();
        if source.is_empty() {
            if parent.is_empty() && active_leaf.is_empty() {
                return Ok(None);
            }
            return Err("That prompt revision is not valid.");
        }
        let source = MessageId::parse(source).ok_or("That prompt revision is not valid.")?;
        if active_leaf.is_empty() {
            return Err("That prompt revision is not valid.");
        }
        let active_leaf =
            MessageId::parse(active_leaf).ok_or("That prompt revision is not valid.")?;
        let parent = if parent.is_empty() {
            None
        } else {
            Some(MessageId::parse(parent).ok_or("That prompt revision is not valid.")?)
        };
        Ok(Some(RevisionRequest {
            source,
            parent,
            active_leaf: Some(active_leaf),
        }))
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CandidateReviewQuery {
    run: String,
    candidate: String,
    diff_base: String,
}

#[derive(Deserialize)]
struct CandidateReviewForm {
    run: String,
    candidate: String,
    diff_base: String,
    brief: String,
    provider: String,
    model: String,
    thinking: String,
    #[serde(default)]
    preset: String,
}

#[derive(Deserialize)]
struct ModelForm {
    revision: String,
    provider: String,
    model: String,
    #[serde(default)]
    thinking: String,
}

#[derive(Deserialize)]
struct NetworkForm {
    revision: String,
    network: String,
    #[serde(default)]
    network_domains: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CatalogueQuery {
    directory: String,
    q: String,
    conversation: String,
    cursor: String,
    index: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ObserveQuery {
    job: String,
    cursor: String,
    before: String,
    after: String,
    around: String,
    /// An explicit inspection leaf. The transcript follows its path without
    /// changing the active leaf.
    leaf: String,
    /// A retained user prompt to restore as a replacement draft. It opens the
    /// composer without a durable mutation or a model call.
    revise: String,
    historical: bool,
    #[serde(default)]
    title: bool,
}

impl ObserveQuery {
    fn transcript_cursor(&self) -> (Option<TranscriptCursor>, &'static str) {
        let positions = [
            ("before", self.before.trim()),
            ("after", self.after.trim()),
            ("around", self.around.trim()),
        ];
        let selected: Vec<_> = positions
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .collect();
        if selected.len() > 1 {
            return (None, "Choose one transcript position.");
        }
        let Some((position, value)) = selected.first() else {
            return (None, "");
        };
        let Some(id) = MessageId::parse(value) else {
            return (None, "That transcript position is not valid.");
        };
        let cursor = match *position {
            "before" => TranscriptCursor::Before(id),
            "after" => TranscriptCursor::After(id),
            _ => TranscriptCursor::Around(id),
        };
        (Some(cursor), "")
    }

    fn has_transcript_cursor(&self) -> bool {
        !self.before.trim().is_empty()
            || !self.after.trim().is_empty()
            || !self.around.trim().is_empty()
    }

    fn transcript_leaf(&self) -> (Option<MessageId>, &'static str) {
        let raw = self.leaf.trim();
        if raw.is_empty() {
            return (None, "");
        }
        match MessageId::parse(raw) {
            Some(leaf) => (Some(leaf), ""),
            None => (None, "That inspection position is not valid."),
        }
    }

    fn has_inspection_leaf(&self) -> bool {
        !self.leaf.trim().is_empty()
    }

    fn revision_source(&self) -> (Option<MessageId>, &'static str) {
        let raw = self.revise.trim();
        if raw.is_empty() {
            return (None, "");
        }
        match MessageId::parse(raw) {
            Some(source) => (Some(source), ""),
            None => (None, "That prompt revision is not valid."),
        }
    }

    fn has_revision(&self) -> bool {
        !self.revise.trim().is_empty()
    }
}

async fn catalogue(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Query(query): Query<CatalogueQuery>,
) -> AppResult<Response> {
    if query.index && graft == GraftRequest::Patch {
        return recent::response(&state);
    }
    let trimmed = query.q.trim();
    let valid_directory = query.directory.is_empty()
        || (query.directory.len() == 33
            && state
                .conversations
                .metadata()
                .iter()
                .flat_map(page::history_grants)
                .any(|grant| page::history_directory_key(grant) == query.directory));
    let cursor = parse_catalogue_cursor(&query.cursor);
    let error = if !valid_directory {
        "Choose a directory from conversation history."
    } else if trimmed.len() > 256 {
        "Search is too long. Use at most 256 characters."
    } else if !query.cursor.trim().is_empty() && cursor.is_none() {
        "That history position is not valid."
    } else {
        ""
    };
    // The optional conversation only selects the return destination, as on
    // the attention page. History always lists every matching conversation.
    let back = crate::conversations::ConversationId::parse(query.conversation.trim())
        .filter(|id| state.conversations.metadata_for(id).is_some());
    let (back_href, back_label) = match back {
        Some(id) => (
            format!("/conversations/{}", id.as_hex()),
            "Back to conversation",
        ),
        None => (String::new(), ""),
    };
    render_catalogue(
        &state,
        graft,
        &query.directory,
        trimmed,
        cursor,
        error,
        back_href,
        back_label,
    )
}

fn parse_catalogue_cursor(raw: &str) -> Option<(u64, crate::conversations::ConversationId)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (updated_at, id) = raw.split_once('-')?;
    Some((updated_at.parse().ok()?, ConversationId::parse(id)?))
}

async fn create(
    State(_state): State<AppState>,
    _session: RequiredSession,
    _graft: PatchGraft,
) -> AppResult<Response> {
    Ok(responses::command_navigation("/conversations/new"))
}

async fn detail(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    Path(conversation_id): Path<String>,
    Query(query): Query<ObserveQuery>,
) -> AppResult<Response> {
    let Some(conversation) = ConversationId::parse(&conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let (leaf, leaf_error) = query.transcript_leaf();
    if (query.has_transcript_cursor() || query.has_inspection_leaf())
        && (!query.job.is_empty() || !query.cursor.is_empty() || query.title || query.historical)
    {
        return render_transcript(
            &state,
            session.0,
            graft,
            conversation,
            None,
            leaf,
            "Observation cannot use a transcript position.",
        );
    }
    if query.has_revision() {
        if query.has_transcript_cursor()
            || !query.job.is_empty()
            || !query.cursor.is_empty()
            || query.title
            || query.historical
        {
            return render_transcript(
                &state,
                session.0,
                graft,
                conversation,
                None,
                None,
                "Observation cannot use a prompt revision.",
            );
        }
        let (source, source_error) = query.revision_source();
        let error = if source_error.is_empty() {
            leaf_error
        } else {
            source_error
        };
        return render_revision(&state, session.0, graft, conversation, source, leaf, error);
    }
    if graft == GraftRequest::Patch && query.title {
        let Some(record) = state.conversations.get(&conversation) else {
            return Ok(responses::request_navigation(graft, "/conversations"));
        };
        return title::response(&record);
    }
    // A transcript cursor and live observation are incompatible modes.
    if query.has_transcript_cursor() {
        let (cursor, cursor_error) = query.transcript_cursor();
        let error = if !cursor_error.is_empty() {
            cursor_error
        } else {
            leaf_error
        };
        return render_transcript(&state, session.0, graft, conversation, cursor, leaf, error);
    }
    // An inspection leaf without a cursor still opens its path, not the live tail.
    if !leaf_error.is_empty() {
        return render_transcript(
            &state,
            session.0,
            graft,
            conversation,
            None,
            None,
            leaf_error,
        );
    }
    if query.has_inspection_leaf() {
        return render_transcript(&state, session.0, graft, conversation, None, leaf, "");
    }
    if graft == GraftRequest::Patch && !query.job.is_empty() {
        return observe_message(state, session.0, conversation, query);
    }
    render_transcript(&state, session.0, graft, conversation, None, None, "")
}

/// Render one bounded transcript window without loading every retained body.
/// A foreign cursor falls back to the latest window and reports the rejection.
fn render_transcript(
    state: &AppState,
    session: crate::sessions::SessionId,
    graft: GraftRequest,
    conversation: ConversationId,
    cursor: Option<TranscriptCursor>,
    mut leaf: Option<MessageId>,
    error: &'static str,
) -> AppResult<Response> {
    let (record, window, error) =
        match state
            .conversations
            .transcript_window(&conversation, cursor, leaf)
        {
            Ok(Some((record, window))) => (record, window, error),
            Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
            Err(ConversationError::Entry) => {
                leaf = None;
                match state
                    .conversations
                    .transcript_window(&conversation, None, None)
                {
                    Ok(Some((record, window))) => {
                        (record, window, ConversationError::Entry.message())
                    }
                    Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
                    Err(error) => return Err(AppError::new("load transcript window", error)),
                }
            }
            Err(error) => return Err(AppError::new("load transcript window", error)),
        };
    let status = if error.is_empty() {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    let view = detail_view_with_transcript(
        state,
        session,
        &record,
        &record.title,
        error,
        Some(&window),
        leaf,
    );
    render_detail(state, session, graft, status, view)
}

/// Restore one retained user prompt as a replacement draft. The route is a
/// canonical GET: it writes no record, selects no branch and starts no model
/// call. Send performs the atomic append.
fn render_revision(
    state: &AppState,
    session: crate::sessions::SessionId,
    graft: GraftRequest,
    conversation: ConversationId,
    source: Option<MessageId>,
    leaf: Option<MessageId>,
    error: &'static str,
) -> AppResult<Response> {
    let (record, window, mut error) =
        match state
            .conversations
            .transcript_window(&conversation, None, leaf)
        {
            Ok(Some((record, window))) => (record, window, error),
            Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
            Err(ConversationError::Entry) => {
                match state
                    .conversations
                    .transcript_window(&conversation, None, None)
                {
                    Ok(Some((record, window))) => {
                        (record, window, ConversationError::Entry.message())
                    }
                    Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
                    Err(error) => return Err(AppError::new("load revision transcript", error)),
                }
            }
            Err(error) => return Err(AppError::new("load revision transcript", error)),
        };
    let draft = if error.is_empty() {
        match source {
            Some(source) => match state.conversations.revision_source(&conversation, source) {
                Ok(Some(revision)) => {
                    let active_leaf = state
                        .conversations
                        .metadata_for(&conversation)
                        .and_then(|metadata| metadata.active_leaf);
                    Some(RevisionDraft::from_source(
                        revision,
                        record.revision,
                        active_leaf,
                    ))
                }
                Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
                Err(entry) => {
                    error = entry.message();
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    let status = if error.is_empty() {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    let mut view = detail_view_with_transcript(
        state,
        session,
        &record,
        &record.title,
        error,
        Some(&window),
        leaf,
    );
    if let Some(draft) = draft {
        view = view.with_revision(draft);
    }
    render_detail(state, session, graft, status, view)
}

struct CandidateReviewSelection {
    run: WorkflowRun,
    candidate: crate::workflows::artefacts::ArtefactReference,
    diff_base: crate::workflows::artefacts::ArtefactReference,
    candidate_hash: crate::workflows::artefacts::CandidateHash,
    diff_base_hash: crate::workflows::artefacts::CandidateHash,
    preview: String,
}

async fn candidate_review(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Query(query): Query<CandidateReviewQuery>,
) -> AppResult<Response> {
    let selection =
        match resolve_candidate_review(&state, &query.run, &query.candidate, &query.diff_base) {
            Ok(selection) => selection,
            Err(error) => return render_candidate_review_error(&state, graft, error),
        };
    render_candidate_review(&state, graft, PatchStatus::Ok, &selection, "", None, None)
}

async fn create_candidate_review(
    State(state): State<AppState>,
    RequiredSession(session): RequiredSession,
    graft: PatchGraft,
    Form(form): Form<CandidateReviewForm>,
) -> AppResult<Response> {
    let selection =
        match resolve_candidate_review(&state, &form.run, &form.candidate, &form.diff_base) {
            Ok(selection) => selection,
            Err(error) => return render_candidate_review_error(&state, graft, error),
        };
    let source = selection
        .run
        .conversation_id
        .and_then(|id| state.conversations.get(&id));
    let model = match candidate_review_model(&state, source.as_ref(), &form) {
        Ok(model) => model,
        Err(error) => {
            return render_candidate_review(
                &state,
                graft,
                PatchStatus::UnprocessableEntity,
                &selection,
                &form.brief,
                Some(error),
                Some(&form),
            );
        }
    };
    let Some(connection) = state.vault.connection_for(&model.settings.model) else {
        return render_candidate_review(
            &state,
            graft,
            PatchStatus::UnprocessableEntity,
            &selection,
            &form.brief,
            Some("Choose a stored provider."),
            Some(&form),
        );
    };
    let secret = match connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
        crate::providers::AuthMethod::Plan => None,
    };
    if let Err(error) = job::validate_candidate_review(
        &state,
        &selection.run,
        &selection.candidate,
        &selection.diff_base,
        secret,
    ) {
        return render_candidate_review(
            &state,
            graft,
            PatchStatus::Conflict,
            &selection,
            &form.brief,
            Some(error),
            Some(&form),
        );
    }
    let source_at_safe_gate = source_at_safe_gate(
        &state,
        &selection.run,
        &selection.candidate,
        &selection.diff_base,
        &session,
    );
    if source
        .as_ref()
        .is_some_and(|record| record.active_job.is_some())
        && !source_at_safe_gate
    {
        return render_candidate_review(
            &state,
            graft,
            PatchStatus::Conflict,
            &selection,
            &form.brief,
            Some("The source run is active. Start this review after it reaches a safe gate."),
            Some(&form),
        );
    }
    let review = match state
        .conversations
        .create_candidate_review(CandidateReviewCreation {
            source_conversation: source.as_ref().map(|record| (record.id, record.revision)),
            title: candidate_review_title(&selection.run),
            model: model.clone(),
            run_id: selection.run.id,
            candidate: selection.candidate.clone(),
            diff_base: selection.diff_base.clone(),
            task_brief: form.brief.clone(),
            source_at_safe_gate,
        }) {
        Ok(review) => review,
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            return Err(AppError::new("store candidate review conversation", error));
        }
        Err(error) => {
            return render_candidate_review(
                &state,
                graft,
                status_for(error),
                &selection,
                &form.brief,
                Some(error.message()),
                Some(&form),
            );
        }
    };
    match start_message(
        &state,
        session,
        review.clone(),
        review.revision,
        model,
        form.brief,
        None,
        Vec::new(),
        String::new(),
    )
    .await
    {
        Ok(_) => Ok(responses::command_navigation(&conversation_path(&review))),
        Err(StartMessageError::Internal(error)) => Err(error),
        Err(StartMessageError::User(status, error)) => {
            let review = state.conversations.get(&review.id).unwrap_or(review);
            render_detail(
                &state,
                session,
                graft.into(),
                status,
                detail_view(&state, session, &review, &review.title, error),
            )
        }
    }
}

fn resolve_candidate_review(
    state: &AppState,
    raw_run: &str,
    raw_candidate: &str,
    raw_diff_base: &str,
) -> Result<CandidateReviewSelection, &'static str> {
    let run_id = workflows::RunId::parse(raw_run).ok_or("Choose an available source run.")?;
    let candidate_id = workflows::ArtefactId::parse(raw_candidate)
        .ok_or("Choose an available candidate artefact.")?;
    let diff_base_id = workflows::ArtefactId::parse(raw_diff_base)
        .ok_or("Choose an available diff base artefact.")?;
    let run = state
        .workflow_runs
        .get(&run_id)
        .ok_or("The source run is unavailable.")?;
    let candidate = run
        .artefact(&candidate_id)
        .filter(|record| record.kind == workflows::definition::ArtefactKind::CandidateRevision)
        .ok_or("The selected candidate is unavailable.")?;
    let diff_base = run
        .artefact(&diff_base_id)
        .filter(|record| record.kind == workflows::definition::ArtefactKind::CandidateRevision)
        .ok_or("The selected diff base is unavailable.")?;
    let candidate_reference = crate::workflows::artefacts::ArtefactReference {
        id: candidate.id,
        kind: candidate.kind,
        artefact_hash: candidate.artefact_hash,
    };
    let diff_base_reference = crate::workflows::artefacts::ArtefactReference {
        id: diff_base.id,
        kind: diff_base.kind,
        artefact_hash: diff_base.artefact_hash,
    };
    let diff = crate::workflows::artefacts::CandidateDiff::load(
        &run,
        &diff_base_reference,
        &candidate_reference,
        &state.workflow_artefacts,
    )
    .map_err(|_| "The selected immutable candidate or diff base is unavailable.")?;
    let preview = candidate_review_preview(&diff, &state.workflow_artefacts)?;
    Ok(CandidateReviewSelection {
        run,
        candidate: candidate_reference,
        diff_base: diff_base_reference,
        candidate_hash: diff.target,
        diff_base_hash: diff.base,
        preview,
    })
}

fn candidate_review_preview(
    diff: &crate::workflows::artefacts::CandidateDiff,
    store: &crate::workflows::artefacts::WorkflowArtefactRepository,
) -> Result<String, &'static str> {
    const MAXIMUM_REVIEW_PREVIEW_BYTES: usize = 256 * 1024;
    let (total, _) = diff
        .manifest_page(0, 0)
        .map_err(|_| "The selected candidate diff is unavailable.")?;
    if total > crate::workflows::artefacts::candidate::MAXIMUM_PREVIEW_PATHS {
        return Err(
            "The selected candidate diff preview is too large. Choose a smaller candidate.",
        );
    }
    let mut preview = String::from("Changed paths:\n");
    for index in 0..total {
        let change = diff
            .change(index, store)
            .map_err(|_| "The selected candidate diff is unavailable.")?;
        preview.push_str("- ");
        preview.push_str(change.status);
        preview.push(' ');
        if !change.directory.is_empty() {
            preview.push_str(&change.directory);
            preview.push('/');
        }
        preview.push_str(&change.path);
        preview.push('\n');
        for (side, facts) in [("Before", &change.old), ("After", &change.new)] {
            if let Some(facts) = facts {
                use std::fmt::Write;
                let _ = writeln!(
                    preview,
                    "{side}: {}, executable: {}, {}",
                    facts.kind, facts.executable, facts.detail,
                );
            }
        }
        if let Some(text) = change.text {
            for fragment in text {
                preview.push_str(&fragment.text);
            }
        } else if change.binary {
            preview.push_str("Binary content is not shown.\n");
        } else if change.text_too_large {
            return Err(
                "The selected candidate diff preview is too large. Choose a smaller candidate.",
            );
        }
        if preview.len() > MAXIMUM_REVIEW_PREVIEW_BYTES {
            return Err(
                "The selected candidate diff preview is too large. Choose a smaller candidate.",
            );
        }
    }
    if crate::markdown::escape_plain(&preview).len() > MAXIMUM_REVIEW_PREVIEW_BYTES {
        return Err(
            "The selected candidate diff preview is too large. Choose a smaller candidate.",
        );
    }
    Ok(preview)
}

fn source_at_safe_gate(
    state: &AppState,
    run: &WorkflowRun,
    candidate: &crate::workflows::artefacts::ArtefactReference,
    diff_base: &crate::workflows::artefacts::ArtefactReference,
    session: &crate::sessions::SessionId,
) -> bool {
    let Some(gate) = run
        .gates
        .iter()
        .rev()
        .find(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)
    else {
        return false;
    };
    matches!(run.state, workflows::run::RunState::AwaitingHuman { .. })
        && gate.candidate == *candidate
        && gate.diff_base == *diff_base
        && state.gate_continuations.available(&run.id, session)
}

fn candidate_review_title(run: &WorkflowRun) -> String {
    let prefix = "Review candidate: ";
    let remaining = crate::conversations::MAXIMUM_TITLE_BYTES - prefix.len();
    let source = run.pinned.definition.name();
    let end = source.floor_char_boundary(remaining.min(source.len()));
    format!("{prefix}{}", &source[..end])
}

fn candidate_review_model(
    state: &AppState,
    source: Option<&ConversationRecord>,
    form: &CandidateReviewForm,
) -> Result<ConversationModelConfiguration, &'static str> {
    let selection = if form.preset.trim().is_empty() {
        candidate_submitted_selection(state, form)?
    } else {
        let preset = AgentId::parse(form.preset.trim())
            .and_then(|id| state.agents.get(&id))
            .ok_or("Choose an available reviewer preset.")?;
        preset
            .selection
            .clone()
            .or_else(|| candidate_submitted_selection(state, form).ok())
            .or_else(|| {
                source.and_then(|record| {
                    effective_model(state, record).map(|model| model.settings.model)
                })
            })
            .ok_or("Choose a model before you start this review.")?
    };
    valid_selection(state, &selection)?;
    let environment = source
        .and_then(|record| record.model.as_ref())
        .map(|model| model.settings.environment)
        .or_else(|| default_environment(state).ok())
        .ok_or("The starter environment is unavailable.")?;
    if form.preset.trim().is_empty() {
        Ok(ConversationModelConfiguration::direct(
            selection,
            environment,
        ))
    } else {
        let preset = AgentId::parse(form.preset.trim())
            .and_then(|id| state.agents.get(&id))
            .ok_or("Choose an available reviewer preset.")?;
        Ok(ConversationModelConfiguration::from_agent_snapshot(
            &preset,
            selection,
            environment,
        ))
    }
}

fn candidate_submitted_selection(
    state: &AppState,
    form: &CandidateReviewForm,
) -> Result<ModelSelection, &'static str> {
    let provider = ProviderKind::parse(form.provider.trim()).ok_or("Choose a stored provider.")?;
    let thinking = if form.thinking.trim().is_empty() {
        None
    } else {
        Some(
            ThinkingEffort::new(form.thinking.clone())
                .ok_or("Choose an available thinking effort.")?,
        )
    };
    let selection = ModelSelection::new(provider, form.model.clone(), thinking)
        .ok_or("Enter a valid model name.")?;
    valid_selection(state, &selection)?;
    Ok(selection)
}

fn candidate_review_view_model(
    state: &AppState,
    source: Option<&ConversationRecord>,
    form: Option<&CandidateReviewForm>,
) -> (ModelPicker, Vec<PresetOption>, String) {
    let (provider, model, thinking) = if let Some(form) = form {
        // Requested values stay visible so validation errors never silently
        // substitute another model. The catalogue picker marks unknown values
        // unavailable and the POST rejects them again.
        (
            form.provider.clone(),
            form.model.clone(),
            form.thinking.clone(),
        )
    } else {
        let selection = source
            .and_then(|record| effective_model(state, record).map(|model| model.settings.model))
            .or_else(|| {
                state
                    .preferences
                    .desk_providers(&state.vault)
                    .into_iter()
                    .find(|provider| provider.selected)
                    .map(|provider| ModelSelection {
                        provider: provider.kind,
                        model: provider.model.clone(),
                        thinking: state.models_dev.effective_effort(
                            provider.kind,
                            &provider.model,
                            provider.thinking.as_ref(),
                        ),
                    })
            });
        (
            selection
                .as_ref()
                .map_or(String::new(), |item| item.provider.as_str().to_owned()),
            selection
                .as_ref()
                .map_or(String::new(), |item| item.model.clone()),
            selection
                .as_ref()
                .and_then(|item| item.thinking.as_ref())
                .map_or(String::new(), |effort| effort.as_str().to_owned()),
        )
    };
    let picker = ModelPicker::new(
        &state.vault,
        &state.preferences,
        &state.models_dev,
        &provider,
        &model,
        &thinking,
    );
    let selected_preset = form.map(|form| form.preset.trim()).unwrap_or_default();
    let presets = state
        .agents
        .list()
        .into_iter()
        .map(|agent| PresetOption {
            id: agent.id.as_hex(),
            name: agent.name.clone(),
            description: agent.selection.as_ref().map_or_else(
                || "Keep the selected direct model".to_owned(),
                |item| format!("{} · {}", item.provider.label(), item.model),
            ),
            selected: agent.id.as_hex() == selected_preset,
        })
        .collect();
    let summary = if let Some(form) = form.filter(|form| !form.preset.trim().is_empty()) {
        state
            .agents
            .list()
            .into_iter()
            .find(|agent| agent.id.as_hex() == form.preset.trim())
            .map_or_else(
                || "Reviewer preset is unavailable".to_owned(),
                |agent| format!("Preset: {}", agent.name),
            )
    } else {
        form.and_then(|form| candidate_submitted_selection(state, form).ok())
            .or_else(|| {
                source.and_then(|record| {
                    effective_model(state, record).map(|model| model.settings.model)
                })
            })
            .map_or_else(
                || "Choose a stored provider and model".to_owned(),
                |item| {
                    format!(
                        "Direct model: {} · {}{}",
                        item.provider.label(),
                        item.model,
                        item.thinking
                            .as_ref()
                            .map(|effort| format!(" · Thinking: {}", effort.label()))
                            .unwrap_or_default()
                    )
                },
            )
    };
    (picker, presets, summary)
}

fn default_candidate_review_brief() -> &'static str {
    "Review this candidate for correctness, risks, missing tests and unintended changes. Return findings and recommendations. Do not approve or apply the candidate."
}

fn render_candidate_review(
    state: &AppState,
    graft: impl Into<GraftRequest>,
    status: PatchStatus,
    selection: &CandidateReviewSelection,
    brief: &str,
    error: Option<&'static str>,
    form: Option<&CandidateReviewForm>,
) -> AppResult<Response> {
    let graft = graft.into();
    let source = selection
        .run
        .conversation_id
        .and_then(|id| state.conversations.get(&id));
    let (model_picker, presets, reviewer_summary) =
        candidate_review_view_model(state, source.as_ref(), form);
    let source_title = source.as_ref().map_or_else(
        || selection.run.pinned.definition.name().to_owned(),
        |record| record.title.clone(),
    );
    let view = CandidateReviewView {
        run_id: selection.run.id.as_hex(),
        source_title,
        candidate_id: selection.candidate.id.as_hex(),
        diff_base_id: selection.diff_base.id.as_hex(),
        candidate_hash: selection.candidate_hash.as_str(),
        diff_base_hash: selection.diff_base_hash.as_str(),
        preview: selection.preview.clone(),
        instructions_summary: "Automatic project instructions come from the selected candidate's root AGENTS.md. This discussion receives the diff and root instructions without filesystem tools. The current host worktree is not used.".to_owned(),
        brief: if brief.is_empty() { default_candidate_review_brief().to_owned() } else { brief.to_owned() },
        reviewer_summary,
        model_picker,
        presets,
        error: error.unwrap_or(""),
    };
    match graft {
        GraftRequest::Document => {
            let mut response =
                responses::chat_page_response("Review candidate | Power Plant", state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            "Review candidate | Power Plant",
            "chat-main",
            &view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::PatchSet::new()
            .title("Review candidate | Power Plant")
            .with_children("candidate-review-detail", &view.contents())?
            .respond(status)?),
    }
}

fn render_candidate_review_error(
    state: &AppState,
    graft: impl Into<GraftRequest>,
    message: &'static str,
) -> AppResult<Response> {
    let graft = graft.into();
    #[derive(askama::Template)]
    #[template(
        source = "<main data-section=\"conversations\" class=\"mx-auto max-w-4xl p-8\"><div role=\"alert\" class=\"alert alert-error\">{{ message }}</div><a href=\"/runs\" data-graft class=\"btn btn-ghost mt-4\">Runs</a></main>",
        ext = "html"
    )]
    struct ErrorView {
        message: &'static str,
    }
    let view = ErrorView { message };
    match graft {
        GraftRequest::Document => {
            let mut response =
                responses::chat_page_response("Review candidate | Power Plant", state, &view)?;
            responses::apply_patch_status(&mut response, PatchStatus::Conflict);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            "Review candidate | Power Plant",
            "chat-main",
            &view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::PatchSet::new()
            .title("Review candidate | Power Plant")
            .with_children("chat-main", &view)?
            .respond(PatchStatus::Conflict)?),
    }
}

async fn send_message(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<MessageForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    // Direct command syntax is classified from the original text before any
    // resource expansion. It never reaches provider validation.
    if let Some(direct) = crate::conversations::input::direct_command(&form.message) {
        if !matches!(form.revision_request(), Ok(None)) {
            return render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    &state,
                    session.0,
                    &record,
                    &record.title,
                    "Cancel prompt revision before a direct command.",
                ),
            );
        }
        let Some(model) = effective_model(&state, &record) else {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(
                    &state,
                    session.0,
                    &record,
                    &record.title,
                    "Direct commands need conversation execution settings.",
                ),
            );
        };
        let attachments = match form.attachment_ids() {
            Ok(attachments) => attachments,
            Err(error) => {
                return render_detail_command(
                    graft,
                    PatchStatus::UnprocessableEntity,
                    detail_view(&state, session.0, &record, &record.title, error),
                );
            }
        };
        return match commands::run::start_saved(
            &state,
            session.0,
            &record,
            revision,
            model,
            direct,
            attachments,
        )
        .await
        {
            Ok(started) => render_detail_command(
                graft,
                PatchStatus::Ok,
                detail_view(&state, session.0, &started, &started.title, ""),
            ),
            Err(StartMessageError::Internal(error)) => Err(error),
            Err(StartMessageError::User(status, error)) => render_detail_command(
                graft,
                status,
                detail_view(&state, session.0, &record, &record.title, error),
            ),
        };
    }
    let Some(model) = effective_model(&state, &record) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                ConversationError::Selection.message(),
            ),
        );
    };
    let catalogue = commands::catalogue_for_record(
        &state,
        session.0,
        record.id,
        record.revision,
        record.model.as_ref(),
    );
    let secret = provider_secret(&state, &model.settings.model);
    let expansion = match commands::expand(
        &form.message,
        &catalogue,
        secret.as_deref(),
        commands::preview_binding(&form.command_source, &form.command_hash),
    ) {
        Ok(expansion) => expansion,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(&state, session.0, &record, &record.title, error.message()),
            );
        }
    };
    let message = expansion.expanded;
    let input = expansion.provenance;
    let revise = match form.revision_request() {
        Ok(revise) => revise,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(&state, session.0, &record, &record.title, error),
            );
        }
    };
    let attachments = match form.attachment_ids() {
        Ok(attachments) => attachments,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(&state, session.0, &record, &record.title, error),
            );
        }
    };
    let result = match revise {
        Some(revise) => {
            start_revision(
                &state,
                session.0,
                record.clone(),
                revision,
                model,
                message,
                input,
                revise,
                attachments,
            )
            .await
        }
        None => {
            start_message(
                &state,
                session.0,
                record.clone(),
                revision,
                model,
                message,
                input,
                attachments,
                record.id.as_hex(),
            )
            .await
        }
    };
    match result {
        Ok(started) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &started, &started.title, ""),
        ),
        Err(StartMessageError::Internal(error)) => Err(error),
        Err(StartMessageError::User(status, error)) => render_detail_command(
            graft,
            status,
            detail_view(&state, session.0, &record, &record.title, error),
        ),
    }
}

pub(super) enum StartMessageError {
    User(PatchStatus, &'static str),
    Internal(AppError),
}

pub(super) async fn preflight_execution(
    state: &AppState,
    session: crate::sessions::SessionId,
    conversation: Option<ConversationId>,
    model: &ConversationModelConfiguration,
) -> Result<(), StartMessageError> {
    if conversation
        .and_then(|id| state.conversations.get(&id))
        .is_some_and(|record| {
            record.messages.iter().any(|message| {
                message.command.as_ref().is_some_and(|entry| {
                    entry.before.is_some()
                        && entry.after.is_none()
                        && (entry.output.is_some()
                            || message.status == crate::conversations::MessageStatus::Interrupted)
                })
            })
        })
    {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "A direct command has no final file snapshot. Its effects remain uncertain. Use a separate conversation after local recovery.",
        ));
    }
    if conversation.is_some_and(|id| has_pending_review(state, id)) {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "Restore or resolve the pending decision before another message.",
        ));
    }
    if conversation.is_some_and(|id| {
        state.conversation_runtime.unsettled(id) || has_uncertain_application(state, id)
    }) {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "Execution remains unsettled. Continuation and retry stay unavailable until recovery and cleanup finish.",
        ));
    }
    if model.settings.location == crate::execution::ToolLocation::Sandbox
        && let Some(conversation) = conversation
        && model.settings.directories.iter().any(|grant| {
            grant.requires_access_consent(state.local_data.root())
                && (!state.sessions.contains_live(&session)
                    || (!state.access_consent.authorised_conversation(
                        session,
                        conversation,
                        &model.settings,
                        grant,
                    ) && !state.conversations.directory_approved(
                        &conversation,
                        &model.settings,
                        grant,
                    )))
        })
    {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "Directory access needs explicit approval. Open Directories to approve it.",
        ));
    }
    if model.settings.location == crate::execution::ToolLocation::Host {
        if model.settings.host_tools() {
            for grant in &model.settings.directories {
                grant.revalidate().map_err(|_| {
                    StartMessageError::User(
                        PatchStatus::UnprocessableEntity,
                        "A work location changed or is not available.",
                    )
                })?;
            }
        }
        if model.settings.host_tools()
            && let Some(conversation) = conversation
            && (!state.sessions.contains_live(&session)
                || !state.access_consent.authorised_host_conversation(
                    session,
                    conversation,
                    &model.settings,
                ))
        {
            return Err(StartMessageError::User(
                PatchStatus::UnprocessableEntity,
                "Unrestricted host access needs explicit approval. Open Settings to approve it.",
            ));
        }
        return Ok(());
    }
    if model.settings.tools.is_empty() {
        return Ok(());
    }
    crate::execution::ProjectFreeAuthority::from_settings(1, &model.settings)
        .map_err(|error| StartMessageError::User(PatchStatus::Conflict, error.message()))?;
    if let Some(missing) = state.sandboxes.missing() {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            missing.message(),
        ));
    }
    if crate::execution::ordinary_kind(&model.settings)
        == Some(crate::execution::OrdinaryKind::Sandbox)
    {
        return workflows::validate_replacement_environment(
            &state.environments,
            &state.environment_snapshots,
            model.settings.environment,
        )
        .await
        .map_err(|error| {
            StartMessageError::User(PatchStatus::UnprocessableEntity, error.message())
        });
    }
    let pinned = workflows::pin_agent_work(&model.settings).map_err(|error| {
        StartMessageError::User(PatchStatus::UnprocessableEntity, error.message())
    })?;
    workflows::resolve_environments(
        &pinned.definition,
        &state.environments,
        &state.environment_snapshots,
    )
    .await
    .map(|_| ())
    .map_err(|error| StartMessageError::User(PatchStatus::UnprocessableEntity, error.message()))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn start_message(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: ConversationRecord,
    revision: u32,
    model: ConversationModelConfiguration,
    text: String,
    input: Option<crate::conversations::InputProvenance>,
    attachments: Vec<crate::conversations::AttachmentId>,
    attachment_scope: String,
) -> Result<ConversationRecord, StartMessageError> {
    start_message_mode(
        state,
        session,
        record,
        revision,
        model,
        text,
        input,
        attachments,
        attachment_scope,
        MessageAppend::Ordinary,
    )
    .await
}

/// Choose which durable exchange an ordinary model request appends.
enum MessageAppend {
    Ordinary,
    FollowUp(crate::conversations::QueueItemId, u32),
    Revision(RevisionRequest),
}

/// Append a replacement prompt as a new branch through the ordinary model
/// path. Preflight, workflow selection and settlement are shared with an
/// ordinary send; only the durable append differs.
#[allow(clippy::too_many_arguments)]
async fn start_revision(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: ConversationRecord,
    revision: u32,
    model: ConversationModelConfiguration,
    text: String,
    input: Option<crate::conversations::InputProvenance>,
    revise: RevisionRequest,
    attachments: Vec<crate::conversations::AttachmentId>,
) -> Result<ConversationRecord, StartMessageError> {
    let scope = record.id.as_hex();
    start_message_mode(
        state,
        session,
        record,
        revision,
        model,
        text,
        input,
        attachments,
        scope,
        MessageAppend::Revision(revise),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn start_message_mode(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: ConversationRecord,
    revision: u32,
    model: ConversationModelConfiguration,
    text: String,
    input: Option<crate::conversations::InputProvenance>,
    attachments: Vec<crate::conversations::AttachmentId>,
    attachment_scope: String,
    append: MessageAppend,
) -> Result<ConversationRecord, StartMessageError> {
    let persisted_model = model.clone();
    // Reject file access for immutable-evidence conversations before any runtime
    // preflight, so the specific reason reaches the user first. A fork of a
    // candidate review keeps the same restriction without candidate ownership.
    let immutable_review = record.candidate_review_context.is_some()
        || record
            .forked_from
            .as_ref()
            .is_some_and(|provenance| provenance.candidate_review);
    if immutable_review
        && (!model.settings.tools.is_empty() || !model.settings.directories.is_empty())
    {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "Candidate reviews use immutable evidence only. Use a separate conversation for file access.",
        ));
    }
    preflight_execution(state, session, Some(record.id), &model).await?;
    if record.active_job.is_some() {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            ConversationError::Active.message(),
        ));
    }
    if record.continuation.is_some() {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "This conversation is paused. Continue or end the pause first.",
        ));
    }
    if let Err(error) = valid_selection(state, &model.settings.model) {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            error,
        ));
    }
    let Some(connection) = state.vault.connection_for(&model.settings.model) else {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "Choose a stored provider.",
        ));
    };
    if !attachments.is_empty() {
        let selection = &model.settings.model;
        match state
            .models_dev
            .supports_images(selection.provider, &selection.model)
        {
            Some(true) => {}
            Some(false) => {
                return Err(StartMessageError::User(
                    PatchStatus::UnprocessableEntity,
                    "The selected model does not accept images. Choose a model with image input.",
                ));
            }
            None => {
                return Err(StartMessageError::User(
                    PatchStatus::UnprocessableEntity,
                    "Power Plant cannot confirm image input for the selected model. Choose a model with image input.",
                ));
            }
        }
    }
    let ordinary = crate::execution::ordinary_kind(&model.settings);
    let workflow = if ordinary == Some(crate::execution::OrdinaryKind::FileChange) {
        let project_free =
            crate::execution::ProjectFreeAuthority::from_settings(record.revision, &model.settings)
                .map_err(|error| StartMessageError::User(PatchStatus::Conflict, error.message()))?;
        let pinned = workflows::pin_agent_work(&model.settings).map_err(|error| {
            StartMessageError::User(PatchStatus::UnprocessableEntity, error.message())
        })?;
        let environments = workflows::resolve_environments(
            &pinned.definition,
            &state.environments,
            &state.environment_snapshots,
        )
        .await
        .map_err(|error| {
            StartMessageError::User(PatchStatus::UnprocessableEntity, error.message())
        })?;
        let execution = state
            .workflow_execution
            .acquire()
            .map_err(|error| StartMessageError::User(PatchStatus::Conflict, error))?;
        let run_id = workflows::RunId::generate().map_err(|error| {
            StartMessageError::Internal(AppError::new("create workflow run identifier", error))
        })?;
        Some((run_id, project_free, pinned, environments, execution))
    } else {
        None
    };
    let ordinary_execution = if matches!(
        ordinary,
        Some(crate::execution::OrdinaryKind::Host | crate::execution::OrdinaryKind::Sandbox)
    ) {
        Some(
            state
                .workflow_execution
                .acquire()
                .map_err(|error| StartMessageError::User(PatchStatus::Conflict, error))?,
        )
    } else {
        None
    };
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .map_err(|_| {
            StartMessageError::User(PatchStatus::Conflict, ConversationError::Active.message())
        })?;
    let launch_brief = text.trim().to_owned();
    let phase_model = model.clone();
    let started = match append {
        MessageAppend::Revision(revise) => state.conversations.revise_message_with_model(
            &record.id,
            revision,
            revise,
            Some(persisted_model),
            job.id(),
            text,
            input,
            session,
            attachments,
        ),
        MessageAppend::FollowUp(item_id, queue_revision) => state.conversations.begin_follow_up(
            &record.id,
            revision,
            queue_revision,
            item_id,
            job.id(),
            Some(persisted_model),
        ),
        MessageAppend::Ordinary => state.conversations.begin_message_with_attachments(
            &record.id,
            revision,
            Some(persisted_model),
            job.id(),
            text,
            input,
            Some(session),
            &attachment_scope,
            attachments,
        ),
    };
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            state
                .sessions
                .finish_conversation_job(&session, record.id, job.id());
            return Err(StartMessageError::User(status_for(error), error.message()));
        }
    };
    if let Some((run_id, project_free, pinned, environments, execution)) = workflow {
        let secret = match connection.auth {
            crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
            crate::providers::AuthMethod::Plan => None,
        };
        let turns = match job::history_with_review(state, &started, secret) {
            Ok(turns) => turns,
            Err(error) => {
                let _ = state.conversations.settle_message(
                    &started.id,
                    job.id(),
                    String::new(),
                    crate::conversations::MessageStatus::Failed,
                    Some(error.to_owned()),
                );
                let _ = state
                    .sessions
                    .finish_conversation_job(&session, started.id, job.id());
                return Err(StartMessageError::User(
                    PatchStatus::UnprocessableEntity,
                    error,
                ));
            }
        };
        let phase_models = pinned
            .definition
            .steps()
            .iter()
            .filter(|step| matches!(&step.action, workflows::definition::StepAction::Agent(_)))
            .map(|step| workflows::PhaseModelSelection {
                step: step.key.clone(),
                selection: phase_model.settings.model.clone(),
                instructions: phase_model.settings.instructions.clone(),
                preset: phase_model
                    .preset
                    .as_ref()
                    .map(|preset| workflows::PinnedPreset {
                        id: preset.id,
                        revision: preset.revision,
                        name: preset.name.clone(),
                    }),
                settings: Some(phase_model.settings.clone()),
            })
            .collect::<Vec<_>>();
        let mut run = WorkflowRun::create_source_free_for_conversation(
            run_id,
            workflows::now_ms(),
            started.id,
            pinned,
            environments,
            phase_models,
        );
        run.launch_brief = launch_brief;
        if let Err(error) = state.workflow_runs.create(run.clone()) {
            let _ = state.conversations.settle_message(
                &started.id,
                job.id(),
                String::new(),
                crate::conversations::MessageStatus::Failed,
                None,
            );
            let _ = state
                .sessions
                .finish_conversation_job(&session, started.id, job.id());
            return Err(StartMessageError::Internal(AppError::new(
                "store workflow run",
                error,
            )));
        }
        spawn_conversation_work(
            state.clone(),
            session,
            started.id,
            crate::execution::conversation::run(
                state.clone(),
                crate::execution::conversation::OrdinaryRun {
                    session,
                    record: started,
                    connection,
                    job,
                    execution,
                    kind: crate::execution::OrdinaryKind::FileChange,
                    turns,
                    file: Some(crate::execution::conversation::FileChangeWork {
                        run_id,
                        project_free,
                    }),
                },
            ),
        );
    } else if let Some(kind) = ordinary {
        let secret = match connection.auth {
            crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose()),
            crate::providers::AuthMethod::Plan => None,
        };
        let turns = match job::history_with_review(state, &started, secret) {
            Ok(turns) => turns,
            Err(error) => {
                let _ = state.conversations.settle_message(
                    &started.id,
                    job.id(),
                    String::new(),
                    crate::conversations::MessageStatus::Failed,
                    Some(error.to_owned()),
                );
                let _ = state
                    .sessions
                    .finish_conversation_job(&session, started.id, job.id());
                return Err(StartMessageError::User(
                    PatchStatus::UnprocessableEntity,
                    error,
                ));
            }
        };
        let execution = ordinary_execution.expect("ordinary work holds reset protection");
        spawn_conversation_work(
            state.clone(),
            session,
            started.id,
            crate::execution::conversation::run(
                state.clone(),
                crate::execution::conversation::OrdinaryRun {
                    session,
                    record: started,
                    connection,
                    job,
                    execution,
                    kind,
                    turns,
                    file: None,
                },
            ),
        );
    } else {
        spawn_conversation_work(
            state.clone(),
            session,
            started.id,
            job::run(state.clone(), session, started.id, started, connection, job),
        );
    }
    Ok(state.conversations.get(&record.id).unwrap_or(record))
}

pub(super) fn spawn_conversation_work<F>(
    state: AppState,
    session: crate::sessions::SessionId,
    conversation: ConversationId,
    work: F,
) where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        work.await;
        continue_follow_ups(state, session, conversation).await;
    });
}

pub(crate) async fn continue_follow_ups(
    state: AppState,
    session: crate::sessions::SessionId,
    conversation: ConversationId,
) {
    if !state.sessions.contains_live(&session) {
        return;
    }
    let Some(record) = state.conversations.get(&conversation) else {
        return;
    };
    if record.active_job.is_some() || state.sessions.conversation_reserved(conversation) {
        return;
    }
    if has_pending_review(&state, conversation)
        || state.conversation_runtime.unsettled(conversation)
        || has_uncertain_application(&state, conversation)
        || record.continuation.is_some()
    {
        return;
    }
    let last_assistant = record
        .messages
        .iter()
        .rev()
        .find(|message| message.role == crate::conversations::MessageRole::Assistant);
    if !last_assistant
        .is_some_and(|message| message.status == crate::conversations::MessageStatus::Complete)
    {
        return;
    }
    let Some(item) = record.queue.next_follow_up().cloned() else {
        return;
    };
    let Some(model) = record.model.clone() else {
        return;
    };
    if item.settings_digest != crate::conversations::queue::launch_digest(&model.settings) {
        return;
    }
    let revision = record.revision;
    let queue_revision = record.queue.revision;
    let _ = start_message_mode(
        &state,
        session,
        record,
        revision,
        model,
        item.text,
        None,
        Vec::new(),
        String::new(),
        MessageAppend::FollowUp(item.id, queue_revision),
    )
    .await;
}

async fn cancel_message(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ObserveQuery>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(job_id) = JobId::parse(&form.job) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(job) = state.sessions.conversation_job(record.id, job_id) else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "This reply is no longer active.",
            ),
        );
    };
    job.request_cancel();
    state.host_approvals.invalidate_job(job.id());
    state.conversations.invalidate_questions_for_job(job.id());
    render_detail_command(
        graft,
        PatchStatus::Ok,
        detail_view(&state, session.0, &record, &record.title, ""),
    )
}

#[derive(Deserialize)]
struct SettlePartialForm {
    #[serde(default)]
    attempt: String,
    #[serde(default)]
    state: String,
}

/// Keep applied files and end a known partial task without another write.
/// Every command revalidates the stored run; disabled controls grant nothing.
async fn settle_partial(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path((conversation_id, run_id)): Path<(String, String)>,
    Form(form): Form<SettlePartialForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(run_id) = crate::workflows::RunId::parse(&run_id) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    if run.conversation_id != Some(record.id) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    }
    let outcome_matches = run
        .latest_apply_attempt()
        .is_some_and(|attempt| attempt.id.as_hex() == form.attempt);
    if !outcome_matches {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That file application changed. Reload the conversation and try again.",
            ),
        );
    }
    // Recovery protection retains its reservations; settlement never force-unlocks.
    if state.gate_continuations.commit_recovery_locked() {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "This operation requires recovery. The conversation remains reserved until a restart reconciles the local records.",
            ),
        );
    }
    if !run.partial_settlement_eligible() {
        let message = if run.apply_is_uncertain() {
            "Execution remains unsettled. Continuation and retry stay unavailable until recovery and cleanup finish."
        } else if run.is_terminal() {
            "That task already ended. Continue the conversation for the next task."
        } else {
            "That file application changed. Reload the conversation and try again."
        };
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, message),
        );
    }
    let expected_state = run
        .latest_apply_attempt()
        .and_then(|attempt| attempt.apply_transaction.as_ref())
        .map(|transaction| match transaction.state {
            crate::workflows::apply::ApplyTransactionState::Recovered => "recovered",
            _ => "",
        })
        .unwrap_or("");
    if form.state != expected_state {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That file application changed. Reload the conversation and try again.",
            ),
        );
    }
    if state
        .workflow_runs
        .mutate(&run_id, |run| {
            run.settle_known_partial(crate::workflows::now_ms())
        })
        .is_err()
    {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "That file application changed. Reload the conversation and try again.",
            ),
        );
    }
    let record = load_conversation(&state, &conversation_id).unwrap_or(record);
    render_detail_command(
        graft,
        PatchStatus::Ok,
        detail_view(&state, session.0, &record, &record.title, ""),
    )
}

fn observe_message(
    state: AppState,
    session: crate::sessions::SessionId,
    conversation: ConversationId,
    query: ObserveQuery,
) -> AppResult<Response> {
    if let Some(job) =
        JobId::parse(&query.job).and_then(|id| state.sessions.conversation_job(conversation, id))
    {
        let cursor = query
            .cursor
            .parse::<u64>()
            .unwrap_or(0)
            .min(job.latest_seq());
        return Ok(job::observe_response(
            state,
            conversation,
            session,
            job,
            cursor,
            query.historical,
        ));
    }
    if query.historical {
        return Ok(hypergraft::PatchSet::new()
            .with_children(
                "conversation-history-status",
                &page::HistoryStatusContents {
                    history_status: "Live observation ended. Open Latest for current status.",
                    latest_href: &format!("/conversations/{}", conversation.as_hex()),
                },
            )?
            .with_children(
                "conversation-observe",
                &page::ConversationObserveContents {
                    id: &conversation.as_hex(),
                    job_id: "",
                    cursor: 0,
                    active: false,
                    historical: true,
                },
            )?
            .respond(PatchStatus::Ok)?);
    }
    render_transcript(
        &state,
        session,
        GraftRequest::Patch,
        conversation,
        None,
        None,
        "",
    )
}

async fn select_model(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ModelForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let Some(provider) = ProviderKind::parse(form.provider.trim()) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Choose a stored provider.",
            ),
        );
    };
    let has_thinking = !form.thinking.trim().is_empty();
    let thinking = if has_thinking {
        ThinkingEffort::new(form.thinking)
    } else {
        None
    };
    if has_thinking && thinking.is_none() {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Choose an available thinking effort.",
            ),
        );
    }
    let Some(selection) = ModelSelection::new(provider, form.model, thinking) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Enter a valid model name.",
            ),
        );
    };
    if let Err(error) = valid_selection(&state, &selection) {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, error),
        );
    }
    if has_pending_review(&state, record.id) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Finish or discard the current work before you change the model.",
            ),
        );
    }
    let environment = record
        .model
        .as_ref()
        .map(|model| model.settings.environment)
        .or_else(|| default_environment(&state).ok())
        .ok_or_else(|| {
            AppError::new(
                "select conversation environment",
                std::io::Error::other("starter environment unavailable"),
            )
        })?;
    match state
        .conversations
        .select_model(&record.id, revision, selection.clone(), environment)
    {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &updated, &updated.title, ""),
        ),
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            Err(AppError::new("store model selection", error))
        }
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(&state, session.0, &record, &record.title, error.message()),
        ),
    }
}

async fn set_network(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<NetworkForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    if conversation_busy(&state, &record) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "This conversation is reserved. Wait until its operation finishes.",
            ),
        );
    }
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    let network =
        match crate::agents::NetworkAccess::parse_form(&form.network, &form.network_domains) {
            Ok(network) => network,
            Err(error) => {
                return render_detail_command(
                    graft,
                    PatchStatus::UnprocessableEntity,
                    detail_view(&state, session.0, &record, &record.title, error.message()),
                );
            }
        };
    match state
        .conversations
        .set_network(&record.id, revision, network)
    {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &updated, &updated.title, ""),
        ),
        Err(error @ (ConversationError::Persist | ConversationError::Corrupt)) => {
            Err(AppError::new("store conversation network access", error))
        }
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(&state, session.0, &record, &record.title, error.message()),
        ),
    }
}

async fn rename_conversation(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<RenameForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &form.title, REVISION_MESSAGE),
        );
    };
    match state
        .conversations
        .rename(&record.id, revision, form.title.clone())
    {
        Ok(updated) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &updated, &updated.title, ""),
        ),
        Err(
            error @ (ConversationError::Random
            | ConversationError::Persist
            | ConversationError::Corrupt),
        ) => Err(AppError::new("store conversation", error)),
        Err(ConversationError::Missing) => Ok(responses::command_navigation("/conversations")),
        Err(ConversationError::Conflict) => {
            let latest = state.conversations.get(&record.id).unwrap_or(record);
            render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    &state,
                    session.0,
                    &latest,
                    &latest.title,
                    ConversationError::Conflict.message(),
                ),
            )
        }
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(&state, session.0, &record, &form.title, error.message()),
        ),
    }
}

async fn delete_conversation(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<RevisionForm>,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let Some(revision) = parse_revision(&form.revision) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    if state.sessions.conversation_reserved(record.id)
        || has_pending_review(&state, record.id)
        || state.conversation_runtime.unsettled(record.id)
        || has_uncertain_application(&state, record.id)
    {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                "Restore or resolve the pending decision before you delete this conversation.",
            ),
        );
    }
    match state.conversations.delete(&record.id, revision) {
        Ok(()) | Err(ConversationError::Missing) => {
            state
                .outputs
                .remove_scope(&crate::execution::OutputScope::conversation(record.id));
            Ok(responses::command_navigation("/conversations"))
        }
        Err(
            error @ (ConversationError::Random
            | ConversationError::Persist
            | ConversationError::Corrupt),
        ) => Err(AppError::new("store conversation", error)),
        Err(ConversationError::Conflict) => {
            let latest = state.conversations.get(&record.id).unwrap_or(record);
            render_detail_command(
                graft,
                PatchStatus::Conflict,
                detail_view(
                    &state,
                    session.0,
                    &latest,
                    &latest.title,
                    ConversationError::Conflict.message(),
                ),
            )
        }
        Err(error) => render_detail_command(
            graft,
            status_for(error),
            detail_view(&state, session.0, &record, &record.title, error.message()),
        ),
    }
}

/// The selected provider credential, if any. Plan authentication has no key,
/// so skill credential validation has nothing to compare.
fn provider_secret(
    state: &AppState,
    selection: &crate::providers::ModelSelection,
) -> Option<String> {
    let connection = state.vault.connection_for(selection)?;
    match connection.auth {
        crate::providers::AuthMethod::ApiKey => Some(connection.api_key.expose().to_owned()),
        crate::providers::AuthMethod::Plan => None,
    }
}

fn effective_model(
    state: &AppState,
    record: &ConversationRecord,
) -> Option<ConversationModelConfiguration> {
    record.model.clone().or_else(|| {
        state
            .preferences
            .desk_providers(&state.vault)
            .into_iter()
            .find(|provider| provider.selected)
            .and_then(|connection| {
                default_environment(state).ok().map(|environment| {
                    ConversationModelConfiguration::direct(
                        ModelSelection {
                            provider: connection.kind,
                            thinking: state.models_dev.effective_effort(
                                connection.kind,
                                &connection.model,
                                connection.thinking.as_ref(),
                            ),
                            model: connection.model,
                        },
                        environment,
                    )
                })
            })
    })
}

pub(super) fn default_environment(state: &AppState) -> Result<EnvironmentId, &'static str> {
    workflows::alpine_git_id(&state.environments).map_err(|error| error.message())
}

pub(super) fn selected_environment(
    state: &AppState,
    raw: &str,
) -> Result<EnvironmentId, &'static str> {
    let environment = if raw.trim().is_empty() {
        default_environment(state).map_err(|_| "Choose an available environment.")?
    } else {
        EnvironmentId::parse(raw.trim()).ok_or("Choose an available environment.")?
    };
    state
        .environments
        .get(&environment)
        .map(|_| environment)
        .ok_or("Choose an available environment.")
}

pub(super) fn has_pending_review(state: &AppState, conversation: ConversationId) -> bool {
    state
        .workflow_runs
        .for_conversation(&conversation)
        .into_iter()
        .any(|run| {
            run.gates
                .iter()
                .any(|gate| gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision)
        })
}

fn has_uncertain_application(state: &AppState, conversation: ConversationId) -> bool {
    state
        .workflow_runs
        .for_conversation(&conversation)
        .into_iter()
        .any(|run| run.apply_is_uncertain())
}

fn valid_selection(state: &AppState, selection: &ModelSelection) -> Result<(), &'static str> {
    if !state.vault.contains(selection.provider) {
        return Err("Choose a stored provider.");
    }
    if state
        .models_dev
        .model(selection.provider, &selection.model)
        .is_none()
    {
        return Err("Choose an available model.");
    }
    match selection.thinking.as_ref() {
        Some(effort)
            if !state
                .models_dev
                .supports(selection.provider, &selection.model, effort) =>
        {
            Err("Choose an available thinking effort.")
        }
        None if !state
            .models_dev
            .efforts(selection.provider, &selection.model)
            .is_empty() =>
        {
            Err("Choose an available thinking effort.")
        }
        _ => Ok(()),
    }
}

fn conversation_busy(state: &AppState, record: &ConversationRecord) -> bool {
    record.active_job.is_some()
        || state.sessions.conversation_reserved(record.id)
        || state.conversation_runtime.unsettled(record.id)
        || has_uncertain_application(state, record.id)
}

fn detail_view(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: &ConversationRecord,
    title: &str,
    error: &'static str,
) -> ConversationDetailView {
    match state
        .conversations
        .transcript_window(&record.id, None, None)
    {
        Ok(Some((window_record, window))) if window_record.revision == record.revision => {
            detail_view_with_transcript(
                state,
                session,
                &window_record,
                title,
                error,
                Some(&window),
                None,
            )
        }
        _ => detail_view_with_transcript(state, session, record, title, error, None, None),
    }
}

fn detail_view_with_transcript(
    state: &AppState,
    session: crate::sessions::SessionId,
    record: &ConversationRecord,
    title: &str,
    error: &'static str,
    transcript: Option<&crate::conversations::TranscriptWindow>,
    leaf: Option<MessageId>,
) -> ConversationDetailView {
    let snapshot = record
        .active_job
        .and_then(|job_id| state.sessions.conversation_job(record.id, job_id))
        .map(|job| job.snapshot());
    let error = if error.is_empty()
        && snapshot
            .as_ref()
            .is_some_and(|job| job.status == crate::sessions::JobStatus::Failed)
    {
        "This operation requires recovery. The conversation remains reserved until a restart reconciles the local records."
    } else {
        error
    };
    let pending_gate = state
        .workflow_runs
        .active_runs()
        .into_iter()
        .find(|run| run.conversation_id == Some(record.id))
        .and_then(|run| {
            let destination = crate::slices::human_gates::application_destination(state, &run);
            page::pending_code_gate(&run, &state.workflow_artefacts, destination)
        });
    let (source_candidate_review, linked_candidate_reviews) = conversation_links(state, record);
    let workflow_progress = state
        .workflow_runs
        .for_conversation(&record.id)
        .first()
        .map(page::workflow_progress);
    ConversationDetailView::from_record_with_gate(
        record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &state.presets.list(),
        },
        &state.agents.list(),
        snapshot.as_ref(),
        title,
        error,
        pending_gate,
        source_candidate_review,
        linked_candidate_reviews,
        attachments::page::view(state, session, &record.id.as_hex()),
        transcript,
        leaf,
    )
    .with_access_status(state, session, record)
    .with_pending_question(
        record
            .active_job
            .and_then(|job_id| state.conversations.pending_question(record.id, job_id)),
    )
    .with_pending_host_command(
        record
            .active_job
            .and_then(|job_id| state.host_approvals.pending_for(record.id, job_id)),
        record
            .model
            .as_ref()
            .map(|model| {
                crate::slices::execution_settings::page::host_approval_label(
                    model.settings.host_approval,
                )
            })
            .unwrap_or("Ask each time"),
    )
    .with_workflow_progress(workflow_progress)
    .with_direct_results(state, record.id)
    .with_prepared_recovery(state, session, record)
}

fn conversation_links(
    state: &AppState,
    record: &ConversationRecord,
) -> (
    Option<CandidateReviewLinkView>,
    Vec<CandidateReviewLinkView>,
) {
    let candidate_link_view = |link: &CandidateReviewLink| CandidateReviewLinkView {
        title: link
            .conversation_id
            .and_then(|id| state.conversations.metadata_for(&id))
            .map_or_else(String::new, |conversation| conversation.title),
        href: link
            .conversation_id
            .map_or_else(String::new, |id| format!("/conversations/{}", id.as_hex())),
        run_href: format!("/runs/{}", link.run_id.as_hex()),
        candidate_hash: link.candidate.artefact_hash.as_str(),
        diff_base_hash: link.diff_base.artefact_hash.as_str(),
    };
    let source_candidate_review = record
        .source_candidate_review
        .as_ref()
        .map(candidate_link_view);
    let linked_candidate_reviews = record
        .candidate_reviews
        .iter()
        .map(candidate_link_view)
        .collect();
    (source_candidate_review, linked_candidate_reviews)
}

fn load_conversation(state: &AppState, raw: &str) -> Option<ConversationRecord> {
    ConversationId::parse(raw).and_then(|id| state.conversations.get(&id))
}
fn parse_revision(raw: &str) -> Option<u32> {
    raw.parse().ok().filter(|revision| *revision > 0)
}
fn conversation_path(record: &ConversationRecord) -> String {
    format!("/conversations/{}", record.id.as_hex())
}
fn status_for(error: ConversationError) -> PatchStatus {
    match error {
        ConversationError::Conflict | ConversationError::Missing | ConversationError::Active => {
            PatchStatus::Conflict
        }
        _ => PatchStatus::UnprocessableEntity,
    }
}
#[allow(clippy::too_many_arguments)]
fn render_catalogue(
    state: &AppState,
    graft: GraftRequest,
    filter: &str,
    query: &str,
    cursor: Option<(u64, ConversationId)>,
    error: &'static str,
    back_href: String,
    back_label: &'static str,
) -> AppResult<Response> {
    render_page(
        state,
        graft,
        if error.is_empty() {
            PatchStatus::Ok
        } else {
            PatchStatus::UnprocessableEntity
        },
        page::CATALOGUE_TITLE,
        &CatalogueView::from_records(
            state,
            &state.conversations.metadata(),
            filter,
            query,
            cursor,
            error,
            back_href,
            back_label,
        ),
    )
}

fn render_detail(
    state: &AppState,
    _session: crate::sessions::SessionId,
    graft: GraftRequest,
    status: PatchStatus,
    view: ConversationDetailView,
) -> AppResult<Response> {
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(&view.document_title, state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            &view.document_title,
            "chat-main",
            &view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "conversation-detail",
            &view.contents(),
        )?),
    }
}
fn render_detail_command(
    _graft: PatchGraft,
    status: PatchStatus,
    view: ConversationDetailView,
) -> AppResult<Response> {
    Ok(hypergraft::PatchSet::new()
        .title(&view.document_title)
        .with_children("conversation-detail", &view.contents())?
        .respond(status)?)
}
fn render_page<T: askama::Template>(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    title: &str,
    view: &T,
) -> AppResult<Response> {
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(title, state, view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(title, "chat-main", view)?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "chat-main",
            view,
        )?),
    }
}
