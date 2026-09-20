use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

use super::{Inner, Source};

impl super::DirectoryPicker {
    pub(crate) fn scripted() -> Self {
        Self {
            inner: Arc::new(Inner {
                permit: Arc::new(Semaphore::new(1)),
                source: Source::Scripted(Mutex::new(VecDeque::new())),
            }),
        }
    }

    pub(crate) fn queue(&self, selected: Option<PathBuf>) {
        let Source::Scripted(script) = &self.inner.source else {
            panic!("scripted directory picker");
        };
        super::lock(script).push_back(selected);
    }
}
