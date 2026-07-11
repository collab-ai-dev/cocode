#[cfg(unix)]
use std::path::Path;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

#[cfg(windows)]
use coco_app_server_transport::NdjsonNamedPipeListener;
#[cfg(unix)]
use coco_app_server_transport::NdjsonUnixListener;
#[cfg(windows)]
use coco_app_server_transport::bind_ndjson_named_pipe_listener;
#[cfg(unix)]
use coco_app_server_transport::bind_ndjson_unix_listener;
use coco_app_server_transport::{
    JsonRpcErrorObject, JsonRpcErrorResponse, JsonRpcFrame, JsonRpcId, JsonRpcNotification,
    JsonRpcRequest, JsonRpcSuccess, NdjsonDuplexConnection, NdjsonFrameWriter, TransportFrameError,
};
use coco_types::{
    ClientRequest, CoreEvent, RequestId, RequestScope, ServerRequest, ServerRequestDelivery,
    SessionId, SurfaceDelivery, SurfaceId, SurfaceLifecycleEffect, SurfaceLifecycleEffectKind,
    error_codes, request_scope,
};
use futures::{SinkExt, StreamExt};
use snafu::{ResultExt, Snafu};
use tokio::{
    io::{AsyncBufRead, AsyncRead, AsyncWrite},
    task::JoinSet,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message as WebSocketMessage};

use crate::{
    AppServer, AppServerError, ConnectionKey, DisconnectOutcome, ResolvedServerRequest,
    ServerRequestErrorReply, ServerRequestReply,
};

const DEFAULT_JSON_RPC_CHANNEL_CAPACITY: usize = 128;
const DEFAULT_JSON_RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_EVENT_METHOD: &str = "session/event";
const SESSION_LIFECYCLE_METHOD: &str = "session/lifecycle";

/// JSON-RPC adapter for remote transports.
///
/// The adapter owns wire-level request/response correlation and delegates
/// runtime semantics for client-initiated requests to a handler supplied by the
/// future runtime wiring layer.
pub struct JsonRpcAdapter<H> {
    server: Arc<AppServer<H>>,
    channel_capacity: usize,
    write_timeout: Duration,
}

impl<H> Clone for JsonRpcAdapter<H> {
    fn clone(&self) -> Self {
        Self {
            server: Arc::clone(&self.server),
            channel_capacity: self.channel_capacity,
            write_timeout: self.write_timeout,
        }
    }
}

impl<H: Clone> JsonRpcAdapter<H> {
    pub fn new(server: Arc<AppServer<H>>) -> Self {
        Self::with_channel_capacity(server, DEFAULT_JSON_RPC_CHANNEL_CAPACITY)
    }

    pub fn with_channel_capacity(server: Arc<AppServer<H>>, channel_capacity: usize) -> Self {
        Self::with_channel_capacity_and_write_timeout(
            server,
            channel_capacity,
            DEFAULT_JSON_RPC_WRITE_TIMEOUT,
        )
    }

