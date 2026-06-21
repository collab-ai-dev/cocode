//! Application state tree (Arc<RwLock<AppState>>).

// All swarm orchestration moved to the `coco_coordinator` crate (PR #3
// steps 3-6). Consumers (CLI, query, TUI, tools) import directly from
// `coco_coordinator::*` instead of routing through this crate.

use coco_types::PermissionMode;
use coco_types::ThinkingLevel;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// The central application state.
///
/// 80+ fields spanning model, session, agent, token tracking,
/// tasks, MCP, plugins, notifications, speculation, remote, and feature flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    // ── Model & Config ──
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    #[serde(default)]
    pub fast_mode: bool,
    pub permission_mode: PermissionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_value: Option<String>,
    #[serde(default)]
    pub thinking_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_model: Option<String>,

    // ── Session ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub is_git_repo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,

    // ── Agent ──
    #[serde(default)]
    pub is_busy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(default)]
    pub turn_count: i32,

    // ── Token Tracking ──
    #[serde(default)]
    pub total_input_tokens: i64,
    #[serde(default)]
    pub total_output_tokens: i64,
    #[serde(default)]
    pub total_cost_usd: f64,

    // ── Tasks & Agents ──
    #[serde(default)]
    pub tasks: HashMap<String, TaskEntry>,
    pub agent_names: HashMap<String, String>,
    #[serde(default)]
    pub sub_agents: HashMap<String, SubAgentState>,

    // ── File History ──
    #[serde(default)]
    pub file_history_enabled: bool,
    #[serde(default)]
    pub file_history_snapshot_count: i32,

    // ── Denial Tracking ──
    #[serde(default)]
    pub denial_consecutive: i32,
    #[serde(default)]
    pub denial_total: i32,

    // ── Feature Flags ──
    #[serde(default)]
    pub has_compacted: bool,
    #[serde(default)]
    pub plan_mode: bool,
    #[serde(default)]
    pub mcp_connected: bool,

    // ── MCP ──
    #[serde(default)]
    pub mcp_clients: HashMap<String, McpClientState>,
    #[serde(default)]
    pub mcp_tools: Vec<String>,
    #[serde(default)]
    pub mcp_commands: Vec<String>,
    #[serde(default)]
    pub mcp_resources: Vec<String>,

    // ── Plugins ──
    #[serde(default)]
    pub plugins: PluginState,

    // ── Notifications ──
    #[serde(default)]
    pub notifications: NotificationState,

    // ── Speculation/Pipelining ──
    #[serde(default)]
    pub speculation_enabled: bool,
    #[serde(default)]
    pub speculation_session_time_saved_ms: i64,

    // ── Bridge / IDE ──
    #[serde(default)]
    pub repl_bridge_enabled: bool,
    #[serde(default)]
    pub repl_bridge_connected: bool,
    #[serde(default)]
    pub repl_bridge_session_active: bool,

    // ── Remote ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_connection_status: Option<String>,
    #[serde(default)]
    pub remote_agent_task_suggestions: Vec<String>,

    // ── Inbox / Teammates ──
    #[serde(default)]
    pub inbox_messages: Vec<InboxEntry>,

    /// Standalone agent context (non-team agent identity).
    ///
    /// **Rust status — declared, not yet wired**: this field exists on
    /// the type but no live `Arc<RwLock<AppState>>` instance is held
    /// by any subsystem (engine + tools share `ToolAppState`; TUI
    /// keeps its own `coco_tui::state::AppState`). `/rename` is the
    /// only TS producer; until a TUI banner consumer exists, populating
    /// this field would be dead state. The matching parity gap is
    /// tracked but intentionally **not** plugged from the rename runner
    /// — when a banner widget lands it adds the reader and the
    /// population call together. See `app/cli/src/session_rename.rs`
    /// header comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standalone_agent_context: Option<StandaloneAgentContext>,

    /// Task ID of the teammate currently being viewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_agent_task_id: Option<String>,

    /// Selection index in the in-process agent list.
    #[serde(default)]
    pub selected_ip_agent_index: i32,

    /// Pending permission request on worker side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_worker_request: Option<PendingWorkerRequest>,

    /// Pending sandbox permission request on worker side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_sandbox_request: Option<PendingSandboxRequest>,

    // ── Coordinator Mode ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_task_index: Option<i32>,
    #[serde(default)]
    pub view_selection_mode: bool,

    // ── Elicitation ──
    #[serde(default)]
    pub elicitation_queue: Vec<ElicitationEntry>,

    // ── Sandbox ──
    /// Worker sandbox permission queue (leader side).
    #[serde(default)]
    pub worker_sandbox_permissions: WorkerSandboxPermissions,

    // ── Prompt Suggestion ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestion: Option<String>,

    // ── Onboarding ──
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default)]
    pub onboarding_step: i32,

    // ── Session Hooks ──
    #[serde(default)]
    pub session_hooks_loaded: bool,

    // ── Bootstrap ──
    #[serde(default)]
    pub bootstrap_loaded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_data: Option<serde_json::Value>,
}

/// MCP client connection state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpClientState {
    pub server_name: String,
    pub status: String,
    #[serde(default)]
    pub tool_count: i32,
    #[serde(default)]
    pub error: Option<String>,
}

/// Plugin subsystem state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub errors: HashMap<String, String>,
    #[serde(default)]
    pub installation_status: HashMap<String, String>,
}

