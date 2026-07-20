//! Per-session runtime container shared by TUI, headless, and AppServer runners.
//!
//! The TUI runner (`tui::run_tui` / `run_agent_driver`), headless
//! runner, and AppServer executor (`app_server_host::SessionTurnExecutor`)
//! all need to:
//!
//! 1. Construct ~12 per-session subsystem state objects at startup
//! (`FileReadState`, `SessionMemoryService`, `HookRegistry`,
//! `CompactionObserverRegistry`, `FileHistoryState`, `ToolAppState`,
//! history Mutex, …).
//! 2. Per-turn, build a `QueryEngine` by chaining ~11 `.with_*` calls
//! that install those subsystems on the engine.
//! 3. For runtime replacement flows such as TUI `/clear`, construct a fresh
//! target-id runtime and swap handles through the local AppServer bridge.
//!
//! Before this module existed, runners had their own copies of steps
//! 1+2+3. The remote/AppServer copy had drifted to ~30% completeness
//! and 7 distinct bugs that all had the same shape ("TUI installed X,
//! remote path forgot to install X"). [`SessionRuntime`] is the single
//! owner of that state; runners construct runtimes through the shared
//! factory, then call [`SessionRuntime::build_engine`] per turn.

use std::{path::PathBuf, sync::Arc};

use tokio::sync::RwLock;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_session::SessionManager;
use coco_tool_runtime::{ToolPermissionBridgeRef, ToolRegistry};
use coco_types::{ModelSpec, PermissionMode, ProviderModelSelection, SessionId, ThinkingLevel};

use crate::AgentHostOptions;
use coco_app_runtime::{ProcessRuntime, ProjectServices};

mod agent_catalog;
mod build;
mod clear;
mod engine;
mod execution_profile;
mod factory;
mod handles;
mod hooks;
mod permissions;
mod reload;
mod resources;
mod roles;
mod sandbox;
mod session_handle;
mod side_chat_seed;
mod state;
mod turn;

pub use clear::ClearReplacementSnapshot;
pub use coco_app_runtime::SessionRuntimeBootstrap;
pub use execution_profile::{HookExecutionPolicy, SessionExecutionProfile};
pub use factory::{
    SessionRuntimeBootstrapSource, SessionRuntimeFactory, SessionRuntimeFactoryHostConfig,
    SessionRuntimeFactoryOpts,
};
pub use hooks::spawn_current_session_config_change_watcher;
pub(crate) use permissions::live_permissions;
pub use permissions::{LivePermissionRulesHandle, PermissionModeChange};
pub(crate) use resources::SessionRuntime;
use resources::*;
pub(crate) use roles::resolve_model_selection_from_runtime_config;
#[cfg(test)]
pub(crate) use roles::thinking_level_for_effort_from;
pub use roles::{RoleOverride, SessionModelRoleChange, SessionModelRoleSelection};
pub(crate) use sandbox::{build_sandbox_state, sandbox_settings_deny_paths};
pub(crate) use session_handle::SessionCloseDrainError;
pub use session_handle::SessionHandle;
pub(crate) use session_handle::ShortcutReservationGuard;
pub use side_chat_seed::SideChatSeed;
pub use turn::SessionStats;
pub(crate) use turn::{
    ActiveTurnDrainState, ActiveTurnHandles, SessionAccounting, SessionTurnCoordinator,
};

#[derive(Clone)]
pub(crate) struct McpRegistrationStatus {
    pub(crate) tool_count: i32,
    pub(crate) skipped_tools: Vec<coco_types::McpSkippedToolStatus>,
    pub(crate) tombstoned_tools: Vec<String>,
}

pub enum SessionMcpConnectionChange {
    Connected,
    NeedsAuth {
        transport: String,
        url: Option<String>,
    },
    Disconnected,
    Failed(String),
}

pub struct SessionInitializeCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
}

