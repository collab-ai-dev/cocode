//! `ClientRequest` — SDK-to-agent protocol requests.
//!
//! Session / turn / runtime / config / MCP / plugin / approval /
//! elicitation primitives.
//!
//! Hook and MCP-route SDK-side responses ride the **synchronous
//! JSON-RPC reply** to the corresponding `hook/callback` /
//! `mcp/routeMessage` server request — there is no separate client
//! request variant for them. See `event-system-design.md` §5.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    AgentColorName, HookEventType, ModelRole, PermissionMode, PermissionUpdate,
    ProviderModelSelection, QueuedCommandEditImage, ReasoningEffort, SessionId,
    SessionUsageSnapshot, SurfaceId, TaskStateBase, ThinkingLevel, wire_tagged::wire_tagged_enum,
};

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
See `event-system-design.md` §5.1 for the base variants and §5.4 for \
gap additions.",
    variants = {
        // === Session lifecycle (11) ===
        "initialize" => Initialize(InitializeParams),
        "session/start" => SessionStart(Box<SessionStartParams>),
        "session/resume" => SessionResume(SessionResumeParams),
        "session/replace" => SessionReplace(Box<SessionReplaceParams>),
        "session/list" => SessionList,
        "session/read" => SessionRead(SessionReadParams),
        "session/turns/list" => SessionTurnsList(SessionTurnsListParams),
        "session/subscribe" => SessionSubscribe(SessionSubscribeParams),
        "session/archive" => SessionArchive(SessionArchiveParams),
        "session/rename" => SessionRename(SessionRenameParams),
        "session/toggleTag" => SessionToggleTag(SessionToggleTagParams),
        "session/cost" => SessionCost(SessionTarget),
        "session/status" => SessionStatus(SessionTarget),

        // === Turn control (2) ===
        "turn/start" => TurnStart(TurnStartParams),
        "turn/interrupt" => TurnInterrupt(InteractiveTarget),

        // === Running task observability (2) ===
        "task/list" => TaskList(SessionTarget),
        "task/detail" => TaskDetail(TaskDetailParams),

        // === Approval + user input resolution (3) ===
        "approval/resolve" => ApprovalResolve(ApprovalResolveParams),
        "input/resolveUserInput" => UserInputResolve(UserInputResolveParams),
        /// Resolve a pending MCP elicitation request. Counterpart to the
        /// `ServerRequest` the agent sends when an MCP server needs
        /// structured user input (form values, OAuth tokens, etc.).
        /// See `event-system-design.md` §5.4.
        "elicitation/resolve" => ElicitationResolve(ElicitationResolveParams),

        // === Runtime control (13) ===
        "control/setModel" => SetModel(SetModelParams),
        "control/setModelRole" => SetModelRole(SetModelRoleParams),
        "control/setPermissionMode" => SetPermissionMode(SetPermissionModeParams),
        "control/setThinking" => SetThinking(SetThinkingParams),
        "control/setAgentColor" => SetAgentColor(SetAgentColorParams),
        "control/applyPermissionUpdate" => ApplyPermissionUpdate(ApplyPermissionUpdateParams),
        "control/resetSessionPermissionRules" => ResetSessionPermissionRules(InteractiveTarget),
        "control/stopTask" => StopTask(StopTaskParams),
        "control/rewindFiles" => RewindFiles(RewindFilesParams),
        "control/updateEnv" => UpdateEnv(UpdateEnvParams),
        "control/backgroundAllTasks" => BackgroundAllTasks(InteractiveTarget),
        "control/keepAlive" => KeepAlive,
        "control/cancelRequest" => CancelRequest(CancelRequestParams),
        /// Interrupt one in-process teammate's active turn without
        /// stopping the teammate lifecycle.
        "agent/interruptCurrentWork" => AgentInterruptCurrentWork(AgentInterruptCurrentWorkParams),

        // === Config (2) ===
        "config/read" => ConfigRead(ConfigReadParams),
        "config/value/write" => ConfigWrite(ConfigWriteParams),

        // === P1 gap additions (7) — event-system-design §5.4 ===
        /// Query MCP server connection status.
        "mcp/status" => McpStatus(SessionTarget),
        /// Get context window usage breakdown.
        "context/usage" => ContextUsage(SessionTarget),
        /// Hot-reload MCP server configurations.
        "mcp/setServers" => McpSetServers(McpSetServersParams),
        /// Reconnect a specific MCP server.
        "mcp/reconnect" => McpReconnect(McpReconnectParams),
        /// Enable/disable a specific MCP server.
        "mcp/toggle" => McpToggle(McpToggleParams),
        /// Reload all plugins from disk.
        "plugin/reload" => PluginReload(InteractiveTarget),
        /// Reload hooks from current settings.
        "hook/reload" => HookReload(InteractiveTarget),
        /// Apply feature flag settings at runtime.
        "config/applyFlags" => ConfigApplyFlags(ConfigApplyFlagsParams),
    }
}

