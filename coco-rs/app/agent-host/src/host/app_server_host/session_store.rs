use std::sync::Arc;

use tokio::sync::RwLock;

pub(crate) struct SessionStore {
    manager: RwLock<Option<Arc<coco_session::SessionManager>>>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new(None)
    }
}

impl SessionStore {
    pub(crate) fn new(manager: Option<Arc<coco_session::SessionManager>>) -> Self {
        Self {
            manager: RwLock::new(manager),
        }
    }

    pub(crate) async fn install(&self, manager: Arc<coco_session::SessionManager>) {
        *self.manager.write().await = Some(manager);
    }

    pub(crate) async fn snapshot(&self) -> Option<Arc<coco_session::SessionManager>> {
        self.manager.read().await.as_ref().map(Arc::clone)
    }
}
