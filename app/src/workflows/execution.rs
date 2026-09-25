#[cfg(feature = "dev")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    #[cfg(feature = "dev")]
    active: AtomicUsize,
}

pub(crate) struct ExecutionGuard {
    execution: Arc<WorkflowExecution>,
    exclusive: bool,
    #[cfg(feature = "dev")]
    blocks_restart: AtomicBool,
}

impl WorkflowExecution {
    pub(crate) fn new() -> Self {
        Self {
            held: Mutex::new(Hold::Free),
            application: tokio::sync::Mutex::new(()),
            #[cfg(feature = "dev")]
            active: AtomicUsize::new(0),
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
        Ok(ExecutionGuard::new(self, false))
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

    #[cfg(feature = "dev")]
    pub(crate) fn has_active_work(&self) -> bool {
        // Recovery can coexist with another operation's unfinished cleanup.
        self.active.load(Ordering::SeqCst) != 0
    }

    pub(crate) fn acquire_exclusive(self: &Arc<Self>) -> Result<ExecutionGuard, ()> {
        let mut held = lock(&self.held);
        if !matches!(*held, Hold::Free) {
            return Err(());
        }
        *held = Hold::Exclusive;
        Ok(ExecutionGuard::new(self, true))
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
    "Restart Frinkworks to reconcile unresolved execution recovery before another operation.";

impl ExecutionGuard {
    fn new(execution: &Arc<WorkflowExecution>, exclusive: bool) -> Self {
        #[cfg(feature = "dev")]
        execution.active.fetch_add(1, Ordering::SeqCst);
        Self {
            execution: Arc::clone(execution),
            exclusive,
            #[cfg(feature = "dev")]
            blocks_restart: AtomicBool::new(true),
        }
    }

    pub(crate) fn require_recovery(&self) {
        // Recovery survives every lease release until process restart.
        *lock(&self.execution.held) = Hold::Recovery;
        #[cfg(feature = "dev")]
        self.release_restart_blocker();
    }

    #[cfg(feature = "dev")]
    fn release_restart_blocker(&self) {
        if self.blocks_restart.swap(false, Ordering::SeqCst) {
            self.execution.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.execution.release(self.exclusive);
        #[cfg(feature = "dev")]
        self.release_restart_blocker();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
