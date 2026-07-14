//! SDK process-mode entrypoint: NDJSON-over-stdio JSON-RPC control protocol.
//!
//! Owns the CLI-side SDK process policy — stdio transport selection, sidecar
//! listener startup, the dispatch-loop + OS-signal shutdown sequencing, and the
//! host drain. The wire/transport machinery (`SdkServer`, transports, sidecar
//! implementations, JSON-RPC/AppServer connection adaptation) lives in
//! `coco-sdk-server`; this module only composes it into the `coco sdk` process.

use anyhow::{Context as _, Result};
use coco_agent_host::remote_host::PreparedHost;
use coco_sdk_server::{SdkServer, SdkSidecarConfig, SdkSidecarListeners, StdioTransport};

/// Run in SDK mode: NDJSON-over-stdio JSON-RPC control protocol.
pub async fn run_sdk_mode(mut host: PreparedHost, sidecar_config: SdkSidecarConfig) -> Result<()> {
    let transport = StdioTransport::new();
    let mut server = SdkServer::new(transport, host.bridge_host());
    if let Some(plugin_notifications) = host.take_plugin_notifications() {
        server = server.with_external_notifications(plugin_notifications);
    }

    tracing::info!(
        target: "coco_cli::sdk",
        "sdk server entering AppServer bridge dispatch loop"
    );
    let sdk_sidecar_listeners = SdkSidecarListeners::start_from_config(sidecar_config, &host)
        .await
        .context("SDK sidecar startup failed")?;
    let connection = host.connect();
    let dispatch_result = tokio::select! {
        result = server.run_app_server_connection(connection) => result.map(|_| ()),
        () = coco_agent_host::shutdown::os_interrupt_signal() => {
            tracing::info!(
                target: "coco_cli::sdk",
                "received OS shutdown signal; draining SDK AppServer sessions"
            );
            Ok(())
        }
    };

    sdk_sidecar_listeners
        .shutdown(host.shutdown_timeout())
        .await;
    let host_shutdown = host.shutdown().await;

    if let Err(error) = dispatch_result {
        tracing::error!(
            target: "coco_cli::sdk",
            error = %error,
            "sdk dispatch loop exited with error"
        );
        eprintln!("sdk mode: dispatch loop exited with error: {error}");
        return Err(anyhow::anyhow!("SDK dispatch failed: {error}"));
    }
    host_shutdown.map_err(|error| anyhow::anyhow!("SDK host shutdown failed: {error}"))
}
