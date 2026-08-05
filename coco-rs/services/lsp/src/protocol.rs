//! JSON-RPC 2.0 over stdio implementation

use crate::config::LifecycleConfig;
use crate::error::LspErr;
use crate::error::Result;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicI64};
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio::time::timeout;
use tracing::debug;
use tracing::info;
use tracing::trace;
use tracing::warn;

/// Default request timeout in seconds (legacy, use TimeoutConfig instead)
pub const REQUEST_TIMEOUT_SECS: i32 = 30;

/// Initialization timeout in seconds (legacy, use TimeoutConfig instead)
pub const INIT_TIMEOUT_SECS: i32 = 45;

/// Maximum allowed Content-Length for LSP messages (10 MB)
/// Prevents memory exhaustion from malformed or malicious servers
const MAX_CONTENT_LENGTH: usize = 10 * 1024 * 1024;

/// LSP error code for "content modified" (document changed during request)
const CONTENT_MODIFIED_ERROR_CODE: i32 = -32801;

/// Maximum retries for ContentModified errors
const CONTENT_MODIFIED_MAX_RETRIES: u32 = 3;

/// Base delay for ContentModified retry (milliseconds)
const CONTENT_MODIFIED_BASE_DELAY_MS: u64 = 500;

/// Configurable timeout settings for LSP operations
#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    /// Initialization timeout in milliseconds
    pub init_timeout_ms: i64,
    /// Request timeout in milliseconds
    pub request_timeout_ms: i64,
    /// Shutdown timeout in milliseconds
    pub shutdown_timeout_ms: i64,
    /// Notification channel buffer size
    pub notification_buffer_size: i32,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            init_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
            shutdown_timeout_ms: 5_000,
            notification_buffer_size: 100,
        }
    }
}

impl From<&LifecycleConfig> for TimeoutConfig {
    fn from(config: &LifecycleConfig) -> Self {
        Self {
            init_timeout_ms: config.startup_timeout_ms,
            request_timeout_ms: config.request_timeout_ms,
            shutdown_timeout_ms: config.shutdown_timeout_ms,
            notification_buffer_size: config.notification_buffer_size,
        }
    }
}

impl TimeoutConfig {
    /// Get init timeout as Duration
    pub fn init_timeout(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.init_timeout_ms).unwrap_or(0))
    }

    /// Get request timeout as Duration
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.request_timeout_ms).unwrap_or(0))
    }

    /// Get shutdown timeout as Duration
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.shutdown_timeout_ms).unwrap_or(0))
    }

    /// Get init timeout in seconds (for legacy API compatibility)
    pub fn init_timeout_secs(&self) -> i32 {
        i32::try_from(self.init_timeout_ms.max(0) / 1000).unwrap_or(i32::MAX)
    }

    /// Get request timeout in seconds (for legacy API compatibility)
    pub fn request_timeout_secs(&self) -> i32 {
        i32::try_from(self.request_timeout_ms.max(0) / 1000).unwrap_or(i32::MAX)
    }
}

