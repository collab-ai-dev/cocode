//! HTTP CONNECT and SOCKS5 proxy servers with domain filtering.

use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::NetworkAskCallback;
use super::filter::DomainFilter;

/// Combined HTTP CONNECT and SOCKS5 proxy server.
///
/// Both servers bind to random available ports and enforce domain-based
/// filtering via the shared `DomainFilter`. Shut down gracefully via
/// `stop()` or by cancelling the provided `CancellationToken`.
pub struct ProxyServer {
    http_port: u16,
    socks_port: u16,
    cancel_token: CancellationToken,
    http_handle: Option<JoinHandle<()>>,
    socks_handle: Option<JoinHandle<()>>,
}

impl ProxyServer {
    /// Start HTTP CONNECT and SOCKS5 proxy servers.
    ///
    /// When `fixed_http_port` or `fixed_socks_port` is `Some`, binds to that
    /// specific port (from `NetworkConfig.http_proxy_port`/`socks_proxy_port`).
    /// Otherwise binds to a random available port.
    pub async fn start(
        filter: Arc<DomainFilter>,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        Self::start_with_ports(filter, cancel_token, None, None, None).await
    }

    /// Start proxy servers with optional fixed ports.
    ///
    /// `ask_cb`, when `Some`, is consulted on a denied CONNECT before refusing —
    /// an approving callback (the user said yes) lets the connection through.
    pub async fn start_with_ports(
        filter: Arc<DomainFilter>,
        cancel_token: CancellationToken,
        fixed_http_port: Option<u16>,
        fixed_socks_port: Option<u16>,
        ask_cb: Option<NetworkAskCallback>,
    ) -> anyhow::Result<Self> {
        let http_addr = match fixed_http_port {
            Some(port) => format!("127.0.0.1:{port}"),
            None => "127.0.0.1:0".to_string(),
        };
        let socks_addr = match fixed_socks_port {
            Some(port) => format!("127.0.0.1:{port}"),
            None => "127.0.0.1:0".to_string(),
        };
        let http_listener = TcpListener::bind(&http_addr).await?;
        let socks_listener = TcpListener::bind(&socks_addr).await?;

        let http_port = http_listener.local_addr()?.port();
        let socks_port = socks_listener.local_addr()?.port();

        tracing::info!(http_port, socks_port, "Proxy servers starting");

        let http_handle = {
            let filter = Arc::clone(&filter);
            let token = cancel_token.clone();
            let ask_cb = ask_cb.clone();
            tokio::spawn(async move {
                run_http_proxy(http_listener, filter, token, ask_cb).await;
            })
        };

        let socks_handle = {
            let filter = Arc::clone(&filter);
            let token = cancel_token.clone();
            let ask_cb = ask_cb.clone();
            tokio::spawn(async move {
                run_socks_proxy(socks_listener, filter, token, ask_cb).await;
            })
        };

        Ok(Self {
            http_port,
            socks_port,
            cancel_token,
            http_handle: Some(http_handle),
            socks_handle: Some(socks_handle),
        })
    }

    /// HTTP proxy port.
    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    /// SOCKS5 proxy port.
    pub fn socks_port(&self) -> u16 {
        self.socks_port
    }

    /// Stop both proxy servers gracefully.
    pub async fn stop(&mut self) {
        self.cancel_token.cancel();
        if let Some(h) = self.http_handle.take() {
            let _ = h.await;
        }
        if let Some(h) = self.socks_handle.take() {
            let _ = h.await;
        }
        tracing::info!("Proxy servers stopped");
    }
}

impl Drop for ProxyServer {
    /// Cancel the accept loops when the owning `SandboxState` is dropped so the
    /// proxy tasks don't outlive the session. (`stop()` additionally awaits the
    /// handles; Drop can't await, but cancelling is enough to wind them down.)
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}

/// Accept loop for the HTTP CONNECT proxy.
async fn run_http_proxy(
    listener: TcpListener,
    filter: Arc<DomainFilter>,
    cancel_token: CancellationToken,
    ask_cb: Option<NetworkAskCallback>,
) {
    loop {
        tokio::select! {
            () = cancel_token.cancelled() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        tracing::debug!(%addr, "HTTP proxy: new connection");
                        let filter = Arc::clone(&filter);
                        let ask_cb = ask_cb.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_http_connect(stream, &filter, ask_cb.as_ref()).await {
                                tracing::debug!(error = %e, "HTTP proxy: connection error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "HTTP proxy: accept error");
                    }
                }
            }
        }
    }
}

