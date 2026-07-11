use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use coco_app_server::AppServer;
use coco_app_server::JsonRpcAdapter;
use coco_app_server::JsonRpcRequestContext;
use coco_app_server::JsonRpcRequestFuture;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server_client::RemoteConnectOptions;
use coco_app_server_client::RemoteJsonRpcClient;
use coco_app_server_client::RemoteJsonRpcEvent;
use coco_types::ClientRequest;

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
    assert!(outcome.detached_surfaces.is_empty());
    assert!(matches!(
        events.recv().await.expect("disconnect event"),
        RemoteJsonRpcEvent::Disconnected
    ));
}