/// Selects persisted or live state without proving interactive ownership.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTarget {
    pub session_id: SessionId,
}

/// Selects one live interactive surface and its immutable session identity.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractiveTarget {
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveTarget {
    Interactive(InteractiveTarget),
    Orphaned(SessionTarget),
}

impl ArchiveTarget {
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Interactive(target) => &target.session_id,
            Self::Orphaned(target) => &target.session_id,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestScope {
    Connection,
    Lifecycle,
    Process,
    SessionRead,
    Interactive,
    Configuration,
}

/// Canonical exhaustive request-scope classification.
pub const fn request_scope(method: ClientRequestMethod) -> RequestScope {
    match method {
        ClientRequestMethod::Initialize
        | ClientRequestMethod::KeepAlive
        | ClientRequestMethod::CancelRequest => RequestScope::Connection,
        ClientRequestMethod::SessionStart
        | ClientRequestMethod::SessionResume
        | ClientRequestMethod::SessionReplace
        | ClientRequestMethod::SessionSubscribe
        | ClientRequestMethod::SessionArchive => RequestScope::Lifecycle,
        ClientRequestMethod::SessionList => RequestScope::Process,
        ClientRequestMethod::SessionRead
        | ClientRequestMethod::SessionTurnsList
        | ClientRequestMethod::SessionRename
        | ClientRequestMethod::SessionToggleTag
        | ClientRequestMethod::SessionCost
        | ClientRequestMethod::SessionStatus
        | ClientRequestMethod::TaskList
        | ClientRequestMethod::TaskDetail
        | ClientRequestMethod::McpStatus
        | ClientRequestMethod::ContextUsage => RequestScope::SessionRead,
        ClientRequestMethod::TurnStart
        | ClientRequestMethod::TurnInterrupt
        | ClientRequestMethod::ApprovalResolve
        | ClientRequestMethod::UserInputResolve
        | ClientRequestMethod::ElicitationResolve
        | ClientRequestMethod::SetModel
        | ClientRequestMethod::SetModelRole
        | ClientRequestMethod::SetPermissionMode
        | ClientRequestMethod::SetThinking
        | ClientRequestMethod::SetAgentColor
        | ClientRequestMethod::ApplyPermissionUpdate
        | ClientRequestMethod::ResetSessionPermissionRules
        | ClientRequestMethod::StopTask
        | ClientRequestMethod::RewindFiles
        | ClientRequestMethod::UpdateEnv
        | ClientRequestMethod::BackgroundAllTasks
        | ClientRequestMethod::AgentInterruptCurrentWork
        | ClientRequestMethod::McpSetServers
        | ClientRequestMethod::McpReconnect
        | ClientRequestMethod::McpToggle
        | ClientRequestMethod::PluginReload
        | ClientRequestMethod::HookReload
        | ClientRequestMethod::ConfigApplyFlags => RequestScope::Interactive,
        ClientRequestMethod::ConfigRead | ClientRequestMethod::ConfigWrite => {
            RequestScope::Configuration
        }
    }
}

impl ClientRequest {
    /// Interactive authority carried by this request, if its scope requires
    /// one. Lifecycle replacement and archive validate their typed targets in
    /// their dedicated lifecycle paths.
    pub fn interactive_target(&self) -> Option<&InteractiveTarget> {
        match self {
            Self::TurnStart(params) => Some(&params.target),
            Self::TurnInterrupt(target)
            | Self::ResetSessionPermissionRules(target)
            | Self::BackgroundAllTasks(target)
            | Self::PluginReload(target)
            | Self::HookReload(target) => Some(target),
            Self::ApprovalResolve(params) => Some(&params.target),
            Self::UserInputResolve(params) => Some(&params.target),
            Self::ElicitationResolve(params) => Some(&params.target),
            Self::SetModel(params) => Some(&params.target),
            Self::SetModelRole(params) => Some(&params.target),
            Self::SetPermissionMode(params) => Some(&params.target),
            Self::SetThinking(params) => Some(&params.target),
            Self::SetAgentColor(params) => Some(&params.target),
            Self::ApplyPermissionUpdate(params) => Some(&params.target),
            Self::StopTask(params) => Some(&params.target),
            Self::RewindFiles(params) => Some(&params.target),
            Self::UpdateEnv(params) => Some(&params.target),
            Self::AgentInterruptCurrentWork(params) => Some(&params.target),
            Self::McpSetServers(params) => Some(&params.target),
            Self::McpReconnect(params) => Some(&params.target),
            Self::McpToggle(params) => Some(&params.target),
            Self::ConfigApplyFlags(params) => Some(&params.target),
            Self::ConfigWrite(ConfigWriteParams {
                target: ConfigWriteTarget::Project(target) | ConfigWriteTarget::Local(target),
                ..
            }) => Some(target),
            Self::Initialize(_)
            | Self::SessionStart(_)
            | Self::SessionResume(_)
            | Self::SessionReplace(_)
            | Self::SessionList
            | Self::SessionRead(_)
            | Self::SessionTurnsList(_)
            | Self::SessionSubscribe(_)
            | Self::SessionArchive(_)
            | Self::SessionRename(_)
            | Self::SessionToggleTag(_)
            | Self::SessionCost(_)
            | Self::SessionStatus(_)
            | Self::TaskList(_)
            | Self::TaskDetail(_)
            | Self::KeepAlive
            | Self::CancelRequest(_)
            | Self::ConfigRead(_)
            | Self::ConfigWrite(ConfigWriteParams {
                target: ConfigWriteTarget::User,
                ..
            })
            | Self::McpStatus(_)
            | Self::ContextUsage(_) => None,
        }
    }

