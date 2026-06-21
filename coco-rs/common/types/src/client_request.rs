//! `ClientRequest` — SDK-to-agent protocol requests.
//!
//! Session / turn / runtime / config / MCP / plugin / approval /
//! elicitation primitives.
//!
//! Hook and MCP-route SDK-side responses ride the **synchronous
//! JSON-RPC reply** to the corresponding `hook/callback` /
//! `mcp/routeMessage` server request — there is no separate client
//! request variant for them. See `event-system-design.md` §5.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use crate::HookEventType;
use crate::PermissionMode;
use crate::PermissionUpdate;
use crate::ThinkingLevel;
use crate::wire_tagged::wire_tagged_enum;

wire_tagged_enum! {
    method_enum = ClientRequestMethod,
    tagged_enum = ClientRequest,
    method_doc = "\
Wire-method identifier for every `ClientRequest` variant.\n\n\
Cross-language protocol constant exported to the JSON schema bundle so \
Python / other SDK codegens obtain the same vocabulary. Consumers should \
reference `ClientRequestMethod::SessionStart` rather than compare against \
raw wire strings.",
    tagged_doc = "\
Bidirectional control protocol — client-initiated requests.\n\n\
Each variant carries a unique `method` string used on the wire. \
The method is the discriminator; params are the variant-specific payload.\n\n\
See `event-system-design.md` §5.1 for the 22 base variants and §5.4 for \
the 8 gap additions. 31 total.",
    variants = {
        // === Session lifecycle (6) ===
        "initialize" => Initialize(InitializeParams),
        "session/start" => SessionStart(Box<SessionStartParams>),
        "session/resume" => SessionResume(SessionResumeParams),
        "session/list" => SessionList,
        "session/read" => SessionRead(SessionReadParams),
        "session/archive" => SessionArchive(SessionArchiveParams),

        // === Turn control (2) ===
        "turn/start" => TurnStart(TurnStartParams),
        "turn/interrupt" => TurnInterrupt,

        // === Approval + user input resolution (3) ===
        "approval/resolve" => ApprovalResolve(ApprovalResolveParams),
        "input/resolveUserInput" => UserInputResolve(UserInputResolveParams),
        /// Resolve a pending MCP elicitation request. Counterpart to the
        /// `ServerRequest` the agent sends when an MCP server needs
        /// structured user input (form values, OAuth tokens, etc.).
        /// See `event-system-design.md` §5.4.
        "elicitation/resolve" => ElicitationResolve(ElicitationResolveParams),

        // === Runtime control (9) ===
        "control/setModel" => SetModel(SetModelParams),
        "control/setPermissionMode" => SetPermissionMode(SetPermissionModeParams),
        "control/setThinking" => SetThinking(SetThinkingParams),
        "control/stopTask" => StopTask(StopTaskParams),
        "control/rewindFiles" => RewindFiles(RewindFilesParams),
        "control/updateEnv" => UpdateEnv(UpdateEnvParams),
        "control/keepAlive" => KeepAlive,
        "control/cancelRequest" => CancelRequest(CancelRequestParams),
        /// Interrupt one in-process teammate's active turn without
        /// stopping the teammate lifecycle.
        "agent/interruptCurrentWork" => AgentInterruptCurrentWork(AgentInterruptCurrentWorkParams),

        // === Config (2) ===
        "config/read" => ConfigRead,
        "config/value/write" => ConfigWrite(ConfigWriteParams),

        // === P1 gap additions (7) — event-system-design §5.4 ===
        /// Query MCP server connection status.
        "mcp/status" => McpStatus,
        /// Get context window usage breakdown.
        "context/usage" => ContextUsage,
        /// Hot-reload MCP server configurations.
        "mcp/setServers" => McpSetServers(McpSetServersParams),
        /// Reconnect a specific MCP server.
        "mcp/reconnect" => McpReconnect(McpReconnectParams),
        /// Enable/disable a specific MCP server.
        "mcp/toggle" => McpToggle(McpToggleParams),
        /// Reload all plugins from disk.
        "plugin/reload" => PluginReload,
        /// Apply feature flag settings at runtime.
        "config/applyFlags" => ConfigApplyFlags(ConfigApplyFlagsParams),
    }
}

// ---------------------------------------------------------------------------
// Param structs (alphabetized by variant)
// ---------------------------------------------------------------------------

/// Sent once at session start for capability negotiation. Carries hooks,
/// SDK MCP servers, output format, system prompt, and agent definitions
/// so the agent can construct its registries before the first turn.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitializeParams {
    /// Hook callbacks keyed by event type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HashMap<HookEventType, Vec<HookCallbackMatcher>>>,
    /// SDK-provided MCP server names (to skip env-configured ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_servers: Option<Vec<String>>,
    /// JSON schema for structured output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
    /// Full system prompt override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Text appended to the default system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    /// Custom agent definitions keyed by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<HashMap<String, SdkAgentDefinition>>,
    /// Enable prompt suggestions in the output stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<bool>,
    /// Enable agent progress summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_progress_summaries: Option<bool>,
}

/// Hook callback matcher with optional tool-name filter and callback IDs.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCallbackMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hook_callback_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,
}

