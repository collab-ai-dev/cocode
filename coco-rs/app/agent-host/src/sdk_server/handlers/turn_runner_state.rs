use std::sync::Arc;

use tokio::sync::RwLock;

use super::NotImplementedRunner;
use super::TurnRunner;

pub(super) struct TurnRunnerState {
    runner: RwLock<Arc<dyn TurnRunner>>,
}

impl Default for TurnRunnerState {
    fn default() -> Self {
        Self {
            runner: RwLock::new(Arc::new(NotImplementedRunner) as Arc<dyn TurnRunner>),
        }
    }
}

impl TurnRunnerState {
    pub(super) fn install_for_startup(&self, runner: Arc<dyn TurnRunner>) {
        let Ok(mut slot) = self.runner.try_write() else {
            panic!("with_turn_runner: state was already locked at construction time");
        };
        *slot = runner;
    }

    pub(super) async fn install(&self, runner: Arc<dyn TurnRunner>) {
        let mut slot = self.runner.write().await;
        *slot = runner;
    }

    pub(super) async fn snapshot(&self) -> Arc<dyn TurnRunner> {
        self.runner.read().await.clone()
    }

    /// Revert to the fail-closed runner when the backing session runtime is torn
    /// down. A later `session/start` reinstalls a real runner.
    pub(super) async fn clear(&self) {
        let mut slot = self.runner.write().await;
        *slot = Arc::new(NotImplementedRunner) as Arc<dyn TurnRunner>;
    }
}
