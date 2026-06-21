//! Shared session bootstrap helpers.
//!
//! TUI (`tui_runner::run_tui`) and SDK (`run_sdk_mode`) both build
//! the same per-session subsystems before handing off to their
//! respective event loops. This module owns the assembly logic so
//! the two runners cannot drift apart on what gets installed.
//!
//! Two stages:
//! 1. [`build_engine_resources`] resolves the model client, tool
//!    registry, and system prompt from
//!    the already-built [`coco_config::RuntimeConfig`].
//! 2. [`install_session_late_binds`] performs the post-`SessionRuntime::build`
//!    wirings (task runtime, agent transcript store, agent-team
//!    handle, fork dispatcher). The MCP handle is intentionally
//!    caller-driven because SDK constructs an `McpConnectionManager`
//!    that the dispatch handlers also need to mutate.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use coco_commands::CommandRegistry;
use coco_commands::build_command_registry;
use coco_config::RuntimeConfig;
use coco_config::global_config;
use coco_tool_runtime::ToolRegistry;
use coco_types::ProviderApi;
use coco_types::UserType;

use crate::Cli;
use crate::headless::StartupPermissionState;
use crate::headless::build_output_style_manager;
use crate::headless::build_system_prompt_for_model;
use crate::headless::resolve_additional_dirs;
use crate::headless::resolve_additional_dirs_display;
use crate::headless::resolve_main_model;
use crate::headless::resolve_startup_permission_state;
use crate::session_runtime::SessionRuntime;

/// Resources produced by [`build_engine_resources`]. Caller threads
/// these through into [`crate::session_runtime::SessionRuntimeBuildOpts`].
pub struct EngineResources {
    pub tools: Arc<ToolRegistry>,
    pub system_prompt: String,
    pub model_id: String,
    pub provider_api: Option<ProviderApi>,
    pub startup: StartupPermissionState,
    /// Slash-command registry built once with the full load order
    /// (builtins → extended → skills → plugin contributions → P1 handlers).
    /// Both the SDK `initialize.commands` advertisement and the TUI
    /// `dispatch_slash_command` chain resolve through this slot. Wrapped in
    /// `RwLock` so `/reload-plugins` can hot-swap the inner
    /// `Arc<CommandRegistry>` without rebuilding the session — consumers
    /// snapshot the inner `Arc` once per dispatch (see
    /// [`crate::session_runtime::SessionRuntime::current_command_registry`]).
    pub command_registry: Arc<tokio::sync::RwLock<Arc<CommandRegistry>>>,
    /// Session-scoped `SkillManager`. Hoisted out of
    /// [`build_session_command_registry`] so the same instance is
    /// shared with the per-engine reminder pipeline (`SkillsSource`)
    /// instead of being dropped after building the command registry.
    pub skill_manager: Arc<coco_skills::SkillManager>,
    /// Resolved output-style catalog + active style. The CLI threads
    /// this into:
    /// - [`Self::system_prompt`] (already injected at build time)
    /// - `SessionBootstrap.output_style` (name only, for SDK init +
    ///   the per-turn reminder generator)
    /// - `CliInitializeBootstrap.{output_style, available_styles}`
    ///
    /// Stored here so SDK / TUI bootstraps don't each re-build it.
    pub output_style_manager: coco_output_styles::OutputStyleManager,
}

