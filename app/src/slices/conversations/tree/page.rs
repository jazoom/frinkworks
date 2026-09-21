use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::Response,
};
use hypergraft::{GraftRequest, PatchStatus};
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

pub(super) struct TreeEntryView {
    pub(super) href: String,
    pub(super) children_href: String,
    pub(super) parent_href: String,
    pub(super) role: &'static str,
    pub(super) status: &'static str,
    pub(super) excerpt: String,
    pub(super) on_active_path: bool,
    pub(super) active_leaf: bool,
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
        Ok(Some(window)) => view(&metadata.title, conversation_href, search, window),
        Ok(None) => return Ok(responses::request_navigation(graft, "/conversations")),
        Err(error) => {
            let mut view = view(
                &metadata.title,
                conversation_href,
                search,
                crate::conversations::TreeWindow {
                    entries: Vec::new(),
                    active_leaf: metadata.active_leaf,
                    has_more: false,
                    next_cursor: None,
                    total: 0,
                    partial: false,
                },
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

fn view(
    title: &str,
    conversation_href: String,
    search: &str,
    window: crate::conversations::TreeWindow,
) -> TreeView {
    let active_leaf = window.active_leaf;
    let entries = window
        .entries
        .iter()
        .map(|entry| {
            let on_active_path = entry.on_active_path;
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
                active_leaf: Some(entry.id) == active_leaf,
            }
        })
        .collect();
    let leaves: Vec<TreeLeafView> = active_leaf
        .map(|leaf| TreeLeafView {
            href: format!("{conversation_href}?leaf={}", leaf.as_hex()),
            label: window
                .entries
                .iter()
                .find(|entry| entry.id == leaf)
                .map_or_else(|| "Active branch".to_owned(), |entry| excerpt(&entry.text)),
            active: true,
        })
        .into_iter()
        .collect();
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