    pub fn session_target(&self) -> Option<&SessionTarget> {
        match self {
            Self::SessionResume(params) => Some(&params.target),
            Self::SessionRead(params) => Some(&params.target),
            Self::SessionTurnsList(params) => Some(&params.target),
            Self::SessionSubscribe(params) => Some(&params.target),
            Self::SessionRename(params) => Some(&params.target),
            Self::SessionToggleTag(params) => Some(&params.target),
            Self::SessionCost(target)
            | Self::SessionStatus(target)
            | Self::TaskList(target)
            | Self::McpStatus(target)
            | Self::ContextUsage(target) => Some(target),
            Self::TaskDetail(params) => Some(&params.target),
            Self::ConfigRead(ConfigReadParams {
                target: ConfigReadTarget::Session(target),
            }) => Some(target),
            Self::Initialize(_)
            | Self::SessionStart(_)
            | Self::SessionReplace(_)
            | Self::SessionList
            | Self::SessionArchive(_)
            | Self::TurnStart(_)
            | Self::TurnInterrupt(_)
            | Self::ApprovalResolve(_)
            | Self::UserInputResolve(_)
            | Self::ElicitationResolve(_)
            | Self::SetModel(_)
            | Self::SetModelRole(_)
            | Self::SetPermissionMode(_)
            | Self::SetThinking(_)
            | Self::SetAgentColor(_)
            | Self::ApplyPermissionUpdate(_)
            | Self::ResetSessionPermissionRules(_)
            | Self::StopTask(_)
            | Self::RewindFiles(_)
            | Self::UpdateEnv(_)
            | Self::BackgroundAllTasks(_)
            | Self::KeepAlive
            | Self::CancelRequest(_)
            | Self::AgentInterruptCurrentWork(_)
            | Self::ConfigRead(ConfigReadParams {
                target: ConfigReadTarget::Process,
            })
            | Self::ConfigWrite(_)
            | Self::McpSetServers(_)
            | Self::McpReconnect(_)
            | Self::McpToggle(_)
            | Self::PluginReload(_)
            | Self::HookReload(_)
            | Self::ConfigApplyFlags(_) => None,
        }
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
    /// Custom workflow body for the plan-mode system reminder.
    #[serde(
        default,
        rename = "planModeInstructions",
        alias = "plan_mode_instructions",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan_mode_instructions: Option<String>,
    /// Custom agent definitions keyed by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<HashMap<String, SdkAgentDefinition>>,
    /// Enable prompt suggestions in the output stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<bool>,
    /// Enable agent progress summaries.
    #[serde(
        default,
        rename = "agentProgressSummaries",
        alias = "agent_progress_summaries",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_progress_summaries: Option<bool>,
}

/// Immutable, normalized initialize data owned by one accepted connection.
#[derive(Debug, Clone)]
pub struct ConnectionProfile {
    initialize: Arc<InitializeParams>,
}

impl ConnectionProfile {
    pub fn initialize(&self) -> &InitializeParams {
        &self.initialize
    }

