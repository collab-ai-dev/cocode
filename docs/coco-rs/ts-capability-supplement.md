# cocode-rs: TS Capability Supplement Design Document

> Based on analysis conclusions from event-system-design.md and sdk-comparison.md
> Principle: Preserve the cocode-rs three-layer CoreEvent architecture; supplement TS capabilities at the ServerNotification / ClientRequest / Params layers

---

## Table of Contents

1. [P0: SDK Consumer Parity (4 items)](#1-p0-sdk-consumer-parity)
2. [P1: Control Protocol Completeness (7 items)](#2-p1-control-protocol-completeness)
3. [P2: Nice-to-Have Notifications (6 items)](#3-p2-nice-to-have-notifications)
4. [Schema Generation Update](#4-schema-generation-update)
5. [Python SDK Client Update](#5-python-sdk-client-update)
6. [Implementation Checklist](#6-implementation-checklist)

---

## 1. P0: SDK Consumer Parity

### 1.1 SessionStateChanged Notification

**TS source**: `sdkEventQueue.ts` SessionStateChangedEvent + `sessionState.ts` notifySessionStateChanged()

**TS behavior**: Emitted at 3 points — `running` (turn start), `idle` (turn end + bg drain complete), `requires_action` (waiting for approval/question/elicitation). Delivered via the `enqueueSdkEvent()` side channel, flushed during `drainSdkEvents()`.

**cocode-rs design**: Directly emitted as a ServerNotification, no side channel needed.

#### Type Definitions

```rust
// common/protocol/src/server_notification/notification.rs

// Add to the server_notification_definitions! macro invocation, Session lifecycle group:
/// Session processing state changed (idle → running → requires_action).
SessionStateChanged => "session/stateChanged" (SessionStateChangedParams),
```

```rust
// common/protocol/src/server_notification/notification.rs — params section

/// Processing state of a session, observable by SDK consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Turn completed, waiting for user input.
    Idle,
    /// Agent is actively processing.
    Running,
    /// Waiting for user action (approval, question, elicitation).
    RequiresAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionStateChangedParams {
    pub state: SessionState,
    /// Present when state is `requires_action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<RequiresActionDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RequiresActionDetails {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}
```

#### Emission Points

```rust
// app-server/src/turn_runner.rs

// 1. Running — at the start of a turn
fn run_turn(...) {
    emit(CoreEvent::Protocol(ServerNotification::SessionStateChanged(
        SessionStateChangedParams {
            state: SessionState::Running,
            details: None,
        },
    )));
    // ... run turn ...
}

// 2. RequiresAction — upon receiving ApprovalRequired / QuestionAsked / ElicitationRequested
CoreEvent::Tui(TuiEvent::ApprovalRequired { ref request }) => {
    emit(CoreEvent::Protocol(ServerNotification::SessionStateChanged(
        SessionStateChangedParams {
            state: SessionState::RequiresAction,
            details: Some(RequiresActionDetails {
                tool_name: request.tool_name.clone(),
                action_description: Some(request.description.clone()),
                tool_use_id: request.tool_use_id.clone(),
                request_id: Some(request.request_id.clone()),
            }),
        },
    )));
    // ... send ServerRequest::AskForApproval ...
}

// 3. Idle — after the turn completes and all post-turn events have been processed
// After TurnCompleted, once all post-turn events are handled:
emit(CoreEvent::Protocol(ServerNotification::SessionStateChanged(
    SessionStateChangedParams {
        state: SessionState::Idle,
        details: None,
    },
)));
```

#### Wire Example

```json
{"method": "session/stateChanged", "params": {"state": "running"}}
{"method": "session/stateChanged", "params": {"state": "requires_action", "details": {"tool_name": "Bash", "action_description": "Run: rm -rf /tmp/build"}}}
{"method": "session/stateChanged", "params": {"state": "idle"}}
```

---

### 1.2 Hook Lifecycle Notifications

**TS source**: `hookEvents.ts` HookStartedEvent/ProgressEvent/ResponseEvent + `print.ts:629-674` registerHookEventHandler()

**TS behavior**: Hook execution produces 3-phase events — started (execution begins), progress (1s polling of stdout/stderr), response (execution completes). Delivered via `registerHookEventHandler()` → `structuredIO.write()` direct output.

**cocode-rs current state**: Only has `HookExecuted` (a single completion notification). Needs to be replaced with a 3-phase lifecycle.

#### Type Definitions

```rust
// common/protocol/src/server_notification/notification.rs

// Replace in server_notification_definitions!:
//   HookExecuted => "hook/executed" (HookExecutedParams),
// with these 3:
/// Hook execution has started.
HookStarted => "hook/started" (HookStartedParams),
/// Hook is producing output while running.
HookProgress => "hook/progress" (HookProgressParams),
/// Hook execution completed (success, error, or cancelled).
HookResponse => "hook/response" (HookResponseParams),
```

```rust
// common/protocol/src/server_notification/notification.rs — params section

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HookStartedParams {
    pub hook_id: String,
    pub hook_name: String,
    /// HookEventType as wire string (e.g. "PreToolUse", "PostToolUse").
    pub hook_event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HookProgressParams {
    pub hook_id: String,
    pub hook_name: String,
    pub hook_event: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HookResponseParams {
    pub hook_id: String,
    pub hook_name: String,
    pub hook_event: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub outcome: HookOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HookOutcome {
    Success,
    Error,
    Cancelled,
}
```

#### Emission Points

```rust
// core/hooks (HookRegistry or HookExecutor)

// At hook execution entry:
emit(CoreEvent::Protocol(ServerNotification::HookStarted(HookStartedParams {
    hook_id: hook_id.clone(),
    hook_name: hook.name.clone(),
    hook_event: hook_event_type.as_str().to_string(),
})));

// During hook stdout/stderr poll (1s interval):
emit(CoreEvent::Protocol(ServerNotification::HookProgress(HookProgressParams {
    hook_id: hook_id.clone(),
    hook_name: hook.name.clone(),
    hook_event: hook_event_type.as_str().to_string(),
    stdout: accumulated_stdout.clone(),
    stderr: accumulated_stderr.clone(),
    output: accumulated_output.clone(),
})));

// Upon hook completion:
emit(CoreEvent::Protocol(ServerNotification::HookResponse(HookResponseParams {
    hook_id,
    hook_name: hook.name.clone(),
    hook_event: hook_event_type.as_str().to_string(),
    output,
    stdout,
    stderr,
    exit_code,
    outcome: if cancelled { HookOutcome::Cancelled }
             else if success { HookOutcome::Success }
             else { HookOutcome::Error },
})));
```

#### Migration

- Remove the `HookExecuted` variant and `HookExecutedParams`
- Replace `HookExecutedParams` schema_for in export.rs with the 3 new types
- Change existing code that emits `HookExecuted` to emit `HookStarted` + `HookResponse`

---

### 1.3 Task Params Enhancement

**TS source**: `sdkEventQueue.ts` TaskStartedEvent/TaskProgressEvent/TaskNotificationSdkEvent

**TS fields beyond cocode-rs**: `tool_use_id`, `description`, `prompt`, `workflow_name` (TaskStarted); `usage`, `last_tool_name`, `summary`, `workflow_progress` (TaskProgress); `tool_use_id`, `usage`, `output_file`, `summary` (TaskCompleted)

#### Type Modifications

```rust
// common/protocol/src/server_notification/notification.rs — modify existing params

pub struct TaskStartedParams {
    pub task_id: String,
    pub task_type: String,
    // ── new fields ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

pub struct TaskProgressParams {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    // ── new fields ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

pub struct TaskCompletedParams {
    pub task_id: String,
    pub result: String,
    #[serde(default)]
    pub is_error: bool,
    // ── new fields ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Token/tool usage for a background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TaskUsage {
    pub total_tokens: i64,
    pub tool_uses: i32,
    pub duration_ms: i64,
}
```

#### Backward Compatibility

All new fields are `Option` + `#[serde(default)]`, so existing wire consumers are not broken.

---

### 1.4 SessionResult Enhancement

**TS source**: `coreSchemas.ts` SDKResultSuccessMessage — `num_api_calls`, `modelUsage`, `permission_denials`, `fast_mode_state`, `result` text

#### Type Modifications

```rust
// common/protocol/src/server_notification/notification.rs — modify existing SessionResultParams

pub struct SessionResultParams {
    pub session_id: String,
    pub total_turns: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_cents: Option<i64>,
    pub duration_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_api_ms: Option<i64>,
    pub usage: Usage,
    pub stop_reason: SessionEndedReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    // ── new fields ──
    /// Number of API calls made during the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_api_calls: Option<i32>,
    /// Per-model token usage breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_usage: Option<std::collections::HashMap<String, Usage>>,
    /// Tools that were denied by the user during the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_denials: Vec<PermissionDenialInfo>,
    /// Fast mode state at session end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode_state: Option<FastModeState>,
    /// Final assistant text result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    /// Errors accumulated during the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PermissionDenialInfo {
    pub tool_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FastModeState {
    Off,
    Cooldown,
    On,
}
```

---

## 2. P1: Control Protocol Completeness

### 2.1 mcp/status Request

**TS source**: `print.ts:2957-2960` mcp_status handler + `buildMcpServerStatuses()`

**TS returns**: McpServerStatus array (name, status, serverInfo, error, config, scope, tools, capabilities)

#### Type Definitions

```rust
// app-server-protocol/src/request.rs — ClientRequest enum

/// Query MCP server connection status.
#[serde(rename = "mcp/status")]
McpStatus(McpStatusRequestParams),
```

```rust
// app-server-protocol/src/request.rs — params + response

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpStatusRequestParams {
    /// Filter by server name. If empty, returns all servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpStatusResult {
    pub servers: Vec<McpServerStatusInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpServerStatusInfo {
    pub name: String,
    pub status: McpConnectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<McpServerInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub tool_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionStatus {
    Connected,
    Connecting,
    Failed,
    Disabled,
    NeedsAuth,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpServerInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
```

#### Processor Handler

```rust
// app-server/src/processor.rs — dispatch_request match arm

ClientRequest::McpStatus(params) => {
    let handle = require_session!(conn_id);
    let servers = handle.state.mcp_manager().server_statuses(params.server_name.as_deref());
    let result = McpStatusResult {
        servers: servers.into_iter().map(|s| McpServerStatusInfo {
            name: s.name,
            status: s.status,
            server_info: s.server_info,
            error: s.error,
            tool_count: s.tool_count,
            scope: s.scope,
        }).collect(),
    };
    self.send_response(conn_id, id, serde_json::to_value(result)?).await;
}
```

---

### 2.2 context/usage Request

**TS source**: `print.ts:2961-2978` get_context_usage handler + `collectContextData()`

**TS returns**: ContextData (categories, totalTokens, maxTokens, percentage, model, memoryFiles, mcpTools, agents, skills, messageBreakdown, apiUsage)

#### Type Definitions

```rust
// app-server-protocol/src/request.rs

#[serde(rename = "context/usage")]
ContextUsage(ContextUsageRequestParams),
```

```rust
// app-server-protocol/src/request.rs — params + response

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsageRequestParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsageResult {
    pub total_tokens: i64,
    pub max_tokens: i64,
    pub percentage: f64,
    pub model: String,
    pub categories: Vec<ContextUsageCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default)]
    pub is_auto_compact_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_breakdown: Option<MessageBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsageCategory {
    pub name: String,
    pub tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub is_deferred: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageBreakdown {
    #[serde(default)]
    pub tool_call_tokens: i64,
    #[serde(default)]
    pub tool_result_tokens: i64,
    #[serde(default)]
    pub attachment_tokens: i64,
    #[serde(default)]
    pub assistant_message_tokens: i64,
    #[serde(default)]
    pub user_message_tokens: i64,
}
```

#### Processor Handler

```rust
ClientRequest::ContextUsage(_) => {
    let handle = require_session!(conn_id);
    match handle.state.collect_context_usage().await {
        Ok(result) => self.send_response(conn_id, id, serde_json::to_value(result)?).await,
        Err(e) => self.send_error(conn_id, id, -32000, e.to_string()).await,
    }
}
```

---

### 2.3 mcp/setServers Request

**TS source**: `print.ts:3055-3064` mcp_set_servers handler + `applyMcpServerChanges()` + `handleMcpSetServers()`

**TS behavior**: Hot-updates MCP server configurations — serialized execution (mutex), distinguishes SDK servers vs process-based servers, applies enterprise policy filtering, returns added/removed/errors

#### Type Definitions

```rust
// app-server-protocol/src/request.rs

/// Hot-reload MCP server configurations.
#[serde(rename = "mcp/setServers")]
McpSetServers(McpSetServersRequestParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpSetServersRequestParams {
    /// New MCP server configurations. Servers not in this map are removed.
    pub servers: std::collections::HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpSetServersResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub errors: std::collections::HashMap<String, String>,
}
```

#### Processor Handler

```rust
ClientRequest::McpSetServers(params) => {
    let handle = require_session!(conn_id);
    match handle.state.mcp_manager().reconcile_servers(params.servers).await {
        Ok(result) => self.send_response(conn_id, id, serde_json::to_value(result)?).await,
        Err(e) => self.send_error(conn_id, id, -32000, e.to_string()).await,
    }
}
```

---

### 2.4 mcp/reconnect Request

**TS source**: `print.ts:3133-3205` mcp_reconnect handler + `reconnectMcpServerImpl()`

**TS behavior**: Looks up server config (5 sources), clears keychain cache, reconnects, refreshes tools/commands/resources, registers elicitation handlers

#### Type Definitions

```rust
/// Reconnect a specific MCP server.
#[serde(rename = "mcp/reconnect")]
McpReconnect(McpReconnectRequestParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpReconnectRequestParams {
    pub server_name: String,
}
```

#### Processor Handler

```rust
ClientRequest::McpReconnect(params) => {
    let handle = require_session!(conn_id);
    match handle.state.mcp_manager().reconnect_server(&params.server_name).await {
        Ok(()) => self.send_response(conn_id, id, json!({"status": "ok"})).await,
        Err(e) => self.send_error(conn_id, id, -32000, e.to_string()).await,
    }
}
```

---

### 2.5 mcp/toggle Request

**TS source**: `print.ts:3206-3296` mcp_toggle handler

**TS behavior**: `enabled=false` → persists disabled flag + disconnects + removes tools/commands; `enabled=true` → persists enabled flag + reconnects

#### Type Definitions

```rust
/// Enable or disable a specific MCP server.
#[serde(rename = "mcp/toggle")]
McpToggle(McpToggleRequestParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpToggleRequestParams {
    pub server_name: String,
    pub enabled: bool,
}
```

#### Processor Handler

```rust
ClientRequest::McpToggle(params) => {
    let handle = require_session!(conn_id);
    let result = if params.enabled {
        handle.state.mcp_manager().enable_server(&params.server_name).await
    } else {
        handle.state.mcp_manager().disable_server(&params.server_name).await
    };
    match result {
        Ok(()) => self.send_response(conn_id, id, json!({"status": "ok"})).await,
        Err(e) => self.send_error(conn_id, id, -32000, e.to_string()).await,
    }
}
```

---

### 2.6 plugin/reload Request

**TS source**: `print.ts:3065-3131` reload_plugins handler + `refreshActivePlugins()`

**TS behavior**: Optionally downloads remote settings → refreshes plugins → collects commands/MCP diff/plugins in parallel → returns full new state

#### Type Definitions

```rust
/// Reload all plugins from disk.
#[serde(rename = "plugin/reload")]
PluginReload(PluginReloadRequestParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginReloadRequestParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginReloadResult {
    pub commands: Vec<CommandInfo>,
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub plugins: Vec<PluginInfo>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatusInfo>,
    #[serde(default)]
    pub error_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PluginInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
```

#### Processor Handler

```rust
ClientRequest::PluginReload(_) => {
    let handle = require_session!(conn_id);
    match handle.state.plugin_manager().reload_all().await {
        Ok(result) => {
            let server_statuses = handle.state.mcp_manager().server_statuses(None);
            let response = PluginReloadResult {
                commands: result.commands,
                agents: result.agents,
                plugins: result.plugins,
                mcp_servers: server_statuses,
                error_count: result.error_count,
            };
            self.send_response(conn_id, id, serde_json::to_value(response)?).await;
        }
        Err(e) => self.send_error(conn_id, id, -32000, e.to_string()).await,
    }
}
```

---

### 2.7 config/applyFlags Request

**TS source**: `print.ts:3699-3758` apply_flag_settings handler

**TS behavior**: Merges incoming settings into the flag settings layer, null deletes a key, notifies change detector to reset cache, detects model changes → updates override + injects breadcrumbs

#### Type Definitions

```rust
/// Apply feature flag settings at runtime.
#[serde(rename = "config/applyFlags")]
ConfigApplyFlags(ConfigApplyFlagsRequestParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigApplyFlagsRequestParams {
    /// Settings to merge. Null values delete the key.
    pub settings: serde_json::Value,
}
```

#### Processor Handler

```rust
ClientRequest::ConfigApplyFlags(params) => {
    let handle = require_session!(conn_id);
    let prev_model = handle.state.active_model_id().to_string();

    // Merge into flag settings layer
    handle.state.config_manager().apply_flag_settings(params.settings);

    // Detect model change
    let new_model = handle.state.active_model_id().to_string();
    if new_model != prev_model {
        handle.state.set_model_override(&new_model);
    }

    self.send_response(conn_id, id, json!({"status": "ok"})).await;
}
```

---

## 3. P2: Nice-to-Have Notifications

### 3.1 AuthStatus — REMOVED (not applicable)

~~**TS source**: `SDKAuthStatusMessage`~~

This event is Claude Code's OAuth flow progress indicator. coco-rs handles auth
differently (multi-provider + MCP auth via `McpAuthStatus`), so this ServerNotification
variant is not implemented. See `event-system-design.md` §2.2 note.

### 3.2 LocalCommandOutput

**TS source**: `SDKLocalCommandOutputMessage` — output from commands executed by the user with the REPL `!` prefix

```rust
/// Output from a user-executed local command (REPL `!` prefix).
LocalCommandOutput => "localCommand/output" (LocalCommandOutputParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LocalCommandOutputParams {
    pub content: serde_json::Value,
}
```

### 3.3 FilesPersisted

**TS source**: `SDKFilesPersistedEvent` — notification that files have been uploaded/persisted

```rust
/// Files have been persisted (uploaded/saved).
FilesPersisted => "files/persisted" (FilesPersistedParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FilesPersistedParams {
    pub files: Vec<PersistedFileInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<PersistedFileError>,
    pub processed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PersistedFileInfo {
    pub filename: String,
    pub file_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PersistedFileError {
    pub filename: String,
    pub error: String,
}
```

### 3.4 ElicitationComplete

**TS source**: `SDKElicitationCompleteMessage` — MCP elicitation completed

```rust
/// MCP server elicitation completed.
ElicitationComplete => "elicitation/complete" (ElicitationCompleteParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ElicitationCompleteParams {
    pub mcp_server_name: String,
    pub elicitation_id: String,
}
```

### 3.5 ToolUseSummary

**TS source**: `SDKToolUseSummaryMessage` — tool use summary

```rust
/// Summary of recent tool uses (for streamlined output).
ToolUseSummary => "tool/useSummary" (ToolUseSummaryParams),
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolUseSummaryParams {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preceding_tool_use_ids: Vec<String>,
}
```

### 3.6 RateLimit Enhancement

**TS source**: `SDKRateLimitEventMessage` — `status`, `rateLimitType`, `utilization`

```rust
// Modify existing RateLimitParams, adding fields:

pub struct RateLimitParams {
    // existing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    // ── new fields ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RateLimitStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}
```

---

## 4. Schema Generation Update

### 4.1 New schema_for! entries in export.rs

Add to the `type_schemas` vec in `app-server-protocol/src/export.rs`:

```rust
// P0 types
("SessionStateChangedParams", schema_for!(cocode_app_server_protocol::SessionStateChangedParams)),
("SessionState", schema_for!(cocode_app_server_protocol::SessionState)),
("RequiresActionDetails", schema_for!(cocode_app_server_protocol::RequiresActionDetails)),
("HookStartedParams", schema_for!(cocode_app_server_protocol::HookStartedParams)),
("HookProgressParams", schema_for!(cocode_app_server_protocol::HookProgressParams)),
("HookResponseParams", schema_for!(cocode_app_server_protocol::HookResponseParams)),
("HookOutcome", schema_for!(cocode_app_server_protocol::HookOutcome)),
("TaskUsage", schema_for!(cocode_app_server_protocol::TaskUsage)),
("PermissionDenialInfo", schema_for!(cocode_app_server_protocol::PermissionDenialInfo)),
("FastModeState", schema_for!(cocode_app_server_protocol::FastModeState)),

// P1 types
("McpStatusRequestParams", schema_for!(cocode_app_server_protocol::McpStatusRequestParams)),
("McpStatusResult", schema_for!(cocode_app_server_protocol::McpStatusResult)),
("McpServerStatusInfo", schema_for!(cocode_app_server_protocol::McpServerStatusInfo)),
("McpConnectionStatus", schema_for!(cocode_app_server_protocol::McpConnectionStatus)),
("McpServerInfo", schema_for!(cocode_app_server_protocol::McpServerInfo)),
("ContextUsageRequestParams", schema_for!(cocode_app_server_protocol::ContextUsageRequestParams)),
("ContextUsageResult", schema_for!(cocode_app_server_protocol::ContextUsageResult)),
("ContextUsageCategory", schema_for!(cocode_app_server_protocol::ContextUsageCategory)),
("MessageBreakdown", schema_for!(cocode_app_server_protocol::MessageBreakdown)),
("McpSetServersRequestParams", schema_for!(cocode_app_server_protocol::McpSetServersRequestParams)),
("McpSetServersResult", schema_for!(cocode_app_server_protocol::McpSetServersResult)),
("McpReconnectRequestParams", schema_for!(cocode_app_server_protocol::McpReconnectRequestParams)),
("McpToggleRequestParams", schema_for!(cocode_app_server_protocol::McpToggleRequestParams)),
("PluginReloadRequestParams", schema_for!(cocode_app_server_protocol::PluginReloadRequestParams)),
("PluginReloadResult", schema_for!(cocode_app_server_protocol::PluginReloadResult)),
("PluginInfo", schema_for!(cocode_app_server_protocol::PluginInfo)),
("ConfigApplyFlagsRequestParams", schema_for!(cocode_app_server_protocol::ConfigApplyFlagsRequestParams)),

// P2 types
("LocalCommandOutputParams", schema_for!(cocode_app_server_protocol::LocalCommandOutputParams)),
("FilesPersistedParams", schema_for!(cocode_app_server_protocol::FilesPersistedParams)),
("PersistedFileInfo", schema_for!(cocode_app_server_protocol::PersistedFileInfo)),
("PersistedFileError", schema_for!(cocode_app_server_protocol::PersistedFileError)),
("ElicitationCompleteParams", schema_for!(cocode_app_server_protocol::ElicitationCompleteParams)),
("ToolUseSummaryParams", schema_for!(cocode_app_server_protocol::ToolUseSummaryParams)),
("RateLimitStatus", schema_for!(cocode_app_server_protocol::RateLimitStatus)),
```

### 4.2 Remove old schema_for!

```rust
// Remove:
("HookExecutedParams", schema_for!(cocode_app_server_protocol::HookExecutedParams)),
```

### 4.3 Regenerate

```bash
# 1. Export JSON Schema
cd cocode-rs && cargo run --bin export-app-server-schema

# 2. Copy to SDK
cd ../cocode-sdk && bash scripts/generate_schemas.sh

# 3. Generate Python types
bash scripts/generate_python.sh
```

---

## 5. Python SDK Client Update

### 5.1 New Client Methods

```python
# cocode-sdk/python/src/cocode_sdk/client.py

async def mcp_status(self, server_name: str | None = None) -> dict[str, Any]:
    """Query MCP server connection status."""
    request = McpStatusRequest(
        params=McpStatusRequest.McpStatusRequestParams(server_name=server_name)
    )
    # Request-response correlation is needed here — the current client is fire-and-forget events()
    # The transport needs to be extended to support JSON-RPC response correlation
    await self._transport.send_line(request.model_dump_json())
    return await self._wait_for_response(request.id)

async def context_usage(self) -> dict[str, Any]:
    """Get context window usage breakdown."""
    request = ContextUsageRequest(params=ContextUsageRequest.ContextUsageRequestParams())
    await self._transport.send_line(request.model_dump_json())
    return await self._wait_for_response(request.id)

async def mcp_set_servers(
    self, servers: dict[str, McpServerConfig]
) -> dict[str, Any]:
    """Hot-reload MCP server configurations."""
    request = McpSetServersRequest(
        params=McpSetServersRequest.McpSetServersRequestParams(
            servers={k: v.model_dump(exclude_none=True) for k, v in servers.items()}
        )
    )
    await self._transport.send_line(request.model_dump_json())
    return await self._wait_for_response(request.id)

async def mcp_reconnect(self, server_name: str) -> None:
    """Reconnect a specific MCP server."""
    request = McpReconnectRequest(
        params=McpReconnectRequest.McpReconnectRequestParams(server_name=server_name)
    )
    await self._transport.send_line(request.model_dump_json())

async def mcp_toggle(self, server_name: str, enabled: bool) -> None:
    """Enable or disable a specific MCP server."""
    request = McpToggleRequest(
        params=McpToggleRequest.McpToggleRequestParams(
            server_name=server_name, enabled=enabled
        )
    )
    await self._transport.send_line(request.model_dump_json())

async def reload_plugins(self) -> dict[str, Any]:
    """Reload all plugins from disk."""
    request = PluginReloadRequest(params=PluginReloadRequest.PluginReloadRequestParams())
    await self._transport.send_line(request.model_dump_json())
    return await self._wait_for_response(request.id)

async def apply_flag_settings(self, settings: dict[str, Any]) -> None:
    """Apply feature flag settings at runtime."""
    request = ConfigApplyFlagsRequest(
        params=ConfigApplyFlagsRequest.ConfigApplyFlagsRequestParams(settings=settings)
    )
    await self._transport.send_line(request.model_dump_json())
```

### 5.2 Request-Response Correlation

The current Python SDK `events()` is a pure notification stream and does not support request-response correlation. P1 items mcp/status, context/usage, and plugin/reload require responses.

**Approach**: Extend the transport layer to support JSON-RPC response routing.

```python
# cocode-sdk/python/src/cocode_sdk/_internal/transport/subprocess_cli.py

class SubprocessCLITransport:
    def __init__(self, ...):
        ...
        self._pending_requests: dict[str, asyncio.Future[dict]] = {}
        self._request_counter = 0

    def _next_request_id(self) -> str:
        self._request_counter += 1
        return str(self._request_counter)

    async def send_request(self, method: str, params: dict) -> dict:
        """Send a JSON-RPC request and await the response."""
        request_id = self._next_request_id()
        future: asyncio.Future[dict] = asyncio.get_event_loop().create_future()
        self._pending_requests[request_id] = future

        msg = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        await self.send_line(json.dumps(msg))
        return await future

    async def _dispatch_message(self, data: dict) -> ServerNotification | None:
        """Route incoming message to pending request or notification."""
        if "id" in data and "result" in data:
            # JSON-RPC response
            rid = str(data["id"])
            if rid in self._pending_requests:
                self._pending_requests.pop(rid).set_result(data["result"])
            return None
        if "id" in data and "error" in data:
            # JSON-RPC error
            rid = str(data["id"])
            if rid in self._pending_requests:
                self._pending_requests.pop(rid).set_exception(
                    CocodeSDKError(data["error"].get("message", "Unknown error"))
                )
            return None
        # ServerNotification or ServerRequest
        return _safe_parse_notification(data)
```

---

## 6. Implementation Checklist

### Changed Files

| File | Changes |
|------|---------|
| `common/protocol/src/server_notification/notification.rs` | +1 variant (SessionStateChanged), +3 variants (Hook lifecycle), +5 P2 variants, -1 variant (HookExecuted), +12 param structs, modify 4 existing param structs |
| `app-server-protocol/src/request.rs` | +7 ClientRequest variants, +11 param/response structs |
| `app-server-protocol/src/export.rs` | +35 schema_for entries, -1 entry |
| `app-server/src/processor.rs` | +7 dispatch match arms |
| `app-server/src/turn_runner.rs` | +3 SessionStateChanged emission points |
| `core/hooks` (hook executor) | Replace HookExecuted with HookStarted/Progress/Response emissions |
| `cocode-sdk/python/.../client.py` | +7 client methods |
| `cocode-sdk/python/.../transport/subprocess_cli.py` | +request-response correlation |

### New Type Statistics

| Category | Count |
|----------|-------|
| ServerNotification variants | +9 (3 hook + 1 session state + 5 P2), -1 (HookExecuted) = net +8 |
| Param structs | +17 new |
| Modified param structs | 4 (TaskStarted, TaskProgress, TaskCompleted, SessionResult, RateLimit) |
| ClientRequest variants | +7 |
| Request param structs | +7 |
| Response structs | +5 (McpStatus, ContextUsage, McpSetServers, PluginReload) |
| Enums | +5 (SessionState, HookOutcome, McpConnectionStatus, RateLimitStatus, FastModeState) |

### Implementation Order

```
Phase 1 (P0): SessionStateChanged → Hook lifecycle → Task params → SessionResult params
  ↓ after each step: just pre-commit
Phase 2 (P1): mcp/status → context/usage → mcp/setServers → mcp/reconnect → mcp/toggle → plugin/reload → config/applyFlags
  ↓ after each step: just pre-commit
Phase 3 (P2): LocalCommandOutput → FilesPersisted → ElicitationComplete → ToolUseSummary → RateLimit enhancement (AuthStatus not applicable to coco-rs)
Phase 4: export.rs + schema regen + Python SDK methods + transport correlation
```
