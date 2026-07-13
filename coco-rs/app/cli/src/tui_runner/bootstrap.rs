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
    cli: coco_agent_host::AgentHostOptions,
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
    let runtime_config = coco_agent_host::headless::build_runtime_config_for_cli(&cli, &cwd)?;
    coco_agent_host::model_card_refresh::spawn_if_enabled(&runtime_config);
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
    let resources = build_engine_resources(&process_runtime, &cli, &runtime_config, &cwd)?;
    coco_cli::startup_profile::mark("engine_resources_built");
    let model_id = resources.model_id.clone();
    let permission_mode = resources.startup.mode;
    let bypass_permissions_available = resources.startup.bypass_available;
    let auto_mode_available = resources.startup.auto_available;
    let plan_mode_available = runtime_config
        .features
        .enabled(coco_types::Feature::PlanMode);
    let startup_notification = resources.startup.notification.clone();
    // The per-session fold builds each session's own tool registry now;
    // the startup `resources.tools` is no longer threaded into the factory.
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
            coco_session::TranscriptStore::new(coco_agent_host::paths::project_paths(&cwd));
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
    let pending_approvals = coco_agent_host::tui_permission_bridge::new_pending_map();
    // Keep a concrete `Arc<TuiPermissionBridge>` alongside the trait
    // object so we can install the SessionRuntime weak-ref after
    // `SessionRuntime::build` returns (used to fire the Notification
    // hook on permission prompts).
    let tui_permission_bridge_concrete = Arc::new(
        coco_agent_host::tui_permission_bridge::TuiPermissionBridge::new(
            notification_tx.clone(),
            pending_approvals.clone(),
        ),
    );
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
    let mut local_app_server_bridge =
        coco_agent_host::app_server_host::AppServerLocalBridge::with_server_config(
            Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
            &runtime_config.server,
        );
    let runtime_factory_cli = Arc::new(cli);
    let runtime_factory = crate::session_runtime::SessionRuntimeFactory::from_host_config(
        crate::session_runtime::SessionRuntimeFactoryHostConfig {
            cli: Arc::clone(&runtime_factory_cli),
            cwd: cwd.clone(),
            model_runtimes: None,
            session_manager,
            fast_model_spec,
            permission_bridge: Some(session_permission_bridge),
            process_runtime: process_runtime.clone(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // Interactive TUI: file-history checkpointing defaults ON.
            is_non_interactive: false,
        },
    );
    let runtime_binding = local_app_server_bridge
        .load_session_runtime_binding_from_factory(
            initial_session_id.clone(),
            runtime_factory.clone(),
            initial_runtime_cwd.clone(),
        )
        .await?;
    let session_handle = runtime_binding.session;
    let event_hub_connector = runtime_binding.event_hub_connector;
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

    coco_agent_host::session_bootstrap::install_session_integrations(
        session_handle.clone(),
        &initial_runtime_cwd,
        process_runtime.clone(),
        coco_agent_host::session_bootstrap::SessionIntegrationOptions {
            event_sink: Some(notification_tx.clone()),
            leader_permission_bridge: Some(tui_permission_bridge.clone()),
            ..Default::default()
        },
    )
    .await?;
    coco_cli::startup_profile::mark("session_late_binds");

    local_app_server_bridge
        .bind_interactive_session(session_handle.clone(), Some(notification_tx.clone()))
        .await?;
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
    let _plugin_watcher_guard = coco_agent_host::plugin_watch::spawn(
        notification_tx.clone(),
        &cwd,
        &coco_config::global_config::config_home(),
    );

    // Skill change detector. Reloads the skill catalog (reminder +
    // SkillTool) and rebuilds the slash-command registry on `.md` edits
    // so authoring skills doesn't require a session restart. Held by
    // `_skill_watcher_guard` until TUI shutdown (drop = clean stop),
    // exactly like the plugin watcher above.
    let _skill_watcher_guard = coco_agent_host::skill_watch::spawn_current_session(
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
    coco_agent_host::team_memory_sync::spawn_for_session(
        runtime,
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
    let pump_cancel = CancellationToken::new();
    let teammate_turn_done_tx = coco_agent_host::teammate_inbox_pump::spawn_for_current_teammate(
        runtime,
        command_tx.clone(),
        pump_cancel.clone(),
    )
    .await;

    // Official marketplace auto-install. Fire-and-forget on the interactive
    // path only: retry-gated + backoff, opt-out via
    // `COCO_PLUGINS_DISABLE_OFFICIAL_MARKETPLACE`, and non-fatal. Never
    // blocks startup — the official marketplace is fetched once in the
    // background and reused on subsequent launches.
    coco_agent_host::session_bootstrap::spawn_marketplace_startup(
        coco_config::global_config::config_home(),
    );

    // Honor `--resume` / `--continue` / `--fork-session`. The binary
    // entry has already loaded the source transcript; route the switch through
    // the local AppServer so startup resume and in-session `/resume` share the
    // same lifecycle registration, surface replacement, and runtime handler
    // boundary.
    let startup_session_start_source = if resume_plan.is_some() {
        coco_hooks::orchestration::SessionStartSource::Resume
    } else {
        coco_hooks::orchestration::SessionStartSource::Startup
    };
    if let Some(plan) = resume_plan {
        tracing::info!(
            target: "coco_agent_host::resume",
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
        if let Some(warning) =
            runtime.reconcile_session_mode_on_resume(plan.conversation.mode.as_deref())
        {
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
    let _cron_tick_guard =
        coco_agent_host::cron_tick::spawn_current_session(Arc::clone(&current_session));

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
        runtime.set_agent_progress_summaries_enabled(true).await;
    }

    // Create TUI app
    let mut app = App::new(command_tx, notification_rx, cwd.clone())
        .map_err(|e| anyhow::anyhow!("Failed to create TUI: {e}"))?;
    app.state_mut()
        .ui
        .apply_display_settings(coco_tui::DisplaySettings::from_runtime_config(
            runtime.runtime_config(),
        ));
    let initial_ui_flags =
        coco_agent_host::session_dialogs::build_initial_session_ui_flags_payload(runtime);
    app.state_mut().ui.coordinator_mode_active = initial_ui_flags.coordinator_mode_active;

    // Hydrate the composer's up-arrow history from the persistent
    // cross-session store (`<config_home>/history.jsonl`), project-scoped
    // to this cwd and newest-first. Text-only recall — paste pills are
    // re-snapshotted per session, matching codex cross-session behaviour.
    {
        let project = cwd.to_string_lossy().to_string();
        let texts = runtime.prompt_history_texts(project).await;
        app.state_mut().ui.input.hydrate_history(texts);
    }
    app = app.with_display_settings_reload(display_settings_rx);
    app = app.with_config_reload_errors(config_reload_errors_rx);
    // Voice input (Feature::Voice): build the STT engine + capture + session and
    // install it. No-op when voice is disabled; best-effort on failure.
    app = coco_agent_host::voice_bootstrap::install_for_session(app, runtime);

    // Wire file_history_enabled into TUI session state so the rewind
    // modal knows whether to show code restore options.
    app.state_mut().session.file_history_enabled = initial_ui_flags.file_history_enabled;

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
    let initial_model_status =
        coco_agent_host::session_dialogs::build_initial_model_status_payload(runtime, &model_id);
    app.state_mut().session.model = initial_model_status.model_id;
    app.state_mut().session.provider = initial_model_status.provider;
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
    if let Some(default_effort) = initial_model_status.default_effort {
        app.state_mut().session.thinking_effort = default_effort;
    }

    // Seed `model_catalog` and `model_by_role` from the resolved
    // `ModelRegistry`. The TUI picker and Ctrl+T cycle both consult
    // these — using the registry view (rather than the L0-only
    // `builtin_models_partial`) means L1 `config home/models.json` entries
    // and L2 `providers.<n>.models.<id>` overrides are visible.
    {
        let mut catalog = model_catalog_from_infos(
            coco_agent_host::session_dialogs::build_model_catalog_payload(runtime),
        );
        let provider_statuses = provider_statuses_from_infos(
            coco_agent_host::session_dialogs::build_provider_status_payload(runtime),
        );
        let by_role = build_model_by_role_from_payload(
            coco_agent_host::session_dialogs::build_model_role_bindings_payload(runtime),
        );
        let state = app.state_mut();
        state.session.model_catalog = std::mem::take(&mut catalog);
        state.session.provider_statuses = provider_statuses;
        state.session.model_by_role = by_role;
        state.session.available_models =
            coco_agent_host::session_dialogs::available_models_payload(runtime);
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
        let agents: Vec<coco_tui::autocomplete::AgentInfo> =
            coco_agent_host::session_dialogs::build_active_agent_definitions_payload(runtime)
                .await
                .iter()
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
    let flag_settings_path = runtime_factory_cli
        .settings
        .as_deref()
        .map(std::path::PathBuf::from);
    let app_server_shutdown_timeout =
        Duration::from_secs(runtime_config.server.shutdown_timeout_secs as u64);
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
        app_server_shutdown_timeout,
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
    let runtime_session_id = runtime_for_resume_hint.session_id().to_string();
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
    coco_agent_host::resume_hint::print_resume_hint(&cwd, session_id.as_deref());

    // On leader exit: kill any orphaned tmux teammate panes, then remove the
    // team dirs (+ worktrees + tasks via cleanup_team_directories) for teams
    // this session led, so neither the child processes nor the dirs orphan.
    if let Some(sid) = session_id.as_deref()
        && let Err(e) = coco_coordinator::team_file::cleanup_session_teams(sid)
    {
        tracing::warn!(error = %e, "team cleanup on exit failed");
    }

    // Wait for agent driver to finish its own teardown.
    let app_server_shutdown = match driver_handle.await {
        Ok(outcome) => outcome,
        Err(error) => coco_agent_host::shutdown::ShutdownDrainOutcome::Failed {
            message: error.to_string(),
        },
    };
    let event_hub_shutdown = if let Some(connector) = event_hub_connector {
        connector
            .shutdown_and_flush_with_timeout(app_server_shutdown_timeout)
            .await
    } else {
        coco_agent_host::shutdown::ShutdownDrainOutcome::Clean
    };

    tui_result.map_err(|e| anyhow::anyhow!("TUI error: {e}"))?;
    if !app_server_shutdown.is_clean() {
        anyhow::bail!("local AppServer shutdown drain {app_server_shutdown}");
    }
    if !event_hub_shutdown.is_clean() {
        anyhow::bail!("local Event Hub shutdown flush {event_hub_shutdown}");
    }
    Ok(())
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

pub(super) struct TuiRuntimeReloadSubscriptions {
    current_session: SharedSessionHandle,
    notification_tx: mpsc::Sender<CoreEvent>,
    pending_approvals: coco_agent_host::tui_permission_bridge::PendingApprovals,
    display_settings_tx: mpsc::Sender<coco_tui::DisplaySettings>,
    config_reload_error_tx: mpsc::Sender<String>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl TuiRuntimeReloadSubscriptions {
    pub(super) fn new(
        current_session: SharedSessionHandle,
        notification_tx: mpsc::Sender<CoreEvent>,
        pending_approvals: coco_agent_host::tui_permission_bridge::PendingApprovals,
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

    pub(super) async fn install_for_session(
        &mut self,
        session: &crate::session_runtime::SessionHandle,
    ) {
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
            self.handles.push(spawn_model_runtime_reload(
                runtime.model_runtimes(),
                &publisher,
            ));
        }

        if let Some(state) = runtime.sandbox_state() {
            let approval_bridge: coco_sandbox::SandboxApprovalBridgeRef = std::sync::Arc::new(
                coco_agent_host::sandbox_approval_bridge_tui::TuiSandboxApprovalBridge::new(
                    self.notification_tx.clone(),
                    self.pending_approvals.clone(),
                ),
            );
            runtime.set_sandbox_approval_bridge(approval_bridge);
            runtime.install_sandbox_reload_supervisor().await;
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
use super::*;
