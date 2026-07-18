use std::{sync::Arc, time::Duration};

use coco_app_server::{
    AppServer, JsonRpcDispatchError, JsonRpcRequestContext, JsonRpcRequestFuture,
    JsonRpcRequestHandler, LocalClientRequestContext, LocalClientRequestFuture,
    LocalClientRequestHandler,
};
use coco_types::ClientRequest;
use tokio::sync::mpsc;

use crate::app_session::AppSessionHandle;

use crate::app_server_host::OutboundMessage;

use super::config::APP_SERVER_TURN_DRAIN_TIMEOUT;
use super::connection_profile::{
    ConnectionProfileSlot, empty_connection_profile_slot, local_connection_profile_slot,
    resolve_connection_profile_for_request,
};
use super::handler_error_mapping::{
    dispatch_app_server_client_request, encode_app_server_result, session_operation_error,
};
use super::request_targeting::resolve_request_runtime;
use super::runtime_replacement_gate::require_runtime_replacement;
use super::session_data::{LocalSessionDataRequest, LocalSessionDataView};
use super::session_local_operations::apply_local_session_operation;
use super::session_operation_input::LocalSessionOperation;
use super::session_replace_operation::replace_app_server_session_with_runtime;
use super::session_request_mapping::{
    session_replace_input_from_params, session_resume_input_from_params,
    session_start_input_from_params,
};
use super::session_resume_operation::resume_app_server_session_with_runtime_replacement;
use super::session_start_operation::start_app_server_session_with_runtime_replacement;
use super::session_surfaces::subscribe_local_app_server_session;
use super::{AppServerHostState, HandlerContext};

/// Runtime-backed request handler for AppServer adapters.
#[derive(Clone)]
pub struct AppServerHostHandler {
    pub(crate) state: Arc<AppServerHostState>,
    notif_tx: mpsc::Sender<OutboundMessage>,
    local_app_server: Option<Arc<AppServer<AppSessionHandle>>>,
    pub(crate) turn_drain_timeout: Duration,
    connection_profile: ConnectionProfileSlot,
    require_initialize: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestOrigin {
    Local,
    Remote,
}

fn targeted_session_id(request: &ClientRequest) -> Option<&coco_types::SessionId> {
    if let ClientRequest::SessionClose(params) = request {
        return Some(params.target.session_id());
    }
    request
        .interactive_target()
        .map(|target| &target.session_id)
        .or_else(|| request.session_target().map(|target| &target.session_id))
}

impl AppServerHostHandler {
    pub async fn set_turn_runner(&self, runner: Arc<dyn super::TurnRunner>) {
        self.state.install_turn_runner(runner).await;
    }

    pub fn new(state: Arc<AppServerHostState>, notif_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: None,
            turn_drain_timeout: APP_SERVER_TURN_DRAIN_TIMEOUT,
            connection_profile: empty_connection_profile_slot(),
            require_initialize: true,
        }
    }

    pub fn with_local_app_server(
        state: Arc<AppServerHostState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<AppSessionHandle>>,
    ) -> Self {
        Self::with_local_app_server_and_turn_drain_timeout(
            state,
            notif_tx,
            app_server,
            APP_SERVER_TURN_DRAIN_TIMEOUT,
        )
    }