    pub fn with_channel_capacity_and_write_timeout(
        server: Arc<AppServer<H>>,
        channel_capacity: usize,
        write_timeout: Duration,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "json-rpc channel capacity must be non-zero"
        );
        assert!(
            !write_timeout.is_zero(),
            "json-rpc write timeout must be non-zero"
        );
        Self {
            server,
            channel_capacity,
            write_timeout,
        }
    }

    pub fn connect(&self) -> JsonRpcAdapterConnection<H> {
        let connection = ConnectionKey::generate();
        let (event_tx, events) = tokio::sync::mpsc::channel(self.channel_capacity);
        let (request_tx, server_requests) = tokio::sync::mpsc::channel(self.channel_capacity);
        let (lifecycle_tx, lifecycle) = tokio::sync::mpsc::channel(self.channel_capacity);
        self.server.connect_with_request_and_lifecycle_senders(
            connection,
            event_tx,
            request_tx,
            lifecycle_tx,
        );
        JsonRpcAdapterConnection {
            server: Arc::clone(&self.server),
            connection,
            events,
            server_requests,
            lifecycle,
            pending_server_requests: HashMap::new(),
            write_timeout: self.write_timeout,
        }
    }

    #[cfg(unix)]
    pub async fn accept_unix_connection<Factory>(
        &self,
        listener: &NdjsonUnixListener,
        factory: Arc<Factory>,
    ) -> Result<
        tokio::task::JoinHandle<Result<DisconnectOutcome, JsonRpcConnectionOwnerError>>,
        JsonRpcConnectionOwnerError,
    >
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let transport = listener.accept().await.context(TransportSnafu)?;
        let connection = self.connect();
        let handler = factory.open(connection.connection_key());
        Ok(tokio::spawn(async move {
            connection.run_ndjson_transport(transport, handler).await
        }))
    }

    #[cfg(unix)]
    pub async fn run_unix_listener_until_shutdown<Factory>(
        &self,
        listener: NdjsonUnixListener,
        factory: Arc<Factory>,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let mut owners = JoinSet::new();

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    break;
                }
                accepted = listener.accept() => {
                    let transport = accepted.context(AcceptTransportSnafu)?;
                    let connection = self.connect();
                    let handler = factory.open(connection.connection_key());
                    owners.spawn(async move {
                        connection.run_ndjson_transport(transport, handler).await
                    });
                }
                joined = owners.join_next(), if !owners.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
                }
            }
        }

        while let Some(joined) = owners.join_next().await {
            joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
        }

        Ok(())
    }

    #[cfg(unix)]
    pub async fn bind_and_run_unix_listener_until_shutdown<Factory>(
        &self,
        path: impl AsRef<Path>,
        factory: Arc<Factory>,
        shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let listener = bind_ndjson_unix_listener(path).context(BindTransportSnafu)?;
        self.run_unix_listener_until_shutdown(listener, factory, shutdown)
            .await
    }

    #[cfg(windows)]
    pub async fn run_named_pipe_listener_until_shutdown<Factory>(
        &self,
        mut listener: NdjsonNamedPipeListener,
        factory: Arc<Factory>,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let mut owners = JoinSet::new();

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    break;
                }
                accepted = listener.accept() => {
                    let transport = accepted.context(AcceptTransportSnafu)?;
                    let connection = self.connect();
                    let handler = factory.open(connection.connection_key());
                    owners.spawn(async move {
                        connection.run_ndjson_transport(transport, handler).await
                    });
                }
                joined = owners.join_next(), if !owners.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
                }
            }
        }

        while let Some(joined) = owners.join_next().await {
            joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
        }

        Ok(())
    }

    #[cfg(windows)]
    pub async fn bind_and_run_named_pipe_listener_until_shutdown<Factory>(
        &self,
        pipe_name: impl AsRef<str>,
        factory: Arc<Factory>,
        shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let listener = bind_ndjson_named_pipe_listener(pipe_name).context(BindTransportSnafu)?;
        self.run_named_pipe_listener_until_shutdown(listener, factory, shutdown)
            .await
    }

    pub async fn run_websocket_listener_until_shutdown<Factory>(
        &self,
        listener: tokio::net::TcpListener,
        factory: Arc<Factory>,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Factory: JsonRpcConnectionHandlerFactory,
    {
        let mut owners = JoinSet::new();

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    break;
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.context(AcceptWebSocketSnafu)?;
                    let connection = self.connect();
                    let handler = factory.open(connection.connection_key());
                    owners.spawn(async move {
                        let websocket = tokio_tungstenite::accept_async(stream)
                            .await
                            .context(WebSocketSnafu)?;
                        connection.run_websocket_transport(websocket, handler).await
                    });
                }
                joined = owners.join_next(), if !owners.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
                }
            }
        }

        while let Some(joined) = owners.join_next().await {
            joined.context(OwnerJoinSnafu)?.context(OwnerSnafu)?;
        }

        Ok(())
    }
}

pub struct JsonRpcAdapterConnection<H> {
    server: Arc<AppServer<H>>,
    connection: ConnectionKey,
    events: tokio::sync::mpsc::Receiver<SurfaceDelivery>,
    server_requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
    lifecycle: tokio::sync::mpsc::Receiver<SurfaceLifecycleEffect>,
    pending_server_requests: HashMap<JsonRpcId, PendingJsonRpcServerRequest>,
    write_timeout: Duration,
}

