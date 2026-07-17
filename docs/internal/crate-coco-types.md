# coco-types — Crate Plan

TS source: `types/` (7 files + `generated/`), `Tool.ts`, `Task.ts`

Note: `types/message.ts` does not exist as a source file — message types are build-time generated
(imports reference `types/message.js`). The message type definitions below are derived from the
generated output and the runtime usage patterns across the codebase.

## Data Definitions

### Message Types (from `types/message.js` — build-time generated)

**设计决策**: 直接包装 vercel-ai 类型，与 TS 包装 `@anthropic-ai/sdk` 的模式一致。
内部 Message = `LlmMessage` (vercel-ai re-export) + 元数据。
发 API 时直接取 `.message` 字段，零转换。

```rust
// === 版本隔离层：re-export vercel-ai 类型为版本无关的别名 ===
// 所有 crate 通过这些别名引用 vercel-ai 类型，不直接 use vercel_ai_provider::*。
// 升级 vercel-ai v5 时，只改这里的 re-export，其他代码无需修改。
pub use vercel_ai_provider::LanguageModelV4Message as LlmMessage;
pub use vercel_ai_provider::LanguageModelV4Prompt as LlmPrompt;  // Vec<LlmMessage>
pub use vercel_ai_provider::UserContentPart as UserContent;
pub use vercel_ai_provider::AssistantContentPart as AssistantContent;
pub use vercel_ai_provider::ToolContentPart as ToolContent;
pub use vercel_ai_provider::TextPart as TextContent;
pub use vercel_ai_provider::FilePart as FileContent;
pub use vercel_ai_provider::ToolCallPart as ToolCallContent;
pub use vercel_ai_provider::ToolResultPart as ToolResultContent;
pub use vercel_ai_provider::ReasoningPart as ReasoningContent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    System(SystemMessage),
    Attachment(AttachmentMessage),
    ToolResult(ToolResultMessage),
    Progress(ProgressMessage),
    Tombstone(TombstoneMessage),
    ToolUseSummary(ToolUseSummaryMessage),
}

pub struct UserMessage {
    // === LLM API 层（发 API 时直接用这个）===
    pub message: LlmMessage,  // User variant, content: Vec<UserContent>

    // === 内部元数据（不发到 API）===
    pub uuid: Uuid,
    pub timestamp: String,
    pub is_meta: bool,               // hidden from UI, visible to model
    pub is_visible_in_transcript_only: bool,
    pub is_virtual: bool,            // not sent to API
    pub is_compact_summary: bool,
    pub permission_mode: Option<PermissionMode>,
    pub origin: Option<MessageOrigin>,
}

pub struct AssistantMessage {
    // === LLM API 层 ===
    pub message: LlmMessage,  // Assistant variant, content: Vec<AssistantContent>

    // === 内部元数据 ===
    pub uuid: Uuid,
    pub model: String,
    pub stop_reason: Option<StopReason>,
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    pub request_id: Option<String>,
    pub api_error: Option<ApiError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason { EndTurn, MaxTokens, StopSequence, ToolUse }

pub struct ProgressMessage {
    pub tool_use_id: String,
    pub data: Value,
    pub parent_message_uuid: Option<Uuid>,
}

pub struct TombstoneMessage {
    pub uuid: Uuid,
    pub original_kind: MessageKind,
}

/// Which message variant was tombstoned. Mirrors `Message` enum variants 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    User, Assistant, System, Attachment, ToolResult,
    Progress, Tombstone, ToolUseSummary,
}

pub struct ToolUseSummaryMessage {
    pub uuid: Uuid,
    pub tool_id: ToolId,
    pub summary: String,
}

/// System messages have sub-types for different notification kinds.
/// All system messages are `role: "user"` with `is_meta: true` for the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemMessage {
    Informational(SystemInformationalMessage),
    ApiError(SystemAPIErrorMessage),
    CompactBoundary(SystemCompactBoundaryMessage),
    MicrocompactBoundary(SystemMicrocompactBoundaryMessage),
    LocalCommand(SystemLocalCommandMessage),
    PermissionRetry(SystemPermissionRetryMessage),
    BridgeStatus(SystemBridgeStatusMessage),
    MemorySaved(SystemMemorySavedMessage),
    AwaySummary(SystemAwaySummaryMessage),
    AgentsKilled(SystemAgentsKilledMessage),
    ApiMetrics(SystemApiMetricsMessage),
    StopHookSummary(SystemStopHookSummaryMessage),
    TurnDuration(SystemTurnDurationMessage),
    ScheduledTaskFire(SystemScheduledTaskFireMessage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageLevel { Info, Warning, Error }

pub struct SystemInformationalMessage {
    pub uuid: Uuid,
    pub level: SystemMessageLevel,
    pub title: String,
    pub message: String,
}

pub struct SystemAPIErrorMessage {
    pub uuid: Uuid,
    pub error: String,
    pub status_code: Option<i32>,
}

pub struct SystemCompactBoundaryMessage {
    pub uuid: Uuid,
    pub tokens_before: i64,
    pub tokens_after: i64,
}

pub struct SystemMicrocompactBoundaryMessage {
    pub uuid: Uuid,
}

pub struct SystemLocalCommandMessage {
    pub uuid: Uuid,
    pub command: String,
    pub output: String,
}

// Other system message variants follow the same pattern:
// SystemPermissionRetryMessage, SystemBridgeStatusMessage,
// SystemMemorySavedMessage, SystemAwaySummaryMessage,
// SystemAgentsKilledMessage, SystemApiMetricsMessage,
// SystemStopHookSummaryMessage, SystemTurnDurationMessage,
// SystemScheduledTaskFireMessage — detailed during implementation.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    UserInput,
    SystemInjected,
    ToolResult,
    CompactSummary,
    SubagentReply,
}

/// API-normalized message forms (produced by coco-messages normalize_for_api).
/// Distinct from internal Message — strips ALL metadata (uuid, is_meta, usage, etc.)
/// and merges consecutive same-role messages. Only content survives.
/// Role is implicit in the enum variant (no String field needed).
/// Serialized with {"role": "user"/"assistant", "content": [...]} via custom Serialize.
#[derive(Debug, Clone)]
pub enum NormalizedMessage {
    User { content: Vec<ContentBlock> },
    Assistant { content: Vec<ContentBlock> },
}

/// Stream events emitted during API response processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolUseStart { id: String, tool_id: ToolId },
    ToolUseInput { id: String, delta: String },
    ToolUseEnd { id: String },
    RequestStart(RequestStartEvent),
    MessageComplete,
}

pub struct RequestStartEvent {
    pub model: String,
    pub request_id: Option<String>,
}

/// Direction hint for partial compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialCompactDirection { Oldest, Newest }

/// Streaming accumulation types — used by coco-messages, coco-inference, coco-query (3+ crates).
/// Task budget for API output pacing. Used by coco-inference and coco-query.
pub struct TaskBudget {
    pub total: i64,
    pub remaining: Option<i64>,
}

pub struct StreamingToolUse {
    pub id: String,
    pub tool_id: ToolId,
    pub input_json: String,  // Accumulated JSON string
}

pub struct StreamingThinking {
    pub text: String,
}
```

