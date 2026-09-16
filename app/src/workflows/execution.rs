use std::sync::{Arc, Mutex};

enum Hold {
    Free,
    Shared(u32),
    Exclusive,
    Recovery,
}

pub(crate) struct WorkflowExecution {
    held: Mutex<Hold>,
    application: tokio::sync::Mutex<()>,
}

pub(crate) struct ExecutionGuard {
    execution: Arc<WorkflowExecution>,
    exclusive: bool,
}

impl WorkflowExecution {
    pub(crate) fn new() -> Self {
        Self {
            held: Mutex::new(Hold::Free),
            application: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn acquire(self: &Arc<Self>) -> Result<ExecutionGuard, &'static str> {
        let mut held = lock(&self.held);
        let count = match *held {
            Hold::Exclusive => return Err(crate::local_data::HOST_PATH_RESET_PENDING),
            Hold::Recovery => return Err(RECOVERY_REQUIRED),
            Hold::Free => 0,
            Hold::Shared(count) => count,
        };
        *held = Hold::Shared(
            count
                .checked_add(1)
                .ok_or("Too many executions are active.")?,
        );
        Ok(ExecutionGuard {
            execution: Arc::clone(self),
            exclusive: false,
        })
    }

    // The caller retains this lock through cleanup, recovery and publication.
    pub(crate) async fn lock_application(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, &'static str> {
        let application = self.application.lock().await;
        if matches!(*lock(&self.held), Hold::Recovery) {
            return Err(RECOVERY_REQUIRED);
        }
        Ok(application)
    }

    pub(crate) fn acquire_exclusive(self: &Arc<Self>) -> Result<ExecutionGuard, ()> {
        let mut held = lock(&self.held);
        if !matches!(*held, Hold::Free) {
            return Err(());
        }
        *held = Hold::Exclusive;
        Ok(ExecutionGuard {
            execution: Arc::clone(self),
            exclusive: true,
        })
    }

    fn release(&self, exclusive: bool) {
        let mut held = lock(&self.held);
        if matches!(*held, Hold::Recovery) {
            return;
        }
        if exclusive {
            *held = Hold::Free;
            return;
        }
        *held = match *held {
            Hold::Shared(0 | 1) | Hold::Free => Hold::Free,
            Hold::Shared(count) => Hold::Shared(count - 1),
            Hold::Exclusive => Hold::Exclusive,
            Hold::Recovery => Hold::Recovery,
        };
    }
}

const RECOVERY_REQUIRED: &str =
    "Restart Power Plant to reconcile unresolved execution recovery before another operation.";

impl ExecutionGuard {
    pub(crate) fn require_recovery(&self) {
        // Recovery survives every lease release until process restart.
        *lock(&self.execution.held) = Hold::Recovery;
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.execution.release(self.exclusive);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
