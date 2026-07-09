//! Bridge from the new AppServer JSON-RPC adapter to the existing SDK handlers.
//!
//! This keeps runtime semantics in `coco-cli` while allowing
//! `coco-app-server` to own JSON-RPC connection dispatch and routing.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use coco_app_server::AppCloseStart;
use coco_app_server::AppLoadStart;
use coco_app_server::AppServer;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::ConnectionKey;
use coco_app_server::DisconnectOutcome;
use coco_app_server::JsonRpcAdapterConnection;
use coco_app_server::JsonRpcAdapterError;
use coco_app_server::JsonRpcDispatchError;
use coco_app_server::JsonRpcRequestContext;
use coco_app_server::JsonRpcRequestFuture;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientRequestContext;
use coco_app_server::LocalClientRequestFuture;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server::SubscribeReplay;
use coco_app_server::SurfaceRole;
use coco_app_server_client::ClientError;
use coco_app_server_client::ServerClient;
use coco_app_server_client::SessionClient;
use coco_app_server_transport::JsonRpcErrorObject as TransportJsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse as TransportJsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcNotification as TransportJsonRpcNotification;
use coco_app_server_transport::JsonRpcRequest as TransportJsonRpcRequest;
use coco_app_server_transport::JsonRpcSuccess;
use coco_hub_connector::HubConnectorSender;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::JSONRPC_VERSION;
use coco_types::JsonRpcError;
use coco_types::JsonRpcErrorObject;
use coco_types::JsonRpcMessage;
use coco_types::JsonRpcNotification;
use coco_types::JsonRpcRequest;
use coco_types::JsonRpcResponse;
use coco_types::RequestId;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::warn;

use crate::sdk_server::dispatcher::SdkHubEgress;
use crate::sdk_server::dispatcher::spawn_sdk_outbound_writer;
use crate::sdk_server::handlers::HandlerContext;
use crate::sdk_server::handlers::HandlerResult;
use crate::sdk_server::handlers::PriorSessionCleanup;
use crate::sdk_server::handlers::ReplacementSessionState;
use crate::sdk_server::handlers::RuntimeReplacementContext;
use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::SessionHandoffState;
use crate::sdk_server::handlers::SessionMetadata;
use crate::sdk_server::handlers::dispatch_client_request;
use crate::sdk_server::handlers::session;
use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::transport::SdkTransport;
use crate::sdk_server::transport::TransportError;

const APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY: usize = 128;
const APP_SERVER_LOCAL_CHANNEL_CAPACITY: usize = 128;
const APP_SERVER_CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct AppServerLocalTurnCompletion {
    pub started: coco_types::TurnStartResult,
    pub ended: coco_types::TurnEndedParams,
}

/// Runtime-backed request handler for AppServer adapters.
#[derive(Clone)]
pub struct AppServerSdkHandler {
    state: Arc<SdkServerState>,
    notif_tx: mpsc::Sender<OutboundMessage>,
    local_app_server: Option<Arc<AppServer<LocalAppSessionHandle>>>,
}

/// Local AppServer registry handle for the current app/cli bridge.
///
/// The registry id is an immutable snapshot that is checked against the
/// optional runtime handle during close cascades. Runtime replacement installs a
/// fresh handle instead of mutating an existing handle in place.
#[derive(Clone)]
pub struct LocalAppSessionHandle {
    session_id: SessionId,
    runtime: Option<crate::session_runtime::SessionHandle>,
}

impl LocalAppSessionHandle {
    fn snapshot(session_id: SessionId) -> Self {
        Self {
            session_id,
            runtime: None,
        }
    }

    pub fn from_runtime(
        session_id: SessionId,
        runtime: crate::session_runtime::SessionHandle,
    ) -> Self {
        Self {
            session_id,
            runtime: Some(runtime),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn runtime(&self) -> Option<&crate::session_runtime::SessionHandle> {
        self.runtime.as_ref()
    }
}

impl AppServerSdkHandler {
    pub fn new(state: Arc<SdkServerState>, notif_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: None,
        }
    }

    pub fn with_local_app_server(
        state: Arc<SdkServerState>,
        notif_tx: mpsc::Sender<OutboundMessage>,
        app_server: Arc<AppServer<LocalAppSessionHandle>>,
    ) -> Self {
        Self {
            state,
            notif_tx,
            local_app_server: Some(app_server),
        }
    }
}

impl JsonRpcRequestHandler for AppServerSdkHandler {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let local_app_server = self.local_app_server.clone();
        if let (Some(app_server), ClientRequest::SessionSubscribe(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection;
            return Box::pin(async move {
                subscribe_local_app_server_session(app_server, connection, params).await
            });
        }
        let lifecycle_request = local_app_server.as_ref().and_then(|app_server| {
            LocalLifecycleRequest::from_client_request(context.connection, &request, app_server)
        });
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
            let params = params.as_ref().clone();
            let connection = context.connection;
            let live_before = app_server
                .list_live_sessions()
                .into_iter()
                .map(|summary| summary.session_id)
                .collect::<Vec<_>>();
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                let replacement = state.runtime_replacement.read().await.clone();
                if let Some(replacement) = replacement {
                    return start_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        live_before,
                        params,
                        replacement,
                    )
                    .await;
                }
                start_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    live_before,
                    params,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionResume(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection;
            let live_before = app_server
                .list_live_sessions()
                .into_iter()
                .map(|summary| summary.session_id)
                .collect::<Vec<_>>();
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                let replacement = state.runtime_replacement.read().await.clone();
                if let Some(replacement) = replacement {
                    return resume_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        live_before,
                        params,
                        replacement,
                    )
                    .await;
                }
                resume_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    live_before,
                    params,
                )
                .await
            });
        }
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
            scoped_session_id: local_app_server.as_ref().and_then(|app_server| {
                app_server.sole_interactive_session_for_connection(context.connection)
            }),
        };
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let dispatch_result = dispatch_sdk_client_request(request, ctx).await;
            let mut result = dispatch_result?;
            if let (Some(app_server), Some(lifecycle_request)) =
                (local_app_server, lifecycle_request)
                && let Some(surface_id) = apply_local_lifecycle_request(
                    app_server,
                    Arc::clone(&state),
                    lifecycle_request,
                    &result,
                )
                .await?
            {
                inject_surface_id(&mut result, surface_id)?;
            }
            Ok(result)
        })
    }
}

impl LocalClientRequestHandler for AppServerSdkHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let local_app_server = self.local_app_server.clone();
        if let (Some(app_server), ClientRequest::SessionSubscribe(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection_key();
            return Box::pin(async move {
                subscribe_local_app_server_session(app_server, connection, params).await
            });
        }
        let lifecycle_request = local_app_server.as_ref().and_then(|app_server| {
            LocalLifecycleRequest::from_client_request(
                context.connection_key(),
                &request,
                app_server,
            )
        });
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
            let params = params.as_ref().clone();
            let connection = context.connection_key();
            let live_before = app_server
                .list_live_sessions()
                .into_iter()
                .map(|summary| summary.session_id)
                .collect::<Vec<_>>();
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                let replacement = state.runtime_replacement.read().await.clone();
                if let Some(replacement) = replacement {
                    return start_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        live_before,
                        params,
                        replacement,
                    )
                    .await;
                }
                start_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    live_before,
                    params,
                )
                .await
            });
        }
        if let (Some(app_server), ClientRequest::SessionResume(params)) =
            (local_app_server.clone(), &request)
        {
            let params = params.clone();
            let connection = context.connection_key();
            let live_before = app_server
                .list_live_sessions()
                .into_iter()
                .map(|summary| summary.session_id)
                .collect::<Vec<_>>();
            let state = Arc::clone(&self.state);
            return Box::pin(async move {
                let replacement = state.runtime_replacement.read().await.clone();
                if let Some(replacement) = replacement {
                    return resume_sdk_session_with_runtime_replacement(
                        app_server,
                        state,
                        connection,
                        live_before,
                        params,
                        replacement,
                    )
                    .await;
                }
                resume_sdk_session_with_scoped_state(
                    app_server,
                    state,
                    connection,
                    live_before,
                    params,
                )
                .await
            });
        }
        let ctx = HandlerContext {
            notif_tx: self.notif_tx.clone(),
            state: Arc::clone(&self.state),
            scoped_session_id: local_app_server.as_ref().and_then(|app_server| {
                app_server.sole_interactive_session_for_connection(context.connection_key())
            }),
        };
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let dispatch_result = dispatch_sdk_client_request(request, ctx).await;
            let mut result = dispatch_result?;
            if let (Some(app_server), Some(lifecycle_request)) =
                (local_app_server, lifecycle_request)
                && let Some(surface_id) = apply_local_lifecycle_request(
                    app_server,
                    Arc::clone(&state),
                    lifecycle_request,
                    &result,
                )
                .await?
            {
                inject_surface_id(&mut result, surface_id)?;
            }
            Ok(result)
        })
    }
}

#[derive(Debug, Clone)]
enum LocalLifecycleRequest {
    Start {
        connection: ConnectionKey,
        live_before: Vec<SessionId>,
    },
    Resume {
        connection: ConnectionKey,
        live_before: Vec<SessionId>,
    },
    Archive(SessionId),
}