### Event System Types (from `event-system-design.md`)

coco-types owns the 3-layer CoreEvent envelope and all its sub-types. The
complete type catalog and semantics live in `event-system-design.md`; this
section serves as the ownership declaration.

```rust
/// 3-layer event envelope. See event-system-design.md §1.4.
pub enum CoreEvent {
    Protocol(ServerNotification),  // 52 variants, shared with SDK/IDE/TUI
    Stream(AgentStreamEvent),      // 7 variants, fed through StreamAccumulator
    Tui(TuiOnlyEvent),             // 20 variants, dropped by SDK consumers
}

/// Accumulation-layer stream events. Higher-level than the inference-layer
/// `coco_types::StreamEvent`. See event-system-design.md §1.5.
pub enum AgentStreamEvent {
    TextDelta { turn_id: String, delta: String },
    ThinkingDelta { turn_id: String, delta: String },
    ToolUseQueued { call_id: String, name: String, input: Value },
    ToolUseStarted { call_id: String, name: String, batch_id: Option<String> },
    ToolUseCompleted { call_id: String, name: String, output: String, is_error: bool },
    McpToolCallBegin { server: String, tool: String, call_id: String },
    McpToolCallEnd { server: String, tool: String, call_id: String, is_error: bool },
}

/// Semantic thread item. See event-system-design.md §1.6 and §6.2.
pub struct ThreadItem { item_id, turn_id, details: ThreadItemDetails }

pub enum ThreadItemDetails {
    CommandExecution { command, output, exit_code, status }  // Bash
    FileChange { changes: Vec<FileChangeInfo>, status }       // Edit/Write
    WebSearch { query, status }                                // WebSearch
    McpToolCall { server, tool, arguments, result, error, status }  // mcp__*
    Subagent { agent_id, agent_type, description, is_background, result, status }  // Agent/Task
    ToolCall { tool, input, output, is_error, status }        // all others
    AgentMessage { text }
    Reasoning { text }
    Error { message }
}

pub enum ItemStatus { InProgress, Completed, Failed, Declined }

/// Protocol-level notifications (52 variants). See event-system-design.md §2.
pub enum ServerNotification { /* 52 variants with #[serde(rename = "...")] wire methods */ }

/// TUI-exclusive events (20 variants). See event-system-design.md §4.
///
/// Note: the design's §1.7 originally proposed owning this type in coco-tui,
/// but since CoreEvent::Tui references it, the type must live here to avoid
/// cyclic deps. The TUI-only semantic contract is preserved via consumer
/// dispatch rules (SDK/App-Server consumers drop Tui events).
pub enum TuiOnlyEvent { /* 20 variants: overlays, toasts, streaming display */ }
```