/// Handle a single HTTP proxy request (CONNECT or plain HTTP).
///
/// Reads the request line, extracts the method and target host, checks
/// the domain filter and network mode, and either tunnels/proxies the
/// connection or responds with 403.
async fn handle_http_connect(
    mut client: TcpStream,
    filter: &DomainFilter,
    ask_cb: Option<&NetworkAskCallback>,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = client.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");

    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 3 {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }

    let method = parts[0];

    // In Limited mode, CONNECT is blocked because tunnels hide inner methods.
    if !filter.allows_method(method) {
        tracing::info!(
            method,
            "HTTP proxy: method blocked by network mode (limited)"
        );
        send_http_forbidden(
            &mut client,
            &format!(
                "Method '{method}' is not allowed in limited network mode. \
                 Only GET, HEAD, and OPTIONS are permitted."
            ),
        )
        .await?;
        return Ok(());
    }

    if !method.eq_ignore_ascii_case("CONNECT") {
        // Not a forward proxy — only CONNECT is supported.
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }

    let target = parts[1];
    let host = extract_host(target);

    if !filter.is_allowed(host) {
        // Denied by policy: consult the interactive ask-callback (if installed)
        // before refusing. An approving user converts the deny to a tunnel —
        // TS `createSandboxAskCallback` "Allow network connection to {host}?".
        let approved = match ask_cb {
            Some(cb) => cb(host.to_string()).await,
            None => false,
        };
        if !approved {
            tracing::info!(host, "HTTP proxy: domain denied");
            send_http_forbidden(
                &mut client,
                &format!("Domain '{host}' is not allowed by sandbox policy"),
            )
            .await?;
            return Ok(());
        }
        tracing::info!(host, "HTTP proxy: domain approved via interactive prompt");
    }

    let mut upstream = TcpStream::connect(target).await?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Send an HTTP 403 Forbidden response with a body message.
async fn send_http_forbidden(client: &mut TcpStream, body: &str) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes()).await?;
    Ok(())
}

/// Extract the host portion from a "host:port" string.
fn extract_host(host_port: &str) -> &str {
    // Handle IPv6 bracket notation: [::1]:443
    if let Some(bracket_end) = host_port.find(']') {
        &host_port[..=bracket_end]
    } else {
        host_port
            .rsplit_once(':')
            .map_or(host_port, |(host, _)| host)
    }
}

/// Accept loop for the SOCKS5 proxy.
async fn run_socks_proxy(
    listener: TcpListener,
    filter: Arc<DomainFilter>,
    cancel_token: CancellationToken,
    ask_cb: Option<NetworkAskCallback>,
) {
    loop {
        tokio::select! {
            () = cancel_token.cancelled() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        tracing::debug!(%addr, "SOCKS5 proxy: new connection");
                        let filter = Arc::clone(&filter);
                        let ask_cb = ask_cb.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_socks5(stream, &filter, ask_cb.as_ref()).await {
                                tracing::debug!(error = %e, "SOCKS5 proxy: connection error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "SOCKS5 proxy: accept error");
                    }
                }
            }
        }
    }
}

// SOCKS5 constants.
const SOCKS5_VERSION: u8 = 0x05;
const SOCKS5_AUTH_NONE: u8 = 0x00;
/// RFC 1928: no acceptable authentication methods (clean refusal).
const SOCKS5_AUTH_NO_ACCEPTABLE: u8 = 0xFF;
const SOCKS5_CMD_CONNECT: u8 = 0x01;
const SOCKS5_ATYP_IPV4: u8 = 0x01;
const SOCKS5_ATYP_DOMAIN: u8 = 0x03;
const SOCKS5_ATYP_IPV6: u8 = 0x04;
const SOCKS5_REPLY_SUCCESS: u8 = 0x00;
const SOCKS5_REPLY_REFUSED: u8 = 0x05;
const SOCKS5_REPLY_CMD_NOT_SUPPORTED: u8 = 0x07;

