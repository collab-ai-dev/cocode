//! Per-session runtime container shared by both TUI and SDK runners.
//!
//! The TUI runner (`tui_runner::run_tui` / `run_agent_driver`) and the SDK
//! runner (`sdk_server::sdk_runner::QueryEngineRunner`) both need to:
//!
//! 1. Construct ~12 per-session subsystem state objects at startup
//!    (`FileReadState`, `SessionMemoryService`, `HookRegistry`,
//!    `CompactionObserverRegistry`, `FileHistoryState`, `ToolAppState`,
//!    history Mutex, …).
//! 2. Per-turn, build a `QueryEngine` by chaining ~11 `.with_*` calls
//!    that install those subsystems on the engine.
//! 3. On `/clear`, perform a full reset (SessionEnd hooks → drop
//!    caches → regen session id → SessionStart hooks).
//!
//! Before this module existed, both runners had their own copies of
//! steps 1+2+3 — the SDK copy had drifted to ~30% completeness and 7
//! distinct bugs that all had the same shape ("TUI installed X, SDK
//! forgot to install X"). [`SessionRuntime`] is the single owner of
//! that state; both runners construct one at startup, then call
//! [`SessionRuntime::build_engine`] per turn and
//! [`SessionRuntime::clear_conversation`] on `/clear`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::info;
use tracing::warn;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_context::FileHistorySnapshotSink;
use coco_context::FileHistoryState;
use coco_context::FileReadState;
use coco_hooks::HookRegistry;
use coco_memory::SessionMemoryService;
use coco_messages::CostTracker;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_query::CommandQueue;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::SessionStartHookSideEffectSink;
use coco_query::SessionStartHookSideEffects;
use coco_session::SessionManager;
use coco_session::TranscriptStore;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::MailboxHandleRef;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::TurnAbortSignal;
use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::PermissionMode;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use coco_types::ToolAppState;
use tokio_util::sync::CancellationToken;

use crate::Cli;

/// `FileHistorySnapshotSink` that writes via [`TranscriptStore`]. Lives
/// here because both runners need to install it on `FileHistoryState`.
///
/// **Deliberately bypasses the `SessionStore` backend trait** and
/// constructs a concrete disk [`TranscriptStore`] directly: file-history
/// checkpoints are a low-frequency, local-only convenience (rewind / disk
/// backup), not authoritative session state — they are not needed to
/// recover a conversation and are explicitly declared local-cache
/// (`docs/coco-rs/session-storage-backend-design.md` §3.4). So even under
/// a non-`Disk` `session.backend` they stay on disk; there's nothing to
/// gain from routing them through the swappable boundary.
///
/// `session_id` is shared via `Arc<std::sync::RwLock<String>>` so
/// `SessionRuntime::clear_conversation` can swap it in place without
/// rebuilding the sink. Without this, a `/clear` regen would leave the
/// sink writing to the OLD session's transcript jsonl forever — phantom
/// snapshots from the new session leaking into the resumed old one.
struct TranscriptFileHistorySink {
    store: TranscriptStore,
    session_id: Arc<std::sync::RwLock<String>>,
}

impl TranscriptFileHistorySink {
    fn new(
        project_paths: Arc<coco_paths::ProjectPaths>,
        session_id: Arc<std::sync::RwLock<String>>,
    ) -> Self {
        Self {
            store: TranscriptStore::new(project_paths),
            session_id,
        }
    }
}

#[derive(Clone)]
struct FileWatchRegistrationContext {
    file_changed_watcher: Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>>,
    hook_registry: Arc<HookRegistry>,
    session_id: Arc<RwLock<String>>,
    engine_config: Arc<RwLock<QueryEngineConfig>>,
    cancel: CancellationToken,
    async_hook_registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    command_queue: CommandQueue,
    hook_llm_handle: Arc<dyn coco_hooks::HookLlmHandle>,
}

struct QuerySessionStartHookSink {
    file_watch: FileWatchRegistrationContext,
}

fn async_rewake_sink(queue: &CommandQueue) -> Arc<dyn coco_hooks::AsyncRewakeSink> {
    Arc::new(crate::command_queue_sink::CommandQueueNotificationSink::new(queue.clone()))
}

#[async_trait::async_trait]
impl SessionStartHookSideEffectSink for QuerySessionStartHookSink {
    async fn handle_session_start_hook_side_effects(&self, effects: SessionStartHookSideEffects) {
        if effects.watch_paths.is_empty() {
            return;
        }
        self.file_watch.add_paths(effects.watch_paths).await;
    }
}

impl FileWatchRegistrationContext {
    async fn add_paths(&self, paths: Vec<String>) {
        let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let mut slot = self.file_changed_watcher.write().await;
        if slot.is_none() {
            let registry = self.hook_registry.clone();
            let session_id = self.session_id.read().await.clone();
            let cfg = self.engine_config.read().await.clone();
            let disable_all_hooks = cfg.disable_all_hooks;
            let allow_managed_hooks_only = cfg.allow_managed_hooks_only;
            let project_dir = cfg.project_dir;
            let cwd = std::env::current_dir().unwrap_or_default();
            let cancel = self.cancel.clone();
            let async_registry = self.async_hook_registry.clone();
            let rewake_sink = async_rewake_sink(&self.command_queue);
            let llm_handle = self.hook_llm_handle.clone();
            let factory: Arc<
                dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync,
            > = Arc::new(move || coco_hooks::orchestration::OrchestrationContext {
                session_id: session_id.clone(),
                cwd: cwd.clone(),
                project_dir: project_dir.clone(),
                permission_mode: None,
                transcript_path: None,
                agent_id: None,
                agent_type: None,
                cancel: cancel.clone(),
                disable_all_hooks,
                allow_managed_hooks_only,
                attachment_emitter: coco_messages::AttachmentEmitter::noop(),
                sync_event_sink: None,
                http_url_allowlist: None,
                http_env_var_policy: None,
                async_registry: Some(async_registry.clone()),
                async_rewake_sink: Some(rewake_sink.clone()),
                llm_handle: Some(llm_handle.clone()),
                workspace_trust_accepted: None,
            });
            *slot = crate::file_changed_watcher::FileChangedHookWatcher::new(registry, factory);
        }
        if let Some(watcher) = slot.as_ref() {
            watcher.add_paths(path_bufs);
        }
    }
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

#[async_trait::async_trait]
impl FileHistorySnapshotSink for TranscriptFileHistorySink {
    async fn record(
        &self,
        message_id: &str,
        snapshot_json: serde_json::Value,
        is_snapshot_update: bool,
    ) {
        let id = self
            .session_id
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        if let Err(e) = self.store.insert_file_history_snapshot(
            &id,
            message_id,
            snapshot_json,
            is_snapshot_update,
        ) {
            warn!(error = %e, message_id, "failed to persist file-history snapshot");
        }
    }
}

/// Map a coco-config-reload [`TrackedKind`] to the `ConfigChangeSource`
/// wire string consumed by the `ConfigChange` hook. Catalog files
/// (`providers.json`, `models.json`) live alongside the user settings
/// in `~/.coco/`, so they share the `user_settings` source.
/// `flag_settings` falls back to `user_settings` since there is no
/// flag-settings hook source variant.
fn config_change_source_for_kind(
    kind: coco_config_reload::TrackedKind,
) -> coco_hooks::orchestration::ConfigChangeSource {
    use coco_config::SettingSource;
    use coco_config::WatchedKind;
    use coco_config_reload::TrackedKind;
    use coco_hooks::orchestration::ConfigChangeSource;
    match kind {
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::User)) => {
            ConfigChangeSource::UserSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Project)) => {
            ConfigChangeSource::ProjectSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Local)) => {
            ConfigChangeSource::LocalSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Policy)) => {
            ConfigChangeSource::PolicySettings
        }
        TrackedKind::Settings(WatchedKind::Settings(
            SettingSource::Plugin | SettingSource::Flag,
        ))
        | TrackedKind::Settings(WatchedKind::ProvidersCatalog | WatchedKind::ModelsCatalog)
        | TrackedKind::FlagSettings => ConfigChangeSource::UserSettings,
    }
}

/// Populate a `HookRegistry` from the current `RuntimeConfig` snapshot
/// + the plugin directories rooted at `config_home`/`cwd`.
///
/// Used both at session bootstrap (`SessionRuntime::new`) and at
/// `/hooks reload` time (`SessionRuntime::reload_hooks`). Settings
/// sources are loaded in lowest-precedence-first order so the registry
/// vec uses deterministic iteration order.
fn populate_hook_registry(
    registry: &HookRegistry,
    runtime_config: &coco_config::RuntimeConfig,
    config_home: &std::path::Path,
    cwd: &std::path::Path,
) {
    let policy = coco_hooks::LoaderPolicy {
        disable_all_hooks: runtime_config.settings.merged.disable_all_hooks,
        allow_managed_hooks_only: runtime_config.settings.merged.allow_managed_hooks_only,
    };
    for source in [
        coco_config::SettingSource::User,
        coco_config::SettingSource::Project,
        coco_config::SettingSource::Local,
        coco_config::SettingSource::Flag,
        coco_config::SettingSource::Policy,
    ] {
        let Some(value) = runtime_config.settings.per_source.get(&source) else {
            continue;
        };
        let Some(hooks_value) = value.get("hooks") else {
            continue;
        };
        let scope = match source {
            coco_config::SettingSource::User => coco_types::HookScope::User,
            coco_config::SettingSource::Project => coco_types::HookScope::Project,
            coco_config::SettingSource::Local => coco_types::HookScope::Local,
            // Flag is treated as Local — closest to the user's
            // explicit per-invocation override.
            coco_config::SettingSource::Flag => coco_types::HookScope::Local,
            coco_config::SettingSource::Policy => coco_types::HookScope::Policy,
            coco_config::SettingSource::Plugin => coco_types::HookScope::Plugin,
        };
        match coco_hooks::load_hooks_from_config_with_policy(hooks_value, scope, policy) {
            Ok(definitions) => {
                for def in definitions {
                    registry.register_deduped(def);
                }
            }
            Err(e) => {
                warn!(error = %e, source = %source, "failed to load hooks from settings — source skipped");
            }
        }
    }
    // Plugin hooks: load the full ENABLED plugin set via the unified V2
    // orchestrator (marketplace versioned cache + local `inline` dirs, gated by
    // settings.json `enabled_plugins`) — not just the local-dir, all-enabled V1
    // scan. `register_plugin_hooks_v2` uses `register_deduped` so a plugin
    // re-declaring a settings hook stays single-fire.
    let plugins = coco_plugins::load_enabled_plugins(config_home, cwd);
    if !plugins.is_empty() {
        info!(
            plugins = plugins.len(),
            "loaded {} enabled plugin(s)",
            plugins.len()
        );
        let refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
        coco_plugins::hook_bridge::register_plugin_hooks_v2(registry, &refs);
    }
}

/// Options for building a [`SessionRuntime`].
pub struct SessionRuntimeBuildOpts<'a> {
    pub cli: &'a Cli,
    pub runtime_config: Arc<RuntimeConfig>,
    pub cwd: PathBuf,
    pub model_id: String,
    pub system_prompt: String,
    pub bypass_permissions_available: bool,
    pub permission_mode: PermissionMode,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub tools: Arc<ToolRegistry>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    /// SDK runner installs an `SdkPermissionBridge`; TUI passes `None`
    /// and uses interactive approval prompts instead.
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    /// Slash-command registry — populated once at startup via
    /// `coco_commands::build_command_registry`. Both the typed
    /// `/foo` path (`process_submit_turn`) and the command-palette
    /// path (`UserCommand::ExecuteSkill`) dispatch through this.
    /// Wrapped in `RwLock` so `/reload-plugins` can rebuild and swap
    /// without restarting the session — consumers snapshot the inner
    /// `Arc<CommandRegistry>` once per dispatch via
    /// [`SessionRuntime::current_command_registry`].
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped `SkillManager` — same Arc that backed
    /// `command_registry`'s skill load, kept alive so the per-turn
    /// reminder pipeline (`SkillsSource`) reads the same catalog.
    pub skill_manager: Arc<coco_skills::SkillManager>,
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
    pub session_id_override: Option<String>,
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

/// Build the live permission base (S1) for the main session's `ToolAppState`
/// from the loaded rule maps + mode + dirs + source roots. This is the single
/// seeding shape used at bootstrap, `/clear` re-seed, and the headless/SDK
/// entry — so the live store (the TS `appState.toolPermissionContext` mirror,
/// the ONLY permission source the factory reads each batch) always starts from
/// the same source.
pub(crate) fn live_permissions(
    mode: PermissionMode,
    allow_rules: coco_types::PermissionRulesBySource,
    deny_rules: coco_types::PermissionRulesBySource,
    ask_rules: coco_types::PermissionRulesBySource,
    additional_dirs: std::collections::HashMap<String, coco_types::AdditionalWorkingDir>,
    permission_rule_source_roots: std::collections::HashMap<
        coco_types::PermissionRuleSource,
        std::path::PathBuf,
    >,
) -> coco_types::LiveToolPermissionState {
    coco_types::LiveToolPermissionState {
        mode: Some(mode),
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        allow_rules,
        deny_rules,
        ask_rules,
        additional_dirs,
        permission_rule_source_roots,
    }
}

/// Construct a [`ThinkingLevel`] for an effort, threading the model's
/// declared `supported_thinking_levels` budget when one is registered.
/// Falls back to a budget-less level (`budget_tokens: None`) so the
/// inference layer's provider-specific conversion picks defaults.
///
/// Lookup is L0-only (`builtin_models_partial`) — same source the TUI
/// picker uses today. Users registering a model in
/// `~/.coco/models.json` without declaring `supported_thinking_levels`
/// get a budget-less wire entry, which is the same behaviour as before
/// this override layer landed.
fn thinking_level_for_effort_from(model_id: &str, effort: ReasoningEffort) -> ThinkingLevel {
    if let Some(level) = coco_config::builtin_models_partial()
        .get(model_id)
        .and_then(|info| info.supported_thinking_levels.as_ref())
        .and_then(|levels| levels.iter().find(|l| l.effort == effort))
    {
        return level.clone();
    }
    ThinkingLevel {
        effort,
        budget_tokens: None,
        options: std::collections::HashMap::new(),
    }
}

/// In-memory binding for a single [`ModelRole`] that overrides the
/// `RuntimeConfig.model_roles` entry for the lifetime of one session.
///
/// Populated by the TUI model picker (`UserCommand::SetModelRole` →
/// [`SessionRuntime::apply_role_override`]) and Ctrl+T thinking cycle
/// (`UserCommand::SetThinkingLevel` →
/// [`SessionRuntime::apply_role_effort`]). The picker carries an
/// explicit `effort`; Ctrl+T preserves the spec and only changes
/// `effort`.
#[derive(Debug, Clone)]
pub struct RoleOverride {
    /// `(provider, model_id, display_name, api)` for the role.
    pub spec: ModelSpec,
    /// User's explicit effort choice. `None` ⇒ engine reaches for the
    /// model's `default_thinking_level` (or provider default if the
    /// model doesn't declare one).
    pub effort: Option<ReasoningEffort>,
}

/// Shared handle to a `QueryEngine`'s post-turn cache-safe-params slot, as
/// returned by `QueryEngine::cache_safe_params_handle`. Kept as an alias so
/// the runtime field that stores the latest one stays readable.
type CacheParamsHandle = Arc<RwLock<Option<coco_types::CacheSafeParams>>>;

/// All per-session state shared by both runners. Construction at startup
/// is done once via [`SessionRuntime::build`]; per-turn engines are
/// assembled via [`SessionRuntime::build_engine`].
pub struct SessionRuntime {
    // ── immutable resources (never change after build) ─────────────────
    /// Tool registry shared by every engine instance. Read by
    /// [`Self::build_engine`] / [`Self::build_engine_from_config`].
    tools: Arc<ToolRegistry>,
    /// Slash-command registry. Read by
    /// [`crate::tui_runner::dispatch_slash_command`] to resolve every
    /// `/foo` typed by the user or selected from the command palette.
    /// Wrapped in `RwLock` so `/reload-plugins` can rebuild and swap
    /// without restarting the session — consumers snapshot the inner
    /// `Arc<CommandRegistry>` once per dispatch via
    /// [`Self::current_command_registry`] so a concurrent swap can't
    /// invalidate borrows.
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped skill catalog. Cloned into `ReminderSources`
    /// (`SkillsSource`) on every per-turn engine so the model receives
    /// the `skill_listing` reminder that gates on
    /// `skill_manager.is_empty()`.
    pub(crate) skill_manager: Arc<coco_skills::SkillManager>,
    pub config_home: PathBuf,
    pub runtime_config: Arc<RuntimeConfig>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    schedule_store: coco_tool_runtime::ScheduleStoreRef,
    model_runtimes: Arc<coco_inference::ModelRuntimeRegistry>,
    side_query: coco_tool_runtime::SideQueryHandle,
    pub auto_title_enabled: bool,
    /// SwarmMailbox handle installed on every engine via `with_mailbox`.
    mailbox: MailboxHandleRef,
    /// Optional SDK permission bridge (None for TUI). Installed via
    /// `with_permission_bridge` when present.
    permission_bridge: Option<ToolPermissionBridgeRef>,
    /// Long-lived parent token for runtime-level lifecycle (hook
    /// orchestration shutdown). Per-turn engine cancels are
    /// independent — see TUI `run_agent_driver` for per-iteration
    /// `CancellationToken::new()`.
    cancel: CancellationToken,

    /// Original CWD captured at session start. Frozen for the lifetime
    /// of this [`SessionRuntime`] — never moves even if the user
    /// `cd`'s away inside a Bash command. Used as the anchor for
    /// `reset_cwd_if_outside_project` (when bash drifts out of the
    /// allowed working directory set, we snap it back here) and for
    /// "Shell cwd was reset to …" stderr annotations.
    pub original_cwd: PathBuf,