**Name collision note**: `coco_types::StreamEvent` (above) is the
**inference-layer** raw LLM stream event consumed by QueryEngine. It is
distinct from `AgentStreamEvent` — the agent-loop-processed stream with
tool lifecycle semantics and MCP tracking. Both coexist in `coco-types`.

### Permission Types (from `types/permissions.ts`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Plan,
    BypassPermissions,
    DontAsk,
    AcceptEdits,
    Auto,     // feature-gated
    Bubble,   // internal: escalate to parent
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehavior { Allow, Deny, Ask }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleSource {
    UserSettings, ProjectSettings, LocalSettings, FlagSettings,
    PolicySettings, CliArg, Command, Session,
}

pub struct PermissionRule {
    pub source: PermissionRuleSource,
    pub behavior: PermissionBehavior,
    pub value: PermissionRuleValue,
}

/// Permission rule value — tool_pattern is a glob/wildcard expression that matches
/// against ToolId wire-format strings. NOT a structured ToolId — because patterns
/// support wildcards ("mcp__slack__*", "*") that ToolId cannot represent.
/// Examples: "Read", "Bash(git *)", "mcp__slack__*", "*"
pub struct PermissionRuleValue {
    pub tool_pattern: String,
    pub rule_content: Option<String>,  // e.g. "git *" (command pattern within tool)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow { updated_input: Option<Value>, feedback: Option<String> },
    Ask { message: String, suggestions: Vec<PermissionUpdate> },
    Deny { message: String, reason: PermissionDecisionReason },
}

/// Why a permission decision was made. Attached to PermissionDecision::Deny.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionDecisionReason {
    Rule { rule: PermissionRule },
    Mode { mode: PermissionMode },
    Classifier { classifier: String, reason: String },
    Hook { hook_name: String, reason: Option<String> },
    SafetyCheck { reason: String, classifier_approvable: bool },
    AsyncAgent { reason: String },
    User,
    Sandboxed,
}

pub struct ToolPermissionContext {
    pub mode: PermissionMode,
    pub additional_dirs: HashMap<String, AdditionalWorkingDir>,
    pub allow_rules: PermissionRulesBySource,
    pub deny_rules: PermissionRulesBySource,
    pub ask_rules: PermissionRulesBySource,
    pub bypass_available: bool,
    pub pre_plan_mode: Option<PermissionMode>,
}
```

### Command Types (from `types/command.ts`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAvailability { ClaudeAi, Console }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource { Skills, Plugin, Bundled, Mcp }

pub struct CommandBase {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub availability: Vec<CommandAvailability>,
    pub is_hidden: bool,
    pub argument_hint: Option<String>,
    pub when_to_use: Option<String>,
    pub user_invocable: bool,
    pub is_sensitive: bool,
    pub loaded_from: Option<CommandSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandType {
    Prompt(PromptCommandData),
    Local(LocalCommandData),
}

pub struct PromptCommandData {
    pub progress_message: String,
    pub content_length: i64,
    pub allowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub context: CommandContext,  // Inline or Fork
    pub agent: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub hooks: Option<Value>,  // deserialized by coco-hooks, not typed here (avoids L1->L4 dep)
}
```

### Tool Types (from `Tool.ts`)

