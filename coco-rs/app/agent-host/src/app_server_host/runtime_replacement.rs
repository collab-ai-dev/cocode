use std::{path::PathBuf, sync::Arc};

use tokio::sync::RwLock;

#[derive(Clone)]
pub struct RuntimeReplacementContext {
    pub startup_session_id: coco_types::SessionId,
    pub runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    pub process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
    pub cwd: PathBuf,
    pub requires_structured_output: bool,
}

#[derive(Default)]
pub(crate) struct RuntimeReplacementState {
    context: RwLock<Option<RuntimeReplacementContext>>,
}

impl RuntimeReplacementState {
    pub(crate) async fn install(&self, context: RuntimeReplacementContext) {
        let mut slot = self.context.write().await;
        *slot = Some(context);
    }

    pub(crate) async fn snapshot(&self) -> Option<RuntimeReplacementContext> {
        self.context.read().await.clone()
    }
}
