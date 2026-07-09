//! TUI runner — orchestrates TUI ↔ QueryEngine ↔ FileHistory.
//!
//! Uses an explicit async task (`run_agent_driver`) since ratatui is not a
//! reactive framework.
//!
//! Architecture:
//! ```text
//! ┌─────────────┐ UserCommand ┌────────────────┐ LLM / tools ┌────────────┐
//! │ TUI App │ ──────────────>│ agent_driver │ ──────────────>│ QueryEngine│
//! │ (ratatui) │ <──────────────│ (tokio task) │ <──────────────│ │
//! └─────────────┘ ServerNotif. └────────────────┘ QueryEvent └────────────┘
//! │
//! FileHistoryState
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use coco_config::EnvKey;
use coco_config::env;
use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_query::CoreEvent;
use coco_query::QueuePriority;
use coco_query::QueuedCommand;
use coco_query::QueuedImage;
use coco_query::ServerNotification;
use coco_system_reminder::QueueOrigin;
use coco_tui::App;
use coco_tui::UserCommand;
use coco_tui::app::create_channels;
use coco_types::SlashCommandStatusKind;
use coco_types::TuiOnlyEvent;
use tokio_util::sync::CancellationToken;

use coco_cli::goal_command;
use coco_cli::process_runtime::ProcessRuntime;
use coco_cli::resume_resolver::ResumePlan;
use coco_cli::session_bootstrap::build_engine_resources;
use coco_cli::session_bootstrap::install_session_late_binds;

use crate::Cli;

type SharedSessionHandle = Arc<RwLock<crate::session_runtime::SessionHandle>>;

/// Run the interactive TUI mode.
/// Spawns agent_driver as background task, runs TUI in foreground.
/// `resume_plan`: resolved by the binary entry from
/// `--resume` / `--continue` / `--fork-session` flags. When `Some`,
/// the runtime is repointed at the source session id and `runtime.history()`
/// is seeded with the loaded messages so the first turn picks up where
/// the prior session left off. Pre-populating the transcript dedup set
/// prevents the per-turn append from re-writing already-persisted
/// messages.
pub async fn run_tui(
    cli: &Cli,
    resume_plan: Option<ResumePlan>,
    cwd: PathBuf,
    process_runtime: Arc<ProcessRuntime>,
) -> Result<()> {
    // One-shot consume the inherited `COCO_*` identity env (team name, agent
    // id/name/color, plan-mode). CC reads its `CLAUDE_INTERNAL_ASSISTANT_*`
    // analog once at module load and `delete`s it so a teammate's own children
    // (grandchildren) never inherit the identity. Done FIRST, at process
    // bootstrap (single-threaded, before any identity resolution below), so the
    // cache is populated before the env vars are removed — later
    // `resolve_teammate_identity()` calls read the cache, not the (now-removed)
    // env. Harmless for the leader: its identity env is unset, so nothing is
    // cached or removed.
    coco_coordinator::identity::consume_inherited_env_identity();

    // Build a one-shot startup snapshot for process/UI initialization. The
    // live session runtime below owns its cwd-folded `RuntimeReloader`.
    let runtime_config = coco_cli::headless::build_runtime_config_for_cli(cli, &cwd)?;
    coco_cli::model_card_refresh::spawn_if_enabled(&runtime_config);
    coco_cli::startup_profile::mark("config_resolved");

    // Freeze the resolved teammate spawn mode ONCE for the session (CC's
    // `teammateModeSnapshot`). A later settings hot-reload republishes
    // `runtime_config` but the snapshot is write-once, so the spawn backend
    // can't change mid-session. The CLI override (if any) wins inside the
    // capture.
    coco_coordinator::teammate::capture_teammate_mode_snapshot(
        runtime_config.agent_teams.teammate_mode.into(),
    );

    // Engine resources (client, tools, system prompt, command registry,
    // startup-permission state) shared with SDK / headless via
    // `session_bootstrap::build_engine_resources`. The slash-command
    // registry uses the full load order (builtins → extended → skills →
    // plugin contributions → P1 handlers), so `dispatch_slash_command`
    // and the SDK `initialize.commands` advertisement share one Arc.
    let resources = build_engine_resources(&process_runtime, cli, &runtime_config, &cwd)?;
    coco_cli::startup_profile::mark("engine_resources_built");
    let model_id = resources.model_id.clone();
    let permission_mode = resources.startup.mode;
    let bypass_permissions_available = resources.startup.bypass_available;
    let auto_mode_available = resources.startup.auto_available;
    let plan_mode_available = runtime_config
        .features
        .enabled(coco_types::Feature::PlanMode);
    let startup_notification = resources.startup.notification.clone();
    let tools = resources.tools;
    let command_registry = resources.command_registry.clone();

    // Session manager for auto-title persistence (F5). Built here so
    // `SessionRuntime::build` can borrow it and the cleanup task can
    // own it. Backend (disk / memory) follows the resolved
    // `session.backend`; `SessionRuntime` sources its engine store from
    // this same manager so both observe one backend.
    let session_manager = Arc::new(coco_session::SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        coco_config::global_config::config_home(),
    ));
    let _ = session_manager.create(&model_id, &cwd);
    {
        // Background housekeeping: prune session files older than the
        // default retention period (30 days). Fire-and-forget.
        let mgr = session_manager.clone();
        let transcript_store =
            coco_session::TranscriptStore::new(coco_cli::paths::project_paths(&cwd));
        tokio::spawn(async move {
            let period = coco_session::default_cleanup_period();
            match tokio::task::spawn_blocking(move || -> coco_session::Result<(i32, i32)> {
                let removed_sessions = mgr.cleanup_older_than(period)?;
                let removed_tool_results =
                    transcript_store.cleanup_tool_results_older_than(period)?;
                Ok((removed_sessions, removed_tool_results))
            })
            .await
            {
                Ok(Ok((removed_sessions, removed_tool_results)))
                    if removed_sessions > 0 || removed_tool_results > 0 =>
                {
                    tracing::info!(
                        target: "coco::session::cleanup",
                        removed_sessions,
                        removed_tool_results,
                        "pruned old session artifacts"
                    );
                }
                Ok(Err(e)) => tracing::warn!(
                    target: "coco::session::cleanup",
                    error = %e,
                    "session cleanup failed"
                ),
                _ => {}
            }
        });
    }

    // Fast-role ModelSpec for auto-title generation (F5). Prefer the
    // JSON-first runtime config; keep the Anthropic Haiku fallback for
    // users who only configured an API key.
    let fast_model_spec = runtime_config
        .model_roles
        .get(coco_types::ModelRole::Fast)
        .cloned()
        .or_else(|| {
            runtime_config
                .providers
                .get("anthropic")
                .and_then(coco_config::ProviderConfig::resolve_api_key)
                .map(|_| coco_types::ModelSpec {
                    provider: "anthropic".to_string(),
                    api: coco_types::ProviderApi::Anthropic,
                    model_id: "claude-haiku-4-5-20251001".to_string(),
                    display_name: "Claude Haiku 4.5".to_string(),
                })
        });

    // P0: build channels FIRST so the TUI permission bridge can
    // capture the notification sender. Without this, the engine's
    // `PermissionDecision::Ask` path falls back to legacy auto-allow
    // (permission_controller.rs:100-107), which is the wrong default
    // for interactive sessions.
    let (command_tx, command_rx, notification_tx, notification_rx) = create_channels();
    let pending_approvals = coco_cli::tui_permission_bridge::new_pending_map();
    // Keep a concrete `Arc<TuiPermissionBridge>` alongside the trait
    // object so we can install the SessionRuntime weak-ref after
    // `SessionRuntime::build` returns (used to fire the Notification
    // hook on permission prompts).
    let tui_permission_bridge_concrete =
        Arc::new(coco_cli::tui_permission_bridge::TuiPermissionBridge::new(
            notification_tx.clone(),
            pending_approvals.clone(),
        ));
    let tui_permission_bridge: coco_tool_runtime::ToolPermissionBridgeRef =
        tui_permission_bridge_concrete.clone();

    // Pick the permission bridge for THIS session's engine. A cross-process
    // pane teammate forwards deny-path prompts to the leader via mailbox IPC
    // (the leader polls its inbox + routes them to its approval UI) rather
    // than prompting in the teammate's own pane; the leader session keeps the
    // TuiPermissionBridge. Pane workers install the mailbox bridge, the
    // leader uses ToolUseConfirm. In-process teammates instead inherit the
    // leader's bridge via `wire_engine` and never reach this branch.
    let session_permission_bridge: coco_tool_runtime::ToolPermissionBridgeRef =
        match coco_coordinator::identity::resolve_teammate_identity() {
            Some(identity) => {
                // Bounded by MailboxPermissionBridge's internal timeout, so a
                // silent/absent leader fails closed rather than hanging.
                let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                Arc::new(coco_coordinator::MailboxPermissionBridge::new(
                    identity, cancelled,
                ))
            }
            None => tui_permission_bridge.clone(),
        };

    // SessionRuntime owns every per-session subsystem (FileReadState,
    // SessionMemoryService, FileHistoryState, ToolAppState,
    // CompactionObserverRegistry, HookRegistry, history Mutex, etc.).
    // Both runners (TUI + SDK) share this construction; the per-turn
    // engine assembly below routes through `runtime.build_engine()`.
    let initial_session_id = resume_plan
        .as_ref()
        .map(|plan| plan.session_id.clone())
        .unwrap_or_else(coco_types::SessionId::generate);
    let initial_runtime_cwd = resume_plan
        .as_ref()
        .map(|plan| plan.cwd.clone())
        .unwrap_or_else(|| cwd.clone());
    let mut local_app_server_bridge = coco_cli::sdk_server::AppServerLocalBridge::new(Arc::new(
        coco_cli::sdk_server::SdkServerState::default(),
    ));
    let runtime_factory_cli = Arc::new(cli.clone());
    let runtime_factory = crate::session_runtime::SessionRuntimeFactory::new(
        crate::session_runtime::SessionRuntimeFactoryOpts {
            cli: Arc::clone(&runtime_factory_cli),
            bootstrap_source:
                crate::session_runtime::SessionRuntimeBootstrapSource::per_session_fold(
                    Arc::clone(&runtime_factory_cli),
                    process_runtime.clone(),
                ),
            cwd: cwd.clone(),
            model_runtimes: None,
            tools,
            session_manager,
            fast_model_spec,
            permission_bridge: Some(session_permission_bridge),
            process_runtime: process_runtime.clone(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // Interactive TUI: file-history checkpointing defaults ON.
            is_non_interactive: false,
        },
    );
    let session_handle = local_app_server_bridge
        .load_session_runtime(initial_session_id.clone(), {
            let runtime_factory = runtime_factory.clone();
            let initial_runtime_cwd = initial_runtime_cwd.clone();
            async move {
                runtime_factory
                    .build_with_session_id_and_cwd(initial_session_id, initial_runtime_cwd)
                    .await
            }
        })
        .await?;
    let event_hub_connector = {
        let session_id = session_handle.current_typed_session_id().await;
        coco_cli::event_hub::RuntimeEventHubConnector::spawn_for_session(
            session_handle.runtime_config(),
            session_id,
            &initial_runtime_cwd,
        )
    };
    if let Some(connector) = &event_hub_connector {
        local_app_server_bridge.set_hub_connector_sender(connector.sender());
    }
    let runtime = &session_handle;
    let current_session = Arc::new(RwLock::new(session_handle.clone()));
    let (mut runtime_reload_subscriptions, display_settings_rx, config_reload_errors_rx) =
        TuiRuntimeReloadSubscriptions::new(
            Arc::clone(&current_session),
            notification_tx.clone(),
            pending_approvals.clone(),
        );
    runtime_reload_subscriptions
        .install_for_session(&session_handle)
        .await;
    let runtime_reload_subscriptions = Arc::new(Mutex::new(runtime_reload_subscriptions));
    tui_permission_bridge_concrete
        .set_notification_session(Arc::downgrade(&current_session))
        .await;

    // Post-build late-binds shared with SDK: task runtime, agent
    // transcript persistence, agent-team wiring, fork dispatcher.
    // Without this TUI used to silently miss background AgentTool,
    // resume, and `/btw`. MCP handle is `None` until TUI grows its
    // own `McpConnectionManager` bootstrap.
    let lsp_handle = coco_cli::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime.clone(),
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    install_session_late_binds(
        session_handle.clone(),
        &initial_runtime_cwd,
        None,
        lsp_handle,
        Some(notification_tx.clone()),
    )
    .await?;
    // Unified MCP bootstrap: load config-file + plugin MCP servers, attach the
    // manager/handle, and connect in the background. The TUI now grows its own
    // `McpConnectionManager` (was SDK-only) — `None` builds a fresh one.
    coco_cli::session_bootstrap::bootstrap_session_mcp(
        &session_handle,
        &initial_runtime_cwd,
        None,
        /*await_connect*/ false,
    )
    .await;
    coco_cli::startup_profile::mark("session_late_binds");

    let bridge_session_id = runtime.current_typed_session_id().await;
    local_app_server_bridge
        .install_session_runtime(session_handle.clone())
        .await;
    local_app_server_bridge.ensure_interactive_surface(bridge_session_id.clone())?;
    local_app_server_bridge.start_passive_event_pump(bridge_session_id, notification_tx.clone())?;
    local_app_server_bridge
        .client()
        .keep_alive(local_app_server_bridge.handler())
        .await?;

    // The permission bridge resolves the active runtime through
    // `current_session`, so `Notification` hooks fire when the user is asked
    // to approve a tool, including after `/resume`, `/branch`, or `/clear`.

    // Plugin change detector. Lifecycle: held by `_plugin_watcher_guard`
    // so the `Arc` lives until this function returns (TUI shutdown). The
    // wrapped `FileWatcher` drops with the Arc, shutting its notify
    // thread + throttle task down cleanly.
    let _plugin_watcher_guard = coco_cli::plugin_watch::spawn(
        notification_tx.clone(),
        &cwd,
        &coco_config::global_config::config_home(),
    );

    // Skill change detector. Reloads the skill catalog (reminder +
    // SkillTool) and rebuilds the slash-command registry on `.md` edits
    // so authoring skills doesn't require a session restart. Held by
    // `_skill_watcher_guard` until TUI shutdown (drop = clean stop),
    // exactly like the plugin watcher above.
    let _skill_watcher_guard = coco_cli::skill_watch::spawn_current_session(
        session_handle.clone(),
        Arc::clone(&current_session),
        notification_tx.clone(),
        cwd.clone(),
        coco_config::global_config::config_home(),
    );

    // Team-memory server sync. Pulls server team memory at session start,
    // then debounce-pushes local edits. Fire-and-forget on the interactive
    // path; no-ops unless team memory is enabled, the repo has a github
    // `origin` slug, and a claude.ai OAuth token is present.
    coco_cli::team_memory_sync::bootstrap(
        runtime.runtime_config(),
        cwd.clone(),
        coco_config::global_config::config_home(),
    );

    // Agent-teams role wiring. A LEADER registers the setter that routes a
    // pane teammate's forwarded permission request to its approval UI +
    // replies via mailbox, plus a continuous 1s inbox poll. A cross-process
    // TEAMMATE instead runs the inbox→turn pump (gap 1) that drives turns
    // from its own mailbox. `teammate_turn_done_tx` is the pump's completion
    // handshake (threaded into `run_agent_driver` below); `None` for a
    // leader. `pump_cancel` lets the exit path stop the pump so it drops
    // its `command_tx` clone and the driver can shut down.
    let mut teammate_turn_done_tx: Option<mpsc::Sender<String>> = None;
    let pump_cancel = CancellationToken::new();
    if runtime
        .runtime_config()
        .features
        .enabled(coco_types::Feature::AgentTeams)
    {
        match coco_coordinator::identity::resolve_teammate_identity() {
            None => {
                // Leader session: register the human approval queue + spawn the
                // continuous 1s inbox poll so worker prompts/idle/shutdown
                // surface even while the leader is idle.
                // Shared with the headless/SDK leader paths.
                coco_cli::leader_inbox_poller::install_leader(
                    session_handle.clone(),
                    Some(tui_permission_bridge.clone()),
                )
                .await;
            }
            Some(identity) => {
                // Seed this teammate's live permission rules from the team's
                // allowed paths (gap 8) so it inherits the team's write fences
                // without prompting. Seed into the session's shared overlay Arc
                // (the same Arc `build_engine` injects onto every engine and the
                // pump extends live on a leader `TeamPermissionUpdate`) — the
                // cross-process analog of the in-process `TeammateControlState`.
                let live_rules = runtime.live_permission_rules();
                live_rules.write().await.extend(
                    coco_coordinator::runner_loop::load_team_allowed_path_rules(
                        &identity.team_name,
                    ),
                );
                // Cross-process teammate: pump this teammate's mailbox into
                // TUI turns. `command_tx` is cloned BEFORE `App::new` consumes
                // it below; the pump injects `SubmitInput` and serializes on
                // the completion handshake.
                let (tx, rx) = mpsc::channel::<String>(16);
                teammate_turn_done_tx = Some(tx);
                coco_cli::teammate_inbox_pump::spawn(
                    identity,
                    command_tx.clone(),
                    rx,
                    pump_cancel.clone(),
                    live_rules,
                );
            }
        }
    }

    // Official marketplace auto-install. Fire-and-forget on the interactive
    // path only: retry-gated + backoff, opt-out via
    // `COCO_PLUGINS_DISABLE_OFFICIAL_MARKETPLACE`, and non-fatal. Never
    // blocks startup — the official marketplace is fetched once in the
    // background and reused on subsequent launches.
    coco_cli::session_bootstrap::spawn_marketplace_startup(
        coco_config::global_config::config_home(),
    );

    // Honor `--resume` / `--continue` / `--fork-session`. The binary
    // entry has already loaded the source transcript; route the switch through
    // the local AppServer so startup resume and in-session `/resume` share the
    // same lifecycle registration, surface replacement, and runtime handler
    // boundary.
    let startup_session_start_source = if resume_plan.is_some() {
        "resume"
    } else {
        "startup"
    };
    if let Some(plan) = resume_plan {
        tracing::info!(
            target: "coco_cli::resume",
            session_id = %plan.session_id,
            source_session_id = %plan.source_session_id,
            prior_messages = plan.prior_messages.len(),
            is_fork = plan.is_fork,
            "resume: hydrating session",
        );
        apply_resume_plan_through_app_server(
            &plan,
            &session_handle,
            &current_session,
            &notification_tx,
            &mut local_app_server_bridge,
            &runtime_factory,
            &process_runtime,
            &runtime_reload_subscriptions,
        )
        .await
        .map_err(|err| anyhow::anyhow!("startup resume via AppServer failed: {err}"))?;
        // Reconcile coordinator mode to the resumed session. This flips
        // `COCO_COORDINATOR_MODE` *before*
        // the coordinator badge (below) and the first per-turn system
        // prompt are computed, so both reflect the resumed session's mode.
        if let Some(warning) = coco_cli::coordinator_mode_resume::reconcile_on_resume(
            plan.conversation.mode.as_deref(),
            &runtime.runtime_config().features,
        ) {
            emit_slash_text(&notification_tx, "resume", "", warning).await;
        }
        eprintln!(
            "{} session {} ({} prior message(s))",
            if plan.is_fork { "Forked" } else { "Resumed" },
            plan.source_session_id,
            plan.prior_messages.len(),
        );
    }

    // Cron tick driver — fires scheduled tasks (project config dir/scheduled_tasks.json +
    // session tasks) into the current session command queue. TUI-only:
    // headless/SDK have no queue-drain pump. Spawn after startup resume so the
    // initial missed-task scan targets the final startup runtime.
    let _cron_tick_guard = coco_cli::cron_tick::spawn_current_session(Arc::clone(&current_session));

    // Fire SessionStart hooks once at session bootstrap. Output queues
    // onto the shared sync-hook buffer and surfaces as `hook_*` reminders
    // on the first turn's reminder pass.
    runtime
        .fire_session_start_hooks(startup_session_start_source)
        .await;

    // TUI users opt into per-spawn periodic AgentSummary timers via
    // `COCO_AGENT_SUMMARY_ENABLE`. Default off keeps LLM cost off the
    // hot path. Coordinator mode auto-enables independently and ignores
    // this flag.
    if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoAgentSummaryEnable) {
        runtime
            .app_state()
            .write()
            .await
            .agent_progress_summaries_enabled = true;
    }

    // Create TUI app
    let mut app = App::new(command_tx, notification_rx, cwd.clone())
        .map_err(|e| anyhow::anyhow!("Failed to create TUI: {e}"))?;
    app.state_mut()
        .ui
        .apply_display_settings(coco_tui::DisplaySettings::from_runtime_config(
            runtime.runtime_config(),
        ));
    app.state_mut().ui.coordinator_mode_active =
        coco_subagent::is_coordinator_mode(&runtime.runtime_config().features);

    // Hydrate the composer's up-arrow history from the persistent
    // cross-session store (`<config_home>/history.jsonl`), project-scoped
    // to this cwd and newest-first. Text-only recall — paste pills are
    // re-snapshotted per session, matching codex cross-session behaviour.
    {
        let project = cwd.to_string_lossy();
        let session_id = runtime.current_typed_session_id().await;
        let texts = coco_session::PromptHistory::new(runtime.config_home(), &project, &session_id)
            .get_history()
            .into_iter()
            .map(|e| e.display)
            .collect();
        app.state_mut().ui.input.hydrate_history(texts);
    }
    app = app.with_display_settings_reload(display_settings_rx);
    app = app.with_config_reload_errors(config_reload_errors_rx);
    // Voice input (Feature::Voice): build the STT engine + capture + session and
    // install it. No-op when voice is disabled; best-effort on failure.
    app = coco_cli::voice_bootstrap::install_voice(app, runtime.runtime_config());

    // Wire file_history_enabled into TUI session state so the rewind
    // modal knows whether to show code restore options.
    app.state_mut().session.file_history_enabled = runtime.file_history().is_some();

    // Seed the capability gates that control the Shift+Tab cycle
    // (`PermissionMode::next_in_cycle`) and the plan-mode exit
    // modal's "Bypass" option. Matches engine_config below so the
    // engine and TUI share one truth. Static for session lifetime.
    app.state_mut().session.bypass_permissions_available = bypass_permissions_available;
    app.state_mut().session.auto_mode_available = auto_mode_available;
    app.state_mut().session.plan_mode_available = plan_mode_available;
    app.state_mut().session.permission_mode = permission_mode;
    // Seed the model + provider for the status bar. Production TUI
    // doesn't currently install a `SessionBootstrap`, so the engine's
    // `emit_session_started` is a no-op and the model field would
    // otherwise stay empty until a fallback fires. Provider is the
    // authoritative id from the resolved Main role; the picker keeps
    // a prefix-match fallback for unregistered builtins.
    app.state_mut().session.model = model_id.clone();
    app.state_mut().session.provider = runtime
        .runtime_config()
        .model_roles
        .get(coco_types::ModelRole::Main)
        .map(|spec| spec.provider.clone())
        .unwrap_or_default();
    // Seed cwd + git branch so the header's "where am I" rows render on
    // the first frame. Production TUI doesn't install `SessionBootstrap`,
    // so the engine's `emit_session_started` never fires the
    // `ServerNotification::SessionStarted` that would populate these via
    // `protocol::handle`. Without this seed the rows stay empty for the
    // session's lifetime.
    app.state_mut().session.working_dir = Some(cwd.to_string_lossy().into_owned());
    app.state_mut().session.git_branch = coco_git::get_current_branch(&cwd).ok().flatten();
    // Mirror `SessionStarted`'s thinking-level seed: read the model's
    // registered default so the header's effort dial reflects the real
    // starting state, not the `ReasoningEffort::Auto` fallback.
    if let Some(default_effort) = runtime
        .runtime_config()
        .model_roles
        .get(coco_types::ModelRole::Main)
        .and_then(|spec| {
            runtime
                .runtime_config()
                .model_registry
                .resolve(&spec.provider, &spec.model_id)
        })
        .and_then(|resolved| resolved.info.default_thinking_level)
    {
        app.state_mut().session.thinking_effort = default_effort;
    }

    // Seed `model_catalog` and `model_by_role` from the resolved
    // `ModelRegistry`. The TUI picker and Ctrl+T cycle both consult
    // these — using the registry view (rather than the L0-only
    // `builtin_models_partial`) means L1 `config home/models.json` entries
    // and L2 `providers.<n>.models.<id>` overrides are visible.
    {
        let mut catalog = build_model_catalog(runtime.runtime_config());
        let provider_statuses = build_provider_statuses(runtime.runtime_config());
        let by_role = build_model_by_role(runtime.runtime_config());
        let state = app.state_mut();
        state.session.model_catalog = std::mem::take(&mut catalog);
        state.session.provider_statuses = provider_statuses;
        state.session.model_by_role = by_role;
        state.session.available_models = runtime
            .runtime_config()
            .settings
            .merged
            .available_models
            .clone();
    }

    // Seed `available_commands` so the `/` autocomplete popup and the
    // `Ctrl+Shift+P` command palette resolve against the live registry
    // (builtins + extended + skills + plugin contributions). Without
    // this snapshot the popup silently shows nothing because the field
    // defaults to an empty Vec.
    // Two seed paths:
    // * **Startup (here)** — direct mutation. The event loop hasn't
    // started yet, so emitting on `notification_tx` would just
    // queue the event behind `App::run()`'s first iteration —
    // adds latency without simplifying anything.
    // * **Reload (`/reload-plugins`)** — see [`run_reload_plugins`].
    // Emits [`TuiOnlyEvent::AvailableCommandsRefreshed`] through
    // the same event channel the agent driver uses; the TUI
    // handler at `server_notification_handler::tui_only` overwrites
    // the slot and re-runs `refresh_suggestions`.
    {
        let snapshot = command_registry.read().await.snapshot_for_ui();
        app.state_mut().session.available_commands = snapshot;
    }

    // Seed `available_agents` so the unified `@` autocomplete popup
    // surfaces agents (Plan / Explore / general-purpose / ...) inline
    // alongside file matches. Without this seed the popup only ever
    // shows file paths because the agent half of `unified::seed_agent_items`
    // iterates an empty slice. The catalog hot-reload path
    // (`session_runtime::reload_agent_catalog`) keeps the wire warm —
    // each `/agents reload` (and, once stage 5 lands, each CRUD edit)
    // re-pushes the updated set via the same notification used for
    // `available_commands`.
    {
        let snapshot = runtime.agent_catalog_snapshot().await;
        let agents: Vec<coco_tui::autocomplete::AgentInfo> = snapshot
            .active()
            .map(coco_tui::autocomplete::AgentInfo::from_definition)
            .collect();
        app.state_mut().session.available_agents = agents;
    }

    // Surface the startup downgrade notification (if any) as a toast
    // so interactive users see it. Headless paths eprintln it; the
    // TUI swallows stderr.
    if let Some(msg) = startup_notification {
        app.state_mut()
            .ui
            .add_toast(coco_tui::state::ui::Toast::warning(msg));
    }

    // Boot the TUI theme stack from config home/theme.json. This is TUI-local
    // config, separate from RuntimeConfig, so user palette edits can hot-reload
    // without rebuilding the agent runtime.
    let _theme_watcher_guard = {
        let coco_tui::theme::ThemeSetup {
            watcher,
            reload_rx,
            initial,
            watch_error,
        } = coco_tui::theme::install_theme().await;
        app.state_mut().ui.apply_theme_runtime(initial.state);
        if let Some(error) = initial.error {
            app.state_mut()
                .ui
                .add_toast(coco_tui::state::ui::Toast::warning(error));
        }
        if let Some(error) = watch_error {
            app.state_mut()
                .ui
                .add_toast(coco_tui::state::ui::Toast::warning(error));
        }
        app = app.with_theme_reload(reload_rx);
        watcher
    };

    // Boot the keybindings stack via the TUI helper: builds a
    // watcher-backed handle (which hot-reloads on file changes via
    // `KeybindingsWatcher`) and gives back a channel of post-startup
    // validation warnings to plumb into the App's event loop.
    let kb_setup = coco_tui::keybinding_setup::install_keybindings().await;

    // Surface **startup** warnings as toasts immediately (subsequent
    // reloads flow through the `kb_setup.warnings_rx` channel below).
    for issue in &kb_setup.initial.warnings {
        let line = coco_keybindings::format_issue_oneline(issue);
        let toast = match issue.severity {
            coco_keybindings::Severity::Error => coco_tui::state::ui::Toast::error(line),
            coco_keybindings::Severity::Warning => coco_tui::state::ui::Toast::warning(line),
        };
        app.state_mut().ui.add_toast(toast);
    }

    // Install the watcher-backed handle into AppState — replaces the
    // defaults-only handle `UiState::new()` initialized. Reads + chord
    // state both flow through this clone.
    app.state_mut().ui.kb_handle = kb_setup.handle;

    // Plug the warnings receiver into the App so post-startup reloads
    // (user edits `keybindings.json` while the TUI is running) also
    // surface as toasts.
    app = app.with_keybinding_warnings(kb_setup.warnings_rx);

    // Hold onto the watcher for the TUI's lifetime — dropping it
    // stops the hot-reload background task.
    let _kb_watcher_guard = kb_setup.watcher;

    let runtime_for_resume_hint = runtime.clone();

    // Spawn agent driver — owns the session handle + transports.
    let flag_settings_path = cli.settings.as_deref().map(std::path::PathBuf::from);
    let driver_handle = tokio::spawn(run_agent_driver(
        command_rx,
        notification_tx,
        current_session,
        local_app_server_bridge,
        pending_approvals,
        runtime_reload_subscriptions,
        runtime_factory,
        process_runtime.clone(),
        cwd.clone(),
        flag_settings_path,
        teammate_turn_done_tx,
    ));

    // Startup is complete; emit the phase profile (COCO_STARTUP_PROFILE)
    // before `app.run()` blocks for the rest of the session. The final
    // `app_ready` mark closes the last window so App construction is counted.
    coco_cli::startup_profile::mark("app_ready");
    coco_cli::startup_profile::report();

    // Run TUI (blocks until exit)
    let tui_result = app.run().await;

    // Stop the cross-process teammate inbox pump (if any) so it drops its
    // `command_tx` clone. Without this the held clone keeps the driver's
    // `command_rx` open and `driver_handle.await` below blocks forever — the
    // teammate process would hang on every exit. No-op for a leader session.
    pump_cancel.cancel();

    // Capture the session id BEFORE dropping the App — the TUI's Drop
    // restores the terminal but moves the AppState out of reach.
    let state_session_id = app.state().session.session_id.clone();
    let runtime_session_id = runtime_for_resume_hint
        .current_typed_session_id()
        .await
        .to_string();
    let session_id = state_session_id.or({
        if runtime_session_id.is_empty() {
            None
        } else {
            Some(runtime_session_id)
        }
    });
    // Explicit drop: `Tui::drop` (inside App) is what leaves alt-screen
    // and disables raw mode. Without this the resume hint below would
    // scroll inside the alt buffer and vanish when the terminal
    // restores the main buffer on exit.
    drop(app);

    // Print the resume hint **before** any async cleanup so the user sees
    // it immediately on Ctrl+C, even when the agent driver is mid-shutdown
    // (tool flush, transcript append, telemetry). The driver writes only
    // to stderr / log files, so it cannot clobber this stdout write.
    coco_cli::resume_hint::print_resume_hint(&cwd, session_id.as_deref());

    // On leader exit: kill any orphaned tmux teammate panes, then remove the
    // team dirs (+ worktrees + tasks via cleanup_team_directories) for teams
    // this session led, so neither the child processes nor the dirs orphan.
    if let Some(sid) = session_id.as_deref()
        && let Err(e) = coco_coordinator::team_file::cleanup_session_teams(sid)
    {
        tracing::warn!(error = %e, "team cleanup on exit failed");
    }

    // Wait for agent driver to finish its own teardown.
    let _ = driver_handle.await;
    if let Some(connector) = event_hub_connector {
        connector.shutdown_and_flush().await;
    }

    tui_result.map_err(|e| anyhow::anyhow!("TUI error: {e}"))
}

fn spawn_display_settings_reload_to(
    mut rx: tokio::sync::watch::Receiver<Arc<coco_config::RuntimeConfig>>,
    tx: mpsc::Sender<coco_tui::DisplaySettings>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let display_settings = coco_tui::DisplaySettings::from_runtime_config(&rx.borrow());
            if tx.send(display_settings).await.is_err() {
                break;
            }
        }
    })
}

