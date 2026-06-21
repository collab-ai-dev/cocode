# crate-coco-bridge

TS source: `bridge/` (31 files, ~12.6K LOC)

## Dependencies

```
coco-bridge depends on:
  - coco-types (Message, Attachment, SessionId)
  - coco-session (SessionState)
  - coco-config (Settings, auth config)
  - tokio-tungstenite (WebSocket transport)
  - reqwest (HTTP API client)

coco-bridge does NOT depend on:
  - coco-query, coco-tools, coco-tui (bridge is a standalone entry point)
  - coco-mcp (bridge IS the MCP-based IDE integration, not a consumer)
```

## Architecture Overview

Bridge enables IDE integration (VS Code, JetBrains, etc.) by connecting local CLI/IDE with remote cloud sessions. Two transport generations:

```
IDE ─► MCP-based protocol ─► Bridge ─► Transport ─► CCR Worker
                                 │
                          Permission relay
                                 │
                          claude.ai Web UI
```

**v1 (env-based):** Environments API polling → HybridTransport (WS read + HTTP write)
**v2 (env-less):** Direct OAuth → worker JWT → SSETransport + CCRClient

## Core Types

```rust
pub struct BridgeConfig {
    pub dir: String,
    pub machine_name: String,
    pub branch: String,
    pub git_repo_url: Option<String>,
    pub max_sessions: i32,
    pub spawn_mode: SpawnMode,
    pub bridge_id: String,          // client-generated UUID
    pub environment_id: String,
    pub reuse_environment_id: Option<String>,
    pub api_base_url: String,
    pub session_ingress_url: String,
    pub session_timeout_ms: Option<i64>,
    pub sandbox: bool,
}

/// CCR daemon spawn modes
pub enum SpawnMode {
    SingleSession,  // one session in cwd, bridge tears down when it ends
    Worktree,       // persistent server, each session gets isolated git worktree
    SameDir,        // persistent server, all sessions share cwd
}

pub enum BridgeState {
    Ready,          // initialized, waiting for work
    Connected,      // transport connected
    Reconnecting,   // attempting restore
    Failed,         // fatal error (expired, auth failed)
}

pub enum SessionDoneStatus {
    Completed,
    Failed,
    Interrupted,
}
```

## Work Polling & Session Lifecycle

```
BridgeConfig
  → register_bridge_environment() → environment_id + secret
  → poll_for_work() → WorkResponse (session_id, data, secret)
  → create_session() → session created
  → acknowledge_work() → work accepted
  → init_bridge_core() or init_env_less_bridge_core() → ReplBridgeHandle
  → sessions run, heartbeat_work() extends lease
  → stop_work() / send_result() → teardown
  → archive_session() / deregister_environment() → cleanup
```

```rust
pub struct WorkResponse {
    pub id: String,
    pub environment_id: String,
    pub state: String,
    pub data: WorkData,      // Session or Healthcheck
    pub secret: String,      // base64url-encoded WorkSecret
    pub created_at: String,
}

pub struct WorkSecret {
    pub version: i32,
    pub session_ingress_token: String,
    pub api_base_url: String,
    pub sources: Vec<WorkSource>,
    pub auth: Vec<WorkAuth>,
    pub mcp_config: Option<Value>,
    pub environment_variables: Option<HashMap<String, String>>,
    pub use_code_sessions: bool,  // CCR v2 compat selector
}
```

## Transport Layer

```rust
/// Unified transport interface for bridge communication
pub trait ReplBridgeTransport: Send + Sync {
    fn write(&self, msg: SdkMessage) -> Result<()>;
    fn write_batch(&self, msgs: Vec<SdkMessage>) -> Result<()>;
    fn get_last_sequence_num(&self) -> Option<i64>;  // SSE sequence recovery
}
```

**v1 HybridTransport**: WebSocket (read) + Session-Ingress POST (write)
**v2 SSETransport**: SSE events (read) + CCRClient POST (write)

Selection: `use_code_sessions` flag in WorkSecret → v2; otherwise v1.

## ReplBridgeHandle

```rust
pub struct ReplBridgeHandle {
    pub bridge_session_id: String,
    pub environment_id: String,
    pub session_ingress_url: String,
}

impl ReplBridgeHandle {
    pub fn write_messages(&self, messages: Vec<Message>);
    pub fn write_sdk_messages(&self, messages: Vec<SdkMessage>);
    pub fn send_control_request(&self, request: SdkControlRequest);
    pub fn send_control_response(&self, response: SdkControlResponse);
    pub fn send_result(&self);
    pub async fn teardown(&self);
}
```

