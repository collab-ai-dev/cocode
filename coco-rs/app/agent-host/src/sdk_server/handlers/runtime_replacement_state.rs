use tokio::sync::RwLock;

use super::RuntimeReplacementContext;

#[derive(Default)]
pub(super) struct RuntimeReplacementState {
    context: RwLock<Option<RuntimeReplacementContext>>,
}

impl RuntimeReplacementState {
    pub(super) async fn install(&self, context: RuntimeReplacementContext) {
        let mut slot = self.context.write().await;
        *slot = Some(context);
    }

    pub(super) async fn snapshot(&self) -> Option<RuntimeReplacementContext> {
        self.context.read().await.clone()
    }
}