impl LocalLifecycleRequest {
    fn from_client_request(
        connection: ConnectionKey,
        request: &ClientRequest,
        app_server: &AppServer<LocalAppSessionHandle>,
    ) -> Option<Self> {
        match request {
            ClientRequest::SessionStart(_) => Some(Self::Start {
                connection,
                live_before: app_server
                    .list_live_sessions()
                    .into_iter()
                    .map(|summary| summary.session_id)
                    .collect(),
            }),
            ClientRequest::SessionResume(_) => Some(Self::Resume {
                connection,
                live_before: app_server
                    .list_live_sessions()
                    .into_iter()
                    .map(|summary| summary.session_id)
                    .collect(),
            }),
            ClientRequest::SessionArchive(params) => Some(Self::Archive(params.session_id.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum LocalSessionDataRequest {
    List,
    Read(coco_types::SessionReadParams),
    TurnsList(coco_types::SessionTurnsListParams),
}

impl LocalSessionDataRequest {
    fn from_client_request(request: &ClientRequest) -> Option<Self> {
        match request {
            ClientRequest::SessionList => Some(Self::List),
            ClientRequest::SessionRead(params) => Some(Self::Read(params.clone())),
            ClientRequest::SessionTurnsList(params) => Some(Self::TurnsList(params.clone())),
            _ => None,
        }
    }
}

struct LocalSessionDataView {
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
}

impl LocalSessionDataView {
    async fn handle(
        &self,
        request: &LocalSessionDataRequest,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        match request {
            LocalSessionDataRequest::List => {
                let listed = self.list_persisted_sessions().await?;
                self.merge_session_list(listed).await
            }
            LocalSessionDataRequest::Read(params) => {
                match self.read_persisted_session(params).await {
                    Ok(result) => encode_local_session_data_result("session/read", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.read_live_session(params).await? {
                            Some(result) => Ok(result),
                            None => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            LocalSessionDataRequest::TurnsList(params) => {
                match self.list_persisted_session_turns(params).await {
                    Ok(result) => encode_local_session_data_result("session/turns/list", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.list_live_session_turns(params).await? {
                            Some(result) => Ok(result),
                            None => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    async fn merge_session_list(
        &self,
        mut listed: coco_types::SessionListResult,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        let mut known: std::collections::HashSet<SessionId> = listed
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        for live in self.app_server.list_live_sessions() {
            if known.contains(&live.session_id) {
                continue;
            }
            if let Some(summary) = self.live_session_summary(&live.session_id).await {
                known.insert(summary.session_id.clone());
                listed.sessions.push(summary);
            }
        }
        encode_local_session_data_result("session/list", listed)
    }

    async fn list_persisted_sessions(
        &self,
    ) -> Result<coco_types::SessionListResult, JsonRpcDispatchError> {
        let manager = {
            let slot = self.state.session_manager.read().await;
            match slot.as_ref() {
                Some(manager) => Arc::clone(manager),
                None => return Ok(coco_types::SessionListResult::default()),
            }
        };
        let list_result = tokio::task::spawn_blocking(move || {
            let sessions = manager
                .list()
                .map_err(|error| format!("session/list failed: {error}"))?;
            sessions
                .iter()
                .map(session_record_to_summary)
                .collect::<Result<Vec<_>, _>>()
                .map(|sessions| coco_types::SessionListResult { sessions })
                .map_err(|error| format!("session/list returned invalid session id: {error}"))
        })
        .await;
        match list_result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(internal_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/list task panicked: {join_err}"
            ))),
        }
    }

    async fn read_persisted_session(
        &self,
        params: &coco_types::SessionReadParams,
    ) -> Result<coco_types::SessionReadResult, JsonRpcDispatchError> {
        let cursor = parse_local_session_data_cursor("session/read", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/read", params.limit)?;
        let manager = self.session_manager_or_invalid().await?;
        let session_id = params.session_id.as_str().to_string();
        let read_result = tokio::task::spawn_blocking(move || {
            let session = manager
                .load(&session_id)
                .map_err(|error| format!("session/read: {error}"))?;
            let store = manager.store_for(&session.working_dir);
            let transcript_messages = store
                .load_transcript_messages(&session_id)
                .map_err(|error| format!("session/read: {error}"))?;
            let page = coco_app_server::session_data_page(transcript_messages.len(), cursor, limit);
            let messages = transcript_messages[page.start..page.end]
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("session/read: {error}"))?;
            Ok::<_, String>(coco_types::SessionReadResult {
                session: session_record_to_summary(&session).map_err(|error| {
                    format!("session/read returned invalid session id: {error}")
                })?,
                messages,
                next_cursor: page.next_cursor(),
                has_more: page.has_more,
            })
        })
        .await;
        match read_result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(invalid_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/read task panicked: {join_err}"
            ))),
        }
    }

    async fn list_persisted_session_turns(
        &self,
        params: &coco_types::SessionTurnsListParams,
    ) -> Result<coco_types::SessionTurnsListResult, JsonRpcDispatchError> {
        let cursor =
            parse_local_session_data_cursor("session/turns/list", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/turns/list", params.limit)?;
        let manager = self.session_manager_or_invalid().await?;
        let session_id = params.session_id.as_str().to_string();
        let list_result = tokio::task::spawn_blocking(move || {
            let session = manager
                .load(&session_id)
                .map_err(|error| format!("session/turns/list: {error}"))?;
            let store = manager.store_for(&session.working_dir);
            let transcript_messages = store
                .load_transcript_messages(&session_id)
                .map_err(|error| format!("session/turns/list: {error}"))?;
            let turns =
                coco_app_server::derive_session_turn_summaries(transcript_messages.iter().map(
                    |entry| coco_app_server::TranscriptTurnEntry {
                        is_user: entry.entry_type == "user",
                        timestamp: Some(entry.timestamp.as_str()),
                    },
                ));
            let (turns, next_cursor, has_more) =
                coco_app_server::page_session_items(&turns, cursor, limit);
            Ok::<_, String>(coco_types::SessionTurnsListResult {
                session: session_record_to_summary(&session).map_err(|error| {
                    format!("session/turns/list returned invalid session id: {error}")
                })?,
                turns,
                next_cursor,
                has_more,
            })
        })
        .await;
        match list_result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(invalid_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/turns/list task panicked: {join_err}"
            ))),
        }
    }

    async fn session_manager_or_invalid(
        &self,
    ) -> Result<Arc<coco_session::SessionManager>, JsonRpcDispatchError> {
        let slot = self.state.session_manager.read().await;
        slot.as_ref().map(Arc::clone).ok_or_else(|| {
            invalid_local_session_data_error("session persistence is not enabled on this server")
        })
    }

    async fn read_live_session(
        &self,
        params: &coco_types::SessionReadParams,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError> {
        if self.app_server.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some((summary, history)) = self
            .live_session_summary_and_history(&params.session_id)
            .await
        else {
            return Ok(None);
        };
        let cursor = parse_local_session_data_cursor("session/read", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/read", params.limit)?;
        let page = coco_app_server::session_data_page(history.len(), cursor, limit);
        let messages = history[page.start..page.end]
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/read encode failed: {error}"),
                data: None,
            })?;
        let result = coco_types::SessionReadResult {
            session: summary,
            messages,
            next_cursor: page.next_cursor(),
            has_more: page.has_more,
        };
        serde_json::to_value(result)
            .map(Some)
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/read encode failed: {error}"),
                data: None,
            })
    }

    async fn list_live_session_turns(
        &self,
        params: &coco_types::SessionTurnsListParams,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError> {
        if self.app_server.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some((summary, history)) = self
            .live_session_summary_and_history(&params.session_id)
            .await
        else {
            return Ok(None);
        };
        let cursor =
            parse_local_session_data_cursor("session/turns/list", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/turns/list", params.limit)?;
        let turns = coco_app_server::derive_session_turn_summaries(history.iter().map(|message| {
            coco_app_server::TranscriptTurnEntry {
                is_user: matches!(message.as_ref(), coco_messages::Message::User(_)),
                timestamp: None,
            }
        }));
        let (turns, next_cursor, has_more) =
            coco_app_server::page_session_items(&turns, cursor, limit);
        let result = coco_types::SessionTurnsListResult {
            session: summary,
            turns,
            next_cursor,
            has_more,
        };
        serde_json::to_value(result)
            .map(Some)
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/turns/list encode failed: {error}"),
                data: None,
            })
    }

    async fn live_session_summary(
        &self,
        session_id: &SessionId,
    ) -> Option<coco_types::SdkSessionSummary> {
        self.live_session_summary_and_history(session_id)
            .await
            .map(|(summary, _)| summary)
    }

    async fn live_session_summary_and_history(
        &self,
        session_id: &SessionId,
    ) -> Option<(
        coco_types::SdkSessionSummary,
        Vec<std::sync::Arc<coco_messages::Message>>,
    )> {
        if let Some(handle) = self.app_server.registry().get(session_id)
            && let Some(result) = live_runtime_session_summary_and_history(&handle).await
        {
            return Some(result);
        }

        live_sdk_session_summary_and_history(&self.state, session_id).await
    }
}

fn encode_local_session_data_result(
    operation: &str,
    result: impl serde::Serialize,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    serde_json::to_value(result).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("local AppServer {operation} encode failed: {error}"),
        data: None,
    })
}

fn invalid_local_session_data_error(message: impl Into<String>) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: message.into(),
        data: None,
    }
}

fn internal_local_session_data_error(message: impl Into<String>) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: message.into(),
        data: None,
    }
}