```rust
/// All 43 built-in tool names (matches crate-coco-tools.md Tool Inventory).
/// Copy + const fn as_str() — zero-cost identity for builtins.
/// MCPTool excluded: MCP proxy instances use ToolId::Mcp, not ToolName.
/// FromStr matches the exact name string ("Read", "Bash", "WebFetch", etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ToolName {
    // File I/O (8)
    Bash, Read, Write, Edit, Glob, Grep, NotebookEdit, ApplyPatch,
    // Web (2)
    WebFetch, WebSearch,
    // Agent & Team (5)
    Agent, Skill, SendMessage, TeamCreate, TeamDelete,
    // Task Management (7)
    TaskCreate, TaskGet, TaskList, TaskUpdate, TaskStop, TaskOutput, TodoWrite,
    // Plan & Worktree (5)
    EnterPlanMode, ExitPlanMode, VerifyPlanExecution, EnterWorktree, ExitWorktree,
    // Utility (5)
    AskUserQuestion, ToolSearch, Config, Brief,
    #[serde(rename = "LSP")]
    Lsp,
    // MCP management (3) — not the MCP proxy itself
    McpAuth, ListMcpResources, ReadMcpResource,
    // Scheduling (4)
    CronCreate, CronDelete, CronList, RemoteTrigger,
    // Shell (2)
    PowerShell,
    #[serde(rename = "REPL")]
    Repl,
    // Internal/SDK (2)
    Sleep, SyntheticOutput,
}

impl ToolName {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Bash => "Bash", Self::Read => "Read", Self::Write => "Write",
            Self::Edit => "Edit", Self::Glob => "Glob", Self::Grep => "Grep",
            Self::NotebookEdit => "NotebookEdit", Self::ApplyPatch => "apply_patch",
            Self::WebFetch => "WebFetch", Self::WebSearch => "WebSearch",
            Self::Agent => "Agent", Self::Skill => "Skill",
            Self::SendMessage => "SendMessage",
            Self::TeamCreate => "TeamCreate", Self::TeamDelete => "TeamDelete",
            Self::TaskCreate => "TaskCreate", Self::TaskGet => "TaskGet",
            Self::TaskList => "TaskList", Self::TaskUpdate => "TaskUpdate",
            Self::TaskStop => "TaskStop", Self::TaskOutput => "TaskOutput",
            Self::TodoWrite => "TodoWrite",
            Self::EnterPlanMode => "EnterPlanMode", Self::ExitPlanMode => "ExitPlanMode",
            Self::VerifyPlanExecution => "VerifyPlanExecution",
            Self::EnterWorktree => "EnterWorktree", Self::ExitWorktree => "ExitWorktree",
            Self::AskUserQuestion => "AskUserQuestion", Self::ToolSearch => "ToolSearch",
            Self::Config => "Config", Self::Brief => "Brief", Self::Lsp => "LSP",
            Self::McpAuth => "McpAuth",
            Self::ListMcpResources => "ListMcpResources",
            Self::ReadMcpResource => "ReadMcpResource",
            Self::CronCreate => "CronCreate", Self::CronDelete => "CronDelete",
            Self::CronList => "CronList", Self::RemoteTrigger => "RemoteTrigger",
            Self::PowerShell => "PowerShell", Self::Repl => "REPL",
            Self::Sleep => "Sleep", Self::SyntheticOutput => "SyntheticOutput",
        }
    }
}

/// FromStr: "Read" → Ok(Read), "REPL" → Ok(Repl), "unknown" → Err
impl FromStr for ToolName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> { /* match on as_str() inverse */ }
}

/// Tool identity — type-safe for all tool kinds.
/// Built-in tools use ToolName (Copy, const fn as_str()), MCP and custom are structured.
///
/// Three distinct concepts:
///   ToolId      = identity ("who am I")         → this enum
///   ToolName    = built-in tools only (Copy)     → inner enum, 43 variants
///   ToolPattern = permission match expression    → String ("Bash(git *)", "mcp__slack__*")
/// Serde: serializes/deserializes as a FLAT STRING via Display/FromStr.
/// "Read" (builtin), "mcp__slack__send" (MCP), "my_plugin_tool" (custom).
/// NOT tagged JSON — wire format is always a single string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolId {
    /// Built-in tool (43 variants, Copy, const fn as_str())
    Builtin(ToolName),

    /// MCP tool: structured server + tool name.
    /// Wire format: "mcp__<server>__<tool>"
    Mcp { server: String, tool: String },

    /// Plugin/custom tool (future extensibility).
    /// Wire format: tool name as-is.
    Custom(String),
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(name) => f.write_str(name.as_str()),
            Self::Mcp { server, tool } => write!(f, "mcp__{server}__{tool}"),
            Self::Custom(name) => f.write_str(name),
        }
    }
}

/// Parses wire-format string. "mcp__server__tool" → Mcp, known → Builtin, else → Custom.
impl FromStr for ToolId {
    type Err = Infallible;  // always succeeds — Custom is catch-all

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix("mcp__") {
            if let Some((server, tool)) = rest.split_once("__") {
                return Ok(Self::Mcp { server: server.into(), tool: tool.into() });
            }
        }
        Ok(ToolName::from_str(s)
            .map(Self::Builtin)
            .unwrap_or_else(|| Self::Custom(s.into())))
    }
}

/// Serde as flat string — delegates to Display/FromStr.
impl Serialize for ToolId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for ToolId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(s.parse().unwrap()) // Infallible
    }
}

impl From<ToolName> for ToolId {
    fn from(name: ToolName) -> Self { Self::Builtin(name) }
}

impl ToolId {
    pub fn is_builtin(&self) -> bool { matches!(self, Self::Builtin(_)) }
    pub fn is_mcp(&self) -> bool { matches!(self, Self::Mcp { .. }) }
    pub fn mcp_server(&self) -> Option<&str> {
        match self { Self::Mcp { server, .. } => Some(server), _ => None }
    }
}

pub struct ToolInputSchema {
    /// JSON Schema properties for tool input. Type is always "object" (not stored).
    pub properties: HashMap<String, Value>,
}

pub struct ToolResult<T> {
    pub data: T,
    pub new_messages: Vec<Message>,
    // Note: context modification is handled by Tool::modify_context_after() in coco-tool,
    // NOT by a closure here. ToolResult is a plain data struct with no trait objects.
}

pub struct ToolProgress {
    pub tool_use_id: String,
    pub data: Value,
}
```

