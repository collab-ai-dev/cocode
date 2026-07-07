use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use coco_app_server_transport::JsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcNotification;
use coco_app_server_transport::JsonRpcRequest;
use coco_app_server_transport::JsonRpcSuccess;
use coco_app_server_transport::NdjsonDuplexConnection;
#[cfg(unix)]
use coco_app_server_transport::NdjsonUnixListener;
use coco_app_server_transport::TransportFrameError;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::RequestId;
use coco_types::ServerRequest;
use coco_types::SurfaceId;
use coco_types::error_codes;
use snafu::ResultExt;
use snafu::Snafu;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
#[cfg(unix)]
use tokio::task::JoinSet;

use crate::AppServer;
use crate::AppServerError;
use crate::ConnectionKey;
use crate::DisconnectOutcome;
use crate::ResolvedServerRequest;
use crate::ServerRequestDelivery;
use crate::ServerRequestErrorReply;
use crate::ServerRequestReply;
use crate::SurfaceDelivery;
use crate::SurfaceLifecycleDelivery;
use crate::SurfaceLifecycleEffectKind;

const DEFAULT_JSON_RPC_CHANNEL_CAPACITY: usize = 128;
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
}

impl<H> Clone for JsonRpcAdapter<H> {
    fn clone(&self) -> Self {
        Self {
            server: Arc::clone(&self.server),
            channel_capacity: self.channel_capacity,
        }
    }
}

impl<H: Clone> JsonRpcAdapter<H> {
    pub fn new(server: Arc<AppServer<H>>) -> Self {
        Self::with_channel_capacity(server, DEFAULT_JSON_RPC_CHANNEL_CAPACITY)
    }

    pub fn with_channel_capacity(server: Arc<AppServer<H>>, channel_capacity: usize) -> Self {
        assert!(
            channel_capacity > 0,
            "json-rpc channel capacity must be non-zero"
        );
        Self {
            server,
            channel_capacity,
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
        }
    }

    #[cfg(unix)]
    pub async fn accept_unix_connection<Handler>(
        &self,
        listener: &NdjsonUnixListener,
        handler: Arc<Handler>,
    ) -> Result<
        tokio::task::JoinHandle<Result<DisconnectOutcome, JsonRpcConnectionOwnerError>>,
        JsonRpcConnectionOwnerError,
    >
    where
        H: Send + Sync + 'static,
        Handler: JsonRpcRequestHandler,
    {
        let transport = listener.accept().await.context(TransportSnafu)?;
        let connection = self.connect();
        Ok(tokio::spawn(async move {
            connection.run_ndjson_transport(transport, handler).await
        }))
    }

    #[cfg(unix)]
    pub async fn run_unix_listener_until_shutdown<Handler>(
        &self,
        listener: NdjsonUnixListener,
        handler: Arc<Handler>,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), JsonRpcListenerError>
    where
        H: Send + Sync + 'static,
        Handler: JsonRpcRequestHandler,
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
                    let handler = Arc::clone(&handler);
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
}

pub struct JsonRpcAdapterConnection<H> {
    server: Arc<AppServer<H>>,
    connection: ConnectionKey,
    events: tokio::sync::mpsc::Receiver<SurfaceDelivery>,
    server_requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
    lifecycle: tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery>,
    pending_server_requests: HashMap<JsonRpcId, PendingJsonRpcServerRequest>,
}

impl<H: Clone> JsonRpcAdapterConnection<H> {
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

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery> {
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
        let id = request.id.clone();
        let response = match client_request_from_json_rpc(&request) {
            Ok(request) => {
                let context = JsonRpcRequestContext {
                    connection: self.connection,
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
                    match frame.context(TransportSnafu)? {
                        Some(frame) => {
                            if let Some(response) = self.handle_inbound_frame(frame, handler.as_ref()).await? {
                                writer.write_frame(&response).await.context(TransportSnafu)?;
                            }
                        }
                        None => break Ok(()),
                    }
                }
                delivery = self.events.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_surface_delivery(delivery)?;
                    writer.write_frame(&frame).await.context(TransportSnafu)?;
                }
                delivery = self.server_requests.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = self.encode_server_request(delivery)?;
                    writer.write_frame(&frame).await.context(TransportSnafu)?;
                }
                delivery = self.lifecycle.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_lifecycle_delivery(delivery);
                    writer.write_frame(&frame).await.context(TransportSnafu)?;
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
        let result = loop {
            tokio::select! {
                frame = inbound.recv() => {
                    let Some(frame) = frame else {
                        break Ok(());
                    };
                    if let Some(response) = self.handle_inbound_frame(frame, handler.as_ref()).await?
                        && outbound.send(response).await.is_err()
                    {
                        break Ok(());
                    }
                }
                delivery = self.events.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_surface_delivery(delivery)?;
                    if outbound.send(frame).await.is_err() {
                        break Ok(());
                    }
                }
                delivery = self.server_requests.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = self.encode_server_request(delivery)?;
                    if outbound.send(frame).await.is_err() {
                        break Ok(());
                    }
                }
                delivery = self.lifecycle.recv() => {
                    let Some(delivery) = delivery else {
                        break Ok(());
                    };
                    let frame = encode_lifecycle_delivery(delivery);
                    if outbound.send(frame).await.is_err() {
                        break Ok(());
                    }
                }
            }
        };

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