fn session_record_to_summary(
    session: &coco_session::Session,
) -> Result<coco_types::SdkSessionSummary, String> {
    Ok(coco_types::SdkSessionSummary {
        session_id: SessionId::try_new(session.id.clone()).map_err(|error| error.to_string())?,
        model: session.model.clone(),
        cwd: session.working_dir.to_string_lossy().into_owned(),
        created_at: session.created_at.clone(),
        updated_at: session.updated_at.clone(),
        title: session.title.clone(),
        message_count: session.message_count,
        total_tokens: session.total_tokens,
    })
}

async fn live_runtime_session_summary_and_history(
    handle: &LocalAppSessionHandle,
) -> Option<(
    coco_types::SdkSessionSummary,
    Vec<std::sync::Arc<coco_messages::Message>>,
)> {
    let runtime = handle.runtime()?;
    let current_session_id = runtime.current_typed_session_id().await;
    if current_session_id != *handle.session_id() {
        return None;
    }

    let config = runtime.current_engine_config().await;
    let history = runtime.history.lock().await.to_vec();
    let usage = runtime.session_usage_snapshot().await;
    let timestamp = chrono::Utc::now().to_rfc3339();
    Some((
        coco_types::SdkSessionSummary {
            session_id: handle.session_id().clone(),
            model: config.model_id,
            cwd: runtime.original_cwd().to_string_lossy().into_owned(),
            created_at: timestamp.clone(),
            updated_at: Some(timestamp),
            title: None,
            message_count: history.len() as i32,
            total_tokens: usage
                .totals
                .input_tokens
                .saturating_add(usage.totals.output_tokens),
        },
        history,
    ))
}

async fn live_sdk_session_summary_and_history(
    state: &Arc<SdkServerState>,
    session_id: &SessionId,
) -> Option<(
    coco_types::SdkSessionSummary,
    Vec<std::sync::Arc<coco_messages::Message>>,
)> {
    let metadata = state.session_metadata_snapshot(session_id)?;
    let handoff = state.session_handoff_snapshot(session_id)?;
    let history = handoff.history.lock().await.clone();
    let accounting = state.session_accounting_snapshot(session_id);
    let timestamp = chrono::Utc::now().to_rfc3339();
    Some((
        coco_types::SdkSessionSummary {
            session_id: session_id.clone(),
            model: metadata.model,
            cwd: metadata.cwd,
            created_at: timestamp.clone(),
            updated_at: Some(timestamp),
            title: None,
            message_count: history.len() as i32,
            total_tokens: accounting.stats.usage.input_tokens.total
                + accounting.stats.usage.output_tokens.total,
        },
        history,
    ))
}

fn parse_local_session_data_cursor(
    operation: &str,
    raw: Option<&str>,
) -> Result<usize, JsonRpcDispatchError> {
    coco_app_server::parse_session_data_cursor(operation, raw)
        .map_err(local_session_data_projection_error)
}

fn parse_local_session_data_limit(
    operation: &str,
    limit: Option<i32>,
) -> Result<Option<usize>, JsonRpcDispatchError> {
    coco_app_server::parse_session_data_limit(operation, limit)
        .map_err(local_session_data_projection_error)
}

fn local_session_data_projection_error(
    error: coco_app_server::SessionDataProjectionError,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.message(),
        data: None,
    }
}

async fn start_sdk_session_with_scoped_state(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    live_before: Vec<SessionId>,
    params: coco_types::SessionStartParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    if state.session_runtime.read().await.is_some() {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message:
                "session/start requires AppServer runtime replacement when a runtime is already installed"
                    .to_string(),
            data: None,
        });
    }
    let prepared = session::prepare_session_start(params, &state, false)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    session::install_scoped_started_session_state(&state, &prepared, None).await;

    let mut result = serde_json::to_value(coco_types::SessionStartResult {
        session_id: prepared.session_id,
        surface_id: None,
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/start result encode failed: {error}"),
        data: None,
    })?;
    if let Some(surface_id) = apply_local_lifecycle_request(
        app_server,
        state,
        LocalLifecycleRequest::Start {
            connection,
            live_before,
        },
        &result,
    )
    .await?
    {
        inject_surface_id(&mut result, surface_id)?;
    }
    Ok(result)
}

async fn resume_sdk_session_with_scoped_state(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    live_before: Vec<SessionId>,
    params: coco_types::SessionResumeParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let matching_runtime = if let Some(runtime) = state.session_runtime.read().await.clone() {
        let runtime_session_id = runtime.current_typed_session_id().await;
        if runtime_session_id != loaded.session_id {
            return Err(JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "session/resume requires AppServer runtime replacement when the installed runtime belongs to a different session".to_string(),
                data: None,
            });
        }
        Some(runtime)
    } else {
        None
    };

    if let Some(runtime) = matching_runtime {
        session::hydrate_runtime_for_resume_messages(
            &runtime,
            &loaded.session_id,
            &loaded.conversation.messages,
        )
        .await;
        runtime.fire_session_start_hooks("resume").await;
        install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
        install_runtime_backed_resumed_sdk_session_state(
            &state,
            &loaded.session,
            loaded.session_id.clone(),
            &runtime,
            &loaded.conversation.messages,
        )
        .await;
    } else {
        session::install_scoped_resumed_session_state(
            &state,
            &loaded.session,
            loaded.session_id.clone(),
            &loaded.conversation.messages,
        )
        .await;
    }

    let mut result = encode_session_resume_result(&loaded.session, None)?;
    if let Some(surface_id) = apply_local_lifecycle_request(
        app_server,
        state,
        LocalLifecycleRequest::Resume {
            connection,
            live_before,
        },
        &result,
    )
    .await?
    {
        inject_surface_id(&mut result, surface_id)?;
    }
    Ok(result)
}

async fn start_sdk_session_with_runtime_replacement(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    live_before: Vec<SessionId>,
    params: coco_types::SessionStartParams,
    replacement: RuntimeReplacementContext,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let prepared = session::prepare_session_start(params, &state, false)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let started_session_id = prepared.session_id.clone();

    let make_factory = || {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let prepared = prepared.clone();
        async move {
            let session_id = prepared.session_id.clone();
            let runtime = build_sdk_runtime_for_start(replacement, state, prepared)
                .await
                .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(session_id, runtime),
            )
        }
    };

    let mut replacement_surface_id = None;
    let mut replacement_handle = None;
    let mut replaced_existing = false;
    for previous_session_id in live_before {
        if previous_session_id == started_session_id {
            continue;
        }
        if !replaced_existing {
            if let Some((handle, surface_id)) = replace_local_app_server_session_with_factory(
                Arc::clone(&app_server),
                Arc::clone(&state),
                previous_session_id.clone(),
                started_session_id.clone(),
                make_factory(),
            )
            .await?
            {
                replacement_handle = Some(handle);
                replacement_surface_id = Some(surface_id);
            } else {
                replacement_handle = Some(
                    replace_detached_local_app_server_session_with_factory(
                        Arc::clone(&app_server),
                        Arc::clone(&state),
                        previous_session_id,
                        started_session_id.clone(),
                        make_factory(),
                    )
                    .await?,
                );
            }
            replaced_existing = true;
        } else {
            close_local_app_server_session(
                Arc::clone(&app_server),
                Arc::clone(&state),
                previous_session_id,
            )
            .await?;
        }
    }

    let handle = match replacement_handle {
        Some(handle) => handle,
        None => {
            load_local_app_server_session_with_factory(
                &app_server,
                started_session_id.clone(),
                make_factory(),
            )
            .await?
        }
    };
    let Some(runtime) = handle.runtime().cloned() else {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!(
                "local AppServer session {} started without a runtime handle",
                handle.session_id()
            ),
            data: None,
        });
    };

    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
    session::install_scoped_started_session_state(
        &state,
        &prepared,
        Some(Arc::clone(runtime.runtime().app_state())),
    )
    .await;

    let surface_id = match replacement_surface_id {
        Some(surface_id) => surface_id,
        None => {
            attach_local_app_server_surface(&app_server, connection, started_session_id.clone())?
        }
    };
    serde_json::to_value(coco_types::SessionStartResult {
        session_id: started_session_id,
        surface_id: Some(surface_id),
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/start result encode failed: {error}"),
        data: None,
    })
}

