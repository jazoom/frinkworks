mod page;

#[cfg(test)]
mod tests;

pub(super) use page::QuestionView;

use axum::{
    Form,
    extract::{Path, State},
    response::Response,
};
use hypergraft::{PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    conversations::QuestionError,
    error::AppResult,
    sessions::{JobId, RequiredSession},
    state::AppState,
};

use super::{REVISION_MESSAGE, detail_view, load_conversation, render_detail_command};

#[derive(Deserialize)]
pub(super) struct QuestionForm {
    job: String,
    tool_call: String,
    #[serde(default)]
    option: String,
    #[serde(default)]
    text: String,
}

pub(super) async fn answer(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<QuestionForm>,
) -> AppResult<Response> {
    decide(state, session, graft, conversation_id, form, false).await
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: PatchGraft,
    Path(conversation_id): Path<String>,
    Form(form): Form<QuestionForm>,
) -> AppResult<Response> {
    decide(state, session, graft, conversation_id, form, true).await
}

async fn decide(
    state: AppState,
    session: RequiredSession,
    graft: PatchGraft,
    conversation_id: String,
    form: QuestionForm,
    cancelled: bool,
) -> AppResult<Response> {
    let Some(record) = load_conversation(&state, &conversation_id) else {
        return Ok(crate::responses::command_navigation("/conversations"));
    };
    let Some(job_id) = JobId::parse(&form.job) else {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(&state, session.0, &record, &record.title, REVISION_MESSAGE),
        );
    };
    if let Err(error) = bind_session(&state, session.0, record.id, job_id) {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(&state, session.0, &record, &record.title, error),
        );
    }
    let Some(question) = state.conversations.pending_question(record.id, job_id) else {
        return render_detail_command(
            graft,
            PatchStatus::Conflict,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                QuestionError::Missing.message(),
            ),
        );
    };
    if question.tool_call != form.tool_call {
        return render_detail_command(
            graft,
            PatchStatus::UnprocessableEntity,
            detail_view(
                &state,
                session.0,
                &record,
                &record.title,
                QuestionError::Invalid.message(),
            ),
        );
    }
    let answer = match crate::conversations::questions::parse_answer(
        &question,
        (!form.option.trim().is_empty()).then_some(form.option.as_str()),
        (!cancelled).then_some(form.text.as_str()),
        cancelled,
    ) {
        Ok(answer) => answer,
        Err(error) => {
            return render_detail_command(
                graft,
                PatchStatus::UnprocessableEntity,
                detail_view(&state, session.0, &record, &record.title, error.message()),
            );
        }
    };
    let result = state
        .sessions
        .conversation_job(record.id, job_id)
        .ok_or(QuestionError::Missing)
        .and_then(|job| {
            state
                .conversations
                .answer_question(record.id, &job, &form.tool_call, answer)
        });
    match result {
        Ok(()) => render_detail_command(
            graft,
            PatchStatus::Ok,
            detail_view(&state, session.0, &record, &record.title, ""),
        ),
        Err(error) => render_detail_command(
            graft,
            question_status(error),
            detail_view(&state, session.0, &record, &record.title, error.message()),
        ),
    }
}

fn bind_session(
    state: &AppState,
    session: crate::sessions::SessionId,
    conversation: crate::conversations::ConversationId,
    job: JobId,
) -> Result<(), &'static str> {
    if state
        .sessions
        .owns_conversation_job(&session, conversation, job)
    {
        Ok(())
    } else {
        Err("This question belongs to another session.")
    }
}

fn question_status(error: QuestionError) -> PatchStatus {
    match error {
        QuestionError::Duplicate | QuestionError::Missing | QuestionError::JobCancelled => {
            PatchStatus::Conflict
        }
        QuestionError::Invalid | QuestionError::Bound => PatchStatus::UnprocessableEntity,
    }
}