pub struct SessionInitializeAgent {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAccountProvider {
    FirstParty,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionInitializeAccount {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub subscription_type: Option<String>,
    pub token_source: Option<String>,
    pub api_key_source: Option<String>,
    pub api_provider: Option<SessionAccountProvider>,
}

pub struct SessionInitializeMetadata {
    pub commands: Vec<SessionInitializeCommand>,
    pub agents: Vec<SessionInitializeAgent>,
    pub output_style: String,
    pub available_output_styles: Vec<String>,
}

pub struct SessionStartRuntimeConfig {
    pub model_id: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub max_turns: Option<i32>,
    pub max_budget_usd: Option<f64>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub agent_progress_summaries_enabled: bool,
    pub plan_mode_custom_instructions: Option<Option<String>>,
    pub requires_structured_output: bool,
}

pub struct SessionTurnPermissionRefresh {
    pub fallback_previous_mode: PermissionMode,
    pub permission_mode: PermissionMode,
    pub allow_rules: coco_types::PermissionRulesBySource,
    pub deny_rules: coco_types::PermissionRulesBySource,
    pub ask_rules: coco_types::PermissionRulesBySource,
    pub permission_rule_source_roots:
        std::collections::HashMap<coco_types::PermissionRuleSource, PathBuf>,
    pub plan_auto_options: coco_permissions::PlanModeAutoOptions,
}

pub struct SessionTurnRuntimeConfig {
    pub is_non_interactive: bool,
    pub avoid_permission_prompts: bool,
    pub permission_mode: PermissionMode,
    pub permission_mode_availability: coco_types::PermissionModeAvailability,
    pub permission_rule_source_roots:
        std::collections::HashMap<coco_types::PermissionRuleSource, PathBuf>,
    pub max_turns: Option<i32>,
    pub total_token_budget: Option<i64>,
    pub cwd_override: Option<PathBuf>,
    pub tool_filter: coco_types::ToolFilter,
    pub plans_directory: Option<String>,
    pub plan_mode_custom_instructions: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionPluginReloadReport {
    pub plugins: Vec<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    pub hook_error_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionTaskError {
    #[error("task runtime is not available for this session")]
    NotAvailable,
    #[error("{0}")]
    Read(String),
    #[error("{0}")]
    Control(String),
}

pub struct SessionTurnEngineConfigRequest {
    pub model_selection: Option<ProviderModelSelection>,
    pub permission_mode: Option<PermissionMode>,
    pub thinking_level: Option<ThinkingLevel>,
    pub max_turns: Option<i32>,
    pub system_prompt: Option<String>,
}

pub(crate) struct SessionTurnEngineConfig {
    pub(crate) config: coco_query::QueryEngineConfig,
    pub(crate) model_runtime_source: coco_inference::ModelRuntimeSource,
    pub(crate) model_id: String,
    pub(crate) turn_cwd: PathBuf,
}

pub struct SessionTurnEngine {
    pub engine: coco_query::QueryEngine,
    pub model_id: String,
    pub turn_cwd: PathBuf,
    pub(crate) has_session_permission_bridge: bool,
}

pub struct SessionFileRewindRequest {
    pub user_message_id: String,
    pub dry_run: bool,
}

pub struct SessionFileRewindResult {
    pub files_changed: Vec<PathBuf>,
    pub insertions: i64,
    pub deletions: i64,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionHistoryTruncateResult {
    pub keep_count: usize,
    pub pre_count: usize,
    pub removed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionFileRewindError {
    #[error("file history not enabled on this server")]
    NotEnabled,
    #[error("no snapshot for user_message_id {0}")]
    SnapshotMissing(String),
    #[error("{context}: {source}")]
    Operation {
        context: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SessionFileDiffError {
    #[error("file history not enabled for this session")]
    NotEnabled,
    #[error("no snapshot found for message id {0}")]
    SnapshotMissing(String),
    #[error("{context}: {source}")]
    Operation {
        context: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

fn clone_std_rwlock<T: Clone>(lock: &std::sync::RwLock<T>) -> T {
    match lock.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn write_std_rwlock<T>(lock: &std::sync::RwLock<T>, value: T) {
    match lock.write() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

/// Options for building a [`SessionRuntime`].
pub struct SessionRuntimeBuildOpts<'a> {
    pub cli: &'a AgentHostOptions,
    pub runtime_config: Arc<RuntimeConfig>,
    pub config_reloader: Option<coco_config_reload::RuntimeReloader>,
    pub cwd: PathBuf,
    pub model_id: String,
    pub system_prompt: String,
    pub permission_mode_availability: coco_types::PermissionModeAvailability,
    pub permission_mode: PermissionMode,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub tools: Arc<ToolRegistry>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    /// Optional in-process permission bridge. When absent, AppServer turn
    /// execution installs its client-facing permission bridge.
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    /// Slash-command registry — populated once at startup via
    /// `coco_commands::build_command_registry`. Both typed `/foo`
    /// dispatch and command-palette execution snapshot this registry
    /// before sending model-bound follow-ups through AppServer turn/start.
    /// Wrapped in `RwLock` so `/reload-plugins` can rebuild and swap
    /// without restarting the session — consumers snapshot the inner
    /// `Arc<CommandRegistry>` once per dispatch via
    /// [`SessionRuntime::current_command_registry`].
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped `SkillManager` — same Arc that backed
    /// `command_registry`'s skill load, kept alive so the per-turn
    /// reminder pipeline (`SkillsSource`) reads the same catalog.
    pub skill_manager: Arc<coco_skills::SkillManager>,
    /// Project-scoped services/catalog loaded for this session's project root.
    pub project_services: Arc<ProjectServices>,
    /// Process-scoped owner used for project-service reloads during this
    /// session's lifetime.
    pub process_runtime: Arc<ProcessRuntime>,
    /// Where to look for markdown agent definitions. Threaded into the
    /// runtime's [`coco_subagent::AgentDefinitionStore`] so AgentTool's
    /// dynamic prompt sees the same set the initialize `agents`
    /// listing reports. Empty = no on-disk agents (built-ins only).
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
    /// Built-in catalog toggles. Defaults to [`coco_subagent::BuiltinAgentCatalog::interactive`]
    /// (CLI / TUI sessions); noninteractive callers may pass
    /// [`coco_subagent::BuiltinAgentCatalog::noninteractive`] to
    /// disable the entire built-in roster.
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Session id to adopt (resume / continue / fork). `None` mints a
    /// fresh per-process uuid. Threaded so every runtime subsystem
    /// (task dirs, task-list id, agent transcripts, usage snapshot)
    /// keys off the SAME id the engine config uses.
    pub session_id_override: Option<SessionId>,
    /// True for noninteractive remote / headless (print) sessions. File-history checkpointing
    /// defaults OFF for these and ON for the interactive TUI, unless
    /// overridden by `COCO_FILE_CHECKPOINTING_*`.
    pub is_non_interactive: bool,
    /// Construction-time capability profile. `Primary` installs every
    /// configured durable/background subsystem; `SideChatReadOnly` is the
    /// ephemeral read-only sidechat child (no persistence, PID, goals, memory,
    /// skills, suggestions, tasks, or title). Gated in `build` /
    /// `install_session_late_binds` like `is_non_interactive`.
    pub execution_profile: SessionExecutionProfile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnginePersistenceMode {
    MainSession,
    Fork,
}

#[cfg(test)]
#[path = "session_runtime.test.rs"]
mod tests;
