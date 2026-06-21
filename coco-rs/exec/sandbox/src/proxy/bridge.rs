//! Unix domain socket bridge for proxy across Linux network namespaces.
//!
//! When bubblewrap uses `--unshare-net`, the sandboxed process cannot reach
//! `localhost` on the host. This module creates UDS-based bridges so the
//! proxy (running outside) is reachable from inside the sandbox.
//!
//! Architecture:
//! ```text
//! Host side:                    socat UNIX-LISTEN:<socket>,fork TCP:localhost:<proxy_port>
//! bwrap bind-mounts:            --bind <socket> <socket>
//! Inside sandbox (wrapper):     socat TCP-LISTEN:<port>,fork UNIX-CONNECT:<socket> &
//! Sandboxed command:            HTTP_PROXY=http://localhost:<port>
//! ```

use std::path::Path;
use std::path::PathBuf;

use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Manages socat bridge processes for proxy across network namespace.
pub struct BridgeManager {
    /// UDS path for the HTTP proxy bridge.
    http_socket: PathBuf,
    /// UDS path for the SOCKS proxy bridge.
    socks_socket: PathBuf,
    /// Host-side socat process handles.
    host_handles: Vec<JoinHandle<()>>,
    /// Cancellation token for graceful shutdown.
    cancel_token: CancellationToken,
    /// Path to the socat binary.
    socat_path: PathBuf,
}

/// Ports used inside the sandbox for the bridged proxies.
pub struct BridgePorts {
    /// HTTP proxy port inside the sandbox.
    pub http_port: u16,
    /// SOCKS proxy port inside the sandbox.
    pub socks_port: u16,
}

impl Default for BridgePorts {
    fn default() -> Self {
        Self {
            http_port: 3128,
            socks_port: 1080,
        }
    }
}

impl BridgeManager {
    /// Create and start a bridge manager.
    ///
    /// Spawns host-side socat processes that listen on Unix domain sockets
    /// and forward to the proxy's TCP ports. The UDS paths are then
    /// bind-mounted into the sandbox by bwrap.
    ///
    /// `session_id` provides uniqueness to prevent socket path collisions
    /// between concurrent sessions.
    pub async fn start(
        socat_path: PathBuf,
        http_proxy_port: u16,
        socks_proxy_port: u16,
        session_id: &str,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        let socket_dir = std::env::temp_dir();
        let http_socket = socket_dir.join(format!("coco-http-{session_id}.sock"));
        let socks_socket = socket_dir.join(format!("coco-socks-{session_id}.sock"));

        // Clean up stale sockets from previous sessions
        let _ = std::fs::remove_file(&http_socket);
        let _ = std::fs::remove_file(&socks_socket);

        let mut host_handles = Vec::new();

        // Host-side bridge: UDS → TCP proxy
        for (socket, port, name) in [
            (&http_socket, http_proxy_port, "HTTP"),
            (&socks_socket, socks_proxy_port, "SOCKS"),
        ] {
            let handle =
                spawn_host_bridge(&socat_path, socket, port, name, cancel_token.clone()).await?;
            host_handles.push(handle);
        }

        tracing::info!(
            http_socket = %http_socket.display(),
            socks_socket = %socks_socket.display(),
            "Proxy bridges started"
        );

        Ok(Self {
            http_socket,
            socks_socket,
            host_handles,
            cancel_token,
            socat_path,
        })
    }

    /// UDS paths that must be bind-mounted into the bwrap sandbox.
    pub fn socket_paths(&self) -> [&PathBuf; 2] {
        [&self.http_socket, &self.socks_socket]
    }

    /// Path to the socat binary (needed for inside-sandbox bridge commands).
    pub fn socat_path(&self) -> &PathBuf {
        &self.socat_path
    }

    /// Generate the shell prefix that starts inside-sandbox bridges.
    ///
    /// This prefix is prepended to the sandboxed command so that socat
    /// listeners run inside the network namespace, forwarding TCP traffic
    /// through the bind-mounted UDS back to the host proxy.
    pub fn inner_bridge_prefix(&self, ports: &BridgePorts) -> String {
        let socat = self.socat_path.display();
        let http_sock = self.http_socket.display();
        let socks_sock = self.socks_socket.display();
        let http_port = ports.http_port;
        let socks_port = ports.socks_port;

        // Start background socat listeners with proper cleanup trap
        format!(
            "{socat} TCP-LISTEN:{http_port},fork,reuseaddr UNIX-CONNECT:{http_sock} & \
             {socat} TCP-LISTEN:{socks_port},fork,reuseaddr UNIX-CONNECT:{socks_sock} & \
             trap 'kill %1 %2 2>/dev/null; exit' EXIT; "
        )
    }

    /// Stop all bridge processes and clean up sockets.
    pub async fn stop(&mut self) {
        self.cancel_token.cancel();
        for handle in self.host_handles.drain(..) {
            let _ = handle.await;
        }
        let _ = std::fs::remove_file(&self.http_socket);
        let _ = std::fs::remove_file(&self.socks_socket);
        tracing::info!("Proxy bridges stopped");
    }
}

impl Drop for BridgeManager {
    /// Wind down host-side socat tasks and clean up the UDS sockets when the
    /// owning [`crate::SandboxState`] is dropped. `stop()` additionally awaits
    /// the `JoinHandle`s; `Drop` cannot await, but cancelling (the socat
    /// children spawn with `kill_on_drop`) is enough.
    fn drop(&mut self) {
        self.cancel_token.cancel();
        let _ = std::fs::remove_file(&self.http_socket);
        let _ = std::fs::remove_file(&self.socks_socket);
    }
}

/// Spawn a host-side socat process: `socat UNIX-LISTEN:<socket>,fork TCP:localhost:<port>`.
///
/// Waits briefly for socat to create the socket file before returning.
async fn spawn_host_bridge(
    socat_path: &Path,
    socket_path: &Path,
    proxy_port: u16,
    name: &'static str,
    cancel_token: CancellationToken,
) -> anyhow::Result<JoinHandle<()>> {
    let listen_arg = format!("UNIX-LISTEN:{},fork", socket_path.display());
    let connect_arg = format!("TCP:localhost:{proxy_port}");

    let socat = socat_path.to_path_buf();
    let socket = socket_path.to_path_buf();

    let handle = tokio::spawn(async move {
        let child = Command::new(&socat)
            .arg(&listen_arg)
            .arg(&connect_arg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();

        match child {
            Ok(mut child) => {
                tokio::select! {
                    () = cancel_token.cancelled() => {
                        let _ = child.kill().await;
                    }
                    status = child.wait() => {
                        if let Ok(status) = status
                            && !status.success()
                        {
                            tracing::warn!(
                                name,
                                code = status.code(),
                                "Bridge socat exited with error"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(name, error = %e, "Failed to spawn bridge socat");
            }
        }

        let _ = std::fs::remove_file(&socket);
    });

    // Wait briefly for socat to create the socket
    for _ in 0..20 {
        if socket_path.exists() {
            tracing::debug!(name, socket = %socket_path.display(), "Bridge socket ready");
            return Ok(handle);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    tracing::warn!(
        name,
        socket = %socket_path.display(),
        "Bridge socket not ready after 1s; proceeding anyway"
    );

    Ok(handle)
}

#[cfg(test)]
#[path = "bridge.test.rs"]
mod tests;