async fn resume_sdk_session_with_runtime_replacement(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    connection: ConnectionKey,
    live_before: Vec<SessionId>,
    params: coco_types::SessionResumeParams,
    replacement: RuntimeReplacementContext,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let loaded = session::load_resume_session(params, &state)
        .await
        .map_err(handler_result_to_dispatch_error)?;
    let resumed_session_id = loaded.session_id.clone();
    let resumed_cwd = loaded.session.working_dir.clone();
    let prior_messages = loaded.conversation.messages.clone();

    if let Some(current_runtime) = state.session_runtime.read().await.clone() {
        let current_session_id = current_runtime.current_typed_session_id().await;
        if current_session_id == resumed_session_id {
            session::hydrate_runtime_for_resume_messages(
                &current_runtime,
                &resumed_session_id,
                &prior_messages,
            )
            .await;
            current_runtime.fire_session_start_hooks("resume").await;
            install_sdk_session_runtime_state(Arc::clone(&state), current_runtime.clone()).await;
            install_runtime_backed_resumed_sdk_session_state(
                &state,
                &loaded.session,
                resumed_session_id.clone(),
                &current_runtime,
                &prior_messages,
            )
            .await;
            let surface_id =
                attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
            return encode_session_resume_result(&loaded.session, Some(surface_id));
        }
    }

    let make_factory = || {
        let state = Arc::clone(&state);
        let replacement = replacement.clone();
        let session_id = resumed_session_id.clone();
        let cwd = resumed_cwd.clone();
        let prior_messages = prior_messages.clone();
        async move {
            let runtime = build_sdk_runtime_for_resume(
                replacement,
                state,
                session_id.clone(),
                cwd,
                prior_messages,
            )
            .await
            .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(session_id, runtime),
            )
        }
    };

    let mut replacement_surface_id = None;
    let mut replacement_handle = None;
    let mut replaced_existing = false;
    for previous_session_id in live_before {
        if previous_session_id == resumed_session_id {
            continue;
        }
        if !replaced_existing {
            if let Some((handle, surface_id)) = replace_local_app_server_session_with_factory(
                Arc::clone(&app_server),
                Arc::clone(&state),
                previous_session_id.clone(),
                resumed_session_id.clone(),
                make_factory(),
            )
            .await?
            {
                replacement_handle = Some(handle);
                replacement_surface_id = Some(surface_id);
            } else {
                replacement_handle = Some(
                    replace_detached_local_app_server_session_with_factory(
                        Arc::clone(&app_server),
                        Arc::clone(&state),
                        previous_session_id,
                        resumed_session_id.clone(),
                        make_factory(),
                    )
                    .await?,
                );
            }
            replaced_existing = true;
        } else {
            close_local_app_server_session(
                Arc::clone(&app_server),
                Arc::clone(&state),
                previous_session_id,
            )
            .await?;
        }
    }

    let handle = match replacement_handle {
        Some(handle) => handle,
        None => {
            load_local_app_server_session_with_factory(
                &app_server,
                resumed_session_id.clone(),
                make_factory(),
            )
            .await?
        }
    };
    let Some(runtime) = handle.runtime().cloned() else {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!(
                "local AppServer session {} resumed without a runtime handle",
                handle.session_id()
            ),
            data: None,
        });
    };
    install_sdk_session_runtime_state(Arc::clone(&state), runtime.clone()).await;
    install_runtime_backed_resumed_sdk_session_state(
        &state,
        &loaded.session,
        resumed_session_id.clone(),
        &runtime,
        &prior_messages,
    )
    .await;

    let surface_id = match replacement_surface_id {
        Some(surface_id) => surface_id,
        None => attach_local_app_server_surface(&app_server, connection, resumed_session_id)?,
    };
    encode_session_resume_result(&loaded.session, Some(surface_id))
}

async fn build_sdk_runtime_for_start(
    replacement: RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    prepared: session::PreparedStartSession,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            prepared.session_id.clone(),
            session_build_cwd_from_str(&replacement.cwd, &prepared.cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(&replacement, state, &session).await?;
    session.runtime().fire_session_start_hooks("startup").await;
    Ok(session)
}

fn session_build_cwd_from_str(base: &std::path::Path, cwd: &str) -> std::path::PathBuf {
    session_build_cwd(base, std::path::Path::new(cwd))
}

fn session_build_cwd(base: &std::path::Path, cwd: &std::path::Path) -> std::path::PathBuf {
    if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        base.join(cwd)
    }
}

async fn build_sdk_runtime_for_resume(
    replacement: RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    session_id: SessionId,
    cwd: std::path::PathBuf,
    prior_messages: Vec<coco_messages::Message>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = replacement
        .runtime_factory
        .build_with_session_id_and_cwd(
            session_id.clone(),
            session_build_cwd(&replacement.cwd, &cwd),
        )
        .await?;
    setup_sdk_replacement_runtime(&replacement, state, &session).await?;
    session::hydrate_runtime_for_resume_messages(&session, &session_id, &prior_messages).await;
    session.runtime().fire_session_start_hooks("resume").await;
    Ok(session)
}

async fn setup_sdk_replacement_runtime(
    replacement: &RuntimeReplacementContext,
    state: Arc<SdkServerState>,
    session: &crate::session_runtime::SessionHandle,
) -> anyhow::Result<()> {
    let runtime = session.runtime().clone();
    let session_cwd = runtime.original_cwd().clone();
    if replacement.requires_structured_output {
        runtime
            .update_engine_config(|cfg| cfg.requires_structured_output = true)
            .await;
    }
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&state), session);
    if let Some(hooks) = state.sdk_initialize_hooks.read().await.clone() {
        crate::sdk_server::sdk_hooks::register_initialize_hooks(session, &hooks);
    }
    let sdk_agents = state.pending_sdk_agents.read().await.clone();
    if !sdk_agents.is_empty() {
        session.set_sdk_supplied_agents(sdk_agents).await;
    }

    let lsp_handle = crate::session_bootstrap::build_lsp_handle_if_enabled(
        Arc::clone(&replacement.process_runtime),
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    crate::session_bootstrap::install_session_late_binds(
        session.clone(),
        &session_cwd,
        None,
        lsp_handle,
        None,
    )
    .await?;
    let mcp_manager = state.mcp_manager.read().await.clone();
    crate::session_bootstrap::bootstrap_session_mcp(
        session,
        &session_cwd,
        mcp_manager,
        /*await_connect*/ false,
    )
    .await;
    crate::leader_inbox_poller::install_leader(session.clone(), None).await;
    Ok(())
}

pub async fn install_sdk_session_runtime_state(
    state: Arc<SdkServerState>,
    session: crate::session_runtime::SessionHandle,
) {
    crate::sdk_server::sdk_hooks::install_runtime_callback(Arc::clone(&state), &session);
    let runtime = session.runtime();
    let session_manager = Arc::clone(runtime.session_manager());
    let file_history = runtime.file_history().cloned();
    let file_history_config_home = runtime
        .file_history()
        .map(|_| runtime.config_home().clone());
    install_sdk_runtime_reload_subscription(Arc::clone(&state), &session).await;
    {
        let mut slot = state.session_runtime.write().await;
        *slot = Some(session);
    }
    {
        let mut slot = state.session_manager.write().await;
        *slot = Some(session_manager);
    }
    {
        let mut slot = state.file_history.write().await;
        *slot = file_history;
    }
    {
        let mut slot = state.file_history_config_home.write().await;
        *slot = file_history_config_home;
    }
}

async fn install_sdk_runtime_reload_subscription(
    state: Arc<SdkServerState>,
    session: &crate::session_runtime::SessionHandle,
) {
    let runtime = session.runtime();
    let mut slot = state.sdk_runtime_reload_subscription.lock().await;
    if let Some(handle) = slot.take() {
        handle.abort();
    }

    let Some(sandbox_state) = runtime.sandbox_state() else {
        return;
    };
    sandbox_state.set_approval_bridge(Arc::new(crate::sdk_server::SdkSandboxApprovalBridge::new(
        Arc::clone(&state),
    )));

    let Some(publisher) = runtime.runtime_publisher() else {
        return;
    };
    *slot = Some(crate::sandbox_reload::spawn_sandbox_reload(
        sandbox_state,
        &publisher,
        runtime.original_cwd().clone(),
    ));
}

async fn install_runtime_backed_resumed_sdk_session_state(
    state: &SdkServerState,
    session: &coco_session::Session,
    session_id: SessionId,
    runtime_handle: &crate::session_runtime::SessionHandle,
    prior_messages: &[coco_messages::Message],
) {
    let plan_mode_instructions = state.pending_plan_mode_instructions.read().await.clone();
    let runtime = runtime_handle.runtime();
    let history = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();

    state.install_scoped_replacement_session_state(ReplacementSessionState {
        session_id: session_id.clone(),
        metadata: SessionMetadata {
            cwd: session.working_dir.to_string_lossy().into_owned(),
            model: session.model.clone(),
        },
        handoff: SessionHandoffState {
            history: Arc::new(tokio::sync::Mutex::new(history)),
            app_state: Arc::clone(runtime.app_state()),
        },
        plan_mode_instructions,
        prior_cleanup: PriorSessionCleanup::ActiveTurnAndHandoff,
        reset_accounting: false,
        cancel_reason: "runtime-backed session/resume",
    });
}

async fn load_local_app_server_session_with_factory<F>(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    session_id: SessionId,
    factory: F,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let mut completion = match app_server
        .spawn_load(session_id.clone(), factory)
        .map_err(|error| local_lifecycle_error("load session", error))?
    {
        AppLoadStart::Started { completion } | AppLoadStart::Loading(completion) => completion,
        AppLoadStart::Live(handle) => return Ok(handle),
        AppLoadStart::Closing(_) => {
            return Err(JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session {session_id} is closing"),
                data: None,
            });
        }
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("load session", error))
}

fn encode_session_resume_result(
    session: &coco_session::Session,
    surface_id: Option<SurfaceId>,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let summary = session::session_to_summary(session).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/resume returned invalid session id: {error}"),
        data: None,
    })?;
    serde_json::to_value(coco_types::SessionResumeResult {
        session: summary,
        surface_id,
    })
    .map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("session/resume result encode failed: {error}"),
        data: None,
    })
}

fn handler_result_to_dispatch_error(error: HandlerResult) -> JsonRpcDispatchError {
    match error {
        HandlerResult::Err {
            code,
            message,
            data,
        } => JsonRpcDispatchError {
            code,
            message,
            data,
        },
        HandlerResult::NotImplemented(method) => JsonRpcDispatchError::method_not_found(method),
        HandlerResult::Ok(_) => JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "session/resume loader returned an unexpected success result".to_string(),
            data: None,
        },
    }
}

