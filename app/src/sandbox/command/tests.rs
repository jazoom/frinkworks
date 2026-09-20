use super::*;

impl CommandSession {
    pub(in crate::sandbox) fn scripted(command: ScriptedCommand) -> Self {
        Self {
            inner: CommandInner::Scripted(command),
            _lifecycle: None,
        }
    }
}

impl ScriptedCommand {
    pub(crate) fn output(text: String, code: i32) -> Self {
        Self {
            events: Mutex::new(vec![
                CommandEvent::Output {
                    stream: crate::execution::CommandStream::Stdout,
                    bytes: text.into_bytes(),
                },
                CommandEvent::Exited(code),
            ]),
            hang: false,
            killed: Mutex::new(false),
            notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn hang() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            hang: true,
            killed: Mutex::new(false),
            notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) async fn recv(&self) -> Option<CommandEvent> {
        loop {
            if *lock_mutex(&self.killed) {
                return Some(CommandEvent::Exited(137));
            }
            {
                let mut events = lock_mutex(&self.events);
                if !events.is_empty() {
                    return Some(events.remove(0));
                }
            }
            if !self.hang {
                return None;
            }
            let notified = self.notify.notified();
            if *lock_mutex(&self.killed) {
                return Some(CommandEvent::Exited(137));
            }
            notified.await;
        }
    }

    pub(crate) fn kill(&self) {
        *lock_mutex(&self.killed) = true;
        self.notify.notify_waiters();
    }
}