### Agent Type (from `tools/AgentTool/`)

```rust
/// 7 built-in subagent types (matches TS AgentTool loadAgentsDir.ts).
/// Copy + const fn as_str() — same pattern as ToolName.
/// Fork is special: implicit agent spawned when subagent_type is omitted + fork experiment enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentType {
    Explore,
    Plan,
    Review,
    StatusLine,
    ClaudeCodeGuide,
    Fork,
    HookAgent,
}

impl SubagentType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::StatusLine => "statusline-setup",
            Self::ClaudeCodeGuide => "claude-code-guide",
            Self::Fork => "fork",
            Self::HookAgent => "hook-agent",
        }
    }
}

/// FromStr: "explore" → Ok(Explore), "unknown" → Err
impl FromStr for SubagentType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> { /* match on as_str() inverse */ }
}

/// Agent identity — same pattern as ToolId.
/// SubagentType (7 builtin, Copy) wrapped with Custom for user-defined agents.
/// User agents loaded from: ~/.claude/agents/*.md, .claude/agents/*.md, plugins.
/// Serde as flat string — same pattern as ToolId.
/// "explore" (builtin), "my-custom-agent" (custom from .claude/agents/).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentTypeId {
    Builtin(SubagentType),
    Custom(String),
}

impl fmt::Display for AgentTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(t) => f.write_str(t.as_str()),
            Self::Custom(name) => f.write_str(name),
        }
    }
}

impl FromStr for AgentTypeId {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(SubagentType::from_str(s)
            .map(Self::Builtin)
            .unwrap_or_else(|| Self::Custom(s.into())))
    }
}

impl Serialize for AgentTypeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for AgentTypeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(s.parse().unwrap())
    }
}

impl From<SubagentType> for AgentTypeId {
    fn from(t: SubagentType) -> Self { Self::Builtin(t) }
}
```

### Task Types (from `Task.ts`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    LocalBash,
    LocalAgent,
    RemoteAgent,
    InProcessTeammate,
    LocalWorkflow,
    MonitorMcp,
    Dream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus { Pending, Running, Completed, Failed, Killed }

pub struct TaskStateBase {
    pub id: String,          // prefix + 8 random base36 chars
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub tool_use_id: Option<String>,
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub total_paused_ms: Option<i64>,
    pub output_file: String,
    pub output_offset: i64,
    pub notified: bool,
}

pub struct TaskHandle {
    pub task_id: String,
    pub cleanup: Option<Box<dyn FnOnce()>>,
}
```

### ID Types (from `types/ids.ts`)

```rust
// Branded newtype pattern
pub struct SessionId(pub String);
pub struct AgentId(pub String);  // format: `a(?:.+-)?[0-9a-f]{16}$`
pub struct TaskId(pub String);

