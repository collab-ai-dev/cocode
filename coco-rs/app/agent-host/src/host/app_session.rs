use std::sync::Arc;

use futures::future::BoxFuture;

use coco_app_server::{AppSessionDataError, AppSessionDataHandle, LiveSessionDataSnapshot};
use coco_types::SessionId;

/// AppServer registry handle for an application-host session.
///
/// The registry id is an immutable snapshot checked against the runtime during
/// close cascades. Runtime replacement installs a fresh handle instead of
/// mutating an existing handle in place.
#[derive(Clone)]
pub struct AppSessionHandle {
    session_id: SessionId,
    runtime: crate::session_runtime::SessionHandle,
}

impl AppSessionHandle {
    pub fn from_runtime(runtime: crate::session_runtime::SessionHandle) -> Self {
        let session_id = runtime.session_id().clone();
        Self {
            session_id,
            runtime,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub(crate) fn runtime(&self) -> &crate::session_runtime::SessionHandle {
        &self.runtime
    }

    pub fn into_session(self) -> crate::session_runtime::SessionHandle {
        self.runtime
    }

    pub(crate) async fn live_summary_and_history(
        &self,
    ) -> (coco_types::SessionSummary, Vec<Arc<coco_messages::Message>>) {
        self.runtime.live_session_summary_and_history().await
    }
}

impl AppSessionDataHandle for AppSessionHandle {
    fn session_data_snapshot(
        &self,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>> {
        Box::pin(async move {
            let snapshot = crate::session_data::live_session_data_snapshot(
                self.live_summary_and_history().await,
            )?;
            Ok(Some(snapshot))
        })
    }
}
