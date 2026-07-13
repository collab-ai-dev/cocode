use std::{future::Future, pin::Pin, sync::Arc};

use coco_types::CoreEvent;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Abstraction over how the host executes a single turn.
///
/// Every call receives the AppServer-selected session capability explicitly.
/// Remote and local adapters share this boundary, then map emitted events to their
/// own transport or UI policy.
pub trait TurnRunner: Send + Sync {
    fn run_turn<'a>(
        &'a self,
        session: crate::session_runtime::SessionHandle,
        app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        event_tx: mpsc::Sender<CoreEvent>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}

pub struct NotImplementedRunner;

impl TurnRunner for NotImplementedRunner {
    fn run_turn<'a>(
        &'a self,
        _session: crate::session_runtime::SessionHandle,
        _app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        _params: coco_types::TurnStartParams,
        _turn_id: coco_types::TurnId,
        _event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async {
            anyhow::bail!(
                "AppServerHost was constructed without a TurnRunner; \
                 install a SessionTurnExecutor before handling turn/start"
            )
        })
    }
}

pub(crate) struct TurnRunnerState {
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
    pub(crate) fn new(runner: Option<Arc<dyn TurnRunner>>) -> Self {
        Self {
            runner: RwLock::new(
                runner.unwrap_or_else(|| Arc::new(NotImplementedRunner) as Arc<dyn TurnRunner>),
            ),
        }
    }

    pub(crate) async fn install(&self, runner: Arc<dyn TurnRunner>) {
        let mut slot = self.runner.write().await;
        *slot = runner;
    }

    pub(crate) async fn snapshot(&self) -> Arc<dyn TurnRunner> {
        self.runner.read().await.clone()
    }
}