async fn apply_local_lifecycle_request(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    request: LocalLifecycleRequest,
    result: &serde_json::Value,
) -> Result<Option<SurfaceId>, JsonRpcDispatchError> {
    match request {
        LocalLifecycleRequest::Start {
            connection,
            live_before,
        } => {
            let started: coco_types::SessionStartResult = serde_json::from_value(result.clone())
                .map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/start decode failed: {error}"),
                    data: None,
                })?;
            let session_id = started.session_id;
            for previous_session_id in live_before {
                if previous_session_id != session_id {
                    close_local_app_server_session(
                        Arc::clone(&app_server),
                        Arc::clone(&state),
                        previous_session_id,
                    )
                    .await?;
                }
            }
            register_local_app_server_session(
                &app_server,
                LocalAppSessionHandle::snapshot(session_id.clone()),
            )
            .await?;
            let surface_id = attach_local_app_server_surface(&app_server, connection, session_id)?;
            Ok(Some(surface_id))
        }
        LocalLifecycleRequest::Resume {
            connection,
            live_before,
        } => {
            let resumed: coco_types::SessionResumeResult =
                serde_json::from_value(result.clone()).map_err(|error| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("local AppServer session/resume decode failed: {error}"),
                    data: None,
                })?;
            let resumed_session_id = resumed.session.session_id;
            let mut replaced_existing = false;
            let mut replaced_surface_id = None;
            for previous_session_id in live_before {
                if previous_session_id != resumed_session_id {
                    if !replaced_existing {
                        let replacement = replace_local_app_server_session(
                            Arc::clone(&app_server),
                            Arc::clone(&state),
                            previous_session_id.clone(),
                            LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                        )
                        .await?;
                        if let Some(surface_id) = replacement {
                            replaced_existing = true;
                            replaced_surface_id = Some(surface_id);
                        } else {
                            replace_detached_local_app_server_session(
                                Arc::clone(&app_server),
                                Arc::clone(&state),
                                previous_session_id,
                                LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                            )
                            .await?;
                            replaced_existing = true;
                        }
                    } else {
                        close_local_app_server_session(
                            Arc::clone(&app_server),
                            Arc::clone(&state),
                            previous_session_id,
                        )
                        .await?;
                    }
                }
            }
            if !replaced_existing {
                register_local_app_server_session(
                    &app_server,
                    LocalAppSessionHandle::snapshot(resumed_session_id.clone()),
                )
                .await?;
                let surface_id =
                    attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
                Ok(Some(surface_id))
            } else if let Some(surface_id) = replaced_surface_id {
                Ok(Some(surface_id))
            } else {
                let surface_id =
                    attach_local_app_server_surface(&app_server, connection, resumed_session_id)?;
                Ok(Some(surface_id))
            }
        }
        LocalLifecycleRequest::Archive(session_id) => {
            close_local_app_server_session(app_server, state, session_id).await?;
            Ok(None)
        }
    }
}

fn inject_surface_id(
    result: &mut serde_json::Value,
    surface_id: SurfaceId,
) -> Result<(), JsonRpcDispatchError> {
    let Some(object) = result.as_object_mut() else {
        return Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "local AppServer lifecycle result was not an object".to_string(),
            data: None,
        });
    };
    object.insert(
        "surface_id".to_string(),
        serde_json::to_value(surface_id).map_err(|error| JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("local AppServer surface id encode failed: {error}"),
            data: None,
        })?,
    );
    Ok(())
}

fn attach_local_app_server_surface(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    connection: ConnectionKey,
    session_id: SessionId,
) -> Result<SurfaceId, JsonRpcDispatchError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..Default::default()
    };
    app_server
        .attach_surface_with_options(connection, surface_id.clone(), session_id, options)
        .map_err(|error| local_lifecycle_error("attach session surface", error))?;
    Ok(surface_id)
}

async fn subscribe_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    connection: ConnectionKey,
    params: coco_types::SessionSubscribeParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Passive,
        ..Default::default()
    };
    match app_server
        .subscribe_surface_with_options(
            connection,
            surface_id.clone(),
            params.session_id.clone(),
            params.after_seq,
            options,
        )
        .map_err(|error| local_lifecycle_error("subscribe session", error))?
    {
        SubscribeReplay::Replayed(replayed) => {
            let replayed = replayed
                .into_iter()
                .map(encode_session_subscribe_envelope)
                .collect();
            serde_json::to_value(coco_types::SessionSubscribeResult {
                session_id: params.session_id,
                surface_id,
                replayed,
            })
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/subscribe encode failed: {error}"),
                data: None,
            })
        }
        SubscribeReplay::SnapshotRequired => Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/subscribe requires a fresh snapshot before passive attach"
                .to_string(),
            data: Some(serde_json::json!({ "kind": "snapshot_required" })),
        }),
    }
}

fn encode_session_subscribe_envelope(
    envelope: SessionEnvelope,
) -> coco_types::SessionSubscribeEnvelope {
    let event = match envelope.event {
        CoreEvent::Protocol(notification) => serde_json::json!({
            "layer": "protocol",
            "payload": notification,
        }),
        CoreEvent::Stream(event) => serde_json::json!({
            "layer": "stream",
            "payload": event,
        }),
        CoreEvent::Tui(event) => serde_json::json!({
            "layer": "tui",
            "payload": event,
        }),
    };
    coco_types::SessionSubscribeEnvelope {
        session_id: envelope.session_id,
        agent_id: envelope.agent_id.map(coco_types::AgentId::into_inner),
        turn_id: envelope.turn_id,
        session_seq: envelope.session_seq,
        event,
    }
}

async fn replace_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_handle: LocalAppSessionHandle,
) -> Result<Option<SurfaceId>, JsonRpcDispatchError> {
    let new_session_id = new_handle.session_id.clone();
    replace_local_app_server_session_with_factory(
        app_server,
        state,
        old_session_id,
        new_session_id,
        async { Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(new_handle) },
    )
    .await
    .map(|replacement| replacement.map(|(_, surface)| surface))
}

async fn replace_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
) -> Result<Option<(LocalAppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    replace_local_app_server_session_with_factory_and_close_reason(
        app_server,
        state,
        old_session_id,
        new_session_id,
        factory,
        coco_hooks::orchestration::ExitReason::Other,
    )
    .await
}

async fn replace_local_app_server_session_with_factory_and_close_reason<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
    close_reason: coco_hooks::orchestration::ExitReason,
) -> Result<Option<(LocalAppSessionHandle, SurfaceId)>, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let Some(calling_surface) = local_replace_calling_surface(&app_server, &old_session_id) else {
        return Ok(None);
    };
    let calling_surface_id = calling_surface.clone();
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace(
            old_session_id,
            new_session_id,
            calling_surface,
            factory,
            move |handle| async move {
                close_sdk_session_state_for_app_server(&close_state, handle.session_id()).await;
                close_local_session_handle_with_reason(handle, close_reason).await;
            },
        )
        .map_err(|error| local_lifecycle_error("replace session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map(|handle| Some((handle, calling_surface_id)))
        .map_err(|error| local_lifecycle_error("replace session", error))
}

async fn replace_detached_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_handle: LocalAppSessionHandle,
) -> Result<(), JsonRpcDispatchError> {
    let new_session_id = new_handle.session_id.clone();
    replace_detached_local_app_server_session_with_factory(
        app_server,
        state,
        old_session_id,
        new_session_id,
        async { Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(new_handle) },
    )
    .await
    .map(|_| ())
}

async fn replace_detached_local_app_server_session_with_factory<F>(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    old_session_id: SessionId,
    new_session_id: SessionId,
    factory: F,
) -> Result<LocalAppSessionHandle, JsonRpcDispatchError>
where
    F: Future<Output = Result<LocalAppSessionHandle, coco_app_server::RegistryError>>
        + Send
        + 'static,
{
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_replace_detached(
            old_session_id,
            new_session_id,
            factory,
            move |handle| async move {
                close_sdk_session_state_for_app_server(&close_state, handle.session_id()).await;
                close_local_session_handle(handle).await;
            },
        )
        .map_err(|error| local_lifecycle_error("replace detached session", error))?
    {
        coco_app_server::AppReplaceStart::Started { completion } => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("replace detached session", error))
}

fn local_replace_calling_surface(
    app_server: &AppServer<LocalAppSessionHandle>,
    session_id: &SessionId,
) -> Option<SurfaceId> {
    let routing = app_server
        .routing()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    routing.interactive_owner(session_id).cloned()
}

async fn register_local_app_server_session(
    app_server: &Arc<AppServer<LocalAppSessionHandle>>,
    handle: LocalAppSessionHandle,
) -> Result<(), JsonRpcDispatchError> {
    let session_id = handle.session_id.clone();
    let handle_for_load = handle.clone();
    match app_server
        .spawn_load(session_id, async {
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(handle_for_load)
        })
        .map_err(|error| local_lifecycle_error("register session", error))?
    {
        AppLoadStart::Started { mut completion } | AppLoadStart::Loading(mut completion) => {
            completion
                .wait()
                .await
                .map(|_| ())
                .map_err(|error| local_lifecycle_error("register session", error))
        }
        AppLoadStart::Live(_) => {
            if handle.runtime().is_some() {
                let refresh_session_id = handle.session_id.clone();
                app_server
                    .registry()
                    .replace_live_handle(&refresh_session_id, handle)
                    .map_err(|error| local_lifecycle_error("refresh live session", error))?;
            }
            Ok(())
        }
        AppLoadStart::Closing(_) => Ok(()),
    }
}

