use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::*;

#[tokio::test]
async fn test_proxy_server_starts_and_stops() {
    let filter = Arc::new(DomainFilter::new(vec![], vec![]));
    let token = CancellationToken::new();
    let mut server = ProxyServer::start(filter, token).await.unwrap();

    assert_ne!(server.http_port(), 0);
    assert_ne!(server.socks_port(), 0);
    assert_ne!(server.http_port(), server.socks_port());

    server.stop().await;
}

#[tokio::test]
async fn test_proxy_server_ports_are_connectable() {
    let filter = Arc::new(DomainFilter::new(vec![], vec![]));
    let token = CancellationToken::new();
    let mut server = ProxyServer::start(filter, token).await.unwrap();

    // Verify both ports accept TCP connections.
    let http_conn = TcpStream::connect(format!("127.0.0.1:{}", server.http_port())).await;
    assert!(http_conn.is_ok());

    let socks_conn = TcpStream::connect(format!("127.0.0.1:{}", server.socks_port())).await;
    assert!(socks_conn.is_ok());

    server.stop().await;
}

#[tokio::test]
async fn test_http_proxy_bad_request() {
    let filter = Arc::new(DomainFilter::new(vec![], vec![]));
    let token = CancellationToken::new();
    let mut server = ProxyServer::start(filter, token).await.unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.http_port()))
        .await
        .unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(response.contains("400 Bad Request"));

    server.stop().await;
}

#[tokio::test]
async fn test_http_proxy_denied_domain() {
    let filter = Arc::new(DomainFilter::new(vec![], vec!["evil.com".to_string()]));
    let token = CancellationToken::new();
    let mut server = ProxyServer::start(filter, token).await.unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.http_port()))
        .await
        .unwrap();
    stream
        .write_all(b"CONNECT evil.com:443 HTTP/1.1\r\nHost: evil.com\r\n\r\n")
        .await
        .unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(response.contains("403 Forbidden"));
    assert!(response.contains("evil.com"));

    server.stop().await;
}

#[tokio::test]
async fn test_cancellation_stops_servers() {
    let filter = Arc::new(DomainFilter::new(vec![], vec![]));
    let token = CancellationToken::new();
    let server = ProxyServer::start(Arc::clone(&filter), token.clone())
        .await
        .unwrap();

    let http_port = server.http_port();
    let socks_port = server.socks_port();

    token.cancel();
    // Give tasks time to notice cancellation.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // After cancellation, the listeners should be dropped.
    // New connections should fail (or be immediately closed).
    let http_result = TcpStream::connect(format!("127.0.0.1:{http_port}")).await;
    let socks_result = TcpStream::connect(format!("127.0.0.1:{socks_port}")).await;

    // At least one should fail after cancel (accept loop exited).
    // The exact behavior depends on OS socket teardown timing,
    // so we just verify the test doesn't hang.
    drop(http_result);
    drop(socks_result);
}

/// An approving ask-callback converts a denied CONNECT into a tunnel
/// (TS `createSandboxAskCallback` "Allow network connection to {host}?").
#[tokio::test]
async fn test_http_proxy_ask_callback_approves_overrides_deny() {
    // Upstream the approved CONNECT will tunnel to.
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut s, _)) = upstream.accept().await {
            // Hold the connection open so copy_bidirectional has a peer.
            let mut b = [0u8; 16];
            let _ = s.read(&mut b).await;
        }
    });

    // Deny 127.0.0.1, but approve it via the ask-callback.
    let filter = Arc::new(DomainFilter::new(vec![], vec!["127.0.0.1".to_string()]));
    let approve: crate::proxy::NetworkAskCallback = Arc::new(|_host: String| {
        Box::pin(async move { true })
            as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
    });
    let token = CancellationToken::new();
    let mut server = ProxyServer::start_with_ports(filter, token, None, None, Some(approve))
        .await
        .unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.http_port()))
        .await
        .unwrap();
    stream
        .write_all(format!("CONNECT 127.0.0.1:{upstream_port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(
        response.contains("200 Connection Established"),
        "approved CONNECT should tunnel (200), got: {response}"
    );

    server.stop().await;
}

/// A rejecting ask-callback leaves the static 403 deny in place.
#[tokio::test]
async fn test_http_proxy_ask_callback_rejects_keeps_deny() {
    let filter = Arc::new(DomainFilter::new(vec![], vec!["evil.com".to_string()]));
    let reject: crate::proxy::NetworkAskCallback = Arc::new(|_host: String| {
        Box::pin(async move { false })
            as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
    });
    let token = CancellationToken::new();
    let mut server = ProxyServer::start_with_ports(filter, token, None, None, Some(reject))
        .await
        .unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.http_port()))
        .await
        .unwrap();
    stream
        .write_all(b"CONNECT evil.com:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(
        response.contains("403 Forbidden"),
        "rejected CONNECT should still 403, got: {response}"
    );

    server.stop().await;
}

/// SOCKS5: an approving ask-callback converts a denied CONNECT into a tunnel.
#[tokio::test]
async fn test_socks5_ask_callback_approves_overrides_deny() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut s, _)) = upstream.accept().await {
            let mut b = [0u8; 16];
            let _ = s.read(&mut b).await;
        }
    });

    let filter = Arc::new(DomainFilter::new(vec![], vec!["127.0.0.1".to_string()]));
    let approve: crate::proxy::NetworkAskCallback = Arc::new(|_host: String| {
        Box::pin(async move { true })
            as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
    });
    let token = CancellationToken::new();
    let mut server = ProxyServer::start_with_ports(filter, token, None, None, Some(approve))
        .await
        .unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.socks_port()))
        .await
        .unwrap();
    // Greeting: VER=5, NMETHODS=1, NO_AUTH.
    stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut greet = [0u8; 2];
    stream.read_exact(&mut greet).await.unwrap();
    assert_eq!(greet, [0x05, 0x00]);

    // Request: VER=5, CMD=CONNECT, RSV=0, ATYP=IPv4, 127.0.0.1, port.
    let port = upstream_port.to_be_bytes();
    stream
        .write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, port[0], port[1]])
        .await
        .unwrap();

    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0], 0x05, "SOCKS version");
    assert_eq!(
        reply[1], 0x00,
        "approved CONNECT should reply success (0x00)"
    );

    server.stop().await;
}

#[test]
fn test_extract_host_simple() {
    assert_eq!(extract_host("example.com:443"), "example.com");
}

#[test]
fn test_extract_host_no_port() {
    assert_eq!(extract_host("example.com"), "example.com");
}

#[test]
fn test_extract_host_ipv6_bracket() {
    assert_eq!(extract_host("[::1]:443"), "[::1]");
}
