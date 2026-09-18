mod forms;
mod page;

#[cfg(test)]
mod tests;

use std::time::Duration;

use axum::{
    Form, Router,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use hypergraft::{GraftRequest, PatchGraft, PatchStatus};
use serde::Deserialize;

use crate::{
    error::AppResult,
    plan_login::PendingPlan,
    providers::{ProviderConnection, ProviderError, ProviderKind},
    responses,
    sessions::{self, OptionalSession, RequiredSession},
    state::AppState,
};

use self::{
    forms::{ConnectForm, ForgetForm, PlanForm},
    page::ConnectViewModel,
};

// A long device-code wait cannot occupy one HTTP response. Each hold ends first.
const PLAN_HOLD: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(15)
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/connect", get(show).post(submit))
        .route("/connect/plan", post(start_plan))
        .route("/connect/forget", post(forget))
}

// The root is a navigation entry point. A live session continues to work; any
// other visitor lands on the connection page through the session extractor.
async fn root(
    State(_state): State<AppState>,
    _session: RequiredSession,
    graft: GraftRequest,
) -> AppResult<Response> {
    Ok(responses::request_navigation(graft, "/conversations"))
}

#[derive(Deserialize)]
struct ConnectQuery {
    provider: Option<ProviderKind>,
    plan: Option<ProviderKind>,
}

async fn show(
    State(state): State<AppState>,
    graft: GraftRequest,
    query: Result<Query<ConnectQuery>, QueryRejection>,
) -> AppResult<Response> {
    let Ok(Query(query)) = query else {
        return Ok(responses::no_store_status_response(
            StatusCode::BAD_REQUEST,
            "Bad request",
        ));
    };
    if graft == GraftRequest::Patch {
        let previous = state.plan_login.snapshot();
        if previous
            .as_ref()
            .is_some_and(|pending| pending.error.is_none())
        {
            state
                .plan_login
                .wait_until_changed(previous, PLAN_HOLD)
                .await;
        }
        if query.plan.is_some_and(|kind| state.vault.contains(kind)) {
            return Ok(responses::command_navigation("/conversations"));
        }
    }
    render(
        &state,
        graft,
        PatchStatus::Ok,
        ConnectViewModel::initial(
            &state.vault,
            state.plan_login.snapshot(),
            sandbox_missing(&state).await,
            query.provider,
        ),
    )
}

async fn submit(
    State(state): State<AppState>,
    graft: PatchGraft,
    Form(form): Form<ConnectForm>,
) -> AppResult<Response> {
    let kind = match form.validate() {
        Ok(kind) => kind,
        Err(error) => {
            return render(
                &state,
                graft.into(),
                PatchStatus::UnprocessableEntity,
                ConnectViewModel::invalid(
                    &state.vault,
                    state.plan_login.snapshot(),
                    sandbox_missing(&state).await,
                    form,
                    error,
                ),
            );
        }
    };

    let connection = ProviderConnection::with_key(kind, form.api_key, kind.default_model());
    if let Err(error) = state.chat.verify(&connection).await {
        return render(
            &state,
            graft.into(),
            error.patch_status(),
            ConnectViewModel::failed(
                &state.vault,
                state.plan_login.snapshot(),
                sandbox_missing(&state).await,
                kind,
                error,
            ),
        );
    }

    state
        .vault
        .insert_api_key(connection)
        .map_err(|error| crate::error::AppError::new("store provider", error))?;
    Ok(responses::command_navigation("/conversations"))
}

async fn start_plan(
    State(state): State<AppState>,
    graft: PatchGraft,
    Form(form): Form<PlanForm>,
) -> AppResult<Response> {
    let kind = match form.validate() {
        Ok(kind) => kind,
        Err(error) => {
            return render(
                &state,
                graft.into(),
                PatchStatus::UnprocessableEntity,
                ConnectViewModel::plan_invalid(
                    &state.vault,
                    state.plan_login.snapshot(),
                    sandbox_missing(&state).await,
                    error,
                ),
            );
        }
    };
    let Some(dir) = state.vault.provider_dir() else {
        return render(
            &state,
            graft.into(),
            PatchStatus::UnprocessableEntity,
            ConnectViewModel::failed(
                &state.vault,
                state.plan_login.snapshot(),
                sandbox_missing(&state).await,
                kind,
                ProviderError::Unreachable,
            ),
        );
    };

    let generation = state.plan_login.begin();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task_state = state.clone();
    let handle = tokio::spawn(async move {
        complete_plan_login(task_state, kind, dir, generation, ready_tx).await;
    });
    state.plan_login.attach_task(generation, handle);

    if let Ok(Err(error)) = ready_rx.await
        && state.plan_login.generation_is_current(generation)
    {
        return render(
            &state,
            graft.into(),
            error.patch_status(),
            ConnectViewModel::failed(
                &state.vault,
                state.plan_login.snapshot(),
                sandbox_missing(&state).await,
                kind,
                error,
            ),
        );
    }

    render(
        &state,
        graft.into(),
        PatchStatus::Ok,
        ConnectViewModel::initial(
            &state.vault,
            state.plan_login.snapshot(),
            sandbox_missing(&state).await,
            Some(kind),
        ),
    )
}

