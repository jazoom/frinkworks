mod cookies;
mod job;
mod language;
mod live;
mod store;
mod tokens;

#[cfg(test)]
mod tests;

pub(crate) use cookies::CookieRead;
#[cfg(test)]
pub(crate) use job::JobEventKind;
pub(crate) use job::{Job, JobId, JobOwner, JobSnapshot, JobStatus};
pub(crate) use language::BrowserLanguage;
pub(crate) use live::LiveSessionGuard;
pub(crate) use store::{ConversationKey, SessionStore};
pub(crate) use tokens::{SessionId, ValidatedToken, generate as generate_session_token};

use std::time::Duration;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::Response,
};
use hypergraft::GraftRequest;

use crate::{
    error::{AppResult, AppResultExt, trace_operation_failure},
    state::AppState,
};

// Cookie max-age and server expiry share this bound.
pub(crate) const SESSION_LIFETIME_HOURS: u64 = 12;
pub(crate) const SESSION_LIFETIME: Duration = Duration::from_secs(SESSION_LIFETIME_HOURS * 60 * 60);

const SESSION_PURGE_INTERVAL: Duration = Duration::from_secs(60);

/// Resolve the provider session before handlers. Invalid cookies are expired.
pub(crate) async fn resolve_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let read = cookies::read_session(request.headers());
    let mut issued = None;
    let resolved = match read {
        CookieRead::Missing => restore_or(ResolvedSession::Anonymous, &state, &mut issued),
        CookieRead::Invalid => restore_or(ResolvedSession::Invalid, &state, &mut issued),
        CookieRead::Valid(token) => existing_or_restore(&state, &token),
    };
    let invalid = matches!(resolved, ResolvedSession::Invalid);
    if let ResolvedSession::Present(id) = &resolved
        && let Some(language) = BrowserLanguage::from_headers(request.headers())
    {
        state.sessions.set_language(id, language);
    }
    request.extensions_mut().insert(resolved);
    let mut response = next.run(request).await;
    if let Some(token) = issued {
        if let Err(error) = set_session_cookie(&mut response, &state, &token) {
            trace_operation_failure("restore session cookie", &error);
        }
    } else if invalid {
        expire_unless_replaced(&mut response, &state);
    }
    response
}

fn restore_or(
    fallback: ResolvedSession,
    state: &AppState,
    issued: &mut Option<ValidatedToken>,
) -> ResolvedSession {
    match issue_restored_session(state) {
        Some((snapshot, token)) => {
            *issued = Some(token);
            ResolvedSession::Present(snapshot)
        }
        None => fallback,
    }
}

fn existing_or_restore(state: &AppState, token: &ValidatedToken) -> ResolvedSession {
    let id = SessionId::from_validated(token);
    if state.sessions.contains_live(&id) {
        if state.vault.has_providers() {
            return ResolvedSession::Present(id);
        }
        if crate::workflows::interrupt_session_continuations(state, id).is_err() {
            return ResolvedSession::Invalid;
        }
        state.sessions.remove(&id);
        state
            .access_consent
            .retain_sessions(|session| state.sessions.contains_live(session));
        state
            .host_approvals
            .retain_sessions(|session| state.sessions.contains_live(session));
        state
            .conversations
            .retain_question_sessions(|session| state.sessions.contains_live(session));
        return ResolvedSession::Invalid;
    }
    // A restored cookie starts a new session lifetime, not a renewal of execution consent.
    state
        .access_consent
        .retain_sessions(|session| state.sessions.contains_live(session));
    state
        .host_approvals
        .retain_sessions(|session| state.sessions.contains_live(session));
    state
        .conversations
        .retain_question_sessions(|session| state.sessions.contains_live(session));
    if state.sessions.contains_expired(&id) {
        if crate::workflows::interrupt_session_continuations(state, id).is_err() {
            return ResolvedSession::Invalid;
        }
        state.sessions.remove(&id);
    }
    if !state.vault.has_providers() {
        return ResolvedSession::Invalid;
    }
    state.sessions.insert(id);
    if state.sessions.contains_live(&id) {
        ResolvedSession::Present(id)
    } else {
        ResolvedSession::Invalid
    }
}

fn issue_restored_session(state: &AppState) -> Option<(SessionId, ValidatedToken)> {
    if !state.vault.has_providers() {
        return None;
    }
    let token = generate_session_token().ok()?;
    state.sessions.insert(token.id());
    if state.sessions.contains_live(&token.id()) {
        Some((token.id(), token.raw().clone()))
    } else {
        None
    }
}

#[derive(Clone)]
pub(crate) enum ResolvedSession {
    Anonymous,
    Invalid,
    Present(SessionId),
}

impl<S: Send + Sync> FromRequestParts<S> for ResolvedSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Self>()
            .cloned()
            .unwrap_or(Self::Anonymous))
    }
}

pub(crate) struct OptionalSession(pub(crate) Option<SessionId>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            match parts.extensions.get::<ResolvedSession>().cloned() {
                Some(ResolvedSession::Present(id)) => Some(id),
                _ => None,
            },
        ))
    }
}

pub(crate) struct RequiredSession(pub(crate) SessionId);

impl<S: Send + Sync> FromRequestParts<S> for RequiredSession {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<ResolvedSession>().cloned() {
            Some(ResolvedSession::Present(id)) => Ok(Self(id)),
            _ => {
                let graft = parts
                    .extensions
                    .get::<GraftRequest>()
                    .copied()
                    .unwrap_or_default();
                Err(crate::responses::request_navigation(graft, "/connect"))
            }
        }
    }
}

pub(crate) fn set_session_cookie(
    response: &mut Response,
    state: &AppState,
    raw: &ValidatedToken,
) -> AppResult<()> {
    let header =
        cookies::session_set_header(&state.config, raw).with_operation("build session cookie")?;
    response.headers_mut().append(header::SET_COOKIE, header);
    Ok(())
}

pub(crate) fn clear_session_cookie(response: &mut Response, state: &AppState) {
    expire_unless_replaced(response, state);
}

/// Drop expired sessions so API keys leave memory without a later request.
pub(crate) async fn purge_expired_sessions(state: AppState) {
    let mut interval = tokio::time::interval(SESSION_PURGE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        state
            .access_consent
            .retain_sessions(|session| state.sessions.contains_live(session));
        state
            .host_approvals
            .retain_sessions(|session| state.sessions.contains_live(session));
        state
            .conversations
            .retain_question_sessions(|session| state.sessions.contains_live(session));
        for session in state.sessions.expired_ids() {
            if crate::workflows::interrupt_session_continuations(&state, session).is_err() {
                continue;
            }
            state.sessions.remove(&session);
        }
    }
}

fn expire_unless_replaced(response: &mut Response, state: &AppState) {
    if cookies::response_has_replacement(response.headers()) {
        return;
    }
    match cookies::session_deletion_header(&state.config) {
        Ok(header) => {
            response.headers_mut().append(header::SET_COOKIE, header);
        }
        Err(error) => trace_operation_failure("expire session cookie", &error),
    }
}