#[derive(Debug, Clone)]
pub struct PendingJsonRpcServerRequest {
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
}

#[derive(Debug, Snafu)]
pub enum JsonRpcConnectionOwnerError {
    #[snafu(display("{source}"))]
    Adapter { source: JsonRpcAdapterError },
    #[snafu(display("{source}"))]
    Transport { source: TransportFrameError },
}

impl From<JsonRpcAdapterError> for JsonRpcConnectionOwnerError {
    fn from(source: JsonRpcAdapterError) -> Self {
        Self::Adapter { source }
    }
}

#[cfg(unix)]
#[derive(Debug, Snafu)]
pub enum JsonRpcListenerError {
    #[snafu(display("{source}"))]
    AcceptTransport { source: TransportFrameError },
    #[snafu(display("{source}"))]
    Owner { source: JsonRpcConnectionOwnerError },
    #[snafu(display("JSON-RPC connection owner task failed: {source}"))]
    OwnerJoin { source: tokio::task::JoinError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonRpcRequestContext {
    pub connection: ConnectionKey,
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
            decode_reply_with_request_id(result, &params.request_id)
                .map(ServerRequestReply::Approval)
        }
        ServerRequest::RequestUserInput(params) => {
            decode_reply_with_request_id(result, &params.request_id)
                .map(ServerRequestReply::UserInput)
        }
        ServerRequest::RequestElicitation(params) => {
            decode_reply_with_request_id(result, &params.request_id)
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

fn decode_reply_with_request_id<T>(
    result: serde_json::Value,
    request_id: &str,
) -> Result<T, JsonRpcAdapterError>
where
    T: serde::de::DeserializeOwned,
{
    let result = ensure_request_id(result, request_id);
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

fn encode_lifecycle_delivery(delivery: SurfaceLifecycleDelivery) -> JsonRpcFrame {
    let kind = match delivery.effect.kind {
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
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use coco_app_server_transport::JsonRpcNotification;
    use coco_app_server_transport::JsonRpcSuccess;
    use coco_app_server_transport::NdjsonDuplexConnection;
    use coco_types::ClientRequestMethod;
    use coco_types::ServerRequest;
    use coco_types::ServerRequestUserInputParams;
    use coco_types::SessionId;
    use coco_types::SurfaceId;
    use coco_types::TurnId;
    use tokio::io::BufReader;
    use tokio::io::split;

    use super::*;
    use crate::AppServer;
    use crate::AttachSurfaceOptions;
    use crate::SurfaceCapabilities;
    use crate::SurfaceCapability;
    use crate::SurfaceRole;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestHandle(&'static str);

    #[derive(Default)]
    struct RecordingHandler {
        methods: Mutex<Vec<ClientRequestMethod>>,
    }

    impl RecordingHandler {
        fn methods(&self) -> Vec<ClientRequestMethod> {
            self.methods.lock().expect("handler lock").clone()
        }
    }

    impl JsonRpcRequestHandler for RecordingHandler {
        fn handle_json_rpc_request(
            &self,
            _context: JsonRpcRequestContext,
            request: ClientRequest,
        ) -> JsonRpcRequestFuture {
            self.methods
                .lock()
                .expect("handler lock")
                .push(request.method());
            Box::pin(async { Ok(serde_json::json!({ "ok": true })) })
        }
    }

    fn test_session_id(value: &str) -> SessionId {
        SessionId::try_new(value).expect("valid test session id")
    }

    fn test_server_request() -> ServerRequest {
        ServerRequest::RequestUserInput(ServerRequestUserInputParams {
            request_id: "payload-request-id".to_string(),
            prompt: "continue?".to_string(),
            description: None,
            choices: Vec::new(),
            default: None,
        })
    }

    #[test]
    fn json_rpc_adapter_encodes_server_request_and_tracks_response_id() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut connection = adapter.connect();
        let session_id = test_session_id("sess-1");
        let surface_id = SurfaceId::from("surface-1");
        server
            .attach_surface_with_options(
                connection.connection_key(),
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach remote interactive surface");
        let routed = server
            .route_server_request(
                session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-1")),
                test_server_request(),
            )
            .expect("route server request");
        let delivery = connection
            .server_requests_mut()
            .try_recv()
            .expect("request delivery");

        let frame = connection
            .encode_server_request(delivery)
            .expect("encode server request");
        let JsonRpcFrame::Request(request) = frame else {
            panic!("expected request frame");
        };
        assert_eq!(
            request.id,
            json_rpc_id_from_request_id(&routed.pending.request_id)
        );
        assert_eq!(request.method, "input/requestUserInput");
        assert_eq!(
            request.params.as_ref().expect("request params")["request_id"],
            "payload-request-id"
        );

        let response = connection
            .complete_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "answer": "yes" }),
            )))
            .expect("complete response correlation");
        let JsonRpcServerRequestResponse::Success { pending, result } = response else {
            panic!("expected success response");
        };
        assert_eq!(pending.surface_id, surface_id);
        assert_eq!(pending.request_id, routed.pending.request_id);
        assert!(matches!(
            pending.request,
            ServerRequest::RequestUserInput(_)
        ));
        assert_eq!(result, serde_json::json!({ "answer": "yes" }));
    }

    #[test]
    fn json_rpc_adapter_rejects_unknown_or_non_response_frames() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
        let mut connection = adapter.connect();

        assert!(matches!(
            connection.complete_server_request_response(JsonRpcFrame::Success(
                JsonRpcSuccess::new(
                    JsonRpcId::String("missing".to_string()),
                    serde_json::json!(true),
                )
            )),
            Err(JsonRpcAdapterError::UnknownResponseId { .. })
        ));
        assert!(matches!(
            connection.complete_server_request_response(JsonRpcFrame::Notification(
                JsonRpcNotification::new("session/event", None),
            )),
            Err(JsonRpcAdapterError::UnexpectedResponseFrame { .. })
        ));
    }

