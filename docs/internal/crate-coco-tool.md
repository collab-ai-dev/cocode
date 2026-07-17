# coco-tool — Crate Plan

TS source: `Tool.ts`, `services/tools/StreamingToolExecutor.ts`, `services/tools/toolOrchestration.ts`, `tools.ts`

## Dependencies

```
coco-tool depends on:
  - coco-types    (ToolInputSchema, ToolResult, PermissionDecision, Message, etc.)
  - coco-config   (ModelInfo — for tools_for_model filtering by excluded_tools + apply_patch_tool_type)
  - coco-error
  - tokio, tokio-util (CancellationToken)
  - serde_json (Value)

coco-tool does NOT depend on:
  - coco-tools    (no concrete tool implementations — that would be circular)
  - coco-inference (no LLM calls)
  - commands/, skills/, tasks/ (no feature modules — avoids cycles)
  - any app/ crate

Re-exports for convenience:
  pub use coco_types::{ToolInputSchema, ToolResult, ToolProgress, PermissionDecision};
```

## Data Definitions

### Tool Trait (from `Tool.ts`)

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn search_hint(&self) -> Option<&str> { None }  // 3-10 words for ToolSearch

    /// Dynamic description based on input
    async fn description(&self, input: &Value, options: &DescriptionOptions) -> String;

    fn input_schema(&self) -> &ToolInputSchema;
    fn input_json_schema(&self) -> Option<&Value> { None }

    /// Execution — maps to TS call()
    async fn execute(
        &self,
        input: Value,
        context: &ToolUseContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult<Value>, ToolError>;

    /// Permission check (TS checkPermissions)
    async fn check_permissions(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> PermissionDecision { PermissionDecision::Allow }

    /// Concurrency: true = can run in parallel with other safe tools
    fn is_concurrency_safe(&self, input: &Value) -> bool { false }
    fn is_read_only(&self, input: &Value) -> bool { false }
    fn is_destructive(&self, input: &Value) -> bool { false }
    fn is_enabled(&self) -> bool { true }
    fn should_defer(&self) -> bool { false }    // lazy-loaded via ToolSearch
    fn always_load(&self) -> bool { false }

    /// Per-tool persistence threshold. Read by `app/query/src/tool_outcome_builder.rs`
    /// after `execute()` returns — this trait method is the source value for
    /// Level 1 of the [Tool Result Budget plan](tool-result-budget-plan.md).
    /// `Chars(n)` caps the per-tool output and the caller clamps `n` against
    /// `DEFAULT_MAX_RESULT_SIZE_CHARS=50_000` (TS parity).
    /// `Unbounded` opts out of persistence entirely (TS `Infinity` semantics)
    /// — e.g. `Read` returns it because the file content is canonical.
    fn max_result_size_bound(&self) -> ResultSizeBound { ResultSizeBound::Chars(100_000) }

    /// For MCP tools
    fn mcp_info(&self) -> Option<&McpToolInfo> { None }

    /// Interrupt behavior when user cancels
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Cancel }

    /// Input validation before execution (runs BEFORE hooks)
    async fn validate_input(&self, input: &Value, context: &ToolUseContext) -> ValidationResult {
        ValidationResult::Valid
    }

    /// Idempotency: are two inputs functionally equivalent?
    /// Used to skip re-execution of identical tool calls.
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool { false }

    /// Prepare closures for hook pattern matching.
    /// Called before hooks run so matchers can compare tool input against patterns.
    fn prepare_permission_matcher(&self, input: &Value) -> Option<Value> { None }

    /// Representation for auto-mode security classifier (Haiku).
    /// Return empty string to skip classifier.
    fn to_auto_classifier_input(&self, input: &Value) -> String { String::new() }

    /// File path this tool operates on (for file-based tools).
    fn get_path(&self, input: &Value) -> Option<String> { None }

    /// Inject legacy fields into input (idempotent backfill).
    fn backfill_observable_input(&self, input: &mut Value) {}

    /// Output schema for structured output validation.
    fn output_schema(&self) -> Option<&Value> { None }

    /// User-facing prompt text for tool description.
    async fn prompt(&self, options: &DescriptionOptions) -> String { self.description(&Value::Null, options).await }

    /// Is this an LSP tool?
    fn is_lsp(&self) -> bool { false }

    /// Strict schema mode flag.
    fn strict(&self) -> bool { false }

    /// Map tool result to API-compatible tool_result block.
    fn map_tool_result_to_block(&self, output: &Value, tool_use_id: &str) -> Value;

    /// Context modification after execution completes.
    /// Called by StreamingToolExecutor with the tool result.
    fn modify_context_after(&self, result: &ToolResult<Value>, ctx: &mut ToolUseContext) {}

    // --- v2: UI/UX methods (not needed for core agent behavior) ---
    // fn is_search_or_read_command(&self, input: &Value) -> Option<SearchReadInfo> { None }
    // fn is_open_world(&self, input: &Value) -> bool { false }
    // fn requires_user_interaction(&self) -> bool { false }
    // fn is_transparent_wrapper(&self) -> bool { false }
    // fn extract_search_text(&self, output: &Value) -> Option<String> { None }
    // fn get_tool_use_summary(&self, input: &Value) -> Option<String> { None }
    // fn get_activity_description(&self, input: &Value) -> Option<String> { None }
    // fn user_facing_name_bg_color(&self, input: &Value) -> Option<String> { None }
}