/// Notification subsystem state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationState {
    #[serde(default)]
    pub current: Option<NotificationEntry>,
    #[serde(default)]
    pub queue: Vec<NotificationEntry>,
}

/// A single notification entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEntry {
    pub id: String,
    pub message: String,
    #[serde(default)]
    pub level: NotificationLevel,
    pub timestamp: i64,
}

/// Notification severity level.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    #[default]
    Info,
    Warning,
    Error,
}

// `TaskEntry` lifted to `coco_types::agent_ipc` (PR #3 step 6) so the
// coordinator runner-loop scheduler can read it without depending on
// `coco-state`.
pub use coco_types::TaskEntry;

/// Inbox entry from a teammate agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxEntry {
    /// Unique message ID.
    #[serde(default)]
    pub id: String,
    pub from_agent: String,
    pub content: String,
    /// Processing status.
    #[serde(default)]
    pub status: InboxMessageStatus,
    #[serde(default)]
    pub consumed: bool,
    pub timestamp: i64,
    /// Sender's UI color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Brief summary (5-10 words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Inbox message processing status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxMessageStatus {
    #[default]
    Pending,
    Processing,
    Processed,
}

// Team identity / membership types lifted to `coco_types::agent_ipc`
// (PR #3 step 4) so `coco-coordinator` can read/write them without
// depending on `coco-state`. Re-exported here so AppState fields keep
// their historical type names.
pub use coco_types::StandaloneAgentContext;

/// Pending permission request on worker side (waiting for leader response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingWorkerRequest {
    pub tool_name: String,
    pub tool_use_id: String,
    pub description: String,
}

/// Pending sandbox permission request on worker side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSandboxRequest {
    pub request_id: String,
    pub host: String,
}

/// Worker sandbox permission queue (leader side).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerSandboxPermissions {
    #[serde(default)]
    pub queue: Vec<SandboxQueueEntry>,
    #[serde(default)]
    pub selected_index: i32,
}

/// Entry in the sandbox permission queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxQueueEntry {
    pub request_id: String,
    pub worker_id: String,
    pub worker_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_color: Option<String>,
    pub host: String,
    pub created_at: i64,
}

/// Elicitation queue entry (pending user input request from MCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationEntry {
    pub server_name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub timestamp: i64,
}

// Sub-agent state, status, mailbox protocol — moved to `coco_types::agent_ipc`
// so the future `coco-coordinator` crate can read/write them without
// depending on `coco-state`. Re-exported here under the historical names
// (`AgentMessage` / `AgentMessageContent`) used inside `app/state` —
// the canonical names (`TeammateProtocolMessage` / `TeammateProtocolContent`)
// avoid collision with the unrelated `ThreadItemDetails::AgentMessage`
// streaming-content variant in `coco-types::event`.
pub use coco_types::IdleReason;
pub use coco_types::SubAgentState;
pub use coco_types::SubAgentStatus;
pub use coco_types::TeammateProtocolContent as AgentMessageContent;
pub use coco_types::TeammateProtocolMessage as AgentMessage;

/// Thread-safe app state store.
pub type AppStateStore = Arc<RwLock<AppState>>;

/// Create a new app state store with defaults.
pub fn create_app_state() -> AppStateStore {
    Arc::new(RwLock::new(AppState::default()))
}

/// Create app state with initial configuration.
pub fn create_app_state_with(
    model: &str,
    cwd: &str,
    permission_mode: PermissionMode,
) -> AppStateStore {
    Arc::new(RwLock::new(AppState {
        model: model.to_string(),
        working_dir: cwd.to_string(),
        permission_mode,
        ..Default::default()
    }))
}

/// Helper functions on the store.
pub async fn update_token_usage(store: &AppStateStore, input: i64, output: i64) {
    let mut state = store.write().await;
    state.total_input_tokens = state.total_input_tokens.saturating_add(input);
    state.total_output_tokens = state.total_output_tokens.saturating_add(output);
}

pub async fn set_busy(store: &AppStateStore, busy: bool, tool: Option<&str>) {
    let mut state = store.write().await;
    state.is_busy = busy;
    state.current_tool = tool.map(String::from);
}

pub async fn increment_turn(store: &AppStateStore) {
    let mut state = store.write().await;
    state.turn_count += 1;
}

/// Register a new sub-agent.
pub async fn register_sub_agent(store: &AppStateStore, agent: SubAgentState) {
    let mut state = store.write().await;
    state
        .agent_names
        .insert(agent.agent_id.clone(), agent.name.clone());
    state.sub_agents.insert(agent.agent_id.clone(), agent);
}

/// Update a sub-agent's status.
pub async fn update_sub_agent_status(
    store: &AppStateStore,
    agent_id: &str,
    status: SubAgentStatus,
    last_message: Option<String>,
) {
    let mut state = store.write().await;
    if let Some(agent) = state.sub_agents.get_mut(agent_id) {
        agent.status = status;
        if let Some(msg) = last_message {
            agent.last_message = Some(msg);
        }
    }
}

/// Get all running sub-agents.
pub async fn running_sub_agents(store: &AppStateStore) -> Vec<SubAgentState> {
    let state = store.read().await;
    state
        .sub_agents
        .values()
        .filter(|a| a.status == SubAgentStatus::Running)
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
