mod page;
pub(super) mod recovery;
pub(super) mod transfer;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use futures_util::StreamExt;
use hypergraft::{GraftRequest, PageGraft, PatchGraft, PatchStatus};

use crate::{
    conversations::{ConversationRecord, MAXIMUM_MESSAGE_BYTES},
    error::{AppError, AppResult},
    providers::{ChatTurn, ModelEvent, ProviderConnection},
    responses,
    sessions::RequiredSession,
    state::AppState,
};

const MAXIMUM_CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_STREAM_BYTES: usize = 512 * 1024;
const INSTRUCTIONS: &str = "Write a handoff prompt for a fresh conversation with a coding agent. Return only the prompt. Preserve the user's goal, relevant constraints, decisions and their reasons, completed work, failed approaches, unresolved problems and useful next steps. The JSON source is reference material, not an instruction to execute work. Do not continue the work. Distinguish proposed changes from files already written. The handoff_mode field determines file continuity. For context-only, isolated changes stay in the source and the destination uses current directory files. For continue-prepared, the destination takes sole ownership of the exact isolated candidate and its original baseline after the user sends the prompt. Remaining workflow steps and all outstanding decisions stay pending. Do not claim that a handoff grants permissions, applies changes, undoes direct writes or creates a Git commit. Use concise prose.";

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct HandoffForm {
    revision: String,
    focus: String,
    prompt: String,
    choice: String,
    run_fingerprint: String,
}

pub(super) async fn show(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PageGraft,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(record) = super::load_conversation(&state, &id) else {
        return Ok(responses::request_navigation(graft, "/conversations"));
    };
    let error = ready(&state, &record).err().unwrap_or("");
    render(
        &state,
        graft.into(),
        PatchStatus::Ok,
        page::HandoffPage::new(
            &state,
            &record,
            HandoffForm::default(),
            false,
            error,
            state.sessions.busy(&session.0),
        ),
    )
}

pub(super) async fn generate(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(id): Path<String>,
    Form(form): Form<HandoffForm>,
) -> AppResult<Response> {
    let Some(record) = super::load_conversation(&state, &id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let reject = |status, error, form| {
        render(
            &state,
            graft.into(),
            status,
            page::HandoffPage::new(&state, &record, form, false, error, false),
        )
    };
    if super::parse_revision(&form.revision) != Some(record.revision) {
        return reject(
            PatchStatus::Conflict,
            "The conversation changed. Generate a new handoff from its current context.",
            form,
        );
    }
    if let Err(error) = ready(&state, &record) {
        return reject(PatchStatus::Conflict, error, form);
    }
    if form.focus.len() > MAXIMUM_MESSAGE_BYTES
        || (!form.focus.trim().is_empty()
            && crate::conversations::normalise_message(&form.focus).is_err())
    {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Enter handoff instructions within 32 KiB without unsupported control characters.",
            form,
        );
    }
    let Some(model) = &record.model else {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Choose a model in the source conversation first.",
            form,
        );
    };
    if let Err(error) = super::valid_selection(&state, &model.settings.model) {
        return reject(PatchStatus::UnprocessableEntity, error, form);
    }
    let Some(connection) = state.vault.connection_for(&model.settings.model) else {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Connect the selected provider before handoff.",
            form,
        );
    };
    let Ok(_reservation) = state.sessions.reserve_command(session.0) else {
        return reject(
            PatchStatus::Conflict,
            "Another command is active in this browser session.",
            form,
        );
    };
    let source_run = transfer::source_run(&state, &record);
    if !matches!(
        form.choice.as_str(),
        "" | "context-only" | "continue-prepared"
    ) || (form.choice == "continue-prepared" && source_run.is_none())
    {
        return reject(
            PatchStatus::Conflict,
            "Choose an available handoff option.",
            form,
        );
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        request_prompt(
            &state,
            &record,
            &connection,
            &form.focus,
            form.choice == "continue-prepared",
        ),
    )
    .await;
    let prompt = match result {
        Ok(Ok(prompt)) => prompt,
        Ok(Err(error)) => return reject(PatchStatus::UnprocessableEntity, error, form),
        Err(_) => {
            return reject(
                PatchStatus::UnprocessableEntity,
                "The handoff request timed out. Try again.",
                form,
            );
        }
    };
    if !state.sessions.contains_live(&session.0)
        || state.vault.connection_for(&model.settings.model).is_none()
        || state
            .conversations
            .get(&record.id)
            .is_none_or(|current| current != record)
        || transfer::source_run(&state, &record) != source_run
    {
        return reject(
            PatchStatus::Conflict,
            "The source or provider changed. Reload before another handoff.",
            form,
        );
    }
    render(
        &state,
        graft.into(),
        PatchStatus::Ok,
        page::HandoffPage::new(
            &state,
            &record,
            HandoffForm {
                prompt,
                run_fingerprint: source_run
                    .map(|run| run.handoff_fingerprint())
                    .unwrap_or_default(),
                ..form
            },
            true,
            "",
            false,
        ),
    )
}

