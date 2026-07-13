use std::time::Duration;

#[cfg(unix)]
use std::path::PathBuf;

use coco_agent_host::remote_host::PreparedRemoteHost;
use coco_app_server_transport::TransportFrameError;
use coco_error::StackError;
use tokio::task::JoinHandle;

const SDK_WEBSOCKET_LISTENER_CHANNEL_CAPACITY: usize = 256;
#[cfg(unix)]
const SDK_UNIX_LISTENER_CHANNEL_CAPACITY: usize = 256;
#[cfg(windows)]
const SDK_NAMED_PIPE_LISTENER_CHANNEL_CAPACITY: usize = 256;

/// Optional SDK sidecar listeners bound in SDK mode.
///
/// These listeners are transport adapters over the same AppServer and host
/// handler as stdio. CLI startup owns mode selection; this crate owns how SDK
/// transports are bound, run, and shut down.
pub struct SdkSidecarListeners {
    #[cfg(unix)]
    unix: Option<SdkUnixListenerTask>,
    websocket: Option<SdkWebSocketListenerTask>,
    #[cfg(windows)]
    named_pipe: Option<SdkNamedPipeListenerTask>,
}

#[derive(Clone, Debug, Default)]
pub struct SdkSidecarConfig {
    #[cfg(unix)]
    unix_socket_path: Option<PathBuf>,
    websocket_bind: Option<String>,
    #[cfg(windows)]
    named_pipe_name: Option<String>,
}

impl SdkSidecarConfig {
    #[cfg(unix)]
    pub fn unix_socket_path(&self) -> Option<&PathBuf> {
        self.unix_socket_path.as_ref()
    }

    pub fn websocket_bind(&self) -> Option<&str> {
        self.websocket_bind.as_deref()
    }

    #[cfg(windows)]
    pub fn named_pipe_name(&self) -> Option<&str> {
        self.named_pipe_name.as_deref()
    }

    #[cfg(unix)]
    pub fn with_unix_socket_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.unix_socket_path = Some(path.into());
        self
    }

    pub fn with_websocket_bind(mut self, bind_addr: impl Into<String>) -> Self {
        self.websocket_bind = Some(bind_addr.into());
        self
    }

    #[cfg(windows)]
    pub fn with_named_pipe_name(mut self, name: impl Into<String>) -> Self {
        self.named_pipe_name = Some(name.into());
        self
    }
}

impl SdkSidecarListeners {
    pub async fn start_from_config(
        config: SdkSidecarConfig,
        host: &PreparedRemoteHost,
    ) -> Result<Self, SdkSidecarError> {
        #[cfg(unix)]
        let unix = start_sdk_unix_listener(config.unix_socket_path, host)?;
        let websocket = start_sdk_websocket_listener(config.websocket_bind, host).await?;
        #[cfg(windows)]
        let named_pipe = start_sdk_named_pipe_listener(config.named_pipe_name, host)?;

        Ok(Self {
            #[cfg(unix)]
            unix,
            websocket,
            #[cfg(windows)]
            named_pipe,
        })
    }

    pub async fn shutdown(self, shutdown_timeout: Duration) {
        #[cfg(unix)]
        shutdown_sdk_unix_listener(self.unix, shutdown_timeout).await;
        shutdown_sdk_websocket_listener(self.websocket, shutdown_timeout).await;
        #[cfg(windows)]
        shutdown_sdk_named_pipe_listener(self.named_pipe, shutdown_timeout).await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SdkSidecarError {
    #[cfg(unix)]
    #[error("failed to bind SDK AppServer Unix socket at {path}: {source}")]
    UnixBind {
        path: PathBuf,
        source: TransportFrameError,
    },
    #[error("failed to bind SDK AppServer WebSocket listener at {bind_addr}: {source}")]
    WebSocketBind {
        bind_addr: String,
        source: std::io::Error,
    },
    #[cfg(windows)]
    #[error("failed to bind SDK AppServer Windows named pipe at {pipe_name}: {source}")]
    NamedPipeBind {
        pipe_name: String,
        source: TransportFrameError,
    },
}

impl StackError for SdkSidecarError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

#[cfg(unix)]
struct SdkUnixListenerTask {
    socket_path: PathBuf,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<(), String>>,
    outbound_forwarder: JoinHandle<()>,
}

struct SdkWebSocketListenerTask {
    bind_addr: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<(), String>>,
    outbound_forwarder: JoinHandle<()>,
}

#[cfg(windows)]
struct SdkNamedPipeListenerTask {
    pipe_name: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<(), String>>,
    outbound_forwarder: JoinHandle<()>,
}

#[cfg(unix)]
fn start_sdk_unix_listener(
    socket_path: Option<PathBuf>,
    host: &PreparedRemoteHost,
) -> Result<Option<SdkUnixListenerTask>, SdkSidecarError> {
    let Some(socket_path) = socket_path else {
        return Ok(None);
    };

    let listener =
        coco_app_server_transport::bind_ndjson_unix_listener(&socket_path).map_err(|source| {
            SdkSidecarError::UnixBind {
                path: socket_path.clone(),
                source,
            }
        })?;
    let adapter = host.adapter();
    let binding = host.open_sidecar_binding(SDK_UNIX_LISTENER_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task_socket_path = socket_path.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_unix_listener_until_shutdown(listener, binding.handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                socket_path = %task_socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener exited with error"
            );
        }
        result.map_err(|error| error.to_string())
    });