// SandboxMode (from cocode-rs, needed by exec/sandbox)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode { ReadOnly, WorkspaceWrite, FullAccess, ExternalSandbox }
```

### Hook Types (from `types/hooks.ts`)

```rust
/// 27 hook event types (synced with TS coreSchemas.ts HOOK_EVENTS).
/// Uses #[non_exhaustive] because TS adds new events across versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum HookEventType {
    // Tool lifecycle
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    // Session lifecycle
    SessionStart,
    SessionEnd,
    Setup,
    Stop,
    StopFailure,
    // Subagent lifecycle
    SubagentStart,
    SubagentStop,
    // User interaction
    UserPromptSubmit,
    PermissionRequest,
    PermissionDenied,
    Notification,
    Elicitation,
    ElicitationResult,
    // Compaction
    PreCompact,
    PostCompact,
    // Task lifecycle
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    // Config & environment
    ConfigChange,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
    // Worktree
    WorktreeCreate,
    WorktreeRemove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutcome { Success, Blocking, NonBlockingError, Cancelled }

pub struct HookResult {
    pub outcome: HookOutcome,
    pub message: Option<Message>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub stop_reason: Option<String>,
    pub updated_input: Option<Value>,
}
```

### Token & Cost Types

```rust
/// Per-request token counts (returned by LLM API)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
}

/// Per-model accumulated usage (for cost tracking in coco-messages)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub web_search_requests: i64,
    pub cost_usd: f64,
}
```

### Thinking Types (multi-provider improvement over TS)

```rust
/// Unified thinking configuration for all providers.
/// The SINGLE type for effort + thinking across the entire pipeline.
///
/// Replaces both TS EffortLevel and ThinkingConfig (redundant with this):
///   TS EffortLevel ('low'|'medium'|'high'|'max') → ThinkingLevel::low()/medium()/high()/xhigh()
///   TS ThinkingConfig { type: 'adaptive' }       → ThinkingLevel { effort: High, budget: None }
///   TS ThinkingConfig { type: 'enabled', N }     → ThinkingLevel { effort: Medium, budget: Some(N) }
///   TS ThinkingConfig { type: 'disabled' }       → ThinkingLevel::none()
///
/// DESIGN (data-driven extensibility):
///   Only 2 typed fields (effort + budget_tokens) — truly universal across providers.
///   All provider-specific thinking params go through `options` (data-driven passthrough):
///     - OpenAI: { "reasoningSummary": "auto", "include": ["reasoning.encrypted_content"] }
///     - Anthropic: { "interleaved": true }
///     - Gemini: { "includeThoughts": true }
///     - Future params: just add to options, no code changes needed.
///
///   This replaces the previous approach of typed fields per provider param
///   (include_thoughts, reasoning_summary, interleaved) which required changing
///   3 crates (L0 protocol + L1 config + L2 inference) for each new param.
///
/// FLOW:
///   ModelInfo.supported_thinking_levels → defines full param sets per effort level
///   ModelInfo.default_thinking_level (ReasoningEffort) → ref to one supported entry
///   User /effort high → resolve from supported_thinking_levels → full ThinkingLevel
///   thinking_convert(level, provider):
///     Step 1: effort + budget_tokens → per-provider typed conversion
///     Step 2: level.options → merge directly into ProviderOptions (passthrough)
///
/// Used by: ModelInfo (coco-config), RoleSelection, InferenceContext, thinking_convert.
/// Evolved from cocode-rs common/protocol/src/thinking.rs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingLevel {
    /// Reasoning effort level — universal across all providers.
    /// thinking_convert maps this to per-provider values (reasoningEffort, thinkingLevel, etc.).
    pub effort: ReasoningEffort,

    /// Token budget — universal for budget-based providers (Anthropic/Gemini/Volcengine/Z.AI).
    /// thinking_convert maps this to budgetTokens/thinkingBudget per provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,

    /// Provider-specific thinking extensions — data-driven passthrough.
    /// Merged directly into ProviderOptions by thinking_convert (no typed conversion needed).
    /// Examples:
    ///   OpenAI:    { "reasoningSummary": "auto", "include": ["reasoning.encrypted_content"], "textVerbosity": "low" }
    ///   Anthropic: { "interleaved": true }
    ///   Gemini:    { "includeThoughts": true }
    ///   Future:    any new provider param, zero code changes
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub options: HashMap<String, serde_json::Value>,
}