/// Build the shared engine resources from a resolved `RuntimeConfig`.
///
/// The `RuntimeConfig` itself is caller-built — TUI typically
/// snapshots it from a hot-reload publisher, SDK / headless build it
/// once via [`crate::headless::build_runtime_config_for_cli`].
/// Build a real LSP handle when `Feature::Lsp` is enabled and the
/// session has a workspace root + coco-home to load
/// `lsp_servers.json` from. Returns `None` otherwise — callers thread
/// the `None` into [`install_session_late_binds`] so the runtime's LSP
/// slot stays empty and `LspTool` is hidden from the model.
///
/// The constructed `LspServerManager` is wrapped in
/// [`crate::lsp_handle_adapter::LspManagerAdapter`] so file mutation
/// tools (Write / Edit / NotebookEdit) can dispatch `didSave` +
/// `clearDeliveredDiagnosticsForFile` through `ctx.lsp.notify_save`.
///
/// The adapter is **prewarmed** before returning so `LSPTool.isEnabled`
/// reads accurate running-state by turn 1. Prewarm is best-effort —
/// servers that fail to spawn just flip the adapter's `is_connected`
/// gate to `false`, hiding the tool cleanly instead of throwing on the
/// first call.
/// Fire-and-forget startup marketplace maintenance, shared by the TUI,
/// headless, and SDK entry points. Ensures the official marketplace,
/// registers seed marketplaces (`COCO_PLUGIN_SEED_DIR`), reconciles
/// declared `extraKnownMarketplaces`, then uninstalls plugins that were
/// delisted from their marketplace.
///
/// Runs on every surface (not just the interactive TUI) so delisting +
/// seed-marketplace enforcement applies to `coco --print` / `chat` / `review`
/// and SDK NDJSON sessions too. Non-fatal and never blocks startup: the
/// official ensure runs first so freshly-cloned manifests are visible to the
/// delisting diff.
pub fn spawn_marketplace_startup(config_home: std::path::PathBuf) {
    tokio::spawn(async move {
        let plugins_dir = config_home.join("plugins");
        let outcome = coco_plugins::official::ensure_official_marketplace(plugins_dir).await;
        tracing::debug!(?outcome, "official marketplace auto-install");
        let delisted = coco_plugins::run_marketplace_startup(&config_home).await;
        if !delisted.is_empty() {
            tracing::info!(
                target: "coco::plugins",
                ?delisted,
                "uninstalled plugins delisted from their marketplace"
            );
        }
    });
}

pub async fn build_lsp_handle_if_enabled(
    runtime_config: &RuntimeConfig,
    coco_home: &Path,
    cwd: &Path,
) -> Option<coco_tool_runtime::LspHandleRef> {
    if !runtime_config.features.enabled(coco_types::Feature::Lsp) {
        return None;
    }
    let manager = coco_lsp::create_manager(Some(coco_home), Some(cwd.to_path_buf()));
    let adapter = crate::lsp_handle_adapter::LspManagerAdapter::new(manager);
    // Merge plugin-contributed LSP servers before prewarm so they spawn
    // eagerly alongside disk-configured servers.
    let plugins = coco_plugins::load_enabled_plugins(coco_home, cwd);
    let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
    adapter
        .merge_plugin_servers(coco_plugins::lsp_bridge::extract_lsp_servers_from_plugins(
            &plugin_refs,
        ))
        .await;
    adapter.prewarm(cwd).await;
    Some(Arc::new(adapter))
}