pub enum InterruptBehavior { Cancel, Block }

/// Tool input validation result.
///
/// error_code is tool-local (no global registry), used for OTel telemetry only.
/// The model sees `message`; error_code is opaque to the model.
/// Analytics key: (tool_name, error_code) — each tool defines its own code semantics.
///
/// TS reference: Tool.ts:95-101, toolExecution.ts:683-732
pub enum ValidationResult {
    Valid,
    Invalid { message: String, error_code: Option<i32> },
}

/// Common error_code conventions (tool-local, not enforced):
///   0 = security/sensitive data violation
///   1 = resource not found / does not exist
///   2 = resource type mismatch (not a directory, etc.)
///   3 = invalid request variant / discriminated union mismatch
///   4 = resource too large / limit exceeded
///   5 = feature disabled or incompatible mode
///   6 = remote resource not found / not discovered
///   7-9 = tool-specific parameter format/bounds errors
///   10+ = tool-specific (e.g., BashTool: blocked sleep pattern)
```

### ToolUseContext (from `Tool.ts` — 40+ fields)

```rust
pub struct ToolUseContext {
    // === Options (from QueryEngineConfig) ===
    pub tools: Arc<Vec<Arc<dyn Tool>>>,
    pub commands: Arc<Vec<Command>>,
    pub main_loop_model: String,
    pub thinking_level: Option<ThinkingLevel>,
    pub mcp_clients: Vec<McpConnection>,
    pub mcp_resources: HashMap<String, Vec<ServerResource>>,
    pub is_non_interactive: bool,
    pub agent_definitions: Vec<AgentDefinition>,
    pub max_budget_usd: Option<f64>,
    pub custom_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub query_source: Option<QuerySource>,
    pub refresh_tools: Option<Arc<dyn Fn() -> Vec<Arc<dyn Tool>> + Send + Sync>>,
    pub debug: bool,
    pub verbose: bool,

    // === Core state ===
    pub abort_controller: CancellationToken,
    pub messages: Vec<Message>,
    pub read_file_state: Arc<RwLock<FileStateCache>>,
    pub app_state: Arc<RwLock<AppState>>,
    pub permission_context: ToolPermissionContext,

    // === Agent identity ===
    pub tool_use_id: Option<String>,
    pub agent_id: Option<AgentId>,
    pub agent_type: Option<AgentTypeId>,

    // === File tracking ===
    pub file_reading_limits: Option<FileReadingLimits>,
    pub glob_limits: Option<GlobLimits>,
    // NOTE: `content_replacement_state` (Level 2 budget, was named here) is
    // re-routed to `app/query::QueryEngine` per
    // [`tool-result-budget-plan.md`](tool-result-budget-plan.md). It is NOT
    // a per-tool-invocation field — Level 2 enforcement runs once per turn
    // before the API call, not inside each tool's execute(). The current
    // `ToolUseContext` in `core/tool-runtime/src/context.rs` does NOT carry
    // this field; do not re-add it without revisiting the plan.
    //
    // Level 1 (per-tool persistence) is invoked by `tool_outcome_builder`
    // AFTER `Tool::execute()` returns, reading `Tool::max_result_size_bound()`.
    // `ToolUseContext.tool_result_session_dir: Option<PathBuf>` is the
    // session-scoped persistence root (None disables persistence).

