use super::*;

#[test]
fn test_json_rpc_request_serialization() {
    let request = JsonRpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "test".to_string(),
        params: serde_json::json!({"key": "value"}),
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"method\":\"test\""));
}

#[test]
fn test_json_rpc_response_parsing() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"data":"test"}}"#;
    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, Some(1));
    assert!(response.result.is_some());
    assert!(response.error.is_none());
}

#[test]
fn test_json_rpc_error_parsing() {
    let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, Some(1));
    assert!(response.result.is_none());
    assert!(response.error.is_some());
    let err = response.error.unwrap();
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "Invalid Request");
}

#[test]
fn server_configuration_request_preserves_item_count() {
    let response = server_request_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "workspace/configuration",
        "params": { "items": [{"section":"rust"}, {"section":"editor"}] }
    }));

    assert_eq!(response["id"], 7);
    assert_eq!(response["result"], serde_json::json!([null, null]));
}

#[test]
fn unknown_server_request_gets_method_not_found() {
    let response = server_request_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": "server-1",
        "method": "custom/unsupported",
        "params": {}
    }));

    assert_eq!(response["id"], "server-1");
    assert_eq!(response["error"]["code"], -32601);
}

fn framed(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    format!("Content-Length: {}\r\n\r\n", body.len())
        .bytes()
        .chain(body)
        .collect()
}

async fn read_frame(reader: &mut tokio::io::DuplexStream) -> serde_json::Value {
    let mut reader = BufReader::new(reader);
    let mut header = String::new();
    reader.read_line(&mut header).await.unwrap();
    let length = header
        .trim()
        .strip_prefix("Content-Length: ")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut separator = String::new();
    reader.read_line(&mut separator).await.unwrap();
    assert_eq!(separator, "\r\n");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

type PendingRequestFixture = (
    Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
    oneshot::Receiver<Result<serde_json::Value>>,
);

fn pending_request() -> PendingRequestFixture {
    let (tx, rx) = oneshot::channel();
    let pending = Arc::new(Mutex::new(HashMap::from([(
        42,
        PendingRequest {
            tx,
            method: "test/pending".to_string(),
        },
    )])));
    (pending, rx)
}

async fn assert_pending_released(
    rx: oneshot::Receiver<Result<serde_json::Value>>,
    closed: &AtomicBool,
) {
    let result = timeout(Duration::from_secs(1), rx)
        .await
        .expect("pending request released")
        .expect("reader sent result");
    assert!(matches!(result, Err(LspErr::ConnectionClosed)));
    assert!(closed.load(Ordering::Acquire));
}

#[tokio::test]
async fn framed_server_request_is_answered_on_the_wire() {
    let (mut server_writer, client_reader) = tokio::io::duplex(4096);
    let (client_writer, mut server_reader) = tokio::io::duplex(4096);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let (notification_tx, _notification_rx) = mpsc::channel(1);
    let task = tokio::spawn(run_reader_task(
        BufReader::new(client_reader),
        pending,
        Arc::new(Mutex::new(client_writer)),
        notification_tx,
        shutdown_rx,
        Arc::clone(&closed),
    ));

    server_writer
        .write_all(&framed(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "workspace/configuration",
            "params": {"items": [{"section": "rust"}, {"section": "editor"}]}
        })))
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(1), read_frame(&mut server_reader))
        .await
        .expect("server response");
    assert_eq!(response["id"], 7);
    assert_eq!(response["result"], serde_json::json!([null, null]));

    drop(server_writer);
    task.await.unwrap();
    assert!(closed.load(Ordering::Acquire));
}

#[tokio::test]
async fn eof_releases_pending_requests() {
    let (server_writer, client_reader) = tokio::io::duplex(1024);
    let (client_writer, _server_reader) = tokio::io::duplex(1024);
    let (pending, rx) = pending_request();
    let closed = Arc::new(AtomicBool::new(false));
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let (notification_tx, _notification_rx) = mpsc::channel(1);
    let task = tokio::spawn(run_reader_task(
        BufReader::new(client_reader),
        pending,
        Arc::new(Mutex::new(client_writer)),
        notification_tx,
        shutdown_rx,
        Arc::clone(&closed),
    ));

    drop(server_writer);
    assert_pending_released(rx, &closed).await;
    task.await.unwrap();
}

async fn assert_framing_error_releases_pending(header: String) {
    let (mut server_writer, client_reader) = tokio::io::duplex(1024);
    let (client_writer, _server_reader) = tokio::io::duplex(1024);
    let (pending, rx) = pending_request();
    let closed = Arc::new(AtomicBool::new(false));
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let (notification_tx, _notification_rx) = mpsc::channel(1);
    let task = tokio::spawn(run_reader_task(
        BufReader::new(client_reader),
        pending,
        Arc::new(Mutex::new(client_writer)),
        notification_tx,
        shutdown_rx,
        Arc::clone(&closed),
    ));

    server_writer.write_all(header.as_bytes()).await.unwrap();
    assert_pending_released(rx, &closed).await;
    task.await.unwrap();
}

#[tokio::test]
async fn malformed_and_oversized_frames_release_pending_requests() {
    assert_framing_error_releases_pending("Content-Length: nope\r\n\r\n".to_string()).await;
    assert_framing_error_releases_pending(format!(
        "Content-Length: {}\r\n\r\n",
        MAX_CONTENT_LENGTH + 1
    ))
    .await;
}

#[tokio::test]
async fn notification_overflow_closes_connection_and_releases_pending_requests() {
    let (mut server_writer, client_reader) = tokio::io::duplex(4096);
    let (client_writer, _server_reader) = tokio::io::duplex(1024);
    let (pending, rx) = pending_request();
    let closed = Arc::new(AtomicBool::new(false));
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let (notification_tx, mut notification_rx) = mpsc::channel(1);
    notification_tx
        .send(("prefilled".to_string(), serde_json::Value::Null))
        .await
        .unwrap();
    let task = tokio::spawn(run_reader_task(
        BufReader::new(client_reader),
        pending,
        Arc::new(Mutex::new(client_writer)),
        notification_tx,
        shutdown_rx,
        Arc::clone(&closed),
    ));

    server_writer
        .write_all(&framed(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": {"message": "overflow"}
        })))
        .await
        .unwrap();
    assert_pending_released(rx, &closed).await;
    task.await.unwrap();
    assert_eq!(notification_rx.recv().await.unwrap().0, "prefilled");
}
