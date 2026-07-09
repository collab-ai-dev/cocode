use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

type SharedFileHistory = Arc<RwLock<coco_context::FileHistoryState>>;

#[derive(Default)]
pub(super) struct FileHistoryStateSlot {
    history: RwLock<Option<SharedFileHistory>>,
    config_home: RwLock<Option<PathBuf>>,
}

impl FileHistoryStateSlot {
    pub(super) fn install_for_startup(&self, history: SharedFileHistory, config_home: PathBuf) {
        {
            let Ok(mut slot) = self.history.try_write() else {
                panic!("with_file_history: state was already locked at construction time");
            };
            *slot = Some(history);
        }
        {
            let Ok(mut slot) = self.config_home.try_write() else {
                panic!("with_file_history: state was already locked at construction time");
            };
            *slot = Some(config_home);
        }
    }

    pub(super) async fn install(
        &self,
        history: Option<SharedFileHistory>,
        config_home: Option<PathBuf>,
    ) {
        *self.history.write().await = history;
        *self.config_home.write().await = config_home;
    }

    pub(super) async fn history_snapshot(&self) -> Option<SharedFileHistory> {
        self.history.read().await.clone()
    }

    pub(super) async fn config_home_snapshot(&self) -> Option<PathBuf> {
        self.config_home.read().await.clone()
    }
}