pub fn build_engine_resources(
    cli: &Cli,
    runtime_config: &RuntimeConfig,
    cwd: &Path,
) -> Result<EngineResources> {
    tracing::info!(
        target: "coco_cli::session_bootstrap",
        cwd = %cwd.display(),
        "building engine resources"
    );

    let main_model = resolve_main_model(runtime_config);
    let provider_api = main_model.provider_api;
    let model_id = main_model.model_id.clone();

    let registry = ToolRegistry::new();
    coco_tools::register_all_tools(&registry);
    let tool_count = registry.len();
    let tools = Arc::new(registry);

    // Load the session's plugin set once, then reuse it for output-style,
    // command, skill, and hook registration.
    let plugins = load_session_plugins(cwd);

    // Resolve the active output style up front: it shapes the system
    // prompt cache prefix and surfaces on the SDK init message + the
    // per-turn reminder generator. Plugin-contributed styles are folded in.
    let plugin_style_sources = plugin_output_style_sources(&plugins);
    let output_style_manager =
        build_output_style_manager(runtime_config, cwd, &plugin_style_sources);

    // `--add-dir` flows into the env block. Single source of truth lives
    // in `headless::resolve_additional_dirs_display`.
    let additional_working_directories = resolve_additional_dirs_display(cli, cwd);

    let system_prompt = build_system_prompt_for_model(
        cwd,
        runtime_config,
        &main_model.provider,
        &model_id,
        output_style_manager.active(),
        &additional_working_directories,
    );

    let startup = resolve_startup_permission_state(cli, &runtime_config.settings.merged)?;

    let (command_registry, skill_manager) =
        build_session_command_registry(cli, runtime_config, cwd, &plugins);
    let command_count = command_registry.len();
    let skill_count = skill_manager.len();

    tracing::info!(
        target: "coco_cli::session_bootstrap",
        provider = main_model.provider,
        model_id = %model_id,
        real_provider = provider_api.is_some(),
        fallback_count = runtime_config.model_roles.fallbacks(coco_types::ModelRole::Main).len(),
        fallback_policy_set = runtime_config
            .model_roles
            .policy(coco_types::ModelRole::Main)
            .is_some(),
        tool_count,
        command_count,
        skill_count,
        permission_mode = ?startup.mode,
        bypass_available = startup.bypass_available,
        sandbox_mode = ?runtime_config.sandbox.mode,
        system_prompt_chars = system_prompt.len(),
        "engine resources built"
    );
    tracing::debug!(
        target: "coco_cli::session_bootstrap",
        max_turns = ?runtime_config.loop_config.max_turns,
        total_token_budget = ?runtime_config.loop_config.total_token_budget,
        streaming_tools = runtime_config.loop_config.enable_streaming_tools,
        auto_compact_enabled = runtime_config.compact.auto.enabled,
        auto_compact_disabled_by_env = runtime_config.compact.auto.auto_disabled_by_env,
        memory_extraction = runtime_config.memory.extraction_enabled,
        web_fetch_enabled = runtime_config.features.enabled(coco_types::Feature::WebFetch),
        web_search_enabled = runtime_config.features.enabled(coco_types::Feature::WebSearch),
        retrieval_enabled = runtime_config.features.enabled(coco_types::Feature::Retrieval),
        "runtime config public knobs"
    );

    Ok(EngineResources {
        tools,
        system_prompt,
        model_id,
        provider_api,
        startup,
        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(command_registry))),
        skill_manager,
        output_style_manager,
    })
}

/// Construct the slash-command registry (builtins → extended → skills →
/// plugin contributions → P1 handlers) AND return the `SkillManager`
/// Arc that backed the skill-derived commands. The caller threads the
/// manager into `SessionRuntime` so the per-turn reminder pipeline's
/// `SkillsSource` reads the same in-memory catalog.
pub(crate) fn build_session_command_registry(
    cli: &Cli,
    runtime_config: &RuntimeConfig,
    cwd: &Path,
    plugins: &[coco_plugins::loader::LoadedPluginV2],
) -> (CommandRegistry, Arc<coco_skills::SkillManager>) {
    let config_home = global_config::config_home();

    let gates = resolve_skill_load_gates(cli, runtime_config, cwd);
    let skill_manager = Arc::new(coco_skills::build_session_skill_manager(
        &config_home,
        cwd,
        &gates,
    ));

    // Plugin-contributed skills (namespaced `plugin:skill`) into the live
    // SkillManager so the model catalog + dispatch see them.
    let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
    for skill in coco_plugins::skill_bridge::load_all_plugin_skills_v2(&plugin_refs) {
        skill_manager.register(skill);
    }

    // Builtin (compiled-in) plugins: seed the registry once, then register any
    // enabled builtin skills. No-op until a builtin is registered in
    // `init_builtin_plugins`.
    coco_plugins::builtins::init_builtin_plugins();
    for skill in coco_plugins::builtin_plugin_skills(&config_home) {
        skill_manager.register(skill);
    }

    let registry = build_command_registry(
        skill_manager.as_ref(),
        plugins,
        UserType::from_env(),
        runtime_config.features.clone(),
        cwd.to_path_buf(),
        dirs::home_dir().unwrap_or_else(|| cwd.to_path_buf()),
        /*managed_root*/ None,
        &runtime_config.skill_overrides,
    );
    (registry, skill_manager)
}

