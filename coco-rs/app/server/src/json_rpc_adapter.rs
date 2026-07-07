use std::collections::HashMap;
use std::sync::Arc;

use coco_app_server_transport::JsonRpcErrorObject;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcRequest;
use coco_types::RequestId;
use coco_types::ServerRequest;
use coco_types::SurfaceId;
use snafu::ResultExt;
use snafu::Snafu;

use crate::AppServer;
use crate::ConnectionKey;
use crate::DisconnectOutcome;
use crate::ServerRequestDelivery;
use crate::SurfaceDelivery;
use crate::SurfaceLifecycleDelivery;

const DEFAULT_JSON_RPC_CHANNEL_CAPACITY: usize = 128;

/// JSON-RPC adapter foundation for remote transports.
///
/// This skeleton registers a real AppServer connection and owns
/// transport-level server-request correlation. Runtime-backed method dispatch
/// and typed client request handling land in later Phase A slices.
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

    pub fn disconnect(self) -> DisconnectOutcome {
        self.server.disconnect(self.connection)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJsonRpcServerRequest {
    pub surface_id: SurfaceId,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq)]
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
    #[snafu(display("unexpected JSON-RPC response frame: {frame:?}"))]
    UnexpectedResponseFrame { frame: JsonRpcFrame },
    #[snafu(display("unknown JSON-RPC response id: {id:?}"))]
    UnknownResponseId { id: JsonRpcId },
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use coco_app_server_transport::JsonRpcNotification;
    use coco_app_server_transport::JsonRpcSuccess;
    use coco_types::ServerRequest;
    use coco_types::ServerRequestUserInputParams;
    use coco_types::SessionId;
    use coco_types::SurfaceId;
    use coco_types::TurnId;

    use super::*;
    use crate::AppServer;
    use crate::AttachSurfaceOptions;
    use crate::SurfaceCapabilities;
    use crate::SurfaceCapability;
    use crate::SurfaceRole;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestHandle(&'static str);

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
        assert_eq!(
            response,
            JsonRpcServerRequestResponse::Success {
                pending: PendingJsonRpcServerRequest {
                    surface_id,
                    request_id: routed.pending.request_id,
                },
                result: serde_json::json!({ "answer": "yes" }),
            }
        );
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
}
