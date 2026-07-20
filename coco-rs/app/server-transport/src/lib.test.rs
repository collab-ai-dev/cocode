use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::io::split;

use super::*;

#[test]
fn request_round_trips_with_string_id_and_params() {
    let frame = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-1".to_string()),
        "session/subscribe",
        Some(json!({ "sessionId": "sess-1", "afterSeq": 4 })),
    ));

    let encoded = serde_json::to_value(&frame).expect("encode frame");
    let decoded: JsonRpcFrame = serde_json::from_value(encoded.clone()).expect("decode frame");

    assert_eq!(decoded, frame);
    assert_eq!(
        encoded,
        json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "session/subscribe",
            "params": { "sessionId": "sess-1", "afterSeq": 4 }
        })
    );
}

#[test]
fn notification_has_no_id() {
    let frame = JsonRpcFrame::Notification(JsonRpcNotification::new(
        "session/event",
        Some(json!({ "sessionId": "session-1" })),
    ));

    let encoded = serde_json::to_value(&frame).expect("encode frame");

    assert_eq!(
        encoded,
        json!({
            "jsonrpc": "2.0",
            "method": "session/event",
            "params": { "sessionId": "session-1" }
        })
    );
}

#[test]
fn response_frames_preserve_number_and_null_ids() {
    let success = JsonRpcFrame::Success(JsonRpcSuccess::new(JsonRpcId::Number(7), json!(true)));
    let error = JsonRpcFrame::Error(JsonRpcErrorResponse::new(
        JsonRpcId::Null,
        JsonRpcErrorObject::new(-32600, "invalid request", Some(json!({ "field": "id" }))),
    ));

    assert_eq!(
        serde_json::from_value::<JsonRpcFrame>(
            serde_json::to_value(&success).expect("encode success")
        )
        .expect("decode success"),
        success
    );
    assert_eq!(
        serde_json::from_value::<JsonRpcFrame>(serde_json::to_value(&error).expect("encode error"))
            .expect("decode error"),
        error
    );
}

#[test]
fn invalid_jsonrpc_version_is_rejected() {
    let err = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "1.0",
        "id": "req-1",
        "method": "session/read"
    }))
    .expect_err("invalid version should fail");

    assert!(err.to_string().contains("unsupported jsonrpc version"));
}

#[test]
fn ndjson_encode_appends_newline_and_escapes_inner_newlines() {
    let frame = JsonRpcFrame::Notification(JsonRpcNotification::new(
        "session/event",
        Some(json!({ "message": "hello\nworld" })),
    ));

    let encoded = encode_ndjson_frame(&frame).expect("encode ndjson");

    assert!(encoded.ends_with(b"\n"));
    assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(
        decode_ndjson_frame(&encoded).expect("decode encoded frame"),
        frame
    );
}

#[test]
fn ndjson_decode_accepts_lf_and_crlf_records() {
    let lf = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\n";
    let crlf = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\r\n";

    let expected = JsonRpcFrame::Success(JsonRpcSuccess::new(JsonRpcId::Number(7), json!(true)));

    assert_eq!(decode_ndjson_frame(lf).expect("decode lf"), expected);
    assert_eq!(decode_ndjson_frame(crlf).expect("decode crlf"), expected);
}

#[test]
fn ndjson_decode_rejects_empty_records() {
    assert!(matches!(
        decode_ndjson_frame(b"\n"),
        Err(TransportFrameError::EmptyFrame)
    ));
    assert!(matches!(
        decode_ndjson_frame(b"\r\n"),
        Err(TransportFrameError::EmptyFrame)
    ));
}

#[test]
fn ndjson_decode_rejects_oversized_records_before_parsing() {
    let oversized = br#"{"jsonrpc":"2.0","id":7,"result":true}"#;
    let err =
        decode_ndjson_frame_with_limit(oversized, 8).expect_err("oversized frame should fail");

    match err {
        TransportFrameError::FrameTooLarge { actual, max } => {
            assert_eq!(actual, oversized.len());
            assert_eq!(max, 8);
        }
        other => panic!("expected FrameTooLarge, got {other:?}"),
    }
}

#[test]
fn ndjson_decode_reports_malformed_json() {
    let err = decode_ndjson_frame(b"{not-json}\n").expect_err("malformed json should fail");

    assert!(matches!(err, TransportFrameError::Decode { .. }));
}

