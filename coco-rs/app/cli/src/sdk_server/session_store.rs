use std::sync::Arc;

use tokio::sync::RwLock;

#[derive(Default)]
pub(crate) struct SessionStore {
    manager: RwLock<Option<Arc<coco_session::SessionManager>>>,
}

impl SessionStore {
    pub(crate) fn install_for_startup(&self, manager: Arc<coco_session::SessionManager>) {
        let Ok(mut slot) = self.manager.try_write() else {
            panic!("SessionStore::install_for_startup: state was already locked");
        };
        *slot = Some(manager);
    }

    pub(crate) async fn install(&self, manager: Arc<coco_session::SessionManager>) {
        *self.manager.write().await = Some(manager);
    }

    pub(crate) async fn snapshot(&self) -> Option<Arc<coco_session::SessionManager>> {
        self.manager.read().await.as_ref().map(Arc::clone)
    }
}
