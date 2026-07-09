use std::ops::Deref;
use std::sync::Arc;

use anyhow::Result;
use coco_types::SessionId;

use super::SessionRuntime;
use super::SessionRuntimeBuildOpts;

/// Cheap cloneable capability for a live session runtime.
///
/// This is the first local shape of the Phase A `SessionHandle` boundary. It
/// carries an immutable session-id snapshot plus an explicit runtime escape
/// hatch while call sites are migrated off direct `Arc<SessionRuntime>`
/// ownership.
#[derive(Clone)]
pub struct SessionHandle {
    session_id: SessionId,
    runtime: Arc<SessionRuntime>,
}

impl SessionHandle {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        let session_id = runtime.current_typed_session_id_snapshot();
        Self {
            session_id,
            runtime,
        }
    }

    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Self> {
        let runtime = SessionRuntime::build(opts).await?;
        Ok(Self::new(runtime))
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn runtime(&self) -> &Arc<SessionRuntime> {
        &self.runtime
    }

    pub fn orchestration_ctx_factory(
        &self,
    ) -> Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync> {
        self.runtime.orchestration_ctx_factory()
    }
}

impl Deref for SessionHandle {
    type Target = SessionRuntime;

    fn deref(&self) -> &Self::Target {
        self.runtime.as_ref()
    }
}