impl ThinkingLevel {
    pub fn none() -> Self { Self { effort: ReasoningEffort::None, budget_tokens: None, options: HashMap::new() } }
    pub fn low() -> Self { Self { effort: ReasoningEffort::Low, ..Self::none() } }
    pub fn medium() -> Self { Self { effort: ReasoningEffort::Medium, ..Self::none() } }
    pub fn high() -> Self { Self { effort: ReasoningEffort::High, ..Self::none() } }
    pub fn xhigh() -> Self { Self { effort: ReasoningEffort::XHigh, ..Self::none() } }
    pub fn is_enabled(&self) -> bool { self.effort != ReasoningEffort::None }
    pub fn with_budget(effort: ReasoningEffort, budget: i32) -> Self {
        Self { effort, budget_tokens: Some(budget), options: HashMap::new() }
    }
}

/// Flexible deserialization: accepts string shorthand ("high") or full object
/// {"effort": "high", "budget_tokens": 32000, "options": {"interleaved": true}}.
impl FromStr for ThinkingLevel { /* parses effort name only, no options */ }

/// Reasoning effort level. Ordered from lowest to highest (derives Ord).
/// Provider-agnostic scale — thinking_convert maps to per-provider values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,     // 0 — thinking disabled
    Minimal,  // 1
    Low,      // 2
    Medium,   // 3 — default
    High,     // 4
    XHigh,    // 5 — ultrathink
}
```

### Provider & Model Types (from multi-provider-plan.md)

```rust
/// Wire-protocol family — which `vercel-ai-*` crate to dispatch to.
/// Consumed by `coco-config::ProviderConfig` and `app/cli/model_factory`.
/// Distinct from provider **instance identifier** (`ProviderConfig.name: String`):
/// a single `ProviderApi::OpenaiCompat` may back N user-named instances
/// (`xai`, `groq`, `azure-east`, …). See multi-provider-plan.md §2.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    Anthropic,
    Openai,
    Gemini,
    Volcengine,
    Zai,
    OpenaiCompat,
}

/// Which purpose a model serves. Multiple roles can map to different models.
/// Consumed by coco-config (ModelRoles) and coco-query (role resolution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Main,       // Primary conversation
    Plan,       // Planning/architecture and plan subagents
    Fast,       // Quick/cheap work
    Explore,    // Codebase exploration subagents
    Review,     // Code review / verification subagents
    Subagent,   // Default role for generic/custom subagent execution
    Memory,     // Memory relevance ranking
    HookAgent,  // Hook agent execution
}

// There is no ModelRole::Compact in the current type. Compaction uses the
// compact service/config path and model-role fallback rules instead of a
// separate closed role variant.
//
// Subagent role resolution is owned by core/subagent:
// AgentDefinition.model_role > built-in subagent mapping > ModelRole::Subagent.
// Built-ins map Explore -> Explore, Plan -> Plan, Verification -> Review, and
// generic/custom subagents default to Subagent.

/// A resolved model identity: provider + model ID.
/// Produced by coco-config, consumed by coco-inference.
///
/// `provider` is a free-form String (not ProviderApi enum) to support sub-provider
/// routing (e.g., "bedrock", "vertex") without expanding the enum.
/// `api` is the ProviderApi enum used for thinking_convert dispatch and provider factory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelSpec {
    pub provider: String,       // "anthropic", "bedrock", "vertex", "openai", ...
    pub api: ProviderApi,       // resolved ProviderApi for dispatch
    pub model_id: String,       // "claude-opus-4-6", "gpt-5"
    pub display_name: String,   // human-readable (excluded from PartialEq/Hash)
}

/// Model capabilities (checked at request time).
/// Aligned with cocode-rs Capability enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    TextGeneration,
    Streaming,
    Vision,
    Audio,
    ToolCalling,
    Embedding,
    ExtendedThinking,
    StructuredOutput,
    ReasoningSummaries,
    ParallelToolCalls,
    FastMode,
    /// Wire supports Anthropic-style `cache_control` blocks. Gates
    /// `cache_strategy` emission and auto-marker placement.
    PromptCache,
    /// `context-1m-2025-08-07` beta — 1M token context window.
    /// Wire forced to `"context_1m"` (serde's snake_case treats digits
    /// as part of the preceding word and would emit `"context1m"`).
    #[serde(rename = "context_1m")]
    Context1m,
    /// `interleaved-thinking-2025-05-14` beta. Also gates
    /// `redact-thinking-2026-02-12` (`betas.ts:272` reuses the same
    /// `modelSupportsISP` predicate for both).
    InterleavedThinking,
    /// `context-management-2025-06-27` beta.
    ContextManagement,
    /// `token-efficient-tools-2026-03-28` beta. Mutually exclusive
    /// with structured outputs.
    TokenEfficientTools,
}