impl<H: Clone> JsonRpcAdapterConnection<H> {
    pub fn app_server(&self) -> Arc<AppServer<H>> {
        Arc::clone(&self.server)
    }

    pub fn connection_key(&self) -> ConnectionKey {
        self.connection
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceDelivery> {
        &mut self.events
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        &mut self.server_requests
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleEffect> {
        &mut self.lifecycle
    }

    pub fn encode_server_request(
        &mut self,
        delivery: ServerRequestDelivery,
    ) -> Result<JsonRpcFrame, JsonRpcAdapterError> {
        let id = json_rpc_id_from_request_id(&delivery.request_id);
        let (method, params) = server_request_method_and_params(&delivery.request)?;
        self.pending_server_requests.insert(
            id.clone(),
            PendingJsonRpcServerRequest {
                session_id: delivery.session_id,
                surface_id: delivery.surface_id,
                request_id: delivery.request_id,
                request: delivery.request,
            },
        );
        Ok(JsonRpcFrame::Request(JsonRpcRequest::new(
            id, method, params,
        )))
    }

    pub fn complete_server_request_response(
        &mut self,
        frame: JsonRpcFrame,
    ) -> Result<JsonRpcServerRequestResponse, JsonRpcAdapterError> {
        match frame {
            JsonRpcFrame::Success(success) => {
                let pending = self.remove_pending_server_request(&success.id)?;
                Ok(JsonRpcServerRequestResponse::Success {
                    pending,
                    result: success.result,
                })
            }
            JsonRpcFrame::Error(error) => {
                let pending = self.remove_pending_server_request(&error.id)?;
                Ok(JsonRpcServerRequestResponse::Error {
                    pending,
                    error: error.error,
                })
            }
            other => UnexpectedResponseFrameSnafu { frame: other }.fail(),
        }
    }

    pub fn resolve_server_request_response(
        &mut self,
        frame: JsonRpcFrame,
    ) -> Result<ResolvedServerRequest, JsonRpcAdapterError> {
        let response = self.complete_server_request_response(frame)?;
        let (request_id, reply) = match response {
            JsonRpcServerRequestResponse::Success { pending, result } => {
                let reply = server_request_reply_from_success(&pending, result)?;
                (pending.request_id, reply)
            }
            JsonRpcServerRequestResponse::Error { pending, error } => {
                let request_id = pending.request_id.as_display();
                (
                    pending.request_id,
                    ServerRequestReply::Error(ServerRequestErrorReply {
                        request_id,
                        code: error.code,
                        message: error.message,
                        data: error.data,
                    }),
                )
            }
        };
        self.server
            .resolve_server_request_by_id(&request_id, reply)
            .context(ResolveServerRequestSnafu)
    }

    pub async fn dispatch_client_request(
        &self,
        request: JsonRpcRequest,
        handler: &dyn JsonRpcRequestHandler,
    ) -> JsonRpcFrame {
        dispatch_client_request_for_connection(self.connection, request, handler).await
    }

    pub async fn run_ndjson_transport<R, W, Handler>(
        mut self,
        transport: NdjsonDuplexConnection<R, W>,
        handler: Arc<Handler>,
    ) -> Result<DisconnectOutcome, JsonRpcConnectionOwnerError>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
        Handler: JsonRpcRequestHandler,
    {
        let (mut reader, mut writer) = transport.split();
        let result = loop {
            tokio::select! {
                frame = reader.read_frame() => {
                    match frame {
                        Ok(Some(frame)) => {
                            let response = match self.handle_inbound_frame(frame, handler.as_ref()).await {
                                Ok(response) => response,
                                Err(error) => break Err(error.into()),
                            };
                            if let Some(response) = response
                                && let Err(error) = write_ndjson_json_rpc_frame_with_timeout(
                                    &mut writer,
                                    &response,
                                    self.write_timeout,
                                )
                                .await
                            {
                                break Err(error);
                            }
                        }
                        Ok(None) => break Ok(()),
                        Err(source) => break Err(JsonRpcConnectionOwnerError::Transport { source }),
                    }
                }
                delivery = self.events.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match encode_surface_delivery(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error.into()),
                    };
                    if let Err(error) = write_ndjson_json_rpc_frame_with_timeout(
                        &mut writer,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
                delivery = self.server_requests.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match self.encode_server_request(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error.into()),
                    };
                    if let Err(error) = write_ndjson_json_rpc_frame_with_timeout(
                        &mut writer,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
                delivery = self.lifecycle.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_lifecycle_delivery(delivery);
                    if let Err(error) = write_ndjson_json_rpc_frame_with_timeout(
                        &mut writer,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
            }
        };

        let outcome = self.server.disconnect(self.connection);
        result.map(|()| outcome)
    }

    pub async fn run_websocket_transport<S, Handler>(
        mut self,
        mut websocket: WebSocketStream<S>,
        handler: Arc<Handler>,
    ) -> Result<DisconnectOutcome, JsonRpcConnectionOwnerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
        Handler: JsonRpcRequestHandler,
    {
        let result = loop {
            tokio::select! {
                message = websocket.next() => {
                    let Some(message) = message else {
                        break Ok(());
                    };
                    let message = match message {
                        Ok(message) => message,
                        Err(source) => break Err(JsonRpcConnectionOwnerError::WebSocket { source }),
                    };
                    let inbound = match json_rpc_frame_from_websocket_message(message) {
                        Ok(inbound) => inbound,
                        Err(error) => break Err(error),
                    };
                    match inbound {
                        WebSocketInboundFrame::Frame(frame) => {
                            let response = match self.handle_inbound_frame(frame, handler.as_ref()).await {
                                Ok(response) => response,
                                Err(error) => break Err(error.into()),
                            };
                            if let Some(response) = response
                                && let Err(error) = write_websocket_json_rpc_frame_with_timeout(
                                    &mut websocket,
                                    &response,
                                    self.write_timeout,
                                )
                                .await
                            {
                                break Err(error);
                            }
                        }
                        WebSocketInboundFrame::Ignore => {}
                        WebSocketInboundFrame::Closed => break Ok(()),
                    }
                }
                delivery = self.events.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match encode_surface_delivery(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error.into()),
                    };
                    if let Err(error) = write_websocket_json_rpc_frame_with_timeout(
                        &mut websocket,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
                delivery = self.server_requests.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match self.encode_server_request(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error.into()),
                    };
                    if let Err(error) = write_websocket_json_rpc_frame_with_timeout(
                        &mut websocket,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
                delivery = self.lifecycle.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_lifecycle_delivery(delivery);
                    if let Err(error) = write_websocket_json_rpc_frame_with_timeout(
                        &mut websocket,
                        &frame,
                        self.write_timeout,
                    )
                    .await
                    {
                        break Err(error);
                    }
                }
            }
        };

        let outcome = self.server.disconnect(self.connection);
        result.map(|()| outcome)
    }

    pub async fn run_frame_channels<Handler>(
        mut self,
        mut inbound: tokio::sync::mpsc::Receiver<JsonRpcFrame>,
        outbound: tokio::sync::mpsc::Sender<JsonRpcFrame>,
        handler: Arc<Handler>,
    ) -> Result<DisconnectOutcome, JsonRpcAdapterError>
    where
        Handler: JsonRpcRequestHandler,
    {
        let mut dispatches = JoinSet::new();
        let result = loop {
            tokio::select! {
                biased;
                delivery = self.events.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match encode_surface_delivery(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error),
                    };
                    match send_json_rpc_frame_with_timeout(&outbound, frame, self.write_timeout).await {
                        Ok(true) => {}
                        Ok(false) => break Ok(()),
                        Err(error) => break Err(error),
                    }
                }
                delivery = self.server_requests.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = match self.encode_server_request(delivery) {
                        Ok(frame) => frame,
                        Err(error) => break Err(error),
                    };
                    match send_json_rpc_frame_with_timeout(&outbound, frame, self.write_timeout).await {
                        Ok(true) => {}
                        Ok(false) => break Ok(()),
                        Err(error) => break Err(error),
                    }
                }
                delivery = self.lifecycle.recv(), if dispatches.is_empty() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_lifecycle_delivery(delivery);
                    match send_json_rpc_frame_with_timeout(&outbound, frame, self.write_timeout).await {
                        Ok(true) => {}
                        Ok(false) => break Ok(()),
                        Err(error) => break Err(error),
                    }
                }
                frame = inbound.recv() => {
                    let Some(frame) = frame else {
                        break Ok(());
                    };
                    match frame {
                        JsonRpcFrame::Request(request) => {
                            let handler = Arc::clone(&handler);
                            let connection = self.connection;
                            dispatches.spawn(async move {
                                Some(
                                    dispatch_client_request_for_connection(
                                        connection,
                                        request,
                                        handler.as_ref(),
                                    )
                                    .await,
                                )
                            });
                        }
                        JsonRpcFrame::Notification(notification) => {
                            if let Ok(request) = client_request_from_method_and_params(
                                notification.method,
                                notification.params,
                            ) {
                                let handler = Arc::clone(&handler);
                                let context = JsonRpcRequestContext {
                                    connection: self.connection,
                                    scope: request_scope(request.method()),
                                };
                                dispatches.spawn(async move {
                                    let _ = handler.handle_json_rpc_request(context, request).await;
                                    None
                                });
                            }
                        }
                        response @ (JsonRpcFrame::Success(_) | JsonRpcFrame::Error(_)) => {
                            if let Err(error) = self.resolve_server_request_response(response) {
                                break Err(error);
                            }
                        }
                    }
                }
                joined = dispatches.join_next(), if !dispatches.is_empty() => {
                    match joined {
                        Some(Ok(Some(response))) => {
                            match send_json_rpc_frame_with_timeout(
                                &outbound,
                                response,
                                self.write_timeout,
                            ).await {
                                Ok(true) => {}
                                Ok(false) => break Ok(()),
                                Err(error) => break Err(error),
                            }
                        }
                        Some(Ok(None)) => {}
                        Some(Err(source)) => {
                            break Err(JsonRpcAdapterError::RequestDispatchJoin { source });
                        }
                        None => {}
                    }
                }
            }
        };

        dispatches.abort_all();
        while dispatches.join_next().await.is_some() {}
        let outcome = self.server.disconnect(self.connection);
        result.map(|()| outcome)
    }

    pub fn disconnect(self) -> DisconnectOutcome {
        self.server.disconnect(self.connection)
    }

    async fn handle_inbound_frame(
        &mut self,
        frame: JsonRpcFrame,
        handler: &dyn JsonRpcRequestHandler,
    ) -> Result<Option<JsonRpcFrame>, JsonRpcAdapterError> {
        match frame {
            JsonRpcFrame::Request(request) => {
                Ok(Some(self.dispatch_client_request(request, handler).await))
            }
            JsonRpcFrame::Notification(notification) => {
                if let Ok(request) =
                    client_request_from_method_and_params(notification.method, notification.params)
                {
                    let context = JsonRpcRequestContext {
                        connection: self.connection,
                        scope: request_scope(request.method()),
                    };
                    let _ = handler.handle_json_rpc_request(context, request).await;
                }
                Ok(None)
            }
            response @ (JsonRpcFrame::Success(_) | JsonRpcFrame::Error(_)) => {
                self.resolve_server_request_response(response)?;
                Ok(None)
            }
        }
    }

    fn remove_pending_server_request(
        &mut self,
        id: &JsonRpcId,
    ) -> Result<PendingJsonRpcServerRequest, JsonRpcAdapterError> {
        self.pending_server_requests
            .remove(id)
            .ok_or_else(|| JsonRpcAdapterError::UnknownResponseId { id: id.clone() })
    }
}

