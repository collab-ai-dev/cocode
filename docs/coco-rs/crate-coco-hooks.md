# coco-hooks — Crate Plan

TS source: `schemas/hooks.ts`, `utils/hooks/` (15+ files, 3.7K LOC)

## Dependencies

```
coco-hooks depends on:
  - coco-types (HookEventType, HookResult), coco-config (Settings — hooks section as Value)
  - coco-tool (ToolUseContext — for hook execution context)
  - tokio, reqwest (HTTP hooks)

coco-hooks does NOT depend on:
  - coco-tools, coco-query, coco-inference, any app/ crate
```

## Data Definitions

```rust
/// Maps hook event types to their matcher arrays.
/// TS uses `Partial<Record<HookEvent, HookMatcher[]>>` — a sparse map.
/// In Rust: HashMap keyed by HookEventType. All 27 event types are valid keys.
/// Missing entries mean no hooks configured for that event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksSettings {
    #[serde(flatten)]
    pub hooks: HashMap<HookEventType, Vec<HookMatcher>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    /// Tool name pattern for PreToolUse/PostToolUse. e.g. "Write", "Bash(git *)"
    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

/// TS discriminates via `type` field: "command", "prompt", "http", "agent".
/// In coco-rs the `once` / `async` / `if` / `statusMessage` fields are
/// hoisted to the outer `HookDefinition` (TS exposes them per-variant
/// in the schema; the loader reads them from raw JSON). Each variant
/// now carries its own `timeout_ms` field — TS allows `timeout` on
/// every handler kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookHandler {
    Command { command: String, timeout_ms: Option<i64>, shell: Option<String> },
    Prompt { prompt: String, model: Option<String>, timeout_ms: Option<i64> },
    Http { url: String, headers: Option<HashMap<String, String>>, timeout_ms: Option<i64>, allowed_env_vars: Vec<String> },
    Agent { prompt: String, model: Option<String>, timeout_ms: Option<i64> },
}
```

## Core Logic

```rust
pub struct HookExecutor;

impl HookExecutor {
    /// Execute hooks matching an event.
    /// 3-phase pipeline:
    ///   1. Parse input: evaluate `if` condition (permission rule syntax), match tool name
    ///   2. Run hook command: bash/prompt/http/agent (parallel if async=true)
    ///   3. Process response: continue, block, modify input, permission decision
    ///
    /// Hook responses can:
    ///   - Allow/deny tool execution (for pre_tool_use)
    ///   - Modify tool input (for pre_tool_use)
    ///   - Inject messages into conversation
    ///   - Return structured permission decisions
    pub async fn run_hooks(
        event: HookEventType,
        tool_id: Option<&ToolId>,
        input: &Value,
        context: &ToolUseContext,
        settings: &HooksSettings,
    ) -> Vec<HookResult>;
}

/// Async hook tracking: hooks with async=true run in background,
/// polled for completion at configurable intervals.
/// Timeout: per-hook timeout field (default: 60s).
/// Once: hooks with once=true run only on first match per session.
pub struct AsyncHookRegistry {
    pending: HashMap<String, AsyncHookState>,
}

impl AsyncHookRegistry {
    pub fn register(&mut self, hook_id: &str, timeout: Duration);
    pub fn poll(&mut self) -> Vec<HookResult>;
    pub fn cancel(&mut self, hook_id: &str);
}

/// Hook scope priority (highest wins, matches TS numeric priority):
///   Session(highest) > Local > Project > User > Plugin > Builtin(lowest)
pub enum HookScope { Session, Local, Project, User, Plugin, Builtin }
```

### Hook Input/Output Mutations

```rust
/// PreToolUse hooks can modify tool input parameters.
/// Returned via hookSpecificOutput.updatedInput in hook JSON output.
/// Type: Record<String, Value> — replaces/merges with original tool input.
///
/// PostToolUse hooks can modify MCP tool output.
/// Returned via hookSpecificOutput.updatedMCPToolOutput in hook JSON output.
/// Type: Value (unknown) — supports any MCP tool return format.

pub struct SingleHookResult {
    pub outcome: HookOutcome,              // Success, Blocking, NonBlockingError, Cancelled
    pub message: Option<Message>,
    pub status_message: Option<String>,    // custom UI status during hook execution
    pub updated_input: Option<Value>,      // PreToolUse: modified tool input
    pub updated_mcp_tool_output: Option<Value>, // PostToolUse: modified MCP output
    pub permission_behavior: Option<PermissionBehavior>, // ask/deny/allow/passthrough
    pub additional_context: Option<String>,
    pub retry: bool,
}
```

### Function Hooks (Session-Scoped Callbacks)

```rust
/// In-memory callbacks that cannot be persisted to settings.json.
/// Execute as Rust closures within the agent runtime.
/// Used by skill-improvement detection and post-model-sampling analysis.
///
/// Timeout: 5000ms default.
/// Returns true if check passes, false to block.
pub struct FunctionHook {
    pub id: Option<String>,
    pub timeout: Duration,
    pub callback: Box<dyn Fn(&[Message], CancellationToken) -> BoxFuture<bool> + Send + Sync>,
    pub error_message: String,
    pub status_message: Option<String>,
}
```

### asyncRewake Protocol

```rust
/// Hooks with async_rewake=true run in background WITHOUT blocking.
/// On process exit code 2 (special signal):
///   - Enqueue as task-notification with mode 'task-notification'
///   - Wake model via queue processor (if idle) or inject via attachments (if busy)
///   - Message wrapped in <system-reminder> tags
/// Exit code 0: success (no rewake). Other: error (no rewake).
pub struct AsyncRewakeHook {
    pub hook_name: String,
    pub async_rewake: bool,
}
```
