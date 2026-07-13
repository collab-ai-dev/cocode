use coco_agent_host::remote_host::PreparedRemoteHost;
use coco_error::{ErrorExt, Location, StatusCode, stack_trace_debug};
use snafu::{ResultExt, Snafu};

use crate::{
    RemoteAppServerBridgeError, SdkServer, SdkSidecarConfig, SdkSidecarError, SdkSidecarListeners,
    StdioTransport,
};

#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum SdkStartupError {
    #[snafu(display("SDK sidecar startup failed: {source}"))]
    Sidecar {
        source: SdkSidecarError,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("SDK dispatch failed: {source}"))]
    Dispatch {
        source: RemoteAppServerBridgeError,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("SDK host shutdown failed: {message}"))]
    HostShutdown {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl SdkStartupError {
    fn host_shutdown(source: impl std::fmt::Display) -> Self {
        HostShutdownSnafu {
            message: source.to_string(),
        }
        .build()
    }
}

impl ErrorExt for SdkStartupError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Sidecar { .. } => StatusCode::IoError,
            Self::Dispatch { .. } | Self::HostShutdown { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Run in SDK mode: NDJSON-over-stdio JSON-RPC control protocol.
pub async fn run_sdk_mode(
    mut host: PreparedRemoteHost,
    sidecar_config: SdkSidecarConfig,
) -> Result<(), SdkStartupError> {
    let transport = StdioTransport::new();
    let mut server = SdkServer::new(transport, host.bridge_host());
    if let Some(plugin_notifications) = host.take_plugin_notifications() {
        server = server.with_external_notifications(plugin_notifications);
    }

    tracing::info!(
        target: "coco_sdk_server::startup",
        "sdk server entering AppServer bridge dispatch loop"
    );
    let sdk_sidecar_listeners = SdkSidecarListeners::start_from_config(sidecar_config, &host)
        .await
        .context(SidecarSnafu)?;
    let connection = host.connect();
    let dispatch_result = tokio::select! {
        result = server.run_app_server_connection(connection) => result.map(|_| ()),
        () = coco_agent_host::shutdown::os_interrupt_signal() => {
            tracing::info!(
                target: "coco_sdk_server::startup",
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
            target: "coco_sdk_server::startup",
            error = %error,
            "sdk dispatch loop exited with error"
        );
        eprintln!("sdk mode: dispatch loop exited with error: {error}");
        return Err(error).context(DispatchSnafu);
    }
    host_shutdown.map_err(SdkStartupError::host_shutdown)
}