async fn dispatch_client_request_for_connection(
    connection: ConnectionKey,
    request: JsonRpcRequest,
    handler: &dyn JsonRpcRequestHandler,
) -> JsonRpcFrame {
    let id = request.id.clone();
    let response = match client_request_from_json_rpc(&request) {
        Ok(request) => {
            let context = JsonRpcRequestContext {
                connection,
                scope: request_scope(request.method()),
            };
            handler.handle_json_rpc_request(context, request).await
        }
        Err(error) => Err(error.into_dispatch_error()),
    };
    match response {
        Ok(result) => JsonRpcFrame::Success(JsonRpcSuccess::new(id, result)),
        Err(error) => JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            id,
            JsonRpcErrorObject::new(error.code, error.message, error.data),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct PendingJsonRpcServerRequest {
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
    pub request_id: RequestId,
    pub request: ServerRequest,
}

#[derive(Debug, Clone)]
pub enum JsonRpcServerRequestResponse {
    Success {
        pending: PendingJsonRpcServerRequest,
        result: serde_json::Value,
    },
    Error {
        pending: PendingJsonRpcServerRequest,
        error: JsonRpcErrorObject,
    },
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum JsonRpcAdapterError {
    #[snafu(display("failed to encode server request: {source}"))]
    EncodeServerRequest { source: serde_json::Error },
    #[snafu(display("failed to decode client request: {source}"))]
    DecodeClientRequest { source: serde_json::Error },
    #[snafu(display("failed to decode server request reply: {source}"))]
    DecodeServerRequestReply { source: serde_json::Error },
    #[snafu(display("failed to resolve server request: {source}"))]
    ResolveServerRequest { source: AppServerError },
    #[snafu(display("unexpected JSON-RPC response frame: {frame:?}"))]
    UnexpectedResponseFrame { frame: JsonRpcFrame },
    #[snafu(display("unknown JSON-RPC response id: {id:?}"))]
    UnknownResponseId { id: JsonRpcId },
    #[snafu(display("JSON-RPC outbound channel did not accept a frame within {timeout:?}"))]
    SlowConsumer { timeout: Duration },
    #[snafu(display("JSON-RPC request dispatch task failed: {source}"))]
    RequestDispatchJoin { source: tokio::task::JoinError },
}

#[derive(Debug, Snafu)]
pub enum JsonRpcConnectionOwnerError {
    #[snafu(display("{source}"))]
    Adapter { source: JsonRpcAdapterError },
    #[snafu(display("{source}"))]
    Transport { source: TransportFrameError },
    #[snafu(display("{source}"))]
    WebSocket {
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[snafu(display("failed to encode websocket JSON-RPC frame: {source}"))]
    EncodeWebSocketFrame { source: serde_json::Error },
    #[snafu(display("failed to decode websocket JSON-RPC frame: {source}"))]
    DecodeWebSocketFrame { source: serde_json::Error },
    #[snafu(display("JSON-RPC transport did not accept a frame within {timeout:?}"))]
    TransportSlowConsumer { timeout: Duration },
}

impl From<JsonRpcAdapterError> for JsonRpcConnectionOwnerError {
    fn from(source: JsonRpcAdapterError) -> Self {
        Self::Adapter { source }
    }
}

#[derive(Debug, Snafu)]
pub enum JsonRpcListenerError {
    #[snafu(display("{source}"))]
    BindTransport { source: TransportFrameError },
    #[snafu(display("{source}"))]
    AcceptTransport { source: TransportFrameError },
    #[snafu(display("failed to accept AppServer WebSocket connection: {source}"))]
    AcceptWebSocket { source: std::io::Error },
    #[snafu(display("{source}"))]
    Owner { source: JsonRpcConnectionOwnerError },
    #[snafu(display("JSON-RPC connection owner task failed: {source}"))]
    OwnerJoin { source: tokio::task::JoinError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonRpcRequestContext {
    pub connection: ConnectionKey,
    pub scope: RequestScope,
}

pub type JsonRpcRequestFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, JsonRpcDispatchError>> + Send>>;

pub trait JsonRpcRequestHandler: Send + Sync + 'static {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture;
}

/// Constructs isolated request state for one accepted transport connection.
pub trait JsonRpcConnectionHandlerFactory: Send + Sync + 'static {
    type Handler: JsonRpcRequestHandler;

    fn open(&self, connection: ConnectionKey) -> Arc<Self::Handler>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcDispatchError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl JsonRpcDispatchError {
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: error_codes::METHOD_NOT_FOUND,
            message: format!("unknown method: {}", method.into()),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: error_codes::INVALID_PARAMS,
            message: message.into(),
            data: None,
        }
    }
}

impl JsonRpcAdapterError {
    fn into_dispatch_error(self) -> JsonRpcDispatchError {
        match self {
            Self::DecodeClientRequest { source } => {
                JsonRpcDispatchError::invalid_params(source.to_string())
            }
            other => JsonRpcDispatchError {
                code: error_codes::INTERNAL_ERROR,
                message: other.to_string(),
                data: None,
            },
        }
    }
}

fn json_rpc_id_from_request_id(request_id: &RequestId) -> JsonRpcId {
    match request_id {
        RequestId::Integer(value) => JsonRpcId::Number(*value),
        RequestId::String(value) => JsonRpcId::String(value.clone()),
    }
}

fn server_request_method_and_params(
    request: &ServerRequest,
) -> Result<(String, Option<serde_json::Value>), JsonRpcAdapterError> {
    let value = serde_json::to_value(request).context(EncodeServerRequestSnafu)?;
    let serde_json::Value::Object(mut object) = value else {
        return Ok((request.method().as_str().to_string(), None));
    };
    let method = match object.remove("method") {
        Some(serde_json::Value::String(method)) => method,
        _ => request.method().as_str().to_string(),
    };
    let params = object.remove("params");
    Ok((method, params))
}

fn client_request_from_json_rpc(
    request: &JsonRpcRequest,
) -> Result<ClientRequest, JsonRpcAdapterError> {
    client_request_from_method_and_params(request.method.clone(), request.params.clone())
}

fn client_request_from_method_and_params(
    method: String,
    params: Option<serde_json::Value>,
) -> Result<ClientRequest, JsonRpcAdapterError> {
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
        Err(source) => {
            let without_params = serde_json::json!({ "method": method });
            serde_json::from_value(without_params)
                .map_err(|_| source)
                .context(DecodeClientRequestSnafu)
        }
    }
}

fn server_request_reply_from_success(
    pending: &PendingJsonRpcServerRequest,
    result: serde_json::Value,
) -> Result<ServerRequestReply, JsonRpcAdapterError> {
    match &pending.request {
        ServerRequest::AskForApproval(params) => {
            decode_targeted_reply(result, &params.request_id, pending)
                .map(ServerRequestReply::Approval)
        }
        ServerRequest::RequestUserInput(params) => {
            decode_targeted_reply(result, &params.request_id, pending)
                .map(ServerRequestReply::UserInput)
        }
        ServerRequest::RequestElicitation(params) => {
            decode_targeted_reply(result, &params.request_id, pending)
                .map(ServerRequestReply::Elicitation)
        }
        ServerRequest::McpRouteMessage(_) => Ok(ServerRequestReply::McpRouteMessage {
            request_id: pending.request_id.as_display(),
            result,
        }),
        ServerRequest::HookCallback(_) => Ok(ServerRequestReply::HookCallback {
            request_id: pending.request_id.as_display(),
            result,
        }),
        ServerRequest::CancelRequest(_) => Ok(ServerRequestReply::McpRouteMessage {
            request_id: pending.request_id.as_display(),
            result,
        }),
    }
}

fn decode_targeted_reply<T>(
    result: serde_json::Value,
    request_id: &str,
    pending: &PendingJsonRpcServerRequest,
) -> Result<T, JsonRpcAdapterError>
where
    T: serde::de::DeserializeOwned,
{
    let mut result = ensure_request_id(result, request_id);
    if let serde_json::Value::Object(object) = &mut result {
        object.insert(
            "target".to_string(),
            serde_json::json!({
                "session_id": pending.session_id,
                "surface_id": pending.surface_id,
            }),
        );
    }
    serde_json::from_value(result).context(DecodeServerRequestReplySnafu)
}

fn ensure_request_id(mut value: serde_json::Value, request_id: &str) -> serde_json::Value {
    if let serde_json::Value::Object(object) = &mut value {
        object
            .entry("request_id")
            .or_insert_with(|| serde_json::Value::String(request_id.to_string()));
    }
    value
}

enum WebSocketInboundFrame {
    Frame(JsonRpcFrame),
    Ignore,
    Closed,
}

fn json_rpc_frame_from_websocket_message(
    message: WebSocketMessage,
) -> Result<WebSocketInboundFrame, JsonRpcConnectionOwnerError> {
    match message {
        WebSocketMessage::Text(text) => serde_json::from_str(text.as_ref())
            .map(WebSocketInboundFrame::Frame)
            .context(DecodeWebSocketFrameSnafu),
        WebSocketMessage::Binary(bytes) => serde_json::from_slice(bytes.as_ref())
            .map(WebSocketInboundFrame::Frame)
            .context(DecodeWebSocketFrameSnafu),
        WebSocketMessage::Close(_) => Ok(WebSocketInboundFrame::Closed),
        WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => Ok(WebSocketInboundFrame::Ignore),
        WebSocketMessage::Frame(_) => Ok(WebSocketInboundFrame::Ignore),
    }
}

async fn write_ndjson_json_rpc_frame_with_timeout<W>(
    writer: &mut NdjsonFrameWriter<W>,
    frame: &JsonRpcFrame,
    timeout: Duration,
) -> Result<(), JsonRpcConnectionOwnerError>
where
    W: AsyncWrite + Unpin,
{
    match tokio::time::timeout(timeout, writer.write_frame(frame)).await {
        Ok(result) => result.context(TransportSnafu),
        Err(_) => TransportSlowConsumerSnafu { timeout }.fail(),
    }
}

async fn write_websocket_json_rpc_frame_with_timeout<S>(
    websocket: &mut WebSocketStream<S>,
    frame: &JsonRpcFrame,
    timeout: Duration,
) -> Result<(), JsonRpcConnectionOwnerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match tokio::time::timeout(timeout, write_websocket_json_rpc_frame(websocket, frame)).await {
        Ok(result) => result,
        Err(_) => TransportSlowConsumerSnafu { timeout }.fail(),
    }
}

async fn write_websocket_json_rpc_frame<S>(
    websocket: &mut WebSocketStream<S>,
    frame: &JsonRpcFrame,
) -> Result<(), JsonRpcConnectionOwnerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame).context(EncodeWebSocketFrameSnafu)?;
    websocket
        .send(WebSocketMessage::Text(text.into()))
        .await
        .context(WebSocketSnafu)
}