fn spawn_model_runtime_reload(
    registry: Arc<coco_inference::ModelRuntimeRegistry>,
    publisher: &coco_config::RuntimePublisher,
) -> tokio::task::JoinHandle<()> {
    let mut rx = publisher.subscribe();
    let _initial = rx.borrow_and_update();
    drop(_initial);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let snapshot = rx.borrow_and_update().clone();
            match registry.reconcile(snapshot) {
                Ok(()) => tracing::debug!("model runtime registry hot-reloaded"),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "model runtime registry hot-reload failed; keeping prior runtimes"
                    );
                }
            }
        }
        tracing::debug!("model runtime reload subscriber: publisher closed; exiting");
    })
}

fn spawn_config_reload_error_toasts_to(
    mut rx: tokio::sync::broadcast::Receiver<coco_config_reload::ConfigReloadError>,
    tx: mpsc::Sender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(err) => {
                    let source = err.kind.as_str();
                    let detail = err.message;
                    let message = format!("{source}: {detail}");
                    if tx.send(message).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

struct TuiRuntimeReloadSubscriptions {
    current_session: SharedSessionHandle,
    notification_tx: mpsc::Sender<CoreEvent>,
    pending_approvals: coco_cli::tui_permission_bridge::PendingApprovals,
    display_settings_tx: mpsc::Sender<coco_tui::DisplaySettings>,
    config_reload_error_tx: mpsc::Sender<String>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl TuiRuntimeReloadSubscriptions {
    fn new(
        current_session: SharedSessionHandle,
        notification_tx: mpsc::Sender<CoreEvent>,
        pending_approvals: coco_cli::tui_permission_bridge::PendingApprovals,
    ) -> (
        Self,
        mpsc::Receiver<coco_tui::DisplaySettings>,
        mpsc::Receiver<String>,
    ) {
        let (display_settings_tx, display_settings_rx) = mpsc::channel(16);
        let (config_reload_error_tx, config_reload_errors_rx) = mpsc::channel(16);
        (
            Self {
                current_session,
                notification_tx,
                pending_approvals,
                display_settings_tx,
                config_reload_error_tx,
                handles: Vec::new(),
            },
            display_settings_rx,
            config_reload_errors_rx,
        )
    }

    async fn install_for_session(&mut self, session: &crate::session_runtime::SessionHandle) {
        self.abort_current();
        let runtime = session;

        if let Some(rx) = runtime.subscribe_config_changes() {
            self.handles.push(
                crate::session_runtime::spawn_current_session_config_change_watcher(
                    Arc::clone(&self.current_session),
                    rx,
                ),
            );
        }

        if let Some(rx) = runtime.subscribe_config_reload_errors() {
            self.handles.push(spawn_config_reload_error_toasts_to(
                rx,
                self.config_reload_error_tx.clone(),
            ));
        }

        if let Some(publisher) = runtime.runtime_publisher() {
            self.handles.push(spawn_display_settings_reload_to(
                publisher.subscribe(),
                self.display_settings_tx.clone(),
            ));
            if let Some(state) = runtime.sandbox_state() {
                self.handles
                    .push(coco_cli::sandbox_reload::spawn_sandbox_reload(
                        state,
                        &publisher,
                        runtime.original_cwd().clone(),
                    ));
            }
            self.handles.push(spawn_model_runtime_reload(
                runtime.model_runtimes(),
                &publisher,
            ));
        }

        if let Some(state) = runtime.sandbox_state() {
            state.set_approval_bridge(std::sync::Arc::new(
                coco_cli::sandbox_approval_bridge_tui::TuiSandboxApprovalBridge::new(
                    self.notification_tx.clone(),
                    self.pending_approvals.clone(),
                ),
            ));
            state.start_violation_monitor();
            if let Some(mut rx) = state.take_violation_observer() {
                let tx = self.notification_tx.clone();
                self.handles.push(tokio::spawn(async move {
                    while let Some(count) = rx.recv().await {
                        let _ = tx
                            .send(CoreEvent::Protocol(
                                ServerNotification::SandboxViolationsDetected { count },
                            ))
                            .await;
                    }
                }));
            }
        }
    }

    fn abort_current(&mut self) {
        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }
}

impl Drop for TuiRuntimeReloadSubscriptions {
    fn drop(&mut self) {
        self.abort_current();
    }
}

/// Agent driver — consumes UserCommands, drives QueryEngine, emits CoreEvents.
/// Runs as a background tokio task alongside the TUI event loop.
/// Events flow directly as `CoreEvent` from QueryEngine → TUI (no mapping layer).
#[allow(clippy::too_many_arguments)]
async fn run_agent_driver(
    mut command_rx: mpsc::Receiver<UserCommand>,
    event_tx: mpsc::Sender<CoreEvent>,
    current_session: SharedSessionHandle,
    mut local_app_server_bridge: coco_cli::sdk_server::AppServerLocalBridge,
    pending_approvals: coco_cli::tui_permission_bridge::PendingApprovals,
    runtime_reload_subscriptions: Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    process_runtime: Arc<ProcessRuntime>,
    cwd: std::path::PathBuf,
    flag_settings: Option<std::path::PathBuf>,
    // Cross-process teammate inbox pump (gap 1) completion handshake. When
    // `Some`, each spawned top-level turn fires its `user_message_id` here on
    // completion so the pump can serialize on its own injected turn. `None`
    // for leader / standalone sessions.
    teammate_turn_done_tx: Option<mpsc::Sender<String>>,
) {
    {
        let session = current_session.read().await.clone();
        session.install_side_query_event_tx(event_tx.clone()).await;
    }
    // Per-session one-shot gate: title gen runs at most once per
    // session id, never the process. After `/resume` or `/clear` the
    // session id changes; the new id is not in the set, so the gate
    // re-arms (each session's title state is independent).
    // `Arc<RwLock<HashSet>>` because the SubmitInput body runs in a
    // spawned task — the outer scope must hand a shared handle to the
    // spawn for cross-turn observation.
    let title_gen_attempted: Arc<RwLock<std::collections::HashSet<String>>> =
        Arc::new(RwLock::new(std::collections::HashSet::new()));
    info!("Agent driver started");

    // Active-turn tracker. AppServer-owned turns keep a completion monitor task
    // plus an interrupt client here; the dispatch loop continues to `recv()` so
    // interrupting commands (`Interrupt`, `Compact`, `Rewind`, `Shutdown`)
    // reach their arms without waiting for the engine to finish.
    let active_turn: Arc<Mutex<Option<ActiveTurn>>> = Arc::new(Mutex::new(None));
    let mut pending_editor_requests: HashMap<String, PendingEditorRequest> = HashMap::new();
    let mut explicit_shutdown = false;
    let (turn_done_tx, mut turn_done_rx) = mpsc::channel::<uuid::Uuid>(16);

    loop {
        let command = tokio::select! {
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                command
            }
            Some(turn_id) = turn_done_rx.recv() => {
                if drain_completed_turn(&active_turn, turn_id).await {
                    let session = current_session.read().await.clone();
                    process_idle_command_queue(
                        &session,
                        &current_session,
                        &event_tx,
                        &mut local_app_server_bridge,
                        &active_turn,
                        &mut pending_editor_requests,
                        &title_gen_attempted,
                        &turn_done_tx,
                        &runtime_reload_subscriptions,
                        &runtime_factory,
                        &process_runtime,
                        &cwd,
                    )
                    .await;
                }
                continue;
            }
            _ = {
                let session = current_session.read().await.clone();
                async move { session.command_queue().wait_for_change().await }
            } => {
                let session = current_session.read().await.clone();
                process_idle_command_queue(
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &active_turn,
                        &mut pending_editor_requests,
                        &title_gen_attempted,
                        &turn_done_tx,
                        &runtime_reload_subscriptions,
                        &runtime_factory,
                        &process_runtime,
                        &cwd,
                )
                .await;
                continue;
            }
        };
        // Re-read each turn so `/clear` regen picks up the new id.
        let session = current_session.read().await.clone();
        let runtime = &session;
        let session_id = runtime.current_typed_session_id().await;
        match command {
            UserCommand::SubmitInput {
                user_message_id,
                content,
                images,
                ..
            } => {
                if content.is_empty() {
                    continue;
                }

                // Slash-command interception. When the user typed `/foo args`,
                // resolve through `runtime.command_registry` BEFORE handing
                // raw text to the model.
                let mut effective_content = content;
                let mut slash_metadata = None;
                let mut slash_thinking_level = None;
                let mut slash_model_runtime_source = None;
                if let Some((name, args)) = parse_slash_command(&effective_content) {
                    let outcome = dispatch_slash_command(
                        name,
                        args,
                        &session,
                        &current_session,
                        &event_tx,
                        &mut local_app_server_bridge,
                        &runtime_factory,
                        &process_runtime,
                        &runtime_reload_subscriptions,
                    )
                    .await;
                    let control_context = LocalRuntimeControlContext {
                        current_session: &current_session,
                        runtime_reload_subscriptions: &runtime_reload_subscriptions,
                        runtime_factory: &runtime_factory,
                        process_runtime: &process_runtime,
                        cwd: &cwd,
                        turn_done_tx: &turn_done_tx,
                    };
                    match handle_slash_outcome(
                        outcome,
                        &session,
                        &control_context,
                        &event_tx,
                        &active_turn,
                        &mut pending_editor_requests,
                        &mut local_app_server_bridge,
                    )
                    .await
                    {
                        SlashFollowup::Done => continue,
                        // Unknown command falls through to the model
                        // as raw text — falls through to the model.
                        SlashFollowup::NotFound => {}
                        SlashFollowup::RunEngine {
                            content,
                            metadata,
                            thinking_level,
                            model_runtime_source,
                        } => {
                            effective_content = content;
                            slash_metadata = metadata;
                            slash_thinking_level = thinking_level;
                            slash_model_runtime_source = model_runtime_source;
                        }
                    }
                }

                // Defensive drain: TUI input layer gates submit on
                // `running` state, but a slow gate could still let a
                // second SubmitInput through. Cancel + await the prior
                // turn before starting the new one — last-write-wins
                // semantics (a new submit aborts the previous turn).
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;

                let turn_id = uuid::Uuid::new_v4();
                local_app_server_bridge
                    .install_session_runtime(session.clone())
                    .await;
                if let Err(error) = local_app_server_bridge
                    .start_passive_event_pump(session_id.clone(), event_tx.clone())
                {
                    tracing::warn!(%error, "TUI SubmitInput could not refresh local AppServer event pump");
                }
                let mut monitor_client = local_app_server_bridge.connect_local_client();
                let passive_surface = match monitor_client.subscribe_session(
                    session_id.clone(),
                    Some(0),
                    coco_app_server::AttachSurfaceOptions::default(),
                ) {
                    Ok(surface) => surface,
                    Err(error) => {
                        tracing::warn!(%error, "TUI SubmitInput could not attach AppServer completion monitor");
                        continue;
                    }
                };
                let params = coco_types::TurnStartParams {
                    prompt: effective_content,
                    history_override: Vec::new(),
                    images: image_data_to_turn_start(&images),
                    slash_metadata,
                    model_selection: model_runtime_source_to_turn_start_selection(
                        slash_model_runtime_source,
                    ),
                    permission_mode: None,
                    thinking_level: slash_thinking_level,
                };
                let started = match local_app_server_bridge
                    .start_turn(session_id.clone(), params)
                    .await
                {
                    Ok(started) => started,
                    Err(error) => {
                        tracing::warn!(%error, "TUI SubmitInput AppServer turn/start failed");
                        continue;
                    }
                };
                let session_t = session.clone();
                let title_gen_attempted_t = title_gen_attempted.clone();
                let session_id_t = session_id.clone();
                let turn_done_tx_t = turn_done_tx.clone();
                // Cross-process teammate pump handshake: fire this turn's
                // `user_message_id` on completion (Drop covers panic + cancel).
                let pump_done = teammate_turn_done_tx.as_ref().map(|tx| PumpDoneGuard {
                    id: user_message_id.clone(),
                    tx: tx.clone(),
                });
                let protocol_turn_id = started.turn_id.clone();
                let auto_title_client = local_app_server_bridge.connect_local_client();
                let auto_title_handler = local_app_server_bridge.handler().clone();

                let task = tokio::spawn(async move {
                    let _done = TurnDoneGuard {
                        turn_id,
                        tx: turn_done_tx_t,
                    };
                    let _pump_done = pump_done;
                    let mut auto_title_client = Some(auto_title_client);
                    let mut auto_title_handler = Some(auto_title_handler);
                    while let Some(envelope) =
                        monitor_client.next_passive_event(&passive_surface).await
                    {
                        if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) =
                            envelope.event
                            && ended.turn_id == protocol_turn_id
                        {
                            if let (Some(client), Some(handler)) =
                                (auto_title_client.take(), auto_title_handler.take())
                            {
                                maybe_spawn_auto_title(
                                    &session_t,
                                    &title_gen_attempted_t,
                                    &session_id_t,
                                    client,
                                    handler,
                                )
                                .await;
                            }
                            break;
                        }
                    }
                });
                let interrupt_client = local_app_server_bridge.connect_local_client();
                let handler = local_app_server_bridge.handler().clone();

                *active_turn.lock().await = Some(ActiveTurn {
                    id: turn_id,
                    task,
                    cancel: ActiveTurnCancel {
                        client: interrupt_client,
                        handler,
                    },
                });
            }

            UserCommand::SubmitBash {
                user_message_id,
                command,
            } => {
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                let event_tx_t = event_tx.clone();
                let session_t = session.clone();
                let active_turn_t = active_turn.clone();
                let turn_done_tx_t = turn_done_tx.clone();
                let cwd = runtime.current_engine_config().await.workspace_cwd();
                tokio::spawn(async move {
                    run_prompt_mode_bash(
                        &cwd,
                        user_message_id,
                        command,
                        session_t,
                        event_tx_t,
                        active_turn_t,
                        turn_done_tx_t,
                    )
                    .await;
                });
            }

            UserCommand::PersistPromptHistory { display } => {
                // Append to the cross-session composer history off the
                // dispatch thread — the JSONL append takes an advisory file
                // lock. Session id is re-read each time so entries written
                // after `/resume` or `/clear` carry the new session tag.
                let config_home = runtime.config_home().clone();
                let project = cwd.to_string_lossy().to_string();
                let session_id = runtime.current_typed_session_id().await;
                tokio::task::spawn_blocking(move || {
                    let history =
                        coco_session::PromptHistory::new(&config_home, &project, &session_id);
                    if let Err(e) = history.add(&display) {
                        warn!(target: "coco_cli::history", error = %e,
                            "failed to persist prompt history");
                    }
                });
            }

            UserCommand::OpenMemoryFile { path } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Memory { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPlanEditor => {
                let path = runtime_session_plan_file_path(&session).await;
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Plan { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPlanPromptEditor {
                request_id,
                initial_content,
                plan_file_path,
            } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::PlanPrompt {
                        request_id,
                        initial_content,
                        path: plan_file_path,
                    },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPromptEditor { initial_content } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Prompt { initial_content },
                    &event_tx,
                )
                .await;
            }

            UserCommand::ExternalEditorTerminalReady { request_id } => {
                let Some(request) = pending_editor_requests.remove(&request_id) else {
                    warn!(%request_id, "terminal ready for unknown external editor request");
                    continue;
                };
                match request {
                    PendingEditorRequest::Memory { path } => {
                        run_open_memory_file(path, event_tx.clone()).await;
                    }
                    PendingEditorRequest::Plan { path } => {
                        run_open_plan_file(path, event_tx.clone()).await;
                    }
                    PendingEditorRequest::PlanPrompt {
                        request_id,
                        initial_content,
                        path,
                    } => {
                        run_plan_prompt_editor(request_id, initial_content, path, event_tx.clone())
                            .await;
                    }
                    PendingEditorRequest::Prompt { initial_content } => {
                        run_prompt_editor(initial_content, event_tx.clone()).await;
                    }
                    PendingEditorRequest::Agent { path } => {
                        run_open_agent_file(session.clone(), path, event_tx.clone()).await;
                    }
                }
            }

            UserCommand::ExternalEditorTerminalPrepareFailed { request_id, error } => {
                let Some(request) = pending_editor_requests.remove(&request_id) else {
                    warn!(%request_id, "terminal prepare failed for unknown editor request");
                    continue;
                };
                emit_editor_prepare_failed(request, error, event_tx.clone()).await;
            }

            UserCommand::SetModelRole {
                role,
                provider,
                model_id,
                effort,
            } => {
                apply_role_through_app_server(
                    &session,
                    role,
                    provider,
                    model_id,
                    effort,
                    &event_tx,
                    &local_app_server_bridge,
                )
                .await;
            }

            UserCommand::ProviderLogin { provider } => {
                // `/login` picker Enter — run the OAuth flow for the chosen
                // instance in the background (the flow blocks on a loopback
                // callback for up to 5 min). On success `dispatch_provider_login`
                // emits the transcript message + `ProviderStatusesRefreshed`, so
                // the `/model` picker's `NotLoggedIn` gate clears live.
                let session_t = session.clone();
                let event_tx_t = event_tx.clone();
                tokio::spawn(async move {
                    dispatch_provider_login(&provider, &session_t, &event_tx_t).await;
                });
            }

            UserCommand::SetThinkingLevel { level } => {
                set_thinking_level_through_app_server(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    level,
                )
                .await;
            }

            UserCommand::ToggleFastMode => {
                toggle_fast_mode_through_app_server(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                )
                .await;
            }

            UserCommand::ExecuteSkill { name, args } => {
                // Command-palette dispatch.
                // Same registry lookup as the typed path, but with no
                // user-supplied chat message — for `Prompt` outcomes
                // [`spawn_slash_run_engine_turn`] mints a fresh
                // user-message UUID so file-history / rewind keys
                // line up.
                let args_str = args.unwrap_or_default();
                let outcome = dispatch_slash_command(
                    &name,
                    &args_str,
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &runtime_factory,
                    &process_runtime,
                    &runtime_reload_subscriptions,
                )
                .await;
                let control_context = LocalRuntimeControlContext {
                    current_session: &current_session,
                    runtime_reload_subscriptions: &runtime_reload_subscriptions,
                    runtime_factory: &runtime_factory,
                    process_runtime: &process_runtime,
                    cwd: &cwd,
                    turn_done_tx: &turn_done_tx,
                };
                match handle_slash_outcome(
                    outcome,
                    &session,
                    &control_context,
                    &event_tx,
                    &active_turn,
                    &mut pending_editor_requests,
                    &mut local_app_server_bridge,
                )
                .await
                {
                    SlashFollowup::Done => {}
                    SlashFollowup::NotFound => {
                        warn!(%name, "ExecuteSkill: command not registered");
                    }
                    SlashFollowup::RunEngine {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    } => {
                        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                        spawn_slash_run_engine_turn(
                            SlashEnginePrompt {
                                content,
                                metadata,
                                thinking_level,
                                model_runtime_source,
                            },
                            &session,
                            &event_tx,
                            &mut local_app_server_bridge,
                            &active_turn,
                            &title_gen_attempted,
                            &turn_done_tx,
                            &session_id,
                        )
                        .await;
                    }
                }
            }

            UserCommand::ExecuteSlashCommand { name, args } => {
                let refresh_plugin_dialog = name.as_str() == "plugin";
                let outcome = dispatch_slash_command(
                    name.as_str(),
                    &args,
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &runtime_factory,
                    &process_runtime,
                    &runtime_reload_subscriptions,
                )
                .await;
                let control_context = LocalRuntimeControlContext {
                    current_session: &current_session,
                    runtime_reload_subscriptions: &runtime_reload_subscriptions,
                    runtime_factory: &runtime_factory,
                    process_runtime: &process_runtime,
                    cwd: &cwd,
                    turn_done_tx: &turn_done_tx,
                };
                match handle_slash_outcome(
                    outcome,
                    &session,
                    &control_context,
                    &event_tx,
                    &active_turn,
                    &mut pending_editor_requests,
                    &mut local_app_server_bridge,
                )
                .await
                {
                    SlashFollowup::Done => {}
                    SlashFollowup::NotFound => {
                        emit_slash_status(
                            &event_tx,
                            name.as_str(),
                            &args,
                            SlashCommandStatusKind::NoHandler,
                        )
                        .await;
                    }
                    SlashFollowup::RunEngine {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    } => {
                        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                        spawn_slash_run_engine_turn(
                            SlashEnginePrompt {
                                content,
                                metadata,
                                thinking_level,
                                model_runtime_source,
                            },
                            &session,
                            &event_tx,
                            &mut local_app_server_bridge,
                            &active_turn,
                            &title_gen_attempted,
                            &turn_done_tx,
                            &session_id,
                        )
                        .await;
                    }
                }
                if refresh_plugin_dialog {
                    refresh_plugin_dialog_payload(&session, &event_tx).await;
                }
            }

            UserCommand::Rewind { message_id, mode } => {
                // Drain first — rewind reads file_history snapshots
                // and rewrites runtime.history(); an in-flight turn that
                // mutates either would race.
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                match mode {
                    coco_tui::command::RewindMode::Explicit {
                        restore_type,
                        rewound_turn,
                    } => {
                        handle_rewind(
                            &restore_type,
                            &message_id,
                            rewound_turn,
                            &event_tx,
                            &session,
                            &local_app_server_bridge,
                        )
                        .await;
                    }
                    coco_tui::command::RewindMode::AutoRestore => {
                        handle_auto_truncate(&message_id, &event_tx, &session).await;
                    }
                }
            }

            UserCommand::RequestPermissionExplanation {
                request_id,
                tool_name,
                tool_input,
            } => {
                // Lazily fetch the risk explanation off the hot path (Ctrl+E
                // panel) and reply with PermissionExplanationReady.
                // `explain_permission_risk` gates on the setting + bounds the
                // call, so a disabled/slow explainer resolves to None and the
                // panel shows "unavailable".
                let runtime = runtime.clone();
                let tx = event_tx.clone();
                tokio::spawn(async move {
                    let params = coco_permissions::ExplainerParams {
                        tool_name: &tool_name,
                        tool_input: &tool_input,
                        tool_description: None,
                        messages: None,
                    };
                    let explanation = runtime.explain_permission_risk(params).await;
                    let _ = tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::PermissionExplanationReady {
                            request_id,
                            explanation,
                        }))
                        .await;
                });
            }

            UserCommand::RequestDiffStats { message_id } => {
                // Async restore-preview diff for the selected checkpoint.
                // `stats == None` carries `fileHistoryCanRestore == false`;
                // the TUI suppresses code-restore choices for that row.
                if let Some(fh) = runtime.file_history() {
                    let fh = fh.read().await;
                    let stats = if fh.can_restore(&message_id) {
                        match fh
                            .get_diff_stats(&message_id, runtime.config_home(), session_id.as_str())
                            .await
                        {
                            Ok(stats) => Some(diff_stats_to_payload(stats)),
                            Err(_) => Some(coco_types::RewindDiffStatsPayload::default()),
                        }
                    } else {
                        None
                    };
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::RewindRestorePreviewReady {
                            message_id,
                            stats,
                        }))
                        .await;
                }
            }
            UserCommand::RequestDiffStatsBatch { message_ids } => {
                // For each non-synthetic picker row, resolve
                // `fileHistoryCanRestore` and (if restorable) compute
                // the per-row `+X -Y` diff against the next row's
                // snapshot — or the working tree for the last row.
                // Uses the snapshot pair instead of walking
                // `msg.toolUseResult.structuredPatch` because
                // coco_messages has no typed tool-output side channel.
                if let Some(fh) = runtime.file_history() {
                    let fh = fh.read().await;
                    let mut rows = Vec::with_capacity(message_ids.len());
                    for (idx, message_id) in message_ids.iter().enumerate() {
                        let metadata = if fh.can_restore(message_id) {
                            let next = message_ids.get(idx + 1).map(String::as_str);
                            match fh
                                .get_diff_stats_between(
                                    message_id,
                                    next,
                                    runtime.config_home(),
                                    session_id.as_str(),
                                )
                                .await
                            {
                                Ok(stats) => Some(diff_stats_to_payload(stats)),
                                Err(_) => Some(coco_types::RewindDiffStatsPayload::default()),
                            }
                        } else {
                            None
                        };
                        rows.push(coco_types::RewindRowMetadata {
                            message_id: message_id.clone(),
                            metadata,
                        });
                    }
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::RewindRowMetadataReady {
                            rows,
                        }))
                        .await;
                }
            }

            UserCommand::Interrupt(_reason) => {
                // Mid-turn cancel now flows through the same AppServer
                // `turn/interrupt` request the SDK uses. The task slot stays
                // Some until the turn naturally emits its terminal event; the
                // next SubmitInput or driver shutdown drains it if needed.
                if let Some(state) = active_turn.lock().await.as_ref() {
                    let ActiveTurnCancel { client, handler } = &state.cancel;
                    match client.turn_interrupt(handler).await {
                        Ok(()) => info!("Interrupt: cancelled AppServer active turn"),
                        Err(error) => {
                            tracing::warn!(%error, "Interrupt: AppServer turn/interrupt failed")
                        }
                    }
                }
            }

            UserCommand::InterruptAgentCurrentWork { agent_id } => {
                local_app_server_bridge
                    .install_session_runtime(session.clone())
                    .await;
                match local_app_server_bridge
                    .client()
                    .agent_interrupt_current_work(
                        local_app_server_bridge.handler(),
                        coco_types::AgentInterruptCurrentWorkParams {
                            agent_id: agent_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => info!(%agent_id, "Interrupt: cancelled teammate current turn"),
                    Err(error) => {
                        tracing::warn!(%agent_id, %error, "Interrupt: teammate current turn failed")
                    }
                }
            }

            UserCommand::OpenAgentEditor { path } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Agent { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::CreateAgent {
                name,
                description,
                source,
            } => {
                // The TUI wizard pre-flights the writable-source +
                // file-exists checks before dispatching, so by the
                // time we reach here only rare I/O races land in the
                // error arm. Toast the failure and move on — the
                // wizard is already closed.
                match prepare_agent_create(&session, &name, &description, source).await {
                    Ok(path) => {
                        prepare_external_editor_request(
                            &mut pending_editor_requests,
                            PendingEditorRequest::Agent { path },
                            &event_tx,
                        )
                        .await;
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "coco::agents",
                            error = %err.to_user_string(),
                            %name,
                            ?source,
                            "CreateAgent: prepare failed"
                        );
                        let _ = event_tx
                            .send(CoreEvent::Tui(TuiOnlyEvent::PromptEditorFailed {
                                error: err.to_user_string(),
                            }))
                            .await;
                    }
                }
            }

            UserCommand::DeleteAgentFile { path } => {
                let session_t = session.clone();
                let event_tx_t = event_tx.clone();
                let path_display = path.display().to_string();
                tokio::spawn(async move {
                    if let Err(err) = std::fs::remove_file(&path) {
                        tracing::warn!(
                            target: "coco::agents",
                            %path_display,
                            error = %err,
                            "DeleteAgentFile: remove failed"
                        );
                        return;
                    }
                    // After delete, rebuild the payload and re-push so
                    // the dialog refreshes without the deleted row.
                    session_t.reload_agent_catalog().await;
                    refresh_agents_dialog(&session_t, &event_tx_t).await;
                });
            }

            UserCommand::CancelSubagent { task_id } => {
                // Fire the cancel token on the running task. The
                // existing task-driver pipeline emits
                // `CoreEvent::Protocol(TaskCompleted { status: Stopped })`
                // when the cancel takes effect, which the TUI handler
                // folds into `SessionState.subagents` so the Running
                // tab refreshes on the next frame. No additional event
                // wiring needed here.
                local_app_server_bridge
                    .install_session_runtime(session.clone())
                    .await;
                match local_app_server_bridge
                    .client()
                    .stop_task(
                        local_app_server_bridge.handler(),
                        coco_types::StopTaskParams {
                            task_id: task_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => info!(%task_id, "CancelSubagent: cancel token fired"),
                    Err(error) => tracing::warn!(
                        %task_id,
                        %error,
                        "CancelSubagent: AppServer stopTask failed"
                    ),
                }
            }

            UserCommand::QueueCommand { prompt, images } => {
                // User typed Enter while the agent was streaming.
                // Push onto the session-scoped command queue so the
                // running engine sees it at its next drain point
                // (mid-turn `Now` drain or end-of-turn full drain).
                // When a turn is active, the prompt is enqueued instead
                // of starting a fresh turn.
                if prompt.trim().is_empty() {
                    continue;
                }
                let queued = QueuedCommand::new(prompt, QueuePriority::Next)
                    .with_origin(QueueOrigin::Human)
                    .with_images(image_data_to_queued(&images));
                let id = queued.id;
                let preview = queued.preview();
                let editable = queued.is_editable_by_user();
                runtime.command_queue().enqueue(queued).await;
                // Round-trip notify: the TUI display
                // (`SessionState::queued_commands`) is a projection of
                // engine state and waits for this event to update —
                // see `update.rs::QueueInput` (no optimistic push).
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::CommandQueued {
                        id: id.to_string(),
                        preview,
                        editable,
                    }))
                    .await;
            }

            UserCommand::EditQueuedCommand { id } => {
                let Ok(uuid) = uuid::Uuid::parse_str(&id) else {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditUnavailable {
                            id,
                            reason: "invalid queued command id".to_string(),
                        }))
                        .await;
                    continue;
                };
                let Some(queued) = runtime.command_queue().remove_by_id(uuid).await else {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditUnavailable {
                            id,
                            reason: "queued command was already processed".to_string(),
                        }))
                        .await;
                    continue;
                };
                let id = queued.id.to_string();
                let images = queued
                    .images
                    .into_iter()
                    .map(|image| coco_types::QueuedCommandEditImage {
                        media_type: image.media_type,
                        data_base64: image.data_base64,
                    })
                    .collect();
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                        id: id.clone(),
                    }))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditReady {
                        id,
                        prompt: queued.prompt,
                        images,
                    }))
                    .await;
            }

            UserCommand::EditQueuedCommands {
                current_input,
                current_cursor,
            } => {
                let queued = runtime.command_queue().dequeue_all_editable().await;
                if queued.is_empty() {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditUnavailable {
                            id: String::new(),
                            reason: "no editable queued commands".to_string(),
                        }))
                        .await;
                    continue;
                }

                let ids: Vec<String> = queued.iter().map(|cmd| cmd.id.to_string()).collect();
                let mut queued_text = String::new();
                for cmd in &queued {
                    if !queued_text.is_empty() {
                        queued_text.push('\n');
                    }
                    queued_text.push_str(&cmd.prompt);
                }
                let mut prompt = queued_text.clone();
                if !current_input.is_empty() {
                    if !prompt.is_empty() {
                        prompt.push('\n');
                    }
                    prompt.push_str(&current_input);
                }
                let cursor = if queued_text.is_empty() {
                    current_cursor
                } else {
                    queued_text
                        .len()
                        .saturating_add(1)
                        .saturating_add(current_cursor)
                };
                let images = queued
                    .into_iter()
                    .flat_map(|cmd| cmd.images)
                    .map(|image| coco_types::QueuedCommandEditImage {
                        media_type: image.media_type,
                        data_base64: image.data_base64,
                    })
                    .collect();

                for id in &ids {
                    let _ = event_tx
                        .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                            id: id.clone(),
                        }))
                        .await;
                }
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
                        queued: runtime.command_queue().len().await as i32,
                    }))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandsEditReady {
                        ids,
                        prompt,
                        cursor,
                        images,
                    }))
                    .await;
            }

            UserCommand::Compact {
                custom_instructions,
            } => {
                // Manual `/compact [instructions]` from the TUI.
                // `custom_instructions` comes from trimming the args.
                info!(
                    session_id = %session_id,
                    has_instructions = custom_instructions.is_some(),
                    "TUI: manual /compact"
                );
                run_manual_compact(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    custom_instructions,
                    &active_turn,
                    &turn_done_tx,
                )
                .await;
            }

            UserCommand::SetPermissionMode { mode } => {
                let cur_session_id = runtime.current_typed_session_id().await;
                let cfg = runtime.current_engine_config().await;
                if mode == coco_types::PermissionMode::BypassPermissions
                    && !cfg.permission_mode_availability.bypass_permissions
                {
                    warn!(
                        session_id = %cur_session_id,
                        requested = ?mode,
                        "TUI SetPermissionMode denied: bypass capability gate is off"
                    );
                    continue;
                }
                local_app_server_bridge
                    .install_session_runtime(session.clone())
                    .await;
                let bridge_session_id = runtime.current_typed_session_id().await;
                if let Err(error) =
                    local_app_server_bridge.ensure_interactive_surface(bridge_session_id.clone())
                {
                    warn!(
                        session_id = %cur_session_id,
                        error = %error,
                        "TUI SetPermissionMode could not attach local AppServer surface"
                    );
                    continue;
                }
                if let Err(error) = local_app_server_bridge
                    .start_passive_event_pump(bridge_session_id, event_tx.clone())
                {
                    warn!(
                        session_id = %cur_session_id,
                        error = %error,
                        "TUI SetPermissionMode could not attach local AppServer event pump"
                    );
                    continue;
                }
                let previous = cfg.permission_mode;
                if let Err(error) = local_app_server_bridge
                    .client()
                    .set_permission_mode(
                        local_app_server_bridge.handler(),
                        coco_types::SetPermissionModeParams { mode },
                    )
                    .await
                {
                    warn!(
                        session_id = %cur_session_id,
                        requested = ?mode,
                        error = %error,
                        "TUI SetPermissionMode via AppServerLocalBridge failed"
                    );
                    continue;
                }
                info!(
                    session_id = %cur_session_id,
                    from = ?previous,
                    to = ?mode,
                    "TUI SetPermissionMode propagated to engine_config + app_state",
                );
                // If THIS session is a teammate (cross-process pane), mirror
                // the new mode into team.json so the leader's roster reflects
                // it — covers both an inbox-applied `ModeSetRequest` and a
                // self-initiated Shift+Tab cycle. Leader sessions are not
                // teammates, so this no-ops.
                if coco_coordinator::identity::is_teammate()
                    && let Some(team) = coco_coordinator::identity::get_team_name()
                    && let Some(agent) = coco_coordinator::identity::get_agent_name()
                    && let Err(e) = coco_coordinator::team_file::set_member_mode_in_team_file(
                        &team, &agent, mode,
                    )
                {
                    warn!(error = %e, "teammate mode write-back to team.json failed");
                }
            }

            UserCommand::SetTeammateMode { name, mode } => {
                // Leader applies a teammate's mode from the roster picker
                // (gap 8): persist to team.json + notify the live teammate.
                if let Some(handle) = runtime.current_agent_handle().await {
                    match handle.set_teammate_mode(&name, mode).await {
                        Ok(msg) => info!(teammate = %name, ?mode, "{msg}"),
                        Err(e) => {
                            warn!(teammate = %name, error = %e, "set teammate mode failed")
                        }
                    }
                }
            }

            UserCommand::SetTeammateModes { updates } => {
                // Leader applies the roster "cycle all" batch: one atomic
                // team.json write + a ModeSetRequest per teammate.
                if let Some(handle) = runtime.current_agent_handle().await {
                    match handle.set_teammate_modes(updates).await {
                        Ok(msg) => info!("{msg}"),
                        Err(e) => warn!(error = %e, "set teammate modes (cycle all) failed"),
                    }
                }
            }

            UserCommand::PlanApprovalResponse {
                request_id,
                teammate_agent,
                approved,
                feedback,
            } => {
                // Leader responding to a teammate's plan-approval
                // request. Write a `PlanApprovalResponse` envelope into
                // the teammate's inbox; their `poll_teammate_approval`
                // picks it up on the next turn boundary.
                let team_name = match env::var(EnvKey::CocoTeamName) {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        info!(%request_id, "PlanApprovalResponse: no COCO_TEAM_NAME; dropping");
                        continue;
                    }
                };
                let agent_name =
                    env::var(EnvKey::CocoAgentName).unwrap_or_else(|_| "team-lead".to_string());
                let mailbox: coco_tool_runtime::MailboxHandleRef =
                    Arc::new(coco_coordinator::mailbox::SwarmMailboxHandle);

                let response = coco_tool_runtime::PlanApprovalMessage::PlanApprovalResponse(
                    coco_tool_runtime::PlanApprovalResponse {
                        request_id: request_id.clone(),
                        approved,
                        feedback: feedback.clone(),
                        permission_mode: None,
                    },
                );
                let envelope = coco_tool_runtime::MailboxEnvelope {
                    text: serde_json::to_string(&response).unwrap_or_default(),
                    from: agent_name.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    summary: Some("plan approval response".to_string()),
                };
                if let Err(e) = mailbox
                    .write_to_mailbox(&teammate_agent, &team_name, envelope)
                    .await
                {
                    info!(%request_id, error = %e, "failed to write PlanApprovalResponse");
                } else {
                    // Clear the leader-side awaiting flag so the
                    // reminder can stop nagging about this request.
                    let mut guard = runtime.app_state().write().await;
                    if guard.awaiting_plan_approval_request_id.as_deref()
                        == Some(request_id.as_str())
                    {
                        guard.awaiting_plan_approval = false;
                        guard.awaiting_plan_approval_request_id = None;
                    }
                }
            }

            UserCommand::ApprovalResponse {
                request_id,
                approved,
                always_allow,
                feedback,
                updated_input,
                resolution_detail,
                mut permission_updates,
                content_blocks,
            } => {
                let pending_entry =
                    coco_cli::tui_permission_bridge::take_pending(&pending_approvals, &request_id)
                        .await;

                let always_allow_options_allowed =
                    coco_cli::tui_permission_bridge::settings_allow_always_allow_options(
                        &runtime.runtime_config().settings,
                    );
                if pending_entry.is_some()
                    && !always_allow_options_allowed
                    && !permission_updates.is_empty()
                {
                    warn!(
                        %request_id,
                        "dropping permission updates because managed policy disables always-allow"
                    );
                    permission_updates.clear();
                }

                // Apply any rule additions the user authorized
                // ("Always Allow" or future destination-picker
                // selections) BEFORE resolving the bridge. Order
                // Order: apply before resolving bridge so subsequent same-tool
                // calls within the turn pick up the rule.
                let mut applied_permission_updates = Vec::new();
                if pending_entry.is_some() && approved && !permission_updates.is_empty() {
                    // Unified apply through local AppServer before resolving
                    // the bridge, so subsequent same-tool calls in this turn
                    // pick up the rule through the runtime-control path.
                    for update in &permission_updates {
                        if apply_and_persist_permission_update(
                            update,
                            &event_tx,
                            &local_app_server_bridge,
                        )
                        .await
                        {
                            applied_permission_updates.push(update.clone());
                        }
                    }

                    // `command_permissions` carries command-scoped allowed
                    // tools. Do not encode deny/ask/reset events as
                    // `allowedTools`. One `command_permissions` attachment
                    // is emitted per slash-command invocation — event-stream
                    // semantics, not a snapshot.
                    let mut allowed_tools = Vec::new();
                    for update in &applied_permission_updates {
                        if let coco_types::PermissionUpdate::AddRules { rules, destination } =
                            update
                        {
                            for rule in rules {
                                if matches!(
                                    destination,
                                    coco_types::PermissionUpdateDestination::Command
                                ) && rule.behavior == coco_types::PermissionBehavior::Allow
                                {
                                    allowed_tools.push(rule.value.tool_pattern.clone());
                                }
                            }
                        }
                    }
                    if !allowed_tools.is_empty() {
                        let emitter = runtime.attachment_emitter();
                        emitter.emit(
                            coco_messages::AttachmentMessage::silent_command_permissions(
                                coco_messages::CommandPermissionsPayload {
                                    allowed_tools,
                                    model: None,
                                },
                            ),
                        );
                    }
                }

                // Always-allow with empty `permission_updates` is the
                // legacy path (pre-Phase A). Treat as one-shot approve
                // — the rule plumbing the prompt produced was lost
                // somewhere between TUI and runner. Log and move
                // on rather than failing.
                if always_allow && permission_updates.is_empty() {
                    debug!(
                        %request_id,
                        "always_allow set without permission_updates; treating as one-shot approve"
                    );
                }

                // Route the user's Approve / Deny back to the pending
                // oneshot the `TuiPermissionBridge` is awaiting.
                // `applied_updates` are forwarded so audit/logging
                // downstream sees the user's intent. Stale request_ids
                // (already resolved or timed-out) are logged and
                // dropped when stale (already resolved or timed-out).
                if let Some(entry) = pending_entry {
                    let resolved = coco_cli::tui_permission_bridge::send_resolution(
                        entry,
                        approved,
                        feedback,
                        applied_permission_updates,
                        updated_input,
                        resolution_detail,
                        content_blocks,
                    );
                    if !resolved {
                        info!(
                            %request_id,
                            approved,
                            "ApprovalResponse receiver dropped after request was taken"
                        );
                    }
                } else {
                    info!(
                        %request_id,
                        approved,
                        "ApprovalResponse for unknown request_id (already resolved or stale)"
                    );
                }
            }

            UserCommand::ApplyPermissionUpdate { update } => {
                // `/permissions` editor add / delete. Apply to the live
                // engine config + persist to the chosen settings file
                // (User / Project / Local), then re-emit the editor payload
                // so the open overlay refreshes from disk.
                let _ = apply_and_persist_permission_update(
                    &update,
                    &event_tx,
                    &local_app_server_bridge,
                )
                .await;
                refresh_permissions_editor(&session, &event_tx).await;
            }

            UserCommand::Shutdown { reason } => {
                info!(%reason, "Shutdown requested by TUI");
                // User-confirmed exit must return control to the
                // terminal promptly. Give an in-flight turn a short
                // cooperative-cancel window, but do not wait on the
                // long memory drains here; those can take up to 75s
                // and make the double-press exit look ignored.
                drain_active_turn(
                    &active_turn,
                    ActiveTurnDrain::AbortAfter(TUI_SHUTDOWN_ACTIVE_TURN_DRAIN_TIMEOUT),
                )
                .await;
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::SessionEnded(
                        coco_types::SessionEndedParams {
                            reason: "User shutdown".into(),
                        },
                    )))
                    .await;
                explicit_shutdown = true;
                break;
            }

            UserCommand::FireIdleNotification { message } => {
                // The TUI detected an idle window past the idle threshold;
                // route through the hook orchestrator so registered
                // `Notification` hooks fire with
                // `notification_type = "idle_prompt"`.
                let registry = runtime.hook_registry();
                let factory = runtime.orchestration_ctx_factory();
                let ctx = (factory)();
                if ctx.disable_all_hooks {
                    continue;
                }
                if let Err(e) = coco_hooks::orchestration::execute_notification(
                    &registry,
                    &ctx,
                    "idle_prompt",
                    &message,
                    /*title*/ None,
                )
                .await
                {
                    tracing::warn!(error = %e, "idle_prompt notification hook failed");
                }
            }

            UserCommand::BackgroundAllTasks => {
                // Ctrl+B single press: flip every foreground BgAgent /
                // Shell row to backgrounded server-side. The TUI mirror
                // in `session.subagents` is updated optimistically inside
                // `TuiCommand::BackgroundAllTasks` (update.rs) — foreground→background
                // is a UI-state transition,
                // not a task lifecycle event, so no `task/*` wire event
                // fires now. The eventual real `task/completed` (with
                // `output_file` populated) flows when the bg task
                // actually terminates. Idempotent — a second press with
                // no foreground tasks transitions nothing.
                match background_all_tasks_through_app_server(&session, &local_app_server_bridge)
                    .await
                {
                    Ok(result) => {
                        let count = result.len();
                        info!(count, "BackgroundAllTasks: backgrounded foreground tools");
                    }
                    Err(error) => {
                        warn!(%error, "BackgroundAllTasks via AppServerLocalBridge failed");
                    }
                }
            }

            UserCommand::PushSystemMessage { kind } => {
                // TUI-originated transcript content (slash output,
                // file-open notices, plan-rejected body, …) round-trips
                // through engine `MessageHistory` so every observer
                // (TUI transcript view, SDK consumers, JSONL transcript)
                // sees it via the same `MessageAppended` event stream as
                // engine-pushed content. See
                // `engine-tui-unified-transcript-plan.md` §3 Commit 2.
                let msg = build_system_message_from_push_kind(kind);
                let mut h = runtime.history().lock().await;
                let event_tx_opt = Some(event_tx.clone());
                coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
            }

            UserCommand::RetryPermissionDenied { tool_name, message } => {
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                let messages = {
                    let msg = build_system_message_from_push_kind(
                        coco_tui::SystemPushKind::PermissionRetry { tool_name, message },
                    );
                    let mut h = runtime.history().lock().await;
                    let event_tx_opt = Some(event_tx.clone());
                    coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt)
                        .await;
                    h.to_vec()
                };
                spawn_history_turn(messages, &session, &event_tx, &active_turn, &turn_done_tx)
                    .await;
            }

            UserCommand::PushSlashResult { messages } => {
                // Pre-built slash echo+result `Message::User`s (see
                // `command_tags`). Push each through engine authority so the
                // transcript view, SDK, and JSONL converge — and so the
                // per-message `is_visible_in_transcript_only` gate is the
                // single source of truth for model visibility.
                let mut h = runtime.history().lock().await;
                let event_tx_opt = Some(event_tx.clone());
                for msg in messages {
                    coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt)
                        .await;
                }
            }

            UserCommand::WriteSkillOverrides { patch } => {
                let runtime_publisher = runtime.runtime_publisher();
                handle_write_skill_overrides(
                    &session,
                    &event_tx,
                    patch,
                    runtime_publisher.as_ref(),
                    &cwd,
                    flag_settings.as_deref(),
                )
                .await;
            }

            // Other commands: log and skip for now
            other => {
                info!(?other, "Unhandled UserCommand in agent driver");
            }
        }
    }

    // Driver loop exited (sender dropped or Shutdown). Drain any
    // turn that's still running so we don't leak a JoinHandle, and
    // wait briefly on any pending auto-memory extraction so partial
    // writes don't get cut off.
    if explicit_shutdown {
        debug!("skipping memory extraction drain after explicit TUI shutdown");
    } else {
        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
        let session = current_session.read().await.clone();
        drain_pending_memory_extraction(&session).await;
    }
    // Re-append metadata one more time at process-exit so the tail window
    // of the final transcript JSONL definitely carries the user's
    // title/tag/agent-name. Best-effort — IO errors here are logged but
    // don't propagate out of the driver.
    {
        let session = current_session.read().await.clone();
        let runtime = &session;
        let session_id = runtime.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        let mgr = Arc::clone(runtime.session_manager());
        // Snapshot the session's coordinator-mode state at the exit
        // checkpoint so `--resume` re-derives it via
        // `reconcile_on_resume`. Gated on agent-teams so non-team
        // transcripts stay clean. `Option<&'static str>` is Send → moved
        // into the blocking closure alongside the re-append.
        let mode = runtime
            .runtime_config()
            .features
            .enabled(coco_types::Feature::AgentTeams)
            .then(|| {
                if coco_subagent::is_coordinator_mode(&runtime.runtime_config().features) {
                    "coordinator"
                } else {
                    "normal"
                }
            });
        if let Err(e) = tokio::task::spawn_blocking(move || {
            let _ = mgr.re_append_session_metadata(&session_id_string);
            if let Some(mode) = mode {
                let _ = mgr.save_mode(&session_id_string, mode);
            }
        })
        .await
        {
            warn!(error = %e, "shutdown re-append task join failed");
        }
    }
    let session = current_session.read().await.clone();
    session.flush_session_usage_snapshot().await;
    info!("Agent driver stopped");
}