/// Load the active plugin set for this session once: marketplace versioned
/// cache + local `inline` dirs, gated by settings.json `enabled_plugins`.
/// Shared by the output-style, command, skill, and hook registration paths so
/// a session loads plugins exactly once.
pub(crate) fn load_session_plugins(cwd: &Path) -> Vec<coco_plugins::loader::LoadedPluginV2> {
    coco_plugins::load_enabled_plugins(&global_config::config_home(), cwd)
}

/// Derive the plugin output-style sources from a loaded plugin set (default
/// `<plugin>/output-styles/` dir + manifest `output_styles` extras). Fed into
/// [`build_output_style_manager`] so plugin-contributed styles surface
/// alongside user / project / managed styles.
pub(crate) fn plugin_output_style_sources(
    plugins: &[coco_plugins::loader::LoadedPluginV2],
) -> Vec<coco_output_styles::PluginOutputStyleSource> {
    plugins
        .iter()
        .map(coco_output_styles::PluginOutputStyleSource::from_loaded_plugin)
        .collect()
}

/// Resolve [`coco_skills::SkillLoadGates`] from the resolved `RuntimeConfig`,
/// the `--setting-sources` set, the `strictPluginOnlyCustomization` policy
/// (`skills` surface), `--add-dir`, and `COCO_DISABLE_POLICY_SKILLS`.
/// Applies `isSettingSourceEnabled(...) && !skillsLocked` guards plus the
/// managed-skill env gate.
pub(crate) fn resolve_skill_load_gates(
    cli: &Cli,
    runtime_config: &RuntimeConfig,
    cwd: &Path,
) -> coco_skills::SkillLoadGates {
    resolve_skill_load_gates_with_add_dirs(runtime_config, cwd, &resolve_additional_dirs(cli, cwd))
}

/// `cli`-free variant for reload paths (`reload_plugins_with`) that don't
/// retain the `Cli`. `cli_add_dirs` are the resolved `--add-dir` roots ( `&[]`
/// when unavailable); settings `permissions.additionalDirectories` are always
/// folded in from `RuntimeConfig`.
pub(crate) fn resolve_skill_load_gates_with_add_dirs(
    runtime_config: &RuntimeConfig,
    cwd: &Path,
    cli_add_dirs: &[std::path::PathBuf],
) -> coco_skills::SkillLoadGates {
    use coco_config::SettingSource;
    use coco_config::env::EnvKey;

    let enabled = &runtime_config.enabled_setting_sources;
    let skills_locked = runtime_config
        .settings
        .merged
        .strict_plugin_only_customization
        .is_restricted_to_plugin_only("skills");

    // Managed/policy skills load unless explicitly disabled via env.
    let managed_enabled =
        !coco_config::env::env_truthy_opt(EnvKey::CocoDisablePolicySkills.as_str())
            .unwrap_or(false);

    let user_enabled = enabled.contains(&SettingSource::User);
    let project_enabled = enabled.contains(&SettingSource::Project);

    // `--add-dir` plus settings `permissions.additionalDirectories`, resolved
    // to `.coco/skills` roots in `build_session_skill_manager`.
    let mut additional_dirs = cli_add_dirs.to_vec();
    for dir in &runtime_config
        .settings
        .merged
        .permissions
        .additional_directories
    {
        let p = Path::new(dir);
        additional_dirs.push(if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        });
    }

    coco_skills::SkillLoadGates {
        managed_enabled,
        user_enabled,
        project_enabled,
        legacy_enabled: project_enabled,
        additional_dirs_enabled: project_enabled,
        additional_dirs,
        skills_locked,
    }
}

