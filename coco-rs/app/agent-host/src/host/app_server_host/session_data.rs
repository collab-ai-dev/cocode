use std::sync::Arc;

use futures::future::BoxFuture;

use coco_app_server::{
    AppServer, AppSessionDataError, AppSessionDataRequest, AppSessionDataSource,
    JsonRpcDispatchError, LiveSessionDataSnapshot,
};
use coco_types::SessionId;

use super::AppServerHostState;
use crate::app_session::AppSessionHandle;

pub(crate) type LocalSessionDataRequest = AppSessionDataRequest;

pub(crate) struct LocalSessionDataView {
    pub(crate) app_server: Arc<AppServer<AppSessionHandle>>,
    pub(crate) state: Arc<AppServerHostState>,
}

impl LocalSessionDataView {
    pub(crate) async fn handle(
        &self,
        request: &LocalSessionDataRequest,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        self.app_server
            .handle_session_data_request(request, self)
            .await
    }
}

impl AppSessionDataSource for LocalSessionDataView {
    fn list_persisted_sessions(
        &self,
    ) -> BoxFuture<'_, Result<coco_types::SessionListResult, AppSessionDataError>> {
        Box::pin(async move {
            crate::session_data::persisted_session_list(self.state.session_manager_snapshot().await)
                .await
        })
    }

    fn read_persisted_session(
        &self,
        params: coco_types::SessionReadParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionReadResult, AppSessionDataError>> {
        Box::pin(async move {
            crate::session_data::persisted_session_read(
                self.state.session_manager_snapshot().await,
                &params,
            )
            .await
        })
    }

    fn list_persisted_session_turns(
        &self,
        params: coco_types::SessionTurnsListParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionTurnsListResult, AppSessionDataError>> {
        Box::pin(async move {
            crate::session_data::persisted_session_turns_list(
                self.state.session_manager_snapshot().await,
                &params,
            )
            .await
        })
    }

    fn live_session_fallback(
        &self,
        _session_id: SessionId,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>> {
        Box::pin(async { Ok(None) })
    }
}