/// SDK-supplied custom subagent spec carried on `InitializeParams.agents`.
///
/// Wire-level DTO. **Distinct** from the internal [`crate::AgentDefinition`]
/// which is the resolved post-load representation merged from markdown /
/// plugin / SDK sources.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SdkAgentDefinition {
    /// Natural-language description shown in the AgentTool prompt list.
    pub description: String,
    /// Agent system prompt body.
    pub prompt: String,
    /// Allowed tool names. `None` inherits all parent tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Explicit tool deny-list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    /// Model alias / full id / `"inherit"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-agent MCP servers (`string` name-ref or inline `{name: config}` map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<crate::AgentMcpServerSpec>>,
    /// Experimental critical system reminder appended to the system prompt.
    /// Wire field name: `criticalSystemReminder_EXPERIMENTAL`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "criticalSystemReminder_EXPERIMENTAL"
    )]
    pub critical_system_reminder_experimental: Option<String>,
    /// Skill names auto-loaded into the agent context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// Auto-submitted as the first user turn when this agent is the main thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    /// Hard ceiling on agentic turns before the agent stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i32>,
    /// Run as a fire-and-forget background task when invoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Auto-loading scope for agent memory files (`user` / `project` / `local`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::MemoryScope>,
    /// Reasoning effort selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::ReasoningEffort>,
    /// Permission mode override for tool executions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

/// Params for `session/start`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    /// Optional initial user prompt to run immediately after start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
}

/// Params for `session/resume`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumeParams {
    pub session_id: String,
}

/// Params for `session/read`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadParams {
    pub session_id: String,
    /// Optional pagination cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

/// Params for `session/archive`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionArchiveParams {
    pub session_id: String,
}

/// Params for `turn/start`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub prompt: String,
    /// Optional turn-scoped permission mode override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Optional turn-scoped thinking override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
}

/// The SDK is *resolving* a pending approval request, sent client→server.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResolveParams {
    pub request_id: String,
    pub decision: ApprovalDecision,
    /// Optional permission update to persist to rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_update: Option<PermissionUpdate>,
    /// Optional feedback to inject back to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Optional rewritten tool input the SDK client supplies at
    /// approval time. When `Some`, the engine substitutes this for the
    /// model-emitted input before invoking the tool. Used by
    /// `AskUserQuestion` to ship user-selected `answers` (and optional
    /// `annotations`) back into the tool's data envelope.
    ///
    /// In-process equivalent is `coco_tool_runtime::ToolPermissionResolution.updated_input`
    /// (TUI mode). Consumed by `app/cli/src/sdk_server/approval_bridge.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    /// Optional content blocks (typically image attachments) the SDK client
    /// wants attached to the next user message. Paste-image-during-
    /// AskUserQuestion or attachments alongside `MCPTool` answers ride this
    /// slot. Carried verbatim as `serde_json::Value` because the underlying
    /// content block shape is provider-specific; consumers translate per
    /// provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<serde_json::Value>>,
}

/// Permission approval decision.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

/// Params for `input/resolveUserInput`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputResolveParams {
    pub request_id: String,
    /// The user's answer to the `AskUserQuestion` prompt.
    pub answer: String,
}

/// Params for `elicitation/resolve`.
///
/// Sent client→server in response to a prior `ServerRequest` that
/// asked the client to collect structured input on behalf of an MCP
/// server (form values, OAuth tokens, etc.). The client populates
/// `values` with the user's input and sets `approved=true`, or sets
/// `approved=false` to reject the elicitation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResolveParams {
    /// Correlation id matching the `ServerRequest` the agent sent.
    pub request_id: String,
    /// Which MCP server the elicitation originated from. Echoed back
    /// so the agent can route the resolution to the right connection.
    pub mcp_server_name: String,
    /// Whether the user approved the elicitation. If `false`, `values`
    /// is ignored and the MCP server sees a rejection.
    pub approved: bool,
    /// The collected field values keyed by field name. Each value is
    /// an opaque JSON payload so typed/untyped fields share the wire
    /// format. Empty when `approved=false`.
    #[serde(default)]
    pub values: std::collections::HashMap<String, serde_json::Value>,
}

/// Params for `control/setModel`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetModelParams {
    /// None means revert to the default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Params for `control/setPermissionMode`.
///
/// The `ultraplan` field (CCR web-UI refinement flow) is intentionally
/// omitted — see CLAUDE.md "Plan Mode — Skip Ultraplan Only".
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetPermissionModeParams {
    pub mode: PermissionMode,
}

/// Params for `control/setThinking`.
/// Uses `ThinkingLevel` which includes effort level and per-provider options.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetThinkingParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
}

/// Params for `control/stopTask`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopTaskParams {
    pub task_id: String,
}

/// Params for `control/rewindFiles`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindFilesParams {
    pub user_message_id: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// Params for `control/updateEnv`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEnvParams {
    pub env: HashMap<String, String>,
}

/// Params for `control/cancelRequest`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequestParams {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Params for `agent/interruptCurrentWork`.
///
/// Aborts the target teammate's current model/tool turn while keeping the
/// teammate process alive for later messages.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInterruptCurrentWorkParams {
    pub agent_id: String,
}

/// Params for `config/value/write`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigWriteParams {
    pub key: String,
    pub value: serde_json::Value,
    /// Optional scope: "user" | "project" | "local".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

// --- Gap additions (7) ---

/// Params for `mcp/setServers`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSetServersParams {
    /// Server configs keyed by name.
    pub servers: HashMap<String, serde_json::Value>,
}

/// Params for `mcp/reconnect`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpReconnectParams {
    pub server_name: String,
}

/// Params for `mcp/toggle`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToggleParams {
    pub server_name: String,
    pub enabled: bool,
}

/// Params for `config/applyFlags`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigApplyFlagsParams {
    pub settings: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
#[path = "client_request.test.rs"]
mod tests;
