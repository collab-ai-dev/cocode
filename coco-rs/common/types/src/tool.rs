use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

/// Prefix for MCP tool qualified names: `mcp__<server>__<tool>`.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// Separator between server and tool in MCP qualified names.
pub const MCP_TOOL_SEPARATOR: &str = "__";

/// Branch prefix for agent worktrees created by `EnterWorktree`.
pub const AGENT_WORKTREE_BRANCH_PREFIX: &str = "agent/task-";

/// All built-in tool names.
/// Copy + const fn as_str() — zero-cost identity for builtins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::EnumIter)]
#[serde(rename_all = "PascalCase")]
pub enum ToolName {
    // File I/O (8)
    Bash,
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    NotebookEdit,
    /// Patch-based file edit format introduced by gpt-5. The model emits
    /// a unified-diff-style patch and the runtime applies it. Lives in
    /// `ToolName` (not just on the model side) because coco-rs must
    /// register a Tool implementation to actually apply the patch.
    /// Visible only when `ToolOverrides::is_extra(ApplyPatch)`.
    #[serde(rename = "apply_patch")]
    ApplyPatch,
    // Web (2)
    WebFetch,
    WebSearch,
    // Agent & Team (5)
    Agent,
    Skill,
    SendMessage,
    TeamCreate,
    TeamDelete,
    // Task Management (7)
    TaskCreate,
    TaskGet,
    TaskList,
    TaskUpdate,
    TaskStop,
    TaskOutput,
    TodoWrite,
    // Plan & Worktree (5)
    EnterPlanMode,
    ExitPlanMode,
    VerifyPlanExecution,
    EnterWorktree,
    ExitWorktree,
    // Utility (5)
    AskUserQuestion,
    ToolSearch,
    Config,
    /// TS wire name `SendUserMessage`.
    SendUserMessage,
    #[serde(rename = "LSP")]
    Lsp,
    // MCP management (3)
    McpAuth,
    #[serde(rename = "ListMcpResourcesTool")]
    ListMcpResources,
    #[serde(rename = "ReadMcpResourceTool")]
    ReadMcpResource,
    // Scheduling (4)
    CronCreate,
    CronDelete,
    CronList,
    RemoteTrigger,
    // Shell (2)
    PowerShell,
    #[serde(rename = "REPL")]
    Repl,
    // Internal/SDK (2)
    Sleep,
    /// Synthetic tool that captures the model's structured JSON
    /// response and forwards it to the SDK result side-channel.
    ///
    /// The wire name is `"StructuredOutput"` — matches what the model
    /// sees. Only injected in non-interactive sessions when `--json-schema`
    /// is supplied; never visible in TUI.
    StructuredOutput,
}