#[tokio::test]
async fn ndjson_reader_returns_frames_until_clean_eof() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"method\":\"session/read\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"session/event\"}\n"
    );
    let mut reader = NdjsonFrameReader::new(BufReader::new(input.as_bytes()));

    assert_eq!(
        reader.read_frame().await.expect("read request"),
        Some(JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-1".to_string()),
            "session/read",
            None
        )))
    );
    assert_eq!(
        reader.read_frame().await.expect("read notification"),
        Some(JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/event",
            None
        )))
    );
    assert_eq!(reader.read_frame().await.expect("read eof"), None);
}

#[tokio::test]
async fn ndjson_reader_accepts_final_record_without_newline() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}";
    let mut reader = NdjsonFrameReader::new(BufReader::new(&input[..]));

    assert_eq!(
        reader.read_frame().await.expect("read partial eof"),
        Some(JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::Number(7),
            json!(true)
        )))
    );
    assert_eq!(reader.read_frame().await.expect("read eof"), None);
}

#[tokio::test]
async fn ndjson_reader_rejects_oversized_record_before_decode() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\n";
    let mut reader = NdjsonFrameReader::with_max_frame_bytes(BufReader::new(&input[..]), 8);

    let err = reader
        .read_frame()
        .await
        .expect_err("oversized frame should fail");

    match err {
        TransportFrameError::FrameTooLarge { actual, max } => {
            assert_eq!(actual, input.len());
            assert_eq!(max, 8);
        }
        other => panic!("expected FrameTooLarge, got {other:?}"),
    }
}

#[tokio::test]
async fn ndjson_writer_serializes_one_frame_per_line() {
    let (client, server) = tokio::io::duplex(1024);
    let frame = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-1".to_string()),
        "session/read",
        Some(json!({ "sessionId": "sess-1" })),
    ));
    let mut writer = NdjsonFrameWriter::new(client);

    writer.write_frame(&frame).await.expect("write frame");
    drop(writer);

    let mut bytes = Vec::new();
    BufReader::new(server)
        .read_to_end(&mut bytes)
        .await
        .expect("read written bytes");

    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(
        decode_ndjson_frame(&bytes).expect("decode written bytes"),
        frame
    );
}

#[tokio::test]
async fn ndjson_duplex_connection_sends_and_receives_frames() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_read, client_write) = split(client_stream);
    let (server_read, server_write) = split(server_stream);
    let mut client = NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
    let mut server = NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
    let request = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-1".to_string()),
        "session/read",
        Some(json!({ "sessionId": "sess-1" })),
    ));
    let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
        JsonRpcId::String("req-1".to_string()),
        json!({ "ok": true }),
    ));

    client
        .send_frame(&request)
        .await
        .expect("client sends request");
    assert_eq!(
        server.recv_frame().await.expect("server reads request"),
        Some(request)
    );

    server
        .send_frame(&response)
        .await
        .expect("server sends response");
    assert_eq!(
        client.recv_frame().await.expect("client reads response"),
        Some(response)
    );
}

#[tokio::test]
async fn ndjson_duplex_connection_close_blocks_future_sends_and_receives() {
    let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"session/event\"}\n";
    let mut connection = NdjsonDuplexConnection::new(BufReader::new(&input[..]), tokio::io::sink());

    connection.close().await.expect("close connection");

    assert!(!connection.is_open());
    assert!(matches!(
        connection
            .send_frame(&JsonRpcFrame::Notification(JsonRpcNotification::new(
                "session/event",
                None
            )))
            .await,
        Err(TransportFrameError::Closed)
    ));
    assert!(matches!(
        connection.recv_frame().await,
        Err(TransportFrameError::Closed)
    ));
}