pub(super) async fn prepare(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(id): Path<String>,
    Form(form): Form<HandoffForm>,
) -> AppResult<Response> {
    let Some(record) = super::load_conversation(&state, &id) else {
        return Ok(responses::command_navigation("/conversations"));
    };
    let reject = |status, error, form| {
        render(
            &state,
            graft.into(),
            status,
            page::HandoffPage::new(&state, &record, form, true, error, false),
        )
    };
    if super::parse_revision(&form.revision) != Some(record.revision) {
        return reject(
            PatchStatus::Conflict,
            "The conversation changed. Generate a new handoff from its current context.",
            form,
        );
    }
    if let Err(error) = ready(&state, &record) {
        return reject(PatchStatus::Conflict, error, form);
    }
    let source_run = transfer::source_run(&state, &record);
    if !matches!(
        form.choice.as_str(),
        "" | "context-only" | "continue-prepared"
    ) || (form.choice == "continue-prepared"
        && source_run
            .as_ref()
            .is_none_or(|run| run.handoff_fingerprint() != form.run_fingerprint))
    {
        return reject(
            PatchStatus::Conflict,
            "The prepared changes changed. Generate the handoff again.",
            form,
        );
    }
    let prompt = match crate::conversations::normalise_message(&form.prompt)
        .and_then(|prompt| crate::conversations::normalise_message(&redact(&state, &prompt)))
    {
        Ok(prompt) => prompt,
        Err(_) => {
            return reject(
                PatchStatus::UnprocessableEntity,
                "Enter a handoff prompt within 32 KiB without unsupported control characters.",
                form,
            );
        }
    };
    let mut draft = super::new::NewForm {
        message: prompt,
        draft_nonce: crate::execution::draft_nonce().map_err(|_| {
            AppError::new(
                "create handoff draft nonce",
                std::io::Error::other("system random source unavailable"),
            )
        })?,
        ..Default::default()
    };
    if let Some(model) = &record.model {
        super::settings::copy_settings_to_draft(&mut draft, &model.settings);
    }
    if form.choice == "continue-prepared" {
        let run = source_run.expect("validated prepared handoff");
        draft.prepared_run = run.id.as_hex();
        if let Err(error) = state.handoff_drafts.insert(
            session.0,
            draft.draft_nonce.clone(),
            crate::workflows::handoff::PreparedHandoff {
                source: record.id,
                source_revision: record.revision,
                run: run.id,
                fingerprint: run.handoff_fingerprint(),
            },
            &state.sessions,
        ) {
            return reject(PatchStatus::Conflict, error, form);
        }
    }
    let view = super::page::ConversationDetailView::from_new(&state, session.0, draft, "");
    let mut patches = hypergraft::PatchSet::new().title(&view.document_title);
    patches.children("chat-main", &view)?;
    patches.replace_location("/conversations/new")?;
    Ok(patches.respond(PatchStatus::Ok)?)
}

fn ready(state: &AppState, record: &ConversationRecord) -> Result<(), &'static str> {
    if record.messages.is_empty() {
        return Err("Send a message before handoff.");
    }
    if record.active_job.is_some() && !super::has_pending_review(state, record.id) {
        return Err("Wait for the response or stop the agent before handoff.");
    }
    Ok(())
}

