use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::RwLock;

type SharedMcpManager = Arc<Mutex<coco_mcp::McpConnectionManager>>;

#[derive(Default)]
pub(super) struct McpManagerState {
    manager: RwLock<Option<SharedMcpManager>>,
}

impl McpManagerState {
    pub(super) fn install_for_startup(&self, manager: SharedMcpManager) {
        let Ok(mut slot) = self.manager.try_write() else {
            panic!("with_mcp_manager: state was already locked at construction time");
        };
        *slot = Some(manager);
    }

    pub(super) async fn install(&self, manager: SharedMcpManager) {
        let mut slot = self.manager.write().await;
        *slot = Some(manager);
    }

    pub(super) async fn snapshot(&self) -> Option<SharedMcpManager> {
        self.manager.read().await.clone()
    }
}
