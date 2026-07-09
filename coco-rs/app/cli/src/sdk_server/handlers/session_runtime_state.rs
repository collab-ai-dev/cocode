use tokio::sync::RwLock;

type SessionHandle = crate::session_runtime::SessionHandle;

#[derive(Default)]
pub(super) struct SessionRuntimeState {
    runtime: RwLock<Option<SessionHandle>>,
}

impl SessionRuntimeState {
    pub(super) fn install_for_startup(&self, runtime: SessionHandle) {
        let Ok(mut slot) = self.runtime.try_write() else {
            panic!("with_session_handle: state was already locked at construction time");
        };
        *slot = Some(runtime);
    }

    pub(super) async fn install(&self, runtime: SessionHandle) {
        let mut slot = self.runtime.write().await;
        *slot = Some(runtime);
    }

    pub(super) async fn snapshot(&self) -> Option<SessionHandle> {
        self.runtime.read().await.clone()
    }

    pub(super) async fn is_installed(&self) -> bool {
        self.runtime.read().await.is_some()
    }
}
