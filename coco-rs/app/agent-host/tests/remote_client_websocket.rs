use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use coco_app_server::AppServer;
use coco_app_server::AttachSessionOptions;
use coco_app_server::JsonRpcAdapter;
use coco_app_server::JsonRpcRequestContext;
use coco_app_server::JsonRpcRequestFuture;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientInbound;
use coco_app_server::ServerRequestReply;
use coco_app_server_client::RemoteConnectOptions;
use coco_app_server_client::RemoteJsonRpcClient;
use coco_app_server_client::RemoteJsonRpcEvent;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::ServerRequest;
use coco_types::ServerRequestUserInputParams;
use coco_types::SessionAccess;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionState;
use coco_types::SessionTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

struct RecordingJsonRpcRequestHandler {
    calls: Arc<Mutex<Vec<String>>>,
}

impl RecordingJsonRpcRequestHandler {
    fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
        Self { calls }
    }
}

impl JsonRpcRequestHandler for RecordingJsonRpcRequestHandler {
    fn handle_json_rpc_request(
        &self,
        _context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.method().as_str().to_string());
            Ok(serde_json::json!({ "ok": true }))
        })
    }
}

struct SharedSessionHandler {
    server: Arc<AppServer<TestHandle>>,
    session_id: SessionId,
    mutations: Arc<Mutex<usize>>,
}

impl JsonRpcRequestHandler for SharedSessionHandler {
    fn handle_json_rpc_request(
        &self,
        context: JsonRpcRequestContext,
        request: ClientRequest,
    ) -> JsonRpcRequestFuture {
        let server = Arc::clone(&self.server);
        let session_id = self.session_id.clone();
        let mutations = Arc::clone(&self.mutations);
        Box::pin(async move {
            match request {
                ClientRequest::KeepAlive => {
                    server
                        .attach_live_session(
                            context.connection,
                            session_id,
                            AttachSessionOptions::full(),
                        )
                        .map_err(|error| {
                            coco_app_server::JsonRpcDispatchError::invalid_params(error.to_string())
                        })?;
                    Ok(serde_json::Value::Null)
                }
                ClientRequest::TurnInterrupt(target) => {
                    server
                        .validate_session_target(context.connection, &target, SessionAccess::Full)
                        .map_err(|error| {
                            coco_app_server::JsonRpcDispatchError::invalid_params(error.to_string())
                        })?;
                    *mutations.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                    Ok(serde_json::Value::Null)
                }
                other => Err(coco_app_server::JsonRpcDispatchError::method_not_found(
                    other.method().as_str(),
                )),
            }
        })
    }
}

#[tokio::test]
async fn remote_json_rpc_client_connects_to_app_server_over_websocket() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind websocket listener");
    let addr = listener.local_addr().expect("listener addr");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(RecordingJsonRpcRequestHandler::new(Arc::clone(&calls)));
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket tcp");
        let websocket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket");
        adapter
            .connect()
            .run_websocket_transport(websocket, handler)
            .await
            .expect("websocket owner exits")
    });

    let (client, connection, mut events) = RemoteJsonRpcClient::connect_websocket_with_options(
        &format!("ws://{addr}"),
        RemoteConnectOptions {
            outbound_channel_capacity: 8,
            event_channel_capacity: 8,
            request_timeout: None,
            write_timeout: None,
        },
    )
    .await
    .expect("connect websocket");
    let connection_task = tokio::spawn(connection.run());
    let request_client = client.clone();
    let request_task =
        tokio::spawn(async move { request_client.request("control/keepAlive", None).await });

    assert_eq!(
        request_task
            .await
            .expect("request task")
            .expect("request success"),
        serde_json::json!({ "ok": true })
    );
    assert_eq!(
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        ["control/keepAlive"]
    );

    drop(client);
    connection_task
        .await
        .expect("connection task")
        .expect("connection exits cleanly");
    let outcome = server_task.await.expect("server task");
    assert!(outcome.detached_sessions.is_empty());
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
}