async fn complete_plan_login(
    state: AppState,
    kind: crate::providers::ProviderKind,
    dir: std::path::PathBuf,
    generation: u64,
    ready: tokio::sync::oneshot::Sender<Result<(), ProviderError>>,
) {
    let mut attempt = match crate::providers::plan::start(kind, dir).await {
        Ok(attempt) => attempt,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    state.plan_login.set_pending(
        generation,
        PendingPlan {
            kind,
            verification_uri: attempt.prompt.verification_uri.clone(),
            user_code: attempt.prompt.user_code.clone(),
            error: None,
        },
    );
    let _ = ready.send(Ok(()));

    let result = attempt.wait().await;
    match result {
        Ok(()) => {
            let Some(staged) = attempt.staged_path().map(std::path::Path::to_path_buf) else {
                state.plan_login.set_error(
                    generation,
                    "Power Plant could not store that provider. Try again.".to_owned(),
                );
                return;
            };
            let installation = state
                .plan_login
                .apply_if_current(generation, || state.vault.install_plan(kind, &staged));
            match installation {
                Some(Ok(())) => {
                    attempt.mark_installed();
                    state.plan_login.finish(generation);
                }
                Some(Err(_)) => {
                    let cleanup = attempt.discard();
                    let message = if cleanup.is_ok() {
                        "Power Plant could not store that provider. Try again."
                    } else {
                        "Power Plant could not remove the failed sign-in. Try again."
                    };
                    state.plan_login.set_error(generation, message.to_owned());
                }
                None => {
                    if let Err(error) = attempt.discard() {
                        crate::error::trace_operation_failure("remove stale plan attempt", &error);
                    }
                }
            }
        }
        Err(ProviderError::Reauthenticate) => {
            let cleanup = attempt.discard();
            let message = if cleanup.is_ok() {
                ProviderError::Reauthenticate.message()
            } else {
                "Power Plant could not remove the failed sign-in. Try again."
            };
            state.plan_login.set_error(generation, message.to_owned());
        }
        Err(_) => {
            let cleanup = attempt.discard();
            let message = if cleanup.is_ok() {
                "Sign-in did not finish. Try again."
            } else {
                "Power Plant could not remove the failed sign-in. Try again."
            };
            state.plan_login.set_error(generation, message.to_owned());
        }
    }
}

async fn forget(
    State(state): State<AppState>,
    OptionalSession(session): OptionalSession,
    graft: PatchGraft,
    Form(form): Form<ForgetForm>,
) -> AppResult<Response> {
    if let Some(kind) = form.provider_kind() {
        crate::workflows::interrupt_provider_continuations(&state, kind)
            .map_err(|error| crate::error::AppError::new("interrupt human gates", error))?;
        state
            .vault
            .forget(kind)
            .map_err(|error| crate::error::AppError::new("forget provider", error))?;
        // Credential removal succeeds even if optional preference cleanup fails.
        if let Err(error) = state.preferences.forget_provider(kind) {
            crate::error::trace_operation_failure("forget provider preferences", &error);
        }
    }

    if !state.vault.has_providers() {
        if let Some(session) = session {
            crate::workflows::interrupt_session_continuations(&state, session)
                .map_err(|error| crate::error::AppError::new("interrupt human gates", error))?;
            state.sessions.remove(&session);
        }
        let mut response = responses::request_navigation(graft, "/connect");
        sessions::clear_session_cookie(&mut response, &state);
        return Ok(response);
    }

    render(
        &state,
        graft.into(),
        PatchStatus::Ok,
        ConnectViewModel::initial(
            &state.vault,
            state.plan_login.snapshot(),
            sandbox_missing(&state).await,
            None,
        ),
    )
}

async fn sandbox_missing(state: &AppState) -> &'static str {
    state.sandboxes.missing_message()
}

fn render(
    state: &AppState,
    graft: GraftRequest,
    status: PatchStatus,
    view: ConnectViewModel,
) -> AppResult<Response> {
    let use_app_shell = view.has_stored_providers;
    let page_target = if use_app_shell {
        "chat-main"
    } else {
        "connect-main"
    };
    match graft {
        GraftRequest::Document => {
            let mut response = responses::connect_page_response(
                page::DOCUMENT_TITLE,
                state,
                &view,
                use_app_shell,
            )?;
            responses::apply_patch_status(&mut response, status);
            Ok(response)
        }
        GraftRequest::Navigation if status == PatchStatus::Ok => Ok(
            hypergraft::outcome::page_patch(page::DOCUMENT_TITLE, page_target, &view)?,
        ),
        GraftRequest::Patch => Ok(hypergraft::outcome::children_patch(
            status,
            "connect-card",
            &view.card_contents(),
        )?),
        _ => Ok(responses::no_store_status_response(
            StatusCode::BAD_REQUEST,
            "Bad request",
        )),
    }
}