    // ── mutable per-session state (changes on /clear or mid-session) ──
    /// Currently active CWD. Updated **across BashTool calls** so the
    /// model's `cd /tmp` in one turn survives into the next turn.
    /// Threaded into every `ToolUseContext` via the engine config so
    /// BashTool can read it as the spawn cwd and write back from
    /// `CommandResult.new_cwd`.
    pub current_cwd: Arc<RwLock<PathBuf>>,
    /// Session id; mutated by [`Self::clear_conversation`] (regen).
    session_id: Arc<RwLock<String>>,
    /// Engine config; mutated by [`Self::clear_conversation`] (session_id)
    /// and [`Self::update_engine_config`]. Read by every per-turn build.
    engine_config: Arc<RwLock<QueryEngineConfig>>,
    /// Synchronous snapshot for detached hook factories. Those
    /// factories run from async tasks but expose a sync `Fn()`, so they
    /// must not call Tokio `blocking_read()` on runtime worker threads.
    orchestration_session_id: Arc<std::sync::RwLock<String>>,
    orchestration_engine_config: Arc<std::sync::RwLock<QueryEngineConfig>>,
    /// Per-session in-memory model-role overrides. Populated by the TUI
    /// model picker (`UserCommand::SetModelRole`) and Ctrl+T thinking
    /// cycle (`UserCommand::SetThinkingLevel`). Layered ABOVE
    /// `runtime_config.model_roles` — [`Self::resolve_role`] checks
    /// overrides first, falls back to the runtime config map second.
    ///
    /// **Not persisted.** Model-role changes via the TUI are session-local;
    /// users who want a binding to survive across sessions edit
    /// `~/.coco.json::model_roles.<role>.primary` themselves.
    ///
    /// Cleared on `Drop` (i.e. session end) via the natural `Arc`
    /// lifecycle. `/clear` keeps overrides — the conversation reset is
    /// orthogonal to model-role bindings.
    role_overrides: Arc<RwLock<HashMap<ModelRole, RoleOverride>>>,
    pub file_read_state: Arc<RwLock<FileReadState>>,
    pub file_history: Option<Arc<RwLock<FileHistoryState>>>,
    pub app_state: Arc<RwLock<ToolAppState>>,
    /// Session-scoped peer-message store, shared (one `Arc`) by every
    /// per-turn engine built via `wire_engine` — including in-process
    /// teammate engines. `SendMessage` pushes into it (`ToolUseContext.
    /// pending_messages`) and the recipient drains it via the
    /// `agent_pending_messages` system-reminder (`SwarmAdapter`). The two
    /// sites MUST share this exact `Arc`, else messages vanish.
    pending_message_store: coco_tool_runtime::PendingMessageStoreRef,
    /// Session-scoped Auto mode classifier state. Installed on every
    /// per-turn engine so `permission_mode = Auto` can auto-approve
    /// safe/read-only tools before falling back to interactive approval.
    auto_mode_state: Arc<coco_permissions::AutoModeState>,
    /// Denial history for Auto mode classifier decisions. Shared across
    /// per-turn engines and cleared when the session changes or compacts.
    denial_tracker: Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
    /// Session-memory extractor + on-disk cache. The same `Arc` as
    /// `memory_runtime.session_memory` when `Feature::AutoMemory` is
    /// on, otherwise `None`. Used by [`Self::wire_engine`] (engine
    /// reads `current_text`) and [`Self::start_new_session`] /
    /// [`Self::clear_conversation`] (session-id retarget + cache wipe).
    session_memory_service: Option<Arc<SessionMemoryService>>,
    /// Auto-memory runtime — extraction / dream / 9-section session
    /// memory / recall ranker. `None` when `Feature::AutoMemory` is
    /// off; otherwise threaded into every engine via
    /// [`coco_query::QueryEngine::with_memory_runtime`].
    memory_runtime: Option<Arc<coco_memory::MemoryRuntime>>,
    /// Real `AgentHandle` for `AgentTool` calls and forked subagents.
    /// Constructed once at session start, installed on every engine
    /// via `wire_engine`. `send_message`, team mgmt, async-launched
    /// agent ops work; sync subagent spawns work once the engine
    /// factory is wired (separately).
    swarm_agent_handle: coco_tool_runtime::AgentHandleRef,
    /// Hook registry merged from settings + plugin manifests. Installed
    /// on every engine + driven by SessionStart / SessionEnd in
    /// [`Self::clear_conversation`].
    pub(crate) hook_registry: Arc<HookRegistry>,
    /// LLM-driven hook handler — implements
    /// [`coco_hooks::HookLlmHandle`] for `Prompt` (full impl) and
    /// `Agent` (stub returning Cancelled — silent fallback) hook
    /// handlers. Threaded into every `OrchestrationContext` so settings
    /// hooks of `type: "prompt"` / `type: "agent"` actually reach an
    /// LLM instead of falling back to passthrough text.
    pub(crate) hook_llm_handle: Arc<coco_query::hook_llm::QueryHookLlm>,
    /// Shared sync-hook-event buffer. SessionStart and UserPromptSubmit
    /// orchestration calls push `HookEvent`s here; the
    /// [`coco_hooks::reminder_source::CombinedHookEventsSource`]
    /// installed on every per-turn engine drains them into the
    /// reminder pipeline. Lifetime spans the whole session — same
    /// instance flows through `OrchestrationContext.sync_event_sink`
    /// and `QueryEngine::sync_hook_buffer`.
    pub(crate) sync_hook_buffer: coco_hooks::SyncHookEventBuffer,
    /// Async hook bookkeeping. Currently no production code path
    /// registers async hooks, but the slot is wired into the combined
    /// reminder source so when async hook execution lands it surfaces
    /// `async_hook_response` reminders without further plumbing.
    pub(crate) async_hook_registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    /// FileChanged hook watcher. Populated when the runtime's hook
    /// registry has any handlers for the `FileChanged` event;
    /// `None` otherwise. Paths are registered lazily from
    /// `SessionStart` / `CwdChanged` hook output.
    pub(crate) file_changed_watcher:
        Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>>,
    /// Multi-turn agent transcript. Each turn snapshots, appends, and
    /// rewrites this on success. Wrapped in `MessageHistory` (the same
    /// type the engine loop uses internally) so TUI-initiated pushes
    /// can call `history_push_and_emit` directly without converting at
    /// the lock boundary.
    pub history: Arc<Mutex<MessageHistory>>,
    /// Shared session id of the `TranscriptFileHistorySink` (when
    /// file_history is enabled). `clear_conversation` writes the
    /// regenerated session id here so the sink targets the new
    /// session's jsonl on the next snapshot. `None` when
    /// file_checkpointing is disabled.
    file_history_sink_session_id: Option<Arc<std::sync::RwLock<String>>>,
    /// Agent-spawn handle used by `AgentTool` / coordinator-mode
    /// workers. Late-bound after `TaskRuntime` is attached because
    /// `SwarmAgentHandle` requires the canonical TaskManager-backed
    /// registry at construction.
    agent_handle: Arc<RwLock<Option<AgentHandleRef>>>,
    /// Skill-execution handle (`QuerySkillRuntime`). Late-bound for the
    /// same Arc-cycle reason as `agent_handle`: the real impl wraps the
    /// subagent `AgentQueryEngineRef` (built in `agent_handle_factory`).
    /// Installed on every per-turn engine via `wire_engine` so the model's
    /// `SkillTool` resolves; `None` ⇒ engine falls back to
    /// `NoOpSkillHandle` (every skill call returns `Unavailable`).
    skill_handle: Arc<RwLock<Option<coco_tool_runtime::SkillHandleRef>>>,
    /// Shared, per-turn-refreshed Bash handle for in-prompt skill shell
    /// expansion (`` !`cmd` ``). Set on every main-session engine build
    /// (`build_engine`) with the same `SessionBashToolHandle` injected into
    /// the command registry, and read by `QuerySkillRuntime` so the
    /// model-invoked + fork-mode skill paths run identical permission-checked
    /// shell. `std::sync::RwLock` (snapshot read, no guard across await).
    skill_bash_cell:
        Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>>,
    /// Post-turn fork dispatcher (D1/D2). Same late-bind pattern as
    /// `agent_handle`: built after `build()` returns the `Arc<Self>`
    /// (the dispatcher impl captures the runtime), and installed on
    /// every per-turn engine via `wire_engine`. `None` ⇒ post-turn
    /// forks degrade to no-op (`/btw` returns a hint message,
    /// `promptSuggestion` skips). Real impl lives in
    /// `app/cli/src/fork_dispatcher.rs`.
    fork_dispatcher: Arc<RwLock<Option<coco_query::forked_agent::ForkDispatcherRef>>>,
    /// Latest per-turn engine's cache-safe-params handle, captured on every
    /// `build_engine`. The `QueryEngine` is rebuilt each turn, but this `Arc`
    /// is shared with the engine's slot, so it keeps observing the params the
    /// engine writes at turn finalize and survives the engine drop. A
    /// between-turns `/btw` reads it to share the parent turn's prompt cache
    /// (the SDK path reads the same data off its persistent `engine`).
    /// `None` until the first engine is built; the inner `Option` is `None`
    /// until the first turn finalises (or after `/clear`).
    last_engine_cache_handle: Arc<RwLock<Option<CacheParamsHandle>>>,
    /// Teammate-scoped live permission-rule overlay, injected onto every
    /// main-session engine's `QueryEngineConfig.live_permission_rules` (which
    /// `ToolContextFactory` merges each batch, post-derivation).
    ///
    /// Since the main-session permission base now lives in `ToolAppState`
    /// (read-through + mutated by `apply_permission_updates_everywhere`), this
    /// overlay is NO LONGER the main-session in-cycle mechanism. It survives as
    /// the channel an in-process teammate uses for leader-pushed
    /// `TeamPermissionUpdate` rules (cross-process teammates use the mailbox →
    /// `runner_loop` `team_permission_rules` analog). Empty for a plain main
    /// session.
    live_permission_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>>,
    /// Session-scoped abort token for the in-flight prompt-suggestion
    /// fork. When a new suggestion fork starts, we cancel the previous
    /// one so users rapidly cycling `/clear` don't accumulate fork tasks
    /// burning tokens. `None` ⇒ no fork in flight.
    pub current_suggestion_abort:
        Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    /// Background task runtime (TaskHandle implementation) — owns
    /// the `TaskManager` + per-task control state. Shared with
    /// `SwarmAgentHandle` so AgentTool's bg path registers spawns
    /// through the same store the engine's `Task*` tools read from.
    /// `None` resolves to `NoOpTaskHandle` semantics (the task tools
    /// surface a clean "no task runtime configured" error).
    task_runtime: Arc<RwLock<Option<Arc<crate::task_runtime::TaskRuntime>>>>,
    /// Durable task-list store shared by the leader, AgentTool children,
    /// and in-process teammates.
    task_list: Arc<RwLock<Option<coco_tool_runtime::TaskListHandleRef>>>,
    team_task_list_router: Arc<RwLock<Option<coco_tool_runtime::TeamTaskListRouterRef>>>,
    /// Per-agent transcript / metadata store for resume support.
    /// Late-bound so CLI bootstrap can construct the impl after
    /// `SessionRuntime::build` returns. `agent_handle_factory`
    /// installs it onto the SwarmAgentHandle when wiring agent-
    /// team support.
    agent_transcript_store: Arc<RwLock<Option<coco_tool_runtime::AgentTranscriptStoreRef>>>,
    /// Main-session transcript store. JSONL writes for the user /
    /// assistant / attachment / tool_result chain land here, keyed
    /// by the live session id (rotates on `/clear`). Cloned into
    /// every per-turn engine via [`Self::wire_engine`]. Backend-agnostic
    /// (`dyn SessionStore`) so it honors the configured `session.backend`.
    transcript_store: Arc<dyn coco_session::SessionStore>,
    /// When false, all transcript / usage / file-history persistence is
    /// suppressed for this run.
    persist_session: bool,
    /// Shared cumulative token/cost tracker for the current session id.
    session_usage_tracker: Arc<tokio::sync::Mutex<CostTracker>>,
    /// Serializes session usage updates and durable writes across all
    /// per-turn engines in this runtime.
    session_usage_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Cross-engine dedup set of message UUIDs already persisted to
    /// the JSONL transcript. Lives on the runtime (not the engine)
    /// so a fresh per-turn engine doesn't re-write history. Reset to
    /// empty by [`Self::clear_conversation`] when the session id
    /// regenerates.
    transcript_dedup: Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    /// Cross-engine tool-result replacement state. QueryEngine is
    /// rebuilt per user message, so this runtime-owned state preserves
    /// Level 2 `seen_ids` / replacement strings across turns.
    tool_result_replacement_state:
        coco_tool_runtime::tool_result_storage::ContentReplacementStateRef,
    /// MCP handle installed on every per-turn engine via `wire_engine`.
    /// Late-bound so CLI bootstrap can construct the
    /// `McpManagerAdapter` (or any other McpHandle impl) after
    /// `SessionRuntime::build` returns. Without this the engine's
    /// `mcp_handle` slot stays `None` and AgentTool's prompt-time
    /// MCP filter degrades to fail-closed (hides MCP-required
    /// agents).
    mcp_handle: Arc<RwLock<Option<coco_tool_runtime::McpHandleRef>>>,
    /// The live MCP connection manager, when one was built for this session.
    /// Distinct from [`Self::mcp_handle`] (the opaque tool-facing handle): this
    /// is the concrete manager so reload paths can re-register plugin-contributed
    /// MCP servers after a `/reload-plugins` / install / delisting. `None` on
    /// entry points that don't build a manager (e.g. the TUI today, headless);
    /// reload then no-ops. Set via [`Self::attach_mcp_manager`].
    mcp_manager: Arc<RwLock<Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>>>,
    /// Monotonic "the MCP server set changed" signal, bumped by
    /// [`Self::reload_plugin_mcp_servers`]. Consumers that own MCP
    /// reconnection re-run their effect when it moves.
    mcp_reconnect_key: Arc<std::sync::atomic::AtomicU64>,
    /// Late-bind slot for the LSP handle. CLI / SDK installs a
    /// `LspManagerAdapter` here when `Feature::Lsp` is on and at
    /// least one language server is configured; `wire_engine` reads
    /// the slot at engine-build time and installs it via
    /// `with_lsp_handle`.
    lsp_handle: Arc<RwLock<Option<coco_tool_runtime::LspHandleRef>>>,
    /// Where the agent loader looks for markdown agents. Cached so
    /// `/agents reload` and the file-watcher reload paths can rebuild
    /// the snapshot without re-resolving the paths from scratch.
    agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
    /// Built-in agent toggles applied to every reload. Set at
    /// `SessionRuntime::build` and treated as immutable thereafter
    /// (toggling the roster mid-session would require a full restart).
    builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Active per-session agent catalog snapshot. Installed on every
    /// per-turn engine via [`Self::wire_engine`] so `AgentTool::prompt`
    /// renders the dynamic agent listing. Wrapped in `RwLock<Arc<...>>`
    /// so a future reload (file watcher or `/agents reload`) can swap
    /// the inner `Arc` without invalidating in-flight per-turn engines
    /// (each engine snapshots the inner Arc at wire time).
    /// `Arc<AgentCatalogSnapshot>` is cheap to clone.
    agent_catalog: Arc<RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>>,
    /// SDK-supplied agent definitions to inject into every fresh
    /// `AgentDefinitionStore` build (initial load + every reload).
    /// Populated by the SDK `initialize` handler via
    /// [`Self::set_sdk_supplied_agents`] when the client pushes an
    /// `initialize.agents` JSON map. Stays alive across `session/start`
    /// → `session/archive` cycles so a single SDK connection's
    /// `initialize` payload survives multiple session boundaries.
    sdk_supplied_agents: Arc<RwLock<Vec<coco_types::AgentDefinition>>>,
    /// Session-scoped sandbox state. Built once at startup via
    /// [`build_sandbox_state`] and inherited by every per-turn engine
    /// (TUI), every SDK control message handler, and every fork
    /// dispatch — so all paths share the same `Arc<SandboxState>` and
    /// hot-reloads via `update_config` are seen everywhere.
    /// `None` when sandbox is disabled.
    sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
    /// Session-scoped attachment channel. Producers outside the per-turn
    /// engine (slash commands via the TUI, future swarm / skill / hook
    /// forwarders) emit typed silent `AttachmentMessage`s through
    /// [`Self::attachment_emitter`]; the engine drains the receiver at the
    /// head of every outer-loop turn via
    /// [`coco_query::QueryEngine::drain_attachment_inbox`]. Lives across
    /// engine rebuilds so cross-turn producers see a stable handle.
    session_attachment_tx: tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
    session_attachment_rx: Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<coco_messages::AttachmentMessage>>,
    >,
    /// Session-scoped mid-turn command queue. Producers (the
    /// TUI-while-busy bridge in `tui_runner`, future task / coordinator /
    /// hook forwarders) push `QueuedCommand`s here at any time, and the
    /// per-turn `QueryEngine` consumes them via [`Self::wire_engine`]
    /// which calls [`QueryEngine::with_command_queue`]. Internally
    /// `Arc`-backed so `Clone` is cheap — every engine instance shares
    /// the same backing storage with the runtime and any other holder.
    ///
    /// Teammate messages and task notifications also flow through this
    /// queue (with `QueueOrigin::Coordinator` /
    /// `QueueOrigin::TaskNotification`) — no separate `Inbox` type;
    /// coordinator messages surface as `queued_command` attachments.
    command_queue: CommandQueue,
    /// Concurrent-sessions PID registry guard. Wraps
    /// `<config_home>/sessions/{pid}.json`; the file is created at
    /// build time and removed when this field is dropped (i.e. when
    /// the last `Arc<SessionRuntime>` reference falls). `None` when
    /// the registration was skipped (subagent context per
    /// `COCO_AGENT_ID`) or the write failed (best-effort — we
    /// `tracing::warn` and proceed without a registry entry rather
    /// than block session startup).
    _pid_registry: Option<coco_session::SessionRegistry>,
}

/// File-history checkpointing gate. Interactive sessions default ON
/// (settings flag, unless the disable env is set); non-interactive
/// (SDK / headless) default OFF and require the SDK-enable env. The
/// disable env always wins.
fn file_checkpointing_enabled(settings_enabled: bool, is_non_interactive: bool) -> bool {
    if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoFileCheckpointingDisable) {
        return false;
    }
    if is_non_interactive {
        coco_config::env::is_env_truthy(coco_config::EnvKey::CocoFileCheckpointingSdkEnable)
    } else {
        settings_enabled
    }
}

impl SessionRuntime {
    /// Build the full session runtime. Constructs every subsystem
    /// `clearConversation` and the per-turn engine assembly need.
    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Arc<Self>> {
        let SessionRuntimeBuildOpts {
            cli,
            runtime_config,
            cwd,
            model_id,
            system_prompt,
            bypass_permissions_available,
            permission_mode,
            model_runtimes,
            tools,
            session_manager,
            fast_model_spec,
            permission_bridge,
            command_registry,
            skill_manager,
            agent_search_paths,
            builtin_agent_catalog,
            session_id_override,
            is_non_interactive,
        } = opts;

        let config_home = coco_config::global_config::config_home();
        let session_id = session_id_override.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Bare mode (`COCO_BARE_MODE` / `--bare`) skips session-start background
        // housekeeping. Recall / memory_runtime build / snapshot start stay on;
        // only the fire-and-forget sweeps + SM warm-load are suppressed.
        let bare_mode = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode);
        // Session-persistence kill switch: `--no-session-persistence`
        // (print-mode-only, validated at startup) suppresses ALL transcript
        // JSONL + usage-snapshot + file-history + subagent-transcript writes
        // for this run.
        let persist_session = !cli.no_session_persistence;

        // Concurrent-sessions PID registry. Skipped for subagent contexts
        // (non-null `COCO_AGENT_ID`), and best-effort: a write failure here
        // is logged and ignored so a constrained FS doesn't block session
        // startup.
        let pid_registry = {
            let agent_id_env = coco_config::env::var(coco_config::env::EnvKey::CocoAgentId).ok();
            match coco_session::SessionRegistry::register(
                &config_home,
                &session_id,
                &cwd,
                agent_id_env.as_deref(),
            ) {
                Ok(reg) => reg,
                Err(e) => {
                    warn!("concurrent-sessions register failed (non-fatal): {e}");
                    None
                }
            }
        };

        // FileReadState — @mention dedup + Read tool dedup.
        let file_read_state = Arc::new(RwLock::new(FileReadState::new()));
        let model_runtimes = match model_runtimes {
            Some(model_runtimes) => model_runtimes,
            None => Arc::new(coco_inference::ModelRuntimeRegistry::new(
                runtime_config.clone(),
                Some(crate::provider_login::shared_resolver()),
                Arc::new(coco_inference::HeaderVars {
                    session_id: session_id.clone(),
                    cwd: cwd.display().to_string(),
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                }),
            )?),
        };
        let side_query: coco_tool_runtime::SideQueryHandle = Arc::new(
            crate::side_query_impl::SideQueryAdapter::new(model_runtimes.clone(), model_id.clone()),
        );