    pub fn with_local_app_server_and_turn_drain_timeout(
        state: Arc<AppServerHostState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<AppSessionHandle>>,
        turn_drain_timeout: Duration,
    ) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: Some(app_server),
            turn_drain_timeout,
            connection_profile: local_connection_profile_slot(),
            require_initialize: false,
        }
    }

    fn profile_for_request(
        &self,
        request: &ClientRequest,
    ) -> Result<Arc<coco_types::ConnectionProfile>, JsonRpcDispatchError> {
        resolve_connection_profile_for_request(
            &self.connection_profile,
            self.require_initialize,
            request,
        )
    }

    fn handle_client_request_for_connection(
        &self,
        connection: coco_app_server::ConnectionKey,
        request: ClientRequest,
        origin: RequestOrigin,
    ) -> JsonRpcRequestFuture {
        let connection_profile = match self.profile_for_request(&request) {
            Ok(profile) => profile,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let local_app_server = self.local_app_server.clone();
        if origin == RequestOrigin::Remote
            && let (Some(app_server), Some(session_id)) =
                (local_app_server.as_ref(), targeted_session_id(&request))
            && app_server
                .registry()
                .policy(session_id)
                .is_some_and(|policy| policy.is_internal())
        {
            let session_id = session_id.clone();
            return Box::pin(async move {
                Err(JsonRpcDispatchError {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "session is not available to remote clients".to_string(),
                    data: Some(serde_json::json!({
                        "kind": "internal_session",
                        "session_id": session_id,
                    })),
                })
            });
        }
        if let (Some(app_server), ClientRequest::SessionSubscribe(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            return Box::pin(async move {
                subscribe_local_app_server_session(app_server, connection, params).await
            });
        }
        if let (Some(app_server), ClientRequest::CancelRequest(params)) =
            (local_app_server.clone(), &request)
        {
            let request_id = coco_types::RequestId::String(params.request_id.clone());
            return Box::pin(async move {
                app_server
                    .cancel_server_request_for_connection(connection, &request_id)
                    .map(|_| serde_json::Value::Null)
                    .map_err(|error| JsonRpcDispatchError {
                        code: coco_types::error_codes::INVALID_REQUEST,
                        message: error.to_string(),
                        data: Some(serde_json::json!({
                            "kind": "pending_request_mismatch",
                            "request_id": request_id,
                        })),
                    })
            });
        }
        if let (Some(app_server), ClientRequest::SessionClose(params)) =
            (local_app_server.clone(), &request)
        {
            let state = Arc::clone(&self.state);
            let notif_tx = self.notif_tx.clone();
            let turn_drain_timeout = self.turn_drain_timeout;
            let target = params.target.clone();
            return Box::pin(async move {
                apply_local_session_operation(
                    app_server,
                    state,
                    LocalSessionOperation::Close { connection, target },
                    turn_drain_timeout,
                    notif_tx,
                )
                .await
                .map_err(session_operation_error)?;
                encode_app_server_result(())
            });
        }
        let session_data_request = local_app_server
            .as_ref()
            .and_then(|_| LocalSessionDataRequest::from_client_request(&request));
        if let (Some(app_server), Some(session_data_request)) =
            (local_app_server.clone(), session_data_request)
        {
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                LocalSessionDataView { app_server, state }
                    .handle(&session_data_request)
                    .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionStart(params)) =
            (local_app_server.clone(), &request)
        {
            let input = session_start_input_from_params(params);
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                let replacement =
                    require_runtime_replacement(&state, "session/start", true).await?;
                start_app_server_session_with_runtime_replacement(
                    app_server,
                    state,
                    connection,
                    input,
                    connection_profile,
                    replacement,
                )
                .await
                .map_err(session_operation_error)
                .and_then(encode_app_server_result)
            });
        }
        if let (Some(app_server), ClientRequest::SessionResume(params)) =
            (local_app_server.clone(), &request)
        {
            let input = session_resume_input_from_params(params);
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement =
                    require_runtime_replacement(&state, "session/resume", true).await?;
                resume_app_server_session_with_runtime_replacement(
                    app_server,
                    state,
                    connection,
                    input,
                    connection_profile,
                    replacement,
                    turn_drain_timeout,
                )
                .await
                .map_err(session_operation_error)
                .and_then(encode_app_server_result)
            });
        }
        if let (Some(app_server), ClientRequest::SessionReplace(params)) =
            (local_app_server.clone(), &request)
        {
            let input = session_replace_input_from_params(params);
            let state = Arc::clone(&self.state);
            let turn_drain_timeout = self.turn_drain_timeout;
            return Box::pin(async move {
                let replacement =
                    require_runtime_replacement(&state, "session/replace", false).await?;
                replace_app_server_session_with_runtime(
                    app_server,
                    state,
                    connection,
                    input,
                    connection_profile,
                    replacement,
                    turn_drain_timeout,
                )
                .await
                .map_err(session_operation_error)
                .and_then(encode_app_server_result)
            });
        }
        let (target_session_id, session) =
            match resolve_request_runtime(local_app_server.as_ref(), connection, &request) {
                Ok(resolved) => resolved,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
            connection_profile,
            app_server: local_app_server,
            target_session_id,
            session,
        };
        Box::pin(async move { dispatch_app_server_client_request(request, ctx).await })
    }
}

impl JsonRpcRequestHandler for AppServerHostHandler {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        self.handle_client_request_for_connection(
            context.connection,
            request,
            RequestOrigin::Remote,
        )
    }
}

impl LocalClientRequestHandler for AppServerHostHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        self.handle_client_request_for_connection(
            context.connection_key(),
            request,
            RequestOrigin::Local,
        )
    }
}

impl coco_app_server::JsonRpcConnectionHandlerFactory for AppServerHostHandler {
    type Handler = Self;

    fn open(&self, _connection: coco_app_server::ConnectionKey) -> Arc<Self::Handler> {
        Arc::new(Self {
            state: Arc::clone(&self.state),
            notif_tx: self.notif_tx.clone(),
            local_app_server: self.local_app_server.clone(),
            turn_drain_timeout: self.turn_drain_timeout,
            connection_profile: empty_connection_profile_slot(),
            require_initialize: true,
        })
    }
}