type RequestId = i64;

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T: Serialize> {
    jsonrpc: &'static str,
    id: RequestId,
    method: String,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<RequestId>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    /// Optional additional data per JSON-RPC 2.0 spec
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

/// Pending request handle
struct PendingRequest {
    tx: oneshot::Sender<Result<serde_json::Value>>,
    method: String,
}

/// JSON-RPC connection over stdio
pub struct JsonRpcConnection {
    next_id: AtomicI64,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
    closed: Arc<AtomicBool>,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Reader task handle for cleanup
    reader_handle: Mutex<Option<JoinHandle<()>>>,
}

impl JsonRpcConnection {
    /// Create connection from child process stdio
    ///
    /// This is async to ensure the reader task is ready before returning,
    /// preventing race conditions where requests are sent before the reader
    /// is ready to receive responses.
    pub async fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        notification_tx: mpsc::Sender<(String, serde_json::Value)>,
    ) -> Self {
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = Arc::clone(&pending);
        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = Arc::clone(&closed);
        let reader_stdin = Arc::clone(&stdin);

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Create ready signal channel
        let (ready_tx, ready_rx) = oneshot::channel::<()>();

        // Spawn reader task with shutdown support and ready signaling
        let reader_handle = tokio::spawn(async move {
            // Create BufReader first, then signal ready
            let reader = BufReader::new(stdout);

            // Signal that reader is ready to receive data
            let _ = ready_tx.send(());

            // Enter the read loop
            run_reader_task(
                reader,
                Arc::clone(&pending_clone),
                reader_stdin,
                notification_tx,
                shutdown_rx,
                reader_closed,
            )
            .await;
        });

        // Wait for reader to be ready (with timeout to prevent hang)
        match timeout(Duration::from_secs(1), ready_rx).await {
            Ok(Ok(())) => {
                debug!("Reader task signaled ready");
            }
            Ok(Err(_)) => {
                warn!("Reader task ready channel closed unexpectedly");
            }
            Err(_) => {
                warn!("Timeout waiting for reader task to be ready");
            }
        }

        info!("JSON-RPC connection established");

        Self {
            next_id: AtomicI64::new(1),
            stdin,
            pending,
            closed,
            shutdown_tx,
            reader_handle: Mutex::new(Some(reader_handle)),
        }
    }

    /// Send request and await response
    pub async fn request<P: Serialize>(
        &self,
        method: &str,
        params: P,
    ) -> Result<serde_json::Value> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT_SECS)
            .await
    }

    /// Send request with custom timeout
    pub async fn request_with_timeout<P: Serialize>(
        &self,
        method: &str,
        params: P,
        timeout_secs: i32,
    ) -> Result<serde_json::Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(LspErr::ConnectionClosed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        // Serialize before registering the request so serialization failure
        // cannot strand an unreachable pending entry.
        let body = serde_json::to_string(&request)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let (tx, rx) = oneshot::channel();

        // Register pending request
        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return Err(LspErr::ConnectionClosed);
            }
            pending.insert(
                id,
                PendingRequest {
                    tx,
                    method: method.to_string(),
                },
            );
        }

        debug!("LSP request [{}]: {}", id, method);
        trace!("LSP request [{}]: {} {}", id, method, body);

        let write_result = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(message.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        if let Err(error) = write_result {
            self.closed.store(true, Ordering::Release);
            fail_pending_requests(&self.pending).await;
            return Err(error.into());
        }

        // Wait for response with timeout
        let method_clone = method.to_string();
        let timeout_secs = u64::try_from(timeout_secs).unwrap_or(0);
        match timeout(Duration::from_secs(timeout_secs), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LspErr::Internal("request cancelled".to_string())),
            Err(_) => {
                // Remove pending request on timeout
                {
                    let mut pending = self.pending.lock().await;
                    pending.remove(&id);
                }

                // Send cancel notification to server (best effort)
                self.cancel_request(id).await;

                warn!(
                    "LSP request [{}] ({}) timed out after {}s - cancel sent",
                    id, method_clone, timeout_secs
                );
                Err(LspErr::RequestTimeout {
                    timeout_secs: i32::try_from(timeout_secs).unwrap_or(i32::MAX),
                })
            }
        }
    }

    /// Send request with automatic retry on ContentModified (-32801) errors.
    ///
    /// LSP servers return `-32801` when the document was modified while
    /// processing a request. Retries with exponential backoff:
    /// 500ms, 1000ms, 2000ms (3 retries max).
    pub async fn request_with_retry<P: Serialize + Clone>(
        &self,
        method: &str,
        params: P,
        timeout_secs: i32,
    ) -> Result<serde_json::Value> {
        let mut last_err = None;
        for attempt in 0..=CONTENT_MODIFIED_MAX_RETRIES {
            match self
                .request_with_timeout(method, params.clone(), timeout_secs)
                .await
            {
                Ok(result) => return Ok(result),
                Err(LspErr::JsonRpc {
                    code: Some(code),
                    ref message,
                    ..
                }) if code == CONTENT_MODIFIED_ERROR_CODE => {
                    if attempt < CONTENT_MODIFIED_MAX_RETRIES {
                        let delay = CONTENT_MODIFIED_BASE_DELAY_MS * 2u64.pow(attempt);
                        debug!(
                            "LSP {method} ContentModified (attempt {}/{}), retrying in {delay}ms: {message}",
                            attempt + 1,
                            CONTENT_MODIFIED_MAX_RETRIES
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    last_err = Some(LspErr::JsonRpc {
                        method: method.to_string(),
                        code: Some(code),
                        message: message.clone(),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        // last_err is always Some after at least one iteration with a ContentModified error.
        // If somehow None (should be unreachable), return a generic error.
        Err(last_err.unwrap_or_else(|| {
            LspErr::Internal(format!("{method}: ContentModified retries exhausted"))
        }))
    }

    /// Cancel a pending request
    ///
    /// Sends $/cancelRequest notification to the server.
    /// This is a best-effort operation - the server may not support cancellation.
    pub async fn cancel_request(&self, id: RequestId) {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": id }
        });

        if let Ok(body) = serde_json::to_string(&notification) {
            let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
            let mut stdin = self.stdin.lock().await;
            let _ = stdin.write_all(message.as_bytes()).await;
            let _ = stdin.flush().await;
            debug!("Sent $/cancelRequest for request {}", id);
        }
    }

    /// Send request and deserialize response, treating null as None
    ///
    /// This is a convenience method that handles the common pattern of:
    /// 1. Sending a request
    /// 2. Checking if the response is null (returns None)
    /// 3. Deserializing the response to the target type
    ///
    /// This reduces boilerplate in LSP operation handlers.
    pub async fn request_optional<P, R>(&self, method: &str, params: P) -> Result<Option<R>>
    where
        P: Serialize + Clone,
        R: for<'de> Deserialize<'de>,
    {
        let value = self
            .request_with_retry(method, params, REQUEST_TIMEOUT_SECS)
            .await?;
        if value.is_null() {
            Ok(None)
        } else {
            serde_json::from_value(value).map(Some).map_err(Into::into)
        }
    }

    /// Send notification (no response expected)
    pub async fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(LspErr::ConnectionClosed);
        }
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        let body = serde_json::to_string(&notification)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        debug!("LSP notify: {}", method);
        trace!("LSP notify: {} {}", method, body);

        let write_result = async {
            let mut stdin = self.stdin.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "LSP connection is closed",
                ));
            }
            stdin.write_all(message.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        match write_result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.closed.store(true, Ordering::Release);
                fail_pending_requests(&self.pending).await;
                Err(error.into())
            }
        }
    }

    /// Read and dispatch incoming LSP messages
    ///
    /// Reads JSON-RPC messages from the server, dispatches responses to pending
    /// request handlers, and forwards notifications to the notification channel.
    /// Supports graceful shutdown via the shutdown_rx watch channel.
    async fn read_messages<R, W>(
        mut reader: BufReader<R>,
        pending: Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
        stdin: Arc<Mutex<W>>,
        notification_tx: mpsc::Sender<(String, serde_json::Value)>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        loop {
            // Check shutdown signal
            if *shutdown_rx.borrow() {
                debug!("LSP read loop received shutdown signal");
                return Ok(());
            }

            // Read headers until empty line
            let mut content_length: Option<usize> = None;

            loop {
                let mut header = String::new();

                // Use select to check for shutdown while reading
                let bytes_read = tokio::select! {
                    result = reader.read_line(&mut header) => result?,
                    _ = shutdown_rx.changed() => {
                        debug!("LSP read loop shutdown during header read");
                        return Ok(());
                    }
                };

                if bytes_read == 0 {
                    let pending_guard = pending.lock().await;
                    let pending_count = pending_guard.len();
                    drop(pending_guard);

                    info!(
                        "LSP connection closed (pending requests: {})",
                        pending_count
                    );

                    return Ok(());
                }

                let header = header.trim();
                if header.is_empty() {
                    break;
                }

                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("Content-Length")
                {
                    content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                        LspErr::Internal(format!("invalid LSP Content-Length header: {header}"))
                    })?);
                }
            }

            let content_length = match content_length {
                Some(len) => len,
                None => {
                    return Err(LspErr::Internal(
                        "missing LSP Content-Length header".to_string(),
                    ));
                }
            };

            // Validate Content-Length to prevent memory exhaustion
            if content_length > MAX_CONTENT_LENGTH {
                return Err(LspErr::Internal(format!(
                    "LSP Content-Length {content_length} exceeds maximum {MAX_CONTENT_LENGTH}"
                )));
            }

            // Read body
            let mut buffer = vec![0u8; content_length];
            reader.read_exact(&mut buffer).await?;

            // Parse message with strict UTF-8 validation
            let raw = match String::from_utf8(buffer) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Invalid UTF-8 in LSP message: {}", e);
                    continue;
                }
            };
            trace!("LSP received: {}", raw);

            let value: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to parse LSP message: {}", e);
                    continue;
                }
            };

            // A message with both method and id is a server-to-client request,
            // not a response. Always answer it so the server cannot hang
            // waiting on a client request we silently discarded.
            if value.get("id").is_some() && value.get("method").is_some() {
                respond_to_server_request(&stdin, &value).await?;
            } else if value.get("id").is_some() {
                // Response
                if let Ok(response) = serde_json::from_value::<JsonRpcResponse>(value)
                    && let Some(id) = response.id
                {
                    let mut pending_guard = pending.lock().await;
                    if let Some(req) = pending_guard.remove(&id) {
                        let result = if let Some(err) = response.error {
                            Err(LspErr::JsonRpc {
                                method: req.method.clone(),
                                message: err.message,
                                code: Some(err.code),
                            })
                        } else {
                            Ok(response.result.unwrap_or(serde_json::Value::Null))
                        };
                        let _ = req.tx.send(result);
                    }
                }
            } else if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                // Notification - check for backpressure
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let notification = (method.to_string(), params);

                // Try non-blocking send first to detect backpressure
                match notification_tx.try_send(notification) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(notification)) => {
                        // Blocking this reader would also block response
                        // correlation. Notifications are best-effort; keep the
                        // response transport alive and drop only this update.
                        warn!(
                            "LSP notification channel full, dropping notification (method: {})",
                            notification.0
                        );
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        // Channel closed, reader is shutting down
                        debug!("Notification channel closed");
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn run_reader_task<R, W>(
    reader: BufReader<R>,
    pending: Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
    stdin: Arc<Mutex<W>>,
    notification_tx: mpsc::Sender<(String, serde_json::Value)>,
    shutdown_rx: watch::Receiver<bool>,
    closed: Arc<AtomicBool>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if let Err(error) = JsonRpcConnection::read_messages(
        reader,
        Arc::clone(&pending),
        stdin,
        notification_tx,
        shutdown_rx,
    )
    .await
    {
        warn!("LSP read loop ended with error: {error}");
    }
    closed.store(true, Ordering::Release);
    fail_pending_requests(&pending).await;
}

async fn fail_pending_requests(pending: &Mutex<HashMap<RequestId, PendingRequest>>) {
    let mut pending = pending.lock().await;
    for (_, request) in pending.drain() {
        let _ = request.tx.send(Err(LspErr::ConnectionClosed));
    }
}

async fn respond_to_server_request<W>(stdin: &Mutex<W>, request: &serde_json::Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let response = server_request_response(request);
    let body = serde_json::to_vec(&response)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut stdin = stdin.lock().await;
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(&body).await?;
    stdin.flush().await?;
    Ok(())
}

fn server_request_response(request: &serde_json::Value) -> serde_json::Value {
    let id = request
        .get("id")
        .filter(|id| id.is_i64() || id.is_u64() || id.is_string())
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let result = match method {
        // The connection does not own user configuration. Preserve the
        // requested array shape while explicitly returning no overrides.
        "workspace/configuration" => {
            let count = request
                .pointer("/params/items")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
            Some(serde_json::Value::Array(vec![
                serde_json::Value::Null;
                count
            ]))
        }
        "workspace/workspaceFolders" => Some(serde_json::Value::Array(Vec::new())),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Some(serde_json::Value::Null),
        // Server-initiated edits bypass the tool permission and file-history
        // pipeline, so reject them at this transport boundary.
        "workspace/applyEdit" => Some(serde_json::json!({
            "applied": false,
            "failureReason": "server-initiated workspace edits are not supported"
        })),
        // No interactive UI callback is wired into this connection.
        "window/showMessageRequest" => Some(serde_json::Value::Null),
        _ => None,
    };
    match result {
        Some(result) => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        None => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("client method not supported: {method}"),
            },
        }),
    }
}

impl Drop for JsonRpcConnection {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        // Signal shutdown to reader task
        let _ = self.shutdown_tx.send(true);

        // Abort reader task if still running
        if let Some(handle) = self.reader_handle.get_mut().take() {
            handle.abort();
            debug!("JsonRpcConnection dropped - reader task aborted");
        }
    }
}

#[cfg(test)]
#[path = "protocol.test.rs"]
mod tests;