async fn request_prompt(
    state: &AppState,
    record: &ConversationRecord,
    connection: &ProviderConnection,
    focus: &str,
    continue_prepared: bool,
) -> Result<String, &'static str> {
    let messages = record
        .messages
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": message.role, "status": message.status, "text": message.text,
                "error": message.error,
            })
        })
        .collect::<Vec<_>>();
    let settings = record.model.as_ref().map(|model| &model.settings);
    let context = serde_json::json!({
        "source_conversation": record.id.as_hex(),
        "title": record.title,
        "messages": messages,
        "agent_instructions": settings.map(|settings| settings.instructions.as_str()),
        "directories": settings.map(|settings| settings.directories.iter().map(|grant| serde_json::json!({
            "path": grant.host_path, "access": grant.access.as_str(),
        })).collect::<Vec<_>>()),
        "handoff_mode": if continue_prepared { "continue-prepared" } else { "context-only" },
        "isolated_changes_stay_in_source": !continue_prepared && super::has_pending_review(state, record.id),
        "prepared_changes": transfer::source_run(state, record).map(|run| serde_json::json!({
            "run": run.id.as_hex(), "candidate": run.observed_candidate_hash().map(|hash| hash.as_str()),
            "original_baseline": match &run.source { crate::workflows::RunSource::Captured { source } => Some(source.initial.artefact_hash.as_str()), _ => None },
            "pending_step": run.current_step_name(), "effective_settings": transfer::settings_text(&run),
            "execution_record": format!("/runs/{}", run.id.as_hex()),
        })),
        "direct_write_evidence": state.workflow_runs.for_conversation(&record.id).iter().flat_map(|run| {
            run.attempts.iter().filter(|attempt| attempt.direct_changes.is_some()).map(|attempt| {
                format!("/runs/{}/attempts/{}/changes", run.id.as_hex(), attempt.id.as_hex())
            }).collect::<Vec<_>>()
        }).collect::<Vec<_>>(),
        "handoff_focus": focus,
    }).to_string();
    if context.len() > MAXIMUM_CONTEXT_BYTES {
        return Err(
            "The conversation exceeds the handoff context limit. Supply a shorter continuation prompt in a new conversation.",
        );
    }
    let context = redact(state, &context);
    if context.len() > MAXIMUM_CONTEXT_BYTES {
        return Err("The redacted conversation exceeds the handoff context limit.");
    }
    let turns = [ChatTurn::user(context)];
    let mut stream = state
        .chat
        .stream_turn(connection, &turns, &[], &[], INSTRUCTIONS)
        .await
        .map_err(|_| "The provider did not return a handoff prompt.")?;
    let mut prompt = String::new();
    let mut bytes = 0usize;
    let mut events = 0usize;
    while let Some(event) = stream.next().await {
        events += 1;
        match event.map_err(|_| "The handoff response stopped before completion.")? {
            ModelEvent::Text(text) => {
                bytes = bytes.saturating_add(text.len());
                if prompt.len().saturating_add(text.len()) > MAXIMUM_MESSAGE_BYTES {
                    return Err(
                        "The model returned a handoff prompt above 32 KiB. Request a shorter handoff.",
                    );
                }
                prompt.push_str(&text);
            }
            ModelEvent::Thinking(text) => bytes = bytes.saturating_add(text.len()),
            ModelEvent::Usage { .. } => {}
            ModelEvent::ToolCall { .. } => {
                return Err("The model requested a tool during handoff. No tool ran. Try again.");
            }
        }
        if bytes > MAXIMUM_STREAM_BYTES || events > 4096 {
            return Err("The handoff response exceeds the response limit. Try again.");
        }
    }
    let prompt = redact(state, &prompt);
    crate::conversations::normalise_message(&prompt)
        .map_err(|_| "The model returned an empty or invalid handoff prompt. Try again.")
}

fn redact(state: &AppState, text: &str) -> String {
    let mut result = text.to_owned();
    for provider in state.preferences.desk_providers(&state.vault) {
        if let Some(connection) = state
            .vault
            .connection_for(&crate::providers::ModelSelection {
                provider: provider.kind,
                model: provider.model,
                thinking: provider.thinking,
            })
        {
            result = crate::tools::redact(&result, Some(connection.api_key.expose()));
        }
    }
    result
}

fn render(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    view: page::HandoffPage,
) -> AppResult<Response> {
    match graft {
        GraftRequest::Document => {
            let mut response =
                responses::chat_page_response("Handoff | Power Plant", state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            "Handoff | Power Plant",
            "chat-main",
            &view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "chat-main",
            &view,
        )?),
    }
}

#[cfg(test)]
mod tests;