/// Wait for scheduled turn-end extraction/session-memory work before
/// shutdown. Silently no-ops when auto-memory is inactive.
async fn drain_pending_memory_extraction(session: &crate::session_runtime::SessionHandle) {
    let Some(memory_runtime) = session.memory_runtime() else {
        return;
    };
    if !memory_runtime
        .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
        .await
    {
        warn!("auto-memory extraction did not drain within timeout — continuing shutdown");
    }
}

/// Body of `UserCommand::SubmitInput` extracted into an async fn so
/// it can be `tokio::spawn`ed. The dispatch loop stores the
/// `JoinHandle` in `active_turn` and continues to recv the next
/// command — letting `Interrupt` / `Compact` /
/// `Rewind` / `Shutdown` reach their arms while the engine runs.
/// All session-scoped Arcs are read out of `runtime` inside the body —
/// the only data piped in are the per-turn user inputs, the cancel
/// token, the cross-turn `title_gen_attempted` latch, and the snapshot
/// of `session_id` taken on the dispatcher side (so the title-gen path
/// uses the same id the rest of the turn observed, not a later
/// `/clear`-regenerated one).
/// Outcome of slash-command resolution against `runtime.command_registry`.
/// `dispatch_slash_command` is the single source of truth for routing
/// `/foo` regardless of whether the user typed it (`SubmitInput`) or
/// picked it from the palette (`ExecuteSkill`).
enum SlashOutcome {
    /// Command consumed locally (Text / Compact / OpenDialog / Skip).
    /// The caller should NOT run the engine.
    Handled,
    /// Re-feed `content` into the engine as the user message
    /// (Prompt / InjectPrompt). For typed commands the original `/foo`
    /// is replaced with the rendered prompt body so the model sees the
    /// expansion, not the slash.
    RunEngine {
        content: String,
        metadata: Option<String>,
        thinking_level: Option<coco_types::ThinkingLevel>,
        model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
    },
    /// A user-typed fork-mode skill (`/<name>` with `context: fork`).
    /// Unlike `RunEngine` (which re-queries the main model with the
    /// expanded body), this runs the skill as a subagent via the
    /// installed `SkillHandle` and injects only its result — mirroring
    /// `executeForkedSlashCommand`. `name` is the canonical skill name.
    RunForkSkill { name: String, args: String },
    /// No command with this name is registered. Caller should fall
    /// through to the existing path (model receives raw text).
    NotFound,
    /// Trigger the same flow as `UserCommand::Compact`. Emitted when
    /// the slash dispatcher detects `COMPACT_SENTINEL` (palette path)
    /// or intercepts `/compact` / `/compact <args>` directly. The agent
    /// driver sends the sentinel through local AppServer `turn/start`, whose
    /// compact shortcut runs the actual summarization task.
    TriggerCompact { custom_instructions: Option<String> },
    /// Trigger the clear flow for `/clear`. The agent driver swaps in a
    /// fresh local AppServer-backed runtime and emits the TUI reset event.
    TriggerClear,
    /// Trigger auto-memory consolidation (when the runtime has a
    /// `MemoryRuntime`). Emitted when the dispatcher sees `DREAM_SENTINEL`.
    TriggerDream,
    /// Trigger a session-memory force update (9-section). Emitted when
    /// the dispatcher sees `SUMMARY_SENTINEL`.
    TriggerSummary,
    /// Render the live multi-provider session cost. Emitted when the
    /// dispatcher sees `COST_SENTINEL`; the runner asks local AppServer
    /// `session/cost` for the live usage snapshot and formatted report.
    ShowCost,
    /// Render the live session status (model / permission mode / thinking /
    /// plan mode / MCP servers). Emitted on `STATUS_SENTINEL`; the runner
    /// asks local AppServer `session/status`.
    ShowStatus,
    /// Install, clear, or show the session-scoped `/goal` Stop hook.
    TriggerGoal {
        request: coco_commands::GoalCommandRequest,
    },
    /// Rename the current session. `Explicit(name)` uses the
    /// caller-supplied name verbatim; `Auto` directs the dispatcher
    /// to derive a kebab-case name via the `ModelRole::Fast`
    /// resolver. Either way the runner persists via
    /// [`coco_session::SessionManager::set_title`] (which writes
    /// both `CustomTitle` and `AgentName`) and patches the PID
    /// registry so `coco ps` reflects the new name live.
    TriggerRename {
        request: coco_commands::ParsedRename,
    },
    /// Toggle a tag on the current session through local AppServer
    /// `session/toggleTag`.
    TriggerTag { tag: String },
    /// Push `path` onto the live `ToolAppState.permissions.additional_dirs`
    /// base so the next batch's permission context sees the wider scope.
    TriggerAddDir { path: String },
    /// Open a concrete session plan file through the same external
    /// editor terminal handoff used by prompt and memory editing.
    TriggerOpenPlanEditor { path: std::path::PathBuf },
    /// Run a `/btw` side question as a one-shot fork that shares the
    /// parent turn's prompt cache, then render the answer inline. Emitted
    /// when the dispatcher sees `BTW_SENTINEL`; the runner reads
    /// `runtime.last_cache_safe_params()` (or the transcript fallback) plus
    /// the installed `ForkDispatcher` (mirrors the SDK `/btw` handler shortcut).
    /// The parent conversation is untouched.
    TriggerBtw {
        request: coco_commands::handlers::btw::BtwRequest,
    },
    /// Rebuild the slash-command registry from disk and atomically
    /// swap. Triggered by `/reload-plugins`.
    TriggerReloadPlugins,
    /// Reload the live `HookRegistry` from the latest `RuntimeConfig`
    /// snapshot. Triggered by `/hooks reload`.
    /// Slash commands run only at turn boundaries (the dispatch loop
    /// `drain_active_turn`s before invoking them), so
    /// PreToolUse/PostToolUse for an in-flight call cannot see
    /// different hook sets.
    TriggerReloadHooks,
}

fn slash_unavailable_in_session_message(name: &str) -> String {
    format!("/{name} isn't available in this session.")
}

/// Split `/<name> <args>` into `(name, args)`. Returns `None` when
/// `text` does not start with `/` or has no name. Whitespace-trimmed.
/// Convert a `coco_context::DiffStats` to the wire payload variant.
/// Centralised so the single-row and batch paths emit identically.
fn diff_stats_to_payload(stats: coco_context::DiffStats) -> coco_types::RewindDiffStatsPayload {
    let file_paths: Vec<String> = stats
        .files_changed
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    coco_types::RewindDiffStatsPayload {
        insertions: stats.insertions,
        deletions: stats.deletions,
        file_paths,
    }
}

fn parse_slash_command(text: &str) -> Option<(&str, &str)> {
    let stripped = text.trim().strip_prefix('/')?;
    if stripped.is_empty() {
        return None;
    }
    Some(match stripped.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim_start()),
        None => (stripped, ""),
    })
}

fn format_slash_command_metadata(name: &str, args: &str) -> String {
    let mut body =
        format!("<command-message>{name}</command-message>\n<command-name>/{name}</command-name>");
    let trimmed_args = args.trim();
    if !trimmed_args.is_empty() {
        body.push_str(&format!("\n<command-args>{trimmed_args}</command-args>"));
    }
    body
}

fn create_slash_metadata_message(metadata: &str) -> coco_messages::Message {
    let attachment = coco_messages::AttachmentMessage::api(
        coco_types::AttachmentKind::SlashCommandMetadata,
        coco_messages::LlmMessage::user_text(metadata),
    );
    coco_messages::Message::Attachment(attachment)
}

async fn emit_resume_plan_ui_state(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    restored_v1_todos: Option<Vec<coco_types::TodoRecord>>,
) {
    let cfg = session.current_engine_config().await;
    let goal = goal_command::restore_goal_from_history(
        &plan
            .prior_messages
            .iter()
            .cloned()
            .map(std::sync::Arc::new)
            .collect::<Vec<_>>(),
        session.app_state(),
        &session.hook_registry(),
        session.session_usage_snapshot().await.totals.output_tokens,
        goal_command::GoalGate {
            hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
            trust_rejected: workspace_trust_rejected(),
        },
    )
    .await;

    // Bulk resume hydration mirrors the startup `--resume` path:
    // reset UI-only state first, then replace transcript scrollback in
    // one pass instead of replaying thousands of individual appends.
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionResetForResume {
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
            },
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::HistoryReplaced {
                messages: plan
                    .prior_messages
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect(),
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
                reason: coco_types::HistoryReplaceReason::Hydrate,
            },
        ))
        .await;
    if let Some(todos) = restored_v1_todos {
        let mut todos_by_agent = HashMap::new();
        if !todos.is_empty() {
            todos_by_agent.insert(plan.session_id.to_string(), todos);
        }
        let _ = event_tx
            .send(CoreEvent::Protocol(
                coco_types::ServerNotification::TaskPanelChanged(
                    coco_types::TaskPanelChangedParams {
                        plan_tasks: Vec::new(),
                        todos_by_agent,
                        expanded_view: coco_types::ExpandedView::None,
                        verification_nudge_pending: false,
                        // Unordered producer: always applied, never
                        // advances the consumer's high-water mark.
                        generation: 0,
                    },
                ),
            ))
            .await;
    }
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionUsageUpdated(Box::new(
                session.session_usage_snapshot().await,
            )),
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
    session
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

async fn emit_resume_plan_ui_state_for_runtime(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let restored_v1_todos = if session
        .runtime_config()
        .features
        .enabled(coco_types::Feature::TaskV2)
    {
        None
    } else {
        latest_todo_write_todos(&plan.prior_messages)
    };
    if let Some(todos) = restored_v1_todos.clone() {
        session
            .seed_todo_list_snapshot(plan.session_id.to_string(), todos)
            .await;
    }
    emit_resume_plan_ui_state(plan, session, event_tx, restored_v1_todos).await;
}

#[derive(serde::Deserialize)]
struct TodoWriteTranscriptInput {
    todos: Vec<coco_types::TodoRecord>,
}

fn todo_write_store_snapshot(todos: Vec<coco_types::TodoRecord>) -> Vec<coco_types::TodoRecord> {
    if !todos.is_empty() && todos.iter().all(|todo| todo.status == "completed") {
        Vec::new()
    } else {
        todos
    }
}