    tracing::info!(
        target: "coco_sdk_server::sidecar",
        socket_path = %socket_path.display(),
        "SDK AppServer Unix listener started"
    );
    Ok(Some(SdkUnixListenerTask {
        socket_path,
        shutdown_tx,
        listener_task,
        outbound_forwarder: binding.outbound_forwarder,
    }))
}

async fn start_sdk_websocket_listener(
    bind_addr: Option<String>,
    host: &PreparedRemoteHost,
) -> Result<Option<SdkWebSocketListenerTask>, SdkSidecarError> {
    let Some(bind_addr) = bind_addr else {
        return Ok(None);
    };

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|source| SdkSidecarError::WebSocketBind {
            bind_addr: bind_addr.clone(),
            source,
        })?;
    let local_addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| bind_addr.clone());
    let binding = host.open_sidecar_binding(SDK_WEBSOCKET_LISTENER_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let adapter = host.adapter();
    let task_bind_addr = local_addr.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_websocket_listener_until_shutdown(listener, binding.handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                bind_addr = %task_bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener exited with error"
            );
        }
        result.map_err(|error| error.to_string())
    });

    tracing::info!(
        target: "coco_sdk_server::sidecar",
        bind_addr = %local_addr,
        "SDK AppServer WebSocket listener started"
    );
    Ok(Some(SdkWebSocketListenerTask {
        bind_addr: local_addr,
        shutdown_tx,
        listener_task,
        outbound_forwarder: binding.outbound_forwarder,
    }))
}

#[cfg(windows)]
fn start_sdk_named_pipe_listener(
    pipe_name: Option<String>,
    host: &PreparedRemoteHost,
) -> Result<Option<SdkNamedPipeListenerTask>, SdkSidecarError> {
    let Some(pipe_name) = pipe_name else {
        return Ok(None);
    };

    let listener = coco_app_server_transport::bind_ndjson_named_pipe_listener(&pipe_name).map_err(
        |source| SdkSidecarError::NamedPipeBind {
            pipe_name: pipe_name.clone(),
            source,
        },
    )?;
    let binding = host.open_sidecar_binding(SDK_NAMED_PIPE_LISTENER_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let adapter = host.adapter();
    let task_pipe_name = pipe_name.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_named_pipe_listener_until_shutdown(listener, binding.handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                pipe_name = %task_pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener exited with error"
            );
        }
        result.map_err(|error| error.to_string())
    });

    tracing::info!(
        target: "coco_sdk_server::sidecar",
        pipe_name = %pipe_name,
        "SDK AppServer named-pipe listener started"
    );
    Ok(Some(SdkNamedPipeListenerTask {
        pipe_name,
        shutdown_tx,
        listener_task,
        outbound_forwarder: binding.outbound_forwarder,
    }))
}

#[cfg(unix)]
async fn shutdown_sdk_unix_listener(
    listener: Option<SdkUnixListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                socket_path = %listener.socket_path.display(),
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer Unix listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}

async fn shutdown_sdk_websocket_listener(
    listener: Option<SdkWebSocketListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                bind_addr = %listener.bind_addr,
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer WebSocket listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}

#[cfg(windows)]
async fn shutdown_sdk_named_pipe_listener(
    listener: Option<SdkNamedPipeListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_sdk_server::sidecar",
                pipe_name = %listener.pipe_name,
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer named-pipe listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}
