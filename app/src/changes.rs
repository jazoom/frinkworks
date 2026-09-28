use tokio::sync::broadcast;

#[derive(Clone)]
pub(crate) struct Changes(broadcast::Sender<()>);

impl Default for Changes {
    fn default() -> Self {
        Self(broadcast::channel(16).0)
    }
}

impl Changes {
    pub(crate) fn notify(&self) {
        let _ = self.0.send(());
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<()> {
        self.0.subscribe()
    }
}
