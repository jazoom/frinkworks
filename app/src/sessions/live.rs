use std::sync::Arc;

use axum::http::Extensions;
use hypergraft::live::{AdmissionPermit, GuardFailure, LiveGuard, SocketAdmission};

use crate::sessions::SessionStore;

use super::{ResolvedSession, SessionId};

const MAXIMUM_SOCKETS_PER_SESSION: usize = 8;

#[derive(Clone)]
pub(crate) struct LiveSessionGuard {
    sessions: Arc<SessionStore>,
    admission: SocketAdmission<SessionId>,
}

impl LiveSessionGuard {
    pub(crate) fn new(sessions: Arc<SessionStore>) -> Self {
        Self {
            sessions,
            admission: SocketAdmission::new(),
        }
    }
}

pub(crate) struct LiveConnection {
    session: SessionId,
    _permit: AdmissionPermit<SessionId>,
}

impl LiveGuard for LiveSessionGuard {
    type Connection = LiveConnection;
    type Context = SessionId;

    async fn bind(&self, extensions: &Extensions) -> Result<Self::Connection, GuardFailure> {
        let Some(ResolvedSession::Present(session)) = extensions.get::<ResolvedSession>() else {
            return Err(GuardFailure::Terminal);
        };
        let permit = self
            .admission
            .try_acquire(*session, MAXIMUM_SOCKETS_PER_SESSION)
            .map_err(|_| GuardFailure::Terminal)?;
        Ok(LiveConnection {
            session: *session,
            _permit: permit,
        })
    }

    async fn revalidate(
        &self,
        connection: &Self::Connection,
    ) -> Result<Self::Context, GuardFailure> {
        if self.sessions.contains_live(&connection.session) {
            Ok(connection.session)
        } else {
            Err(GuardFailure::Terminal)
        }
    }
}