#[tokio::test]
async fn local_and_websocket_full_connections_share_one_session_independently() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = SessionId::try_new("shared-local-web-session").expect("session id");
    assert!(matches!(
        server.registry().begin_load(session_id.clone()),
        Ok(coco_app_server::LoadStart::Reserved)
    ));
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("shared"))
        .expect("activate session");

    let local_adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut local = local_adapter.connect();
    local
        .handle()
        .attach_session(session_id.clone(), AttachSessionOptions::full())
        .expect("attach local connection");

    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind websocket listener");
    let addr = listener.local_addr().expect("listener addr");
    let mutations = Arc::new(Mutex::new(0));
    let handler = Arc::new(SharedSessionHandler {
        server: Arc::clone(&server),
        session_id: session_id.clone(),
        mutations: Arc::clone(&mutations),
    });
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket tcp");
        let websocket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket");
        adapter
            .connect()
            .run_websocket_transport(websocket, handler)
            .await
            .expect("websocket owner exits")
    });
    let (web, web_connection, mut web_events) =
        RemoteJsonRpcClient::connect_websocket_with_options(
            &format!("ws://{addr}"),
            RemoteConnectOptions {
                outbound_channel_capacity: 8,
                event_channel_capacity: 8,
                request_timeout: Some(std::time::Duration::from_secs(2)),
                write_timeout: None,
            },
        )
        .await
        .expect("connect web client");
    let web_connection_task = tokio::spawn(web_connection.run());
    web.keep_alive().await.expect("attach web connection");

    assert_eq!(server.list_live_sessions()[0].connection_counts.full, 2);
    server.route_envelope(SessionEnvelope::durable(
        session_id.clone(),
        None,
        None,
        1,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    ));
    assert!(matches!(
        local.recv().await,
        Some(LocalClientInbound::Event(delivery)) if delivery.envelope.session_id == session_id
    ));
    assert!(matches!(
        web_events.recv().await,
        Some(RemoteJsonRpcEvent::SessionDelivery(delivery))
            if delivery.envelope.session_id == session_id
    ));

    web.session_handle(session_id.clone())
        .interrupt()
        .await
        .expect("web full connection mutates session");
    assert_eq!(*mutations.lock().unwrap_or_else(PoisonError::into_inner), 1);

    let pending = server
        .route_server_request_with_reply(
            session_id.clone(),
            None,
            ServerRequest::RequestUserInput(ServerRequestUserInputParams {
                request_id: "shared-input".to_string(),
                prompt: "continue?".to_string(),
                description: None,
                choices: Vec::new(),
                default: None,
            }),
        )
        .expect("broadcast input request");
    let local_request = loop {
        if let Some(LocalClientInbound::ServerRequest(delivery)) = local.recv().await {
            break *delivery;
        }
    };
    let web_request = loop {
        if let Some(RemoteJsonRpcEvent::ServerRequest(request)) = web_events.recv().await {
            break request;
        }
    };
    web.reply_server_request_success(web_request.id, serde_json::json!({ "answer": "web won" }))
        .await
        .expect("web response");
    assert!(matches!(
        pending.await.expect("first response wins"),
        ServerRequestReply::UserInput(params) if params.answer == "web won"
    ));
    assert!(matches!(
        server.resolve_server_request(
            local.connection_key(),
            &SessionTarget {
                session_id: session_id.clone(),
            },
            ServerRequestReply::UserInput(coco_types::UserInputResolveParams {
                target: SessionTarget {
                    session_id: session_id.clone(),
                },
                request_id: local_request.request_id.as_display(),
                answer: "local was late".to_string(),
            }),
        ),
        Err(coco_app_server::AppServerError::ServerRequestNotFound { .. })
    ));

    drop(web);
    web_connection_task
        .await
        .expect("web connection task")
        .expect("web connection exits");
    server_task.await.expect("server task");
    assert_eq!(server.list_live_sessions()[0].connection_counts.full, 1);
    assert_eq!(
        server
            .route_envelope(SessionEnvelope::durable(
                session_id,
                None,
                None,
                2,
                CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                    state: SessionState::Running,
                }),
            ))
            .delivered,
        1
    );
}
