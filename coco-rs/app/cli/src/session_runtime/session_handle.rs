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

    /// Create a new handle snapshot for the runtime's current mutable session id.
    ///
    /// This is a compatibility bridge for the remaining in-place retarget paths
    /// (`/clear`, `/resume`, SDK archive/start cycling). The steady-state
    /// runtime split should construct a new handle instead of retargeting.
    pub fn snapshot_current(&self) -> Self {
        Self::new(Arc::clone(&self.runtime))
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
