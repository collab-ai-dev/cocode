# coco-remote — Crate Plan

Directory: `remote/` (v3)
TS source: `remote/` (4 files, ~1.1K LOC), `upstreamproxy/` (2 files, ~740 LOC)
Total: ~1.9K LOC across 6 files

## Dependencies

```
coco-remote depends on:
  - coco-types    (Message, PermissionDecision, SessionId)
  - coco-config   (OAuth token, org UUID)
  - coco-error
  - tokio         (async runtime, net::TcpListener)
  - tokio-tungstenite (WebSocket client)
  - reqwest       (HTTP POST for user messages)

coco-remote does NOT depend on:
  - coco-tui      (TUI consumes remote events via channels)
  - coco-tool     (creates tool stubs for unknown remote tools)
  - coco-inference (no LLM calls — relays to remote CCR)
```

## Data Definitions

### Remote Session (from `remote/`)

```rust
/// Remote session configuration.
pub struct RemoteSessionConfig {
    pub session_id: SessionId,
    pub base_url: String,
    pub get_oauth_token: Box<dyn Fn() -> BoxFuture<'static, Result<String>> + Send + Sync>,
    pub org_uuid: Option<String>,
}

/// Callbacks for remote session events.
pub trait RemoteSessionCallbacks: Send + Sync {
    fn on_message(&self, message: Message);
    fn on_permission_request(&self, request: RemotePermissionRequest);
    fn on_connected(&self);
    fn on_disconnected(&self);
    fn on_error(&self, error: String);
    fn on_reconnecting(&self);
}

/// Permission request from remote CCR worker.
pub struct RemotePermissionRequest {
    pub request_id: String,
    pub tool_id: ToolId,
    pub tool_use_id: String,
    pub description: Option<String>,
    pub input: Value,
    pub permission_suggestions: Option<Vec<String>>,
    pub blocked_path: Option<String>,
}

/// Permission response to send back.
pub enum RemotePermissionResponse {
    Allow { updated_input: Option<Value> },
    Deny { message: String },
}

/// SDK message types from CCR backend.
pub enum SdkMessage {
    Assistant(AssistantMessage),
    User(UserMessage),
    StreamEvent(StreamEvent),
    Result { subtype: String, text: Option<String> },
    System { subtype: String, content: Option<String> },
    ToolProgress { tool_use_id: String, progress: Value },
    Ignored,
}

/// Converted message for local rendering.
pub enum ConvertedMessage {
    Message(Message),
    StreamEvent(StreamEvent),
    Ignored,
}
```

### Upstream Proxy (from `upstreamproxy/`)

```rust
/// Proxy state after initialization.
pub struct UpstreamProxyState {
    pub enabled: bool,
    pub port: Option<u16>,
    pub ca_bundle_path: Option<PathBuf>,
}

/// TCP CONNECT relay configuration.
pub struct RelayConfig {
    pub base_url: String,
    pub session_token: String,
    pub ca_bundle_path: Option<PathBuf>,
}

/// Running relay handle.
pub struct UpstreamProxyRelay {
    pub port: u16,
    stop_tx: oneshot::Sender<()>,
}
```

## Core Logic

### Remote Session Manager (from `RemoteSessionManager.ts`, 343 LOC)

```rust
/// Orchestrates remote CCR session lifecycle.
pub struct RemoteSessionManager {
    config: RemoteSessionConfig,
    ws: SessionsWebSocket,
    pending_permissions: HashMap<String, RemotePermissionRequest>,
}

impl RemoteSessionManager {
    pub async fn connect(&mut self, callbacks: Arc<dyn RemoteSessionCallbacks>) -> Result<()>;

    /// HTTP POST user input to remote session.
    pub async fn send_message(&self, input: &str) -> Result<()>;

    /// Send allow/deny control response over WebSocket.
    pub async fn respond_to_permission_request(
        &mut self,
        request_id: &str,
        response: RemotePermissionResponse,
    ) -> Result<()>;

    /// Send interrupt control request.
    pub async fn cancel_session(&self) -> Result<()>;

    pub fn is_connected(&self) -> bool;
    pub async fn disconnect(&mut self);
    pub async fn reconnect(&mut self) -> Result<()>;
}
```

