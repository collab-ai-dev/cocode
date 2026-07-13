use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct RuntimeReplacementContext {
    pub runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    pub process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
    pub cwd: PathBuf,
    pub requires_structured_output: bool,
    pub integration_options: crate::session_bootstrap::SessionIntegrationOptions,
}

#[derive(Default)]
pub(crate) struct RuntimeReplacementState {
    context: Option<RuntimeReplacementContext>,
}

impl RuntimeReplacementState {
    pub(crate) fn new(context: Option<RuntimeReplacementContext>) -> Self {
        Self { context }
    }

    pub(crate) async fn snapshot(&self) -> Option<RuntimeReplacementContext> {
        self.context.clone()
    }
}
