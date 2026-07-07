use std::ops::Deref;
use std::sync::Arc;

use anyhow::Result;

use super::SessionRuntime;
use super::SessionRuntimeBuildOpts;

/// Cheap cloneable capability for a live session runtime.
///
/// This is the first local shape of the Phase A `SessionHandle` boundary. It
/// intentionally keeps an explicit runtime escape hatch while call sites are
/// migrated off direct `Arc<SessionRuntime>` ownership.
#[derive(Clone)]
pub struct SessionHandle {
    runtime: Arc<SessionRuntime>,
}

impl SessionHandle {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        Self { runtime }
    }

    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Self> {
        let runtime = SessionRuntime::build(opts).await?;
        Ok(Self::new(runtime))
    }

    pub fn runtime(&self) -> &Arc<SessionRuntime> {
        &self.runtime
    }

    pub fn into_runtime(self) -> Arc<SessionRuntime> {
        self.runtime
    }
}

impl Deref for SessionHandle {
    type Target = SessionRuntime;

    fn deref(&self) -> &Self::Target {
        self.runtime.as_ref()
    }
}