impl ToolName {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Edit => "Edit",
            Self::Glob => "Glob",
            Self::Grep => "Grep",
            Self::NotebookEdit => "NotebookEdit",
            Self::ApplyPatch => "apply_patch",
            Self::WebFetch => "WebFetch",
            Self::WebSearch => "WebSearch",
            Self::Agent => "Agent",
            Self::Skill => "Skill",
            Self::SendMessage => "SendMessage",
            Self::TeamCreate => "TeamCreate",
            Self::TeamDelete => "TeamDelete",
            Self::TaskCreate => "TaskCreate",
            Self::TaskGet => "TaskGet",
            Self::TaskList => "TaskList",
            Self::TaskUpdate => "TaskUpdate",
            Self::TaskStop => "TaskStop",
            Self::TaskOutput => "TaskOutput",
            Self::TodoWrite => "TodoWrite",
            Self::EnterPlanMode => "EnterPlanMode",
            Self::ExitPlanMode => "ExitPlanMode",
            Self::VerifyPlanExecution => "VerifyPlanExecution",
            Self::EnterWorktree => "EnterWorktree",
            Self::ExitWorktree => "ExitWorktree",
            Self::AskUserQuestion => "AskUserQuestion",
            Self::ToolSearch => "ToolSearch",
            Self::Config => "Config",
            Self::SendUserMessage => "SendUserMessage",
            Self::Lsp => "LSP",
            Self::McpAuth => "McpAuth",
            Self::ListMcpResources => "ListMcpResourcesTool",
            Self::ReadMcpResource => "ReadMcpResourceTool",
            Self::CronCreate => "CronCreate",
            Self::CronDelete => "CronDelete",
            Self::CronList => "CronList",
            Self::RemoteTrigger => "RemoteTrigger",
            Self::PowerShell => "PowerShell",
            Self::Repl => "REPL",
            Self::Sleep => "Sleep",
            Self::StructuredOutput => "StructuredOutput",
        }
    }

    /// Model-aware file-mutation tool resolution — the single rule shared by
    /// every prompt that names a write/edit tool (plan-mode reminder, AgentTool
    /// examples, post-compaction plan reference).
    ///
    /// There is no per-model table: the answer is derived from what the model
    /// actually has. `native` is the canonical builtin for the operation
    /// (`Write` or `Edit`). The rule:
    ///
    /// 1. native tool present  → `native` (Claude family keeps Write/Edit)
    /// 2. else `apply_patch` present → [`Self::ApplyPatch`] (gpt-5 family swaps
    ///    native edits for the freeform patch tool; its `*** Add File` /
    ///    `*** Update File` hunks cover create + edit)
    /// 3. else → `native` (harmless fallback for degenerate tool sets)
    ///
    /// Any future model family that follows the same "drop native, add
    /// apply_patch" shape is handled automatically.
    pub fn file_mutation_tool(
        native: ToolName,
        has_native: bool,
        has_apply_patch: bool,
    ) -> ToolName {
        if has_native {
            native
        } else if has_apply_patch {
            ToolName::ApplyPatch
        } else {
            native
        }
    }

    /// [`Self::file_mutation_tool`] resolved from the model's available tool
    /// names this turn (e.g. `GeneratorContext::tools` / `PromptOptions::tool_names`).
    /// Guarantees the returned name is one the model can actually call.
    pub fn write_tool_for(available: &[String]) -> ToolName {
        Self::file_mutation_tool_from_names(Self::Write, available)
    }

    /// Edit-operation counterpart of [`Self::write_tool_for`].
    pub fn edit_tool_for(available: &[String]) -> ToolName {
        Self::file_mutation_tool_from_names(Self::Edit, available)
    }

    fn file_mutation_tool_from_names(native: ToolName, available: &[String]) -> ToolName {
        let has = |t: ToolName| available.iter().any(|n| n.as_str() == t.as_str());
        Self::file_mutation_tool(native, has(native), has(ToolName::ApplyPatch))
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ToolName> for String {
    fn from(name: ToolName) -> Self {
        name.as_str().to_string()
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ToolName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Bash" => Ok(Self::Bash),
            "Read" => Ok(Self::Read),
            "Write" => Ok(Self::Write),
            "Edit" => Ok(Self::Edit),
            "Glob" => Ok(Self::Glob),
            "Grep" => Ok(Self::Grep),
            "NotebookEdit" => Ok(Self::NotebookEdit),
            "apply_patch" => Ok(Self::ApplyPatch),
            "WebFetch" => Ok(Self::WebFetch),
            "WebSearch" => Ok(Self::WebSearch),
            "Agent" => Ok(Self::Agent),
            "Skill" => Ok(Self::Skill),
            "SendMessage" => Ok(Self::SendMessage),
            "TeamCreate" => Ok(Self::TeamCreate),
            "TeamDelete" => Ok(Self::TeamDelete),
            "TaskCreate" => Ok(Self::TaskCreate),
            "TaskGet" => Ok(Self::TaskGet),
            "TaskList" => Ok(Self::TaskList),
            "TaskUpdate" => Ok(Self::TaskUpdate),
            "TaskStop" => Ok(Self::TaskStop),
            "TaskOutput" => Ok(Self::TaskOutput),
            "TodoWrite" => Ok(Self::TodoWrite),
            "EnterPlanMode" => Ok(Self::EnterPlanMode),
            "ExitPlanMode" => Ok(Self::ExitPlanMode),
            "VerifyPlanExecution" => Ok(Self::VerifyPlanExecution),
            "EnterWorktree" => Ok(Self::EnterWorktree),
            "ExitWorktree" => Ok(Self::ExitWorktree),
            "AskUserQuestion" => Ok(Self::AskUserQuestion),
            "ToolSearch" => Ok(Self::ToolSearch),
            "Config" => Ok(Self::Config),
            "SendUserMessage" => Ok(Self::SendUserMessage),
            "LSP" => Ok(Self::Lsp),
            "McpAuth" => Ok(Self::McpAuth),
            "ListMcpResourcesTool" => Ok(Self::ListMcpResources),
            "ReadMcpResourceTool" => Ok(Self::ReadMcpResource),
            "CronCreate" => Ok(Self::CronCreate),
            "CronDelete" => Ok(Self::CronDelete),
            "CronList" => Ok(Self::CronList),
            "RemoteTrigger" => Ok(Self::RemoteTrigger),
            "PowerShell" => Ok(Self::PowerShell),
            "REPL" => Ok(Self::Repl),
            "Sleep" => Ok(Self::Sleep),
            "StructuredOutput" => Ok(Self::StructuredOutput),
            _ => Err(format!("unknown tool name: {s}")),
        }
    }
}