/// Install the post-`SessionRuntime::build` late-binds shared by TUI
/// and SDK. Without this both runners must independently remember to
/// attach `task_runtime`, `agent_transcript_store`, the agent-team
/// wiring, and the fork dispatcher — TUI used to forget all four,
/// causing background AgentTool, transcript resume, and `/btw` to
/// silently degrade to no-ops.
///
/// `mcp_handle` is optional because TUI does not yet bootstrap an
/// `McpConnectionManager`. SDK passes `Some(handle)` and gets the
/// original install ordering preserved (mcp before agent-team).
///
/// `lsp_handle` is optional and **independently gated** by
/// [`coco_types::Feature::Lsp`] at the caller (CLI / SDK / TUI). When
/// `None`, the runtime's LSP slot stays unset and
/// `LspTool::is_enabled()` reports `false` (via `NoOpLspHandle`), so
/// the tool is hidden from the model's tool list.
///
/// `event_sink` is the long-lived `CoreEvent` sender the surface
/// consumes. Wired into the `TaskManager` so background-task and
/// subagent lifecycle events (`TaskStarted` / `TaskProgress` /
/// `TaskCompleted`) reach the TUI — without it the manager drops them
/// silently and the subagent activity panel never populates. TUI passes
/// `Some(notification_tx.clone())`; SDK/headless pass `None` (they build
/// their `CoreEvent` channel per-turn after this point).
pub async fn install_session_late_binds(
    runtime: Arc<SessionRuntime>,
    cwd: &Path,
    mcp_handle: Option<coco_tool_runtime::McpHandleRef>,
    lsp_handle: Option<coco_tool_runtime::LspHandleRef>,
    event_sink: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
) -> Result<()> {
    // Background task runtime — owns the `TaskManager` and per-task
    // disk output; shared with `SwarmAgentHandle` so AgentTool
    // background spawns and the engine's `Task*` tools see one
    // source of truth.
    //
    // Disk-output session dir: `<config_home>/cache/tasks/<session_id>/`.
    // Captured ONCE here so subsequent `/clear` regenerations don't
    // invalidate paths held by in-flight `DiskTaskOutput` instances.
    let task_session_id = runtime.current_session_id().await;
    let task_session_dir = coco_config::global_config::config_home()
        .join("cache")
        .join("tasks")
        .join(&task_session_id);
    // Wire the session-scoped `CommandQueue` into the TaskRuntime
    // via the `NotificationSink` trait so terminal lifecycle events
    // (mark_completed / mark_failed / kill_task / bg shell exit)
    // push a `<task-notification>` envelope onto the queue. The
    // engine's per-turn drain
    // (`engine_finalize_turn::drain_command_queue_into_history`)
    // then injects it as a User message wrapped in `<system-reminder>`.
    let sink: coco_tasks::NotificationSinkRef = Arc::new(
        crate::command_queue_sink::CommandQueueNotificationSink::new(
            runtime.command_queue().clone(),
        ),
    );
    // Wire the surface's long-lived `CoreEvent` sink into the manager so
    // `TaskStarted` / `TaskProgress` / `TaskCompleted` reach the TUI. Absent
    // a sink the manager short-circuits every emit (`emit_progress` bails at
    // the no-sink branch), starving the subagent activity panel.
    let mut task_manager = coco_tasks::TaskManager::new();
    if let Some(event_sink) = event_sink {
        task_manager = task_manager.with_event_sink(event_sink);
    }
    let task_runtime = Arc::new(
        crate::task_runtime::TaskRuntime::with_session_dir(
            Arc::new(task_manager),
            task_session_dir,
        )
        .with_notification_sink(sink),
    );
    runtime.attach_task_runtime(task_runtime).await;
    let task_list_id = coco_tasks::resolve_task_list_id(None, None, &task_session_id);
    let task_list_root = coco_config::global_config::config_home().join("tasks");
    let task_list_router =
        crate::team_task_list_router::RoutedTaskList::open(task_list_root, task_list_id)?;
    runtime
        .attach_task_list(task_list_router.clone() as coco_tool_runtime::TaskListHandleRef)
        .await;
    runtime
        .attach_team_task_list_router(task_list_router as coco_tool_runtime::TeamTaskListRouterRef)
        .await;

    // Per-agent transcript persistence. The project paths match the
    // runtime's transcript path so
    // `<project_dir>/<session_id>/subagents/agent-<id>.*` lives
    // alongside the main session JSONL. Skipped under
    // `--no-session-persistence` so a print run that spawns subagents
    // writes no subagent JSONL.
    if runtime.persist_session() {
        let agent_transcript_store: Arc<dyn coco_tool_runtime::AgentTranscriptStore> = Arc::new(
            crate::agent_transcript_persistence::SessionAgentTranscriptStore::new(Arc::new(
                coco_session::TranscriptStore::new(crate::paths::project_paths(cwd)),
            )),
        );
        runtime
            .attach_agent_transcript_store(agent_transcript_store)
            .await;
    }

    // MCP handle (if any). Installed BEFORE `install_agent_team` so
    // AgentTool's prompt-time MCP filter sees a populated handle on
    // the very first engine build, matching the original SDK ordering.
    if let Some(handle) = mcp_handle {
        runtime.attach_mcp_handle(handle).await;
    }

    // LSP handle install: same pattern as MCP. When the caller did the
    // `Feature::Lsp` gate + manager construction (CLI / SDK), the
    // handle threads in here so per-turn engines pick it up via
    // `wire_engine`. TUI passes `None` (no LSP boot yet).
    if let Some(handle) = lsp_handle {
        runtime.attach_lsp_handle(handle).await;
    }

    // Agent wiring (`SwarmAgentHandle` + `QueryEngineAdapter`
    // factory). LocalAgent requires the TaskRuntime registry installed
    // above; team-specific tools remain feature-gated at the tool layer.
    crate::agent_handle_factory::install_agent_team(runtime.clone(), cwd.display().to_string())
        .await?;

    // Post-turn fork dispatcher (`/btw`, `promptSuggestion`). Captures
    // `Arc<SessionRuntime>` and routes every dispatch through
    // `build_engine_from_config`, leaving the parent loop untouched.
    crate::fork_dispatcher::install(runtime.clone()).await;
    crate::hook_agent_runner::install(runtime).await;

    // In-prompt skill / slash-command shell routing is wired at the
    // engine-build site (`SessionRuntime::build_engine`): it calls
    // `QueryEngine::build_base_tool_context()` and
    // `crate::bash_tool_handle::build_session_bash_handle(base_ctx)`, then
    // installs the one handle into both the command registry and the skill
    // runtime's shared cell so skill / `ShellExpandingPromptHandler` markers
    // route through the real Bash tool with a per-command permission check.
    // Refreshing it there (rather than once here) also survives a
    // `/reload-plugins` registry swap.

    Ok(())
}

