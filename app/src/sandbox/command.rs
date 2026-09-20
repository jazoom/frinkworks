#[cfg(test)]
mod tests;

#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
use tokio::sync::Notify;

use tokio::sync::OwnedMutexGuard;

use super::SandboxError;
#[cfg(test)]
use super::lock_mutex;

pub(crate) enum CommandEvent {
    /// Raw bytes in adapter receive order. Incomplete UTF-8 is decoded later.
    Output {
        stream: crate::execution::CommandStream,
        bytes: Vec<u8>,
    },
    Exited(i32),
    Failed,
}

pub(crate) struct CommandSession {
    inner: CommandInner,
    // Drop after the command ends so stop/start cannot run mid-exec.
    _lifecycle: Option<OwnedMutexGuard<()>>,
}

enum CommandInner {
    Microsandbox(Box<MicrosandboxCommand>),
    #[cfg(test)]
    Scripted(ScriptedCommand),
}

struct MicrosandboxCommand {
    sandbox: microsandbox::Sandbox,
    exec: microsandbox::ExecHandle,
}

#[cfg(test)]
pub(super) struct ScriptedCommand {
    events: Mutex<Vec<CommandEvent>>,
    hang: bool,
    killed: Mutex<bool>,
    notify: Arc<Notify>,
}

impl CommandSession {
    pub(super) fn microsandbox(
        sandbox: microsandbox::Sandbox,
        exec: microsandbox::ExecHandle,
    ) -> Self {
        Self {
            inner: CommandInner::Microsandbox(Box::new(MicrosandboxCommand { sandbox, exec })),
            _lifecycle: None,
        }
    }

    pub(super) fn with_lifecycle(mut self, lifecycle: OwnedMutexGuard<()>) -> Self {
        self._lifecycle = Some(lifecycle);
        self
    }

    pub(crate) async fn recv(&mut self) -> Option<CommandEvent> {
        match &mut self.inner {
            CommandInner::Microsandbox(command) => loop {
                match command.exec.recv().await? {
                    microsandbox::ExecEvent::Stdout(data) => {
                        if data.is_empty() {
                            continue;
                        }
                        return Some(CommandEvent::Output {
                            stream: crate::execution::CommandStream::Stdout,
                            bytes: data.to_vec(),
                        });
                    }
                    microsandbox::ExecEvent::Stderr(data) => {
                        if data.is_empty() {
                            continue;
                        }
                        return Some(CommandEvent::Output {
                            stream: crate::execution::CommandStream::Stderr,
                            bytes: data.to_vec(),
                        });
                    }
                    microsandbox::ExecEvent::Exited { code } => {
                        return Some(CommandEvent::Exited(code));
                    }
                    microsandbox::ExecEvent::Failed(_) => {
                        return Some(CommandEvent::Failed);
                    }
                    microsandbox::ExecEvent::Started { .. }
                    | microsandbox::ExecEvent::StdinError(_) => {}
                }
            },
            #[cfg(test)]
            CommandInner::Scripted(command) => command.recv().await,
        }
    }

    pub(crate) async fn kill(&self) {
        match &self.inner {
            CommandInner::Microsandbox(command) => {
                let _ = command.exec.kill().await;
            }
            #[cfg(test)]
            CommandInner::Scripted(command) => command.kill(),
        }
    }

    pub(crate) async fn close(self) {
        match self.inner {
            CommandInner::Microsandbox(command) => {
                command.sandbox.detach().await;
            }
            #[cfg(test)]
            CommandInner::Scripted(_) => {}
        }
    }
}

pub(super) fn map_exec_error(error: microsandbox::MicrosandboxError) -> SandboxError {
    super::map_error(error, SandboxError::Exec)
}