    // === State mutation callbacks ===
    // These are closures because ToolUseContext is shared across tools
    // and must not hold direct mutable references.
    pub set_app_state: Arc<dyn Fn(Box<dyn FnOnce(&mut AppState)>) + Send + Sync>,
    pub set_app_state_for_tasks: Option<Arc<dyn Fn(Box<dyn FnOnce(&mut AppState)>) + Send + Sync>>,
    pub set_in_progress_tool_use_ids: Arc<dyn Fn(Box<dyn FnOnce(&mut HashSet<String>)>) + Send + Sync>,
    pub set_has_interruptible_tool_in_progress: Option<Arc<dyn Fn(bool) + Send + Sync>>,
    pub set_response_length: Arc<dyn Fn(Box<dyn FnOnce(i64) -> i64>) + Send + Sync>,
    pub set_stream_mode: Option<Arc<dyn Fn(SpinnerMode) + Send + Sync>>,
    pub set_sdk_status: Option<Arc<dyn Fn(SdkStatus) + Send + Sync>>,

    // === Event callbacks ===
    pub handle_elicitation: Option<Arc<dyn Fn(String, ElicitRequestParams, CancellationToken) -> BoxFuture<ElicitResult> + Send + Sync>>,
    pub add_notification: Option<Arc<dyn Fn(Notification) + Send + Sync>>,
    pub append_system_message: Option<Arc<dyn Fn(SystemMessage) + Send + Sync>>,
    pub send_os_notification: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    pub on_compact_progress: Option<Arc<dyn Fn(CompactProgressEvent) + Send + Sync>>,
    pub push_api_metrics_entry: Option<Arc<dyn Fn(f64) + Send + Sync>>,
    pub open_message_selector: Option<Arc<dyn Fn() + Send + Sync>>,
    pub request_prompt: Option<Arc<dyn Fn(String, Option<String>) -> BoxFuture<PromptResponse> + Send + Sync>>,

    // === File history + attribution ===
    pub update_file_history_state: Arc<dyn Fn(Box<dyn FnOnce(&mut FileHistoryState)>) + Send + Sync>,
    pub update_attribution_state: Arc<dyn Fn(Box<dyn FnOnce(&mut AttributionState)>) + Send + Sync>,
    pub set_conversation_id: Option<Arc<dyn Fn(Uuid) + Send + Sync>>,

    // === Tracking sets (session-scoped dedup) ===
    pub nested_memory_attachment_triggers: Option<Arc<RwLock<HashSet<String>>>>,
    pub loaded_nested_memory_paths: Option<Arc<RwLock<HashSet<String>>>>,
    pub dynamic_skill_dir_triggers: Option<Arc<RwLock<HashSet<String>>>>,
    pub discovered_skill_names: Option<Arc<RwLock<HashSet<String>>>>,

    // === Decision tracking ===
    pub tool_decisions: Option<Arc<RwLock<HashMap<String, ToolDecision>>>>,
    pub query_tracking: Option<QueryChainTracking>,
    pub local_denial_tracking: Option<Arc<tokio::sync::Mutex<DenialTracker>>>,
    // ^^ Per-context auto-mode denial tracker. `Some` on fork contexts
    // (isolated from parent); `None` on main session (falls back to the
    // engine-level session tracker at the classifier callsite). TS:
    // `permissions.ts:553-558` (`context.localDenialTracking ?? appState.denialTracking`).

    // === Flags ===
    pub user_modified: bool,
    pub require_can_use_tool: bool,
    pub preserve_tool_use_results: bool,

    // === Cached prompt ===
    pub rendered_system_prompt: Option<SystemPrompt>,
    pub critical_system_reminder_experimental: Option<String>,
}

pub struct ToolDecision {
    pub source: String,
    pub decision: ToolDecisionKind, // Accept, Reject
    pub timestamp: i64,
}