    pub fn callback_requirements(&self) -> SessionCallbackRequirements {
        let hook_callback_ids = self
            .initialize
            .hooks
            .iter()
            .flat_map(HashMap::values)
            .flatten()
            .flat_map(|matcher| matcher.hook_callback_ids.iter().cloned())
            .collect();
        SessionCallbackRequirements { hook_callback_ids }
    }
}

impl TryFrom<InitializeParams> for ConnectionProfile {
    type Error = ConnectionProfileError;

    fn try_from(mut initialize: InitializeParams) -> Result<Self, Self::Error> {
        if let Some(names) = &mut initialize.sdk_mcp_servers {
            for name in names.iter_mut() {
                *name = name.trim().to_string();
                if name.is_empty() {
                    return Err(ConnectionProfileError::EmptyMcpServerName);
                }
            }
            names.sort();
            names.dedup();
        }
        if let Some(hooks) = &initialize.hooks {
            for callback_id in hooks
                .values()
                .flatten()
                .flat_map(|matcher| &matcher.hook_callback_ids)
            {
                if callback_id.trim().is_empty() {
                    return Err(ConnectionProfileError::EmptyHookCallbackId);
                }
            }
        }
        if initialize
            .agents
            .as_ref()
            .is_some_and(|agents| agents.keys().any(|name| name.trim().is_empty()))
        {
            return Err(ConnectionProfileError::EmptyAgentName);
        }
        Ok(Self {
            initialize: Arc::new(initialize),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionProfileError {
    EmptyMcpServerName,
    EmptyHookCallbackId,
    EmptyAgentName,
}

impl std::fmt::Display for ConnectionProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMcpServerName => formatter.write_str("SDK MCP server name cannot be empty"),
            Self::EmptyHookCallbackId => formatter.write_str("hook callback id cannot be empty"),
            Self::EmptyAgentName => formatter.write_str("SDK agent name cannot be empty"),
        }
    }
}

impl std::error::Error for ConnectionProfileError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionCallbackRequirements {
    pub hook_callback_ids: BTreeSet<String>,
}

impl SessionCallbackRequirements {
    pub fn is_satisfied_by(&self, profile: &ConnectionProfile) -> bool {
        let available = profile.callback_requirements();
        self.hook_callback_ids
            .is_subset(&available.hook_callback_ids)
    }
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
    pub target: SessionTarget,
}

/// Destination selected by explicit `session/replace`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReplacement {
    Fresh(SessionStartParams),
    Resume(SessionTarget),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReplaceParams {
    pub source: InteractiveTarget,
    pub destination: SessionReplacement,
}

/// Params for `session/read`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadParams {
    pub target: SessionTarget,
    /// Optional pagination cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

/// Params for `session/turns/list`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTurnsListParams {
    pub target: SessionTarget,
    /// Optional pagination cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

/// Params for passive `session/subscribe`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSubscribeParams {
    pub target: SessionTarget,
    /// Last durable envelope sequence the client already has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<i64>,
}

/// Params for `session/archive`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionArchiveParams {
    pub target: ArchiveTarget,
}

/// Params for `session/rename`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRenameParams {
    pub target: SessionTarget,
    pub name: String,
}

/// Result for `session/rename`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRenameResult {
    pub name: String,
}

/// Params for `session/toggleTag`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToggleTagParams {
    pub target: SessionTarget,
    pub tag: String,
}

/// Result for `session/toggleTag`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionToggleTagResult {
    pub tag: String,
    pub added: bool,
}

