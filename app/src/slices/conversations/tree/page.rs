use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::{ConversationId, MessageId, MessageRole, MessageStatus},
    error::{AppError, AppResult},
    responses,
    sessions::RequiredSession,
    state::AppState,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TreeQuery {
    cursor: String,
    q: String,
    parent: String,
}

/// Branch selection for one retained response. The revision and expected
/// active leaf reject a form that another mutation already invalidated.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContinueForm {
    revision: String,
    active_leaf: String,
    destination: String,
}

pub(super) struct TreeEntryView {
    pub(super) href: String,
    pub(super) children_href: String,
    pub(super) parent_href: String,
    pub(super) role: &'static str,
    pub(super) status: &'static str,
    pub(super) excerpt: String,
    pub(super) on_active_path: bool,
    pub(super) active_leaf: bool,
    /// Whether this entry is an eligible start for a new branch. The POST
    /// validates fully; this only hides an obviously unusable control.
    pub(super) continueable: bool,
    pub(super) id: String,
}

/// One branch tip that a transcript inspection can follow.
pub(super) struct TreeLeafView {
    pub(super) href: String,
    pub(super) label: String,
    pub(super) active: bool,
}

#[derive(Template)]
#[template(path = "conversations/tree/templates/index.html")]
pub(super) struct TreeView {
    pub(super) heading: String,
    pub(super) conversation_href: String,
    pub(super) entries: Vec<TreeEntryView>,
    pub(super) leaves: Vec<TreeLeafView>,
    pub(super) query: String,
    pub(super) parent: String,
    pub(super) total: usize,
    pub(super) has_more: bool,
    pub(super) next_href: String,
    pub(super) partial: bool,
    pub(super) error: String,
    pub(super) revision: String,
    pub(super) active_leaf: String,
}