#[tokio::test]
async fn ndjson_duplex_connection_clean_eof_marks_closed() {
    let mut connection = NdjsonDuplexConnection::new(BufReader::new(&b""[..]), tokio::io::sink());

    assert_eq!(connection.recv_frame().await.expect("read eof"), None);
    assert!(!connection.is_open());
    assert!(matches!(
        connection
            .send_frame(&JsonRpcFrame::Notification(JsonRpcNotification::new(
                "session/event",
                None
            )))
            .await,
        Err(TransportFrameError::Closed)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_connection_sends_and_receives_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind unix listener");
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept unix stream");
        ndjson_unix_connection(stream)
    });

    let mut client = connect_ndjson_unix(&socket_path)
        .await
        .expect("connect unix stream");
    let mut server = server_task.await.expect("server task");
    let request = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-uds".to_string()),
        "control/keepAlive",
        Some(json!({})),
    ));
    let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
        JsonRpcId::String("req-uds".to_string()),
        json!({ "ok": true }),
    ));

    client
        .send_frame(&request)
        .await
        .expect("client sends request");
    assert_eq!(
        server.recv_frame().await.expect("server reads request"),
        Some(request)
    );

    server
        .send_frame(&response)
        .await
        .expect("server sends response");
    assert_eq!(
        client.recv_frame().await.expect("client reads response"),
        Some(response)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_accepts_framed_connections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
    let server_task =
        tokio::spawn(async move { listener.accept().await.expect("accept unix stream") });

    let mut client = connect_ndjson_unix(&socket_path)
        .await
        .expect("connect unix stream");
    let mut server = server_task.await.expect("server task");
    let request = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-listener".to_string()),
        "control/keepAlive",
        Some(json!({})),
    ));
    let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
        JsonRpcId::String("req-listener".to_string()),
        json!({ "ok": true }),
    ));

    client
        .send_frame(&request)
        .await
        .expect("client sends request");
    assert_eq!(
        server.recv_frame().await.expect("server reads request"),
        Some(request)
    );

    server
        .send_frame(&response)
        .await
        .expect("server sends response");
    assert_eq!(
        client.recv_frame().await.expect("client reads response"),
        Some(response)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_removes_socket_path_on_drop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    {
        let _listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
        assert!(socket_path.exists());
    }

    assert!(
        !socket_path.exists(),
        "dropping the listener should remove its socket file"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_into_inner_transfers_socket_cleanup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");
    let listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
    let inner = listener.into_inner();
    drop(inner);

    assert!(
        socket_path.exists(),
        "into_inner hands socket lifecycle to the caller"
    );
    std::fs::remove_file(&socket_path).expect("cleanup socket");
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_bind_reclaims_stale_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");

    // A crashed process leaves the socket file behind with nothing listening.
    let stale = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind stale socket");
    drop(stale);
    assert!(socket_path.exists(), "stale socket file should linger");

    // Bind must probe liveness (connect refused), unlink the stale file, and
    // rebind rather than failing with AddrInUse.
    let listener = bind_ndjson_unix_listener(&socket_path).expect("reclaim stale socket");
    let mut client = connect_ndjson_unix(&socket_path)
        .await
        .expect("connect to reclaimed socket");
    let request = JsonRpcFrame::Request(JsonRpcRequest::new(
        JsonRpcId::String("req-stale".to_string()),
        "control/keepAlive",
        Some(json!({})),
    ));
    client.send_frame(&request).await.expect("client sends");
    let mut server = listener.accept().await.expect("accept on reclaimed socket");
    assert_eq!(
        server.recv_frame().await.expect("server reads"),
        Some(request)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_bind_refuses_live_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");

    // A live server is accepting on the path; connect succeeds, so bind must
    // surface AddrInUse rather than unlinking a live socket.
    let _live = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind live socket");

    let result = bind_ndjson_unix_listener(&socket_path);
    assert!(
        matches!(result, Err(TransportFrameError::Io { .. })),
        "binding over a live socket must fail, not reclaim it"
    );
    assert!(socket_path.exists(), "the live socket must survive");
}

#[cfg(unix)]
#[tokio::test]
async fn ndjson_unix_listener_drop_leaves_successor_socket_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("app-server.sock");

    let listener = bind_ndjson_unix_listener(&socket_path).expect("bind first listener");

    // Simulate a successor that re-created the path with a fresh inode. The
    // first listener still holds the old inode open, so the successor is
    // guaranteed a distinct `(dev, ino)`.
    std::fs::remove_file(&socket_path).expect("unlink first socket");
    let successor =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("bind successor socket");

    // Dropping the first listener must not delete the successor's socket.
    drop(listener);
    assert!(
        socket_path.exists(),
        "listener must not delete a successor's socket at the same path"
    );
    drop(successor);
}