/// Result for `session/cost`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionCostResult {
    pub text: String,
    pub usage: SessionUsageSnapshot,
}

/// Result for `session/status`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionStatusResult {
    pub text: String,
}

/// Params for `task/detail`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetailParams {
    pub target: SessionTarget,
    pub task_id: String,
}

/// Result for `task/list`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskListResult {
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub tasks: Vec<TaskStateBase>,
}

/// Result for `task/detail`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetailResult {
    pub task_id: String,
    pub stdout: String,
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}

/// Result for `control/backgroundAllTasks`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundAllTasksResult {
    pub task_ids: Vec<String>,
}

/// Params for `turn/start`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub target: InteractiveTarget,
    pub prompt: String,
    /// Optional full-history override for local compatibility turns that have
    /// already assembled transcript messages outside a plain prompt. Each item
    /// is a serialized `coco_messages::Message`; kept as JSON here to avoid a
    /// reverse dependency from `coco-types` to `coco-messages`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_override: Vec<serde_json::Value>,
    /// Optional clipboard/paste images to include on the user message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<QueuedCommandEditImage>,
    /// Optional slash-command metadata to prepend as a model-visible
    /// attachment before the user prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_metadata: Option<String>,
    /// Optional explicit turn-scoped model selection. When absent, the session
    /// model remains authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ProviderModelSelection>,
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
    pub target: InteractiveTarget,
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
    /// (TUI mode). Consumed by `app/agent-host/src/sdk_server/approval_bridge.rs`.
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
    pub target: InteractiveTarget,
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
    pub target: InteractiveTarget,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetModelParams {
    pub target: InteractiveTarget,
    /// None means revert to the default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Params for `control/setModelRole`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetModelRoleParams {
    pub target: InteractiveTarget,
    pub role: ModelRole,
    pub provider: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
}

/// Params for `control/setPermissionMode`.
///
/// The `ultraplan` field (CCR web-UI refinement flow) is intentionally
/// omitted — see CLAUDE.md "Plan Mode — Skip Ultraplan Only".
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetPermissionModeParams {
    pub target: InteractiveTarget,
    pub mode: PermissionMode,
}

/// Params for `control/setThinking`.
/// Uses `ThinkingLevel` which includes effort level and per-provider options.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetThinkingParams {
    pub target: InteractiveTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
}

/// Params for `control/setAgentColor`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAgentColorParams {
    pub target: InteractiveTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<AgentColorName>,
}

/// Params for `control/applyPermissionUpdate`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPermissionUpdateParams {
    pub target: InteractiveTarget,
    pub update: PermissionUpdate,
}

/// Result for `control/resetSessionPermissionRules`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResetSessionPermissionRulesResult {
    pub cleared_allow_rules: usize,
    pub cleared_deny_rules: usize,
}

/// Params for `control/stopTask`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopTaskParams {
    pub target: InteractiveTarget,
    pub task_id: String,
}

/// Params for `control/rewindFiles`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindFilesParams {
    pub target: InteractiveTarget,
    pub user_message_id: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// Params for `control/updateEnv`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEnvParams {
    pub target: InteractiveTarget,
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
    pub target: InteractiveTarget,
    pub agent_id: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigReadTarget {
    Process,
    Session(SessionTarget),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReadParams {
    pub target: ConfigReadTarget,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWriteTarget {
    User,
    Project(InteractiveTarget),
    Local(InteractiveTarget),
}

/// Params for `config/value/write`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigWriteParams {
    pub target: ConfigWriteTarget,
    pub key: String,
    pub value: serde_json::Value,
}

// --- Gap additions (7) ---

/// Params for `mcp/setServers`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSetServersParams {
    pub target: InteractiveTarget,
    /// Server configs keyed by name.
    pub servers: HashMap<String, serde_json::Value>,
}

/// Params for `mcp/reconnect`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpReconnectParams {
    pub target: InteractiveTarget,
    pub server_name: String,
}

/// Params for `mcp/toggle`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToggleParams {
    pub target: InteractiveTarget,
    pub server_name: String,
    pub enabled: bool,
}

/// Params for `config/applyFlags`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigApplyFlagsParams {
    pub target: InteractiveTarget,
    pub settings: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
#[path = "client_request.test.rs"]
mod tests;
