//! Per-session runtime container shared by both TUI and SDK runners.
//!
//! The TUI runner (`tui_runner::run_tui` / `run_agent_driver`) and the SDK
//! runner (`sdk_server::sdk_runner::QueryEngineRunner`) both need to:
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
//! Before this module existed, both runners had their own copies of
//! steps 1+2+3 — the SDK copy had drifted to ~30% completeness and 7
//! distinct bugs that all had the same shape ("TUI installed X, SDK
//! forgot to install X"). [`SessionRuntime`] is the single owner of
//! that state; both runners construct runtimes through the shared factory, then
//! call [`SessionRuntime::build_engine`] per turn.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_session::SessionManager;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_types::ModelSpec;
use coco_types::PermissionMode;
use coco_types::SessionId;

use crate::Cli;
use coco_app_runtime::ProcessRuntime;
use coco_app_runtime::ProjectServices;

mod agent_catalog;
mod build;
mod clear;
mod engine;
mod factory;
mod handles;
mod hooks;
mod permissions;
mod reload;
mod resources;
mod roles;
mod sandbox;
mod session_handle;
mod state;

pub use coco_app_runtime::SessionRuntimeBootstrap;
pub use factory::SessionRuntimeBootstrapSource;
pub use factory::SessionRuntimeFactory;
pub use factory::SessionRuntimeFactoryOpts;
pub use hooks::spawn_current_session_config_change_watcher;
pub(crate) use permissions::live_permissions;
pub use resources::SessionRuntime;
use resources::*;
pub use roles::RoleOverride;
pub(crate) use roles::resolve_model_selection_from_runtime_config;
#[cfg(test)]
pub(crate) use roles::thinking_level_for_effort_from;
pub(crate) use sandbox::build_sandbox_state;
pub(crate) use sandbox::sandbox_settings_deny_paths;
pub use session_handle::SessionHandle;

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
    pub cli: &'a Cli,
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
    /// SDK runner installs an `SdkPermissionBridge`; TUI passes `None`
    /// and uses interactive approval prompts instead.
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
    /// dynamic prompt sees the same set the SDK `initialize.agents`
    /// listing reports. Empty = no on-disk agents (built-ins only).
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
    /// Built-in catalog toggles. Defaults to [`coco_subagent::BuiltinAgentCatalog::interactive`]
    /// (CLI / TUI sessions); SDK noninteractive callers may pass
    /// [`coco_subagent::BuiltinAgentCatalog::sdk_noninteractive`] to
    /// disable the entire built-in roster.
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Session id to adopt (resume / continue / fork). `None` mints a
    /// fresh per-process uuid. Threaded so every runtime subsystem
    /// (task dirs, task-list id, agent transcripts, usage snapshot)
    /// keys off the SAME id the engine config uses.
    pub session_id_override: Option<SessionId>,
    /// True for SDK / headless (print) sessions. File-history checkpointing
    /// defaults OFF for these and ON for the interactive TUI, unless
    /// overridden by `COCO_FILE_CHECKPOINTING_*`.
    pub is_non_interactive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnginePersistenceMode {
    MainSession,
    Fork,
}

#[cfg(test)]
#[path = "session_runtime.test.rs"]
mod tests;
