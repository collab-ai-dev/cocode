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

    /// Clear the installed runtime only when it still matches `session_id`.
    ///
    /// The compare-and-clear runs under the same write lock as `install`, so a
    /// concurrent replacement swap that already installed a different runtime is
    /// never torn down by a stale close. Returns whether
    /// the slot was cleared.
    pub(super) async fn clear_if_matches(&self, session_id: &coco_types::SessionId) -> bool {
        let mut slot = self.runtime.write().await;
        let matches = slot
            .as_ref()
            .is_some_and(|handle| handle.session_id().as_str() == session_id.as_str());
        if matches {
            *slot = None;
        }
        matches
    }
}