### WebSocket Client (from `SessionsWebSocket.ts`, 404 LOC)

```rust
/// WebSocket with reconnect, auth, heartbeat.
/// URL: wss://api.anthropic.com/v1/sessions/ws/{id}/subscribe
struct SessionsWebSocket;

impl SessionsWebSocket {
    async fn connect(config: &RemoteSessionConfig) -> Result<Self>;
    fn send_control_response(&self, response: &SdkControlResponse) -> Result<()>;
    fn send_control_request(&self, request: &SdkControlRequest) -> Result<()>;
    fn is_connected(&self) -> bool;
    async fn close(&mut self);
    async fn reconnect(&mut self) -> Result<()>;
}
```

Reconnect strategy:
- 4003 (unauthorized) -> stop
- 4001 (session not found) -> retry 3x with linear backoff (RECONNECT_DELAY_MS * attempt_count; server briefly sees session as stale during compaction)
- Other transient -> retry 5x, base delay 2s
- Heartbeat ping every 30s

### SDK Message Adapter (from `sdkMessageAdapter.ts`, 302 LOC)

```rust
/// Convert SDK messages from remote CCR to internal Message types.
pub fn convert_sdk_message(
    msg: &Value,
    convert_tool_results: bool,
    convert_user_text: bool,
) -> ConvertedMessage;

/// Check if message signals session end.
pub fn is_session_end_message(msg: &Value) -> bool;
```

### Permission Bridge (from `remotePermissionBridge.ts`, 78 LOC)

```rust
/// Create synthetic AssistantMessage for permission prompts on unknown remote tools.
pub fn create_synthetic_assistant_message(request: &RemotePermissionRequest) -> AssistantMessage;

/// Minimal tool stub for tools unknown to the local client.
pub fn create_tool_stub(tool_name: &str, schema: Option<&Value>) -> ToolStub;
```

### Upstream Proxy Init (from `upstreamproxy.ts`, 285 LOC)

```rust
/// Initialize container-side upstream proxy (fail-open).
/// Steps: set_non_dumpable() -> download CA bundle -> start relay -> delete token file.
pub async fn init_upstream_proxy() -> UpstreamProxyState;

/// Return env vars for subprocess inheritance.
/// When enabled: HTTPS_PROXY, SSL_CERT_FILE, NODE_EXTRA_CA_CERTS, etc.
/// NO_PROXY: loopback, RFC1918, anthropic.com, github.com, package registries.
pub fn get_upstream_proxy_env(state: &UpstreamProxyState) -> HashMap<String, String>;
```

### TCP CONNECT Relay (from `relay.ts`, 455 LOC)

```rust
/// TCP CONNECT -> WebSocket tunnel with credential injection.
/// Injects Proxy-Authorization: Basic(session:token) in first chunk.
pub async fn start_relay(config: RelayConfig) -> Result<UpstreamProxyRelay>;
```

Per-connection state machine:
1. Phase 1: Accumulate HTTP CONNECT header, parse `CONNECT host:port`
2. Phase 2: Forward bytes over WebSocket to `{base_url}/v1/code/upstreamproxy/ws`

Wire format: hand-rolled protobuf chunks (`[0x0a, varint_len, data]`, max 512KB per chunk).
Keepalive: empty chunk every 30s (sidecar idle timeout = 50s).
Security: prctl(PR_SET_DUMPABLE, 0) on Linux, token file deleted after relay confirms up.

## Module Layout

```
remote/
  mod.rs                    — pub mod, re-exports
  session_manager.rs        — RemoteSessionManager orchestration
  websocket_client.rs       — SessionsWebSocket with reconnect
  message_adapter.rs        — SDK message -> internal message conversion
  permission_bridge.rs      — synthetic messages for unknown tools
  proxy/
    mod.rs                  — init_upstream_proxy, get_upstream_proxy_env
    relay.rs                — TCP CONNECT -> WebSocket tunnel
    chunk_codec.rs          — protobuf chunk encode/decode
```
