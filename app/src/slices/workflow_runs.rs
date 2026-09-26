mod page;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{Path, Query, State},
    response::Response,
    routing::get,
};
use hypergraft::{GraftRequest, PageGraft, PatchStatus};

use crate::{
    error::AppResult, responses, sessions::RequiredSession, state::AppState, workflows::RunId,
};

use self::page::{
    ArtefactView, RunDetailView, RunIndexView, attempt_activity_view, attempt_result_view,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/runs", get(index))
        .route("/runs/{run_id}", get(detail))
        .route(
            "/runs/{run_id}/attempts/{attempt_id}/context",
            get(initial_context),
        )
        .route(
            "/runs/{run_id}/attempts/{attempt_id}/activity",
            get(attempt_activity),
        )
        .route(
            "/runs/{run_id}/attempts/{attempt_id}/result",
            get(attempt_result),
        )
        .route("/runs/{run_id}/artefacts/{artefact_id}", get(artefact))
}

async fn index(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Query(query): Query<HistoryQuery>,
) -> AppResult<Response> {
    let valid = query.directory.is_empty() || known_history_directory(&state, &query.directory);
    let error = if valid {
        ""
    } else {
        "Choose a directory from run history."
    };
    let status = if valid {
        PatchStatus::Ok
    } else {
        PatchStatus::UnprocessableEntity
    };
    let view = RunIndexView::filtered(&state, &query.directory, error);
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(page::INDEX_TITLE, &state, &view)?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            page::INDEX_TITLE,
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

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HistoryQuery {
    directory: String,
}

fn known_history_directory(state: &AppState, filter: &str) -> bool {
    if filter.len() > crate::agents::MAXIMUM_PATH_BYTES {
        return false;
    }
    state.workflow_runs.all_summaries().iter().any(|summary| {
        state.workflow_runs.get(&summary.id).is_some_and(|run| {
            page::run_grants(&run).any(|grant| page::run_directory_key(grant) == filter)
        })
    })
}

async fn detail(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
    Path(run_id): Path<String>,
) -> AppResult<Response> {
    let Some(id) = RunId::parse(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(run) = state.workflow_runs.get(&id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let pending = run.conversation_id.and_then(|conversation| {
        state
            .conversations
            .get(&conversation)
            .and_then(|record| record.active_job)
            .and_then(|job_id| state.host_approvals.pending_for(conversation, job_id))
    });
    let mut view = RunDetailView::from_run(
        &run,
        &state.workflows,
        &state.environments,
        &state.workflow_evidence,
    )
    .with_pending_host_command(
        pending.filter(|command| command.run.as_deref() == Some(run.id.as_hex().as_str())),
    );
    if run
        .conversation_id
        .is_some_and(|id| state.conversations.get(&id).is_none())
    {
        view.conversation_href.clear();
        view.catalogue_note
            .push_str(" Owning conversation unavailable. Run evidence remains available.");
    }
    match graft {
        GraftRequest::Document => {
            let mut response = responses::chat_page_response(page::DETAIL_TITLE, &state, &view)?;
            responses::apply_patch_status(&mut response, PatchStatus::Ok);
            Ok(response)
        }
        GraftRequest::Navigation => Ok(hypergraft::outcome::page_patch(
            page::DETAIL_TITLE,
            "chat-main",
            &view,
        )?),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            PatchStatus::Ok,
            "run-detail",
            &view.contents(),
        )?),
    }
}

#[derive(Default, serde::Deserialize)]
struct ContextQuery {
    #[serde(default)]
    part: usize,
    #[serde(default)]
    offset: usize,
}

async fn initial_context(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path((run_id, attempt_id)): Path<(String, String)>,
    Query(query): Query<ContextQuery>,
) -> AppResult<Response> {
    let Some(run) = RunId::parse(&run_id).and_then(|id| state.workflow_runs.get(&id)) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let run_href = format!("/runs/{}", run.id.as_hex());
    let Some(attempt) = run
        .attempts
        .iter()
        .find(|attempt| attempt.id.as_hex() == attempt_id)
    else {
        return Ok(responses::request_navigation(graft, &run_href));
    };
    let context_href = format!("{run_href}/attempts/{}/context", attempt.id.as_hex());
    let Some(view) = attempt.initial_context.as_ref().and_then(|packet| {
        page::initial_context_view(packet, &run_href, &context_href, query.part, query.offset)
    }) else {
        return Ok(responses::request_navigation(graft, &run_href));
    };
    match graft {
        PageGraft::Document => responses::chat_page_response("Initial context", &state, &view),
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            "Initial context",
            "chat-main",
            &view,
        )?),
    }
}

async fn attempt_activity(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path((run_id, attempt_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(run_id) = RunId::parse(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(attempt) = run
        .attempts
        .iter()
        .find(|attempt| attempt.id.as_hex() == attempt_id)
    else {
        return Ok(responses::request_navigation(
            graft,
            &format!("/runs/{}", run.id.as_hex()),
        ));
    };
    let evidence = crate::workflows::AttemptId::parse(&attempt_id)
        .and_then(|attempt_id| state.workflow_evidence.get(&run.id, &attempt_id));
    let view = attempt_activity_view(&run, attempt, evidence);
    match graft {
        PageGraft::Document => responses::chat_page_response("Attempt activity", &state, &view),
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            "Attempt activity",
            "chat-main",
            &view,
        )?),
    }
}

async fn attempt_result(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path((run_id, attempt_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(run_id) = RunId::parse(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(attempt) = run
        .attempts
        .iter()
        .find(|attempt| attempt.id.as_hex() == attempt_id)
    else {
        return Ok(responses::request_navigation(
            graft,
            &format!("/runs/{}", run.id.as_hex()),
        ));
    };
    let evidence = crate::workflows::AttemptId::parse(&attempt_id)
        .and_then(|attempt_id| state.workflow_evidence.get(&run.id, &attempt_id));
    let view = attempt_result_view(&run, attempt, evidence);
    match graft {
        PageGraft::Document => responses::chat_page_response("Attempt result", &state, &view),
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            "Attempt result",
            "chat-main",
            &view,
        )?),
    }
}

async fn artefact(
    State(state): State<AppState>,
    _session: RequiredSession,
    graft: PageGraft,
    Path((run_id, artefact_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(run_id) = RunId::parse(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(artefact_id) = crate::workflows::ArtefactId::parse(&artefact_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(run) = state.workflow_runs.get(&run_id) else {
        return Ok(responses::request_navigation(graft, "/runs"));
    };
    let Some(record) = run.artefact(&artefact_id) else {
        return Ok(responses::request_navigation(
            graft,
            &format!("/runs/{}", run.id.as_hex()),
        ));
    };
    let view = ArtefactView::from_record(&run, record, &state);
    match graft {
        PageGraft::Document => {
            let mut response = responses::chat_page_response(page::ARTEFACT_TITLE, &state, &view)?;
            responses::apply_patch_status(&mut response, PatchStatus::Ok);
            Ok(response)
        }
        PageGraft::Navigation => Ok(hypergraft::outcome::page_patch(
            page::ARTEFACT_TITLE,
            "chat-main",
            &view,
        )?),
    }
}
