use std::path::PathBuf;

use super::*;

#[test]
fn test_bridge_ports_default() {
    let ports = BridgePorts::default();
    assert_eq!(ports.http_port, 3128);
    assert_eq!(ports.socks_port, 1080);
}

#[test]
fn test_inner_bridge_prefix_format() {
    // Construct a BridgeManager-like state manually for prefix generation
    let socat_path = PathBuf::from("/usr/bin/socat");
    let http_socket = PathBuf::from("/tmp/coco-http-test.sock");
    let socks_socket = PathBuf::from("/tmp/coco-socks-test.sock");
    let ports = BridgePorts::default();

    let prefix = format!(
        "{socat} TCP-LISTEN:{http_port},fork,reuseaddr UNIX-CONNECT:{http_sock} & \
         {socat} TCP-LISTEN:{socks_port},fork,reuseaddr UNIX-CONNECT:{socks_sock} & \
         trap 'kill %1 %2 2>/dev/null; exit' EXIT; ",
        socat = socat_path.display(),
        http_port = ports.http_port,
        http_sock = http_socket.display(),
        socks_port = ports.socks_port,
        socks_sock = socks_socket.display(),
    );

    assert!(prefix.contains("TCP-LISTEN:3128"));
    assert!(prefix.contains("TCP-LISTEN:1080"));
    assert!(prefix.contains("UNIX-CONNECT:/tmp/coco-http-test.sock"));
    assert!(prefix.contains("UNIX-CONNECT:/tmp/coco-socks-test.sock"));
    assert!(prefix.contains("trap"));
}

#[test]
fn test_socket_path_uses_session_id() {
    let dir = std::env::temp_dir();
    let http = dir.join("coco-http-abc123.sock");
    let socks = dir.join("coco-socks-abc123.sock");

    assert!(http.to_str().unwrap().contains("coco-http-abc123"));
    assert!(socks.to_str().unwrap().contains("coco-socks-abc123"));
}