async fn close_local_app_server_session(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    session_id: SessionId,
) -> Result<(), JsonRpcDispatchError> {
    if !app_server
        .list_live_sessions()
        .iter()
        .any(|summary| summary.session_id == session_id)
    {
        return Ok(());
    }
    let close_state = Arc::clone(&state);
    let mut completion = match app_server
        .spawn_close(session_id, move |handle| async move {
            close_sdk_session_state_for_app_server(&close_state, handle.session_id()).await;
            close_local_session_handle(handle).await;
        })
        .map_err(|error| local_lifecycle_error("archive session", error))?
    {
        AppCloseStart::Started { completion }
        | AppCloseStart::Loading(completion)
        | AppCloseStart::Closing(completion) => completion,
    };
    completion
        .wait()
        .await
        .map_err(|error| local_lifecycle_error("archive session", error))
}

async fn close_local_session_handle(handle: LocalAppSessionHandle) {
    close_local_session_handle_with_reason(handle, coco_hooks::orchestration::ExitReason::Other)
        .await;
}

async fn close_local_session_handle_with_reason(
    handle: LocalAppSessionHandle,
    reason: coco_hooks::orchestration::ExitReason,
) {
    let has_runtime = handle.runtime.is_some();
    if let Some(runtime) = handle.runtime() {
        let current = runtime.current_typed_session_id().await;
        if current != *handle.session_id() {
            debug!(
                target: "coco::app_server_local",
                registry_session_id = %handle.session_id(),
                current_session_id = %current,
                "skipping local AppServer close cascade for stale registry snapshot"
            );
            return;
        }
        runtime.runtime().fire_session_end_hooks(reason).await;
        runtime.runtime().shutdown_signal().cancel();
    }
    debug!(
        target: "coco::app_server_local",
        session_id = %handle.session_id(),
        has_runtime,
        "local AppServer close cascade completed fused runtime boundary"
    );
}

async fn close_sdk_session_state_for_app_server(state: &SdkServerState, session_id: &SessionId) {
    let active_turn = state.clear_scoped_session_state(session_id).await;

    if let Some(active_turn) = &active_turn {
        active_turn.cancel_token.cancel();
    }

    if let Some(active_turn) = active_turn {
        match tokio::time::timeout(APP_SERVER_CLOSE_DRAIN_TIMEOUT, active_turn.turn_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "local AppServer close: turn task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                timeout_secs = APP_SERVER_CLOSE_DRAIN_TIMEOUT.as_secs(),
                "local AppServer close: turn task did not drain before timeout"
            ),
        }
        match tokio::time::timeout(APP_SERVER_CLOSE_DRAIN_TIMEOUT, active_turn.forwarder_task).await
        {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => warn!(
                session_id = %session_id,
                error = %join_err,
                "local AppServer close: forwarder task join failed"
            ),
            Err(_) => warn!(
                session_id = %session_id,
                timeout_secs = APP_SERVER_CLOSE_DRAIN_TIMEOUT.as_secs(),
                "local AppServer close: forwarder task did not drain before timeout"
            ),
        }
    }
}

fn local_lifecycle_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("local AppServer {operation} failed: {error}"),
        data: None,
    }
}

pub struct AppServerLocalBridge {
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    client: ServerClient<LocalAppSessionHandle>,
    handler: AppServerSdkHandler,
    outbound_forwarder: JoinHandle<()>,
    hub_connector: Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
    event_pump: Option<JoinHandle<()>>,
    event_pump_session_id: Option<SessionId>,
    interactive_surface: Option<SessionClient>,
    channel_capacity: usize,
}