## Permission Relay

Flow: child process → session runner → bridge → server (claude.ai) → user → response back

```rust
pub struct PermissionRequest {
    pub request_id: String,
    pub subtype: String,        // "can_use_tool"
    pub tool_name: String,
    pub input: Value,
    pub tool_use_id: String,
}

pub struct BridgePermissionResponse {
    pub behavior: PermissionBehavior,   // Allow | Deny
    pub updated_input: Option<Value>,
    pub updated_permissions: Option<Vec<PermissionUpdate>>,
    pub message: Option<String>,
}
```

## Session Runner

```rust
pub struct SessionHandle {
    pub session_id: String,
    pub done: JoinHandle<SessionDoneStatus>,
    pub activities: VecDeque<SessionActivity>,  // ring buffer ~10
    pub current_activity: Option<SessionActivity>,
    pub last_stderr: VecDeque<String>,          // ring buffer ~10
}

pub struct SessionActivity {
    pub activity_type: ActivityType,  // ToolStart, Text, Result, Error
    pub summary: String,              // e.g. "Reading src/foo.ts"
    pub timestamp: i64,
}

impl SessionHandle {
    pub fn kill(&self);
    pub fn force_kill(&self);
    pub fn write_stdin(&self, data: &str);
    pub fn update_access_token(&self, token: &str);
}
```

## JWT & Authentication

```rust
/// Proactive token refresh before expiry
pub struct TokenRefreshScheduler {
    // Buffer: 5 minutes before expiry
    // Failure retry: 60s delay, max 3 consecutive failures
    // Fallback: 30-minute refresh for long-running sessions
}

pub fn decode_jwt_payload(token: &str) -> Result<Value>;
pub fn decode_jwt_expiry(token: &str) -> Option<i64>;

/// Trusted device enrollment (90-day rolling)
/// Gate: tengu_sessions_elevated_auth_enforcement
/// Storage: secure keychain (memoized)
/// Header: X-Trusted-Device-Token on bridge API calls
pub struct TrustedDeviceManager {
    pub fn enroll(&self) -> Result<String>;  // must happen within 10min of login
    pub fn get_token(&self) -> Option<String>;
}
```

## Message Routing

```rust
/// Inbound message handler with deduplication
pub fn handle_ingress_message(
    data: &str,
    recent_posted_uuids: &mut BoundedUuidSet,   // echo dedup
    recent_inbound_uuids: &mut BoundedUuidSet,   // re-delivery dedup
    on_inbound_message: impl Fn(Message),
    on_permission_response: impl Fn(PermissionResponse),
    on_control_request: impl Fn(SdkControlRequest),
);

/// Server control request subtypes
pub enum ServerControlSubtype {
    Initialize,         // session setup (returns capabilities)
    SetModel,           // change model mid-session
    Interrupt,          // cancel current turn
    SetPermissionMode,  // switch auto/manual
    SetMaxThinkingTokens,
}
```

## Bridge API Client

Endpoints (Environments API):
- `POST /v1/environments/bridge` — register bridge
- `GET /v1/environments/{id}/work/poll` — poll for work
- `POST /v1/sessions/{id}/events` — send permission responses
- `POST /v1/environments/{id}/work/{id}/heartbeat` — lease extension
- `POST /v1/code/sessions/{id}/bridge` — v2: get worker JWT + epoch

Error handling: `BridgeFatalError` with 401/403/410 retry logic.

## File Map

| File | LOC | Purpose |
|------|-----|---------|
| `bridgeMain.ts` | 2,800 | Standalone bridge CLI entry point |
| `replBridge.ts` | 2,400 | Main env-based bridge core |
| `bridgeApi.ts` | 540 | Environments API HTTP client |
| `bridgeMessaging.ts` | 462 | Message routing, control requests |
| `remoteBridgeCore.ts` | 400 | Env-less bridge core (v2) |
| `sessionRunner.ts` | 300 | Child process spawn & activity |
| `replBridgeTransport.ts` | 250 | Transport abstraction (v1/v2) |
| `types.ts` | 262 | Core type definitions |
| `jwtUtils.ts` | 257 | JWT decode, token refresh |
| `trustedDevice.ts` | 211 | Device auth enrollment |
| `createSession.ts` | 200 | Session creation API |
| `initReplBridge.ts` | 150 | REPL wrapper, bootstrap |
| `bridgeConfig.ts` | 49 | Auth/URL resolution |
| `bridgePermissionCallbacks.ts` | 44 | Permission response routing |
| Others | ~600 | Debug, status, UI, attachments |