/// Unified MCP bootstrap shared by SDK / headless / TUI (the single
/// config-driven init the user asked for). Builds (or reuses) the
/// `McpConnectionManager`, registers config-file servers
/// (`McpConfigLoader::load` — `.mcp.json`, `.claude/mcp.json`,
/// `~/.coco/mcp.json`, managed / enterprise, local) plus plugin-contributed
/// servers, attaches the manager + an `McpManagerAdapter` handle to the runtime,
/// then connects every registered server in the background (concurrent,
/// per-server error-isolated) and registers each connected server's tools into
/// the live `ToolRegistry` so they reach the model. A best-effort MCP skill sync
/// follows.
///
/// `existing_manager` lets the SDK path share the manager it already handed to
/// `SdkServer` (for `mcp/setServers`); `None` builds a fresh one for TUI /
/// headless. No UI: server-initiated elicitations during the connect handshake
/// are declined.
pub async fn bootstrap_session_mcp(
    runtime: &Arc<SessionRuntime>,
    cwd: &Path,
    existing_manager: Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>,
    await_connect: bool,
) {
    let config_home = global_config::config_home();
    let manager = existing_manager.unwrap_or_else(|| {
        Arc::new(tokio::sync::Mutex::new(
            coco_mcp::McpConnectionManager::new_with_runtime_config(
                config_home.clone(),
                &runtime.runtime_config.mcp,
            ),
        ))
    });

    // Register config-file + plugin servers (config-map seeding only; the actual
    // connect is deferred to the background pass below).
    let config_servers = coco_mcp::McpConfigLoader::load(cwd, &config_home);
    let plugins = coco_plugins::load_enabled_plugins(&config_home, cwd);
    let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
    let plugin_servers = coco_plugins::mcp_bridge::extract_mcp_servers_from_plugins(&plugin_refs);
    {
        let mut mgr = manager.lock().await;
        mgr.register_all(config_servers);
        mgr.register_all(plugin_servers);
    }

    // Build + attach the handle (elicitation hooks for runtime `add_dynamic_server`
    // + the MCP-skill bridge) and the concrete manager.
    let skill_cache = Arc::new(tokio::sync::RwLock::new(
        coco_mcp::discovery::DiscoveryCache::default(),
    ));
    let elicit_counter = runtime
        .app_state
        .read()
        .await
        .elicitation_pending_count
        .clone();
    let adapter = crate::mcp_handle_adapter::McpManagerAdapter::new(manager.clone())
        .with_elicitation_hooks(
            runtime.hook_registry(),
            runtime.orchestration_ctx_factory(),
            Some(elicit_counter),
        )
        .with_skill_bridge(
            runtime.skill_manager(),
            skill_cache.clone(),
            runtime.runtime_config.clone(),
        );
    runtime.attach_mcp_manager(manager.clone()).await;
    runtime.attach_mcp_handle(Arc::new(adapter)).await;

    // Post-OAuth reconnect → registry re-reconcile. The manager can't touch
    // the `ToolRegistry` (layering), so it notifies this app-layer listener
    // with the server name after each background reconnect settles; we then
    // install real tools (success) or re-surface the authenticate tool (login
    // failed). This is what makes the model-driven authenticate flow complete:
    // the per-server pseudo-tool starts OAuth, and on completion the real tools
    // swap in automatically.
    {
        let (reconnect_tx, mut reconnect_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        manager.lock().await.set_reconnect_notifier(reconnect_tx);
        let listener_manager = manager.clone();
        let listener_registry = runtime.tools().clone();
        tokio::spawn(async move {
            while let Some(server) = reconnect_rx.recv().await {
                let snapshot = listener_manager.lock().await.clone();
                reconcile_mcp_server_registration(&snapshot, &listener_registry, &server).await;
            }
        });
    }

    // Connect + tool registration. Each server connects concurrently; a failure /
    // timeout logs and is skipped so the registry degrades gracefully. MCP skills
    // sync once connections settle. `await_connect` chooses the timing:
    //   - `true`  (headless / single-turn): block so MCP tools are registered
    //     before the first turn. Bounded by the per-server timeout in
    //     `connect_and_register_mcp`.
    //   - `false` (interactive / long-lived SDK): connect in the background so
    //     startup isn't blocked (codex-rs pattern); tools appear within seconds.
    let registry = runtime.tools().clone();
    let features = runtime.runtime_config.features.clone();
    let skills = runtime.skill_manager();
    let connect_task = async move {
        connect_and_register_mcp(manager.clone(), registry).await;
        let snapshot = manager.lock().await.clone();
        let summary = coco_mcp_skills::sync_all(&snapshot, &skill_cache, &skills, &features).await;
        if summary.servers > 0 || summary.errors > 0 {
            tracing::info!(
                servers = summary.servers,
                registered = summary.total_registered,
                errors = summary.errors,
                "MCP skill sync (bootstrap)"
            );
        }
    };
    if await_connect {
        connect_task.await;
    } else {
        tokio::spawn(connect_task);
    }
}

/// Connect every registered-but-not-yet-connected MCP server concurrently
/// (per-server error-isolated + time-boxed) and register each connected server's
/// tools into `registry` so the model can see them. Best-effort: a failed or slow
/// server logs a warning and is skipped — a broken server never aborts the rest.
/// No UI: elicitations during connect are declined.
/// Reused by [`bootstrap_session_mcp`] and `SessionRuntime::reload_plugin_mcp_servers`.
pub(crate) async fn connect_and_register_mcp(
    manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    registry: Arc<coco_tool_runtime::ToolRegistry>,
) {
    let names = manager.lock().await.registered_server_names();
    let mut set = tokio::task::JoinSet::new();
    for name in names {
        let snapshot = manager.lock().await.clone();
        // Idempotent on reload: skip servers already connected.
        if matches!(
            snapshot.get_state(&name).await,
            Some(coco_mcp::McpConnectionState::Connected(_))
        ) {
            continue;
        }
        // Skip a doomed connect and surface the authenticate tool directly,
        // either because the server recently 401'd (cached) or because we hold
        // OAuth discovery for it but no usable token. Both avoid a network
        // round-trip.
        if snapshot.is_needs_auth_cached(&name).await || snapshot.needs_auth_without_connect(&name)
        {
            snapshot.mark_needs_auth(&name).await;
            reconcile_mcp_server_registration(&snapshot, &registry, &name).await;
            continue;
        }
        let registry = registry.clone();
        set.spawn(async move {
            let send: coco_mcp::SendElicitation = Box::new(|_id, _req| {
                Box::pin(async move {
                    Err(coco_mcp::RmcpClientError::generic(
                        "MCP elicitation is not supported without an interactive UI",
                    ))
                })
            });
            match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                snapshot.connect(&name, send),
            )
            .await
            {
                // A connect `Err` is no longer terminal: it may have landed in
                // NeedsAuth (a 401 for lack of credentials). Reconcile by the
                // resulting state rather than the `Result`.
                Ok(Ok(())) | Ok(Err(_)) => {
                    reconcile_mcp_server_registration(&snapshot, &registry, &name).await;
                }
                Err(_) => tracing::warn!(server = %name, "MCP connect timed out; skipping"),
            }
        });
    }
    while set.join_next().await.is_some() {}
}