impl AppServerLocalBridge {
    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self::with_channel_capacity(state, APP_SERVER_LOCAL_CHANNEL_CAPACITY)
    }

    pub fn with_hub_connector_sender(
        state: Arc<SdkServerState>,
        hub_connector: HubConnectorSender,
    ) -> Self {
        Self::with_channel_capacity_and_hub_connector(
            state,
            APP_SERVER_LOCAL_CHANNEL_CAPACITY,
            Some(hub_connector),
        )
    }

    pub fn with_channel_capacity(state: Arc<SdkServerState>, channel_capacity: usize) -> Self {
        Self::with_channel_capacity_and_hub_connector(state, channel_capacity, None)
    }

    fn with_channel_capacity_and_hub_connector(
        state: Arc<SdkServerState>,
        channel_capacity: usize,
        hub_connector: Option<HubConnectorSender>,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "local AppServer bridge channel capacity must be non-zero"
        );
        let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(
            /*max_sessions*/ 1,
            channel_capacity,
        ));
        let adapter =
            LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), channel_capacity);
        let client = ServerClient::connect_local(&adapter);
        let (outbound_tx, outbound_rx) = mpsc::channel(channel_capacity);
        let handler = AppServerSdkHandler::with_local_app_server(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
        );
        let hub_connector = Arc::new(std::sync::RwLock::new(hub_connector));
        let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
            Arc::clone(&app_server),
            state,
            outbound_rx,
            Arc::clone(&hub_connector),
        );
        Self {
            app_server,
            client,
            handler,
            outbound_forwarder,
            hub_connector,
            event_pump: None,
            event_pump_session_id: None,
            interactive_surface: None,
            channel_capacity,
        }
    }

    pub fn app_server(&self) -> &Arc<AppServer<LocalAppSessionHandle>> {
        &self.app_server
    }

    pub fn client(&self) -> &ServerClient<LocalAppSessionHandle> {
        &self.client
    }

    pub fn set_hub_connector_sender(&self, sender: HubConnectorSender) {
        match self.hub_connector.write() {
            Ok(mut guard) => *guard = Some(sender),
            Err(poisoned) => *poisoned.into_inner() = Some(sender),
        }
    }

    pub fn client_mut(&mut self) -> &mut ServerClient<LocalAppSessionHandle> {
        &mut self.client
    }

    pub fn connect_local_client(&self) -> ServerClient<LocalAppSessionHandle> {
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        ServerClient::connect_local(&adapter)
    }

    pub fn handler(&self) -> &AppServerSdkHandler {
        &self.handler
    }

    pub async fn close_registered_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        close_local_app_server_session(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            session_id,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn load_session_runtime<F>(
        &self,
        session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = session_id.clone();
        let mut completion = match self.app_server.spawn_load(session_id.clone(), async move {
            let runtime = factory
                .await
                .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::from_runtime(registry_session_id, runtime),
            )
        })? {
            AppLoadStart::Started { completion } | AppLoadStart::Loading(completion) => completion,
            AppLoadStart::Live(handle) => {
                return handle.runtime().cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "local AppServer session {session_id} is live without a runtime handle"
                    )
                });
            }
            AppLoadStart::Closing(_) => {
                anyhow::bail!("local AppServer session {session_id} is closing")
            }
        };
        let handle = completion.wait().await?;
        handle.runtime().cloned().ok_or_else(|| {
            anyhow::anyhow!("local AppServer session {session_id} loaded without a runtime handle")
        })
    }

    pub async fn replace_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<Option<(crate::session_runtime::SessionHandle, SurfaceId)>>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        replace_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
        )
        .await
        .and_then(|replacement| {
            replacement
                .map(|(handle, surface_id)| {
                    handle
                        .runtime()
                        .cloned()
                        .map(|runtime| (runtime, surface_id))
                        .ok_or_else(|| JsonRpcDispatchError {
                            code: coco_types::error_codes::INTERNAL_ERROR,
                            message: format!(
                                "local AppServer session {} replaced without a runtime handle",
                                handle.session_id()
                            ),
                            data: None,
                        })
                })
                .transpose()
        })
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub async fn replace_detached_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        replace_detached_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
        )
        .await
        .and_then(|handle| {
            handle
                .runtime()
                .cloned()
                .ok_or_else(|| JsonRpcDispatchError {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!(
                        "local AppServer session {} replaced without a runtime handle",
                        handle.session_id()
                    ),
                    data: None,
                })
        })
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub fn ensure_interactive_surface(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        if self
            .interactive_surface
            .as_ref()
            .is_some_and(|surface| surface.session_id() == &session_id)
        {
            return Ok(());
        }
        if let Some(surface) = self.interactive_surface.as_ref()
            && self.surface_is_attached_to_session(surface.surface_id(), &session_id)
        {
            self.interactive_surface = Some(surface.with_session_id(session_id));
            return Ok(());
        }
        let surface = self
            .client
            .attach_interactive_session(session_id, AttachSurfaceOptions::default())?;
        self.interactive_surface = Some(surface);
        Ok(())
    }

    fn surface_is_attached_to_session(
        &self,
        surface_id: &SurfaceId,
        session_id: &SessionId,
    ) -> bool {
        self.surface_session_id(surface_id).as_ref() == Some(session_id)
    }

    fn surface_session_id(&self, surface_id: &SurfaceId) -> Option<SessionId> {
        let routing = self
            .app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        routing.surface_session(surface_id).cloned()
    }

    pub fn start_passive_event_pump(
        &mut self,
        session_id: SessionId,
        event_tx: mpsc::Sender<CoreEvent>,
    ) -> Result<(), ClientError> {
        if self
            .event_pump_session_id
            .as_ref()
            .is_some_and(|active| active == &session_id)
        {
            return Ok(());
        }
        if let Some(handle) = self.event_pump.take() {
            handle.abort();
        }
        self.event_pump_session_id = None;
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        let mut connection = adapter.connect();
        let surface = connection
            .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
            .map_err(ClientError::from)?;
        let surface_id = surface.surface_id;
        self.event_pump = Some(tokio::spawn(async move {
            while let Some(delivery) = connection.events_mut().recv().await {
                if delivery.surface_id == surface_id
                    && event_tx.send(delivery.envelope.event).await.is_err()
                {
                    break;
                }
            }
        }));
        self.event_pump_session_id = Some(session_id);
        Ok(())
    }

    pub async fn drain_interactive_events_to(&mut self, event_tx: &mpsc::Sender<CoreEvent>) {
        let Some(surface) = self.interactive_surface.clone() else {
            return;
        };
        for pass in 0..2 {
            while let Some(envelope) = self.client.try_next_session_event(&surface) {
                if event_tx.send(envelope.event).await.is_err() {
                    return;
                }
            }
            if pass == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    pub async fn start_turn_and_wait_for_end(
        &mut self,
        session_id: SessionId,
        params: coco_types::TurnStartParams,
    ) -> Result<AppServerLocalTurnCompletion, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        let surface = self
            .interactive_surface
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("interactive surface missing".into()))?;
        let handler = self.handler.clone();
        let started = self.client.turn_start(&handler, params).await?;
        loop {
            let Some(envelope) = self.client.next_session_event(&surface).await else {
                return Err(ClientError::Disconnected);
            };
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == started.turn_id
            {
                return Ok(AppServerLocalTurnCompletion { started, ended });
            }
        }
    }

    pub async fn start_turn(
        &mut self,
        session_id: SessionId,
        params: coco_types::TurnStartParams,
    ) -> Result<coco_types::TurnStartResult, ClientError> {
        self.ensure_interactive_surface(session_id)?;
        self.client.turn_start(&self.handler, params).await
    }

    pub async fn current_session_result(&self) -> Option<coco_types::SessionResultParams> {
        let session_id = if let Some(surface) = self.interactive_surface.as_ref()
            && let Some(session_id) = self.surface_session_id(surface.surface_id())
        {
            session_id
        } else {
            self.handler.state.runtime_or_active_session_id().await?
        };
        Some(session::build_aggregated_session_result(
            &session_id,
            self.handler.state.as_ref(),
            "end_turn",
        ))
    }

    pub async fn replace_session_runtime_for_clear<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<Option<(crate::session_runtime::SessionHandle, SurfaceId)>>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let registry_session_id = new_session_id.clone();
        let replacement = replace_local_app_server_session_with_factory_and_close_reason(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ))
            },
            coco_hooks::orchestration::ExitReason::Clear,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;

        match replacement {
            Some((handle, surface_id)) => {
                let runtime = handle
                    .runtime()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("replacement handle did not include runtime"))?;
                Ok(Some((runtime, surface_id)))
            }
            None => Ok(None),
        }
    }

    pub async fn install_session_snapshot(
        &self,
        session_id: SessionId,
        cwd: impl Into<String>,
        model: impl Into<String>,
    ) {
        let cwd = cwd.into();
        let model = model.into();
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            LocalAppSessionHandle::snapshot(session_id.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry snapshot install failed");
        }
        self.handler
            .state
            .set_session_metadata(session_id.clone(), SessionMetadata { cwd, model });
        self.handler
            .state
            .set_session_handoff(session_id.clone(), SessionHandoffState::new());
        self.handler
            .state
            .clear_session_plan_mode_instructions(&session_id);
        self.handler
            .state
            .reset_session_accounting(session_id.clone());
    }

    pub async fn install_session_runtime(&self, session: crate::session_runtime::SessionHandle) {
        crate::sdk_server::sdk_hooks::install_runtime_callback(
            Arc::clone(&self.handler.state),
            &session,
        );
        let (
            session_id,
            cwd,
            model,
            max_turns,
            system_prompt,
            bypass_permissions_available,
            app_state,
            history,
            session_manager,
            file_history,
            config_home,
        ) = {
            let runtime = session.runtime();
            let session_id = session.session_id().clone();
            let cwd = runtime
                .current_cwd()
                .read()
                .await
                .to_string_lossy()
                .into_owned();
            let config = runtime.current_engine_config().await;
            let history = runtime.history.lock().await.iter().cloned().collect();
            (
                session_id,
                cwd,
                config.model_id.clone(),
                config.max_turns,
                config.system_prompt.clone(),
                config.permission_mode_availability.bypass_permissions,
                Arc::clone(runtime.app_state()),
                history,
                Arc::clone(runtime.session_manager()),
                runtime.file_history().cloned(),
                runtime.config_home().clone(),
            )
        };
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            LocalAppSessionHandle::from_runtime(session_id.clone(), session.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry install failed");
        }

        self.handler
            .state
            .set_session_metadata(session_id.clone(), SessionMetadata { cwd, model });
        self.handler.state.set_session_handoff(
            session_id.clone(),
            SessionHandoffState {
                history: Arc::new(tokio::sync::Mutex::new(history)),
                app_state,
            },
        );
        self.handler
            .state
            .clear_session_plan_mode_instructions(&session_id);
        self.handler
            .state
            .reset_session_accounting(session_id.clone());
        self.handler.state.bypass_permissions_available.store(
            bypass_permissions_available,
            std::sync::atomic::Ordering::Relaxed,
        );
        {
            let mut runner = self.handler.state.turn_runner.write().await;
            *runner = Arc::new(crate::sdk_server::QueryEngineRunner::new(
                session.clone(),
                max_turns,
                system_prompt,
            ));
        }
        {
            let mut slot = self.handler.state.session_manager.write().await;
            *slot = Some(session_manager);
        }
        {
            let mut slot = self.handler.state.file_history.write().await;
            *slot = file_history;
        }
        {
            let mut slot = self.handler.state.file_history_config_home.write().await;
            *slot = Some(config_home);
        }
        let mut slot = self.handler.state.session_runtime.write().await;
        *slot = Some(session);
    }
}

impl Drop for AppServerLocalBridge {
    fn drop(&mut self) {
        self.outbound_forwarder.abort();
        if let Some(handle) = &self.event_pump {
            handle.abort();
        }
    }
}

pub fn spawn_app_server_local_outbound_forwarder<H>(
    server: Arc<AppServer<H>>,
    state: Arc<SdkServerState>,
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    hub_connector: Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> JoinHandle<()>
where
    H: Clone + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut next_session_seq: HashMap<SessionId, i64> = HashMap::new();
        while let Some(outbound) = outbound_rx.recv().await {
            match outbound {
                OutboundMessage::CoreEvent(event) => {
                    let Some(session_id) = current_app_server_session_id(&state).await else {
                        warn!("dropping local AppServer event without an active session");
                        continue;
                    };
                    let hub_connector = clone_hub_connector_sender(&hub_connector);
                    route_local_outbound_event(
                        &server,
                        hub_connector.as_ref(),
                        &mut next_session_seq,
                        session_id,
                        *event,
                    );
                }
                OutboundMessage::SessionCoreEvent { session_id, event } => {
                    let hub_connector = clone_hub_connector_sender(&hub_connector);
                    route_local_outbound_event(
                        &server,
                        hub_connector.as_ref(),
                        &mut next_session_seq,
                        session_id,
                        *event,
                    );
                }
                OutboundMessage::JsonRpc(_) => {
                    warn!("dropping JSON-RPC outbound message on local AppServer forwarder");
                }
            }
        }
    })
}