fn latest_todo_write_todos(messages: &[Message]) -> Option<Vec<coco_types::TodoRecord>> {
    for message in messages.iter().rev() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        for part in content.iter().rev() {
            let AssistantContent::ToolCall(call) = part else {
                continue;
            };
            if call.tool_name != coco_types::ToolName::TodoWrite.as_str() {
                continue;
            }
            match serde_json::from_value::<TodoWriteTranscriptInput>(call.input.clone()) {
                Ok(input) => return Some(todo_write_store_snapshot(input.todos)),
                Err(err) => {
                    warn!(error = %err, "failed to restore TodoWrite state from transcript input");
                    return None;
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_resume(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let target = args.trim();
    if target.is_empty() {
        let manager = Arc::clone(runtime.session_manager());
        let sessions = match tokio::task::spawn_blocking(move || manager.list()).await {
            Ok(Ok(sessions)) => sessions,
            Ok(Err(err)) => {
                emit_slash_text(
                    event_tx,
                    "resume",
                    args,
                    &format!("Failed to list sessions: {err}"),
                )
                .await;
                return SlashOutcome::Handled;
            }
            Err(err) => {
                emit_slash_text(
                    event_tx,
                    "resume",
                    args,
                    &format!("Session listing task failed: {err}"),
                )
                .await;
                return SlashOutcome::Handled;
            }
        };
        let sessions = sessions
            .into_iter()
            .filter_map(session_to_sdk_summary)
            .collect();
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenSessionBrowser {
                sessions,
            }))
            .await;
        return SlashOutcome::Handled;
    }

    match load_resume_plan_for_target(session, target).await {
        Ok(plan) => {
            tracing::info!(
                target: "coco_cli::resume",
                session_id = %plan.session_id,
                source_session_id = %plan.source_session_id,
                prior_messages = plan.prior_messages.len(),
                "slash resume: hydrating session",
            );
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "resume",
                args,
                session,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_factory,
                process_runtime,
                runtime_reload_subscriptions,
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode to the resumed session. Runs at a
            // turn boundary, so the env flip is
            // observed by the next prompt assembly.
            if let Some(warning) = coco_cli::coordinator_mode_resume::reconcile_on_resume(
                plan.conversation.mode.as_deref(),
                &runtime.runtime_config().features,
            ) {
                emit_slash_text(event_tx, "resume", args, warning).await;
            }
        }
        Err(err) => {
            emit_slash_text(
                event_tx,
                "resume",
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
        }
    }

    SlashOutcome::Handled
}

#[allow(clippy::too_many_arguments)]
async fn switch_to_resume_plan_through_app_server(
    plan: &ResumePlan,
    command_name: &str,
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> bool {
    match apply_resume_plan_through_app_server(
        plan,
        session,
        current_session,
        event_tx,
        local_app_server_bridge,
        runtime_factory,
        process_runtime,
        runtime_reload_subscriptions,
    )
    .await
    {
        Ok(()) => true,
        Err(err) => {
            emit_slash_text(
                event_tx,
                command_name,
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_resume_plan_through_app_server(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> anyhow::Result<()> {
    let old_session_id = session.current_typed_session_id().await;
    if old_session_id == plan.session_id {
        local_app_server_bridge
            .install_session_runtime(session.clone())
            .await;
        hydrate_runtime_for_resume_plan(session, &plan.session_id, &plan.prior_messages).await;
        local_app_server_bridge.ensure_interactive_surface(plan.session_id.clone())?;
        local_app_server_bridge
            .start_passive_event_pump(plan.session_id.clone(), event_tx.clone())?;
        emit_resume_plan_ui_state_for_runtime(plan, session, event_tx).await;
        return Ok(());
    }

    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;

    let make_runtime_factory = || {
        let runtime_factory = runtime_factory.clone();
        let process_runtime = Arc::clone(process_runtime);
        let cwd = plan.cwd.clone();
        let event_tx = event_tx.clone();
        let session_id = plan.session_id.clone();
        let prior_messages = plan.prior_messages.clone();
        async move {
            build_runtime_for_resume_plan(
                runtime_factory,
                session_id,
                prior_messages,
                process_runtime,
                cwd,
                event_tx,
            )
            .await
        }
    };

    let replacement = local_app_server_bridge
        .replace_session_runtime(
            old_session_id.clone(),
            plan.session_id.clone(),
            make_runtime_factory(),
        )
        .await?;
    let new_session = match replacement {
        Some((session, _surface_id)) => session,
        None => {
            local_app_server_bridge
                .replace_detached_session_runtime(
                    old_session_id,
                    plan.session_id.clone(),
                    make_runtime_factory(),
                )
                .await?
        }
    };

    local_app_server_bridge
        .install_session_runtime(new_session.clone())
        .await;
    {
        let mut current = current_session.write().await;
        *current = new_session.clone();
    }
    runtime_reload_subscriptions
        .lock()
        .await
        .install_for_session(&new_session)
        .await;
    local_app_server_bridge.ensure_interactive_surface(plan.session_id.clone())?;
    local_app_server_bridge.start_passive_event_pump(plan.session_id.clone(), event_tx.clone())?;
    emit_resume_plan_ui_state_for_runtime(plan, &new_session, event_tx).await;
    Ok(())
}

async fn build_runtime_for_resume_plan(
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    session_id: coco_types::SessionId,
    prior_messages: Vec<coco_messages::Message>,
    process_runtime: Arc<ProcessRuntime>,
    cwd: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id.clone(), cwd.clone())
        .await?;
    let runtime = &session;
    runtime.install_side_query_event_tx(event_tx.clone()).await;
    hydrate_runtime_for_resume_plan(&session, &session_id, &prior_messages).await;

    let lsp_handle = coco_cli::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime,
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    install_session_late_binds(
        session.clone(),
        &cwd,
        None,
        lsp_handle,
        Some(event_tx.clone()),
    )
    .await?;
    coco_cli::session_bootstrap::bootstrap_session_mcp(
        &session, &cwd, None, /*await_connect*/ false,
    )
    .await;
    coco_cli::leader_inbox_poller::install_leader(session.clone(), None).await;

    runtime.fire_session_start_hooks("resume").await;
    Ok(session)
}

async fn build_runtime_for_clear(
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    session_id: coco_types::SessionId,
    permissions: coco_types::LiveToolPermissionState,
    rewind_messages: Option<Vec<Arc<Message>>>,
    process_runtime: Arc<ProcessRuntime>,
    cwd: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id.clone(), cwd.clone())
        .await?;
    let runtime = &session;
    runtime.install_side_query_event_tx(event_tx.clone()).await;
    {
        let mut app_state = runtime.app_state().write().await;
        app_state.permissions = permissions;
    }
    runtime
        .seed_pre_clear_rewind_messages(rewind_messages)
        .await;

    let lsp_handle = coco_cli::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime,
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    install_session_late_binds(
        session.clone(),
        &cwd,
        None,
        lsp_handle,
        Some(event_tx.clone()),
    )
    .await?;
    coco_cli::session_bootstrap::bootstrap_session_mcp(
        &session, &cwd, None, /*await_connect*/ false,
    )
    .await;
    coco_cli::leader_inbox_poller::install_leader(session.clone(), None).await;

    Ok(session)
}

async fn hydrate_runtime_for_resume_plan(
    session: &crate::session_runtime::SessionHandle,
    session_id: &coco_types::SessionId,
    prior_messages: &[coco_messages::Message],
) {
    let runtime = &session;
    {
        let mut history = runtime.history().lock().await;
        history.clear();
        for message in prior_messages.iter().cloned() {
            history.push(message);
        }
    }
    runtime
        .seed_transcript_dedup(prior_messages.iter().filter_map(|m| m.uuid().copied()))
        .await;
    runtime
        .seed_tool_result_replacement_state(prior_messages, session_id, None)
        .await;

    if prior_messages.is_empty() {
        return;
    }
    let cfg = runtime.current_engine_config().await;
    let messages = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();
    let goal = goal_command::restore_goal_from_history(
        &messages,
        runtime.app_state(),
        &runtime.hook_registry(),
        runtime.session_usage_snapshot().await.totals.output_tokens,
        goal_command::GoalGate {
            hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
            trust_rejected: false,
        },
    )
    .await;
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

/// `/branch` (alias `/fork`) — fork the current conversation at this point
/// into a NEW session and switch to it live.
/// Copies the current transcript to a fresh uuid via `fork_conversation` (the
/// same primitive `--fork-session` uses), then hydrates the runtime onto the
/// fork through local AppServer `session/resume`. The original session is left
/// untouched on disk.
#[allow(clippy::too_many_arguments)]
async fn dispatch_branch(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let custom_title = args.trim().to_string();
    let source_id = runtime.current_typed_session_id().await;
    if source_id.as_str().is_empty() {
        emit_slash_text(event_tx, "branch", "", "No active session to branch from.").await;
        return SlashOutcome::Handled;
    }
    let working_dir = runtime.original_cwd().clone();
    let memory_base = runtime.session_manager().memory_base().to_path_buf();
    let plan = tokio::task::spawn_blocking(move || -> anyhow::Result<ResumePlan> {
        let store = coco_session::TranscriptStore::new(std::sync::Arc::new(
            coco_paths::ProjectPaths::new(memory_base, &working_dir),
        ));
        let source_path = store.transcript_path(source_id.as_str());
        if !coco_session::recovery::can_resume_session(&source_path) {
            anyhow::bail!(
                "nothing to branch yet — send a message first so there's a conversation to fork"
            );
        }
        let dest_id = coco_types::SessionId::generate();
        let dest_path = store.transcript_path(dest_id.as_str());
        coco_session::recovery::fork_conversation(&source_path, &dest_path, dest_id.as_str())
            .map_err(|e| {
                anyhow::anyhow!(
                    "fork copy {} → {} failed: {e}",
                    source_path.display(),
                    dest_path.display()
                )
            })?;
        let conversation = coco_session::recovery::load_conversation_for_resume(&dest_path)?;
        let prior_messages = conversation.messages.clone();
        Ok(ResumePlan {
            session_id: dest_id,
            source_session_id: source_id,
            source_path,
            destination_path: dest_path,
            cwd: working_dir,
            prior_messages,
            conversation,
            is_fork: true,
        })
    })
    .await;
    match plan {
        Ok(Ok(plan)) => {
            let new_id = plan.session_id.to_string();
            let source_id = plan.source_session_id.to_string();
            // Derive the fork title: explicit arg, else the first user prompt
            // (truncated), suffixed " (Branch)" — branch.ts. Done
            // BEFORE hydrate moves `plan`.
            let base_title = if custom_title.is_empty() {
                first_user_prompt_title(&plan.prior_messages)
            } else {
                Some(custom_title)
            };
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "branch",
                args,
                session,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_factory,
                process_runtime,
                runtime_reload_subscriptions,
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode onto the fork, same as /resume — the
            // fork inherits the source's persisted mode. Runs at a turn
            // boundary so the next prompt assembly observes the flip.
            if let Some(warning) = coco_cli::coordinator_mode_resume::reconcile_on_resume(
                plan.conversation.mode.as_deref(),
                &runtime.runtime_config().features,
            ) {
                emit_slash_text(event_tx, "branch", args, warning).await;
            }
            // The fork is now the live session, so session/rename titles it.
            if let Some(base) = base_title {
                let title = format!("{base} (Branch)");
                if let Err(e) = local_app_server_bridge
                    .client()
                    .session_rename(
                        local_app_server_bridge.handler(),
                        coco_types::SessionRenameParams { name: title },
                    )
                    .await
                {
                    warn!(error = %e, "failed to set /branch fork title");
                }
            }
            emit_slash_text(
                event_tx,
                "branch",
                args,
                &format!(
                    "Branched into a new session ({new_id}). \
                     To return to the original, /resume {source_id}."
                ),
            )
            .await;
        }
        Ok(Err(e)) => {
            emit_slash_text(event_tx, "branch", "", &format!("Failed to branch: {e}")).await;
        }
        Err(e) => {
            emit_slash_text(event_tx, "branch", "", &format!("Branch task failed: {e}")).await;
        }
    }
    SlashOutcome::Handled
}

/// Derive a short title from the first user message's text (first line,
/// truncated), for naming a `/branch` fork when no explicit title is given.
fn first_user_prompt_title(messages: &[coco_messages::Message]) -> Option<String> {
    let text = messages.iter().find_map(|m| {
        matches!(m, coco_messages::Message::User(_))
            .then(|| coco_messages::wrapping::extract_text_from_message(m))
            .filter(|t| !t.trim().is_empty())
    })?;
    let first_line = text.trim().lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let truncated: String = first_line.chars().take(40).collect();
    Some(truncated)
}

async fn load_resume_plan_for_target(
    session: &crate::session_runtime::SessionHandle,
    target: &str,
) -> anyhow::Result<ResumePlan> {
    let runtime = session;
    let manager = Arc::clone(runtime.session_manager());
    let target = target.to_string();
    // Project root for THIS runtime. Resume targets whose project root differs
    // from this are cross-project: refuse rather than mid-flight re-point the
    // runtime's transcript store and project-scoped services.
    let runtime_project_root = runtime.project_root().clone();
    tokio::task::spawn_blocking(move || {
        let session = match manager.resume(&target) {
            Ok(session) => session,
            Err(id_err) => {
                resolve_resume_target_by_title(&manager, &target, &runtime_project_root, &id_err)?
            }
        };
        let session_project_root = coco_cli::paths::resolve_project_root(&session.working_dir);
        if session_project_root != runtime_project_root {
            anyhow::bail!(
                "session {} lives under project {} but this runtime is at {} — \
                 cross-project /resume is not supported. cd to the source \
                 project and try again.",
                session.id,
                session_project_root.display(),
                runtime_project_root.display(),
            );
        }
        let transcript_path = coco_session::TranscriptStore::new(coco_cli::paths::project_paths(
            &session.working_dir,
        ))
        .transcript_path(&session.id);
        if !coco_session::recovery::can_resume_session(&transcript_path) {
            anyhow::bail!(
                "transcript at {} is empty or unreadable; nothing to resume",
                transcript_path.display()
            );
        }
        let conversation = coco_session::recovery::load_conversation_for_resume(&transcript_path)?;
        let prior_messages = conversation.messages.clone();
        let session_id = coco_types::SessionId::try_new(session.id.clone())
            .map_err(|e| anyhow::anyhow!("invalid session id '{}': {e}", session.id))?;
        Ok(ResumePlan {
            session_id: session_id.clone(),
            source_session_id: session_id,
            source_path: transcript_path.clone(),
            destination_path: transcript_path,
            cwd: session.working_dir,
            prior_messages,
            conversation,
            is_fork: false,
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("resume task failed: {err}"))?
}

/// Case-insensitive exact resolve of `/resume <name>` when the
/// argument doesn't match any session id directly.
/// Returns the unique session on a 1-match (after project filtering),
/// or bails with a diagnostic listing the top-N candidates. The
/// project filter keeps cross-project matches from leaking into the
/// "did you mean X" hint — it would be misleading to suggest a
/// session the runtime can't actually resume.
fn resolve_resume_target_by_title(
    manager: &coco_session::SessionManager,
    target: &str,
    runtime_project_root: &std::path::Path,
    id_err: &coco_session::SessionError,
) -> anyhow::Result<coco_session::Session> {
    let mut matches = manager
        .find_by_title(target, true)?
        .into_iter()
        .filter(|s| same_project(&s.working_dir, runtime_project_root))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => anyhow::bail!("no session found for id or title '{target}': {id_err}"),
        1 => Ok(matches.remove(0)),
        n => {
            const MAX_CANDIDATES_SHOWN: usize = 5;
            let lines: Vec<String> = matches
                .iter()
                .take(MAX_CANDIDATES_SHOWN)
                .map(|s| format!("  {}  {}", s.id, s.title.as_deref().unwrap_or("(untitled)")))
                .collect();
            let more = if n > MAX_CANDIDATES_SHOWN {
                format!("\n  …and {} more", n - MAX_CANDIDATES_SHOWN)
            } else {
                String::new()
            };
            anyhow::bail!(
                "ambiguous resume target '{target}' — {n} sessions match. \
                 Re-run with a session id:\n{}{more}",
                lines.join("\n"),
            )
        }
    }
}

fn same_project(session_cwd: &std::path::Path, runtime_root: &std::path::Path) -> bool {
    coco_cli::paths::resolve_project_root(session_cwd) == runtime_root
}

fn session_to_sdk_summary(session: coco_session::Session) -> Option<coco_types::SdkSessionSummary> {
    let session_id = match coco_types::SessionId::try_new(session.id.clone()) {
        Ok(id) => id,
        Err(err) => {
            warn!(
                session_id = %session.id,
                error = %err,
                "skipping session with invalid id in resume browser"
            );
            return None;
        }
    };
    Some(coco_types::SdkSessionSummary {
        session_id,
        model: session.model,
        cwd: session.working_dir.to_string_lossy().into_owned(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        title: session.title,
        message_count: session.message_count,
        total_tokens: session.total_tokens,
    })
}

fn session_plans_dir(
    config_home: &std::path::Path,
    project_dir: Option<&std::path::Path>,
    plans_directory_setting: Option<&str>,
) -> std::path::PathBuf {
    coco_context::resolve_plans_directory(config_home, project_dir, plans_directory_setting)
}

fn session_plan_file_path(
    config_home: &std::path::Path,
    project_dir: Option<&std::path::Path>,
    plans_directory_setting: Option<&str>,
    session_id: &coco_types::SessionId,
) -> std::path::PathBuf {
    let plans_dir = session_plans_dir(config_home, project_dir, plans_directory_setting);
    coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None)
}

async fn runtime_session_plan_file_path(
    session: &crate::session_runtime::SessionHandle,
) -> std::path::PathBuf {
    let runtime = session;
    let session_id = runtime.current_typed_session_id().await;
    session_plan_file_path(
        runtime.config_home(),
        runtime.runtime_config().paths.project_dir.as_deref(),
        runtime
            .runtime_config()
            .settings
            .merged
            .plans_directory
            .as_deref(),
        &session_id,
    )
}

async fn prepare_external_editor_request(
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    request: PendingEditorRequest,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let request_id = uuid::Uuid::new_v4().to_string();
    pending_editor_requests.insert(request_id.clone(), request);
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::ExternalEditorPrepare {
            request_id,
        }))
        .await;
}

/// Decision-tree classifier for sentinel-prefixed handler output.
/// Pure, no side-effects — used by `dispatch_slash_command` to decide
/// whether the Text result actually carries a request to fire a real
/// feature (compact / dream / summary / rename / tag). Extracted as a
/// free function so the routing logic is testable without a full
/// `SessionRuntime`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SentinelTrigger {
    Compact {
        custom_instructions: Option<String>,
    },
    Dream,
    Summary,
    Cost,
    Status,
    Goal {
        request: coco_commands::GoalCommandRequest,
    },
    Rename {
        request: coco_commands::ParsedRename,
    },
    Tag {
        tag: String,
    },
    AddDir {
        path: String,
    },
    ReloadPlugins,
    ReloadHooks,
    Btw {
        request: coco_commands::handlers::btw::BtwRequest,
    },
}

fn classify_sentinel_trigger(text: &str) -> Option<SentinelTrigger> {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    use coco_commands::handlers::compact::parse_compact_sentinel;
    use coco_commands::handlers::dream::DREAM_SENTINEL;
    use coco_commands::handlers::dream::parse_dream_sentinel;
    use coco_commands::handlers::summary::SUMMARY_SENTINEL;
    use coco_commands::handlers::summary::parse_summary_sentinel;
    if text.starts_with(COMPACT_SENTINEL) {
        let req = parse_compact_sentinel(text)?;
        let trimmed = req.custom_instructions.trim();
        let custom_instructions = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        return Some(SentinelTrigger::Compact {
            custom_instructions,
        });
    }
    if text.starts_with(DREAM_SENTINEL) && parse_dream_sentinel(text).is_some() {
        return Some(SentinelTrigger::Dream);
    }
    if text.starts_with(SUMMARY_SENTINEL) && parse_summary_sentinel(text).is_some() {
        return Some(SentinelTrigger::Summary);
    }
    if text.starts_with(coco_commands::handlers::cost::COST_SENTINEL)
        && coco_commands::handlers::cost::parse_cost_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::Cost);
    }
    if text.starts_with(coco_commands::STATUS_SENTINEL)
        && coco_commands::parse_status_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::Status);
    }
    if text.starts_with(coco_commands::GOAL_SENTINEL)
        && let Some(request) = coco_commands::parse_goal_sentinel(text)
    {
        return Some(SentinelTrigger::Goal { request });
    }
    if text.starts_with(coco_commands::RENAME_SENTINEL)
        && let Some(request) = coco_commands::parse_rename_sentinel(text)
    {
        return Some(SentinelTrigger::Rename { request });
    }
    if text.starts_with(coco_commands::TAG_SENTINEL)
        && let Some(tag) = coco_commands::parse_tag_sentinel(text)
    {
        return Some(SentinelTrigger::Tag { tag });
    }
    if text.starts_with(coco_commands::ADD_DIR_SENTINEL)
        && let Some(path) = coco_commands::parse_add_dir_sentinel(text)
    {
        return Some(SentinelTrigger::AddDir { path });
    }
    if text.starts_with(coco_commands::RELOAD_PLUGINS_SENTINEL)
        && coco_commands::parse_reload_plugins_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::ReloadPlugins);
    }
    if text.starts_with(coco_commands::RELOAD_HOOKS_SENTINEL)
        && coco_commands::parse_reload_hooks_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::ReloadHooks);
    }
    if text.starts_with(coco_commands::handlers::btw::BTW_SENTINEL)
        && let Some(request) = coco_commands::handlers::btw::parse_btw_sentinel(text)
    {
        return Some(SentinelTrigger::Btw { request });
    }
    None
}

/// Mutating subcommand of `/permissions`. `None` for the read-only
/// (`list` / no-arg) path, which falls through to the registry handler.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionsMutation {
    Allow(String),
    Deny(String),
    Reset,
}

fn parse_permissions_mutation(args: &str) -> Option<PermissionsMutation> {
    let trimmed = args.trim();
    if trimmed == "reset" {
        return Some(PermissionsMutation::Reset);
    }
    if let Some(tool) = trimmed.strip_prefix("allow ") {
        let tool = tool.trim();
        if tool.is_empty() {
            return None;
        }
        return Some(PermissionsMutation::Allow(tool.to_string()));
    }
    if let Some(tool) = trimmed.strip_prefix("deny ") {
        let tool = tool.trim();
        if tool.is_empty() {
            return None;
        }
        return Some(PermissionsMutation::Deny(tool.to_string()));
    }
    None
}

/// Resolve `/<name> <args>` through the registry and route the result.
/// What's left for the caller to do after [`handle_slash_outcome`]
/// has processed an outcome.
/// The 9 `SlashOutcome::Trigger*` variants and `Handled` all fold to
/// [`SlashFollowup::Done`] inside the helper — caller has nothing
/// further to do (TUI may `continue`, palette / SDK may simply
/// no-op). The remaining two cases differ per call site and are
/// surfaced as variants here so each site renders the right
/// notification / continuation.
#[derive(Debug)]
enum SlashFollowup {
    /// Outcome fully handled inside the helper. Caller continues.
    Done,
    /// Registry / palette did not recognise the command. Caller
    /// decides: typed input falls through to the LLM as raw text;
    /// palette logs; SDK emits `SlashCommandStatusKind::NoHandler`.
    NotFound,
    /// Command expanded to a model prompt. Caller spawns a turn
    /// (palette / SDK) or substitutes `effective_content` (typed input).
    RunEngine {
        content: String,
        metadata: Option<String>,
        thinking_level: Option<coco_types::ThinkingLevel>,
        model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
    },
}

struct SlashEnginePrompt {
    content: String,
    metadata: Option<String>,
    thinking_level: Option<coco_types::ThinkingLevel>,
    model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
}

struct LocalRuntimeControlContext<'a> {
    current_session: &'a SharedSessionHandle,
    runtime_reload_subscriptions: &'a Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    runtime_factory: &'a crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &'a Arc<ProcessRuntime>,
    cwd: &'a std::path::Path,
    turn_done_tx: &'a mpsc::Sender<uuid::Uuid>,
}