/// Reconcile the tool registry for one MCP server against its current
/// connection state: install real tools when `Connected`, surface the
/// per-server `mcp__<server>__authenticate` pseudo-tool when `NeedsAuth`, and
/// leave existing registrations untouched otherwise. The "leave untouched"
/// arm prevents a transient `Failed`/`Pending` from deregistering an
/// already-surfaced auth tool. Reused by [`connect_and_register_mcp`]
/// and the post-OAuth reconnect listener in [`bootstrap_session_mcp`].
pub(crate) async fn reconcile_mcp_server_registration(
    manager: &coco_mcp::McpConnectionManager,
    registry: &coco_tool_runtime::ToolRegistry,
    name: &str,
) {
    match manager.get_state(name).await {
        Some(coco_mcp::McpConnectionState::Connected(_)) => {
            let schemas =
                crate::sdk_server::handlers::mcp::collect_server_schemas_for_manager(manager, name)
                    .await;
            let report = coco_tools::register_mcp_tools(registry, name, schemas);
            tracing::info!(
                server = %name,
                tools = report.registered.len(),
                skipped = report.skipped.len(),
                "MCP server connected; tools registered"
            );
        }
        Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
            if let Some((transport, url)) = manager.auth_descriptor(name) {
                coco_tools::register_mcp_auth_tool(registry, name, &transport, url.as_deref());
                tracing::info!(
                    server = %name,
                    %transport,
                    "MCP server needs auth; surfaced per-server authenticate tool"
                );
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "session_bootstrap.test.rs"]
mod tests;