pub enum ToolDecisionKind { Accept, Reject }
```

### StreamingToolExecutor (from `services/tools/StreamingToolExecutor.ts`)

```rust
/// Tools execute DURING API streaming, not after.
/// As the API streams tool_use blocks, add_tool() queues them immediately.
/// Safe tools start executing while the API is still streaming.
/// Results are yielded in tool-received order (not completion order).
pub struct StreamingToolExecutor {
    tracked_tools: Vec<TrackedTool>,
    results_tx: mpsc::Sender<ToolExecutionResult>,
    discarded: bool,
}

struct TrackedTool {
    id: String,
    block: ToolUseBlock,
    assistant_message: AssistantMessage,
    status: ToolStatus,
    is_concurrency_safe: bool,
    promise: Option<JoinHandle<()>>,
    results: Option<Vec<Message>>,
    pending_progress: Vec<Message>,      // yielded immediately, not buffered
    context_modifiers: Vec<Box<dyn FnOnce(&mut ToolUseContext)>>,
}

enum ToolStatus { Queued, Executing, Completed, Yielded }

/// Synthetic errors generated when tool execution is interrupted.
pub enum SyntheticToolError {
    /// Another tool in the same batch failed
    SiblingError { failed_tool: String },
    /// User pressed interrupt (ESC/Ctrl-C)
    UserInterrupted,
    /// Streaming fallback: retry discarded previous results
    StreamingFallback,
}

impl StreamingToolExecutor {
    /// Queue a tool for execution. Called as API streams tool_use blocks.
    /// Safe tools start immediately if only safe tools are running.
    pub fn add_tool(&mut self, block: ToolUseBlock, assistant_msg: AssistantMessage);

    /// Can a tool with this safety level start now?
    fn can_execute_tool(&self, is_concurrency_safe: bool) -> bool;

    /// Process the queue: start tools when conditions allow.
    async fn process_queue(&mut self);

    /// Execute a single tool. Updates status, handles results + context modifiers.
    async fn execute_tool(&mut self, tool: &mut TrackedTool);

    /// Yield completed results in tool-received order.
    /// Progress messages are yielded immediately (not buffered).
    pub async fn get_remaining_results(&mut self) -> impl Stream<Item = Message>;

    /// Abandon pending tools (on streaming fallback).
    /// Generates SyntheticToolError::StreamingFallback for discarded tools.
    pub fn discard(&mut self);
}

/// Context modifier stacking: modifiers from tool results are collected
/// and applied in tool-received order after all tools in a batch complete.
/// This preserves deterministic context mutation regardless of completion order.
```

### Tool Registry (from `tools.ts`)

```rust
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Load all tools with feature gating
    pub fn new(config: &ToolRegistryConfig) -> Self;

    pub fn find_by_name(&self, name: &str) -> Option<&Arc<dyn Tool>>;

    /// Feature-gated tools:
    /// - CronCreate/Delete/List (in ScheduleCronTool/): AGENT_TRIGGERS
    /// - RemoteTrigger: AGENT_TRIGGERS_REMOTE
    /// - Sleep: PROACTIVE || KAIROS
    /// - Team tools: AGENT_SWARMS
    /// - SyntheticOutputTool: SDK non-interactive sessions only
    /// - REPLTool: REPL mode (ant + cli entrypoint, opt-out via CLAUDE_CODE_REPL=0)
    /// - PowerShellTool: Windows platform only
    pub fn enabled_tools(&self) -> Vec<&Arc<dyn Tool>>;
}
```

## Core Logic

### Tool Orchestration (from `services/tools/toolOrchestration.ts`)

```rust
/// Execute tool calls from assistant response.
/// Partitions into batches: (1 unsafe) or (N safe concurrent).
/// Max concurrency: CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY env, default 10.
pub async fn run_tools(
    tool_uses: &[ToolUseBlock],
    assistant_msg: &AssistantMessage,
    context: &ToolUseContext,
    cancel: CancellationToken,
) -> Vec<ToolExecutionResult>;

/// Partition tool calls into safe/unsafe batches
fn partition_tool_calls(
    tool_uses: &[ToolUseBlock],
    registry: &ToolRegistry,
) -> Vec<ToolBatch>;