/// Process a [`SlashOutcome`] into a [`SlashFollowup`] for the
/// caller. Handles the trigger variants in one place so the dispatch
/// arms in `run_agent_loop` no longer triple-duplicate the same match.
async fn handle_slash_outcome(
    outcome: SlashOutcome,
    session: &crate::session_runtime::SessionHandle,
    control_context: &LocalRuntimeControlContext<'_>,
    event_tx: &mpsc::Sender<CoreEvent>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
) -> SlashFollowup {
    match outcome {
        SlashOutcome::Handled => SlashFollowup::Done,
        SlashOutcome::NotFound => SlashFollowup::NotFound,
        SlashOutcome::RunEngine {
            content,
            metadata,
            thinking_level,
            model_runtime_source,
        } => SlashFollowup::RunEngine {
            content,
            metadata,
            thinking_level,
            model_runtime_source,
        },
        SlashOutcome::RunForkSkill { name, args } => {
            run_fork_skill(session, event_tx, &name, &args, active_turn).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerCompact {
            custom_instructions,
        } => {
            run_manual_compact(
                session,
                event_tx,
                local_app_server_bridge,
                custom_instructions,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerClear => {
            run_clear_conversation(
                session,
                control_context,
                active_turn,
                event_tx,
                local_app_server_bridge,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerDream => {
            run_dream_consolidation(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerSummary => {
            run_session_memory_force(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerBtw { request } => {
            run_side_question(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
                request,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::ShowCost => {
            run_show_cost(event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::ShowStatus => {
            run_show_status(event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerGoal { request } => run_goal_command(session, event_tx, request).await,
        SlashOutcome::TriggerRename { request } => {
            run_session_rename(session, event_tx, local_app_server_bridge, request).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerTag { tag } => {
            run_session_tag(session, event_tx, local_app_server_bridge, &tag).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerAddDir { path } => {
            let _ = apply_session_add_directory(&path, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerOpenPlanEditor { path } => {
            prepare_external_editor_request(
                pending_editor_requests,
                PendingEditorRequest::Plan { path },
                event_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerReloadPlugins => {
            run_reload_plugins(session, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerReloadHooks => {
            run_reload_hooks(event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_idle_command_queue(
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    cwd: &std::path::Path,
) {
    if active_turn.lock().await.is_some() {
        return;
    }

    drain_queued_slash_commands(
        session,
        current_session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        pending_editor_requests,
        title_gen_attempted,
        turn_done_tx,
        runtime_reload_subscriptions,
        runtime_factory,
        process_runtime,
        cwd,
    )
    .await;

    if active_turn.lock().await.is_none() {
        spawn_command_queue_turn(session, event_tx, active_turn, turn_done_tx).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn drain_queued_slash_commands(
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    cwd: &std::path::Path,
) {
    let runtime = &session;
    while let Some(cmd) = runtime
        .command_queue()
        .dequeue_first_matching(|c| c.is_slash_command && c.agent_id.is_none())
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                id: cmd.id.to_string(),
            }))
            .await;
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
                queued: runtime.command_queue().len().await as i32,
            }))
            .await;

        let Some((name, args)) = parse_slash_command(&cmd.prompt) else {
            continue;
        };
        let outcome = dispatch_slash_command(
            name,
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_factory,
            process_runtime,
            runtime_reload_subscriptions,
        )
        .await;
        match outcome {
            SlashOutcome::Handled => {}
            SlashOutcome::NotFound => {
                emit_slash_status(event_tx, name, args, SlashCommandStatusKind::NoHandler).await;
            }
            SlashOutcome::RunEngine {
                content,
                metadata,
                thinking_level,
                model_runtime_source,
            } => {
                let session_id = runtime.current_typed_session_id().await;
                spawn_slash_run_engine_turn(
                    SlashEnginePrompt {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    },
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    title_gen_attempted,
                    turn_done_tx,
                    &session_id,
                )
                .await;
                break;
            }
            SlashOutcome::RunForkSkill { name, args } => {
                run_fork_skill(session, event_tx, &name, &args, active_turn).await;
            }
            SlashOutcome::TriggerCompact {
                custom_instructions,
            } => {
                run_manual_compact(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    custom_instructions,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerClear => {
                let control_context = LocalRuntimeControlContext {
                    current_session,
                    runtime_reload_subscriptions,
                    runtime_factory,
                    process_runtime,
                    cwd,
                    turn_done_tx,
                };
                run_clear_conversation(
                    session,
                    &control_context,
                    active_turn,
                    event_tx,
                    local_app_server_bridge,
                )
                .await;
            }
            SlashOutcome::TriggerDream => {
                run_dream_consolidation(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerSummary => {
                run_session_memory_force(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerBtw { request } => {
                run_side_question(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                    request,
                )
                .await;
            }
            SlashOutcome::ShowCost => {
                run_show_cost(event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::ShowStatus => {
                run_show_status(event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::TriggerGoal { request } => {
                if let SlashFollowup::RunEngine {
                    content,
                    metadata,
                    thinking_level,
                    model_runtime_source,
                } = run_goal_command(session, event_tx, request).await
                {
                    let session_id = runtime.current_typed_session_id().await;
                    spawn_slash_run_engine_turn(
                        SlashEnginePrompt {
                            content,
                            metadata,
                            thinking_level,
                            model_runtime_source,
                        },
                        session,
                        event_tx,
                        local_app_server_bridge,
                        active_turn,
                        title_gen_attempted,
                        turn_done_tx,
                        &session_id,
                    )
                    .await;
                    break;
                }
            }
            SlashOutcome::TriggerRename { request } => {
                run_session_rename(session, event_tx, local_app_server_bridge, request).await;
            }
            SlashOutcome::TriggerTag { tag } => {
                run_session_tag(session, event_tx, local_app_server_bridge, &tag).await;
            }
            SlashOutcome::TriggerAddDir { path } => {
                let _ = apply_session_add_directory(&path, event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::TriggerOpenPlanEditor { path } => {
                prepare_external_editor_request(
                    pending_editor_requests,
                    PendingEditorRequest::Plan { path },
                    event_tx,
                )
                .await;
            }
            SlashOutcome::TriggerReloadPlugins => {
                run_reload_plugins(session, event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::TriggerReloadHooks => {
                run_reload_hooks(event_tx, local_app_server_bridge).await;
            }
        }
    }
}

async fn background_all_tasks_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> Result<Vec<String>, coco_app_server_client::ClientError> {
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    local_app_server_bridge
        .client()
        .background_all_tasks(local_app_server_bridge.handler())
        .await
        .map(|result| result.task_ids)
}

async fn spawn_command_queue_turn(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    let runtime = session;
    let Some(first) = runtime
        .command_queue()
        .dequeue_first_matching(|c| !c.is_slash_command && c.agent_id.is_none())
        .await
    else {
        return;
    };
    let first_priority = first.priority;
    let first_origin = first.origin.clone();
    let mut queued = vec![first];
    let mut rest = runtime
        .command_queue()
        .dequeue_matching(|c| {
            !c.is_slash_command
                && c.agent_id.is_none()
                && c.priority == first_priority
                && c.origin == first_origin
        })
        .await;
    queued.append(&mut rest);

    let ids: Vec<String> = queued.iter().map(|cmd| cmd.id.to_string()).collect();
    let messages = {
        let mut h = runtime.history().lock().await;
        let event_tx_opt = Some(event_tx.clone());
        for cmd in &queued {
            coco_query::history_sync::history_push_and_emit(
                &mut h,
                coco_query::queued_command_to_message(cmd),
                &event_tx_opt,
            )
            .await;
        }
        h.to_vec()
    };
    for id in ids {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                id,
            }))
            .await;
    }
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
            queued: runtime.command_queue().len().await as i32,
        }))
        .await;

    spawn_history_turn(messages, session, event_tx, active_turn, turn_done_tx).await;
}

async fn spawn_history_turn(
    messages: Vec<std::sync::Arc<coco_messages::Message>>,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    let session_id = session.current_typed_session_id().await;
    let history_override = match messages
        .iter()
        .map(|message| serde_json::to_value(message.as_ref()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(history_override) => history_override,
        Err(error) => {
            tracing::warn!(%error, "history turn AppServer serialization failed");
            return;
        }
    };

    let mut local_app_server_bridge = coco_cli::sdk_server::AppServerLocalBridge::new(Arc::new(
        coco_cli::sdk_server::SdkServerState::default(),
    ));
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(session_id.clone(), event_tx.clone())
    {
        tracing::warn!(%error, "history turn could not attach AppServer event pump");
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    let passive_surface = match monitor_client.subscribe_session(
        session_id.clone(),
        Some(0),
        coco_app_server::AttachSurfaceOptions::default(),
    ) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "history turn could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        prompt: String::new(),
        history_override,
        images: Vec::new(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "history turn AppServer turn/start failed");
            return;
        }
    };

    let turn_id = uuid::Uuid::new_v4();
    let turn_done_tx_t = turn_done_tx.clone();
    let protocol_turn_id = started.turn_id.clone();
    let interrupt_client = local_app_server_bridge.connect_local_client();
    let handler = local_app_server_bridge.handler().clone();
    let task = tokio::spawn(async move {
        let _bridge = local_app_server_bridge;
        let _done = TurnDoneGuard {
            turn_id,
            tx: turn_done_tx_t,
        };
        while let Some(envelope) = monitor_client.next_passive_event(&passive_surface).await {
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == protocol_turn_id
            {
                break;
            }
        }
    });
    *active_turn.lock().await = Some(ActiveTurn {
        id: turn_id,
        task,
        cancel: ActiveTurnCancel {
            client: interrupt_client,
            handler,
        },
    });
}

/// Spawn the per-turn engine task for a slash command that expanded
/// to a model prompt (`SlashFollowup::RunEngine`). Used by the
/// command-palette + SDK invocation paths; the typed-input path
/// substitutes `effective_content` instead so it keeps the outer
/// `user_message_id` from the original TUI submit.
/// The active-turn slot is installed inline (locking `active_turn`)
/// before this returns — callers can immediately start observing
/// `ActiveTurn` from a peer task without a TOCTOU window.
#[allow(clippy::too_many_arguments)]
async fn spawn_slash_run_engine_turn(
    prompt: SlashEnginePrompt,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    session_id: &coco_types::SessionId,
) {
    let SlashEnginePrompt {
        content,
        metadata,
        thinking_level,
        model_runtime_source,
    } = prompt;
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(session_id.clone(), event_tx.clone())
    {
        tracing::warn!(%error, "slash RunEngine could not refresh local AppServer event pump");
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    let passive_surface = match monitor_client.subscribe_session(
        session_id.clone(),
        Some(0),
        coco_app_server::AttachSurfaceOptions::default(),
    ) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "slash RunEngine could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        prompt: content,
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: metadata,
        model_selection: model_runtime_source_to_turn_start_selection(model_runtime_source),
        permission_mode: None,
        thinking_level,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "slash RunEngine AppServer turn/start failed");
            return;
        }
    };
    let turn_id = uuid::Uuid::new_v4();
    let session_t = session.clone();
    let title_gen_attempted_t = title_gen_attempted.clone();
    let turn_done_tx_t = turn_done_tx.clone();
    let session_id_t = session_id.clone();
    let protocol_turn_id = started.turn_id.clone();
    let auto_title_client = local_app_server_bridge.connect_local_client();
    let auto_title_handler = local_app_server_bridge.handler().clone();
    let task = tokio::spawn(async move {
        let _done = TurnDoneGuard {
            turn_id,
            tx: turn_done_tx_t,
        };
        let mut auto_title_client = Some(auto_title_client);
        let mut auto_title_handler = Some(auto_title_handler);
        while let Some(envelope) = monitor_client.next_passive_event(&passive_surface).await {
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == protocol_turn_id
            {
                if let (Some(client), Some(handler)) =
                    (auto_title_client.take(), auto_title_handler.take())
                {
                    maybe_spawn_auto_title(
                        &session_t,
                        &title_gen_attempted_t,
                        &session_id_t,
                        client,
                        handler,
                    )
                    .await;
                }
                break;
            }
        }
    });
    let interrupt_client = local_app_server_bridge.connect_local_client();
    let handler = local_app_server_bridge.handler().clone();
    *active_turn.lock().await = Some(ActiveTurn {
        id: turn_id,
        task,
        cancel: ActiveTurnCancel {
            client: interrupt_client,
            handler,
        },
    });
}

/// Record one inline slash-skill invocation. No-op for non-skill commands
/// (Local/overlay). Usage/telemetry pairing and the blocking-thread dispatch
/// live in `coco_skills::telemetry`. Keyed on the canonical name so aliases
/// collapse.
fn record_slash_skill_invocation(
    cmd: &coco_commands::RegisteredCommand,
    outcome: coco_skills::telemetry::SkillOutcome,
) {
    if !matches!(cmd.command_type, coco_types::CommandType::Prompt(_)) {
        return;
    }
    coco_skills::telemetry::record_invocation_outcome_detached(
        coco_config::global_config::config_home(),
        cmd.base.name.clone(),
        outcome,
    );
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_slash_command(
    name: &str,
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    // Runtime-state-aware commands intercepted before registry lookup:
    // their behavior depends on per-session state (session_id, plan
    // file, app_state) that the static registry can't carry.
    if matches!(name, "plan" | "planning") {
        return dispatch_plan(args, session, event_tx).await;
    }
    // `/permissions` (no arg) / `/permissions list` — open the tabbed
    // rule-editor overlay. The subcommand
    // forms (`allow` / `deny` / `reset`) keep their session-mutation
    // behavior below for power users + SDK parity.
    if name == "permissions" && matches!(args.trim(), "" | "list") {
        let payload = build_permissions_editor_payload(session).await;
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenPermissionsEditor {
                payload,
            }))
            .await;
        return SlashOutcome::Handled;
    }
    // `/permissions allow|deny|reset` — the registry handler can't
    // mutate the live `ToolAppState.permissions` base. Intercept the
    // mutating subcommands so they actually take effect.
    if name == "permissions"
        && let Some(outcome) =
            dispatch_permissions_mutation(args, event_tx, local_app_server_bridge).await
    {
        return outcome;
    }
    // `/color <name|default>` mutates session app state through local AppServer.
    // The registry handler is sync + has no runtime context, so the intercept
    // owns the teammate guard. Falls through to the registry (handler lists
    // colors) when args are empty.
    if name == "color"
        && let Some(outcome) = dispatch_color(args, event_tx, local_app_server_bridge).await
    {
        return outcome;
    }
    // `/clear` mutates runtime state. Keep it in the command layer so
    // typed and palette dispatch both run the real clear flow instead
    // of letting a registry text handler print without clearing.
    // Resolve aliases (`/reset`, `/new`) to the canonical `clear` name
    // first so they trigger the same flow instead of falling through to
    // the generic registry handler (`clear` declares aliases `['reset', 'new']`).
    let resolves_to_clear = runtime
        .current_command_registry()
        .await
        .get(name)
        .is_some_and(|cmd| cmd.base.name == "clear");
    if name == "clear" || resolves_to_clear {
        return SlashOutcome::TriggerClear;
    }
    if name == "context" {
        return dispatch_context(session, event_tx, local_app_server_bridge).await;
    }
    // `/config` (alias `/settings`) with no args opens the interactive settings
    // panel, reusing the same overlay as the `Ctrl+,` keybind. `config <key>
    // <value>` still falls through to the
    // registry text handler that writes settings.json.
    if matches!(name, "config" | "settings") && args.trim().is_empty() {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenSettings))
            .await;
        return SlashOutcome::Handled;
    }
    if name == "add-dir" {
        if args.trim().is_empty() {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenAddDirectory))
                .await;
            return SlashOutcome::Handled;
        }
        return dispatch_add_dir(args, session, event_tx, local_app_server_bridge).await;
    }
    // `/export` (no arg) opens the Markdown/JSON/Text format picker;
    // `/export <format>` renders the live conversation history in that format
    // and writes it to a file in the session's original cwd. The sync registry
    // handler has no runtime access (can't reach `MessageHistory`), so the real
    // export lives here..
    if name == "export" {
        if args.trim().is_empty() {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenExport))
                .await;
            return SlashOutcome::Handled;
        }
        return run_export(args, session, event_tx).await;
    }
    // `/branch` (alias `/fork`) forks the conversation at this point into a new
    // session and switches to it live. The sync registry handler only echoes
    // text — the real fork needs runtime + session-store access.
    if matches!(name, "branch" | "fork") {
        return dispatch_branch(
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_factory,
            process_runtime,
            runtime_reload_subscriptions,
        )
        .await;
    }
    if name == "resume" {
        return dispatch_resume(
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_factory,
            process_runtime,
            runtime_reload_subscriptions,
        )
        .await;
    }
    // `/copy [N]` — the picker + arg-parsing + lookback logic lives in
    // the TUI (only it owns the transcript view); the dispatcher just
    // hands off the raw args.
    if name == "copy" {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::CopyCommandRequested {
                args: args.to_string(),
            }))
            .await;
        return SlashOutcome::Handled;
    }
    // `/login [provider]` / `/logout [provider]` activate a configured OAuth
    // subscription against the SHARED `AuthService`, so the running session's
    // clients pick up the new token immediately. Handled here (not the
    // registry) because the auth flow lives in `app/cli` + needs the runtime.
    if name == "login" {
        // No-arg `/login` opens the provider picker (built CLI-side from the
        // OAuth-capable providers); `/login <provider>` logs in directly.
        if args.trim().is_empty() {
            let entries = build_login_entries(runtime.runtime_config());
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenLoginPicker { entries }))
                .await;
            return SlashOutcome::Handled;
        }
        return dispatch_provider_login(args, session, event_tx).await;
    }
    if name == "logout" {
        return dispatch_provider_logout(args, session, event_tx).await;
    }
    if name == "model" && !args.trim().is_empty() {
        let available_models = runtime
            .runtime_config()
            .settings
            .merged
            .available_models
            .as_deref();
        if let Some(resolved) = coco_commands::handlers::model::resolve_model(args)
            && !coco_config::is_model_allowed(
                resolved.provider,
                &resolved.model_id,
                available_models,
            )
        {
            let full_model_name = format!("{}/{}", resolved.provider, resolved.model_id);
            emit_slash_text(
                event_tx,
                "model",
                args,
                &format!(
                    "Model '{full_model_name}' is restricted by your organization's settings. Run /model to choose a different model.",
                ),
            )
            .await;
            return SlashOutcome::Handled;
        }
    }
    // `/rewind` flows through the standard registry → handler →
    // `DialogSpec::MessageSelector` → `OpenRewindPicker` path. The
    // handler ignores args; this dispatcher does only the
    // mechanical translation in the generic `DialogSpec` arm below.
    if name == "tasks" || name == "bashes" {
        run_tasks_command(session, event_tx, local_app_server_bridge, name, args).await;
        return SlashOutcome::Handled;
    }
    if name == "diff" && matches!(args.split_whitespace().next(), Some("session" | "turn")) {
        run_file_history_diff_command(session, event_tx, args).await;
        return SlashOutcome::Handled;
    }

    // Snapshot once per dispatch — `/reload-plugins` may swap the
    // registry mid-call, but the snapshot keeps the resolved command
    // valid through the handler's await chain.
    let registry_snapshot = runtime.current_command_registry().await;
    let Some(cmd) = registry_snapshot.get(name) else {
        return SlashOutcome::NotFound;
    };
    if !cmd.is_active() {
        let text = slash_unavailable_in_session_message(name);
        emit_slash_text(event_tx, name, args, &text).await;
        return SlashOutcome::Handled;
    }
    if cmd.base.name == "loop" {
        let cwd = runtime.current_cwd().read().await.clone();
        let prompt = coco_skills::bundled::loop_skill::prompt_for_command(
            args,
            runtime.original_cwd(),
            &cwd,
            runtime.runtime_config().loop_config.default_prompt_enabled,
            runtime.runtime_config().loop_config.dynamic_enabled,
            runtime
                .runtime_config()
                .loop_config
                .persistent_preamble_enabled,
            runtime
                .runtime_config()
                .features
                .enabled(coco_types::Feature::AgentTriggersRemote),
        );
        return SlashOutcome::RunEngine {
            content: prompt,
            metadata: Some(format_slash_command_metadata(&cmd.base.name, args)),
            thinking_level: None,
            model_runtime_source: None,
        };
    }
    // Fork-mode skills (`context: fork`) run as a subagent via the
    // installed `SkillHandle`, not by expanding inline into the main loop.
    // (`context === 'fork'` →
    // `executeForkedSlashCommand`); the handler path below only renders
    // inline expansions. `cmd.base.name` is canonical so the gate's
    // user-invoked check matches even when the user typed an alias.
    if let coco_types::CommandType::Prompt(data) = &cmd.command_type
        && data.context == coco_types::CommandContext::Fork
    {
        return SlashOutcome::RunForkSkill {
            name: cmd.base.name.clone(),
            args: args.to_string(),
        };
    }
    let Some(handler) = cmd.handler.as_ref() else {
        // Registered shell with no handler. For Prompt-type commands the
        // safe default is to fall through to the model so it sees the
        // raw `/foo` — safe default when the loader returns nothing.
        // Local-type commands genuinely need a handler; surface a
        // breadcrumb so the user knows the command is mis-wired.
        if matches!(cmd.command_type, coco_types::CommandType::Prompt(_)) {
            return SlashOutcome::NotFound;
        }
        emit_slash_status(event_tx, name, args, SlashCommandStatusKind::NoHandler).await;
        return SlashOutcome::Handled;
    };

    let result = match handler.execute_command(args).await {
        Ok(r) => {
            // Skill lifecycle telemetry for the INLINE slash path. Fork-mode
            // skills returned above (RunForkSkill → QuerySkillRuntime records);
            // the model/SkillTool path records in skill_runtime. This is the
            // only seam that sees a user typing `/name` for an inline skill —
            // the accrual path the Curator promotes/retires on. `handler` is
            // the resolved trait object, so the registry-wrapper record site is
            // never reached in production.
            record_slash_skill_invocation(cmd, coco_skills::telemetry::SkillOutcome::Success);
            r
        }
        Err(e) => {
            record_slash_skill_invocation(cmd, coco_skills::telemetry::SkillOutcome::Failure);
            emit_slash_status(
                event_tx,
                name,
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
            return SlashOutcome::Handled;
        }
    };

    use coco_commands::CommandResult;
    use coco_commands::DialogSpec;
    use coco_commands::PromptPart;
    match result {
        CommandResult::Skip => SlashOutcome::Handled,
        CommandResult::Text(text) => {
            // Sentinel detection — handlers like `/compact`, `/dream`,
            // `/summary` produce a sentinel-prefixed string instead of
            // having direct access to the runtime. Convert the sentinel
            // into a structured `SlashOutcome` so the agent driver runs
            // the real feature (compaction, consolidation, extraction).
            // Mirrors the SDK turn/start sentinel detection for the
            // non-interactive path.
            if let Some(trigger) = classify_sentinel_trigger(&text) {
                return match trigger {
                    SentinelTrigger::Compact {
                        custom_instructions,
                    } => SlashOutcome::TriggerCompact {
                        custom_instructions,
                    },
                    SentinelTrigger::Dream => SlashOutcome::TriggerDream,
                    SentinelTrigger::Summary => SlashOutcome::TriggerSummary,
                    SentinelTrigger::Cost => SlashOutcome::ShowCost,
                    SentinelTrigger::Status => SlashOutcome::ShowStatus,
                    SentinelTrigger::Goal { request } => SlashOutcome::TriggerGoal { request },
                    SentinelTrigger::Rename { request } => SlashOutcome::TriggerRename { request },
                    SentinelTrigger::Tag { tag } => SlashOutcome::TriggerTag { tag },
                    SentinelTrigger::AddDir { path } => SlashOutcome::TriggerAddDir { path },
                    SentinelTrigger::ReloadPlugins => SlashOutcome::TriggerReloadPlugins,
                    SentinelTrigger::ReloadHooks => SlashOutcome::TriggerReloadHooks,
                    SentinelTrigger::Btw { request } => SlashOutcome::TriggerBtw { request },
                };
            }
            emit_slash_text(event_tx, name, args, &text).await;
            SlashOutcome::Handled
        }
        CommandResult::InjectPrompt(text) => SlashOutcome::RunEngine {
            content: text,
            metadata: Some(format_slash_command_metadata(&cmd.base.name, args)),
            thinking_level: None,
            model_runtime_source: None,
        },
        CommandResult::MoaOneShot { prompt } => {
            let preset = runtime
                .runtime_config()
                .settings
                .merged
                .moa
                .default_preset_name()
                .to_string();
            SlashOutcome::RunEngine {
                content: prompt,
                metadata: Some(format_slash_command_metadata(&cmd.base.name, args)),
                thinking_level: None,
                model_runtime_source: Some(coco_inference::ModelRuntimeSource::Explicit(
                    coco_types::ProviderModelSelection {
                        provider: coco_config::MOA_PROVIDER.to_string(),
                        model_id: preset,
                    },
                )),
            }
        }
        CommandResult::Prompt { parts, .. } => {
            // Concatenate text parts. `File` parts are not yet wired —
            // none of the in-tree Prompt handlers emit them today.
            let mut buf = String::new();
            for part in parts {
                match part {
                    PromptPart::Text { text } => {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(&text);
                    }
                    PromptPart::File { .. } => {
                        warn!(%name, "Prompt::File parts not yet rendered to engine input");
                    }
                }
            }
            if buf.is_empty() {
                emit_slash_status(event_tx, name, args, SlashCommandStatusKind::EmptyPrompt).await;
                SlashOutcome::Handled
            } else {
                SlashOutcome::RunEngine {
                    content: buf,
                    metadata: Some(format_slash_command_metadata(&cmd.base.name, args)),
                    thinking_level: match &cmd.command_type {
                        coco_types::CommandType::Prompt(data) => data.thinking_level.clone(),
                        _ => None,
                    },
                    model_runtime_source: None,
                }
            }
        }
        CommandResult::Compact {
            display_text,
            summary,
        } => {
            // Pre-computed summary path: a handler that already ran
            // compaction (or has a summary in hand) returns the summary
            // string + display text. We push the summary as a
            // `is_compact_summary: true` user message so the next turn
            // sees it as a compact boundary; the LLM-summarized engine
            // path is unchanged (it's still the entry-point for typed
            // `/compact` from the TUI fast-path).
            // Truncation of pre-summary rounds is intentionally left to
            // the handler — when no handler emits this today, we err on
            // the side of preserving history rather than dropping it.
            if !summary.trim().is_empty() {
                // I-1 (Authority): pre-computed compact summary push
                // goes through history_push_and_emit so the TUI
                // TranscriptView and SDK observers see the new
                // boundary marker, not just the slash text echo.
                let mut h = runtime.history().lock().await;
                let event_tx_opt = Some(event_tx.clone());
                coco_query::history_sync::history_push_and_emit(
                    &mut h,
                    coco_compact::build_compact_summary_message(&summary),
                    &event_tx_opt,
                )
                .await;
            }
            emit_slash_text(event_tx, name, args, &display_text).await;
            SlashOutcome::Handled
        }
        CommandResult::OpenDialog(spec) => {
            // Wired dialogs route to TuiOnlyEvent so the TUI opens the
            // modal; unwired dialogs emit a localized breadcrumb.
            match spec {
                DialogSpec::MessageSelector => {
                    tracing::debug!(
                        target: "rewind::dispatch",
                        "translating DialogSpec::MessageSelector → OpenRewindPicker",
                    );
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenRewindPicker))
                        .await;
                }
                DialogSpec::MemoryFileSelector { entries } => {
                    // Convert from coco_commands::MemoryFileEntry to the
                    // wire-payload struct in coco-types so the TUI can
                    // consume the event without depending on coco-commands.
                    let wire_entries: Vec<coco_types::MemoryDialogEntry> = entries
                        .into_iter()
                        .map(|e| {
                            let row_kind = if e.is_folder {
                                coco_types::MemoryDialogRowKind::Folder { enabled: true }
                            } else {
                                coco_types::MemoryDialogRowKind::File {
                                    exists: !e.is_new,
                                    read_only: false,
                                }
                            };
                            coco_types::MemoryDialogEntry {
                                path: e.path.display().to_string(),
                                label: e.label,
                                scope: match e.scope {
                                    coco_commands::MemoryScope::Managed => {
                                        coco_types::MemoryDialogScope::Managed
                                    }
                                    coco_commands::MemoryScope::User => {
                                        coco_types::MemoryDialogScope::User
                                    }
                                    coco_commands::MemoryScope::Project => {
                                        coco_types::MemoryDialogScope::Project
                                    }
                                    coco_commands::MemoryScope::ProjectLocal => {
                                        coco_types::MemoryDialogScope::ProjectLocal
                                    }
                                    coco_commands::MemoryScope::ProjectConfig => {
                                        coco_types::MemoryDialogScope::ProjectConfig
                                    }
                                    coco_commands::MemoryScope::Subdir => {
                                        coco_types::MemoryDialogScope::Subdir
                                    }
                                    coco_commands::MemoryScope::Imported => {
                                        coco_types::MemoryDialogScope::Imported
                                    }
                                    coco_commands::MemoryScope::AutoMemFolder => {
                                        coco_types::MemoryDialogScope::AutoMemFolder
                                    }
                                    coco_commands::MemoryScope::TeamMemFolder => {
                                        coco_types::MemoryDialogScope::TeamMemFolder
                                    }
                                    coco_commands::MemoryScope::AgentMemFolder => {
                                        coco_types::MemoryDialogScope::AgentMemFolder
                                    }
                                },
                                row_kind,
                            }
                        })
                        .collect();
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenMemoryDialog {
                            entries: wire_entries,
                        }))
                        .await;
                }
                DialogSpec::ModelPicker => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenModelPicker))
                        .await;
                }
                DialogSpec::ProviderWizard => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenProviderWizard))
                        .await;
                }
                DialogSpec::ThemePicker => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenThemePicker))
                        .await;
                }
                DialogSpec::WorkflowPicker => {
                    let cfg = runtime.current_engine_config().await;
                    let cwd = if let Some(session_cwd) = cfg.session_cwd.as_ref() {
                        Some(session_cwd.read().await.clone())
                    } else {
                        cfg.original_cwd
                            .clone()
                            .or_else(|| Some(runtime.original_cwd().clone()))
                    };
                    let entries = coco_workflow::list_workflows(cwd)
                        .into_iter()
                        .map(|entry| coco_types::WorkflowDialogEntry {
                            name: entry.name,
                            description: entry.description,
                            source_path: entry.source_path.display().to_string(),
                        })
                        .collect();
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenWorkflowPicker {
                            payload: coco_types::WorkflowDialogPayload { entries },
                        }))
                        .await;
                }
                DialogSpec::SkillsList { mut payload } => {
                    // The `SkillsHandler` runs through the
                    // `CommandHandler` trait, which doesn't carry a
                    // `RuntimeConfig` ref — so it ships every entry
                    // with empty-tier defaults (`baseline=On`, no
                    // lock, no `current_local`). Reach into the live
                    // engine_config here to populate the real
                    // override / lock state before forwarding to
                    // the TUI; otherwise the dialog renders every
                    // row as if no overrides existed and the user's
                    // edits would silently overwrite policy-locked
                    // or already-persisted state.
                    let cfg = runtime.current_engine_config().await;
                    let skills = runtime.skill_manager();
                    coco_commands::handlers::skills::enrich_payload_with_tiers(
                        &mut payload,
                        &cfg.skill_overrides,
                        &skills,
                    );
                    // Stamp the live main-model bytes/token ratio so
                    // the dialog's `~N tok` column tracks the model
                    // the user is actually talking to. Handler can't
                    // do this — it has no `QueryEngineConfig` in
                    // scope.
                    payload.bytes_per_token =
                        coco_model_card::bytes_per_token_for_model(&cfg.model_id);
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenSkillsDialog { payload }))
                        .await;
                }
                DialogSpec::AgentsList { payload } => {
                    // The handler ships the agent catalog as it
                    // looks on disk; running counts are derived TUI-
                    // side from the live `SessionState.subagents`
                    // mirror, so no enrichment is needed here. Mid-
                    // session edits via stage-5 CRUD trigger a
                    // `reload_agent_catalog()` and a fresh payload
                    // round-trip rather than mutating in place.
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenAgentsDialog { payload }))
                        .await;
                }
                DialogSpec::PluginPicker => {
                    refresh_plugin_dialog_payload(session, event_tx).await;
                }
                DialogSpec::McpbConfig { .. } | DialogSpec::Confirm { .. } => {
                    let dialog_kind = match spec {
                        DialogSpec::McpbConfig { .. } => "MCPB config form",
                        DialogSpec::Confirm { .. } => "confirm dialog",
                        DialogSpec::MessageSelector
                        | DialogSpec::MemoryFileSelector { .. }
                        | DialogSpec::SkillsList { .. }
                        | DialogSpec::AgentsList { .. }
                        | DialogSpec::PluginPicker
                        | DialogSpec::ModelPicker
                        | DialogSpec::ProviderWizard
                        | DialogSpec::WorkflowPicker
                        | DialogSpec::ThemePicker => unreachable!(),
                    }
                    .to_string();
                    emit_slash_status(
                        event_tx,
                        name,
                        args,
                        SlashCommandStatusKind::DialogPending { dialog_kind },
                    )
                    .await;
                }
            }
            SlashOutcome::Handled
        }
    }
}

const MAX_FILE_HISTORY_DIFF_CHARS: usize = 6000;

async fn run_file_history_diff_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    args: &str,
) {
    let Some(file_history) = session.file_history() else {
        emit_slash_text(
            event_tx,
            "diff",
            args,
            "File history is not enabled for this session.",
        )
        .await;
        return;
    };

    let mut parts = args.split_whitespace();
    match parts.next() {
        Some("session") => {
            let session_id = session.current_typed_session_id().await.to_string();
            let rendered = {
                let file_history = file_history.read().await;
                file_history
                    .render_session_diff(session.config_home(), &session_id)
                    .await
            };
            let text = match rendered {
                Ok(diff) => format_file_history_diff("Session diff", diff),
                Err(err) => format!("Unable to build session diff: {err}"),
            };
            emit_slash_text(event_tx, "diff", args, &text).await;
        }
        Some("turn") => {
            let Some(message_id) = parts.next() else {
                emit_slash_text(event_tx, "diff", args, "Usage: /diff turn <message-id>").await;
                return;
            };
            let session_id = session.current_typed_session_id().await.to_string();
            let rendered = {
                let file_history = file_history.read().await;
                let Some(next_message_id) =
                    next_file_history_snapshot_id(&file_history, message_id)
                else {
                    emit_slash_text(event_tx, "diff", args, "No snapshot found for message id.")
                        .await;
                    return;
                };
                file_history
                    .render_diff_between(
                        message_id,
                        next_message_id.as_deref(),
                        session.config_home(),
                        &session_id,
                    )
                    .await
            };
            let text = match rendered {
                Ok(diff) => format_file_history_diff("Turn diff", diff),
                Err(err) => format!("Unable to build turn diff: {err}"),
            };
            emit_slash_text(event_tx, "diff", args, &text).await;
        }
        _ => {
            emit_slash_text(
                event_tx,
                "diff",
                args,
                "Usage: /diff session | /diff turn <message-id>",
            )
            .await;
        }
    }
}

fn next_file_history_snapshot_id(
    file_history: &coco_context::FileHistoryState,
    message_id: &str,
) -> Option<Option<String>> {
    let idx = file_history
        .snapshots
        .iter()
        .position(|snapshot| snapshot.message_id == message_id)?;
    Some(
        file_history
            .snapshots
            .get(idx + 1)
            .map(|snapshot| snapshot.message_id.clone()),
    )
}

fn format_file_history_diff(title: &str, diff: coco_context::RenderedDiff) -> String {
    if diff.stats.files_changed.is_empty() {
        return format!("{title}: no file-history changes.");
    }

    let mut out = format!(
        "{title}: {} file{}, +{}, -{}\n\n",
        diff.stats.files_changed.len(),
        if diff.stats.files_changed.len() == 1 {
            ""
        } else {
            "s"
        },
        diff.stats.insertions,
        diff.stats.deletions
    );
    append_truncated_file_history_diff(&mut out, &diff.unified_diff);
    out
}

fn append_truncated_file_history_diff(out: &mut String, diff: &str) {
    let trimmed = diff.trim();
    if trimmed.len() <= MAX_FILE_HISTORY_DIFF_CHARS {
        out.push_str(trimmed);
        return;
    }

    let head = coco_utils_string::take_bytes_at_char_boundary(trimmed, MAX_FILE_HISTORY_DIFF_CHARS);
    let truncate_at = head.rfind('\n').unwrap_or(head.len());
    out.push_str(&trimmed[..truncate_at]);
    let remaining_lines = trimmed[truncate_at..].lines().count();
    out.push_str(&format!("\n\n... truncated ({remaining_lines} more lines)"));
}

async fn run_tasks_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
    name: &str,
    args: &str,
) {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenBackgroundTasks))
            .await;
        return;
    }

    let mut parts = trimmed.split_whitespace();
    match parts.next() {
        Some("list") => {
            local_app_server_bridge
                .install_session_runtime(session.clone())
                .await;
            match local_app_server_bridge
                .client()
                .task_list(local_app_server_bridge.handler())
                .await
            {
                Ok(result) => {
                    let text = format_task_list(&result.tasks);
                    emit_slash_text(event_tx, name, args, &text).await;
                }
                Err(err) => {
                    emit_slash_text(
                        event_tx,
                        name,
                        args,
                        &format!("Failed to list tasks: {err}"),
                    )
                    .await;
                }
            }
        }
        Some("detail") => {
            let Some(task_id) = parts.next() else {
                emit_slash_text(event_tx, name, args, "Usage: /tasks detail <id>").await;
                return;
            };
            local_app_server_bridge
                .install_session_runtime(session.clone())
                .await;
            match local_app_server_bridge
                .client()
                .task_detail(
                    local_app_server_bridge.handler(),
                    coco_types::TaskDetailParams {
                        task_id: task_id.to_string(),
                    },
                )
                .await
            {
                Ok(result) => {
                    let text = format_task_detail(&result);
                    emit_slash_text(event_tx, name, args, &text).await;
                }
                Err(err) => {
                    emit_slash_text(event_tx, name, args, &format!("Failed to read task: {err}"))
                        .await;
                }
            }
        }
        Some("cancel") => {
            let Some(task_id) = parts.next() else {
                emit_slash_text(event_tx, name, args, "Usage: /tasks cancel <id>").await;
                return;
            };
            local_app_server_bridge
                .install_session_runtime(session.clone())
                .await;
            match local_app_server_bridge
                .client()
                .stop_task(
                    local_app_server_bridge.handler(),
                    coco_types::StopTaskParams {
                        task_id: task_id.to_string(),
                    },
                )
                .await
            {
                Ok(()) => {
                    emit_slash_text(event_tx, name, args, &format!("Cancelled task {task_id}."))
                        .await;
                }
                Err(err) => {
                    emit_slash_text(
                        event_tx,
                        name,
                        args,
                        &format!("Failed to cancel task {task_id}: {err}"),
                    )
                    .await;
                }
            }
        }
        Some(_) | None => {
            emit_slash_text(
                event_tx,
                name,
                args,
                "Usage: /tasks [list|detail <id>|cancel <id>]",
            )
            .await;
        }
    }
}

async fn toggle_fast_mode_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    let cfg = runtime.current_engine_config().await;
    let requested = !cfg.fast_mode;
    let active = requested && coco_config::is_fast_mode_supported_by_model(&cfg.model_id);
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    let bridge_session_id = runtime.current_typed_session_id().await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(bridge_session_id, event_tx.clone())
    {
        warn!(%error, "TUI ToggleFastMode could not attach local AppServer event pump");
        return;
    }

    let mut settings = HashMap::new();
    settings.insert("fast_mode".to_string(), serde_json::json!(active));
    if let Err(error) = local_app_server_bridge
        .client()
        .config_apply_flags(
            local_app_server_bridge.handler(),
            coco_types::ConfigApplyFlagsParams { settings },
        )
        .await
    {
        warn!(%error, "TUI ToggleFastMode via AppServerLocalBridge failed");
    }
}

async fn set_thinking_level_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    level: String,
) {
    let effort = match level.parse::<coco_types::ReasoningEffort>() {
        Ok(effort) => effort,
        Err(err) => {
            tracing::warn!(level = %level, error = %err, "SetThinkingLevel: bad effort string, ignoring");
            return;
        }
    };
    let runtime = session;
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    let bridge_session_id = runtime.current_typed_session_id().await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(bridge_session_id, event_tx.clone())
    {
        warn!(%error, "TUI SetThinkingLevel could not attach local AppServer event pump");
        return;
    }

    if let Err(error) = local_app_server_bridge
        .client()
        .set_thinking(
            local_app_server_bridge.handler(),
            coco_types::SetThinkingParams {
                thinking_level: Some(coco_types::ThinkingLevel {
                    effort,
                    budget_tokens: None,
                    options: HashMap::new(),
                }),
            },
        )
        .await
    {
        warn!(%error, "TUI SetThinkingLevel via AppServerLocalBridge failed");
    }
}

fn format_task_list(tasks: &[coco_types::TaskStateBase]) -> String {
    if tasks.is_empty() {
        return "No tasks in this session.".to_string();
    }

    let mut out = String::from("Active tasks:\n\n");
    for task in tasks {
        out.push_str(&format!(
            "- {}  {:?}  {}\n",
            task.id, task.status, task.description
        ));
    }
    out
}

fn format_task_detail(result: &coco_types::TaskDetailResult) -> String {
    let mut out = format!("Task {}\n\n", result.task_id);
    out.push_str(&format!("Interrupted: {}\n", result.interrupted));
    if let Some(code) = result.exit_code {
        out.push_str(&format!("Exit code: {code}\n"));
    }
    if !result.stdout.trim().is_empty() {
        out.push_str("\nstdout:\n");
        out.push_str(&result.stdout);
        if !result.stdout.ends_with('\n') {
            out.push('\n');
        }
    }
    if !result.stderr.trim().is_empty() {
        out.push_str("\nstderr:\n");
        out.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Pure decision used by `dispatch_plan`: after a `/plan <description>`
/// successfully flips into plan mode, should the slash command fire a
/// query for the description? Returns `Some(trimmed_description)` when a
/// query should fire (`description` is non-empty and not `"open"`), else
/// `None`. Pure so this rule is regression-tested without a
/// `SessionRuntime` fixture.
fn plan_command_query_after_flip(args: &str) -> Option<&str> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "open" {
        None
    } else {
        Some(trimmed)
    }
}

/// `/plan` dispatch with full session-runtime context.
/// Typing `/plan` IS the consent to enter plan mode, so the dispatcher
/// flips state directly via the same dual-write path
/// `UserCommand::SetPermissionMode` uses (engine_config + app_state)
/// plus the plan-mode-specific patch (`pre_plan_mode`,
/// `plan_mode_entry_ms`, `needs_plan_mode_exit_attachment` cleared).
/// The model never sees a redundant `EnterPlanMode` Yes/No dialog.
/// Per-arg behaviour:
/// - `""` → flip if needed, then show current plan or hint
/// - `"open"` → flip if needed, ensure file, launch `$EDITOR`/`vi`
/// - `<description>` → flip if needed; if state changed, fire a query
/// with the description; if already in plan mode, ignore the
/// description and show the plan.
async fn dispatch_plan(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let runtime = session;
    let args = args.trim();

    // Plan mode opted out via `features.plan_mode = false`: don't flip into
    // Plan, just tell the user. Mirrors the hidden plan-mode tools, the
    // suppressed reminders, and the Plan rung removed from the Shift+Tab cycle.
    if !runtime
        .runtime_config()
        .features
        .enabled(coco_types::Feature::PlanMode)
    {
        emit_slash_text(
            event_tx,
            "plan",
            args,
            "Plan mode is disabled (`features.plan_mode = false`). \
             Re-enable it in settings.json to use `/plan`.",
        )
        .await;
        return SlashOutcome::Handled;
    }

    let session_id = runtime.current_typed_session_id().await;
    let project_dir = runtime.runtime_config().paths.project_dir.as_deref();
    let plans_directory_setting = runtime
        .runtime_config()
        .settings
        .merged
        .plans_directory
        .as_deref();
    let plans_dir = session_plans_dir(runtime.config_home(), project_dir, plans_directory_setting);

    // Live cross-turn state (`app_state.permission_mode`) wins when
    // present, else fall back to the engine_config value (covers the
    // "app_state not yet primed" case at the start of a fresh session).
    let live_app_mode = runtime.app_state().read().await.permissions.mode;
    let prev_mode = match live_app_mode {
        Some(m) => m,
        None => runtime.current_engine_config().await.permission_mode,
    };
    let was_in_plan = prev_mode == coco_types::PermissionMode::Plan;

    // Flip state for ALL `/plan` invocations when not already in plan
    // mode — bare `/plan`, `/plan open`, and `/plan <description>` all
    // consent to plan mode equally.
    if !was_in_plan {
        let cfg = runtime.current_engine_config().await;
        let change = coco_cli::live_permission_mode::apply_to_runtime(
            session,
            coco_types::PermissionMode::Plan,
            event_tx,
            cfg.permission_mode_availability.bypass_permissions,
        )
        .await;
        info!(
            session_id = %session_id,
            from = ?change.previous,
            to = ?coco_types::PermissionMode::Plan,
            "TUI /plan: direct-toggle to Plan mode",
        );
    }

    // Path to the (resolved) session plan file — used by every arm.
    let plan_path =
        coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None);

    if args.is_empty() {
        let content =
            coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
        let body = match content {
            Some(body) if !body.trim().is_empty() => format!(
                "## Current Plan\n\n*{}*\n\n{}\n\nRun `/plan open` to edit in $EDITOR.",
                plan_path.display(),
                body
            ),
            _ => format!(
                "No plan written yet for this session.\n\n\
                 Plan file: `{}`\n\n\
                 Run `/plan <description>` to plan for a task in plan mode, \
                 or `/plan open` to start an empty plan in $EDITOR.",
                plan_path.display()
            ),
        };
        let text = if was_in_plan {
            body
        } else {
            format!("Enabled plan mode.\n\n{body}")
        };
        emit_slash_text(event_tx, "plan", args, &text).await;
        return SlashOutcome::Handled;
    }

    if args == "open" {
        let text = if was_in_plan {
            format!("Opening plan file: {}", plan_path.display())
        } else {
            format!(
                "Enabled plan mode.\n\nOpening plan file: {}",
                plan_path.display()
            )
        };
        emit_slash_text(event_tx, "plan", args, &text).await;
        return SlashOutcome::TriggerOpenPlanEditor { path: plan_path };
    }

    // `/plan <description>` —
    // - Flipped to plan mode → fire query with the user input.
    // Returns `RunEngine { content: <description> }`.
    // - Already in plan mode → ignore the description, just show the plan.
    if was_in_plan {
        let content =
            coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
        let text = match content {
            Some(body) if !body.trim().is_empty() => format!(
                "Already in plan mode.\n\n## Current Plan\n\n*{}*\n\n{}\n\n\
                 Run `/plan open` to edit in $EDITOR.",
                plan_path.display(),
                body
            ),
            _ => "Already in plan mode. No plan written yet.".to_string(),
        };
        emit_slash_text(event_tx, "plan", args, &text).await;
        return SlashOutcome::Handled;
    }
    match plan_command_query_after_flip(args) {
        Some(desc) => SlashOutcome::RunEngine {
            content: desc.to_string(),
            metadata: Some(format_slash_command_metadata("plan", args)),
            thinking_level: None,
            model_runtime_source: None,
        },
        None => {
            // Unreachable in practice — bare `/plan` and `/plan open`
            // are handled by the earlier branches. Kept defensive so
            // future edits to the cascade can't silently fall through.
            SlashOutcome::Handled
        }
    }
}

