use tokio::sync::Mutex;

#[derive(Default)]
pub(super) struct RuntimeReloadState {
    subscription: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl RuntimeReloadState {
    pub(super) async fn abort_current(&self) {
        let mut slot = self.subscription.lock().await;
        if let Some(handle) = slot.take() {
            handle.abort();
        }
    }

    pub(super) async fn install(&self, handle: tokio::task::JoinHandle<()>) {
        let mut slot = self.subscription.lock().await;
        if let Some(handle) = slot.take() {
            handle.abort();
        }
        *slot = Some(handle);
    }
}