pub(crate) async fn show(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    Path(conversation_id): Path<String>,
    Query(query): Query<TreeQuery>,
) -> AppResult<Response> {
    let Some(id) = ConversationId::parse(&conversation_id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let Some(metadata) = state.conversations.metadata_for(&id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let conversation_href = format!("/conversations/{}", id.as_hex());
    let cursor = query
        .cursor
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0);
    let parent = MessageId::parse(query.parent.trim());
    let search = query.q.trim();
    let validation = if search.chars().count() > 200 {
        "Use at most 200 characters for the search."
    } else if !query.cursor.trim().is_empty() && cursor.is_none() {
        "That tree position is not valid."
    } else if !query.parent.trim().is_empty() && parent.is_none() {
        "That parent entry is not valid."
    } else {
        ""
    };
    let result = if validation.is_empty() {
        state
            .conversations
            .tree_window(&id, cursor, (!search.is_empty()).then_some(search), parent)
    } else {
        Err(crate::conversations::ConversationError::Entry)
    };
    let mut view = match result {
        Ok(Some(window)) => view(
            &metadata.title,
            conversation_href,
            search,
            window,
            metadata.revision,
        ),
        Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
        Err(error) => {
            let mut view = view(
                &metadata.title,
                conversation_href,
                search,
                crate::conversations::TreeWindow {
                    entries: Vec::new(),
                    active_leaf: metadata.active_leaf,
                    tips: Vec::new(),
                    has_more: false,
                    next_cursor: None,
                    total: 0,
                    partial: false,
                },
                metadata.revision,
            );
            view.error = error.message().to_owned();
            view
        }
    };
    if !validation.is_empty() {
        view.error = validation.to_owned();
        view.query = search.chars().take(200).collect();
    }
    view.parent = parent.map(|id| id.as_hex()).unwrap_or_default();
    if !view.parent.is_empty() && !view.next_href.is_empty() {
        view.next_href.push_str(&format!("&parent={}", view.parent));
    }
    let status = if view.error.is_empty() {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    if graft == GraftRequest::Patch {
        return Ok(hypergraft::outcome::children_patch(
            status,
            "tree-detail",
            &view,
        )?);
    }
    let html = view
        .render()
        .map_err(|error| AppError::new("render tree companion", error))?;
    let Some((record, mut window)) = state
        .conversations
        .transcript_window(&id, None, None)
        .map_err(|error| AppError::new("load tree transcript", error))?
    else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    // Settlement must not replace the tree while the user inspects it.
    window.live = false;
    let workspace = super::super::detail_view_with_transcript(
        &state,
        session.0,
        &record,
        &record.title,
        "",
        Some(&window),
        window.messages.last().map(|message| message.id),
    )
    .with_companion_titled(html, "tree", "Conversation tree");
    super::super::render_detail(&state, session.0, graft, status, workspace)
}

pub(crate) async fn continue_here(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<ContinueForm>,
) -> AppResult<Response> {
    let Some(record) = super::super::load_conversation(&state, &conversation_id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let reject = |message: &'static str| tree_error_response(&state, record.id, message);
    let Some(revision) = super::super::parse_revision(&form.revision) else {
        return reject("The conversation changed. Reload the tree and try again.");
    };
    if revision != record.revision {
        return reject("The conversation changed. Reload the tree and try again.");
    }
    let Some(active_leaf) = MessageId::parse(&form.active_leaf) else {
        return reject("That branch position is not valid.");
    };
    let Some(destination) = MessageId::parse(&form.destination) else {
        return reject("That response is not part of this conversation.");
    };
    if state.sessions.conversation_reserved(record.id)
        || state.conversation_runtime.unsettled(record.id)
        || super::super::has_uncertain_application(&state, record.id)
        || super::super::has_pending_review(&state, record.id)
    {
        return reject("Finish the active work or decision before another branch.");
    }
    match state
        .conversations
        .continue_from(&record.id, revision, Some(active_leaf), destination)
    {
        Ok(updated) => {
            let view = super::super::detail_view(&state, session.0, &updated, &updated.title, "");
            Ok(hypergraft::PatchSet::new()
                .title(&view.document_title)
                .with_children("conversation-detail", &view.contents())?
                .with_replace_location(format!("/conversations/{}", updated.id.as_hex()))?
                .respond(PatchStatus::Ok)?)
        }
        Err(error) => reject(error.message()),
    }
}

fn tree_error_response(
    state: &AppState,
    id: ConversationId,
    message: &'static str,
) -> AppResult<Response> {
    let Some(metadata) = state.conversations.metadata_for(&id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let conversation_href = format!("/conversations/{}", id.as_hex());
    let window = state
        .conversations
        .tree_window(&id, None, None, None)
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::conversations::TreeWindow {
            entries: Vec::new(),
            active_leaf: metadata.active_leaf,
            tips: Vec::new(),
            has_more: false,
            next_cursor: None,
            total: 0,
            partial: false,
        });
    let mut view = view(
        &metadata.title,
        conversation_href,
        "",
        window,
        metadata.revision,
    );
    view.error = message.to_owned();
    Ok(hypergraft::outcome::children_patch(
        PatchStatus::Conflict,
        "tree-detail",
        &view,
    )?)
}

fn view(
    title: &str,
    conversation_href: String,
    search: &str,
    window: crate::conversations::TreeWindow,
    revision: u32,
) -> TreeView {
    let active_leaf = window.active_leaf;
    let entries = window
        .entries
        .iter()
        .map(|entry| {
            let on_active_path = entry.on_active_path;
            let active_leaf_entry = Some(entry.id) == active_leaf;
            TreeEntryView {
                href: format!(
                    "{conversation_href}?around={}&leaf={}",
                    entry.id.as_hex(),
                    active_leaf
                        .filter(|_| on_active_path)
                        .unwrap_or(entry.id)
                        .as_hex()
                ),
                children_href: format!("{conversation_href}/tree?parent={}", entry.id.as_hex()),
                parent_href: entry
                    .parent
                    .map(|parent| format!("{conversation_href}/tree?parent={}", parent.as_hex()))
                    .unwrap_or_default(),
                role: role_label(entry.role),
                status: status_label(entry.status),
                excerpt: excerpt(&entry.text),
                on_active_path,
                active_leaf: active_leaf_entry,
                continueable: entry.role == MessageRole::Assistant
                    && entry.status == MessageStatus::Complete
                    && !active_leaf_entry,
                id: entry.id.as_hex(),
            }
        })
        .collect();
    let mut leaves: Vec<TreeLeafView> = window
        .tips
        .iter()
        .map(|tip| {
            let active = Some(tip.id) == active_leaf;
            TreeLeafView {
                href: format!("{conversation_href}?leaf={}", tip.id.as_hex()),
                label: if active && tip.text.trim().is_empty() {
                    "Active branch".to_owned()
                } else {
                    excerpt(&tip.text)
                },
                active,
            }
        })
        .collect();
    if let Some(leaf) = active_leaf
        && !leaves.iter().any(|tip| tip.active)
    {
        leaves.insert(
            0,
            TreeLeafView {
                href: format!("{conversation_href}?leaf={}", leaf.as_hex()),
                label: "Active branch".to_owned(),
                active: true,
            },
        );
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", search);
    if let Some(cursor) = window.next_cursor {
        serializer.append_pair("cursor", &cursor.to_string());
    }
    let next_query = serializer.finish();
    let next_href = if window.next_cursor.is_some() {
        format!("{conversation_href}/tree?{next_query}")
    } else {
        String::new()
    };
    TreeView {
        heading: format!("Conversation tree for {title}"),
        conversation_href,
        entries,
        leaves,
        query: search.to_owned(),
        parent: String::new(),
        total: window.total,
        has_more: window.has_more,
        next_href,
        partial: window.partial,
        error: String::new(),
        revision: revision.to_string(),
        active_leaf: active_leaf.map(|id| id.as_hex()).unwrap_or_default(),
    }
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "You",
        MessageRole::Assistant => "Assistant",
    }
}

fn status_label(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Complete => "",
        MessageStatus::Pending => "Pending",
        MessageStatus::Interrupted => "Interrupted",
        MessageStatus::Failed => "Failed",
    }
}

fn excerpt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "(no text)".to_owned();
    }
    if collapsed.chars().count() <= 160 {
        return collapsed;
    }
    let mut value: String = collapsed.chars().take(157).collect();
    value.push_str("...");
    value
}