/// In-flight turn handle. Each `SubmitInput` / `ExecuteSkill` spawns
/// the engine call into a child task so the `command_rx` recv loop stays
/// responsive (Interrupt / ClearConversation / Compact / Rewind / Shutdown
/// can reach their arms while the engine runs). Rust's explicit
/// `tokio::spawn` keeps the recv loop unblocked.
struct ActiveTurn {
    id: uuid::Uuid,
    task: tokio::task::JoinHandle<()>,
    cancel: ActiveTurnCancel,
}

struct ActiveTurnCancel {
    /// AppServer-owned turn: cancellation flows through the same
    /// `turn/interrupt` request the SDK uses.
    client: coco_app_server_client::ServerClient<coco_cli::sdk_server::LocalAppSessionHandle>,
    handler: coco_cli::sdk_server::AppServerSdkHandler,
}

/// Always-fires completion signaller for spawned turn tasks.
/// The main `select!` loop in `run_agent_driver` blocks on
/// `turn_done_rx.recv()` to drain a completed turn from `active_turn`.
/// Sending `turn_id` as the last statement of the spawned task only
/// covers the happy path: a panic inside a spawned turn body unwinds
/// before reaching the send, so the `active_turn` slot stays occupied
/// with a corpse `JoinHandle` until the next user command forces
/// `drain_active_turn()` to collect it.
/// `Drop` runs on both normal scope-exit and panic unwind. `try_send`
/// is non-blocking and safe in `Drop`; the receiver is drained promptly
/// so the bounded channel (buffer 16) should never be full in practice.
struct TurnDoneGuard {
    turn_id: uuid::Uuid,
    tx: mpsc::Sender<uuid::Uuid>,
}

impl Drop for TurnDoneGuard {
    fn drop(&mut self) {
        if let Err(err) = self.tx.try_send(self.turn_id) {
            warn!(
                turn_id = %self.turn_id,
                error = ?err,
                "turn completion signal failed in Drop; active_turn may stay locked until next drain"
            );
        }
    }
}

/// Completion signaller for the cross-process teammate inbox pump (gap 1).
/// Fires the turn's `user_message_id` so the pump (`teammate_inbox_pump`)
/// can release its serialized wait and inject the next mailbox message.
/// `Drop` (not a tail send) so the signal fires on normal completion,
/// cancellation, AND panic — same reasoning as [`TurnDoneGuard`]. Only
/// attached in a teammate session (the pump is the sole consumer); the
/// `try_send` is best-effort against the bounded handshake channel.
struct PumpDoneGuard {
    id: String,
    tx: mpsc::Sender<String>,
}

impl Drop for PumpDoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.try_send(self.id.clone());
    }
}

const TUI_SHUTDOWN_ACTIVE_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTurnDrain {
    Wait,
    AbortAfter(Duration),
}

enum PendingEditorRequest {
    Memory {
        path: std::path::PathBuf,
    },
    Plan {
        path: std::path::PathBuf,
    },
    PlanPrompt {
        request_id: String,
        initial_content: String,
        path: Option<std::path::PathBuf>,
    },
    Prompt {
        initial_content: String,
    },
    /// `/agents` Library tab Enter on an editable agent row → fork
    /// `$EDITOR` against the markdown source path. On editor exit the
    /// runner re-reads the agent catalog and re-emits the dialog
    /// payload so the dialog refreshes against the new on-disk state.
    Agent {
        path: std::path::PathBuf,
    },
}

/// Cancel the in-flight turn (if any) and drain its task.
/// Used by every arm whose semantics conflict with a concurrent
/// turn (Clear / Compact / Rewind / Shutdown / next SubmitInput).
/// `AbortAfter` is reserved for explicit process shutdown so a stuck
/// tool or stream cannot leave the terminal sitting on the exit hint.
/// Cancellation now goes through AppServer `turn/interrupt`; the server-side
/// runner owns the terminal `TurnEnded` emission and reason mapping.
async fn drain_active_turn(slot: &Arc<Mutex<Option<ActiveTurn>>>, mode: ActiveTurnDrain) {
    let state = { slot.lock().await.take() };
    if let Some(s) = state {
        let ActiveTurnCancel { client, handler } = &s.cancel;
        if let Err(error) = client.turn_interrupt(handler).await {
            tracing::warn!(%error, "drain_active_turn: AppServer turn/interrupt failed");
        }
        match mode {
            ActiveTurnDrain::Wait => {
                let _ = s.task.await;
            }
            ActiveTurnDrain::AbortAfter(timeout) => {
                let mut task = s.task;
                tokio::select! {
                    result = &mut task => {
                        let _ = result;
                    }
                    _ = tokio::time::sleep(timeout) => {
                        warn!(
                            timeout_ms = timeout.as_millis(),
                            "active turn did not stop during TUI shutdown; aborting task"
                        );
                        task.abort();
                        let _ = task.await;
                    }
                }
            }
        }
    }
}

async fn drain_completed_turn(slot: &Arc<Mutex<Option<ActiveTurn>>>, turn_id: uuid::Uuid) -> bool {
    let state = {
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|s| s.id == turn_id) {
            guard.take()
        } else {
            None
        }
    };
    if let Some(s) = state {
        let _ = s.task.await;
        true
    } else {
        false
    }
}

/// Run a manual full LLM compaction. Used by `UserCommand::Compact` and
/// the slash dispatcher's `TriggerCompact` outcome — both routes feed
/// through here so typed `/compact` and palette `/compact` behave
/// identically.
async fn run_manual_compact(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    custom_instructions: Option<String>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    // Drain any active turn before compacting: the AppServer compact shortcut
    // mutates the active history and runs an LLM call.
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt =
        match coco_commands::handlers::compact::handler(custom_instructions.unwrap_or_default())
            .await
        {
            Ok(prompt) => prompt,
            Err(error) => {
                warn!(%error, "TUI /compact handler failed");
                return;
            }
        };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /compact",
    )
    .await;
}

async fn run_local_app_server_shortcut_turn(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    prompt: String,
    log_label: &'static str,
) {
    let session_id = session.current_typed_session_id().await;
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(session_id.clone(), event_tx.clone())
    {
        tracing::warn!(%error, "{log_label} could not refresh local AppServer event pump");
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    let passive_surface = match monitor_client.subscribe_session(
        session_id.clone(),
        Some(0),
        coco_app_server::AttachSurfaceOptions::default(),
    ) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "{log_label} could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        prompt,
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "{log_label} AppServer turn/start failed");
            return;
        }
    };
    let turn_id = uuid::Uuid::new_v4();
    let protocol_turn_id = started.turn_id.clone();
    let turn_done_tx_t = turn_done_tx.clone();
    let task = tokio::spawn(async move {
        let _done = TurnDoneGuard {
            turn_id,
            tx: turn_done_tx_t,
        };
        while let Some(envelope) = monitor_client.next_passive_event(&passive_surface).await {
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == protocol_turn_id
            {
                break;
            }
        }
    });
    let interrupt_client = local_app_server_bridge.connect_local_client();
    let handler = local_app_server_bridge.handler().clone();
    *active_turn.lock().await = Some(ActiveTurn {
        id: turn_id,
        task,
        cancel: ActiveTurnCancel {
            client: interrupt_client,
            handler,
        },
    });
}

/// Run a user-typed fork-mode skill (`/<name>` with `context: fork`) as a
/// subagent and inject its result into the transcript.
/// `executeForkedSlashCommand`: the subagent runs synchronously, its final
/// text lands as a `<local-command-stdout>` user message, and there is NO
/// follow-up main-model query.
/// Drains the in-flight turn first (the subagent runs LLM calls / mutates
/// shared state) — same contract as `run_manual_compact`.
async fn run_fork_skill(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    name: &str,
    args: &str,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
) {
    let runtime = session;
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;

    let body = match runtime.invoke_skill_fork(name, args).await {
        Ok(output) => format!("<local-command-stdout>\n{output}\n</local-command-stdout>"),
        Err(e) => {
            warn!(skill = %name, error = %e, "fork-mode skill failed");
            format!("<local-command-stderr>\nSkill '/{name}' failed: {e}\n</local-command-stderr>")
        }
    };
    // Persist the command marker + result via history_push_and_emit so the
    // TUI transcript renders them and the next turn's model sees what ran.
    let mut h = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    coco_query::history_sync::history_push_and_emit(
        &mut h,
        create_slash_metadata_message(&format_slash_command_metadata(name, args)),
        &event_tx_opt,
    )
    .await;
    coco_query::history_sync::history_push_and_emit(
        &mut h,
        coco_messages::create_user_message(&body),
        &event_tx_opt,
    )
    .await;
}

/// Run the clear flow. Drains any active turn first since clear mutates
/// session_id + resets several per-session caches.
/// Plan I-1 (Authority): emits a wire-visible event after the clear so
/// the TUI's `TranscriptView` and SDK NDJSON observers stay coherent.
/// `/clear` rotates session_id → emit
/// `SessionResetForResume { session_id: new }`.
async fn run_clear_conversation(
    session: &crate::session_runtime::SessionHandle,
    control_context: &LocalRuntimeControlContext<'_>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let old_session_id = runtime.current_typed_session_id().await;
    if let Err(error) = local_app_server_bridge.ensure_interactive_surface(old_session_id.clone()) {
        warn!(
            %error,
            session_id = %old_session_id,
            "/clear could not confirm local AppServer interactive surface"
        );
        return;
    }
    let permissions = runtime.app_state().read().await.permissions.clone();
    let rewind_messages = runtime.prepare_for_clear_replacement().await;
    let new_session_id = coco_types::SessionId::generate();
    let make_runtime_factory = {
        let runtime_factory = control_context.runtime_factory.clone();
        let process_runtime = Arc::clone(control_context.process_runtime);
        let cwd = control_context.cwd.to_path_buf();
        let event_tx = event_tx.clone();
        let new_session_id = new_session_id.clone();
        let permissions = permissions.clone();
        let rewind_messages = rewind_messages.clone();
        async move {
            build_runtime_for_clear(
                runtime_factory,
                new_session_id,
                permissions,
                rewind_messages,
                process_runtime,
                cwd,
                event_tx,
            )
            .await
        }
    };
    let new_session = match local_app_server_bridge
        .replace_session_runtime_for_clear(
            old_session_id.clone(),
            new_session_id.clone(),
            make_runtime_factory,
        )
        .await
    {
        Ok(Some((session, _surface_id))) => session,
        Ok(None) => {
            warn!(
                session_id = %old_session_id,
                "/clear could not find local AppServer calling surface"
            );
            return;
        }
        Err(error) => {
            warn!(%error, "/clear failed to build replacement runtime");
            return;
        }
    };
    new_session.fire_session_start_hooks("clear").await;
    local_app_server_bridge
        .install_session_runtime(new_session.clone())
        .await;
    {
        let mut current = control_context.current_session.write().await;
        *current = new_session.clone();
    }
    control_context
        .runtime_reload_subscriptions
        .lock()
        .await
        .install_for_session(&new_session)
        .await;
    if let Err(error) = local_app_server_bridge.ensure_interactive_surface(new_session_id.clone()) {
        warn!(
            %error,
            session_id = %new_session_id,
            "/clear could not attach local AppServer interactive surface"
        );
    }
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(new_session_id.clone(), event_tx.clone())
    {
        warn!(
            %error,
            session_id = %new_session_id,
            "/clear could not refresh local AppServer event pump"
        );
    }
    let notif = ServerNotification::SessionResetForResume {
        identity: coco_types::ServerNotificationIdentity::new(Some(new_session_id), None),
    };
    let _ = event_tx.send(CoreEvent::Protocol(notif)).await;
    if let Some(messages) = new_session.pre_clear_rewind_messages().await {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::RewindPreClearSnapshot {
                messages,
            }))
            .await;
    }
}

/// Force auto-memory consolidation through the local AppServer `turn/start`
/// sentinel shortcut, matching SDK behavior.
async fn run_dream_consolidation(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = match coco_commands::handlers::dream::handler(String::new()).await {
        Ok(prompt) => prompt,
        Err(error) => {
            warn!(%error, "TUI /dream handler failed");
            return;
        }
    };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /dream",
    )
    .await;
}

/// Force a 9-section session-memory update through the local AppServer
/// `turn/start` sentinel shortcut, matching SDK behavior.
async fn run_session_memory_force(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = match coco_commands::handlers::summary::handler(String::new()).await {
        Ok(prompt) => prompt,
        Err(error) => {
            warn!(%error, "TUI /summary handler failed");
            return;
        }
    };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /summary",
    )
    .await;
}

/// `/btw <question>` runner — routes the existing sentinel through local
/// AppServer `turn/start`, matching SDK behavior and keeping the fork+answer
/// logic in the handler shortcut. The shortcut appends model-invisible slash
/// messages and emits a synthetic turn lifecycle for the TUI completion
/// monitor.
async fn run_side_question(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_cli::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    request: coco_commands::handlers::btw::BtwRequest,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = coco_commands::handlers::btw::handler(&request.question);
    if coco_commands::handlers::btw::parse_btw_sentinel(&prompt).is_none() {
        warn!("TUI /btw handler returned a non-sentinel prompt");
        return;
    }
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /btw",
    )
    .await;
}

/// `/export <filename>` runner — renders the live conversation `MessageHistory`
/// (incl. tool activity) and writes it to a file in the session's original cwd,
/// then confirms the path. The sync registry handler has no runtime access, so
/// the real export lives here.: the arg
/// is a FILENAME and the file is written under the cwd. coco infers the format
/// from the extension (`.md`→markdown, `.json`→json, else plain text) — TS
/// exports plain text only. The no-arg format-picker modal re-enters here with
/// a bare format keyword (`markdown`/`json`/`text`), for which a timestamped
/// default filename is generated. (Clipboard export lives in `/copy`.)
async fn run_export(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let runtime = session;
    use crate::conversation_export::ExportFormat;
    let arg = args.trim();
    // A bare format keyword comes from the modal → timestamped default name;
    // anything else is treated as the target filename (TS-style).
    let (format, filename) = match ExportFormat::from_keyword(&arg.to_ascii_lowercase()) {
        Some(format) => {
            let ts = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
            (format, format!("conversation-{ts}.{}", format.ext()))
        }
        None => {
            let format = ExportFormat::from_filename(arg);
            // Append the inferred extension when the filename carries none.
            let filename = if arg.contains('.') {
                arg.to_string()
            } else {
                format!("{arg}.{}", format.ext())
            };
            (format, filename)
        }
    };
    // Render under the lock, then drop it before the file write / await.
    let body = {
        let history = runtime.history().lock().await;
        format.render(history.as_slice())
    };
    let path = runtime.original_cwd().join(&filename);
    let message = match tokio::fs::write(&path, body).await {
        Ok(()) => format!("Conversation exported to {}", path.display()),
        Err(e) => format!("Failed to write export to {}: {e}", path.display()),
    };
    emit_slash_text(event_tx, "export", args, &message).await;
    SlashOutcome::Handled
}

/// `/rename [name]` runner — resolves the new name (explicit or
/// Fast-role auto-generated), persists it via local AppServer
/// `session/rename`, and
/// surfaces a single system-line confirmation.
async fn run_session_rename(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
    request: coco_commands::ParsedRename,
) {
    use coco_cli::session_rename::auto_generate_session_name;

    // Teammate guard — names are set by the team leader.
    if coco_coordinator::identity::is_teammate() {
        emit_slash_text(
            event_tx,
            "rename",
            "",
            "Cannot rename: This session is a swarm teammate. \
             Teammate names are set by the team leader.",
        )
        .await;
        return;
    }

    // Resolve the new name. `Auto` runs the Fast-role generator
    // against `messages_after_compact_boundary`.
    let name = match request {
        coco_commands::ParsedRename::Explicit(n) => n,
        coco_commands::ParsedRename::Auto => match auto_generate_session_name(session).await {
            Ok(n) => n,
            Err(err) => {
                emit_slash_text(event_tx, "rename", "", err.user_message()).await;
                return;
            }
        },
    };

    let text = match local_app_server_bridge
        .client()
        .session_rename(
            local_app_server_bridge.handler(),
            coco_types::SessionRenameParams { name: name.clone() },
        )
        .await
    {
        Ok(result) => format!("Session renamed to: {}", result.name),
        Err(coco_app_server_client::ClientError::Server { message, .. })
            if message.starts_with("Cannot rename:") =>
        {
            message
        }
        Err(error) => format!("Failed to rename session: {error}"),
    };
    emit_slash_text(event_tx, "rename", "", &text).await;
}

/// `/reload-plugins` runner — rescans plugin + skill dirs and
/// atomically swaps the active `CommandRegistry`. Snapshots taken by
/// in-flight dispatches stay valid (they hold the prior `Arc`); the
/// swap is observed by the next dispatch.
/// After the swap we also push the fresh visible-command list to the
/// TUI via [`TuiOnlyEvent::AvailableCommandsRefreshed`] so the `/`
/// autocomplete popup and command palette stop pointing at stale names
/// from removed plugins.
async fn run_reload_plugins(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    let result = match local_app_server_bridge
        .client()
        .plugin_reload(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let body = format!("Plugin reload failed: {error}");
            emit_slash_text(event_tx, "reload-plugins", "", &body).await;
            return;
        }
    };
    let hook_note = if result.error_count == 0 {
        String::new()
    } else {
        format!(" · {} reload error(s)", result.error_count)
    };
    let body = format!(
        "Reloaded — {} commands{hook_note}; agents + LSP refreshed.",
        result.commands.len()
    );
    emit_slash_text(event_tx, "reload-plugins", "", &body).await;

    let snapshot = runtime.current_command_registry().await.snapshot_for_ui();
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
            commands: snapshot,
        }))
        .await;
}

/// `/hooks reload` runner — rebuild the live `HookRegistry` from the
/// latest `RuntimeConfig` snapshot.
async fn run_reload_hooks(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    let body = match local_app_server_bridge
        .client()
        .hook_reload(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => format!(
            "Reloaded — {} hook(s) registered from current settings.",
            result.hook_count
        ),
        Err(error) => format!("Hook reload failed: {error}"),
    };
    emit_slash_text(event_tx, "hooks", "", &body).await;
}

async fn run_show_cost(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_cost(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => emit_slash_text(event_tx, "cost", "", &result.text).await,
        Err(error) => {
            let body = format!("Failed to read session cost: {error}");
            emit_slash_text(event_tx, "cost", "", &body).await;
        }
    }
}

async fn run_show_status(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_status(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus {
                    title: "Status".to_string(),
                    body: result.text,
                }))
                .await;
        }
        Err(error) => {
            let body = format!("Failed to read session status: {error}");
            emit_slash_text(event_tx, "status", "", &body).await;
        }
    }
}

async fn run_goal_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    request: coco_commands::GoalCommandRequest,
) -> SlashFollowup {
    let runtime = session;
    let is_status = matches!(request, coco_commands::GoalCommandRequest::Status);
    let args = goal_command::goal_display_args(&request).to_string();
    let gate = goal_command::GoalGate {
        hooks_restricted: {
            let cfg = runtime.current_engine_config().await;
            cfg.disable_all_hooks || cfg.allow_managed_hooks_only
        },
        // Trust is required only interactively; the TUI is the interactive surface.
        trust_rejected: workspace_trust_rejected(),
    };
    let tokens_at_start = runtime.session_usage_snapshot().await.totals.output_tokens;
    let history_snapshot = runtime.history().lock().await.to_vec();
    let outcome = goal_command::resolve_goal_request(
        request,
        runtime.app_state(),
        &runtime.hook_registry(),
        &history_snapshot,
        tokens_at_start,
        gate,
    )
    .await;

    match outcome {
        goal_command::GoalOutcome::Text(text) => {
            if is_status {
                let (title, body) =
                    build_goal_status_modal(session, &history_snapshot, tokens_at_start, text)
                        .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus { title, body }))
                    .await;
            } else {
                emit_slash_text(event_tx, "goal", &args, &text).await;
            }
            SlashFollowup::Done
        }
        goal_command::GoalOutcome::StatusThenText { status, text } => {
            append_goal_status_and_slash_text(session, event_tx, status, &args, &text).await;
            emit_active_goal_snapshot(session, event_tx).await;
            SlashFollowup::Done
        }
        goal_command::GoalOutcome::SetAndRun {
            status,
            text,
            kickoff,
        } => {
            append_goal_status(session, event_tx, status).await;
            emit_active_goal_snapshot(session, event_tx).await;
            emit_slash_text(event_tx, "goal", &args, &text).await;
            SlashFollowup::RunEngine {
                content: kickoff,
                metadata: Some(format_slash_command_metadata("goal", &args)),
                thinking_level: None,
                model_runtime_source: None,
            }
        }
    }
}

async fn build_goal_status_modal(
    session: &crate::session_runtime::SessionHandle,
    history: &[std::sync::Arc<coco_messages::Message>],
    current_output_tokens: i64,
    fallback_text: String,
) -> (String, String) {
    let runtime = session;
    if let Some(goal) = runtime.app_state().read().await.active_goal.clone() {
        return (
            "Goal active".to_string(),
            active_goal_modal_body(&goal, current_output_tokens),
        );
    }
    if let Some(goal) = goal_command::find_latest_goal_status(history)
        && goal.met
        && !goal.failed
        && !goal.sentinel
    {
        return ("Goal achieved".to_string(), achieved_goal_modal_body(&goal));
    }
    ("Goal".to_string(), fallback_text)
}

fn active_goal_modal_body(goal: &coco_types::ActiveGoal, current_output_tokens: i64) -> String {
    let mut lines = vec![
        format!(
            "Running: {}",
            format_goal_duration_ms(goal_command::unix_time_ms().saturating_sub(goal.set_at_ms))
        ),
        format!(
            "Tokens: {}",
            current_output_tokens.saturating_sub(goal.tokens_at_start)
        ),
        format!("Iterations: {}", format_goal_iterations(goal.iterations)),
        String::new(),
        "Goal:".to_string(),
        goal.condition.clone(),
    ];
    if let Some(reason) = goal
        .last_reason
        .as_deref()
        .map(goal_command::format_goal_last_reason)
        .filter(|reason| !reason.is_empty())
    {
        lines.extend([String::new(), "Last check:".to_string(), reason]);
    }
    lines.extend([String::new(), "/goal clear to stop early".to_string()]);
    lines.join("\n")
}

fn achieved_goal_modal_body(goal: &coco_types::GoalStatusPayload) -> String {
    let mut lines = Vec::new();
    let mut stats = Vec::new();
    if let Some(duration_ms) = goal.duration_ms {
        stats.push(format!("duration {}", format_goal_duration_ms(duration_ms)));
    }
    if let Some(iterations) = goal.iterations {
        stats.push(format!(
            "{} {}",
            iterations,
            if iterations == 1 { "turn" } else { "turns" }
        ));
    }
    if let Some(tokens) = goal.tokens {
        stats.push(format!("{} tokens", tokens.max(0)));
    }
    if !stats.is_empty() {
        lines.push(format!("Stats: {}", stats.join(" · ")));
        lines.push(String::new());
    }
    lines.push("Goal:".to_string());
    lines.push(goal.condition.clone());
    if let Some(reason) = goal
        .reason
        .as_deref()
        .map(goal_command::format_goal_last_reason)
        .filter(|reason| !reason.is_empty())
    {
        lines.extend([String::new(), "Reason:".to_string(), reason]);
    }
    lines.join("\n")
}

fn format_goal_iterations(iterations: i32) -> String {
    if iterations <= 0 {
        "not yet evaluated".to_string()
    } else {
        format!(
            "{} {}",
            iterations,
            if iterations == 1 { "turn" } else { "turns" }
        )
    }
}

fn format_goal_duration_ms(ms: i64) -> String {
    let seconds = (ms / 1000).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{minutes}m")
        }
    }
}

async fn append_goal_status(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
) {
    let runtime = session;
    let message = goal_status_message(payload);
    let mut history = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx_opt).await;
}

async fn append_goal_status_and_slash_text(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let runtime = session;
    let mut messages = vec![goal_status_message(payload)];
    messages.extend(coco_messages::build_slash_command_messages(
        "goal", args, text, /*is_sensitive*/ false,
    ));
    {
        let mut history = runtime.history().lock().await;
        let event_tx_opt = Some(event_tx.clone());
        for message in messages.iter().cloned() {
            coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx_opt)
                .await;
        }
    }
    runtime.persist_local_transcript_messages(&messages).await;
}

fn goal_status_message(payload: coco_types::GoalStatusPayload) -> coco_messages::Message {
    coco_messages::Message::Attachment(coco_messages::AttachmentMessage::silent_goal_status(
        payload,
    ))
}

async fn emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    let goal = runtime.app_state().read().await.active_goal.clone();
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

fn workspace_trust_rejected() -> bool {
    workspace_trust_rejected_from_env(
        std::env::var("COCO_WORKSPACE_TRUST_ACCEPTED")
            .ok()
            .as_deref(),
    )
}

fn workspace_trust_rejected_from_env(value: Option<&str>) -> bool {
    matches!(value, Some("0"))
}

/// `/add-dir <path>` runner — validates and routes a session-scoped
/// `AddDirectories` update through local AppServer so the next batch's
/// permission context sees the wider scope. Source is `Session` — never
/// persisted to settings.json.
async fn dispatch_add_dir(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> SlashOutcome {
    let runtime = session;
    let raw_path = args.trim();
    let current_cwd = runtime.current_cwd().read().await.clone();
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        current_cwd.join(raw_path)
    };
    let absolute = match candidate.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            emit_slash_text(
                event_tx,
                "add-dir",
                args,
                &format!("Cannot add directory `{raw_path}`: {e}"),
            )
            .await;
            return SlashOutcome::Handled;
        }
    };
    if !absolute.is_dir() {
        emit_slash_text(
            event_tx,
            "add-dir",
            args,
            &format!(
                "Cannot add directory `{}`: not a directory",
                absolute.display()
            ),
        )
        .await;
        return SlashOutcome::Handled;
    }

    let current = canonicalize_or_self(current_cwd);
    let additional_dirs: Vec<PathBuf> = runtime
        .app_state()
        .read()
        .await
        .permissions
        .additional_dirs
        .values()
        .map(|dir| canonicalize_or_self(PathBuf::from(&dir.path)))
        .collect();

    if let Some(message) = add_dir_already_message(&absolute, &current, &additional_dirs) {
        emit_slash_text(event_tx, "add-dir", args, &message).await;
        return SlashOutcome::Handled;
    }

    let path = absolute.to_string_lossy().into_owned();
    if !apply_session_add_directory(&path, event_tx, local_app_server_bridge).await {
        return SlashOutcome::Handled;
    }
    emit_slash_text(
        event_tx,
        "add-dir",
        args,
        &format!("Added {} as a working directory.", absolute.display()),
    )
    .await;
    SlashOutcome::Handled
}

fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn add_dir_already_message(
    directory_path: &Path,
    current_cwd: &Path,
    additional_dirs: &[PathBuf],
) -> Option<String> {
    if directory_path == current_cwd {
        return Some(format!(
            "{} is already the current working directory.",
            directory_path.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path == working_dir {
            return Some(format!(
                "{} is already added as a working directory.",
                directory_path.display()
            ));
        }
    }
    if directory_path.starts_with(current_cwd) {
        return Some(format!(
            "{} is already accessible within the current working directory {}.",
            directory_path.display(),
            current_cwd.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path.starts_with(working_dir) {
            return Some(format!(
                "{} is already accessible within the additional working directory {}.",
                directory_path.display(),
                working_dir.display()
            ));
        }
    }
    None
}

async fn apply_session_add_directory(
    path: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> bool {
    apply_and_persist_permission_update(
        &coco_types::PermissionUpdate::AddDirectories {
            directories: vec![path.to_string()],
            destination: coco_types::PermissionUpdateDestination::Session,
        },
        event_tx,
        local_app_server_bridge,
    )
    .await
}

/// `/tag <name>` runner — toggles the tag via `SessionManager`. Reports
/// "added" or "removed" so the user knows the new state.
async fn run_session_tag(
    _session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
    tag: &str,
) {
    let result = local_app_server_bridge
        .client()
        .session_toggle_tag(
            local_app_server_bridge.handler(),
            coco_types::SessionToggleTagParams {
                tag: tag.to_string(),
            },
        )
        .await;
    let text = match result {
        Ok(result) if result.added => format!("Tag added: {}", result.tag),
        Ok(result) => format!("Tag removed: {}", result.tag),
        Err(error) => format!("Failed to toggle tag `{tag}`: {error}"),
    };
    emit_slash_text(event_tx, "tag", tag, &text).await;
}

/// `/permissions allow|deny|reset` dispatch with live-base mutation.
/// The static registry handler can return text but can't mutate the live
/// `ToolAppState.permissions` base. This intercepts the three mutating
/// subcommands so they take real effect — routing allow/deny through
/// `control/applyPermissionUpdate` (live base + disk persist) and reset
/// through local AppServer runtime control; `list` /
/// no-arg fall through to the registry handler that reads settings.json.
/// Returns `None` for non-mutating args so the caller falls through.
/// `/color <name|default>` — set the prompt bar color for this session.
/// Persists to the live `ToolAppState.agent_color` so the prompt-bar UI
/// sees the change without a session restart. Returns `None` for the
/// empty-args case so the registry handler still produces the
/// "Available colors: …" listing.
async fn dispatch_color(
    args: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> Option<SlashOutcome> {
    use coco_coordinator::identity::is_teammate;
    use coco_types::AgentColorName;

    if is_teammate() {
        emit_slash_text(
            event_tx,
            "color",
            args,
            "Cannot set color: This session is a swarm teammate. \
             Teammate colors are assigned by the team leader.",
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    let trimmed = args.trim();
    if trimmed.is_empty() {
        // Empty args fall through to the registry handler, which
        // produces the canonical "Please provide a color..." listing
        // (identical to the registry handler's empty-args output).
        return None;
    }

    // Reset aliases.
    const RESET_ALIASES: &[&str] = &["default", "reset", "none", "gray", "grey"];
    let lower = trimmed.to_ascii_lowercase();
    if RESET_ALIASES.contains(&lower.as_str()) {
        if !set_agent_color(None, event_tx, local_app_server_bridge).await {
            return Some(SlashOutcome::Handled);
        }
        emit_slash_text(event_tx, "color", args, "Session color reset to default").await;
        return Some(SlashOutcome::Handled);
    }

    match lower.parse::<AgentColorName>() {
        Ok(color) => {
            if !set_agent_color(Some(color), event_tx, local_app_server_bridge).await {
                return Some(SlashOutcome::Handled);
            }
            emit_slash_text(
                event_tx,
                "color",
                args,
                &format!("Session color set to: {color}"),
            )
            .await;
            Some(SlashOutcome::Handled)
        }
        Err(_) => {
            let list = AgentColorName::ALL
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            emit_slash_text(
                event_tx,
                "color",
                args,
                &format!("Invalid color \"{lower}\". Available colors: {list}, default"),
            )
            .await;
            Some(SlashOutcome::Handled)
        }
    }
}

async fn dispatch_permissions_mutation(
    args: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> Option<SlashOutcome> {
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;

    // Empty `allow` / `deny` (no tool name) is a usage error — surface
    // the hint without falling through to the registry handler. The
    // pure parser returns `None` in that case (vs. None for read-only
    // / unrecognized which DO fall through).
    let trimmed = args.trim();
    if trimmed == "allow" || trimmed.starts_with("allow  ") || trimmed == "allow " {
        // Route through the typed status enum so the TUI translates via
        // `slash.permissions.usage_allow` (i18n parity with the other
        // dispatcher status messages).
        emit_slash_status(
            event_tx,
            "permissions",
            args,
            SlashCommandStatusKind::PermissionsUsageAllow,
        )
        .await;
        return Some(SlashOutcome::Handled);
    }
    if trimmed == "deny" || trimmed.starts_with("deny  ") || trimmed == "deny " {
        emit_slash_status(
            event_tx,
            "permissions",
            args,
            SlashCommandStatusKind::PermissionsUsageDeny,
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    let mutation = parse_permissions_mutation(args)?;

    let confirmation = match &mutation {
        PermissionsMutation::Allow(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Allow,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            if !apply_and_persist_permission_update(
                &coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            format!(
                "Added allow rule for `{tool}`.\n\nSource: Session (highest priority — \
                 active until end of session or `/permissions reset`)."
            )
        }
        PermissionsMutation::Deny(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Deny,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            if !apply_and_persist_permission_update(
                &coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            format!(
                "Added deny rule for `{tool}`.\n\nSource: Session (highest priority — \
                 active until end of session or `/permissions reset`)."
            )
        }
        PermissionsMutation::Reset => {
            // Reset is Session-source-only and never persists to disk. Route
            // through AppServer so the TUI does not mutate SessionRuntime.
            if !reset_session_permission_rules(event_tx, local_app_server_bridge).await {
                return Some(SlashOutcome::Handled);
            }
            {
                let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
                format!(
                    "Session permission rules reset. Custom session allow/deny entries were cleared; \
                     built-in read-only tools remain allowed by the active permission mode. File-based rules \
                     ({config_dir}/settings.json, ~/{config_dir}/settings.json) are unchanged — \
                     edit those files directly to modify persistent rules."
                )
            }
        }
    };
    emit_slash_text(event_tx, "permissions", args, &confirmation).await;
    Some(SlashOutcome::Handled)
}

/// Emit a `TuiOnlyEvent::SlashCommandResult` so the TUI appends a
/// system-role chat message carrying handler-rendered content (verbatim,
/// no translation).
async fn emit_slash_text(event_tx: &mpsc::Sender<CoreEvent>, name: &str, args: &str, text: &str) {
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandResult {
            name: name.to_string(),
            args: args.to_string(),
            text: text.to_string(),
        }))
        .await;
}

async fn dispatch_context(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> SlashOutcome {
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    match local_app_server_bridge
        .client()
        .context_usage(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenContextUsage { result }))
                .await;
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "context",
                /*args*/ "",
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// Optional `/login <provider>` arg → instance name. Empty → builtin default.
fn slash_provider_arg(args: &str) -> Option<String> {
    let a = args.trim();
    (!a.is_empty()).then(|| a.to_string())
}

/// `/login [provider]` — runs the OAuth flow on the shared `AuthService`, shows
/// the authorize URL + result in the transcript. Loopback-only (the TUI owns
/// stdin, so the paste fallback isn't available in-session — use `coco login
/// --no-browser` on a plain terminal for that).
/// Rebuild provider availability and push it to the TUI so the `/model`
/// picker reflects a credential change (login/logout) without a restart.
/// Only `provider_statuses` is auth-dependent — the model catalog and role
/// map derive from static config and are left untouched.
async fn emit_provider_statuses_refresh(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    let statuses = build_provider_statuses(runtime.runtime_config())
        .into_iter()
        .map(|(provider, status)| coco_types::ProviderStatusInfo {
            provider,
            provider_display: status.provider_display,
            unavailable_reasons: status.unavailable_reasons,
        })
        .collect();
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::ProviderStatusesRefreshed {
            statuses,
        }))
        .await;
}

async fn dispatch_provider_login(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let runtime = session;
    let provider = slash_provider_arg(args);
    let tx = event_tx.clone();
    let url_sink: std::sync::Arc<dyn Fn(String) + Send + Sync> = std::sync::Arc::new(move |url| {
        let _ = tx.try_send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandResult {
            name: "login".to_string(),
            args: String::new(),
            text: format!("Opening your browser to sign in. If it doesn't open, visit:\n{url}"),
        }));
    });
    let cwd = runtime.current_cwd().read().await.clone();
    match coco_cli::provider_login::run_login_session(provider, &cwd, url_sink).await {
        Ok(msg) => {
            emit_slash_text(event_tx, "login", args, &msg).await;
            emit_provider_statuses_refresh(session, event_tx).await;
            // Best-effort: discover the provider's live model list so
            // subscription-only models surface in `/model` without a restart.
            let instance =
                coco_cli::provider_login::instance_name(slash_provider_arg(args).as_deref());
            let base = model_catalog_infos(runtime.runtime_config());
            coco_cli::openai_model_refresh::spawn_after_login(
                session.clone(),
                instance,
                event_tx.clone(),
                base,
            );
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "login",
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// `/logout [provider]` — clears the subscription credential on the shared
/// `AuthService` (best-effort server-side revocation included).
async fn dispatch_provider_logout(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let provider = slash_provider_arg(args);
    match coco_cli::provider_login::run_logout_session(provider).await {
        Ok(msg) => {
            emit_slash_text(event_tx, "logout", args, &msg).await;
            emit_provider_statuses_refresh(session, event_tx).await;
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "logout",
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// Emit a `TuiOnlyEvent::SlashCommandStatus` so the TUI renders a
/// localized dispatcher breadcrumb (handler missing, handler error,
/// empty Prompt body, dialog wiring pending).
async fn emit_slash_status(
    event_tx: &mpsc::Sender<CoreEvent>,
    name: &str,
    args: &str,
    kind: SlashCommandStatusKind,
) {
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandStatus {
            name: name.to_string(),
            args: args.to_string(),
            kind,
        }))
        .await;
}

/// One-shot, fire-and-forget title generation. Returns immediately
/// without spawning if any precondition (auto-title disabled, already
/// attempted for this session id, no Fast spec, plan not exited,
/// plan empty) fails.
async fn maybe_spawn_auto_title(
    session: &crate::session_runtime::SessionHandle,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    session_id: &coco_types::SessionId,
    client: coco_app_server_client::ServerClient<coco_cli::sdk_server::LocalAppSessionHandle>,
    handler: coco_cli::sdk_server::AppServerSdkHandler,
) {
    let runtime = session;
    let plan_exited = runtime.app_state().read().await.has_exited_plan_mode;
    let plans_dir = coco_context::resolve_plans_directory(
        runtime.config_home(),
        /*project_dir*/ None,
        /*setting*/ None,
    );
    let plan_text = coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
    let plan_non_empty = plan_text
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    let already_attempted = title_gen_attempted
        .read()
        .await
        .contains(session_id.as_str());
    if !should_trigger_title_gen(
        runtime.auto_title_enabled(),
        already_attempted,
        runtime.fast_model_spec().is_some(),
        plan_exited,
        plan_non_empty,
    ) {
        return;
    }
    let (Some(_spec), Some(text)) = (runtime.fast_model_spec().cloned(), plan_text) else {
        return;
    };
    title_gen_attempted
        .write()
        .await
        .insert(session_id.to_string());
    spawn_auto_title_task(session.clone(), text, client, handler);
}

/// Synchronous TUI-cancel cleanup.
/// Truncates the runtime history at the target user message and emits
/// the authoritative `MessageTruncated` event so SDK + TUI observers
/// converge. Never touches the workspace — file rewind belongs to the
/// explicit [`handle_rewind`] flow. See
/// `engine-tui-unified-transcript-plan.md` §7.4.
async fn handle_auto_truncate(
    message_id: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    session: &crate::session_runtime::SessionHandle,
) {
    let runtime = session;
    let mut h = runtime.history().lock().await;
    let Some(idx) = h.as_slice().iter().position(|m| match m.as_ref() {
        coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
        _ => false,
    }) else {
        // Auto-restore is fire-and-forget; if the target uuid is gone
        // (e.g. a compaction wiped it between TUI dispatch and engine
        // handler), we'd rather skip silently than panic. `warn` so
        // ops can correlate "auto-restore quietly did nothing" with
        // an upstream truncation race.
        tracing::warn!(
            target: "coco_cli::auto_truncate",
            message_id,
            history_len = h.len(),
            "AutoTruncate target message not found in history (likely raced with compaction)",
        );
        return;
    };
    let pre_count = h.len() as i32;
    let removed = (pre_count - idx as i32).max(0);
    h.truncate(idx);
    tracing::info!(
        target: "coco_cli::auto_truncate",
        message_id,
        keep_count = idx,
        removed,
        "AutoTruncate applied",
    );
    coco_otel::events::emit_conversation_rewind(
        pre_count as i64,
        h.len() as i64,
        removed as i64,
        idx as i64,
    );
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::MessageTruncated {
            keep_count: idx as i64,
            identity: coco_types::ServerNotificationIdentity::default(),
        }))
        .await;
}

/// Explicit `/rewind` command driver — picker-confirmed.
/// Branches on `restore_type`:
/// - `Both` / `CodeOnly` — `file_history.rewind()` restores files.
/// - `Both` / `ConversationOnly` — truncate history and emit
/// `MessageTruncated`.
/// - `SummarizeFrom` / `SummarizeUpTo` — dispatch to
/// `handle_summarize_rewind` (partial compaction).
/// Always emits `RewindCompleted` so the TUI dismisses the picker overlay.
async fn handle_rewind(
    restore_type: &coco_tui::state::RestoreType,
    message_id: &str,
    rewound_turn: i32,
    event_tx: &mpsc::Sender<CoreEvent>,
    session: &crate::session_runtime::SessionHandle,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    use coco_tui::state::RestoreType;

    let mut files_changed = 0i32;
    let mut messages_removed = 0i32;
    let mut keep_count_to_emit = None;
    let mut history_replacement_to_emit: Option<Vec<coco_messages::Message>> = None;

    tracing::info!(
        target: "coco_cli::rewind",
        message_id,
        rewound_turn,
        ?restore_type,
        "Explicit rewind: dispatching",
    );

    // Summarize variants: dispatch to partial_compact_conversation
    // and replace the history with the resulting messages.
    if matches!(
        restore_type,
        RestoreType::SummarizeFrom { .. } | RestoreType::SummarizeUpTo { .. }
    ) {
        handle_summarize_rewind(restore_type, message_id, session, event_tx).await;
        return;
    }

    // Code rewind (file restore)
    // CodeOnly + Both restore files; Summarize variants do NOT
    // restore files — summarize keeps the workspace intact, only
    // the conversation is rewritten.
    if matches!(restore_type, RestoreType::Both | RestoreType::CodeOnly)
        && runtime.file_history().is_some()
    {
        local_app_server_bridge
            .install_session_runtime(session.clone())
            .await;
        match local_app_server_bridge
            .client()
            .rewind_files(
                local_app_server_bridge.handler(),
                coco_types::RewindFilesParams {
                    user_message_id: message_id.to_string(),
                    dry_run: false,
                },
            )
            .await
        {
            Ok(result) => {
                files_changed = result.files_changed.len() as i32;
                info!(files_changed, message_id, "File history rewind completed");
            }
            Err(error) => {
                warn!("File history rewind failed: {error}");
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::Error(
                        coco_types::ErrorParams {
                            message: format!("File rewind failed: {error}"),
                            category: Some("rewind".into()),
                            retryable: false,
                        },
                    )))
                    .await;
                return;
            }
        }
    }

    // Conversation rewind: truncate the agent-side history at the
    // target message, emit TuiOnlyEvent so the TUI mirrors the
    // truncate on its display side.
    let should_truncate = matches!(
        restore_type,
        RestoreType::Both | RestoreType::ConversationOnly
    );

    if should_truncate {
        let mut h = runtime.history().lock().await;
        match h.as_slice().iter().position(|m| match m.as_ref() {
            coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
            _ => false,
        }) {
            Some(idx) => {
                let pre_count = h.len() as i32;
                messages_removed = (pre_count - idx as i32).max(0);
                h.truncate(idx);
                tracing::info!(
                    target: "coco_cli::rewind",
                    message_id,
                    keep_count = idx,
                    messages_removed,
                    files_changed,
                    "Explicit rewind: truncated history",
                );
                coco_otel::events::emit_conversation_rewind(
                    pre_count as i64,
                    h.len() as i64,
                    messages_removed as i64,
                    idx as i64,
                );
                keep_count_to_emit = Some(idx as i64);
            }
            None => {
                let history_len = h.len();
                drop(h);
                if let Some((keep_count, removed, kept)) =
                    runtime.restore_pre_clear_rewind_prefix(message_id).await
                {
                    messages_removed = removed;
                    keep_count_to_emit = Some(keep_count as i64);
                    history_replacement_to_emit = Some(kept);
                    tracing::info!(
                        target: "coco_cli::rewind",
                        message_id,
                        keep_count,
                        messages_removed,
                        "Explicit rewind: restored pre-clear history prefix",
                    );
                } else {
                    tracing::warn!(
                        target: "coco_cli::rewind",
                        message_id,
                        history_len,
                        "Explicit rewind: target user message not found in history",
                    );
                }
            }
        }
    }

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::RewindCompleted {
            target_message_id: if should_truncate {
                message_id.to_string()
            } else {
                String::new()
            },
            files_changed,
        }))
        .await;

    // Explicit-rewind converges on the same `MessageTruncated` event the
    // AutoRestore path emits, but it must arrive after the TUI-only
    // completion event. `on_rewind_completed` restores the selected prompt
    // from the still-intact transcript before this truncation applies.
    if let Some(keep_count) = keep_count_to_emit {
        if let Some(messages) = history_replacement_to_emit {
            let _ = event_tx
                .send(CoreEvent::Protocol(ServerNotification::HistoryReplaced {
                    messages: messages.into_iter().map(Arc::new).collect(),
                    identity: coco_types::ServerNotificationIdentity::default(),
                    reason: coco_types::HistoryReplaceReason::Rewind,
                }))
                .await;
        }
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::MessageTruncated {
                keep_count,
                identity: coco_types::ServerNotificationIdentity::default(),
            }))
            .await;
    }

    // Protocol-level event for SDK consumers (Phase 3.2).
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::RewindCompleted(
            coco_types::RewindCompletedParams {
                rewound_turn,
                restored_files: files_changed,
                messages_removed,
            },
        )))
        .await;
}

/// Run `partial_compact_conversation` for SummarizeFrom / SummarizeUpTo
/// rewind options, replace the agent history with the result, and
/// emit a TUI signal to mirror the truncation in the display.
/// Direction mapping: `SummarizeFrom` == `Newest`; `SummarizeUpTo` == `Oldest`.
async fn handle_summarize_rewind(
    restore_type: &coco_tui::state::RestoreType,
    message_id: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    use coco_messages::PartialCompactDirection;
    use coco_tui::state::RestoreType;

    let (direction, feedback) = match restore_type {
        RestoreType::SummarizeFrom { feedback } => (PartialCompactDirection::Newest, feedback),
        RestoreType::SummarizeUpTo { feedback } => (PartialCompactDirection::Oldest, feedback),
        _ => return,
    };

    let messages: Vec<std::sync::Arc<coco_messages::Message>> = {
        let h = runtime.history().lock().await;
        h.as_slice().to_vec()
    };

    // Pivot index: position of the picked user message in the
    // history vec.
    let pivot_index = match messages.iter().position(|m| match m.as_ref() {
        coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
        _ => false,
    }) {
        Some(i) => i,
        None => {
            warn!(
                message_id,
                "summarize-rewind: target message not found in history"
            );
            let _ = event_tx
                .send(CoreEvent::Protocol(coco_query::ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: "summarize: message not in active history".into(),
                        category: Some("rewind".into()),
                        retryable: false,
                    },
                )))
                .await;
            return;
        }
    };

    let engine = runtime.build_engine(CancellationToken::new()).await;
    let mut history = coco_messages::MessageHistory::new();
    for arc in messages {
        history.push_arc(arc);
    }
    let event_tx_opt = Some(event_tx.clone());
    let outcome = engine
        .run_partial_compact(
            &mut history,
            &event_tx_opt,
            pivot_index,
            direction,
            feedback.clone(),
            /*custom_instructions*/ None,
        )
        .await;

    match outcome {
        coco_compact::CompactOutcome::Applied => {
            {
                let mut h = runtime.history().lock().await;
                *h = history;
            }
            // Emit a RewindCompleted with empty target so the TUI
            // dismisses the modal + shows a toast, but does NOT try
            // to truncate by message_id (the message is gone after
            // summarization).
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::RewindCompleted {
                    target_message_id: String::new(),
                    files_changed: 0,
                }))
                .await;
        }
        coco_compact::CompactOutcome::Skipped | coco_compact::CompactOutcome::Failed => {
            warn!("partial-compact rewind failed");
            let _ = event_tx
                .send(CoreEvent::Protocol(coco_query::ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: "Summarize failed".into(),
                        category: Some("rewind".into()),
                        retryable: false,
                    },
                )))
                .await;
        }
    }
}

/// Decide whether the driver should fire an auto-title task this turn.
/// Pure gate function factored out of the driver loop so we can unit
/// test the precedence without spinning up a real engine. All five
/// conditions must hold; missing any single one short-circuits.
fn should_trigger_title_gen(
    auto_title_enabled: bool,
    already_attempted: bool,
    fast_spec_present: bool,
    plan_has_exited: bool,
    plan_text_non_empty: bool,
) -> bool {
    auto_title_enabled
        && !already_attempted
        && fast_spec_present
        && plan_has_exited
        && plan_text_non_empty
}

/// Spawn a detached tokio task that auto-names the session from the approved
/// plan text via the same generator used by bare `/rename`.
fn spawn_auto_title_task(
    session: crate::session_runtime::SessionHandle,
    plan_text: String,
    client: coco_app_server_client::ServerClient<coco_cli::sdk_server::LocalAppSessionHandle>,
    handler: coco_cli::sdk_server::AppServerSdkHandler,
) {
    tokio::spawn(async move {
        let session_id = session.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if session
            .session_manager()
            .load(&session_id_string)
            .map(|session| session.title.is_some())
            .unwrap_or(false)
        {
            return;
        }

        let plan_head = plan_text.chars().take(1_000).collect::<String>();
        let Ok(name) = coco_cli::session_rename::generate_session_name_from_text(
            session.side_query(),
            plan_head,
        )
        .await
        else {
            return;
        };
        let session_id = session.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if session
            .session_manager()
            .load(&session_id_string)
            .map(|session| session.title.is_some())
            .unwrap_or(false)
        {
            return;
        }
        let _ = client
            .session_rename(&handler, coco_types::SessionRenameParams { name })
            .await;
    });
}

/// Persist a `skill_overrides` JSON patch to
/// `project config dir/settings.local.json`, refresh the in-process
/// registry, and notify the TUI so the dialog's toast + `/`
/// autocomplete pick up the change.
/// **No user-visible string generation here** — the localized
/// "Updated N / No changes / Failed: …" toast is rendered by the
/// TUI from the `SkillOverridesSaved` event payload (the i18n
/// catalog is anchored at `coco-tui` and can't be reached from
/// `coco-cli`).
/// Steps:
/// - Atomic write to `project config dir/settings.local.json` via
/// [`coco_config::LocalSettingsWriter::write_local`] — the writer
/// also republishes `RuntimeConfig` synchronously so the next
/// agent turn reads the new tiers.
/// - Rebuild the command registry against the freshly-published
/// `RuntimeConfig` (NOT the stale snapshot in
/// `runtime.runtime_config()`) so the `off`-overridden skills drop
/// out of the visible command set.
/// - Push `AvailableCommandsRefreshed` so the TUI's `/`
/// autocomplete updates in the same frame.
/// - Emit `SkillOverridesSaved` so the TUI renders the localized
/// toast.
async fn handle_write_skill_overrides(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    patch: serde_json::Value,
    runtime_publisher: Option<&Arc<coco_config::RuntimePublisher>>,
    cwd: &std::path::Path,
    flag_settings: Option<&std::path::Path>,
) {
    let runtime = session;
    let result = match runtime_publisher {
        Some(publisher) => {
            let catalogs = coco_config::CatalogPaths::default();
            let write_result = coco_config::write_local_settings(
                cwd.to_path_buf(),
                flag_settings.map(std::path::Path::to_path_buf),
                catalogs,
                Arc::clone(publisher),
                patch,
            )
            .await;
            match write_result {
                Ok(()) => {
                    // Use the freshly-republished RuntimeConfig so
                    // the rebuilt registry sees the new tiers — the
                    // snapshot bound to SessionRuntime at startup
                    // would silently drop the changes.
                    let fresh = publisher.current();
                    // Sync the per-session engine_config too. Per-
                    // turn QueryEngine builds clone from
                    // `engine_config.skill_overrides`; without
                    // this update, every PR2 runtime gate
                    // (SkillTool / listing budget / reminder source)
                    // keeps reading the stale snapshot and the
                    // override silently fails to take effect.
                    let fresh_tiers = Arc::new(fresh.skill_overrides.clone());
                    runtime
                        .update_engine_config(move |cfg| {
                            cfg.skill_overrides = fresh_tiers;
                        })
                        .await;
                    let _ = runtime.reload_plugins_with(cwd, &fresh).await;
                    let snapshot = runtime.current_command_registry().await.snapshot_for_ui();
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
                            commands: snapshot,
                        }))
                        .await;
                    coco_types::SkillOverridesSaveResult::Ok
                }
                Err(e) => coco_types::SkillOverridesSaveResult::Err {
                    kind: save_error_kind(&e),
                    message: e.to_string(),
                },
            }
        }
        None => coco_types::SkillOverridesSaveResult::Err {
            kind: coco_types::SkillOverridesSaveErrorKind::NoPublisher,
            message: "settings hot-reload disabled; restart the process to pick up changes"
                .to_string(),
        },
    };

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SkillOverridesSaved { result }))
        .await;
}

/// Map a [`coco_config::SettingsWriteError`] to its wire-categorical
/// kind for the TUI to dispatch by category (toast severity / future
/// retry affordance) rather than rely on string parsing.
fn save_error_kind(e: &coco_config::SettingsWriteError) -> coco_types::SkillOverridesSaveErrorKind {
    use coco_config::SettingsWriteError as E;
    use coco_types::SkillOverridesSaveErrorKind as K;
    match e {
        E::Io { .. } => K::Io,
        E::Parse { .. } => K::Parse,
        E::Rebuild { .. } => K::Rebuild,
    }
}

/// Encode TUI paste-pill image bytes as base64 [`QueuedImage`]s for
/// `CommandQueue` storage. `QueuedImage` carries a base64 payload (the
/// shape coco-rs uses for system-reminder image attachments) so we
/// encode once at the bridge and the engine ships it through unchanged.
/// MIME defaults to `image/png` when missing.
fn image_data_to_queued(images: &[coco_tui::ImageData]) -> Vec<QueuedImage> {
    use base64::Engine;
    images
        .iter()
        .map(|img| QueuedImage {
            media_type: if img.mime.is_empty() {
                "image/png".to_string()
            } else {
                img.mime.clone()
            },
            data_base64: base64::engine::general_purpose::STANDARD.encode(&img.bytes),
        })
        .collect()
}

fn image_data_to_turn_start(
    images: &[coco_tui::ImageData],
) -> Vec<coco_types::QueuedCommandEditImage> {
    image_data_to_queued(images)
        .into_iter()
        .map(|image| coco_types::QueuedCommandEditImage {
            media_type: image.media_type,
            data_base64: image.data_base64,
        })
        .collect()
}

fn model_runtime_source_to_turn_start_selection(
    source: Option<coco_inference::ModelRuntimeSource>,
) -> Option<coco_types::ProviderModelSelection> {
    match source {
        Some(coco_inference::ModelRuntimeSource::Explicit(selection)) => Some(selection),
        Some(coco_inference::ModelRuntimeSource::Role(_)) | None => None,
    }
}

async fn refresh_plugin_dialog_payload(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = build_plugin_dialog_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenPluginDialog { payload }))
        .await;
}