/// How a model handles file editing / apply_patch tool.
/// Aligned with cocode-rs ApplyPatchToolType.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchToolType {
    #[default]
    Freeform,    // String-schema function tool (GPT-5.2+, codex models)
    Function,    // JSON function tool (gpt-oss)
    Shell,       // Shell-based, prompt instructions only (GPT-5, o3, o4-mini)
}

/// Communication protocol (OpenAI has two APIs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Chat,       // Standard chat completions
    Responses,  // OpenAI responses API (supports apply_patch)
}
```

### Prompt-cache types (from `prompt-cache-design.md`)

Provider-neutral types carried through `services/inference` to the adapter as opaque pass-through. Live in `coco-rs/common/types/src/cache.rs`.

```rust
/// Wire-level cache strategy (per-call).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    #[default]
    Disabled,
    Auto,    // Adapter places cache markers automatically.
    Manual,  // Caller is responsible for marker placement.
}

/// Caller-requested TTL. Adapter may downgrade based on eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

/// Where the cache lives (Anthropic-only knob).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    Org,
    Global,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheConfig {
    pub mode: PromptCacheMode,
    pub ttl: CacheTtl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CacheScope>,
    #[serde(default)]
    pub skip_cache_write: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_betas: Vec<BetaCapability>,
}

/// Typed beta capability the user can request per call. The adapter
/// translates each variant into its Anthropic-server header string
/// (kebab-case + date suffix). `Context1m` keeps explicit serde rename
/// because snake_case treats digits as part of the preceding word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BetaCapability {
    #[serde(rename = "context_1m")]
    Context1m,
    InterleavedThinking,
    ContextManagement,
    StructuredOutputs,
    TokenEfficientTools,
    FastMode,
    PromptCachingScope,
    RedactThinking,
    Advisor,
}

/// Auth/billing identity. **Session-stable** — set once on
/// `RuntimeConfig.account` and read by `vercel-ai-anthropic` at
/// provider construction. Mid-session flip requires settings reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    #[default]
    ApiKey,
    ClaudeAiSubscriber,
}
```

### Log Types (from `types/logs.ts`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserType { Human, Api }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entrypoint { Cli, SdkTs, SdkPy, Vscode, Jetbrains, Web }

/// Serialized message for log persistence (session replay, analytics)
pub struct SerializedMessage {
    pub message: Message,
    pub cwd: String,
    pub user_type: UserType,
    pub entrypoint: Option<Entrypoint>,
    pub session_id: String,
    pub timestamp: String,
    pub version: String,
    pub git_branch: Option<String>,
    pub model_id: Option<String>,
}

pub struct LogOption {
    pub date: String,
    pub path: String,
}
```

### Plugin Types (from `types/plugin.ts`)

```rust
/// Re-exports from utils/plugins/schemas (canonical definitions in coco-plugins)

/// Built-in plugin that ships with the CLI (can be enabled/disabled by users)
/// NOTE: To avoid L1→L4 dependency on coco-plugins, the manifest field uses
/// serde_json::Value. The consuming crate (coco-plugins) deserializes it.
pub struct BuiltinPluginDefinition {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub manifest: serde_json::Value,  // PluginManifest — deserialized by coco-plugins
}
```

### Text Input Types (from `types/textInputTypes.ts`)

```rust
/// These are TUI-layer types. In coco-rs, they live in coco-tui, not coco-types.
/// Listed here for TS mapping completeness only.
///
/// InlineGhostText, InputCommand, TextInputProps, PromptInputState, etc.
/// → coco-tui (v1, TUI-specific)
```

## Dependencies

```
coco-types depends on:
  - vercel-ai-provider (L0 types: LanguageModelV4Message, UserContentPart, AssistantContentPart, ToolContentPart)
  - serde, serde_json (serialization)
  - uuid (ID types)
  - chrono (timestamps)
  - strum (enum derive)

coco-types depends on vercel-ai-provider because Message wraps LanguageModelV4Message directly
(same pattern as TS wrapping @anthropic-ai/sdk types). vercel-ai-provider is L0 pure types.

coco-types does NOT depend on:
  - any other coco-* crate (it is the foundation layer)
  - any app/, services/, exec/ crate

Every other crate in the workspace can depend on coco-types.
```