enum ToolBatch {
    SingleUnsafe(ToolUseBlock),
    ConcurrentSafe(Vec<ToolUseBlock>),
}
```

### Tool Execution Error Handling (from `services/tools/toolExecution.ts`)

```rust
/// Errors during tool execution. Wraps cocode-error StatusCode.
///
/// Unlike ValidationResult (pre-execution, tool-local codes for telemetry),
/// ToolError uses StatusCode (system-level classification for retry/routing).
#[stack_trace_debug]
#[derive(Snafu)]
pub enum ToolError {
    #[snafu(display("Tool not found: {name}"))]
    NotFound { name: String, #[snafu(implicit)] location: Location },

    #[snafu(display("Invalid input: {message}"))]
    InvalidInput { message: String, #[snafu(implicit)] location: Location },

    #[snafu(display("Execution failed: {message}"))]
    ExecutionFailed {
        message: String,
        #[snafu(source)] source: BoxedError,
        #[snafu(implicit)] location: Location,
    },

    #[snafu(display("Permission denied: {message}"))]
    PermissionDenied { message: String, #[snafu(implicit)] location: Location },

    #[snafu(display("Tool execution timed out"))]
    Timeout { #[snafu(implicit)] location: Location },

    #[snafu(display("Cancelled by user"))]
    Cancelled { #[snafu(implicit)] location: Location },
}

impl ErrorExt for ToolError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } => StatusCode::NotFound,
            Self::InvalidInput { .. } => StatusCode::InvalidArguments,
            Self::ExecutionFailed { source, .. } => source.status_code(),
            Self::PermissionDenied { .. } => StatusCode::PermissionDenied,
            Self::Timeout { .. } => StatusCode::Timeout,
            Self::Cancelled { .. } => StatusCode::Cancelled,
        }
    }
}

/// Error formatting for model consumption.
/// TS: toolErrors.ts formatError()
///
/// Rules:
/// - AbortError → fixed INTERRUPT_MESSAGE
/// - ShellError → structured: exit_code + stderr + stdout
/// - Other → output_msg() (from ErrorExt cause chain)
/// - Truncate at 10000 chars: first 5k + "... [N truncated] ..." + last 5k
pub fn format_tool_error(error: &ToolError) -> String;

/// Error classification for OTel telemetry.
/// TS: toolExecution.ts classifyToolError()
///
/// Strategy (in order):
/// 1. telemetry_msg() if overridden (sanitized, no paths/code)
/// 2. StatusCode name (e.g., "IoError", "PermissionDenied")
/// 3. Fallback: "Unknown"
///
/// Unlike TS which needs errno preservation and minification-safety,
/// Rust enum variant names are stable at runtime.
pub fn classify_tool_error(error: &ToolError) -> &'static str;
```

### OTel 遥测事件 (from `toolExecution.ts:691-716`)

```rust
/// 工具执行完成时发送的遥测事件。
/// validation error 和 execution error 都通过此事件记录。
pub struct ToolUseEvent {
    pub tool_id: ToolId,
    pub success: bool,
    pub duration_ms: i64,
    // ValidationResult 失败时填充
    pub validation_error_code: Option<i32>,
    pub validation_error_message: Option<String>,
    // ToolError 时填充
    pub error_class: Option<String>,           // classify_tool_error() 结果
    pub error_status_code: Option<StatusCode>,
    // 上下文
    pub is_mcp: bool,
    pub is_concurrency_safe: bool,
    pub query_chain_id: Option<String>,
    pub query_depth: Option<i32>,
}
```

### Concurrency Model

```
Assistant response with tool_use blocks:
  [Read(file1), Read(file2), Bash("ls"), Edit(file3)]
       safe         safe       unsafe       unsafe

Partitioned into batches:
  Batch 1: [Read(file1), Read(file2)]  — concurrent (both safe)
  Batch 2: [Bash("ls")]               — serial (unsafe)
  Batch 3: [Edit(file3)]              — serial (unsafe)

Execution:
  Batch 1 runs in parallel → results
  Batch 2 runs alone → result
  Batch 3 runs alone → result
  All results appended to messages
```