/// Resolve a legacy tool-name alias to its canonical form.
///
/// TS parity: `utils/permissions/permissionRuleParser.ts` —
/// `LEGACY_TOOL_NAME_ALIASES`. Used by hook matchers and permission
/// rule parsing so renamed tools keep matching prior config.
///
/// Aliases:
/// - `Task` → `Agent`
/// - `KillShell` → `TaskStop`
/// - `AgentOutputTool` / `BashOutputTool` → `TaskOutput`
pub fn normalize_legacy_tool_name(name: &str) -> &str {
    match name {
        "Task" => "Agent",
        "KillShell" => "TaskStop",
        "AgentOutputTool" | "BashOutputTool" => "TaskOutput",
        other => other,
    }
}

/// Reverse lookup: list legacy aliases for a canonical name.
///
/// TS parity: `getLegacyToolNames` — used by regex matchers so
/// `^Task$` keeps matching after the rename to `Agent`.
pub fn legacy_tool_name_aliases_of(canonical: &str) -> &'static [&'static str] {
    match canonical {
        "Agent" => &["Task"],
        "TaskStop" => &["KillShell"],
        "TaskOutput" => &["AgentOutputTool", "BashOutputTool"],
        _ => &[],
    }
}

/// Model-facing shell tool protocol declared by a model entry.
///
/// This is intentionally distinct from the runtime shell used to spawn
/// processes. It answers "which shell tool schema/protocol can this model
/// consume?", not "which binary should execute commands?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelShellToolType {
    #[default]
    ShellCommand,
    UnifiedExec,
    Disabled,
}

/// Shell tool exposed to the model for this session.
///
/// `Disabled` means no model-facing shell tool is visible, regardless of
/// whether shell binaries exist locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveShellTool {
    Bash,
    PowerShell,
    #[default]
    Disabled,
}

/// Tool identity — type-safe for all tool kinds.
///
/// Three distinct concepts:
///   ToolId      = identity ("who am I")         → this enum
///   ToolName    = built-in tools only (Copy)     → inner enum
///   ToolPattern = permission match expression    → String ("Bash(git *)", "mcp__slack__*")
///
/// Serde: serializes/deserializes as a FLAT STRING via Display/FromStr.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolId {
    /// Built-in tool (Copy, const fn as_str()).
    Builtin(ToolName),
    /// MCP tool: structured server + tool name.
    /// Wire format: "mcp__<server>__<tool>"
    Mcp { server: String, tool: String },
    /// Plugin/custom tool (future extensibility).
    Custom(String),
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(name) => f.write_str(name.as_str()),
            Self::Mcp { server, tool } => {
                write!(f, "{MCP_TOOL_PREFIX}{server}{MCP_TOOL_SEPARATOR}{tool}")
            }
            Self::Custom(name) => f.write_str(name),
        }
    }
}

/// Parses wire-format string. "mcp__server__tool" → Mcp, known → Builtin, else → Custom.
impl FromStr for ToolId {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix(MCP_TOOL_PREFIX)
            && let Some((server, tool)) = rest.split_once(MCP_TOOL_SEPARATOR)
        {
            return Ok(Self::Mcp {
                server: server.into(),
                tool: tool.into(),
            });
        }
        Ok(ToolName::from_str(s)
            .map(Self::Builtin)
            .unwrap_or_else(|_| Self::Custom(s.into())))
    }
}

impl Serialize for ToolId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ToolId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        // Infallible — unwrap is safe
        Ok(s.parse().unwrap())
    }
}

// Wire shape is a flat string ("Read", "mcp__slack__send",
// "my_plugin_tool"). Skip the auto-derive and emit the String schema
// so SDK schema consumers don't see a tagged-enum shape.
//
// `inline_schema = true` keeps the schemars 0.8 behavior for this kind
// of String-aliased newtype: parent schemas inline the
// `{"type": "string"}` shape instead of emitting a `$ref` to a `ToolId`
// entry in `$defs`. SDK codegen pipelines that map `$ref` names to
// generated classes otherwise need a separate alias for what is
// already-a-string on the wire.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for ToolId {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ToolId".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

impl From<ToolName> for ToolId {
    fn from(name: ToolName) -> Self {
        Self::Builtin(name)
    }
}

impl ToolId {
    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin(_))
    }

    pub fn is_mcp(&self) -> bool {
        matches!(self, Self::Mcp { .. })
    }

    pub fn mcp_server(&self) -> Option<&str> {
        match self {
            Self::Mcp { server, .. } => Some(server),
            _ => None,
        }
    }
}

// `ToolResult<T>` (which carries `Vec<Message>`) lives in `coco-messages`;
// the foundational tool identity types (ToolName, ToolId, ToolProgress,
// MCP_TOOL_PREFIX, …) stay here. The self-validating runtime input schema
// (compiled validator + closed JSON Schema) lives in `coco-tool-runtime`
// (`coco_tool_runtime::ToolInputSchema`), not here — it depends on
// `jsonschema`, an L3 concern.

/// Progress report during tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgress {
    pub tool_use_id: String,
    pub data: serde_json::Value,
}

#[cfg(test)]
#[path = "tool.test.rs"]
mod tests;