async fn build_plugin_dialog_payload(
    session: &crate::session_runtime::SessionHandle,
) -> coco_types::PluginDialogPayload {
    let runtime = session;
    let cfg = runtime.current_engine_config().await;
    let project_dir = cfg.workspace_cwd();
    let config_home = runtime.config_home().clone();
    let plugins = coco_plugins::load_all_installed_plugins(&config_home, &project_dir);
    let policy = coco_plugins::security::EnterprisePolicy::from_managed_settings();

    let installed = plugins
        .iter()
        .map(|plugin| {
            let id = plugin.id.to_string();
            let blocked_by_policy = {
                let parsed = coco_plugins::identifier::PluginId::parse(&id);
                matches!(
                    coco_plugins::security::check_policy(&parsed, true, &policy),
                    coco_plugins::security::PolicyVerdict::BlockedPlugin { .. }
                        | coco_plugins::security::PolicyVerdict::BlockedMarketplace { .. }
                        | coco_plugins::security::PolicyVerdict::UnapprovedMarketplace { .. }
                        | coco_plugins::security::PolicyVerdict::UserScopeForbidden
                )
            };
            let source = match &plugin.load_source {
                coco_plugins::loader::PluginLoadSource::Marketplace { marketplace } => {
                    format!("marketplace:{marketplace}")
                }
                coco_plugins::loader::PluginLoadSource::SessionDir => "local".to_string(),
                coco_plugins::loader::PluginLoadSource::Builtin => "builtin".to_string(),
            };
            let options = plugin
                .manifest
                .user_config
                .as_ref()
                .map(|config| {
                    let mut rows = config
                        .iter()
                        .map(|(key, option)| coco_types::PluginDialogOptionRow {
                            key: key.clone(),
                            title: option.title.clone(),
                            description: option.description.clone(),
                            value_type: format!("{:?}", option.config_type).to_ascii_lowercase(),
                            required: option.required.unwrap_or(false),
                            current_value: option.default.clone(),
                        })
                        .collect::<Vec<_>>();
                    rows.sort_by(|a, b| a.key.cmp(&b.key));
                    rows
                })
                .unwrap_or_default();
            let mcp_servers = coco_plugins::mcp_bridge::load_plugin_mcp_servers(plugin)
                .into_iter()
                .map(|server| {
                    let display_name = server
                        .name
                        .strip_prefix("plugin:")
                        .unwrap_or(&server.name)
                        .to_string();
                    coco_types::PluginDialogMcpServerRow {
                        name: server.name,
                        display_name,
                        enabled: true,
                        needs_config: false,
                        tools: Vec::new(),
                        actions: vec![coco_types::PluginDialogAction {
                            label: "Show plugin info".to_string(),
                            plugin_args: format!("info {}", plugin.id.name),
                        }],
                    }
                })
                .collect();
            let mut actions = Vec::new();
            if plugin.enabled {
                actions.push(coco_types::PluginDialogAction {
                    label: "Disable plugin".to_string(),
                    plugin_args: format!("disable {id}"),
                });
            } else {
                actions.push(coco_types::PluginDialogAction {
                    label: "Enable plugin".to_string(),
                    plugin_args: format!("enable {id}"),
                });
            }
            actions.push(coco_types::PluginDialogAction {
                label: "Uninstall plugin".to_string(),
                plugin_args: format!("uninstall {id}"),
            });
            coco_types::PluginDialogInstalledRow {
                id,
                name: plugin.manifest.name.clone(),
                version: plugin.manifest.version.clone(),
                description: plugin.manifest.description.clone(),
                source,
                path: plugin.path.display().to_string(),
                enabled: plugin.enabled,
                blocked_by_policy,
                options,
                mcp_servers,
                actions,
            }
        })
        .collect();

    let skills = build_plugin_dialog_skill_rows(
        &runtime.skill_manager(),
        &cfg.skill_overrides,
        &config_home,
        coco_model_card::bytes_per_token_for_model(&cfg.model_id),
    );

    let plugins_dir = config_home.join("plugins");
    let mut manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    let known = manager.load_known_marketplaces();
    let mut marketplaces = Vec::new();
    for (name, known_marketplace) in known {
        let _ = manager.load_cached_marketplace(&name);
        let plugin_count = manager
            .cached_marketplace(&name)
            .map(|m| i64::try_from(m.plugins.len()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        marketplaces.push(coco_types::PluginDialogMarketplaceRow {
            official: coco_plugins::marketplace::is_official_marketplace_name(&name),
            source: Some(format!("{:?}", known_marketplace.source)),
            name: name.clone(),
            plugin_count,
            actions: vec![coco_types::PluginDialogAction {
                label: "Update marketplace".to_string(),
                plugin_args: format!("marketplace update {name}"),
            }],
        });
    }
    marketplaces.sort_by(|a, b| a.name.cmp(&b.name));

    coco_types::PluginDialogPayload {
        installed,
        skills,
        marketplaces,
        errors: Vec::new(),
    }
}

fn build_plugin_dialog_skill_rows(
    skill_manager: &Arc<coco_skills::SkillManager>,
    tiers: &coco_config::SkillOverrideTiers,
    config_home: &Path,
    bytes_per_token: i64,
) -> Vec<coco_types::PluginDialogSkillRow> {
    let usage = coco_skills::usage::load_all(config_home);
    let now_ms = system_time_ms();
    let bytes_per_token = bytes_per_token.max(1);
    let mut rows = skill_manager
        .all_including_conditional()
        .into_iter()
        .filter(|skill| {
            !matches!(
                skill.source,
                coco_skills::SkillSource::Bundled | coco_skills::SkillSource::Plugin { .. }
            )
        })
        .map(|skill| {
            let lock = coco_skills::resolve_skill_override_lock(&skill, tiers);
            let state = lock
                .as_ref()
                .map(|lock| lock.forced_value)
                .unwrap_or_else(|| coco_skills::effective_skill_state(&skill, tiers));
            let usage = usage.get(&skill.name).map(|stats| {
                let elapsed = now_ms.saturating_sub(stats.last_used_at_ms);
                coco_types::PluginDialogSkillUsage {
                    count: stats.usage_count,
                    days_since_use: elapsed / 86_400_000,
                }
            });
            let token_estimate =
                i64::try_from(coco_skills::estimate_skill_frontmatter_bytes(&skill))
                    .unwrap_or(i64::MAX)
                    / bytes_per_token;
            coco_types::PluginDialogSkillRow {
                id: format!("skill:{}", skill.name),
                name: skill.name.clone(),
                description: skill.description.clone(),
                source: plugin_dialog_skill_source(&skill.source),
                override_state: state,
                lock_source: lock.map(|lock| lock.source),
                token_estimate,
                usage,
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        plugin_dialog_skill_source_sort_key(a.source)
            .cmp(plugin_dialog_skill_source_sort_key(b.source))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn plugin_dialog_skill_source(source: &coco_skills::SkillSource) -> coco_types::SkillsDialogSource {
    match source {
        coco_skills::SkillSource::Bundled => coco_types::SkillsDialogSource::BuiltIn,
        coco_skills::SkillSource::Project { .. } => coco_types::SkillsDialogSource::Project,
        coco_skills::SkillSource::User { .. } => coco_types::SkillsDialogSource::User,
        coco_skills::SkillSource::Managed { .. } => coco_types::SkillsDialogSource::Policy,
        coco_skills::SkillSource::Plugin { .. } => coco_types::SkillsDialogSource::Plugin,
        coco_skills::SkillSource::Mcp { .. } => coco_types::SkillsDialogSource::Mcp,
    }
}

fn plugin_dialog_skill_source_sort_key(source: coco_types::SkillsDialogSource) -> &'static str {
    match source {
        coco_types::SkillsDialogSource::BuiltIn => "built-in",
        coco_types::SkillsDialogSource::Project => "project",
        coco_types::SkillsDialogSource::User => "user",
        coco_types::SkillsDialogSource::Policy => "policy",
        coco_types::SkillsDialogSource::Plugin => "plugin",
        coco_types::SkillsDialogSource::Mcp => "mcp",
    }
}

fn system_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Construct the engine `Message::System(...)` payload from a
/// TUI-originated [`coco_tui::SystemPushKind`]. Centralises the
/// kind → sub-variant mapping so every TUI-side push site agrees on
/// shape, and so adding a new kind only touches one match arm.
fn build_system_message_from_push_kind(kind: coco_tui::SystemPushKind) -> coco_messages::Message {
    let sys = match kind {
        coco_tui::SystemPushKind::Informational {
            level,
            title,
            message,
        } => {
            coco_messages::SystemMessage::Informational(coco_messages::SystemInformationalMessage {
                uuid: uuid::Uuid::new_v4(),
                level,
                title,
                message,
            })
        }
        coco_tui::SystemPushKind::LocalCommand { command, output } => {
            coco_messages::SystemMessage::LocalCommand(coco_messages::SystemLocalCommandMessage {
                uuid: uuid::Uuid::new_v4(),
                command,
                output,
            })
        }
        coco_tui::SystemPushKind::PermissionRetry { tool_name, message } => {
            coco_messages::SystemMessage::PermissionRetry(
                coco_messages::SystemPermissionRetryMessage {
                    uuid: uuid::Uuid::new_v4(),
                    tool_name,
                    message,
                },
            )
        }
    };
    coco_messages::Message::System(sys)
}

/// Run a prompt-mode bash submission (`!ls -la`). The command runs once in the
/// session cwd via [`coco_shell::ShellExecutor`] and the merged stdout+stderr
/// is folded back into the transcript as local-command output. By default this
/// then starts a model turn so the assistant responds to the shell output;
/// users can set `respondToBashCommands=false` to keep it context-only.
/// Output is capped at 200 lines / ~8 KB so a `find /` doesn't fill the
/// chat scrollback. The TUI's renderer already truncates display to 20
/// lines (`render_user.rs::BashOutput`) but we keep the wire payload
/// modest to avoid bloating the JSONL transcript.
async fn run_prompt_mode_bash(
    cwd: &std::path::Path,
    user_message_id: String,
    command: String,
    session: crate::session_runtime::SessionHandle,
    event_tx: mpsc::Sender<CoreEvent>,
    active_turn: Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: mpsc::Sender<uuid::Uuid>,
) {
    let runtime = &session;
    const MAX_OUTPUT_BYTES: usize = 8 * 1024;
    const MAX_OUTPUT_LINES: usize = 200;

    let mut executor = coco_shell::ShellExecutor::new(cwd);
    let exec_opts = coco_shell::ExecOptions::default();
    let mut command_failed_to_run = false;
    let (output, exit_code) = match executor.execute(&command, &exec_opts).await {
        Ok(result) => {
            let mut merged = String::new();
            if !result.stdout.is_empty() {
                merged.push_str(&result.stdout);
            }
            if !result.stderr.is_empty() {
                if !merged.is_empty() && !merged.ends_with('\n') {
                    merged.push('\n');
                }
                merged.push_str(&result.stderr);
            }
            (
                truncate_output(merged, MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
                result.exit_code,
            )
        }
        Err(err) => {
            command_failed_to_run = true;
            (format!("error: {err}"), -1)
        }
    };

    let should_respond = should_prompt_mode_bash_respond(&session) && !command_failed_to_run;

    // Push the local command into engine MessageHistory so the chat transcript
    // (TUI + SDK consumers + JSONL) records the bash invocation via the
    // standard `MessageAppended` event path. When the command is context-only,
    // prepend the carryover "DO NOT respond" caveat so a later model turn does
    // not comment on stale shell output.
    {
        let mut h = runtime.history().lock().await;
        let event_tx_opt = Some(event_tx.clone());
        if !should_respond {
            let caveat = coco_messages::create_meta_message(
                "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.</local-command-caveat>",
            );
            coco_query::history_sync::history_push_and_emit(&mut h, caveat, &event_tx_opt).await;
        }
        let msg = coco_messages::Message::System(coco_messages::SystemMessage::LocalCommand(
            coco_messages::SystemLocalCommandMessage {
                uuid: uuid::Uuid::new_v4(),
                command: command.clone(),
                output: output.clone(),
            },
        ));
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::BashCommandCompleted {
            user_message_id,
            output,
            exit_code,
        }))
        .await;

    if should_respond {
        let messages = {
            let h = runtime.history().lock().await;
            h.to_vec()
        };
        spawn_history_turn(messages, &session, &event_tx, &active_turn, &turn_done_tx).await;
    }
}

fn should_prompt_mode_bash_respond(session: &crate::session_runtime::SessionHandle) -> bool {
    session
        .runtime_config()
        .settings
        .merged
        .respond_to_bash_commands
        .unwrap_or(true)
}

/// Create a selected `/memory` target if needed and launch the configured
/// editor. Effects live in the CLI bridge so TUI reducers stay pure.
async fn run_open_memory_file(path: std::path::PathBuf, event_tx: mpsc::Sender<CoreEvent>) {
    let path_display = path.display().to_string();
    let result = tokio::task::spawn_blocking(move || open_memory_file_blocking(&path)).await;

    let event = match result {
        Ok(Ok(())) => TuiOnlyEvent::MemoryFileOpened { path: path_display },
        Ok(Err(error)) => TuiOnlyEvent::MemoryFileOpenFailed {
            path: path_display,
            error,
        },
        Err(err) => {
            warn!(error = %err, "memory editor task panicked");
            TuiOnlyEvent::MemoryFileOpenFailed {
                path: path_display,
                error: format!("memory editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

/// Create this session's plan target if needed and launch the configured
/// editor. Uses the same terminal handoff as prompt and memory editing.
async fn run_open_plan_file(path: std::path::PathBuf, event_tx: mpsc::Sender<CoreEvent>) {
    let path_display = path.display().to_string();
    let result = tokio::task::spawn_blocking(move || open_plan_file_blocking(&path)).await;

    let event = match result {
        Ok(Ok(())) => TuiOnlyEvent::PlanFileOpened { path: path_display },
        Ok(Err(error)) => TuiOnlyEvent::PlanFileOpenFailed {
            path: path_display,
            error,
        },
        Err(err) => {
            warn!(error = %err, "plan editor task panicked");
            TuiOnlyEvent::PlanFileOpenFailed {
                path: path_display,
                error: format!("plan editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

async fn run_plan_prompt_editor(
    request_id: String,
    initial_content: String,
    path: Option<std::path::PathBuf>,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let event_request_id = request_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        open_plan_prompt_editor_blocking(&initial_content, path.as_deref())
    })
    .await;

    let event = match result {
        Ok(Ok((content, modified))) => TuiOnlyEvent::ExitPlanPromptEditorCompleted {
            request_id: event_request_id,
            content,
            modified,
        },
        Ok(Err(error)) => TuiOnlyEvent::ExitPlanPromptEditorFailed {
            request_id: event_request_id,
            error,
        },
        Err(err) => {
            warn!(error = %err, "exit-plan prompt editor task panicked");
            TuiOnlyEvent::ExitPlanPromptEditorFailed {
                request_id: event_request_id,
                error: format!("plan editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

async fn emit_editor_prepare_failed(
    request: PendingEditorRequest,
    error: String,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let message = format!("failed to prepare terminal for editor: {error}");
    let event = match request {
        PendingEditorRequest::Memory { path } => TuiOnlyEvent::MemoryFileOpenFailed {
            path: path.display().to_string(),
            error: message,
        },
        PendingEditorRequest::Plan { path } => TuiOnlyEvent::PlanFileOpenFailed {
            path: path.display().to_string(),
            error: message,
        },
        PendingEditorRequest::PlanPrompt { request_id, .. } => {
            TuiOnlyEvent::ExitPlanPromptEditorFailed {
                request_id,
                error: message,
            }
        }
        PendingEditorRequest::Prompt { .. } => TuiOnlyEvent::PromptEditorFailed { error: message },
        // Agent editor preparation failure is surfaced via the
        // generic prompt-editor channel (no dedicated wire event).
        // The user still sees a toast and the dialog stays mounted.
        PendingEditorRequest::Agent { .. } => TuiOnlyEvent::PromptEditorFailed { error: message },
    };
    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

/// Typed error from `prepare_agent_create`. Variants map 1:1 to
/// `coco_tui::state::WizardError`; the CLI bridge produces them so
/// the TUI side can stamp the wizard's `error` slot with a typed
/// payload instead of trying to parse a stringly error.
#[derive(Debug)]
enum CreateAgentError {
    NonWritableSource(coco_types::AgentSource),
    AlreadyExists(std::path::PathBuf),
    Io(String),
}

impl CreateAgentError {
    fn to_user_string(&self) -> String {
        match self {
            Self::NonWritableSource(s) => {
                format!("source {s:?} is not writable from the wizard")
            }
            Self::AlreadyExists(p) => {
                format!("agent file already exists at {}", p.display())
            }
            Self::Io(m) => m.clone(),
        }
    }
}

/// Stage the new-agent markdown file ahead of the `$EDITOR` fork.
/// 1. Resolves the target directory via
/// [`coco_subagent::resolve_writable_agent_dir`].
/// 2. Pulls the live catalog snapshot **once** so the colour picker
/// and the post-write reload share the same view.
/// 3. Wraps `create_dir_all` + `write` in `spawn_blocking` so a slow
/// disk doesn't stall the async runtime.
/// 4. Refuses to overwrite an existing file.
/// The caller then hands off to the standard editor flow.
async fn prepare_agent_create(
    session: &crate::session_runtime::SessionHandle,
    name: &str,
    description: &str,
    source: coco_types::AgentSource,
) -> Result<std::path::PathBuf, CreateAgentError> {
    let runtime = session;
    // Snapshot the catalog ONCE — the colour picker reads it, and
    // the post-write reload supersedes it on its own. Repeated
    // `agent_catalog_snapshot().await` calls add lock churn for no
    // benefit since the data is immutable per snapshot.
    let snapshot = runtime.agent_catalog_snapshot().await;
    let color = coco_subagent::next_unused_color(&snapshot);

    let name_owned = name.to_string();
    let description_owned = description.to_string();
    let cwd = runtime.current_engine_config().await.workspace_cwd();
    let blocking =
        tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf, CreateAgentError> {
            let config_home = coco_config::global_config::config_home();
            let dir = coco_subagent::resolve_writable_agent_dir(source, &config_home, &cwd)
                .ok_or(CreateAgentError::NonWritableSource(source))?;
            std::fs::create_dir_all(&dir).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            let path = dir.join(format!("{name_owned}.md"));
            if path.exists() {
                return Err(CreateAgentError::AlreadyExists(path));
            }
            let template = build_agent_template(&name_owned, &description_owned, color);
            std::fs::write(&path, template).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            Ok(path)
        })
        .await
        .map_err(|join_err| CreateAgentError::Io(format!("write task panicked: {join_err}")))??;

    // Pre-warm the catalog so observers see the new file without
    // waiting on the editor to exit — handy for SDK consumers that
    // listen to `agents/refreshed` between the create and the edit.
    runtime.reload_agent_catalog().await;
    Ok(blocking)
}

/// Build the markdown body written by the create wizard. Frontmatter carries the wizard inputs plus
/// an auto-assigned color from the eight-color palette so new agents
/// land with visual distinction in the Library list.
fn build_agent_template(
    name: &str,
    description: &str,
    color: Option<coco_types::AgentColorName>,
) -> String {
    let description_yaml = yaml_single_quote(description);
    let color_line = match color {
        Some(c) => format!("color: {}\n", c.as_str()),
        None => String::new(),
    };
    format!(
        "---\n\
         name: {name}\n\
         description: {description_yaml}\n\
         {color_line}\
         ---\n\
         \n\
         # {name}\n\
         \n\
         <!-- Describe how this agent should behave. Frontmatter \
         fields you can add: tools, model, memory, isolation, \
         background, maxTurns, initialPrompt. -->\n",
    )
}

/// Encode a single-line string as a YAML single-quoted scalar. YAML
/// single-quoted form is the simplest robust escape: the only
/// in-string syntax is the single quote itself, which doubles to
/// `''`. Control characters and backslashes pass through literally,
/// dodging the double-quote escape surface entirely.
/// The wizard's `wizard_input_char` already rejects literal newlines
/// (`InsertNewline` is unbound) and control characters on the
/// description step, so by the time text reaches here it's a single
/// physical line — exactly what the YAML single-quoted format
/// requires.
fn yaml_single_quote(s: &str) -> String {
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// Fork `$EDITOR` against the agent markdown file. On clean exit
/// the runner triggers a `reload_agent_catalog()` **only when the
/// file actually changed** so an editor session that quit without
/// saving doesn't churn the catalog. Falls back to reload on any
/// mtime-read error so a missing-stat doesn't strand the dialog.
async fn run_open_agent_file(
    session: crate::session_runtime::SessionHandle,
    path: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let path_display = path.display().to_string();
    let mtime_before = file_mtime(&path);
    let editor_path = path.clone();
    let result = tokio::task::spawn_blocking(move || run_editor_on_file(&editor_path)).await;

    match result {
        Ok(Ok(())) => {
            let mtime_after = file_mtime(&path);
            // Skip the reload when mtime is known on both sides and
            // unchanged — common case for "opened, looked, quit
            // without writing". Either side missing falls back to
            // reload so a transient stat() failure doesn't desync
            // the dialog.
            let unchanged = matches!((mtime_before, mtime_after), (Some(a), Some(b)) if a == b);
            if unchanged {
                tracing::debug!(
                    target: "coco::agents",
                    %path_display,
                    "agent editor exited with no file changes; skipping reload"
                );
                refresh_agents_dialog(&session, &event_tx).await;
                return;
            }
            // Reload + republish the dialog payload so the user sees
            // their edits immediately. Live registry refresh + dialog
            // refresh both go through the existing wire so observers
            // (subagent dispatch, dialog renderer) stay coherent.
            session.reload_agent_catalog().await;
            refresh_agents_dialog(&session, &event_tx).await;
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco::agents",
                %path_display,
                %error,
                "agent editor failed"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "coco::agents",
                %path_display,
                error = %err,
                "agent editor task panicked"
            );
        }
    }
}

/// Read the file's modification time, dropping any error to `None`.
/// Used as a cheap change-detection signal for the post-edit reload
/// short-circuit; any stat hiccup falls back to the safe "reload"
/// path so we never serve a stale dialog.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Build a fresh `AgentsDialogPayload` from the live catalog snapshot
/// and push it to the TUI via `OpenAgentsDialog`. Used after CRUD
/// (`OpenAgentEditor` exit, `DeleteAgentFile`) so the dialog refreshes
/// in place rather than waiting for the user to re-issue `/agents`.
async fn refresh_agents_dialog(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    let snapshot = runtime.agent_catalog_snapshot().await;

    let active_source: std::collections::BTreeMap<String, coco_types::AgentSource> = snapshot
        .active()
        .map(|d| (d.name.clone(), d.source))
        .collect();

    let entries: Vec<coco_types::AgentsDialogEntry> = snapshot
        .all()
        .iter()
        .map(|loaded| {
            let def = &loaded.definition;
            let is_overridden = active_source
                .get(&def.name)
                .map(|winning| *winning != def.source)
                .unwrap_or(false);
            coco_types::AgentsDialogEntry {
                name: def.name.clone(),
                description: def.description.clone().unwrap_or_default(),
                source: def.source,
                color: def.color,
                is_overridden,
                source_path: loaded.path.clone(),
            }
        })
        .collect();
    let payload = coco_types::AgentsDialogPayload { entries };
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenAgentsDialog { payload }))
        .await;
}

/// Build a `PermissionsEditorPayload` snapshot from the on-disk settings
/// stores for the `/permissions` overlay. Reads every file-backed rule
/// (user / project / local / flag / policy) plus additional directories,
/// projecting them into the wire payload the TUI partitions into tabs.
async fn build_permissions_editor_payload(
    session: &crate::session_runtime::SessionHandle,
) -> coco_types::PermissionsEditorPayload {
    let runtime = session;
    use coco_permissions::permissions_store::PermissionStore;

    let cwd = runtime.current_engine_config().await.workspace_cwd();
    let store = coco_permissions::SettingsPermissionStore::new(cwd.clone());

    // Reading several small JSON files — push onto the blocking pool so a
    // slow filesystem can't stall the runner's command loop.
    let (rules, directories, managed_only) = tokio::task::spawn_blocking(move || {
        let by_behavior = store.load_all_rules();
        let rules: Vec<coco_types::PermissionsEditorRule> = by_behavior
            .allow
            .into_iter()
            .chain(by_behavior.ask)
            .chain(by_behavior.deny)
            .map(|r| coco_types::PermissionsEditorRule {
                behavior: r.behavior,
                source: r.source,
                tool_pattern: r.value.tool_pattern,
                rule_content: r.value.rule_content,
            })
            .collect();
        let directories: Vec<coco_types::PermissionsEditorDir> = store
            .load_additional_directories()
            .into_iter()
            .map(|(source, path)| coco_types::PermissionsEditorDir { path, source })
            .collect();
        // `show_always_allow_options()` is the inverse of managed-only.
        let managed_only = !store.show_always_allow_options();
        (rules, directories, managed_only)
    })
    .await
    .unwrap_or_else(|_| (Vec::new(), Vec::new(), false));

    coco_types::PermissionsEditorPayload {
        rules,
        directories,
        cwd: cwd.to_string_lossy().into_owned(),
        managed_only,
    }
}

/// Re-emit `OpenPermissionsEditor` with a fresh snapshot so the open
/// overlay refreshes in place after a persisted edit.
async fn refresh_permissions_editor(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = build_permissions_editor_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenPermissionsEditor {
            payload,
        }))
        .await;
}

/// Apply one `/permissions`-editor update to the live `ToolAppState.permissions`
/// base and persist it to its destination settings file. Mirrors the
/// `ApprovalResponse` "Always Allow" apply+persist path, but the editor
/// targets any of the three writable scopes (User / Project / Local).
/// Routes through local AppServer `control/applyPermissionUpdate`; the SDK
/// handler folds the update into the live base (via
/// `apply_permission_updates_to_live`) AND persists persistable destinations
/// to disk.
async fn apply_and_persist_permission_update(
    update: &coco_types::PermissionUpdate,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .apply_permission_update(
            local_app_server_bridge.handler(),
            coco_types::ApplyPermissionUpdateParams {
                update: update.clone(),
            },
        )
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to apply permission update: {error}"),
                    category: Some("permission_update_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

/// Clear session-scoped permission rules through local AppServer runtime control.
async fn reset_session_permission_rules(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .reset_session_permission_rules(local_app_server_bridge.handler())
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to reset session permission rules: {error}"),
                    category: Some("permission_reset_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

async fn set_agent_color(
    color: Option<coco_types::AgentColorName>,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .set_agent_color(
            local_app_server_bridge.handler(),
            coco_types::SetAgentColorParams { color },
        )
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to set session color: {error}"),
                    category: Some("agent_color_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

fn open_memory_file_blocking(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create parent directory: {err}"))?;
    }

    // `wx` semantics: create exclusively, but an existing memory file is
    // fine. We just need the target present before launching the editor.
    if let Err(err) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        && err.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("failed to create memory file: {err}"));
    }

    run_editor_on_file(path)
}

fn open_plan_file_blocking(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create plans directory: {err}"))?;
    }

    if let Err(err) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        && err.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("failed to create plan file: {err}"));
    }

    run_editor_on_file(path)
}

async fn run_prompt_editor(initial_content: String, event_tx: mpsc::Sender<CoreEvent>) {
    let result =
        tokio::task::spawn_blocking(move || open_prompt_editor_blocking(&initial_content)).await;

    let event = match result {
        Ok(Ok((content, modified))) => TuiOnlyEvent::PromptEditorCompleted { content, modified },
        Ok(Err(error)) => TuiOnlyEvent::PromptEditorFailed { error },
        Err(err) => {
            warn!(error = %err, "prompt editor task panicked");
            TuiOnlyEvent::PromptEditorFailed {
                error: format!("prompt editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

fn open_prompt_editor_blocking(initial_content: &str) -> Result<(String, bool), String> {
    let path = std::env::temp_dir().join(format!("coco-prompt-edit-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, initial_content)
        .map_err(|err| format!("failed to write editor temp file: {err}"))?;

    let result = run_editor_on_file(&path).and_then(|()| {
        let content = std::fs::read_to_string(&path)
            .map_err(|err| format!("failed to read editor temp file: {err}"))?;
        let modified = content != initial_content;
        Ok((content, modified))
    });

    if let Err(err) = std::fs::remove_file(&path)
        && result.is_ok()
    {
        return Err(format!("failed to remove editor temp file: {err}"));
    }

    result
}

fn open_plan_prompt_editor_blocking(
    initial_content: &str,
    path: Option<&std::path::Path>,
) -> Result<(String, bool), String> {
    let Some(path) = path else {
        return open_prompt_editor_blocking(initial_content);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create plans directory: {err}"))?;
    }
    if !path.exists() {
        std::fs::write(path, initial_content)
            .map_err(|err| format!("failed to write plan file: {err}"))?;
    }
    run_editor_on_file(path)?;
    let content =
        std::fs::read_to_string(path).map_err(|err| format!("failed to read plan file: {err}"))?;
    let modified = content != initial_content;
    Ok((content, modified))
}

fn resolve_editor_command() -> Result<(String, Vec<String>), String> {
    let raw = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());

    parse_editor_command(&raw)
}

fn parse_editor_command(raw: &str) -> Result<(String, Vec<String>), String> {
    let mut parts =
        shlex::split(raw).ok_or_else(|| format!("failed to parse editor command `{raw}`"))?;
    if parts.is_empty() {
        return Err("editor command resolved to an empty argv".to_string());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

fn run_editor_on_file(path: &std::path::Path) -> Result<(), String> {
    let (program, args) = resolve_editor_command()?;
    let status = std::process::Command::new(&program)
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to launch editor `{program}`: {err}"))?;

    if !status.success() {
        return Err(format!("editor `{program}` exited with status {status}"));
    }

    Ok(())
}

/// Cap `text` at the smaller of `max_bytes` or `max_lines`, appending a
/// short notice when truncation occurs. Splits on char boundaries so
/// UTF-8 stays intact even when the byte limit lands mid-codepoint.
fn truncate_output(text: String, max_bytes: usize, max_lines: usize) -> String {
    let line_count = text.lines().count();
    let byte_over = text.len() > max_bytes;
    if !byte_over && line_count <= max_lines {
        return text;
    }
    let mut truncated: String = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if truncated.len() > max_bytes {
        let cut = truncated
            .char_indices()
            .take_while(|(i, _)| *i <= max_bytes)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        truncated.truncate(cut);
    }
    truncated.push_str("\n… (truncated)");
    truncated
}

/// Build the TUI's session-frozen model catalog from the resolved
/// `ModelRegistry`. Each registered `(provider, model_id)` pair becomes
/// one entry; the same `model_id` shared across providers (e.g.
/// `deepseek-v4` under both `deepseek-openai` and `deepseek-anthropic`)
/// yields one entry per provider. Models not paired with any registered
/// provider are unreachable at runtime and therefore not surfaced.
fn build_model_catalog(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_tui::state::ModelCatalogEntry> {
    use coco_tui::state::ModelCatalogEntry;
    let mut entries: Vec<ModelCatalogEntry> = runtime_config
        .model_registry
        .resolved
        .iter()
        .map(|((provider, model_id), resolved)| {
            let info = &resolved.info;
            let supported_efforts: Vec<coco_types::ReasoningEffort> = info
                .supported_thinking_levels
                .as_ref()
                .map(|levels| levels.iter().map(|l| l.effort).collect())
                .unwrap_or_default();
            ModelCatalogEntry {
                provider: provider.clone(),
                provider_display: provider_display_label(provider),
                model_id: model_id.clone(),
                display_name: info
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model_id.clone()),
                context_window: Some(info.context_window.get() as i64),
                supported_efforts,
                default_effort: info.default_thinking_level,
            }
        })
        .collect();
    for endpoint in runtime_config.model_roles.moa_presets.values() {
        if entries.iter().any(|entry| {
            entry.provider == endpoint.display_provider()
                && entry.model_id == endpoint.display_model_id()
        }) {
            continue;
        }
        let context_window = runtime_config
            .model_registry
            .resolve(&endpoint.aggregator.provider, &endpoint.aggregator.model_id)
            .map(|resolved| resolved.info.context_window.get() as i64);
        entries.push(ModelCatalogEntry {
            provider: endpoint.display_provider().to_string(),
            provider_display: "MoA".to_string(),
            model_id: endpoint.display_model_id().to_string(),
            display_name: format!("MoA {}", endpoint.display_model_id()),
            context_window,
            supported_efforts: Vec::new(),
            default_effort: None,
        });
    }

    // Stable sort: provider_display → display_name. Matches the
    // picker's section-by-provider rendering.
    entries.sort_by(|a, b| {
        a.provider_display
            .cmp(&b.provider_display)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    entries
}

/// Convert the static model catalog into the wire payload used by the
/// post-login `/models` refresh (`ModelCatalogRefreshed`).
fn model_catalog_infos(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::ModelCatalogInfo> {
    build_model_catalog(runtime_config)
        .into_iter()
        .map(|e| coco_types::ModelCatalogInfo {
            provider: e.provider,
            provider_display: e.provider_display,
            model_id: e.model_id,
            display_name: e.display_name,
            context_window: e.context_window,
            supported_efforts: e.supported_efforts,
            default_effort: e.default_effort,
        })
        .collect()
}

fn build_provider_statuses(
    runtime_config: &coco_config::RuntimeConfig,
) -> std::collections::HashMap<String, coco_tui::state::ProviderStatus> {
    use coco_tui::state::ProviderStatus;
    use coco_tui::state::ProviderUnavailableReason;

    let resolver = coco_cli::provider_login::shared_resolver();
    runtime_config
        .providers
        .iter()
        .map(|(provider, cfg)| {
            let mut unavailable_reasons = Vec::new();
            if cfg.base_url.trim().is_empty() {
                unavailable_reasons.push(ProviderUnavailableReason::MissingBaseUrl);
            }
            // Branch on auth mode so a logged-in OAuth provider isn't mislabeled
            // "missing API key" (env_key is empty for OAuth instances). Reuses
            // the same credential-presence decision as the client-build gate.
            match cfg.auth {
                coco_config::ProviderAuth::OAuth { .. } => {
                    if !coco_inference::model_factory::provider_credential_present(
                        cfg,
                        Some(&resolver),
                    ) {
                        unavailable_reasons.push(ProviderUnavailableReason::NotLoggedIn {
                            provider: cfg.name.clone(),
                        });
                    }
                }
                coco_config::ProviderAuth::ApiKey => {
                    let has_api_key = cfg
                        .resolve_api_key()
                        .is_some_and(|key| !key.trim().is_empty())
                        || cfg.client_options.auth_token.is_some();
                    if !has_api_key {
                        unavailable_reasons.push(ProviderUnavailableReason::MissingApiKey {
                            env_key: cfg.env_key.clone(),
                        });
                    }
                }
            }
            (
                provider.clone(),
                ProviderStatus {
                    provider_display: provider_display_label(provider),
                    unavailable_reasons,
                },
            )
        })
        .collect()
}

/// Build the `/login` picker rows: every OAuth-capable provider instance with
/// its logged-in state. API-key providers are excluded (they authenticate via
/// env var / `providers.json`, not `/login`). Kept CLI-side — like
/// `build_provider_statuses` — since only the CLI can reach `RuntimeConfig`.
fn build_login_entries(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::LoginEntryInfo> {
    let resolver = coco_cli::provider_login::shared_resolver();
    let mut entries: Vec<coco_types::LoginEntryInfo> = runtime_config
        .providers
        .iter()
        .filter_map(|(name, cfg)| match cfg.auth {
            coco_config::ProviderAuth::OAuth { .. } => Some(coco_types::LoginEntryInfo {
                provider: name.clone(),
                provider_display: provider_display_label(name),
                auth_label: "OAuth".to_string(),
                logged_in: coco_inference::model_factory::provider_credential_present(
                    cfg,
                    Some(&resolver),
                ),
            }),
            coco_config::ProviderAuth::ApiKey => None,
        })
        .collect();
    entries.sort_by(|a, b| a.provider_display.cmp(&b.provider_display));
    entries
}

/// Build the initial `model_by_role` map from
/// `RuntimeConfig.model_roles`. Each role gets a `ModelBinding` with
/// `effort: None` (the engine's resolver picks the model's default
/// thinking level when no explicit effort is set).
fn build_model_by_role(
    runtime_config: &coco_config::RuntimeConfig,
) -> std::collections::HashMap<coco_types::ModelRole, coco_tui::state::ModelBinding> {
    use coco_tui::state::ModelBinding;
    use coco_types::ModelRole;
    const ROLES: [ModelRole; 8] = [
        ModelRole::Main,
        ModelRole::Fast,
        ModelRole::Plan,
        ModelRole::Explore,
        ModelRole::Review,
        ModelRole::HookAgent,
        ModelRole::Memory,
        ModelRole::Subagent,
    ];
    let mut out = std::collections::HashMap::new();
    for role in ROLES {
        if let Some(spec) = runtime_config.model_roles.get(role) {
            let display = runtime_config.model_roles.moa_endpoint(role);
            let provider = display
                .map(|endpoint| endpoint.display_provider().to_string())
                .unwrap_or_else(|| spec.provider.clone());
            let model_id = display
                .map(|endpoint| endpoint.display_model_id().to_string())
                .unwrap_or_else(|| spec.model_id.clone());
            let context_window = runtime_config
                .model_registry
                .resolve(&spec.provider, &spec.model_id)
                .map(|resolved| resolved.info.context_window.get() as i64);
            out.insert(
                role,
                ModelBinding {
                    model_id,
                    provider,
                    context_window,
                    effort: None,
                },
            );
        }
    }
    out
}

/// Provider id → human display label. Falls back to the raw id for
/// providers without an explicit label (e.g. user-named custom
/// providers, or `deepseek-openai` / `deepseek-anthropic` which keep
/// their qualified id so the picker can distinguish them).
fn provider_display_label(provider: &str) -> String {
    match provider {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "google" => "Google",
        "deepseek" => "DeepSeek",
        "bytedance" => "ByteDance",
        other => return other.to_string(),
    }
    .to_string()
}

/// Apply a `(role, provider, model_id, effort)` selection through the local
/// AppServer handler, which updates the live runtime in memory and emits
/// [`ServerNotification::ModelRoleChanged`] so the TUI refreshes its
/// `model_by_role` mirror (and, when `role == Main`, the status-bar
/// fields).
/// **No file write.** Users who want the binding to survive across
/// sessions edit `the global config file::model_roles.<role>.primary` themselves.
/// The picker is for fast experimentation, not persistence.
/// Non-Main roles take effect on the next turn that drives that role.
/// Main effort takes effect immediately; Main model_id changes only
/// take effect on next session restart — see
/// [`SessionRuntime::client_for_role`] doc-comment.
async fn apply_role_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    role: coco_types::ModelRole,
    provider: String,
    model_id: String,
    effort: Option<coco_types::ReasoningEffort>,
    event_tx: &tokio::sync::mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_cli::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    let result = match local_app_server_bridge
        .client()
        .set_model_role(
            local_app_server_bridge.handler(),
            coco_types::SetModelRoleParams {
                role,
                provider: provider.clone(),
                model_id: model_id.clone(),
                effort,
            },
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(
                role = %role.as_str(),
                provider = %provider,
                model_id = %model_id,
                error = %error,
                "control/setModelRole failed; reverting picker mirror"
            );
            let _ = event_tx
                .send(CoreEvent::Protocol(ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: format!(
                            "failed to apply {role_label} -> {provider}/{model_id}: {error}",
                            role_label = role.as_str(),
                        ),
                        category: Some("model_role_apply_failed".to_string()),
                        retryable: true,
                    },
                )))
                .await;
            return;
        }
    };
    let coco_types::SetModelRoleResult {
        changed,
        display_name,
    } = result;
    tracing::info!(
        role = %changed.role.as_str(),
        provider = %changed.provider,
        model_id = %changed.model_id,
        effort = ?changed.effort,
        "applied in-memory model-role override through local AppServer (not persisted)"
    );

    // Tool-style confirmation for the `/model` picker (no-args → modal →
    // Enter). Rendered `❯ /model` + `⎿ Set …` like every slash result, but
    // `System` (transcript-only): model/role selection is a tool-config
    // action — the LLM must NOT see it in its context. Engine-side push so
    // it fires ONLY for the picker; the Ctrl+T effort cycle reuses
    // `ModelRoleChanged` but stays silent (status-bar only).
    let role_label = title_case_role(changed.role);
    let effort_suffix = changed
        .effort
        .map(|e| format!(" · thinking: {e}"))
        .unwrap_or_default();
    let display_label = if changed.provider == "moa" {
        format!("{}/{}", changed.provider, changed.model_id)
    } else {
        format!("{}/{}", changed.provider, display_name)
    };
    let output = format!("Set {role_label} → {display_label}{effort_suffix}");
    let messages = coco_messages::build_slash_command_messages(
        "model", /*args*/ "", &output, /*is_sensitive*/ false,
    );
    let mut h = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    for msg in messages {
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }
    let is_remote =
        coco_config::EnvSnapshot::from_current_process().is_truthy(coco_config::EnvKey::CocoRemote);
    if let Some(msg) = build_remote_model_change_reminder(changed.role, &display_name, is_remote) {
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }
}

/// Title-case a `ModelRole` for display (`main` → `Main`).
fn title_case_role(role: coco_types::ModelRole) -> String {
    let mut chars = role.as_str().chars();
    chars.next().map_or_else(String::new, |first| {
        format!("{}{}", first.to_uppercase(), chars.as_str())
    })
}

fn build_remote_model_change_reminder(
    role: coco_types::ModelRole,
    display_name: &str,
    is_remote: bool,
) -> Option<coco_messages::Message> {
    if !is_remote || role != coco_types::ModelRole::Main {
        return None;
    }
    Some(coco_messages::wrapping::create_system_reminder_message(
        &format!(
            "The model for this session has been changed to {display_name}. You are now running as {display_name}."
        ),
    ))
}

#[cfg(test)]
#[path = "tui_runner.test.rs"]
mod tests;