        // Per-project filesystem layout — one `Arc<ProjectPaths>` shared
        // by the memory runtime, the transcript enumerator, and any
        // future subsystem that needs the same canonical slug. Built
        // once via `crate::paths::project_paths` (canonical-git-root
        // + slug).
        let project_paths = crate::paths::project_paths(&cwd);

        // ── Auto-memory runtime ──
        //
        // Built once per session, gated on `Feature::AutoMemory`. The
        // runtime owns the three services (extract / dream / session
        // memory) plus the recall ranker state. We hand it the
        // resolved `MemoryConfig` (already merged with env overrides),
        // the shared `Arc<ProjectPaths>` (so the SM file lives at
        // `<projectDir>/<sid>/session-memory/summary.md`), and an
        // `AgentHandle` so the forked extraction / dream subagents
        // spawn against the same swarm runtime that user-facing
        // `Agent` tool spawns use.
        //
        // The handle starts as `NoOpAgentHandle`; the SDK / TUI
        // runner calls `MemoryRuntime::install_agent` once the real
        // `SwarmAgentHandle` is built. Recall + system-prompt
        // rendering work without an agent handle.
        let active_shell_tool =
            crate::shell_tool_selection::active_shell_tool_from_runtime(&runtime_config)?;
        let memory_runtime = if runtime_config
            .features
            .enabled(coco_types::Feature::AutoMemory)
        {
            let agent: coco_tool_runtime::AgentHandleRef =
                Arc::new(coco_tool_runtime::NoOpAgentHandle);
            let mem_cfg = coco_memory::MemoryConfig::from(runtime_config.memory.clone());
            // Transcript root for dream's grep examples / searching-past-context
            // section. Lives at `<memory_base>/projects/<slug>/`.
            let transcript_root = project_paths.project_dir();
            // Wire the production tracing-backed telemetry emitter so
            // the ~17 MemoryEvent variants land in the global tracing
            // subscriber (installed by app/cli's tracing_init). Without
            // this every event silently no-ops via NoopEmitter.
            let memory_telemetry: Arc<dyn coco_memory::telemetry::MemoryTelemetryEmitter> =
                Arc::new(coco_memory::telemetry::TracingEmitter::new());
            // Whether auto-compact is active for this session — surfaced
            // by SessionMemoryInit so dashboards correlate SM activity
            // with the compact gate. `is_active()` honors both the user
            // toggle and the kill-switch envs (`COCO_COMPACT_DISABLE`,
            // `COCO_COMPACT_DISABLE_AUTO`), so a session bootstrapped
            // with compact off reports `auto_compact_enabled = false`.
            let auto_compact_enabled = runtime_config.compact.auto.is_active();
            let runtime = coco_memory::runtime::MemoryRuntimeBuilder::new(
                config_home.clone(),
                cwd.clone(),
                session_id.clone(),
                mem_cfg,
                agent,
            )
            .with_project_paths(project_paths.clone())
            .with_transcript_dir(transcript_root)
            .with_telemetry(memory_telemetry)
            .with_active_shell_tool(active_shell_tool)
            .with_auto_compact_enabled(auto_compact_enabled)
            .build();
            info!(
                personal_dir = %runtime.personal_dir().display(),
                "auto-memory runtime initialized"
            );
            let runtime_arc = Arc::new(runtime);
            let enumerator_project_paths = project_paths.clone();
            let enumerator_session_id = session_id.clone();
            let enumerator_memory_dir = runtime_arc.personal_dir().to_path_buf();
            // Wire the session enumerator backed by `TranscriptStore`
            // so per-turn `tick_dream` can list real prior sessions.
            // Lists sessions touched since `lastAt`, drops the current
            // session. Invoked only after the time + scan throttle
            // gates pass inside `DreamService` so cost is bounded.
            let enumerator: coco_memory::SessionEnumerator = Arc::new(move || {
                let store = coco_session::TranscriptStore::new(enumerator_project_paths.clone());
                let last_ms =
                    coco_memory::lock::last_consolidated_at(&enumerator_memory_dir).unwrap_or(0);
                match store.list_main_sessions() {
                    Ok(metas) => metas
                        .into_iter()
                        .filter(|m| m.session_id != enumerator_session_id)
                        .filter(|m| {
                            m.modified_at
                                .parse::<i64>()
                                .map(|t| t > last_ms)
                                .unwrap_or(false)
                        })
                        .map(|m| m.session_id)
                        .collect(),
                    Err(_) => Vec::new(),
                }
            });
            // install_* are one-shot in production (this is the only
            // call site per slot); swallow the duplicate-install Err so
            // a future double-install in tests doesn't blow up startup.
            let _ = runtime_arc.install_session_enumerator(enumerator);
            // NOTE: the tick_dream fire-and-forget is intentionally
            // deferred to AFTER `install_agent` below. Spawning it here
            // (before the real `SwarmAgentHandle` lands in the slot)
            // creates a multi-threaded race where the dream task can
            // read the NoOp handle, fail to spawn the consolidation
            // subagent, and emit a spurious `AutoDreamFailed` event.
            Some(runtime_arc)
        } else {
            // Feature gate off — emit MemdirDisabled so dashboards
            // can split sessions that never bootstrapped memory from
            // those that did. Emitted directly here instead of through
            // `MemoryRuntime` because no runtime exists at this point.
            tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_memdir_disabled",
                reason = "feature_gate",
                "auto-memory feature gate off"
            );
            None
        };

        // The production swarm handle is late-bound after TaskRuntime is
        // attached, because LocalAgent task registration is a required
        // constructor dependency. Until then engines carry the explicit
        // no-op handle and `attach_agent_handle` replaces it everywhere.
        let swarm_agent_handle: coco_tool_runtime::AgentHandleRef =
            Arc::new(coco_tool_runtime::NoOpAgentHandle);

        // Now that the real `AgentHandle` exists, install it on the
        // memory runtime so forked extraction / dream agents reach
        // the same swarm runtime instead of the no-op fallback.
        // Install the SideQuery adapter too so the recall ranker
        // dispatches a real `ModelRole::Memory` query instead of
        // falling back to the recency heuristic.
        if let Some(runtime) = &memory_runtime {
            runtime.install_agent(swarm_agent_handle.clone());
            let _ = runtime.install_side_query(side_query.clone());

            // Fire-and-forget auto-dream gate-check at session start.
            // Deferred here (vs. inside the runtime build above) so
            // the task observes the just-installed real
            // `SwarmAgentHandle` — multi-threaded schedulers were
            // racing the NoOp slot and emitting spurious
            // `AutoDreamFailed` events. Skipped in bare mode.
            if !bare_mode {
                let dream_clone = runtime.clone();
                tokio::spawn(async move {
                    let now_ms = coco_memory::service::dream::DreamService::now_ms();
                    let outcome = dream_clone.tick_dream(now_ms).await;
                    tracing::debug!(?outcome, "auto-dream gate check at session start");
                });
            }
        }

        // Session-memory handle threaded into the engine. Same `Arc`
        // the memory runtime holds when `Feature::AutoMemory` is on;
        // `None` otherwise (engine's SM-first compact path then falls
        // back to LLM summarization). Warm the on-disk cache here so
        // the first compact short-circuit doesn't have to read disk.
        let session_memory_service = memory_runtime.as_ref().map(|r| r.session_memory.clone());
        if let Some(svc) = &session_memory_service
            && !bare_mode
        {
            svc.load_from_disk().await;
        }

        // Reap abandoned per-session SM dirs (left behind by every
        // prior `/clear`, which regenerates the session id). 30-day
        // retention mirrors the worktree GC cadence; mtime-only, fire-
        // and-forget so a wedged filesystem can't block startup.
        if memory_runtime.is_some() && !bare_mode {
            let pdir = project_paths.project_dir();
            let sid = session_id.clone();
            tokio::spawn(async move {
                match coco_memory::service::session::cleanup_stale_session_memories(
                    &pdir,
                    &sid,
                    coco_memory::service::session::DEFAULT_SM_RETENTION,
                )
                .await
                {
                    Ok(n) if n > 0 => {
                        info!(
                            "reaped {n} orphan session-memory dirs under {}",
                            pdir.display()
                        );
                    }
                    Ok(_) => {}
                    Err(e) => warn!("session-memory cleanup failed: {e}"),
                }
            });
        }

        // FileHistoryState — backed by JSONL transcript when enabled.
        // Sink shares the session_id Arc with SessionRuntime so
        // /clear regen propagates immediately (no rebuild required).
        let (file_history, file_history_sink_session_id) = if file_checkpointing_enabled(
            runtime_config.settings.merged.file_checkpointing_enabled,
            is_non_interactive,
        ) {
            let project_paths = crate::paths::project_paths(&cwd);
            let sink_id = Arc::new(std::sync::RwLock::new(session_id.clone()));
            let sink: Arc<dyn FileHistorySnapshotSink> = Arc::new(TranscriptFileHistorySink::new(
                project_paths,
                sink_id.clone(),
            ));
            let mut state = FileHistoryState::new();
            state.set_sink(sink);
            (Some(Arc::new(RwLock::new(state))), Some(sink_id))
        } else {
            (None, None)
        };

        // Shared per-session ToolAppState (plan-mode reminder cadence,
        // exited_plan_mode flag, last_emitted_date latch, etc.).
        let app_state: Arc<RwLock<ToolAppState>> = Arc::new(RwLock::new(ToolAppState::default()));
        let auto_mode_state = Arc::new(coco_permissions::AutoModeState::new());
        auto_mode_state.set_active(permission_mode == coco_types::PermissionMode::Auto);
        auto_mode_state.set_cli_flag(permission_mode == coco_types::PermissionMode::Auto);
        let denial_tracker = Arc::new(tokio::sync::Mutex::new(
            coco_permissions::DenialTracker::new(),
        ));

        // Hook registry — settings hooks first, then plugin hooks
        // layered on top via the bridge so plugin manifests can
        // declare their own SessionStart / PreToolUse / PostCompact /
        // etc. hooks. The loaded plugin set is only needed
        // for the duration of registration — `register_plugin_hooks_v2`
        // copies hook definitions into the registry, so dropping the
        // `Vec<LoadedPluginV2>` afterward is safe. If a future SDK
        // `plugin/reload` path needs the live set it can be reintroduced
        // as a stored field; until then we don't pay for the storage.
        let hook_registry = {
            let registry = HookRegistry::new();
            populate_hook_registry(&registry, &runtime_config, &config_home, &cwd);
            Arc::new(registry)
        };

        let mailbox: MailboxHandleRef = Arc::new(coco_coordinator::mailbox::SwarmMailboxHandle);

        // Augment the caller-provided system prompt with the
        // auto-memory section (type taxonomy, how-to-save, MEMORY.md
        // body). The memory crate hands us a pre-rendered block so
        // this crate stays free of memory-prompt assembly logic.
        // Cache-broken upstream by `coco_context::build_system_prompt`
        // when the section is non-empty; we splice the same string in
        // here so the engine's prompt cache prefix sees it.
        let system_prompt_with_memory = if let Some(runtime) = &memory_runtime
            && let Some(section) = runtime.render_system_prompt_section().await
            && !section.is_empty()
        {
            format!("{system_prompt}\n\n{section}")
        } else {
            system_prompt
        };

        // Bootstrap the sandbox runtime state from settings + permission
        // rules. When sandbox isn't enabled or required dependencies are
        // missing the bootstrap returns `None` (degrade to unsandboxed)
        // — unless `sandbox.fail_if_unavailable` is set, in which case
        // it returns an error and we exit before the REPL starts.
        let sandbox_state = build_sandbox_state(&runtime_config, &cwd).await?;

        // Session-scoped attachment channel. The engine drains the rx at
        // the head of each turn (drain_attachment_inbox), while producers
        // outside the per-turn engine (TUI slash commands, future swarm /
        // skill forwarders) push via the cloned tx — see
        // `Self::attachment_emitter`. One channel per session, threaded
        // into each per-turn engine via `wire_engine`.
        let (session_attachment_tx, session_attachment_rx) =
            tokio::sync::mpsc::unbounded_channel::<coco_messages::AttachmentMessage>();
        let session_attachment_rx = Arc::new(tokio::sync::Mutex::new(session_attachment_rx));

        // Bootstrap the per-source permission rule maps. Parses every
        // settings.json layer (user/project/local/flag/policy) into typed
        // `PermissionRulesBySource` keyed by `PermissionRuleSource`.
        // Default-empty maps before this wiring meant `permissions.allow`
        // / `deny` / `ask` from settings.json were loaded but never
        // consulted at evaluation time.
        let (allow_rules, deny_rules, ask_rules) =
            crate::permission_rule_loader::typed_permission_rules(&runtime_config.settings);
        let permission_rule_source_roots =
            crate::permission_rule_loader::permission_rule_source_roots(
                &runtime_config.settings,
                &cwd,
            );

        // ── Session-scoped CWD state ──
        //
        // Frozen anchor + live tracker. The live tracker is
        // threaded through every `ToolUseContext` so BashTool can
        // read it as the spawn cwd and write back `new_cwd` after
        // each command — `cd /tmp` in turn N survives into turn N+1.
        let session_original_cwd = cwd.clone();
        let session_current_cwd = Arc::new(RwLock::new(cwd.clone()));

        // ── Session-scoped shell provider ──
        //
        // Build once at session start so the provider keeps the same
        // resolved shell binary and session-scoped `/env` store across all
        // shell-tool invocations. Bash additionally keeps snapshot watch +
        // session-env reader state.
        let shell_provider: Option<Arc<dyn coco_shell::ShellProvider>> = match active_shell_tool {
            coco_types::ActiveShellTool::Bash => {
                let mut shell = coco_shell::shell_from_config(&runtime_config.shell);
                let snap_cfg = coco_shell::SnapshotConfig::new(&config_home);
                if !runtime_config.shell.disable_snapshot {
                    coco_shell::ShellSnapshot::start_snapshotting(
                        snap_cfg.clone(),
                        &session_id,
                        &mut shell,
                    );
                    // Sweep prior-run residue in the background — mtime-only,
                    // no await needed on the hot path. Skipped in bare mode.
                    if !bare_mode {
                        let dir = snap_cfg.snapshot_dir.clone();
                        let sid = session_id.clone();
                        let retention = snap_cfg.retention;
                        tokio::spawn(async move {
                            match coco_shell::cleanup_stale_snapshots(&dir, &sid, retention).await {
                                Ok(n) if n > 0 => {
                                    info!(
                                        "reaped {n} stale shell snapshots from {}",
                                        dir.display()
                                    );
                                }
                                Ok(_) => {}
                                Err(e) => warn!("shell snapshot cleanup failed: {e}"),
                            }
                        });
                    }
                }
                let session_env_reader = Some(Arc::new(coco_shell::SessionEnvReader::new(
                    &config_home,
                    &session_id,
                )));
                // `COCO_SHELL_PREFIX` is consumed here (BashProvider wraps the
                // assembled command). The same env var is also consumed by
                // `coco-hooks` for hook-command execution — they share the
                // value but apply it independently.
                let shell_prefix = std::env::var("COCO_SHELL_PREFIX").ok();
                let session_env_vars = coco_shell::SessionEnvVars::new();
                Some(Arc::new(coco_shell::BashProvider::new(
                    shell,
                    session_env_reader,
                    session_env_vars,
                    shell_prefix,
                )) as Arc<dyn coco_shell::ShellProvider>)
            }
            coco_types::ActiveShellTool::PowerShell => {
                let shell = crate::shell_tool_selection::require_shell(
                    coco_shell::ShellType::PowerShell,
                    "PowerShell tool selected, but neither `pwsh` nor `powershell` was found",
                )?;
                let session_env_vars = coco_shell::SessionEnvVars::new();
                Some(
                    Arc::new(coco_shell::PowerShellProvider::new(shell, session_env_vars))
                        as Arc<dyn coco_shell::ShellProvider>,
                )
            }
            coco_types::ActiveShellTool::Disabled => None,
        };

        // Seed --add-dir + settings additionalDirectories into the session
        // working-dir allowlist. Computed before the engine config since the
        // rules + dirs now live ONLY on the live `ToolAppState.permissions`
        // base (the config no longer carries them).
        let session_additional_dirs = crate::permission_rule_loader::seed_session_additional_dirs(
            cli,
            &runtime_config.settings,
            &cwd,
        );

        // Build the engine config — owns most settings drawn from
        // RuntimeConfig + CLI overrides.
        let engine_config = QueryEngineConfig {
            model_id,
            permission_mode,
            bypass_permissions_available,
            permission_rule_source_roots: permission_rule_source_roots.clone(),
            context_window: 200_000,
            max_output_tokens: 16_384,
            // Interactive: unbounded unless the user set `loop.max_turns`;
            // `--max-turns` is `--print`-only.
            max_turns: runtime_config.loop_config.max_turns,
            total_token_budget: cli
                .max_tokens
                .or_else(|| runtime_config.loop_config.total_token_budget.map(i64::from)),
            prompt_cache: model_runtimes
                .snapshot_for_role(ModelRole::Main)
                .ok()
                .is_some_and(|snapshot| snapshot.supports_prompt_cache)
                .then(|| coco_types::PromptCacheConfig {
                    mode: coco_types::PromptCacheMode::Auto,
                    ttl: coco_types::CacheTtl::OneHour,
                    scope: None,
                    requested_betas: Default::default(),
                    skip_cache_write: false,
                }),
            system_prompt: Some(system_prompt_with_memory),
            streaming_tool_execution: runtime_config.loop_config.enable_streaming_tools,
            session_id: session_id.clone(),
            project_dir: runtime_config
                .paths
                .project_dir
                .clone()
                .or_else(|| Some(cwd.clone())),
            plan_mode_settings: runtime_config.settings.merged.plan_mode.clone(),
            system_reminder: runtime_config.settings.merged.system_reminder.clone(),
            tool_config: runtime_config.tool.clone(),
            sandbox_config: runtime_config.sandbox.clone(),
            sandbox_state: sandbox_state.clone(),
            memory_config: runtime_config.memory.clone(),
            shell_config: runtime_config.shell.clone(),
            active_shell_tool,
            shell_provider,
            original_cwd: Some(session_original_cwd.clone()),
            session_cwd: Some(session_current_cwd.clone()),
            web_fetch_config: runtime_config.web_fetch.clone(),
            web_search_config: runtime_config.web_search.clone(),
            lsp_config: runtime_config.lsp.clone(),
            compact: runtime_config.compact.clone(),
            // Per-session raw-wire dumper. Built only when the operator
            // opts in via settings.json / env; `None` is zero overhead.
            wire_dump: {
                let d = &runtime_config.diagnostics;
                (!d.wire_dump.is_off()).then(|| {
                    coco_query::WireDumpConfig::new(
                        project_paths.session_dir(&session_id),
                        d.wire_dump,
                        d.wire_dump_max_body_bytes.max(0) as usize,
                        d.wire_dump_redact,
                    )
                })
            },
            features: Arc::new(runtime_config.features.clone()),
            skill_overrides: Arc::new(runtime_config.skill_overrides.clone()),
            tool_overrides: runtime_config.tool_overrides.clone(),
            include_hook_events: cli.include_hook_events,
            ..Default::default()
        };

        // Seed the live permission base (S1). `ToolAppState` is the single
        // live source of truth — the ONLY permission base the factory reads
        // each batch. app_state is uncontended here (freshly created above,
        // not yet shared with any engine). The rules + dirs flow from the
        // locals loaded above, NOT from the config (which no longer carries
        // them).
        app_state.write().await.permissions = live_permissions(
            permission_mode,
            allow_rules,
            deny_rules,
            ask_rules,
            session_additional_dirs,
            permission_rule_source_roots,
        );

        let auto_title_enabled = runtime_config.settings.merged.session.auto_title;

        // LLM-driven hook handler. It resolves HookAgent and per-hook
        // role overrides through the shared ModelRuntimeRegistry.
        let hook_llm_handle =
            Arc::new(coco_query::hook_llm::QueryHookLlm::for_session(model_runtimes.clone()).await);
        // Main-session transcript store, selected by `session.backend`.
        // Constructed once so the per-turn message append in
        // `engine_finalize_turn` and the agent-transcript persistence path
        // share one instance. `Disk` keys at `<memory_base>/projects/<slug>/`
        // for this cwd (today's behavior); `Memory` pulls the shared
        // in-memory store from the session manager's catalog so the engine
        // and `SessionManager` observe the same ephemeral state.
        let transcript_store: Arc<dyn coco_session::SessionStore> =
            match runtime_config.settings.merged.session.backend {
                coco_config::SessionBackend::Disk => {
                    Arc::new(TranscriptStore::new(project_paths.clone()))
                }
                coco_config::SessionBackend::Memory => session_manager.store_for(&cwd),
            };
        let session_usage_tracker = Arc::new(tokio::sync::Mutex::new(
            transcript_store
                .load_usage_snapshot(&session_id)
                .ok()
                .flatten()
                .map(CostTracker::from_snapshot)
                .unwrap_or_default(),
        ));
        let session_usage_write_lock = Arc::new(tokio::sync::Mutex::new(()));
        let transcript_dedup = Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::<
            uuid::Uuid,
        >::new()));
        let tool_result_replacement_state = Arc::new(tokio::sync::RwLock::new(
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX),
        ));

        // ── Agent definition catalog ──
        //
        // Build the per-session [`AgentDefinitionStore`] once at startup
        // so AgentTool's dynamic prompt sees the same set the SDK
        // `initialize.agents` listing returns. The snapshot inspector
        // wires `pending_snapshot_update` per definition so `/agents show`
        // can flag drift without each consumer re-running the
        // `check_agent_memory_snapshot` IO.
        //
        // Errors / missing dirs are non-fatal: the store keeps the
        // built-in roster and the per-turn engine reads the resulting
        // (mostly built-in) catalog. Snapshot is reload-able via
        // [`Self::reload_agent_catalog`]; this initial build lives on
        // the blocking pool because the markdown loader is sync IO.
        let auto_memory_enabled = runtime_config
            .features
            .enabled(coco_types::Feature::AutoMemory);
        // Initial agent-catalog load. SDK-supplied agents from
        // `initialize.agents` get injected here on session start —
        // they live on `SessionRuntime.sdk_supplied_agents` until
        // [`Self::set_sdk_supplied_agents`] is called by the SDK
        // `initialize` handler, which fires BEFORE `session/start`.
        // For pure TUI / SDK-less paths the Vec is empty.
        let initial_agent_snapshot = {
            let catalog = builtin_agent_catalog;
            let paths = agent_search_paths.clone();
            let cwd_for_inspector = cwd.clone();
            let home_for_inspector =
                dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            // SDK-supplied agents are an empty Vec at this point — the
            // SessionRuntime is being constructed for the FIRST time;
            // `set_sdk_supplied_agents` hasn't been called yet. The
            // reload path picks them up once they're stashed.
            tokio::task::spawn_blocking(move || {
                let mut store = coco_subagent::AgentDefinitionStore::new(catalog, paths);
                store.set_snapshot_inspector(Some(
                    coco_memory::agent_memory_snapshot::build_pending_inspector(
                        cwd_for_inspector,
                        home_for_inspector,
                    ),
                ));
                // Auto-adds `Read`/`Edit`/`Write` to non-wildcard
                // agent tool-lists when AutoMemory is on AND the
                // agent declares a `memory` scope. Forward the live
                // feature gate so the catalog the engine sees
                // includes the injected tools.
                store.set_auto_memory_enabled(auto_memory_enabled);
                store.load();
                store.snapshot()
            })
            .await
            .unwrap_or_else(|_| {
                Arc::new(coco_subagent::AgentCatalogSnapshot::new(
                    std::collections::BTreeMap::new(),
                    Vec::new(),
                ))
            })
        };
        let agent_catalog = Arc::new(RwLock::new(initial_agent_snapshot));

        let orchestration_session_id = Arc::new(std::sync::RwLock::new(session_id.clone()));
        let orchestration_engine_config = Arc::new(std::sync::RwLock::new(engine_config.clone()));

        Ok(Arc::new(Self {
            original_cwd: session_original_cwd,
            current_cwd: session_current_cwd,
            tools,
            command_registry,
            skill_manager,
            config_home,
            runtime_config,
            session_manager,
            fast_model_spec,
            // Disk-backed so `durable` cron tasks survive restarts;
            // session tasks stay in-memory. The cron tick driver shares this store.
            schedule_store: Arc::new(coco_tool_runtime::DiskBackedScheduleStore::new(
                cwd.join(".coco").join("scheduled_tasks.json"),
            )),
            model_runtimes,
            side_query,
            auto_title_enabled,
            mailbox,
            permission_bridge,
            cancel: CancellationToken::new(),
            session_id: Arc::new(RwLock::new(session_id.clone())),
            engine_config: Arc::new(RwLock::new(engine_config)),
            orchestration_session_id,
            orchestration_engine_config,
            role_overrides: Arc::new(RwLock::new(HashMap::new())),
            sandbox_state,
            file_read_state,
            file_history,
            app_state,
            pending_message_store: Arc::new(coco_tool_runtime::InMemoryPendingMessageStore::new()),
            auto_mode_state,
            denial_tracker,
            session_memory_service,
            memory_runtime,
            swarm_agent_handle,
            hook_registry,
            hook_llm_handle,
            sync_hook_buffer: coco_hooks::SyncHookEventBuffer::new(),
            async_hook_registry: Arc::new(coco_hooks::async_registry::AsyncHookRegistry::new()),
            file_changed_watcher: Arc::new(RwLock::new(None)),
            history: Arc::new(Mutex::new({
                let mut h = MessageHistory::new();
                // Stamp F9 envelope onto history so every history_sync
                // emit carries session_id automatically. agent_id is
                // None for the main session; subagents stamp their own
                // via a separate construction site in `engine_session`.
                h.set_envelope(session_id, None);
                h
            })),
            file_history_sink_session_id,
            // Late-bound — `attach_agent_handle()` installs after the
            // Arc<SessionRuntime> is constructed so the
            // QueryEngineAdapter factory can close over Arc<Self>.
            agent_handle: Arc::new(RwLock::new(None)),
            skill_handle: Arc::new(RwLock::new(None)),
            skill_bash_cell: Arc::new(std::sync::RwLock::new(None)),
            fork_dispatcher: Arc::new(RwLock::new(None)),
            last_engine_cache_handle: Arc::new(RwLock::new(None)),
            live_permission_rules: Arc::new(RwLock::new(Vec::new())),
            current_suggestion_abort: Arc::new(tokio::sync::Mutex::new(None)),
            task_runtime: Arc::new(RwLock::new(None)),
            task_list: Arc::new(RwLock::new(None)),
            team_task_list_router: Arc::new(RwLock::new(None)),
            agent_transcript_store: Arc::new(RwLock::new(None)),
            mcp_handle: Arc::new(RwLock::new(None)),
            mcp_manager: Arc::new(RwLock::new(None)),
            mcp_reconnect_key: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            lsp_handle: Arc::new(RwLock::new(None)),
            agent_search_paths,
            builtin_agent_catalog,
            agent_catalog,
            sdk_supplied_agents: Arc::new(RwLock::new(Vec::new())),
            session_attachment_tx,
            session_attachment_rx,
            transcript_store,
            persist_session,
            session_usage_tracker,
            session_usage_write_lock,
            transcript_dedup,
            tool_result_replacement_state,
            command_queue: CommandQueue::new(),
            _pid_registry: pid_registry,
        }))
    }

    /// Re-scan the configured agent search paths and replace the
    /// in-memory catalog snapshot. Subsequent per-turn engines built
    /// via [`Self::wire_engine`] pick up the new snapshot; engines
    /// already in flight keep the snapshot they captured at wire time.
    ///
    /// Borrow a stable pointer to the active agent catalog snapshot.
    ///
    /// The returned `Arc` is a pointer clone — readers continue to
    /// observe the snapshot they cloned even if [`reload_agent_catalog`]
    /// swaps in a new one mid-read. Callers (TUI bootstrap, `/agents`
    /// renderer) should re-call this for each fresh read rather than
    /// caching the result long-term.
    ///
    /// Uses the `RwLock<Arc<...>>` pattern to make `reload_agent_catalog`
    /// an atomic swap with no observer drift.
    pub async fn agent_catalog_snapshot(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.agent_catalog.read().await.clone()
    }

    /// Triggered by `/agents reload`, `/reload-plugins`, and the
    /// future agent-dir file watcher.
    pub async fn reload_agent_catalog(&self) {
        let catalog = self.builtin_agent_catalog;
        let paths = self.agent_search_paths.clone();
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let auto_memory_enabled = self
            .runtime_config
            .features
            .enabled(coco_types::Feature::AutoMemory);
        // Clone the SDK-supplied agents Vec into the worker. After
        // `set_sdk_supplied_agents` populates the slot, every reload
        // picks up the same set as additional FlagSettings entries.
        // The Vec lives across `session/start` → `session/archive`
        // cycles so a single SDK connection's `initialize` payload
        // survives the whole connection lifetime.
        let sdk_agents = self.sdk_supplied_agents.read().await.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let mut store = coco_subagent::AgentDefinitionStore::new(catalog, paths);
            store.set_snapshot_inspector(Some(
                coco_memory::agent_memory_snapshot::build_pending_inspector(cwd, home),
            ));
            store.set_auto_memory_enabled(auto_memory_enabled);
            store.load();
            // Inject SDK-pushed agents AFTER on-disk load so they
            // participate in source-precedence resolution (FlagSettings
            // > ProjectSettings > UserSettings > Plugin > BuiltIn).
            // The store re-applies precedence on each `insert_definition`,
            // so an SDK agent with the same `agent_type` as a built-in
            // overrides the built-in.
            for def in sdk_agents {
                store.insert_definition(def);
            }
            store.snapshot()
        })
        .await
        .ok();
        if let Some(snapshot) = snapshot {
            *self.agent_catalog.write().await = snapshot;
        }
    }

    /// Replace the set of SDK-supplied agent definitions used by every
    /// future catalog (re)load. Called by the SDK `initialize` handler
    /// when the client pushes `initialize.agents`.
    ///
    /// Triggers an immediate `reload_agent_catalog()` so the new agents
    /// land in the active snapshot before the next `turn/start` (the
    /// engine snapshots the catalog when wiring per-turn).
    pub async fn set_sdk_supplied_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        let count = agents.len();
        {
            let mut slot = self.sdk_supplied_agents.write().await;
            *slot = agents;
        }
        self.reload_agent_catalog().await;
        tracing::info!(
            target: "coco::session_runtime",
            count,
            "SDK-supplied agents applied; agent catalog reloaded"
        );
    }

    /// Cheap pointer-clone of the active catalog snapshot. The returned
    /// `Arc` is stable for the lifetime of the caller — a concurrent
    /// reload swaps the inner `Arc` but doesn't invalidate handles
    /// previously taken.
    pub async fn current_agent_catalog(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.agent_catalog.read().await.clone()
    }

    /// Session-scoped attachment emitter for producers outside the
    /// per-turn engine (TUI slash commands, swarm forwarders, …).
    ///
    /// Each `emit()` enqueues a typed `AttachmentMessage` (typically
    /// silent-* variants) onto the session channel. The engine drains
    /// at the head of each outer-loop turn via
    /// [`coco_query::QueryEngine::drain_attachment_inbox`] so producers
    /// don't need access to `MessageHistory`.
    pub fn attachment_emitter(&self) -> coco_messages::AttachmentEmitter {
        coco_messages::AttachmentEmitter::new(self.session_attachment_tx.clone())
    }

    /// The tool registry shared by every engine instance.
    ///
    /// Callers that need to register or deregister tools at runtime (e.g.
    /// the SDK MCP lifecycle handlers) use this to mutate the registry
    /// via its interior-mutability API.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    /// Session-scoped sandbox state. Cheap-clone via `Arc`; consumers
    /// (fork dispatch, SDK handler) inherit the same instance so
    /// `SandboxState::update_config` hot-reloads propagate everywhere.
    pub fn sandbox_state(&self) -> Option<Arc<coco_sandbox::SandboxState>> {
        self.sandbox_state.clone()
    }

    /// Install the MCP handle that every per-turn engine receives via
    /// `wire_engine`. Call this after `SessionRuntime::build` returns
    /// so the bootstrap can wrap a real `McpConnectionManager`.
    pub async fn attach_mcp_handle(&self, handle: coco_tool_runtime::McpHandleRef) {
        let mut slot = self.mcp_handle.write().await;
        *slot = Some(handle);
    }

    /// Snapshot the installed MCP handle. `None` ⇒ no handle wired.
    pub async fn current_mcp_handle(&self) -> Option<coco_tool_runtime::McpHandleRef> {
        self.mcp_handle.read().await.clone()
    }

    /// Install the live `McpConnectionManager` so reload paths can re-register
    /// plugin-contributed MCP servers. Call this after `SessionRuntime::build`
    /// on entry points that own a manager (the SDK path today).
    pub async fn attach_mcp_manager(
        &self,
        manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    ) {
        let mut slot = self.mcp_manager.write().await;
        *slot = Some(manager);
    }

    /// Current MCP reconnect key. Increments each time
    /// [`Self::reload_plugin_mcp_servers`] changes the registered set.
    pub fn mcp_reconnect_key(&self) -> u64 {
        self.mcp_reconnect_key
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Re-register plugin-contributed MCP servers with the attached
    /// `McpConnectionManager` and bump the reconnect key. Called from
    /// `/reload-plugins` (and after install / delisting) so a plugin
    /// enable/disable flows into the MCP layer.
    ///
    /// Reconciles the live MCP set against the currently-enabled plugins:
    /// servers from now-disabled/uninstalled plugins (the `plugin:` namespace)
    /// are unregistered + their tools deregistered; newly-enabled servers are
    /// registered, connected, and their tools registered. Bumps the reconnect
    /// key. No-op (returns 0) when no manager is attached. Returns the count of
    /// currently-enabled plugin MCP servers.
    pub async fn reload_plugin_mcp_servers(&self, cwd: &std::path::Path) -> usize {
        let Some(manager) = self.mcp_manager.read().await.clone() else {
            return 0;
        };
        let plugins = coco_plugins::load_enabled_plugins(&self.config_home, cwd);
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
        let scoped = coco_plugins::mcp_bridge::extract_mcp_servers_from_plugins(&plugin_refs);
        let count = scoped.len();
        let new_names: std::collections::HashSet<String> =
            scoped.iter().map(|s| s.name.clone()).collect();

        // Reconcile: drop plugin servers (`plugin:` namespace) no longer present,
        // then (re)register the current set. Plugin servers are keyed
        // `plugin:<plugin>:<server>` by `mcp_bridge`, so the prefix isolates them
        // from config-file servers, which this reload must never touch.
        let stale: Vec<String> = {
            let mut mgr = manager.lock().await;
            let stale: Vec<String> = mgr
                .registered_server_names()
                .into_iter()
                .filter(|n| n.starts_with("plugin:") && !new_names.contains(n))
                .collect();
            for name in &stale {
                mgr.unregister_server(name).await;
            }
            mgr.register_all(scoped);
            stale
        };
        for name in &stale {
            coco_tools::deregister_mcp_server(self.tools(), name);
        }
        if count > 0 {
            // Connect the newly-registered servers + register their tools into the
            // live registry (idempotent — already-connected servers are skipped).
            crate::session_bootstrap::connect_and_register_mcp(
                manager.clone(),
                self.tools().clone(),
            )
            .await;
        }
        self.mcp_reconnect_key
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        count
    }

    /// Re-read the on-disk LSP config, re-merge plugin-contributed LSP servers,
    /// and re-prewarm via the attached LSP handle. No-op when no handle is
    /// attached. Called from `/reload-plugins`.
    pub async fn reload_lsp_servers(&self, cwd: &std::path::Path) {
        if let Some(handle) = self.current_lsp_handle().await {
            handle.reload(cwd).await;
        }
    }

    /// Install or replace the late-bound LSP handle. Same semantics as
    /// [`Self::attach_mcp_handle`] — slot is read at every
    /// `wire_engine` call so per-turn engines pick up swaps.
    pub async fn attach_lsp_handle(&self, handle: coco_tool_runtime::LspHandleRef) {
        let mut slot = self.lsp_handle.write().await;
        *slot = Some(handle);
    }

    /// Snapshot the installed LSP handle. `None` ⇒ no handle wired —
    /// `wire_engine` falls back to `NoOpLspHandle` and `LspTool` hides
    /// from the model.
    pub async fn current_lsp_handle(&self) -> Option<coco_tool_runtime::LspHandleRef> {
        self.lsp_handle.read().await.clone()
    }

    /// Snapshot the current session id (cheap clone of the inner String).
    pub async fn current_session_id(&self) -> String {
        self.session_id.read().await.clone()
    }

    /// Whether this run persists session artifacts (transcript / usage /
    /// file-history / subagent transcripts). False under
    /// `--no-session-persistence`.
    pub fn persist_session(&self) -> bool {
        self.persist_session
    }

    pub async fn flush_session_usage_snapshot(&self) {
        if !self.persist_session {
            return;
        }
        let session_id = self.current_session_id().await;
        let _write_guard = self.session_usage_write_lock.lock().await;
        let snapshot = self
            .session_usage_tracker
            .lock()
            .await
            .snapshot(&session_id);
        let store = Arc::clone(&self.transcript_store);
        let session_id_for_write = session_id.clone();
        match tokio::task::spawn_blocking(move || {
            store.write_usage_snapshot(&session_id_for_write, &snapshot)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(error = %e, session_id, "failed to flush session usage snapshot");
            }
            Err(e) => {
                warn!(error = %e, session_id, "session usage flush task failed");
            }
        }
    }

    pub async fn session_usage_snapshot(&self) -> coco_types::SessionUsageSnapshot {
        let session_id = self.current_session_id().await;
        self.session_usage_tracker.lock().await.snapshot(session_id)
    }

    async fn load_usage_tracker_for_session(&self, session_id: &str) -> CostTracker {
        let store = Arc::clone(&self.transcript_store);
        let session_id_for_load = session_id.to_string();
        let session_id_for_log = session_id_for_load.clone();
        match tokio::task::spawn_blocking(move || store.load_usage_snapshot(&session_id_for_load))
            .await
        {
            Ok(Ok(Some(snapshot))) => CostTracker::from_snapshot(snapshot),
            Ok(Ok(None)) => CostTracker::new(),
            Ok(Err(e)) => {
                warn!(error = %e, session_id = %session_id_for_log, "failed to load session usage snapshot");
                CostTracker::new()
            }
            Err(e) => {
                warn!(error = %e, session_id = %session_id_for_log, "session usage load task failed");
                CostTracker::new()
            }
        }
    }

    pub fn side_query(&self) -> coco_tool_runtime::SideQueryHandle {
        self.side_query.clone()
    }

    /// Generate the on-demand LLM risk explanation for a permission prompt.
    /// Runs the explainer via the session `SideQuery` handle, gated on
    /// `permission_explainer_enabled` (default-on) and bounded by a timeout.
    /// Graceful-degrades to `None` when the setting is off, the side query
    /// errors, or the timeout elapses. The single home for the explainer call
    /// — `TuiPermissionBridge::explain_risk` and the tui_runner Ctrl+E path
    /// both delegate here.
    pub async fn explain_permission_risk(
        &self,
        params: coco_permissions::ExplainerParams<'_>,
    ) -> Option<coco_types::PermissionExplanation> {
        if !self
            .runtime_config
            .settings
            .merged
            .permissions
            .explainer_enabled()
        {
            return None;
        }
        let handle = self.side_query();
        let fut =
            coco_permissions::generate_permission_explanation(params, move |req| async move {
                handle.query(req).await.map_err(|e| e.to_string())
            });
        // Bound the timeout so a slow/hung side query can't pin the explainer panel.
        tokio::time::timeout(std::time::Duration::from_secs(8), fut)
            .await
            .unwrap_or_default()
    }

    pub fn model_runtimes(&self) -> Arc<coco_inference::ModelRuntimeRegistry> {
        self.model_runtimes.clone()
    }

    /// Best-effort PID-registry live patch — push the human-readable
    /// session name into the `<config_home>/sessions/<pid>.json` file
    /// that `coco ps` reads. Silently no-ops when the session isn't
    /// registered (subagent context, FS-constrained startup, etc.)
    /// because `_pid_registry` is `Option`. Without this patch the live
    /// registry shows the stale startup name forever.
    pub fn update_session_registry_name(&self, name: &str) {
        if let Some(reg) = self._pid_registry.as_ref() {
            reg.update_session_name(name);
        }
    }

    /// Seed the transcript dedup set with uuids that are already
    /// persisted on disk. Called on resume / fork so the first
    /// post-load turn doesn't re-write the loaded messages.
    ///
    /// MUST clear the dedup set first. In-TUI `/resume` reuses the
    /// runtime, so without the clear the prior session's UUIDs leak
    /// into the new session and any colliding new write gets silently
    /// suppressed.
    pub async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        let mut g = self.transcript_dedup.lock().await;
        g.clear();
        g.extend(uuids);
    }

    /// Reconstruct Level 2 tool-result replacement state from the
    /// restored messages plus transcript content-replacement records.
    /// Called on resume/fork before the first resumed turn.
    ///
    /// `agent_id` MUST be the runtime's current agent_id (None for
    /// main-thread sessions, Some for subagents). The transcript
    /// content-replacement records are stamped with `agent_id` at
    /// write time (`engine_prompt.rs:200-216`); reading with
    /// `agent_id: None` for a subagent resume would silently drop
    /// every Level-2 replacement and force the model to re-read the
    /// full tool result, breaking prompt-cache stability.
    pub async fn seed_tool_result_replacement_state(
        &self,
        messages: &[Message],
        session_id: &str,
        agent_id: Option<&str>,
    ) {
        let records = self
            .transcript_store
            .load_content_replacements_for_chain(session_id, agent_id)
            .unwrap_or_default();
        let mut next =
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX);
        for msg in messages {
            if let Message::ToolResult(tr) = msg {
                next.seen_ids.insert(tr.tool_use_id.clone());
            }
        }
        for record in records {
            next.seen_ids.insert(record.tool_use_id().to_string());
            next.replacements.insert(
                record.tool_use_id().to_string(),
                record.replacement().to_string(),
            );
        }
        *self.tool_result_replacement_state.write().await = next;
    }

    /// Borrow the optional `MemoryRuntime`. `None` when
    /// `Feature::AutoMemory` is off. Callers (e.g. the slash dispatcher's
    /// `/dream` and `/summary` triggers) clone the inner `Arc`.
    pub fn memory_runtime(&self) -> Option<&Arc<coco_memory::MemoryRuntime>> {
        self.memory_runtime.as_ref()
    }

    /// The production swarm `AgentHandle` once `attach_agent_handle` has
    /// late-bound it (the eager `swarm_agent_handle` is a no-op until then).
    /// `None` before attach / in non-swarm sessions. Used by the leader
    /// inbox poller to resolve the active team via `active_team_name`.
    pub async fn current_agent_handle(&self) -> Option<AgentHandleRef> {
        self.agent_handle.read().await.clone()
    }

    /// Resolve this session's team snapshot for the team-coordination
    /// reminder. A teammate session uses its own identity (env/context);
    /// a leader session uses the roster active team. `None` when not in a
    /// team. Computed per-turn in `wire_engine` so the app-query
    /// `SwarmAdapter` stays coordinator-free.
    async fn resolve_team_snapshot(&self) -> Option<coco_system_reminder::TeamContextSnapshot> {
        let (agent_id, agent_name, team_name) =
            if let Some(id) = coco_coordinator::identity::resolve_teammate_identity() {
                (id.agent_id, id.agent_name, id.team_name)
            } else {
                let handle = self.current_agent_handle().await?;
                let team = handle.active_team_name().await?;
                (format!("team-lead@{team}"), "team-lead".to_string(), team)
            };
        let task_list_id = coco_coordinator::types::sanitize_name(&team_name);
        Some(coco_system_reminder::TeamContextSnapshot {
            team_config_path: coco_coordinator::team_file::get_team_file_path(&team_name)
                .display()
                .to_string(),
            task_list_path: self
                .config_home
                .join("tasks")
                .join(coco_tasks::task_list::sanitize_path_component(
                    &task_list_id,
                ))
                .display()
                .to_string(),
            agent_id,
            agent_name,
            team_name,
        })
    }

    /// Resolve a role to `(spec, effort)`, layering overrides above
    /// `runtime_config.model_roles`. Returns `None` only when the role
    /// is not configured anywhere (model picker / engine consumers
    /// already guard on this via the Main fallback chain).
    ///
    /// Used by [`Self::current_engine_config`] to project the active
    /// Main effort onto `QueryEngineConfig.thinking_level`.
    pub async fn resolve_role(&self, role: ModelRole) -> Option<RoleOverride> {
        {
            let overrides = self.role_overrides.read().await;
            if let Some(ov) = overrides.get(&role) {
                return Some(ov.clone());
            }
        }
        self.runtime_config
            .model_roles
            .get(role)
            .map(|spec| RoleOverride {
                spec: spec.clone(),
                effort: None,
            })
    }

    /// Install (or replace) an in-memory override for `role`. The
    /// override layers above `runtime_config.model_roles` and is NOT
    /// persisted to `~/.coco.json` — re-bind on every session via the
    /// picker, or edit settings to make the change durable.
    ///
    /// For `role == Main` this also rewrites
    /// `engine_config.{model_id, thinking_level}` so UI and tool
    /// context mirrors see the new identity on the next turn.
    pub async fn apply_role_override(
        &self,
        role: ModelRole,
        ov: RoleOverride,
    ) -> anyhow::Result<()> {
        let effort = ov.effort;
        let spec = ov.spec.clone();
        let model_id = ov.spec.model_id.clone();

        if role == ModelRole::Main {
            self.model_runtimes
                .rebind_role_primary(role, spec)
                .map_err(anyhow::Error::from)?;
            // Store the override only after the registry accepted the
            // replacement runtime.
            {
                let mut overrides = self.role_overrides.write().await;
                overrides.insert(role, ov);
            }
            self.update_engine_config(move |cfg| {
                cfg.model_id = model_id;
                cfg.thinking_level =
                    effort.map(|e| thinking_level_for_effort_from(&cfg.model_id, e));
            })
            .await;
            return Ok(());
        }

        let spec = ov.spec.clone();
        self.model_runtimes
            .rebind_role_primary(role, spec)
            .map_err(anyhow::Error::from)?;
        let mut overrides = self.role_overrides.write().await;
        overrides.insert(role, ov);
        Ok(())
    }

    /// Update only the `effort` on an existing role override, preserving
    /// the spec. The Main role's `engine_config.thinking_level` is
    /// rewritten so the next turn picks up the change.
    ///
    /// When the role has no prior override, the current
    /// `runtime_config.model_roles` spec is captured and stored
    /// alongside the new effort so subsequent reads see a consistent
    /// `RoleOverride`.
    pub async fn apply_role_effort(&self, role: ModelRole, effort: Option<ReasoningEffort>) {
        let spec_for_seed = self.runtime_config.model_roles.get(role).cloned();
        let mut overrides = self.role_overrides.write().await;
        match overrides.get_mut(&role) {
            Some(existing) => existing.effort = effort,
            None => {
                if let Some(spec) = spec_for_seed {
                    overrides.insert(role, RoleOverride { spec, effort });
                }
            }
        }
        drop(overrides);
        if role == ModelRole::Main {
            self.update_engine_config(|cfg| {
                cfg.thinking_level =
                    effort.map(|e| thinking_level_for_effort_from(&cfg.model_id, e));
            })
            .await;
        }
    }

    /// Snapshot the current `QueryEngineConfig` (clones the inner struct).
    /// Per-turn engine builds use this so mid-session mutations
    /// (`set_permission_mode`, `/clear` regen) propagate immediately.
    pub async fn current_engine_config(&self) -> QueryEngineConfig {
        self.engine_config.read().await.clone()
    }

    /// Render a live `/status` report from the session runtime. Replaces the
    /// former all-hardcoded-placeholder output with real values: the resolved
    /// Main/Fast model roles, permission mode, thinking level, plan-mode gate,
    /// and connected MCP servers. (Plugin count is intentionally omitted — the
    /// runtime holds no persistent plugin manager to read without a reload.)
    pub async fn status_report(&self) -> String {
        use std::fmt::Write as _;

        let cfg = self.current_engine_config().await;
        let mut out = String::from("Session status:\n");
        let _ = writeln!(out, "  Version: {}", env!("CARGO_PKG_VERSION"));

        if let Some(main) = self.resolve_role(ModelRole::Main).await {
            let _ = writeln!(
                out,
                "  Model: {} ({})",
                main.spec.model_id, main.spec.provider
            );
            if let Some(effort) = main.effort {
                let _ = writeln!(out, "  Effort: {effort:?}");
            }
        }
        let _ = writeln!(out, "  Permission mode: {:?}", cfg.permission_mode);
        match cfg.thinking_level.as_ref().map(|t| t.effort) {
            Some(effort) => {
                let _ = writeln!(out, "  Thinking: {effort:?}");
            }
            None => {
                let _ = writeln!(out, "  Thinking: off");
            }
        }
        let _ = writeln!(
            out,
            "  Plan mode: {}",
            if cfg.plan_mode_required { "on" } else { "off" }
        );
        if let Some(fast) = self.resolve_role(ModelRole::Fast).await {
            let _ = writeln!(
                out,
                "  Fast model: {} ({})",
                fast.spec.model_id, fast.spec.provider
            );
        }

        let servers = match self.current_mcp_handle().await {
            Some(handle) => handle.connected_servers().await,
            None => Vec::new(),
        };
        if servers.is_empty() {
            let _ = write!(out, "  MCP servers: none connected");
        } else {
            let _ = write!(
                out,
                "  MCP servers: {} connected ({})",
                servers.len(),
                servers.join(", ")
            );
        }
        out
    }

    /// Build a fresh `QueryEngine` for one turn using the runtime's
    /// stored `engine_config`. Both runners share this so the wiring
    /// can never drift. The session-memory text is refreshed from disk
    /// before each build so a fresh extraction shows up on the next turn.
    pub async fn build_engine(&self, cancel: CancellationToken) -> QueryEngine {
        self.build_engine_with_turn_abort(TurnAbortSignal::from_token(cancel))
            .await
    }

    /// The shared live permission-rule overlay (see field docs). Callers push
    /// mid-cycle approvals / team-rule updates here; every main-session engine
    /// reads it each tool batch.
    pub fn live_permission_rules(&self) -> Arc<RwLock<Vec<coco_types::PermissionRule>>> {
        self.live_permission_rules.clone()
    }

    /// Inject the live permission-rule overlay onto a main-session engine
    /// config. The overlay is now teammate-only: teammate `team_permission_update`
    /// rules never graduate into the live `ToolAppState.permissions` base, so
    /// there is nothing to reconcile/dedup against — the previous base-dedup
    /// step keyed off the now-deleted config rule maps and is gone.
    ///
    /// Called from every main-session build path (TUI `build_engine_with_turn_abort`
    /// and SDK/headless `build_engine_from_config_with_persistence`) so the
    /// in-cycle approval mechanism is uniform across transports. NOT called for
    /// subagents/forks — they keep their own isolated config-cloned rules.
    async fn prepare_live_permission_overlay(&self, config: &mut QueryEngineConfig) {
        config.live_permission_rules = Some(self.live_permission_rules.clone());
    }

    /// Single source of truth for applying user-approved permission updates,
    /// shared by every transport (TUI dialog, SDK approval reply, headless
    /// permission-prompt tool). Lands the rules in both places they must
    /// reach so in-cycle and cross-cycle behavior stays aligned:
    ///
    /// 1. the live `ToolAppState.permissions` base — the single authoritative
    ///    source the factory reads each batch (shared by Arc with the in-flight
    ///    engine + subagents/forks), so the approval is visible THIS cycle on
    ///    the next batch (e.g. an `Edit(...)` grant then satisfies a same-cycle
    ///    Read via the "edit access implies read" branch) AND across cycles;
    /// 2. disk, for destinations that persist (User/Project/Local).
    ///
    /// This is coco-rs's analog of TS `applyPermissionUpdate` +
    /// `setToolPermissionContext`.
    pub async fn apply_permission_updates_everywhere(
        &self,
        updates: &[coco_types::PermissionUpdate],
    ) {
        if updates.is_empty() {
            return;
        }
        // 1. Mutate the live shared base (`ToolAppState.permissions`) — the
        // single authoritative source the factory reads each batch, shared by
        // Arc with subagents/forks. This is both the in-cycle (the in-flight
        // engine re-reads it next batch) and cross-cycle home for the rule. The
        // Rust analog of TS `setToolPermissionContext(applyPermissionUpdate(...))`.
        {
            let mut guard = self.app_state.write().await;
            coco_permissions::apply_permission_updates_to_live(&mut guard.permissions, updates);
        }
        // 2. Persist destinations that wire to a settings.json layer.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let store = coco_permissions::SettingsPermissionStore::new(cwd);
        use coco_permissions::permissions_store::PermissionStore;
        for update in updates {
            let Some(dest) = update.destination() else {
                continue;
            };
            if !coco_permissions::permission_updates::supports_persistence(dest) {
                continue;
            }
            if let Err(e) = store.persist_update(update) {
                tracing::warn!(error = %e, "failed to persist permission update");
            }
        }
        let applied: Vec<String> = updates
            .iter()
            .filter_map(|u| match u {
                coco_types::PermissionUpdate::AddRules { rules, destination } => Some(
                    rules
                        .iter()
                        .map(|r| {
                            format!(
                                "{destination:?}:{}({})",
                                r.value.tool_pattern,
                                r.value.rule_content.as_deref().unwrap_or("*")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                _ => None,
            })
            .collect();
        tracing::info!(
            applied = ?applied,
            "permission updates applied to live base (ToolAppState.permissions) + disk persist",
        );
    }

    pub async fn build_engine_with_turn_abort(&self, turn_abort: TurnAbortSignal) -> QueryEngine {
        let mut engine_config = self.current_engine_config().await;
        self.prepare_live_permission_overlay(&mut engine_config)
            .await;
        let engine = QueryEngine::new_with_turn_abort(
            engine_config,
            self.model_runtimes.clone(),
            self.tools.clone(),
            turn_abort,
            Some(self.hook_registry.clone()),
        );
        let engine = self
            .wire_engine(engine, None, EnginePersistenceMode::MainSession)
            .await;
        // Inject the in-prompt-shell Bash handle into the LIVE command registry
        // so skill / `ShellExpandingPromptHandler` markers route through the real
        // Bash tool with a per-command permission check. Refreshed on every
        // main-session engine build, so it also survives a `/reload-plugins`
        // registry swap (the new registry starts with an empty handle cell).
        let base_ctx = engine.build_base_tool_context().await;
        let registry = self.command_registry.read().await.clone();
        // One handle, two consumers: the command registry (slash /
        // shell-expanding prompt commands) and the skill runtime's shared
        // cell (model-invoked + fork-mode skills). Refreshed every build so
        // it survives a `/reload-plugins` registry swap and tracks the
        // latest tool config / cwd.
        let bash_handle = crate::bash_tool_handle::build_session_bash_handle(base_ctx);
        registry.set_bash_tool_handle(bash_handle.clone());
        if let Ok(mut cell) = self.skill_bash_cell.write() {
            *cell = Some(bash_handle);
        }
        // Late-bind the session id so user-typed skill slash commands can
        // substitute `${CLAUDE_SESSION_ID}`.
        registry.set_session_id(self.session_id.read().await.clone());
        // Capture this engine's cache-safe-params handle so a between-turns
        // `/btw` fork can read the parent turn's `CacheSafeParams` even though
        // the engine is rebuilt per turn. The handle is an `Arc` shared with
        // the engine's slot — it keeps observing writes the engine makes at
        // turn finalize and outlives the engine drop.
        *self.last_engine_cache_handle.write().await = Some(engine.cache_safe_params_handle());
        engine
    }

    pub async fn analyze_main_context(
        &self,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        let history = {
            let guard = self.history.lock().await;
            guard.snapshot()
        };
        self.analyze_context_snapshot(history, None).await
    }

    pub async fn analyze_context_snapshot(
        &self,
        history: MessageHistory,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        let engine = self.build_engine(CancellationToken::new()).await;
        let engine = if let Some(app_state) = app_state_override {
            engine.with_app_state(app_state)
        } else {
            engine
        };
        coco_query::context_analysis::analyze_engine_context_with_sources(
            &engine,
            &history,
            Some(self.skill_manager.clone()),
        )
        .await
    }

    /// Public accessor for the hook registry. Same `Arc` as the one
    /// installed on every per-turn engine; safe to clone.
    pub fn hook_registry(&self) -> Arc<HookRegistry> {
        self.hook_registry.clone()
    }

    /// Public accessor for the session-scoped [`coco_skills::SkillManager`].
    /// Same `Arc` that backed the command-registry build and the
    /// reminder pipeline — safe to clone (cheap ref-count bump).
    /// Used by binary-entry wiring (e.g. `mcp_handle_adapter`) that
    /// sits outside the crate's `pub(crate)` field-access scope.
    pub fn skill_manager(&self) -> Arc<coco_skills::SkillManager> {
        self.skill_manager.clone()
    }

    /// Session-scoped command queue handle. Producers outside the
    /// per-turn engine — the TUI bridge in `tui_runner` (user typing
    /// while busy), future task-completion / coordinator / hook
    /// forwarders — call `enqueue` on this handle to inject mid-turn
    /// steering messages. Returned by reference; callers `.clone()` if
    /// they need an owned `Arc`-backed handle.
    ///
    /// Teammate messages and task notifications use the same queue
    /// with `QueueOrigin::Coordinator` / `QueueOrigin::TaskNotification`.
    pub fn command_queue(&self) -> &CommandQueue {
        &self.command_queue
    }

    /// The session's schedule store (cron tasks + triggers). Shared with the
    /// cron tick driver ([`crate::cron_tick`]) so it reads/writes the same
    /// tasks the `Cron*` tools persist.
    pub fn schedule_store(&self) -> coco_tool_runtime::ScheduleStoreRef {
        self.schedule_store.clone()
    }

    /// Session shutdown signal — long-lived background tasks (e.g. the cron
    /// tick) observe it for clean teardown.
    pub fn shutdown_signal(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Build a closure that materialises an
    /// [`coco_hooks::orchestration::OrchestrationContext`] tied to the
    /// current session's identity / cwd / disable flags.
    ///
    /// Used by detached hook firings (e.g. the `Elicitation` /
    /// `ElicitationResult` wrapper around `SendElicitation`, the
    /// FileChanged file watcher) that need a context built from inside
    /// a sync closure. Each call reads the synchronous snapshot mirrors
    /// kept up to date by session/config mutations, avoiding Tokio
    /// `blocking_read()` on runtime worker threads.
    pub fn orchestration_ctx_factory(
        self: &Arc<Self>,
    ) -> Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync> {
        let runtime = self.clone();
        Arc::new(move || {
            let cfg = clone_std_rwlock(&runtime.orchestration_engine_config);
            let session_id = clone_std_rwlock(&runtime.orchestration_session_id);
            coco_hooks::orchestration::OrchestrationContext {
                session_id,
                cwd: std::env::current_dir().unwrap_or_default(),
                project_dir: cfg.project_dir.clone(),
                permission_mode: None,
                transcript_path: None,
                agent_id: None,
                agent_type: None,
                cancel: runtime.cancel.clone(),
                disable_all_hooks: cfg.disable_all_hooks,
                allow_managed_hooks_only: cfg.allow_managed_hooks_only,
                attachment_emitter: coco_messages::AttachmentEmitter::noop(),
                sync_event_sink: None,
                http_url_allowlist: None,
                http_env_var_policy: None,
                async_registry: Some(runtime.async_hook_registry.clone()),
                async_rewake_sink: Some(async_rewake_sink(&runtime.command_queue)),
                llm_handle: Some(runtime.hook_llm_handle.clone()),
                workspace_trust_accepted: None,
            }
        })
    }

    /// Build a fresh `QueryEngine` from a caller-provided
    /// `QueryEngineConfig`. Used by SDK paths whose per-turn config
    /// fields (model, session_id, max_*) come from the
    /// `turn/start` request and override the runtime defaults.
    ///
    /// `app_state_override` lets the caller pin a specific
    /// `ToolAppState` Arc — SDK passes `Some(handoff.app_state)` so
    /// per-session app state and the compaction observers built from
    /// it stay coherent. TUI passes `None` and inherits
    /// `runtime.app_state`.
    pub async fn build_engine_from_config(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> QueryEngine {
        self.build_engine_from_config_with_persistence(
            config,
            cancel,
            app_state_override,
            EnginePersistenceMode::MainSession,
        )
        .await
    }

    /// Build a fork engine from a caller-provided config. Fork engines
    /// share runtime services but never write to the parent main-session
    /// transcript, usage tracker, or file-history sink.
    pub(crate) async fn build_fork_engine_from_config(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> QueryEngine {
        self.build_engine_from_config_with_persistence(
            config,
            cancel,
            app_state_override,
            EnginePersistenceMode::Fork,
        )
        .await
    }

    async fn build_engine_from_config_with_persistence(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
        persistence: EnginePersistenceMode,
    ) -> QueryEngine {
        // Top-level SDK/headless session engines get the same live overlay as
        // the TUI main engine so a mid-cycle approval takes effect this cycle.
        // Gated to the main session (`MainSession` + no `agent_id`): subagents
        // and forks keep their own isolated config-cloned rules — they must not
        // share or reconcile the main session's overlay.
        //
        // `config` is owned and local — the `mut` is confined to this block so
        // the rest of the function sees an immutable binding. The only shared
        // state touched is the overlay Arc, serialized by its `RwLock` inside
        // the helper; there is no cross-task sharing of `config` itself.
        let config = if matches!(persistence, EnginePersistenceMode::MainSession)
            && config.agent_id.is_none()
        {
            let mut config = config;
            self.prepare_live_permission_overlay(&mut config).await;
            config
        } else {
            config
        };
        // Fork isolation for the file-read dedup cache: when a fork sets
        // `clone_file_read_state` (default true for every framework fork),
        // give the child a *deep clone* of the parent's `FileReadState`
        // instead of the shared Arc `wire_engine` installs. The fork then
        // sees the parent's already-seen ids (cache parity preserved) but
        // its own reads/edits can't pollute the parent's cache. Mirrors TS
        // `createSubagentContext`, which clones `readFileState` per fork.
        let isolate_file_read_state = config
            .fork_isolation
            .as_ref()
            .is_some_and(|iso| iso.clone_file_read_state);
        let engine = QueryEngine::new(
            config,
            self.model_runtimes.clone(),
            self.tools.clone(),
            cancel,
            Some(self.hook_registry.clone()),
        );
        let mut engine = self
            .wire_engine(engine, app_state_override, persistence)
            .await;
        if isolate_file_read_state {
            let snapshot = self.file_read_state.read().await.clone();
            engine = engine.with_file_read_state(Arc::new(RwLock::new(snapshot)));
        }
        engine
    }

    /// Install every per-session subsystem on a pre-built engine. The
    /// single source of truth for "what subsystems an engine needs" —
    /// both runners route through this so a new subsystem only needs
    /// adding here, not in two transport-specific spots.
    ///
    /// `app_state_override`: when `Some`, this Arc is what the engine
    /// gets via `with_app_state`, AND it's what the compaction
    /// observers reset on `notify_all`. When `None`, falls back to the
    /// runtime's own `app_state`. Without this override, SDK's
    /// `handoff.app_state` would be installed on the engine but
    /// `runtime.app_state` would be reset by observers — the two would
    /// drift after every compaction.
    async fn wire_engine(
        &self,
        mut engine: QueryEngine,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
        persistence: EnginePersistenceMode,
    ) -> QueryEngine {
        let app_state = app_state_override.unwrap_or_else(|| self.app_state.clone());
        engine = engine.with_file_read_state(self.file_read_state.clone());
        engine = engine.with_app_state(app_state.clone());
        // `auto_mode_state` is a SESSION-GLOBAL flag shared by every engine in
        // this runtime. It is NO LONGER the classifier gate — that now reads the
        // per-call `permission_context.mode` (see `tool_call_preparer`, TS
        // parity), so a stale/raced flag can't suppress the classifier. The flag
        // survives only for the Plan→Auto bridge, denial-streak reset, and TUI
        // display. Sync it from the session's authoritative `self.app_state`
        // (NOT the per-build `app_state` override): a fork/skill/compaction
        // sub-engine carrying a non-Auto override would otherwise clobber it.
        // Every build re-syncs from the single source, covering all mode-change
        // funnels (TUI + SDK) uniformly without threading the flag through each.
        let auto_active = self
            .app_state
            .read()
            .await
            .permissions
            .mode
            .is_some_and(|mode| mode == coco_types::PermissionMode::Auto);
        self.auto_mode_state.set_active(auto_active);
        // Build the classifier rules from settings (`auto_mode` is restricted
        // to user/policy sources by the per-source validator). Previously this
        // passed `::default()`, so allow/soft_deny/environment AND the
        // classifier mode were all silently dropped.
        let auto_mode_rules = self
            .runtime_config
            .settings
            .merged
            .auto_mode
            .as_ref()
            .map(|c| coco_permissions::AutoModeRules {
                allow: c.allow.clone(),
                soft_deny: c.soft_deny.clone(),
                environment: c.environment.clone(),
                classifier_mode: c.classifier_mode,
                classifier_unavailable_fail_open: c.classifier_unavailable_fail_open,
            })
            .unwrap_or_default();
        engine = engine.with_auto_mode(
            self.auto_mode_state.clone(),
            self.denial_tracker.clone(),
            auto_mode_rules,
        );
        // Skill-emitted `permission_updates` now flow through the
        // engine's own per-engine `EngineLiveRulesHandle`
        // (auto-installed by `QueryEngine::new`) which writes into
        // `QueryEngine.live_command_rules` — a fresh Arc per engine
        // = per user message. No session-level handle install: that
        // would leak rules across user messages. See `engine_live_rules`
        // for the lifecycle invariant.
        // Session-scoped steering primitive. Without this, a fresh
        // `CommandQueue::new()` is constructed in `QueryEngine::new` and
        // dies with the per-turn engine, so any producer (TUI bridge,
        // future task / coordinator forwarders) enqueueing on
        // `runtime.command_queue()` would land on an instance the
        // running engine cannot see.
        engine = engine.with_command_queue(self.command_queue.clone());
        // Same lifetime argument as `with_command_queue`: the attachment
        // channel must live across engine rebuilds so cross-turn
        // producers (TUI slash commands, future swarm forwarders) see a
        // stable handle. The engine's own per-instance attachment
        // channel is replaced by the session-scoped one.
        engine = engine.with_attachment_channel(
            self.session_attachment_tx.clone(),
            self.session_attachment_rx.clone(),
        );
        if let Some(svc) = &self.session_memory_service {
            let sm_text_now = svc.current_text().await;
            engine = engine.with_session_memory_text(sm_text_now);
            engine = engine.with_session_memory_service(svc.clone());
        }
        // Install the real swarm-backed AgentHandle so AgentTool /
        // SendMessageTool / TeamCreateTool reach the swarm runtime
        // on every engine instance.
        engine = engine.with_agent_handle(self.swarm_agent_handle.clone());
        // Install the per-engine sync-hook-event buffer so the
        // `OrchestrationContext.sync_event_sink` constructed from this
        // engine's `orchestration_ctx()` writes into the same buffer
        // that the reminder source below drains.
        engine = engine.with_sync_hook_buffer(self.sync_hook_buffer.clone());
        // Same wiring for async hooks: the engine's `orchestration_ctx`
        // populates `async_registry` so engine-fired async hooks
        // (PreToolUse / PostToolUse / Stop / SubagentStop with
        // `is_async: true`) deliver via `CombinedHookEventsSource`.
        engine = engine.with_async_hook_registry(self.async_hook_registry.clone());
        // Same wiring for the LLM-driven hook handler so the engine's
        // `orchestration_ctx` carries it on every fired event — Prompt
        // / Agent settings hooks reach the LLM via `QueryHookLlm`.
        engine = engine.with_hook_llm_handle(self.hook_llm_handle.clone());
        engine = engine.with_model_runtimes(self.model_runtimes.clone());
        engine =
            engine.with_session_start_hook_side_effect_sink(Arc::new(QuerySessionStartHookSink {
                file_watch: self.file_watch_registration_context(),
            }));
        if let Some(runtime) = &self.memory_runtime {
            engine = engine.with_memory_runtime(runtime.clone());
        }
        // Reminder sources — populated unconditionally so non-memory
        // sessions still get hook + skill reminders. Each slot is
        // optional and silently skips if its data is empty.
        let team_snapshot = self.resolve_team_snapshot().await;
        let sources = coco_system_reminder::ReminderSources {
            // Combined hook source: async-hook registry drains first,
            // then the sync-hook buffer that orchestration just wrote.
            hook_events: Some(Arc::new(
                coco_hooks::reminder_source::CombinedHookEventsSource::new(
                    self.async_hook_registry.clone(),
                    self.sync_hook_buffer.clone(),
                ),
            )),
            // Memory source: only when the runtime is built (gated on
            // `Feature::AutoMemory` upstream).
            memory: self.memory_runtime.as_ref().map(|runtime| {
                Arc::new(coco_query::reminder_adapters::MemoryAdapter::new(
                    runtime.clone(),
                )) as Arc<dyn coco_system_reminder::MemorySource>
            }),
            // Skills source: in-process `SkillManager` Arc kept alive
            // for the session. Empty manager ⇒ generator short-circuits.
            skills: Some(self.skill_manager.clone() as Arc<dyn coco_system_reminder::SkillsSource>),
            // Swarm source: drains peer messages from the shared pending
            // store, so a teammate's `SendMessage` surfaces as an
            // `agent_pending_messages` reminder on the recipient's next turn.
            // MUST share the SAME `Arc` as `engine.with_pending_messages`
            // below (the producer side) — otherwise messages vanish.
            swarm: Some(Arc::new(
                coco_query::reminder_adapters::SwarmAdapter::new()
                    .with_pending_messages(self.pending_message_store.clone())
                    .with_team_context(team_snapshot),
            ) as Arc<dyn coco_system_reminder::SwarmSource>),
            ..Default::default()
        };
        engine = engine.with_reminder_sources(sources);
        // Producer side of the pending-message pipeline: `SendMessage` pushes
        // into `ToolUseContext.pending_messages` (= this store). Shared across
        // the leader + in-process teammate engines (both via `wire_engine`).
        engine = engine.with_pending_messages(self.pending_message_store.clone());
        // Build observers fresh per call so the FileReadState and
        // AppState observers reference the engine's actual handles.
        // Cheap — the registry is just a Vec of Arc<dyn Observer>.
        let observers = coco_query::observers::build_default_registry(
            Some(self.file_read_state.clone()),
            Some(self.denial_tracker.clone()),
            Some(app_state),
        );
        engine = engine.with_compaction_observers(observers);
        engine = engine.with_mailbox(self.mailbox.clone());
        // Install the MCP handle so AgentTool::prompt's per-turn
        // dynamic listing can pre-filter agents whose
        // `required_mcp_servers` aren't connected. Snapshot semantics:
        // each engine instance reads the handle slot at wire time;
        // hot-reloads land on the next engine.
        if let Some(mcp) = self.mcp_handle.read().await.clone() {
            engine = engine.with_mcp_handle(mcp);
        }
        engine = engine.with_schedule_store(self.schedule_store.clone());
        // Same snapshot pattern as MCP — every per-turn engine reads
        // the late-bound LSP slot once at wire time. Hot-reloads of
        // the LSP config land on the next engine build.
        if let Some(lsp) = self.lsp_handle.read().await.clone() {
            engine = engine.with_lsp_handle(lsp);
        }
        // Install the agent catalog snapshot so `AgentTool::prompt`
        // renders the dynamic per-turn agent listing. Without this the
        // engine falls back to `AgentTool`'s static description and
        // the model never sees the agents it can actually spawn.
        // Each engine instance captures the inner `Arc<...>` once at
        // wire time; concurrent `/agents reload` swaps land on the
        // next per-turn engine, not the in-flight one.
        engine = engine.with_agent_catalog(self.agent_catalog.read().await.clone());
        // config_home drives plan-mode (`plans_dir` / `session_plan_file`)
        // independent of persistence — always wire it; only the file-history
        // snapshot store is gated by persistence.
        engine = engine.with_config_home(self.config_home.clone());
        if persistence == EnginePersistenceMode::MainSession
            && self.persist_session
            && let Some(fh) = &self.file_history
        {
            engine = engine.with_file_history(fh.clone(), self.config_home.clone());
        }
        if let Some(bridge) = &self.permission_bridge {
            engine = engine.with_permission_bridge(bridge.clone());
        }
        // Main-session transcript persistence. Same `TranscriptStore`
        // instance feeds both the per-turn user / assistant JSONL
        // append in `engine_finalize_turn::record_transcript_tail`
        // and the metadata writes already wired there. The dedup set
        // lives on `SessionRuntime` so a fresh per-turn engine doesn't
        // re-write history each time; writes are keyed by session id and
        // skip already-persisted uuids.
        if persistence == EnginePersistenceMode::MainSession && self.persist_session {
            let live_session_id = self.session_id.read().await.clone();
            engine = engine.with_transcript_store(self.transcript_store.clone(), live_session_id);
            engine = engine.with_session_usage_tracker(self.session_usage_tracker.clone());
            engine = engine.with_session_usage_write_lock(self.session_usage_write_lock.clone());
            engine = engine.with_transcript_dedup(self.transcript_dedup.clone());
            engine = engine
                .with_tool_result_replacement_state(self.tool_result_replacement_state.clone());
        }
        // Agent handle: installed by bootstrap after TaskRuntime exists.
        // Until then the engine carries the explicit no-op handle from
        // `swarm_agent_handle`.
        if let Some(handle) = self.agent_handle.read().await.clone() {
            engine = engine.with_agent_handle(handle);
        }
        // Skill handle: installed by bootstrap (`agent_handle_factory`)
        // once the subagent engine adapter exists. Until then the engine
        // carries `NoOpSkillHandle` and every `SkillTool` call returns
        // `Unavailable`. Installed on subagent engines too (this runs via
        // `build_engine_from_config`) so children can invoke skills.
        if let Some(handle) = self.skill_handle.read().await.clone() {
            engine = engine.with_skill_handle(handle);
        }
        // Fork dispatcher (D1/D2). Same late-bind contract as
        // `agent_handle` — installed only when `attach_fork_dispatcher`
        // ran at bootstrap. Without it, post-turn forks fall back to
        // their no-op paths (placeholder text / silent skip).
        if let Some(dispatcher) = self.fork_dispatcher.read().await.clone() {
            engine = engine.with_fork_dispatcher(dispatcher);
        }
        // Session-scoped prompt-suggestion abort slot. Sharing the same
        // `Arc` across every per-turn engine lets a new spawn cancel the
        // in-flight previous one.
        engine = engine.with_current_suggestion_abort(self.current_suggestion_abort.clone());
        // Production task runtime — same `Arc` is shared with
        // `SwarmAgentHandle` so AgentTool background spawns and the
        // engine's `Task*` tools see one source of truth.
        if let Some(rt) = self.task_runtime.read().await.clone() {
            engine = engine.with_task_handle(rt as coco_tool_runtime::BackgroundTaskHandleRef);
        }
        if let Some(task_list) = self.task_list.read().await.clone() {
            engine = engine.with_task_list(task_list);
        }
        if let Some(router) = self.team_task_list_router.read().await.clone() {
            engine = engine.with_team_task_list_router(router);
        }
        engine
    }

    /// Install the agent-spawn handle on this runtime. Called once
    /// after `build()` returns the `Arc<Self>`. The handle is
    /// late-bound because the adapter inside it needs to capture
    /// `Arc<Self>` to drive per-spawn engine builds — calling this
    /// from inside `build()` would create a cycle.
    pub async fn attach_agent_handle(&self, handle: AgentHandleRef) {
        *self.agent_handle.write().await = Some(handle.clone());
        if let Some(runtime) = &self.memory_runtime {
            runtime.install_agent(handle);
        }
    }

    /// Install the skill-execution handle (`QuerySkillRuntime`). Late-bound
    /// alongside `attach_agent_handle` because the real impl wraps the same
    /// subagent `AgentQueryEngineRef` the swarm handle uses. Once set,
    /// `wire_engine` installs it on every per-turn engine so the model's
    /// `SkillTool` and user-typed fork-mode `/slash` skills resolve.
    pub async fn attach_skill_handle(&self, handle: coco_tool_runtime::SkillHandleRef) {
        *self.skill_handle.write().await = Some(handle);
    }

    /// Snapshot the installed skill handle, if any. Used by the TUI
    /// slash-command dispatch to run user-typed fork-mode skills through
    /// the same `SkillHandle` the model's `SkillTool` uses.
    pub async fn skill_handle(&self) -> Option<coco_tool_runtime::SkillHandleRef> {
        self.skill_handle.read().await.clone()
    }

    /// The shared Bash-handle cell threaded into `QuerySkillRuntime` so the
    /// skill paths run the same permission-checked in-prompt shell as
    /// slash-command handlers. Clone shares the same `Arc` cell.
    pub(crate) fn skill_bash_cell(
        &self,
    ) -> Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>> {
        self.skill_bash_cell.clone()
    }

    /// Run a user-typed fork-mode skill (`/<name>`) through the installed
    /// `SkillHandle`, mirroring TS `executeForkedSlashCommand`: the skill
    /// body runs as a subagent and its final text is returned for the
    /// caller to inject as a `<local-command-stdout>` block — no follow-up
    /// main-model query.
    ///
    /// The gate marks the skill as user-invoked (`typed_slashes_in_turn`),
    /// so it bypasses the `disable_model_invocation` author lock and the
    /// `user-invocable-only` override. Inheritance is taken from the live
    /// engine config so the subagent runs with the session's features /
    /// permission mode / tool overrides. Returns `Err` when no skill
    /// runtime is installed or the fork fails.
    pub async fn invoke_skill_fork(&self, name: &str, args: &str) -> Result<String, String> {
        let handle = self
            .skill_handle()
            .await
            .ok_or_else(|| "no skill runtime installed".to_string())?;
        let cfg = self.current_engine_config().await;
        let inherit = coco_tool_runtime::SubagentInheritance {
            session_id: self.current_session_id().await,
            permission_mode: cfg.permission_mode,
            features: Some(cfg.features.clone()),
            tool_overrides: Some(cfg.tool_overrides.clone()),
            active_shell_tool: cfg.active_shell_tool,
            parent_tool_filter: None,
        };
        let gate = coco_tool_runtime::SkillGateContext {
            overrides: cfg.skill_overrides.clone(),
            typed_slashes_in_turn: std::iter::once(name.to_string()).collect(),
        };
        match handle.invoke_skill(name, args, inherit, gate).await {
            Ok(coco_tool_runtime::SkillInvocationResult::Forked { output, .. }) => Ok(output),
            // A fork-context skill always resolves to `Forked`; the inline
            // arm is defensive (e.g. a skill whose context flipped between
            // registry snapshot and dispatch) — surface its summary.
            Ok(coco_tool_runtime::SkillInvocationResult::Inline { summary, .. }) => Ok(summary),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Interrupt an in-process teammate's current turn without
    /// cancelling the teammate lifecycle.
    pub async fn interrupt_agent_current_work(&self, agent_id: &str) -> Result<bool, String> {
        let handle = self
            .agent_handle
            .read()
            .await
            .clone()
            .unwrap_or_else(|| self.swarm_agent_handle.clone());
        handle.interrupt_agent_current_work(agent_id).await
    }

    /// Install the post-turn fork dispatcher (D1/D2). Late-bound for
    /// the same Arc-cycle reason as `attach_agent_handle`: the
    /// dispatcher impl captures `Arc<Self>` to build per-fork engines.
    pub async fn attach_fork_dispatcher(
        &self,
        dispatcher: coco_query::forked_agent::ForkDispatcherRef,
    ) {
        *self.fork_dispatcher.write().await = Some(dispatcher);
    }

    /// Install the runtime-backed Agent hook runner onto the shared
    /// LLM hook handle. Called after `SessionRuntime::build` returns
    /// because the runner captures `Arc<SessionRuntime>`.
    pub async fn attach_hook_agent_runner(&self, runner: coco_query::hook_llm::HookAgentRunnerRef) {
        self.hook_llm_handle.install_agent_runner(runner).await;
    }

    /// Snapshot the registered tool set for scoped child registries.
    pub(crate) fn registered_tools(&self) -> Vec<Arc<dyn coco_tool_runtime::DynTool>> {
        self.tools.all()
    }

    /// Build an engine with caller-supplied scoped registries, then
    /// apply the standard wiring. Used by the hook-agent runner, a scoped
    /// child engine that must not write to the main-session transcript,
    /// usage tracker, or file-history sink — so it wires as `Fork`.
    pub(crate) async fn build_engine_from_config_with_registries(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        tools: Arc<ToolRegistry>,
        hooks: Option<Arc<coco_hooks::HookRegistry>>,
    ) -> QueryEngine {
        let engine = QueryEngine::new(config, self.model_runtimes.clone(), tools, cancel, hooks);
        self.wire_engine(engine, None, EnginePersistenceMode::Fork)
            .await
    }

    /// Read the currently installed fork dispatcher. Returns `None`
    /// before bootstrap installs one (or in unit tests). Used by SDK
    /// runners that want to dispatch a fork outside of the engine's
    /// post-turn hook (`/btw` over the SDK protocol).
    pub async fn current_fork_dispatcher(
        &self,
    ) -> Option<coco_query::forked_agent::ForkDispatcherRef> {
        self.fork_dispatcher.read().await.clone()
    }

    /// Read the most recent turn's cache-safe params. `None` before the
    /// first turn finalises (or after `/clear`). The TUI `/btw` dispatch
    /// reads this to share the parent's prompt cache; the SDK path reads
    /// the same data straight off its persistent `engine`.
    pub async fn last_cache_safe_params(&self) -> Option<coco_types::CacheSafeParams> {
        let handle = self.last_engine_cache_handle.read().await.clone();
        match handle {
            Some(h) => h.read().await.clone(),
            None => None,
        }
    }

    /// Install the background task runtime. Called once during CLI
    /// bootstrap; the same `Arc` flows into `SwarmAgentHandle` for
    /// the registration side. Idempotent — re-attaching replaces.
    pub async fn attach_task_runtime(&self, rt: Arc<crate::task_runtime::TaskRuntime>) {
        *self.task_runtime.write().await = Some(rt);
    }

    /// Read the installed task runtime. `None` when no production
    /// runtime is wired (tests, headless paths that don't use bg
    /// AgentTool). Used by `agent_handle_factory` to share the same
    /// instance with `SwarmAgentHandle`.
    pub async fn current_task_runtime(&self) -> Option<Arc<crate::task_runtime::TaskRuntime>> {
        self.task_runtime.read().await.clone()
    }

    pub async fn attach_task_list(&self, handle: coco_tool_runtime::TaskListHandleRef) {
        *self.task_list.write().await = Some(handle);
    }

    pub async fn attach_team_task_list_router(
        &self,
        router: coco_tool_runtime::TeamTaskListRouterRef,
    ) {
        *self.team_task_list_router.write().await = Some(router);
    }

    pub async fn current_task_list(&self) -> Option<coco_tool_runtime::TaskListHandleRef> {
        self.task_list.read().await.clone()
    }

    pub async fn current_team_task_list_router(
        &self,
    ) -> Option<coco_tool_runtime::TeamTaskListRouterRef> {
        self.team_task_list_router.read().await.clone()
    }

    /// Install the per-agent transcript / metadata store used for
    /// background AgentTool resume. Late-bind: same lifecycle as
    /// `attach_task_runtime`. `agent_handle_factory` reads this and
    /// forwards onto `SwarmAgentHandle::set_transcript_store`.
    pub async fn attach_agent_transcript_store(
        &self,
        store: coco_tool_runtime::AgentTranscriptStoreRef,
    ) {
        *self.agent_transcript_store.write().await = Some(store);
    }

    /// Read the installed agent-transcript store.
    pub async fn current_agent_transcript_store(
        &self,
    ) -> Option<coco_tool_runtime::AgentTranscriptStoreRef> {
        self.agent_transcript_store.read().await.clone()
    }

    /// Reset all per-session subsystems and adopt a new session id.
    ///
    /// Used by SDK `session/start` to flip from an archived session to
    /// a fresh one without rebuilding the entire `SessionRuntime`.
    /// Caller-owned state (`SessionHandle.history`,
    /// `SessionHandle.app_state` per the SDK protocol) is created fresh
    /// by the caller; this method only refreshes runtime-owned state
    /// keyed on session_id.
    ///
    /// What gets reset:
    /// - `runtime.session_id` → `new_session_id`
    /// - `runtime.engine_config.session_id` (next per-turn engine sees it)
    /// - `runtime.session_memory_service` (`set_session_id` + cache wipe)
    /// - `runtime.file_read_state` (LRU cleared so prior session's
    ///   @mention dedup doesn't leak)
    /// - `runtime.file_history_sink_session_id` Arc (next snapshot
    ///   targets new session's transcript jsonl)
    /// - cache-break detectors on registry-owned model runtimes
    ///   (baseline drop on first new-session call won't false-positive)
    ///
    /// Fire SessionStart hooks with the given `source` string ("startup",
    /// "resume", "compact", "clear"). Output flows into the shared
    /// `sync_hook_buffer` so it surfaces as `hook_*` reminders on the
    /// Fire SessionStart hooks for the given source. The result is buffered
    /// into `sync_hook_buffer` to surface as reminders on the next turn.
    ///
    /// Runners call this once at session bootstrap (TUI / SDK) so the
    /// first turn's reminder pass picks up the events. Failure is
    /// logged + tolerated; no panic on hook misconfig.
    pub async fn fire_session_start_hooks(&self, source: &str) {
        // `source` is the closed enum `('startup' | 'resume' | 'clear' | 'compact')`.
        // Parse here so callers using bare strings still work, but log + skip if
        // the string is unrecognised to avoid wiring an out-of-spec value.
        let parsed_source = match source {
            "startup" => coco_hooks::orchestration::SessionStartSource::Startup,
            "resume" => coco_hooks::orchestration::SessionStartSource::Resume,
            "clear" => coco_hooks::orchestration::SessionStartSource::Clear,
            "compact" => coco_hooks::orchestration::SessionStartSource::Compact,
            other => {
                warn!(
                    source = other,
                    "SessionStart hook fired with unrecognised source; skipping"
                );
                return;
            }
        };
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        let model_arg = if cfg.model_id.is_empty() {
            None
        } else {
            Some(cfg.model_id.as_str())
        };
        match coco_hooks::orchestration::execute_session_start(
            &self.hook_registry,
            &ctx,
            parsed_source,
            /*agent_type*/ None,
            model_arg,
        )
        .await
        {
            Ok(agg) => {
                // Hook output may register paths the FileChanged watcher
                // should monitor. Hand them off to the runtime's shared
                // watcher so subsequent file events fire FileChanged hooks.
                // Empty vec is a no-op.
                if !agg.watch_paths.is_empty() {
                    self.add_file_watch_paths(agg.watch_paths.clone()).await;
                }
            }
            Err(e) => {
                warn!(error = %e, source, "SessionStart hook execution failed at startup");
            }
        }
    }

    /// Fire Setup hooks (`Maintenance` at bootstrap, `Init` at `coco init`).
    ///
    /// Output is fire-and-forget — Setup is observability-only (no blocking,
    /// no continuation signals). Failure is logged.
    pub async fn fire_setup_hooks(&self, trigger: coco_hooks::orchestration::SetupTrigger) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        if let Err(e) =
            coco_hooks::orchestration::execute_setup(&self.hook_registry, &ctx, trigger).await
        {
            warn!(error = %e, ?trigger, "Setup hook execution failed");
        }
    }

    /// Fire UserPromptSubmit hooks for the given prompt text. Output
    /// flows into the shared `sync_hook_buffer`. Returns the aggregated
    /// result so the caller can honour `blocking_error` (suppress the
    /// turn) and `prevent_continuation` (skip the turn but keep the
    /// prompt).
    pub async fn fire_user_prompt_submit_hooks(
        &self,
        prompt: &str,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        match coco_hooks::orchestration::execute_user_prompt_submit(
            &self.hook_registry,
            &ctx,
            prompt,
        )
        .await
        {
            Ok(agg) => agg,
            Err(e) => {
                warn!(error = %e, "UserPromptSubmit hook execution failed");
                coco_hooks::orchestration::AggregatedHookResult::default()
            }
        }
    }

    /// Fire Notification hooks. Called from `TuiPermissionBridge` /
    /// `SdkPermissionBridge` when the user is about to be asked for
    /// input (`permission_prompt`), and from any future idle / elicitation
    /// prompts. Output is fire-and-forget — awaited only to preserve
    /// ordering before the actual UI notification, never to block the
    /// prompt itself.
    pub async fn fire_notification_hooks(
        &self,
        notification_type: &str,
        message: &str,
        title: Option<&str>,
    ) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        if let Err(e) = coco_hooks::orchestration::execute_notification(
            &self.hook_registry,
            &ctx,
            notification_type,
            message,
            title,
        )
        .await
        {
            warn!(
                error = %e,
                notification_type,
                "Notification hook execution failed"
            );
        }
    }

    fn file_watch_registration_context(&self) -> FileWatchRegistrationContext {
        FileWatchRegistrationContext {
            file_changed_watcher: self.file_changed_watcher.clone(),
            hook_registry: self.hook_registry.clone(),
            session_id: self.session_id.clone(),
            engine_config: self.engine_config.clone(),
            cancel: self.cancel.clone(),
            async_hook_registry: self.async_hook_registry.clone(),
            command_queue: self.command_queue.clone(),
            hook_llm_handle: self.hook_llm_handle.clone(),
        }
    }

    /// Append paths to the `FileChanged` watcher, lazily constructing
    /// it on first call. Empty input is a no-op.
    pub async fn add_file_watch_paths(&self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.file_watch_registration_context()
            .add_paths(paths)
            .await;
    }

    /// Fire CwdChanged hooks.
    ///
    /// Callers must capture the old cwd before mutating
    /// `std::env::current_dir`. Surfacing the helper lets ad-hoc
    /// cwd-mutating code paths (worktree exit, SDK setCwd control) wire
    /// the hook without re-implementing the orchestration context build.
    pub async fn fire_cwd_changed_hooks(&self, old_cwd: &str, new_cwd: &str) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::path::PathBuf::from(new_cwd),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        match coco_hooks::orchestration::execute_cwd_changed(
            &self.hook_registry,
            &ctx,
            old_cwd,
            new_cwd,
        )
        .await
        {
            Ok(agg) => {
                // The cwd swap is a natural moment for hooks to update
                // the FileChanged watch list (e.g. add the new project's
                // `.envrc`).
                if !agg.watch_paths.is_empty() {
                    self.add_file_watch_paths(agg.watch_paths.clone()).await;
                }
            }
            Err(e) => {
                warn!(error = %e, old_cwd, new_cwd, "CwdChanged hook execution failed");
            }
        }
    }

    /// Fire ConfigChange hooks.
    pub async fn run_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        match coco_hooks::orchestration::execute_config_change(
            &self.hook_registry,
            &ctx,
            source,
            file_path,
        )
        .await
        {
            Ok(agg) => agg,
            Err(e) => {
                warn!(error = %e, source = ?source, "ConfigChange hook execution failed");
                coco_hooks::orchestration::AggregatedHookResult::default()
            }
        }
    }

    /// Fire ConfigChange hooks for observe-only reload pipelines.
    pub async fn fire_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) {
        let _ = self.run_config_change_hooks(source, file_path).await;
    }

    /// Spawn a tokio task that subscribes to a [`coco_config_reload::ConfigChange`]
    /// stream and fires the corresponding `ConfigChange` hook for each event.
    /// Returns the [`tokio::task::JoinHandle`] so the caller can hold it for
    /// the session lifetime; dropping it aborts the watcher.
    ///
    /// `cancel` lets callers terminate the watcher proactively
    /// (typically the session-level [`Self::cancel`] token); when the
    /// broadcast channel closes (reloader dropped), the loop exits on
    /// its own.
    pub fn spawn_config_change_watcher(
        self: &Arc<Self>,
        mut rx: tokio::sync::broadcast::Receiver<coco_config_reload::ConfigChange>,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = Arc::clone(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    recv = rx.recv() => match recv {
                        Ok(change) => {
                            let source = config_change_source_for_kind(change.kind);
                            let path = change.path.to_string_lossy().into_owned();
                            runtime
                                .fire_config_change_hooks(source, Some(&path))
                                .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(skipped, "ConfigChange watcher lagged; events dropped");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        })
    }

    /// What stays:
    /// - `hook_registry`, `tools`, `client` (and Arc identity), other
    ///   process-level resources — these are correctly cross-session.
    ///
    /// Distinct from `clear_conversation`: that fires SessionEnd /
    /// SessionStart hooks and performs the full `/clear` flow.
    /// This method skips both — SDK `session/archive` is the hook
    /// boundary on its own, not the new session's start.
    pub async fn start_new_session(&self, new_session_id: String) {
        self.flush_session_usage_snapshot().await;
        self.adopt_session_id(&new_session_id).await;
        let usage_tracker = self.load_usage_tracker_for_session(&new_session_id).await;
        *self.session_usage_tracker.lock().await = usage_tracker;
        {
            let mut frs = self.file_read_state.write().await;
            frs.clear();
        }
        self.denial_tracker.lock().await.clear();
        *self.tool_result_replacement_state.write().await =
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX);
        self.reset_cache_break_detectors().await;
    }

    /// Repoint every session-id-keyed subsystem at `new_session_id`.
    ///
    /// Both `start_new_session` (SDK `session/start`) and the full
    /// `/clear` path call this so the swap stays in lockstep:
    /// `runtime.session_id`, `engine_config.session_id`,
    /// `SessionMemoryService` (which also clears its caches),
    /// and the `TranscriptFileHistorySink`'s shared id Arc.
    async fn adopt_session_id(&self, new_session_id: &str) {
        {
            let mut s = self.session_id.write().await;
            *s = new_session_id.to_string();
        }
        write_std_rwlock(&self.orchestration_session_id, new_session_id.to_string());
        let new_id_for_cfg = new_session_id.to_string();
        self.update_engine_config(|cfg| cfg.session_id = new_id_for_cfg)
            .await;
        if let Some(runtime) = &self.memory_runtime {
            runtime.set_session_id(new_session_id.to_string()).await;
        } else if let Some(svc) = &self.session_memory_service {
            svc.set_session_id(new_session_id.to_string()).await;
        }
        if let Some(sink_id) = &self.file_history_sink_session_id
            && let Ok(mut g) = sink_id.write()
        {
            *g = new_session_id.to_string();
        }
        // Refresh `${SESSION_ID}` (and other session-scoped header-template
        // vars) so templated provider headers re-expand against the new id and
        // the affected clients rebuild. Without this, the model-runtime
        // registry keeps the bootstrap id baked into every client and a
        // `/clear` / `/resume` regen sends a stale session id to the gateway.
        if let Err(e) = self.model_runtimes.update_session_id(new_session_id) {
            warn!(error = %e, "failed to refresh model-runtime header vars after session-id change");
        }
    }

    /// Clear cache-break tracking on every registry-owned runtime client.
    /// Called whenever the agent transcript is being reset (new SDK
    /// session, full `/clear`, history-only `/clear`) so the next
    /// outbound prompt establishes a fresh baseline rather than
    /// false-positive-firing against the prior session's snapshot.
    async fn reset_cache_break_detectors(&self) {
        self.model_runtimes.reset_cache_break_detectors().await;
    }

    /// Mutate `engine_config` under lock. Use for mid-session updates
    /// like `SetPermissionMode`.
    pub async fn update_engine_config<F>(&self, f: F)
    where
        F: FnOnce(&mut QueryEngineConfig),
    {
        let snapshot = {
            let mut g = self.engine_config.write().await;
            f(&mut g);
            g.clone()
        };
        write_std_rwlock(&self.orchestration_engine_config, snapshot);
    }

    /// Snapshot the current command registry. Cheap (single Arc clone).
    /// Callers should hold the snapshot for the duration of one
    /// dispatch — a concurrent `/reload-plugins` may swap the inner
    /// Arc, but existing snapshots stay live until dropped.
    pub async fn current_command_registry(&self) -> Arc<CommandRegistry> {
        self.command_registry.read().await.clone()
    }

    /// Rebuild the slash-command registry from disk and atomically
    /// swap it in. Triggered by `/reload-plugins` so the user can pick
    /// up plugin / skill / command edits without restarting the
    /// session. A new `SkillManager` and a freshly resolved enabled plugin
    /// set (`load_enabled_plugins`) are constructed each call; resolution
    /// order matches the original bootstrap
    /// (`commands::build_command_registry`).
    ///
    /// Uses the frozen [`Self::runtime_config`] snapshot — fine for
    /// the user-initiated `/reload-plugins` path where settings
    /// haven't been mutated. Callers that just wrote to
    /// `settings.local.json` must use [`Self::reload_plugins_with`]
    /// to pass the freshly-republished `RuntimeConfig` (otherwise
    /// the registry rebuild reads stale `skill_overrides` tiers).
    ///
    /// Returns the count of registered commands in the new registry
    /// so the caller can show the user a confirmation.
    pub async fn reload_plugins(&self, cwd: &std::path::Path) -> usize {
        self.reload_plugins_with(cwd, &self.runtime_config).await
    }

    /// Variant of [`Self::reload_plugins`] that takes an explicit
    /// `RuntimeConfig`. Use this when the caller has just mutated
    /// settings (e.g. `/skills` dialog save) and the publisher's
    /// `current()` snapshot is fresher than [`Self::runtime_config`].
    pub async fn reload_plugins_with(
        &self,
        cwd: &std::path::Path,
        runtime_config: &coco_config::RuntimeConfig,
    ) -> usize {
        // Reload the LIVE skill manager (the one feeding the model catalog +
        // dispatch), not a throwaway. Build a fresh catalog off disk with the
        // resolved load gates, fold it into `self.skill_manager`, and clear the
        // announcement map so edited skills re-announce.
        let gates = crate::session_bootstrap::resolve_skill_load_gates_with_add_dirs(
            runtime_config,
            cwd,
            &[],
        );
        let fresh = coco_skills::build_session_skill_manager(&self.config_home, cwd, &gates);
        let fresh_skills: Vec<_> = fresh
            .all_including_conditional()
            .into_iter()
            .map(|skill| (*skill).clone())
            .collect();
        self.skill_manager.reload_disk_skills(fresh_skills);
        self.skill_manager.reset_announcements();
        // Unified V2 plugin load (marketplace cache + local inline dirs, gated
        // by enabled_plugins) so a `/reload-plugins` picks up disable/enable and
        // marketplace installs, registering real plugin-command bodies.
        let plugins = coco_plugins::load_enabled_plugins(&self.config_home, cwd);
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
        for skill in coco_plugins::skill_bridge::load_all_plugin_skills_v2(&plugin_refs) {
            self.skill_manager.register(skill);
        }
        // Builtin plugin skills — symmetric with bootstrap so a reload
        // re-registers them too.
        coco_plugins::builtins::init_builtin_plugins();
        for skill in coco_plugins::builtin_plugin_skills(&self.config_home) {
            self.skill_manager.register(skill);
        }
        let registry = coco_commands::build_command_registry(
            &self.skill_manager,
            &plugins,
            coco_types::UserType::from_env(),
            runtime_config.features.clone(),
            cwd.to_path_buf(),
            dirs::home_dir().unwrap_or_else(|| cwd.to_path_buf()),
            None,
            &runtime_config.skill_overrides,
        );
        let count = registry.len();
        let new_registry = Arc::new(registry);
        {
            let mut slot = self.command_registry.write().await;
            *slot = new_registry;
        }
        // Re-register plugin MCP servers with the live manager (if attached) so a
        // reload picks up newly enabled/disabled plugin MCP contributions.
        // No-op without a manager (e.g. the TUI today).
        let _ = self.reload_plugin_mcp_servers(cwd).await;
        count
    }

    /// Reload the live `HookRegistry` from the latest `RuntimeConfig`
    /// snapshot (settings + plugin hooks). Triggered by `/hooks reload`.
    ///
    /// Atomic semantics:
    /// - Settings hooks (User/Project/Local/Flag/Policy scopes) and
    ///   plugin hooks are both rebuilt.
    /// - `fired_once` set is **preserved** so a `once` hook that
    ///   already fired this session doesn't re-fire after reload.
    /// - Per-agent `agent_scoped` hook layer is **preserved** — those are
    ///   owned by the coordinator's spawn lifecycle, not by settings.
    /// - Slash commands run only at turn boundaries (the dispatch loop
    ///   in `tui_runner` `drain_active_turn`s before invoking them),
    ///   so PreToolUse/PostToolUse for an in-flight call cannot see
    ///   different hook sets.
    ///
    /// Returns the count of hooks now registered.
    pub async fn reload_hooks(&self) -> Result<usize> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| self.config_home.clone());
        let policy = coco_hooks::LoaderPolicy {
            disable_all_hooks: self.runtime_config.settings.merged.disable_all_hooks,
            allow_managed_hooks_only: self.runtime_config.settings.merged.allow_managed_hooks_only,
        };

        // Build (scope, value) pairs for every active settings source.
        // Plugin hooks are layered separately because they live on
        // disk inside plugin directories, not in settings.json.
        let mut sources: Vec<(coco_types::HookScope, serde_json::Value)> = Vec::new();
        for source in [
            coco_config::SettingSource::User,
            coco_config::SettingSource::Project,
            coco_config::SettingSource::Local,
            coco_config::SettingSource::Flag,
            coco_config::SettingSource::Policy,
        ] {
            let Some(value) = self.runtime_config.settings.per_source.get(&source) else {
                continue;
            };
            let Some(hooks_value) = value.get("hooks") else {
                continue;
            };
            let scope = match source {
                coco_config::SettingSource::User => coco_types::HookScope::User,
                coco_config::SettingSource::Project => coco_types::HookScope::Project,
                coco_config::SettingSource::Local => coco_types::HookScope::Local,
                coco_config::SettingSource::Flag => coco_types::HookScope::Local,
                coco_config::SettingSource::Policy => coco_types::HookScope::Policy,
                coco_config::SettingSource::Plugin => coco_types::HookScope::Plugin,
            };
            sources.push((scope, hooks_value.clone()));
        }

        // Atomic settings-hook swap.
        let settings_count = self
            .hook_registry
            .reload_from_runtime(&sources, policy)
            .map_err(|e| anyhow::anyhow!("hook reload failed: {e}"))?;

        // Re-layer plugin hooks on top — they aren't in settings.json
        // so `reload_from_runtime` doesn't see them. Unified V2 source.
        let plugins = coco_plugins::load_enabled_plugins(&self.config_home, &cwd);
        if !plugins.is_empty() {
            let refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
            coco_plugins::hook_bridge::register_plugin_hooks_v2(&self.hook_registry, &refs);
        }

        Ok(self.hook_registry.len().max(settings_count))
    }

    /// Full `/clear` reset: SessionEnd hooks → drop subsystem caches →
    /// regen session id → SessionStart hooks (whose result messages seed
    /// the new transcript).
    ///
    /// `/clear` has one behavior: full reset, regardless of arguments.
    pub async fn clear_conversation(&self) -> Result<()> {
        // Step 1: SessionEnd hooks fire BEFORE the reset, with the bounded
        // SESSION_END timeout (1.5s default;
        // `COCO_SESSIONEND_HOOKS_TIMEOUT_MS` overrides).
        let cur_session_id = self.current_session_id().await;
        let cfg = self.current_engine_config().await;
        let pre_ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id: cur_session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            // SessionEnd doesn't surface as a reminder, so no sink needed here.
            sync_event_sink: None,
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        if let Err(e) = coco_hooks::orchestration::execute_session_end(
            &self.hook_registry,
            &pre_ctx,
            coco_hooks::orchestration::ExitReason::Clear,
        )
        .await
        {
            warn!(error = %e, "SessionEnd hook execution failed during /clear");
        }

        // Step 2: reset session caches. `/clear` clears the CONVERSATION, not
        // the session's permission grants — preserve the current live
        // permission base (mode + allow/deny/ask + dirs, incl. mid-session
        // approvals and `/add-dir`) and reset only the plan-mode latches /
        // todos / snapshots to Default.
        {
            let mut guard = self.app_state.write().await;
            let preserved = std::mem::take(&mut guard.permissions);
            *guard = ToolAppState::default();
            guard.permissions = preserved;
        }
        self.reset_cache_break_detectors().await;
        // Drop the captured post-turn cache-safe-params handle. Otherwise a
        // `/btw` issued between this `/clear` and the first post-clear turn
        // would read the dropped pre-clear engine's slot (still holding the
        // discarded conversation in `fork_context_messages`) and fork against
        // content the user just cleared. Nulling it makes `last_cache_safe_params`
        // return `None`, so `/btw` shows the "no parent turn yet" hint until a
        // fresh turn repopulates it. Matches the field/getter doc contract.
        *self.last_engine_cache_handle.write().await = None;
        // Clear the per-agent skill announcement map so every skill
        // re-announces in the post-clear transcript's first listing pass.
        self.skill_manager.reset_announcements();

        // Drop any queued steering messages — `/clear` semantically
        // says "fresh start", and a queued prompt from the pre-clear
        // session would otherwise surface as the first user input in
        // the post-clear transcript.
        self.command_queue.clear().await;

        // Step 3: full reset.
        let cur_session_id = self.current_session_id().await;
        coco_context::clear_plan_slug(&cur_session_id);
        {
            let mut frs = self.file_read_state.write().await;
            frs.clear();
        }
        if let Some(fh) = &self.file_history {
            let mut fh = fh.write().await;
            *fh = FileHistoryState::default();
        }
        if let Some(svc) = &self.session_memory_service {
            svc.set_last_summarized_message_id(None).await;
        }

        // Reset the auto-memory runtime's per-conversation state — recall
        // PrefetchState, extraction cursor + throttle counter, and the
        // session-memory init flag. The on-disk MEMORY.md and topic
        // files are deliberately left alone; they're cross-conversation
        // memory that a /clear is not meant to wipe.
        if let Some(runtime) = &self.memory_runtime {
            runtime.reset().await;
        }

        // Step 4: regenerate the session id and propagate it to every
        // id-keyed subsystem. Without this, post-clear writes would land in
        // the OLD session's directory and surface as "extra memory" /
        // "phantom file-history snapshots" on the next `--resume` of the
        // pre-clear session.
        self.flush_session_usage_snapshot().await;
        let new_session_id = uuid::Uuid::new_v4().to_string();
        self.adopt_session_id(&new_session_id).await;
        *self.session_usage_tracker.lock().await = CostTracker::new();
        // Reset the transcript dedup set so the new session writes a
        // fresh JSONL from message #1 — without this, the post-clear
        // turn would skip persisting any UUID that happened to match
        // a pre-clear message (impossible in practice, but the empty
        // set is the correct invariant).
        self.transcript_dedup.lock().await.clear();
        *self.tool_result_replacement_state.write().await =
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX);

        // Step 5: SessionStart hooks. Result messages seed the post-clear transcript.
        let cfg = self.current_engine_config().await;
        let post_ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id: new_session_id,
            cwd: std::env::current_dir().unwrap_or_default(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            // Surface SessionStart hook output as `hook_*` reminders on
            // the next turn.
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        let model_arg = if cfg.model_id.is_empty() {
            None
        } else {
            Some(cfg.model_id.as_str())
        };
        // Clear the in-memory transcript before invoking SessionStart
        // hooks. The hook output flows into the sync hook buffer
        // (`post_ctx.sync_event_sink`) and surfaces as `hook_*`
        // reminders on the next turn.
        {
            let mut h = self.history.lock().await;
            h.clear();
        }
        if let Err(e) = coco_hooks::orchestration::execute_session_start(
            &self.hook_registry,
            &post_ctx,
            coco_hooks::orchestration::SessionStartSource::Clear,
            /*agent_type*/ None,
            model_arg,
        )
        .await
        {
            warn!(error = %e, "SessionStart hook execution failed during /clear");
        }

        Ok(())
    }
}

/// Construct an `Arc<SandboxState>` for the active session, or return `None`
/// when sandbox is disabled / unavailable.
///
/// Returns `Ok(None)` when:
/// - `Feature::Sandbox` is off, or
/// - mode is `FullAccess`, or
/// - bootstrap gates fail AND `sandbox.fail_if_unavailable` is `false`
///   (commands will run unsandboxed; user gets a startup banner).
///
/// Returns `Err` when bootstrap gates fail AND
/// `sandbox.fail_if_unavailable` is `true` — the caller propagates this
/// so coco exits before the REPL starts.
pub(crate) async fn build_sandbox_state(
    runtime_config: &RuntimeConfig,
    cwd: &std::path::Path,
) -> anyhow::Result<Option<Arc<coco_sandbox::SandboxState>>> {
    use coco_sandbox::adapter::AdapterInputs;

    if !runtime_config
        .features
        .enabled(coco_types::Feature::Sandbox)
    {
        return Ok(None);
    }

    let mode = runtime_config.sandbox.mode;
    if matches!(mode, coco_types::SandboxMode::FullAccess) {
        return Ok(None);
    }

    // Mark `enabled = true` because reaching this point already implies the
    // feature gate passed and the user requested an enforcing mode.
    let mut sandbox_settings = runtime_config.sandbox.clone();
    sandbox_settings.enabled = true;

    let gate = coco_sandbox::check_enable_gates(&sandbox_settings);
    if !matches!(gate, coco_sandbox::EnableCheckResult::Enabled) {
        // Surface a startup banner via `sandbox_unavailable_reason` so the
        // user understands *why* sandboxing is degraded. When
        // `fail_if_unavailable` is set, this is a hard error.
        let missing_deps: Vec<String> = match &gate {
            coco_sandbox::EnableCheckResult::DisabledByMissingDeps { missing } => missing.clone(),
            _ => Vec::new(),
        };
        let reason = coco_sandbox::sandbox_unavailable_reason(
            &sandbox_settings,
            coco_sandbox::current_platform_supported(),
            sandbox_settings.is_platform_enabled(),
            &missing_deps,
        );

        if sandbox_settings.fail_if_unavailable {
            let detail = reason.unwrap_or_else(|| format!("sandbox bootstrap failed: {gate:?}"));
            return Err(anyhow::anyhow!(
                "sandbox.fail_if_unavailable is set but sandbox cannot start: {detail}"
            ));
        }

        if let Some(banner) = reason {
            // stderr so the message survives any TUI redirection.
            eprintln!("[coco] sandbox unavailable: {banner}");
            warn!(?gate, banner, "sandbox enabled but runtime cannot start");
        } else {
            warn!(?gate, "sandbox enabled but runtime cannot start");
        }
        return Ok(None);
    }

    let settings_root = runtime_config
        .paths
        .project_dir
        .clone()
        .unwrap_or_else(|| cwd.to_path_buf());

    let permission_allow_rules: Vec<String> =
        runtime_config.settings.merged.permissions.allow.clone();
    let permission_deny_rules: Vec<String> =
        runtime_config.settings.merged.permissions.deny.clone();
    let additional_directories: Vec<PathBuf> = runtime_config
        .settings
        .merged
        .permissions
        .additional_directories
        .iter()
        .map(PathBuf::from)
        .collect();

    let coco_temp_dir = std::env::temp_dir().join("coco");
    let worktree = coco_sandbox::detect_worktree_main_repo(cwd);

    // Per-source rule plumbing — drives the `allow_managed_*_only`
    // gates. The adapter needs source provenance because the merged
    // `SandboxSettings` collapses every layer; only allow rules need
    // sourcing (deny rules apply uniformly regardless of source).
    // The sandbox adapter only consumes allow-source provenance today
    // (deny rules apply uniformly regardless of source). `_ask` is
    // ignored here; the ask map is consumed at the engine config layer
    // via `permission_rule_loader::typed_permission_rules`.
    let (sourced_allow_rules, _sourced_deny_rules, _sourced_ask_rules) =
        runtime_config.settings.sourced_permission_rules();
    let sourced_fs_allow_read = runtime_config.settings.sourced_filesystem_allow_read();

    // Deny writes to every settings source so a sandboxed command can't edit
    // its own permission rules (or disable the sandbox), plus the `.coco/`
    // command/agent definitions.
    let settings_files = sandbox_settings_deny_paths(&settings_root);

    let inputs = AdapterInputs {
        settings: &sandbox_settings,
        mode,
        settings_root: &settings_root,
        original_cwd: cwd,
        current_cwd: cwd,
        permission_allow_rules: &permission_allow_rules,
        permission_deny_rules: &permission_deny_rules,
        additional_directories: &additional_directories,
        coco_temp_dir: &coco_temp_dir,
        settings_files: &settings_files,
        worktree_main_repo: worktree.as_deref(),
        sourced_permission_allow_rules: Some(&sourced_allow_rules),
        sourced_filesystem_allow_read: Some(&sourced_fs_allow_read),
    };
    let out = coco_sandbox::build_runtime_config(inputs);
    // `allow_network == false` means network is isolated (the safe default once
    // the sandbox is enabled, unless the coarse `sandbox.allow_network` toggle
    // is set, or the mode is `FullAccess`).
    let network_isolated = !out.config.allow_network;

    let platform = coco_sandbox::platform::create_platform();
    let state = match mode {
        coco_types::SandboxMode::ExternalSandbox => {
            coco_sandbox::SandboxState::external(out.enforcement, out.settings, out.config)
        }
        _ => coco_sandbox::SandboxState::new(out.enforcement, out.settings, out.config, platform),
    };
    let state = Arc::new(state);

    if network_isolated {
        // Start the egress proxy so the `DomainFilter` enforces deny-by-default
        // per-domain filtering. macOS reaches the proxy over loopback directly;
        // Linux runs inside a `--unshare-net` namespace and needs the netns
        // socat bridge (requires `socat`) to forward egress through the proxy.
        // On any failure / missing socat the posture stays fail-closed (network
        // blocked) — never unrestricted egress. The coarse
        // `sandbox.allow_network` toggle opts back into full network.
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = state.start_network_proxy().await {
                warn!(error = %e, "sandbox egress proxy failed to start; network is blocked this session");
            }
        }
        #[cfg(target_os = "linux")]
        {
            let tag = state.session_tag().to_string();
            match coco_sandbox::deps::socat_path() {
                Some(socat) => {
                    if let Err(e) = state.start_network_proxy_with_bridge(socat, &tag).await {
                        warn!(error = %e, "sandbox network bridge failed to start; network is blocked this session");
                    }
                }
                None => warn!(
                    "socat not found — install socat (apt install socat) to enable \
                     per-domain network filtering; network is blocked this session"
                ),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            warn!(
                "sandbox network isolation blocks ALL egress on this platform — \
                 per-domain filtering is unsupported; set sandbox.allow_network = true \
                 to allow network"
            );
        }
    }

    Ok(Some(state))
}

/// Settings / definition files denied write inside the sandbox, so a sandboxed
/// command cannot rewrite its own permission rules, disable the sandbox, or
/// inject agents.
///
/// MUST be the single source for both the bootstrap (`build_sandbox_state`) and
/// the hot-reload (`sandbox_reload::reapply_sandbox`) paths — passing `&[]` on
/// reload silently re-opened the self-permission escape after the first
/// settings change.
pub(crate) fn sandbox_settings_deny_paths(settings_root: &std::path::Path) -> Vec<PathBuf> {
    use coco_config::global_config;
    let managed = global_config::managed_settings_path();
    let mut paths = vec![
        global_config::user_settings_path(),
        global_config::project_settings_path(settings_root),
        global_config::local_settings_path(settings_root),
        managed.clone(),
        global_config::global_config_path(),
        settings_root.join(".coco").join("agents"),
    ];
    // Managed drop-in directory (`managed-settings.d`) next to the managed
    // settings file — deny the whole drop-in dir, not just the .json.
    if let Some(dir) = managed.parent() {
        paths.push(dir.join("managed-settings.d"));
    }
    paths
}

#[cfg(test)]
#[path = "session_runtime.test.rs"]
mod tests;