async fn send_json_rpc_frame_with_timeout(
    outbound: &tokio::sync::mpsc::Sender<JsonRpcFrame>,
    frame: JsonRpcFrame,
    timeout: Duration,
) -> Result<bool, JsonRpcAdapterError> {
    match tokio::time::timeout(timeout, outbound.send(frame)).await {
        Ok(Ok(())) => Ok(true),
        Ok(Err(_)) => Ok(false),
        Err(_) => SlowConsumerSnafu { timeout }.fail(),
    }
}

fn encode_surface_delivery(delivery: SurfaceDelivery) -> Result<JsonRpcFrame, JsonRpcAdapterError> {
    let envelope = delivery.envelope;
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
    let params = serde_json::to_value(serde_json::json!({
        "surface_id": delivery.surface_id,
        "envelope": {
            "session_id": envelope.session_id,
            "agent_id": envelope.agent_id,
            "turn_id": envelope.turn_id,
            "session_seq": envelope.session_seq,
            "event": event,
        },
    }))
    .context(EncodeServerRequestSnafu)?;
    Ok(JsonRpcFrame::Notification(JsonRpcNotification::new(
        SESSION_EVENT_METHOD,
        Some(params),
    )))
}

fn encode_lifecycle_delivery(delivery: SurfaceLifecycleEffect) -> JsonRpcFrame {
    let kind = match delivery.kind {
        SurfaceLifecycleEffectKind::SessionStarted { session_id } => {
            serde_json::json!({
                "type": "session_started",
                "session_id": session_id,
            })
        }
        SurfaceLifecycleEffectKind::SessionReplaced {
            old_session_id,
            new_session_id,
        } => {
            serde_json::json!({
                "type": "session_replaced",
                "old_session_id": old_session_id,
                "new_session_id": new_session_id,
            })
        }
        SurfaceLifecycleEffectKind::SessionEnded { session_id } => {
            serde_json::json!({
                "type": "session_ended",
                "session_id": session_id,
            })
        }
    };
    JsonRpcFrame::Notification(JsonRpcNotification::new(
        SESSION_LIFECYCLE_METHOD,
        Some(serde_json::json!({
            "surface_id": delivery.surface_id,
            "effect": kind,
        })),
    ))
}

#[cfg(test)]
#[path = "json_rpc_adapter.test.rs"]
mod tests;