fn clone_hub_connector_sender(
    hub_connector: &Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> Option<HubConnectorSender> {
    match hub_connector.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn route_local_outbound_event<H>(
    server: &AppServer<H>,
    hub_connector: Option<&HubConnectorSender>,
    next_session_seq: &mut HashMap<SessionId, i64>,
    session_id: SessionId,
    event: CoreEvent,
) where
    H: Clone + Send + Sync + 'static,
{
    let seq_session_id = session_id.clone();
    let envelope = SessionEnvelope::stamp(session_id, None, event, || {
        let next = next_session_seq.entry(seq_session_id).or_insert(1);
        let seq = *next;
        *next += 1;
        seq
    });
    let hub_envelope = envelope.clone();
    server.route_envelope(envelope);
    if let Some(hub_connector) = hub_connector
        && let Err(error) = hub_connector.try_enqueue(hub_envelope)
    {
        warn!(%error, "dropping local AppServer event from Hub connector queue");
    }
}

async fn current_app_server_session_id(state: &SdkServerState) -> Option<SessionId> {
    state.runtime_or_active_session_id().await
}

#[cfg(test)]
fn decode_client_request(
    method: impl Into<String>,
    params: Option<serde_json::Value>,
) -> Result<ClientRequest, serde_json::Error> {
    let method = method.into();
    let mut object = serde_json::Map::new();
    object.insert(
        "method".to_string(),
        serde_json::Value::String(method.clone()),
    );
    if let Some(params) = params {
        object.insert("params".to_string(), params);
    }

    let with_params = serde_json::Value::Object(object);
    match serde_json::from_value(with_params) {
        Ok(request) => Ok(request),
        Err(with_params_error) => {
            let without_params = serde_json::json!({ "method": method });
            serde_json::from_value(without_params).map_err(|_| with_params_error)
        }
    }
}

pub async fn dispatch_sdk_client_request(
    request: ClientRequest,
    ctx: HandlerContext,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    match dispatch_client_request(request, ctx).await {
        HandlerResult::Ok(result) => Ok(result),
        HandlerResult::Err {
            code,
            message,
            data,
        } => Err(JsonRpcDispatchError {
            code,
            message,
            data,
        }),
        HandlerResult::NotImplemented(method) => {
            Err(JsonRpcDispatchError::method_not_found(method))
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
async fn run_app_server_connection_over_sdk_transport<H, Handler>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    handler: Arc<Handler>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
    Handler: JsonRpcRequestHandler,
{
    run_app_server_connection_over_sdk_transport_inner(connection, transport, handler, None, None)
        .await
}

#[cfg(test)]
async fn run_app_server_sdk_state_over_sdk_transport(
    connection: JsonRpcAdapterConnection<LocalAppSessionHandle>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError> {
    run_app_server_sdk_state_over_sdk_transport_with_external_notifications(
        connection,
        transport,
        state,
        Vec::new(),
    )
    .await
}

#[cfg(test)]
async fn run_app_server_sdk_state_over_sdk_transport_with_external_notifications(
    connection: JsonRpcAdapterConnection<LocalAppSessionHandle>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError> {
    run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
        connection,
        transport,
        state,
        external_notifications,
        None,
    )
    .await
}

pub async fn run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
    connection: JsonRpcAdapterConnection<LocalAppSessionHandle>,
    transport: Arc<dyn SdkTransport>,
    state: Arc<SdkServerState>,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
    hub_connector: Option<HubConnectorSender>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError> {
    let app_server = connection.app_server();
    let (outbound_tx, outbound_rx) = mpsc::channel::<OutboundMessage>(256);
    {
        let mut slot = state.transport.write().await;
        *slot = Some(Arc::clone(&transport));
    }
    {
        let mut slot = state.outbound_tx.write().await;
        *slot = Some(outbound_tx.clone());
    }

    let mcp_manager = {
        let slot = state.mcp_manager.read().await;
        slot.as_ref().cloned()
    };
    if let Some(manager) = mcp_manager {
        crate::sdk_server::sdk_mcp::install_route(
            manager,
            Arc::clone(&state),
            Arc::clone(&transport),
        )
        .await;
    }

    let mut external_forwarders = Vec::new();
    for mut rx in external_notifications {
        let forwarded_tx = outbound_tx.clone();
        external_forwarders.push(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if forwarded_tx
                    .send(OutboundMessage::core_event(event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    let hub_egress = hub_connector.map(|sender| SdkHubEgress::new(Arc::clone(&state), sender));
    let writer_task = spawn_sdk_outbound_writer(Arc::clone(&transport), outbound_rx, hub_egress);
    let handler = Arc::new(AppServerSdkHandler::with_local_app_server(
        Arc::clone(&state),
        outbound_tx.clone(),
        app_server,
    ));
    let result = run_app_server_connection_over_sdk_transport_inner(
        connection,
        transport,
        handler,
        Some(outbound_tx.clone()),
        Some(Arc::clone(&state)),
    )
    .await;

    {
        let mut slot = state.outbound_tx.write().await;
        *slot = None;
    }
    for forwarder in external_forwarders {
        forwarder.abort();
        let _ = forwarder.await;
    }
    drop(outbound_tx);
    writer_task.await.map_err(SdkAppServerBridgeError::join)?;
    result
}

async fn run_app_server_connection_over_sdk_transport_inner<H, Handler>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    handler: Arc<Handler>,
    outbound_messages: Option<mpsc::Sender<OutboundMessage>>,
    legacy_response_state: Option<Arc<SdkServerState>>,
) -> Result<DisconnectOutcome, SdkAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
    Handler: JsonRpcRequestHandler,
{
    let (inbound_tx, inbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);
    let (outbound_tx, mut outbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);

    let reader_transport = Arc::clone(&transport);
    let mut reader_task = tokio::spawn(async move {
        loop {
            let Some(message) = reader_transport.recv().await? else {
                break Ok(());
            };
            if matches!(
                message,
                JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_)
            ) && let Some(state) = &legacy_response_state
                && state.resolve_server_request(message.clone()).await
            {
                continue;
            }
            let frame = legacy_json_rpc_message_to_frame(message)?;
            if inbound_tx.send(frame).await.is_err() {
                break Ok(());
            }
        }
    });

    let writer_transport = Arc::clone(&transport);
    let outbound_messages_for_frames = outbound_messages.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            let message = json_rpc_frame_to_legacy_message(frame)?;
            if let Some(outbound_messages) = &outbound_messages_for_frames {
                outbound_messages
                    .send(OutboundMessage::JsonRpc(message))
                    .await
                    .map_err(|_| TransportError::PeerDropped)?;
            } else {
                writer_transport.send(message).await?;
            }
        }
        Ok::<(), SdkAppServerBridgeError>(())
    });

    let owner = connection.run_frame_channels(inbound_rx, outbound_tx, handler);
    tokio::pin!(owner);
    let owner_result = tokio::select! {
        result = &mut owner => result.map_err(SdkAppServerBridgeError::from),
        reader = &mut reader_task => {
            match reader.map_err(SdkAppServerBridgeError::join)? {
                Ok(()) => owner.await.map_err(SdkAppServerBridgeError::from),
                Err(error) => {
                    let _ = owner.await;
                    Err(error)
                }
            }
        }
    };

    if !reader_task.is_finished() {
        reader_task.abort();
        let _ = reader_task.await;
    }
    writer_task.await.map_err(SdkAppServerBridgeError::join)??;
    owner_result
}

fn legacy_json_rpc_message_to_frame(
    message: JsonRpcMessage,
) -> Result<JsonRpcFrame, JsonRpcBridgeError> {
    match message {
        JsonRpcMessage::Request(request) => {
            Ok(JsonRpcFrame::Request(TransportJsonRpcRequest::new(
                json_rpc_id_from_request_id(request.request_id),
                request.method,
                Some(request.params),
            )))
        }
        JsonRpcMessage::Response(response) => Ok(JsonRpcFrame::Success(JsonRpcSuccess::new(
            json_rpc_id_from_request_id(response.request_id),
            response.result,
        ))),
        JsonRpcMessage::Error(error) => {
            Ok(JsonRpcFrame::Error(TransportJsonRpcErrorResponse::new(
                json_rpc_id_from_request_id(error.request_id),
                TransportJsonRpcErrorObject::new(
                    error.error.code,
                    error.error.message,
                    error.error.data,
                ),
            )))
        }
        JsonRpcMessage::Notification(notification) => Ok(JsonRpcFrame::Notification(
            TransportJsonRpcNotification::new(notification.method, Some(notification.params)),
        )),
    }
}

fn json_rpc_frame_to_legacy_message(
    frame: JsonRpcFrame,
) -> Result<JsonRpcMessage, JsonRpcBridgeError> {
    match frame {
        JsonRpcFrame::Request(request) => Ok(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(request.id)?,
            method: request.method,
            params: request.params.unwrap_or(serde_json::Value::Null),
        })),
        JsonRpcFrame::Success(success) => Ok(JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(success.id)?,
            result: success.result,
        })),
        JsonRpcFrame::Error(error) => Ok(JsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request_id_from_json_rpc_id(error.id)?,
            error: JsonRpcErrorObject {
                code: error.error.code,
                message: error.error.message,
                data: error.error.data,
            },
        })),
        JsonRpcFrame::Notification(notification) => {
            Ok(JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.into(),
                method: notification.method,
                params: notification.params.unwrap_or(serde_json::Value::Null),
            }))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JsonRpcBridgeError {
    #[error("legacy SDK JSON-RPC ids cannot represent null")]
    NullId,
}

#[derive(Debug, thiserror::Error)]
pub enum SdkAppServerBridgeError {
    #[error("{source}")]
    Adapter { source: JsonRpcAdapterError },
    #[error("{source}")]
    Transport { source: TransportError },
    #[error("{source}")]
    JsonRpcBridge { source: JsonRpcBridgeError },
    #[error("SDK app-server bridge task failed: {source}")]
    Join { source: tokio::task::JoinError },
}

impl SdkAppServerBridgeError {
    fn join(source: tokio::task::JoinError) -> Self {
        Self::Join { source }
    }
}

impl From<JsonRpcAdapterError> for SdkAppServerBridgeError {
    fn from(source: JsonRpcAdapterError) -> Self {
        Self::Adapter { source }
    }
}

impl From<TransportError> for SdkAppServerBridgeError {
    fn from(source: TransportError) -> Self {
        Self::Transport { source }
    }
}

impl From<JsonRpcBridgeError> for SdkAppServerBridgeError {
    fn from(source: JsonRpcBridgeError) -> Self {
        Self::JsonRpcBridge { source }
    }
}

fn json_rpc_id_from_request_id(request_id: RequestId) -> JsonRpcId {
    match request_id {
        RequestId::Integer(value) => JsonRpcId::Number(value),
        RequestId::String(value) => JsonRpcId::String(value),
    }
}

fn request_id_from_json_rpc_id(id: JsonRpcId) -> Result<RequestId, JsonRpcBridgeError> {
    match id {
        JsonRpcId::Number(value) => Ok(RequestId::Integer(value)),
        JsonRpcId::String(value) => Ok(RequestId::String(value)),
        JsonRpcId::Null => Err(JsonRpcBridgeError::NullId),
    }
}

#[cfg(test)]
#[path = "app_server_bridge.test.rs"]
mod tests;