    #[tokio::test]
    async fn json_rpc_adapter_dispatches_client_request_to_handler() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
        let connection = adapter.connect();
        let handler = RecordingHandler::default();

        let response = connection
            .dispatch_client_request(
                JsonRpcRequest::new(
                    JsonRpcId::String("req-1".to_string()),
                    "turn/interrupt",
                    None,
                ),
                &handler,
            )
            .await;

        assert_eq!(handler.methods(), vec![ClientRequestMethod::TurnInterrupt]);
        assert_eq!(
            response,
            JsonRpcFrame::Success(JsonRpcSuccess::new(
                JsonRpcId::String("req-1".to_string()),
                serde_json::json!({ "ok": true }),
            ))
        );
    }

    #[tokio::test]
    async fn json_rpc_adapter_accepts_unit_request_with_empty_params() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
        let connection = adapter.connect();
        let handler = RecordingHandler::default();

        let response = connection
            .dispatch_client_request(
                JsonRpcRequest::new(
                    JsonRpcId::String("req-1".to_string()),
                    "control/keepAlive",
                    Some(serde_json::json!({})),
                ),
                &handler,
            )
            .await;

        assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
        assert!(matches!(response, JsonRpcFrame::Success(_)));
    }

    #[test]
    fn json_rpc_adapter_resolves_server_request_response_through_app_server() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let mut connection = adapter.connect();
        let session_id = test_session_id("sess-1");
        let surface_id = SurfaceId::from("surface-1");
        server
            .attach_surface_with_options(
                connection.connection_key(),
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach remote interactive surface");
        let routed = server
            .route_server_request(
                session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-1")),
                test_server_request(),
            )
            .expect("route server request");
        let delivery = connection
            .server_requests_mut()
            .try_recv()
            .expect("request delivery");
        let JsonRpcFrame::Request(request) = connection
            .encode_server_request(delivery)
            .expect("encode server request")
        else {
            panic!("expected request frame");
        };

        let resolved = connection
            .resolve_server_request_response(JsonRpcFrame::Success(JsonRpcSuccess::new(
                request.id,
                serde_json::json!({ "answer": "yes" }),
            )))
            .expect("resolve server request response");

        assert_eq!(resolved.pending, routed.pending);
        let ServerRequestReply::UserInput(params) = resolved.reply else {
            panic!("expected user input reply");
        };
        assert_eq!(params.request_id, "payload-request-id");
        assert_eq!(params.answer, "yes");
        let routing = server.routing().read().expect("routing lock");
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn json_rpc_owner_task_disconnects_app_server_on_transport_eof() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let connection = adapter.connect();
        let connection_key = connection.connection_key();
        let surface_id = SurfaceId::from("surface-1");
        server
            .attach_surface_with_options(
                connection_key,
                surface_id.clone(),
                test_session_id("sess-1"),
                AttachSurfaceOptions::default(),
            )
            .expect("attach surface");
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (server_read, server_write) = split(server_stream);
        let transport = NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
        drop(client_stream);

        let outcome = connection
            .run_ndjson_transport(transport, Arc::new(RecordingHandler::default()))
            .await
            .expect("owner loop exits cleanly");

        assert_eq!(outcome.detached_surfaces, vec![surface_id]);
        assert_eq!(
            server
                .routing()
                .read()
                .expect("routing lock")
                .connection_surface_count(connection_key),
            0
        );
    }

    #[tokio::test]
    async fn json_rpc_frame_channel_owner_dispatches_request_and_disconnects_on_eof() {
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let connection = adapter.connect();
        let connection_key = connection.connection_key();
        let surface_id = SurfaceId::from("surface-1");
        server
            .attach_surface_with_options(
                connection_key,
                surface_id.clone(),
                test_session_id("sess-1"),
                AttachSurfaceOptions::default(),
            )
            .expect("attach surface");
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(8);
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(8);
        let handler = Arc::new(RecordingHandler::default());
        let owner = tokio::spawn(connection.run_frame_channels(
            inbound_rx,
            outbound_tx,
            Arc::clone(&handler),
        ));

        inbound_tx
            .send(JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::Number(7),
                "control/keepAlive",
                Some(serde_json::json!({})),
            )))
            .await
            .expect("send inbound request");
        let response = outbound_rx.recv().await.expect("outbound response");
        assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
        assert_eq!(
            response,
            JsonRpcFrame::Success(JsonRpcSuccess::new(
                JsonRpcId::Number(7),
                serde_json::json!({ "ok": true }),
            ))
        );

        drop(inbound_tx);
        let outcome = owner
            .await
            .expect("owner task")
            .expect("owner loop exits cleanly");
        assert_eq!(outcome.detached_surfaces, vec![surface_id]);
        assert_eq!(
            server
                .routing()
                .read()
                .expect("routing lock")
                .connection_surface_count(connection_key),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn json_rpc_adapter_accepts_unix_connection_and_dispatches_requests() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
            .expect("bind unix listener");
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let handler = Arc::new(RecordingHandler::default());
        let (owner_task, client) = tokio::join!(
            adapter.accept_unix_connection(&listener, Arc::clone(&handler)),
            coco_app_server_transport::connect_ndjson_unix(&socket_path)
        );
        let owner_task = owner_task.expect("accept unix connection");
        let mut client = client.expect("connect unix socket");
        client
            .send_frame(&JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::String("req-uds".to_string()),
                "control/keepAlive",
                None,
            )))
            .await
            .expect("client sends request");

        let Some(JsonRpcFrame::Success(response)) =
            client.recv_frame().await.expect("client reads response")
        else {
            panic!("expected success response");
        };
        assert_eq!(response.id, JsonRpcId::String("req-uds".to_string()));
        assert_eq!(response.result, serde_json::json!({ "ok": true }));

        drop(client);
        owner_task
            .await
            .expect("owner task")
            .expect("owner exits cleanly");
        assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn json_rpc_adapter_unix_listener_runs_until_shutdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
            .expect("bind unix listener");
        let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
        let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
        let handler = Arc::new(RecordingHandler::default());
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let listener_task = tokio::spawn({
            let adapter = adapter.clone();
            let handler = Arc::clone(&handler);
            async move {
                adapter
                    .run_unix_listener_until_shutdown(listener, handler, shutdown_rx)
                    .await
            }
        });

        let mut client = coco_app_server_transport::connect_ndjson_unix(&socket_path)
            .await
            .expect("connect unix socket");
        client
            .send_frame(&JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::String("req-listener".to_string()),
                "control/keepAlive",
                None,
            )))
            .await
            .expect("client sends request");

        let Some(JsonRpcFrame::Success(response)) =
            client.recv_frame().await.expect("client reads response")
        else {
            panic!("expected success response");
        };
        assert_eq!(response.id, JsonRpcId::String("req-listener".to_string()));
        assert_eq!(response.result, serde_json::json!({ "ok": true }));

        drop(client);
        shutdown_tx.send(()).expect("send shutdown");
        listener_task
            .await
            .expect("listener task")
            .expect("listener exits cleanly");
        assert_eq!(handler.methods(), vec![ClientRequestMethod::KeepAlive]);
    }
}