/// Handle a single SOCKS5 connection.
///
/// Negotiates NO AUTH, reads the CONNECT request, checks the domain filter,
/// and either tunnels or refuses the connection.
///
/// In Limited network mode, SOCKS5 is blocked entirely because HTTP methods
/// cannot be inspected through SOCKS tunnels.
async fn handle_socks5(
    mut client: TcpStream,
    filter: &DomainFilter,
    ask_cb: Option<&NetworkAskCallback>,
) -> anyhow::Result<()> {
    // Limited mode: block SOCKS5 entirely (can't inspect HTTP methods through tunnel).
    if filter.network_mode() == crate::config::NetworkMode::Limited {
        tracing::info!("SOCKS5 proxy: blocked by limited network mode");
        // Per RFC 1928, 0xFF = "no acceptable authentication methods" — cleanest refusal.
        let _ = client
            .write_all(&[SOCKS5_VERSION, SOCKS5_AUTH_NO_ACCEPTABLE])
            .await;
        return Ok(());
    }

    // --- Auth negotiation ---
    let version = client.read_u8().await?;
    if version != SOCKS5_VERSION {
        anyhow::bail!("unsupported SOCKS version: {version}");
    }

    let nmethods = client.read_u8().await?;
    let mut methods = vec![0u8; nmethods as usize];
    client.read_exact(&mut methods).await?;

    // Reply with NO AUTH (0x00).
    client
        .write_all(&[SOCKS5_VERSION, SOCKS5_AUTH_NONE])
        .await?;

    // --- Request ---
    let ver = client.read_u8().await?;
    if ver != SOCKS5_VERSION {
        anyhow::bail!("unexpected SOCKS version in request: {ver}");
    }

    let cmd = client.read_u8().await?;
    let _rsv = client.read_u8().await?;
    let atyp = client.read_u8().await?;

    if cmd != SOCKS5_CMD_CONNECT {
        send_socks5_reply(&mut client, SOCKS5_REPLY_CMD_NOT_SUPPORTED).await?;
        anyhow::bail!("unsupported SOCKS command: {cmd}");
    }

    let (domain, target_addr) = match atyp {
        SOCKS5_ATYP_IPV4 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            let port = client.read_u16().await?;
            let addr = format!("{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
            let domain = format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
            (domain, addr)
        }
        SOCKS5_ATYP_DOMAIN => {
            let len = client.read_u8().await? as usize;
            let mut domain_buf = vec![0u8; len];
            client.read_exact(&mut domain_buf).await?;
            let domain = String::from_utf8(domain_buf)?;
            let port = client.read_u16().await?;
            let addr = format!("{domain}:{port}");
            (domain, addr)
        }
        SOCKS5_ATYP_IPV6 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            let port = client.read_u16().await?;
            let segments: Vec<String> = ip
                .chunks(2)
                .map(|c| format!("{:02x}{:02x}", c[0], c[1]))
                .collect();
            let ipv6 = segments.join(":");
            let addr = format!("[{ipv6}]:{port}");
            (ipv6, addr)
        }
        _ => {
            send_socks5_reply(&mut client, SOCKS5_REPLY_REFUSED).await?;
            anyhow::bail!("unsupported SOCKS address type: {atyp}");
        }
    };

    if !filter.is_allowed(&domain) {
        // Denied by policy: consult the interactive ask-callback (if installed)
        // before refusing — an approving user converts the deny to a tunnel.
        let approved = match ask_cb {
            Some(cb) => cb(domain.clone()).await,
            None => false,
        };
        if !approved {
            tracing::info!(domain, "SOCKS5 proxy: domain denied");
            send_socks5_reply(&mut client, SOCKS5_REPLY_REFUSED).await?;
            return Ok(());
        }
        tracing::info!(
            domain,
            "SOCKS5 proxy: domain approved via interactive prompt"
        );
    }

    let mut upstream = TcpStream::connect(&target_addr).await?;
    send_socks5_reply(&mut client, SOCKS5_REPLY_SUCCESS).await?;

    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Send a SOCKS5 reply with the given status code.
///
/// Uses a minimal reply with a bound address of 0.0.0.0:0.
async fn send_socks5_reply(client: &mut TcpStream, status: u8) -> anyhow::Result<()> {
    // VER | REP | RSV | ATYP(IPv4) | BND.ADDR(0.0.0.0) | BND.PORT(0)
    let reply = [
        SOCKS5_VERSION,
        status,
        0x00,
        SOCKS5_ATYP_IPV4,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    client.write_all(&reply).await?;
    Ok(())
}

#[cfg(test)]
#[path = "server.test.rs"]
mod tests;
