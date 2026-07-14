use super::*;

// ─── run_chat ────────────────────────────────────────────────────────

/// Outcome of a single headless `coco -p` invocation.
/// Mirrors the data the binary's `main()` would have printed, but
/// returns it structured so tests / embeddings can assert on individual
/// fields.
#[derive(Debug)]
pub struct RunChatOutcome {
    /// Final assistant response text (what the binary prints to stdout).
    pub response_text: String,
    /// Number of agent loop turns executed.
    pub turns: i32,
    /// Total token usage accumulated across the session.
    pub total_usage: TokenUsage,
    /// Per-model cost / token tracking.
    pub cost_tracker: CostTracker,
    /// Resolved model id (provider-side wire name).
    pub model_id: String,
    /// `Some (api)` when a real provider was wired; `None` for mock fallback.
    pub provider_api: Option<coco_types::ProviderApi>,
    /// Resolved permission mode after CLI + settings + killswitch merge.
    pub permission_mode: coco_types::PermissionMode,
    /// `true` when the session is allowed to transition to `BypassPermissions`.
    pub bypass_permissions_available: bool,
    /// Optional notification surfaced when permission resolution downgraded
    /// (e.g. killswitch forced Bypass → AcceptEdits). Caller should print
    /// to stderr.
    pub permission_notification: Option<String>,
    /// Total wall-clock duration in milliseconds.
    pub duration_ms: i64,
    /// API time in milliseconds.
    pub duration_api_ms: i64,
    /// Whether the run hit the budget limit.
    pub budget_exhausted: bool,
    /// Whether the run was cancelled.
    pub cancelled: bool,
    /// Last continue reason from the engine loop.
    pub last_continue_reason: Option<ContinueReason>,
    /// Number of fallback runtime slots installed on the engine.
    /// (from `--fallback-model` flags + `models.<role>.fallbacks`).
    pub installed_fallback_count: usize,
    /// Final message history at session end, including the user prompt,
    /// any tool calls + results, and the final assistant reply. Tests
    /// or embedding callers can feed this into the next [`run_chat_with_options`]
    /// call (`opts.prior_messages = previous.final_messages`) to
    /// continue the conversation through typed `session/start.initial_messages`.
    pub final_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Working directory the engine actually used. Reflects the
    /// effective resolution: `--cwd <flag>` then `RunChatOptions::cwd`.
    pub effective_cwd: PathBuf,
    /// Additional directories declared via `--add-dir` (resolved to
    /// absolute paths). Threaded onto every tool's permission context
    /// so file-system tools may read from them. Empty = no extras.
    pub additional_dirs: Vec<PathBuf>,
    /// Tool filter built from `--allowed-tools` / `--disallowed-tools`.
    /// `None` ⇒ both flags were empty (engine uses `unrestricted()`).
    pub tool_filter_summary: Option<ToolFilterSummary>,
    /// Result of the local AppServer shutdown drain after the print-mode turn.
    pub app_server_shutdown: ShutdownDrainOutcome,
    /// Result of the Event Hub connector shutdown flush after the print-mode turn.
    pub event_hub_shutdown: ShutdownDrainOutcome,
}

/// Lightweight surface of [`coco_types::ToolFilter`] for tests — the
/// underlying type uses `HashSet<ToolId>` whose iteration is
/// non-deterministic, so we project to sorted vectors.
#[derive(Debug, Clone, Default)]
pub struct ToolFilterSummary {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
}

/// Options for [`run_chat_with_options`].
#[derive(Default)]
pub struct RunChatOptions {
    /// Working directory for this run. Required unless the CLI carries
    /// `--cwd`; pass an explicit path to keep parallel tests / embeddings
    /// isolated.
    pub cwd: Option<PathBuf>,
    /// Cancellation token threaded into the engine. When the token is
    /// cancelled mid-run, the engine returns a `cancelled = true`
    /// outcome. `None` = a fresh token is created internally.
    pub cancel: Option<CancellationToken>,
    /// Pre-built message history to seed the conversation. Empty =
    /// start a fresh conversation.
    /// Non-empty fresh runs enter through `session/start.initial_messages`;
    /// production resume enters through `session/resume`.
    pub prior_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Override the engine's session id. Used by `--resume` /
    /// `--continue` / `--fork-session` so the resumed run writes
    /// transcript entries under the source (or fork) session id
    /// instead of a freshly generated `SessionId`. `None` lets
    /// print mode use `--session-id` or mint a fresh id.
    pub session_id_override: Option<coco_types::SessionId>,
    /// Production CLI resume target. When present, startup enters through the
    /// local AppServer `session/resume` lifecycle instead of constructing and
    /// hydrating a runtime directly.
    pub resume_target: Option<coco_types::SessionTarget>,
    /// Stored coordinator/normal mode of the resumed session, used to
    /// reconcile coordinator mode. `None` = no
    /// resume / no stored mode.
    pub stored_mode: Option<String>,
    /// Process-scoped owner for shared runtime managers. Production callers pass
    /// the startup-owned instance; tests/embedders may omit it and get a
    /// call-scoped compatibility runtime.
    pub process_runtime: Option<Arc<ProcessRuntime>>,
}

/// Drive one headless agent run with explicit options.
/// Equivalent to `coco -p "<prompt>"` with the same flag plumbing the
/// binary uses, plus three test-friendly knobs:
/// - `opts.cwd` — explicit cwd used when `--cwd` is not set.
/// - `opts.cancel` — thread an external [`CancellationToken`] for
/// mid-run cancellation.
/// - `opts.prior_messages` — seed fresh process-local runs through
/// `session/start.initial_messages`; production resume uses
/// `session/resume`.
/// Honors these `AgentHostOptions` flags end-to-end:
/// `--models.main`, `--fallback-model`, `--permission-mode`,
/// `--dangerously-skip-permissions` / `--allow-…`, `--max-turns`,
/// `--max-tokens`, `--settings`, `--system-prompt`,
/// `--append-system-prompt`, `--append-system-prompt-file`,
/// `--cwd`, `--add-dir`, `--allowed-tools`, `--disallowed-tools`.
pub async fn run_chat_with_options(
    cli: &AgentHostOptions,
    prompt: Option<&str>,
    opts: RunChatOptions,
) -> Result<RunChatOutcome> {
    let prompt = prompt.unwrap_or("Hello!");
    // Cwd precedence: explicit user `--cwd` flag > `RunChatOptions::cwd`
    // (startup/test/embedder injection).
    let cwd: PathBuf = if let Some(flag) = cli.cwd.as_deref() {
        std::path::Path::new(flag).to_path_buf()
    } else if let Some(p) = opts.cwd {
        p
    } else {
        anyhow::bail!("run_chat_with_options requires RunChatOptions::cwd when --cwd is not set")
    };
    let process_runtime = opts.process_runtime.unwrap_or_else(ProcessRuntime::global);
    // Resolve the session id before any local no-model-turn exits. A
    // print-mode local command should still leave a resumable transcript, and
    // `--session-id` is the automation-facing way to address that session.
    let session_id = if let Some(session_id) = opts.session_id_override.clone() {
        session_id
    } else if let Some(session_id_string) = cli.session_id.clone() {
        coco_types::SessionId::try_new(session_id_string.clone())
            .map_err(|e| anyhow::anyhow!("invalid session id '{session_id_string}': {e}"))?
    } else {
        coco_types::SessionId::generate()
    };
    if let Some(goal_args) = parse_headless_goal_slash(prompt) {
        match coco_commands::parse_goal_command_args(goal_args) {
            Err(text) => {
                return Ok(headless_local_goal_text_outcome(
                    cli,
                    &cwd,
                    &session_id,
                    goal_args,
                    text,
                    opts.prior_messages,
                )
                .await);
            }
            Ok(coco_commands::GoalCommandRequest::Status) => {
                let text =
                    crate::goal_command::format_latest_goal_history_status(&opts.prior_messages)
                        .unwrap_or_else(|| "No goal set. Usage: `/goal <condition>`".to_string());
                return Ok(headless_local_goal_text_outcome(
                    cli,
                    &cwd,
                    &session_id,
                    "",
                    text,
                    opts.prior_messages,
                )
                .await);
            }
            Ok(coco_commands::GoalCommandRequest::Clear) => {
                if crate::goal_command::find_restorable_goal_condition(&opts.prior_messages)
                    .is_none()
                {
                    return Ok(headless_local_goal_text_outcome(
                        cli,
                        &cwd,
                        &session_id,
                        "clear",
                        "No goal set".to_string(),
                        opts.prior_messages,
                    )
                    .await);
                }
            }
            Ok(coco_commands::GoalCommandRequest::Set { .. }) => {}
        }
    }
    tracing::info!(
        target: "coco_agent_host::headless",
        cwd = %cwd.display(),
        prompt_len = prompt.len(),
        has_prior_messages = !opts.prior_messages.is_empty(),
        "headless run starting"
    );

    let runtime_config = build_runtime_config_for_cli(cli, &cwd)?;
    crate::model_card_refresh::spawn_if_enabled(&runtime_config);
    // Reconcile coordinator mode to a resumed session. Flips the env flag
    // before the engine assembles its system prompt below.
    if let Some(warning) = crate::coordinator_mode_resume::reconcile_on_resume(
        opts.stored_mode.as_deref(),
        &runtime_config.features,
    ) {
        eprintln!("{warning}");
    }
    let settings = &runtime_config.settings;

    // Startup marketplace maintenance (seed/reconcile/delist) on the headless
    // surface too; background + non-fatal, mirroring the TUI.
    crate::session_bootstrap::spawn_marketplace_startup(coco_config::global_config::config_home());

    let main_model = resolve_main_model(&runtime_config);
    let provider_api = main_model.provider_api;
    let model_id = main_model.model_id.clone();
    let installed_fallback_count = runtime_config
        .model_roles
        .fallbacks(coco_types::ModelRole::Main)
        .len();
    let fallback_policy = runtime_config
        .model_roles
        .policy(coco_types::ModelRole::Main);
    tracing::info!(
        target: "coco_agent_host::headless",
        provider = main_model.provider,
        model_id = %model_id,
        real_provider = provider_api.is_some(),
        fallback_count = installed_fallback_count,
        fallback_policy_set = fallback_policy.is_some(),
        "model client resolved"
    );

    let registry = ToolRegistry::new();
    coco_tools::register_all_tools(&registry);

    // The registry is built only for the startup tool-count metric; the
    // per-session fold builds each session's own registry now.
    let tool_count = registry.len();
    let cancel = opts.cancel.unwrap_or_default();

    let startup = resolve_startup_permission_state(cli, &settings.merged)?;
    let permission_mode = startup.mode;
    let bypass_permissions_available = startup.bypass_available;
    tracing::info!(
        target: "coco_agent_host::headless",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        permission_notification = startup.notification.is_some(),
        tool_count,
        sandbox_mode = ?runtime_config.sandbox.mode,
        "permissions + tools ready"
    );

    // Build the one canonical SessionRuntime — same shape as TUI/AppServer — so the
    // leader engine and every subagent share ONE config, ONE session id, and
    // ONE `wire_engine` install list (agent + task handles, memory_runtime,
    // file_read_state, transcript/usage). Print mode forks subagents from a
    // single context, not a second session container.
    let config_home = coco_config::global_config::config_home();
    let session_manager = Arc::new(coco_session::SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        config_home.clone(),
    ));
    // Shared local-host assembly (factory + local bridge + Event Hub egress),
    // identical to the TUI path — see `crate::local_host`. Headless-specific
    // policy: no model-runtime prebuild (the fold builds one with the session's
    // header vars), no permission bridge, non-interactive, MCP awaited, LSP off,
    // late-bind failures downgraded to warnings, and no plugin watcher (a
    // one-shot print run exits before any hot-reload could fire).
    let crate::local_host::PreparedLocalHost {
        bridge: mut local_app_server_bridge,
        runtime_factory: _,
        event_hub_connector,
        event_hub_membership_watcher,
        plugin_watcher_guard: _plugin_watcher_guard,
    } = crate::local_host::build_local_host(
        crate::local_host::LocalHostInputs {
            cli: Arc::new(cli.clone()),
            cwd: cwd.clone(),
            session_manager: Arc::clone(&session_manager),
            process_runtime: process_runtime.clone(),
            model_runtimes: None,
            fast_model_spec: None,
            permission_bridge: None,
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // Headless / print: file-history checkpointing defaults OFF.
            is_non_interactive: true,
            integration_options: crate::session_bootstrap::SessionIntegrationOptions {
                lsp: crate::session_bootstrap::SessionLspIntegration::Disabled,
                mcp_connect: crate::session_bootstrap::SessionMcpConnectMode::Await,
                late_binds_failure:
                    crate::session_bootstrap::SessionLateBindFailure::WarnAndContinue,
                ..Default::default()
            },
            bypass_permissions_available,
            requires_structured_output: cli.json_schema.is_some(),
            plugin_watch: crate::local_host::LocalPluginWatch::Disabled,
        },
        &runtime_config,
    );
    let startup_binding = if let Some(target) = opts.resume_target.clone() {
        local_app_server_bridge
            .resume_interactive_session(
                coco_types::SessionResumeParams {
                    target,
                    plan_mode_instructions: None,
                },
                None,
            )
            .await
            .map_err(|err| anyhow::anyhow!("headless session/resume failed: {err}"))?
    } else {
        local_app_server_bridge
            .start_interactive_session(
                coco_types::SessionStartParams {
                    session_id: Some(session_id.clone()),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    model: Some(model_id.clone()),
                    permission_mode: Some(permission_mode),
                    initial_messages: opts
                        .prior_messages
                        .iter()
                        .map(|message| (**message).clone())
                        .collect(),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(|err| anyhow::anyhow!("headless session/start failed: {err}"))?
    };
    let session_handle = startup_binding.session;
    let session = session_handle.clone();

    // Sandbox hot-reload: re-flow settings.json `sandbox.*` edits into the live
    // SandboxState on the headless/print path through the session-owned config
    // publisher.
    session.install_sandbox_reload_supervisor().await;

    // `StructuredOutput` tool + inline enforcement. Registers into the
    // session's own fold registry, not a discarded startup one.
    session
        .install_structured_output_tool_if_requested(cli.json_schema.as_deref())
        .await?;

    let interactive_target = local_app_server_bridge
        .interactive_session()
        .map(crate::local_client::LocalSessionClient::interactive_target)
        .ok_or_else(|| anyhow::anyhow!("interactive surface was not installed"))?;
    local_app_server_bridge
        .client()
        .keep_alive(local_app_server_bridge.handler())
        .await?;

    let session_id = session.session_id().clone();

    // Bootstrap the per-source permission rule maps; see
    // `crate::permission_rule_loader` for the conversion path. Headless runs
    // honor the same settings.json deny/allow/ask rules as the TUI.
    let (allow_rules, deny_rules, ask_rules) =
        crate::permission_rule_loader::typed_permission_rules(&runtime_config.settings);
    let permission_rule_source_roots =
        crate::permission_rule_loader::permission_rule_source_roots(&runtime_config.settings, &cwd);

    let turn_thinking_level = session.thinking_level().await;
    let permission_mode_availability = coco_types::PermissionModeAvailability::new(
        bypass_permissions_available,
        startup.auto_available,
    );
    // Seed --add-dir + settings additionalDirectories into the session
    // working-dir allowlist. Lives ONLY on the live base now.
    let session_additional_dirs = crate::permission_rule_loader::seed_session_additional_dirs(
        cli,
        &runtime_config.settings,
        &cwd,
    );
    // `--print`: honor `--max-turns` then `loop.max_turns`; unbounded when
    // neither is set.
    let max_turns = cli.max_turns.or(runtime_config.loop_config.max_turns);
    let total_token_budget = cli
        .max_tokens
        .or_else(|| runtime_config.loop_config.total_token_budget.map(i64::from));

    tracing::info!(
        target: "coco_agent_host::headless",
        max_turns = ?max_turns,
        total_token_budget = ?total_token_budget,
        "engine config built"
    );

    // Seed the live permission base from the headless-loaded rule maps (the
    // runtime's bootstrap seed used the un-overridden base). The engine built
    // below shares this `app_state` (app_state_override = None). The rules +
    // dirs live ONLY on the live base now — the config no longer carries them.
    session
        .set_live_permissions(crate::session_runtime::live_permissions(
            permission_mode,
            allow_rules,
            deny_rules,
            ask_rules,
            session_additional_dirs,
            permission_rule_source_roots.clone(),
        ))
        .await;
    session
        .apply_turn_runtime_config(crate::session_runtime::SessionTurnRuntimeConfig {
            is_non_interactive: true,
            avoid_permission_prompts: true,
            permission_mode,
            permission_mode_availability,
            permission_rule_source_roots: permission_rule_source_roots.clone(),
            max_turns,
            total_token_budget,
            cwd_override: Some(cwd.clone()),
            tool_filter: build_tool_filter(cli),
            plans_directory: settings.merged.plans_directory.clone(),
            plan_mode_custom_instructions: cli.plan_mode_instructions.clone(),
        })
        .await;

    let mut effective_prompt = prompt.to_string();
    let mut prefix_messages: Vec<std::sync::Arc<coco_messages::Message>> = Vec::new();
    let prior_messages = opts.prior_messages;

    if let Some(goal_args) = parse_headless_goal_slash(prompt) {
        match coco_commands::parse_goal_command_args(goal_args) {
            Err(text) => {
                append_headless_slash_text(&mut prefix_messages, "goal", goal_args, &text);
                persist_headless_local_transcript_messages(
                    cli,
                    &cwd,
                    &session_id,
                    &prior_messages,
                    &prefix_messages,
                )
                .await;
                let mut final_messages = prior_messages;
                final_messages.extend(prefix_messages);
                return Ok(headless_text_outcome(
                    cli,
                    &cwd,
                    text,
                    final_messages,
                    model_id,
                    provider_api,
                    permission_mode,
                    bypass_permissions_available,
                    startup.notification,
                    installed_fallback_count,
                ));
            }
            Ok(request) => {
                let args = crate::goal_command::goal_display_args(&request).to_string();
                // Headless is non-interactive; the trust gate is deliberately skipped.
                let outcome = crate::goal_command::resolve_goal_request_for_session_with_history(
                    &session,
                    request,
                    &prior_messages,
                    false,
                )
                .await;

                match outcome {
                    crate::goal_command::GoalOutcome::Text(text) => {
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        persist_headless_local_transcript_messages(
                            cli,
                            &cwd,
                            &session_id,
                            &prior_messages,
                            &prefix_messages,
                        )
                        .await;
                        let mut final_messages = prior_messages;
                        final_messages.extend(prefix_messages);
                        return Ok(headless_text_outcome(
                            cli,
                            &cwd,
                            text,
                            final_messages,
                            model_id,
                            provider_api,
                            permission_mode,
                            bypass_permissions_available,
                            startup.notification,
                            installed_fallback_count,
                        ));
                    }
                    crate::goal_command::GoalOutcome::StatusThenText { status, text } => {
                        append_headless_goal_status(&mut prefix_messages, status);
                        crate::goal_command::persist_active_goal_snapshot(&session).await;
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        persist_headless_local_transcript_messages(
                            cli,
                            &cwd,
                            &session_id,
                            &prior_messages,
                            &prefix_messages,
                        )
                        .await;
                        let mut final_messages = prior_messages;
                        final_messages.extend(prefix_messages);
                        return Ok(headless_text_outcome(
                            cli,
                            &cwd,
                            text,
                            final_messages,
                            model_id,
                            provider_api,
                            permission_mode,
                            bypass_permissions_available,
                            startup.notification,
                            installed_fallback_count,
                        ));
                    }
                    crate::goal_command::GoalOutcome::SetAndRun {
                        status,
                        text,
                        kickoff,
                    } => {
                        append_headless_goal_status(&mut prefix_messages, status);
                        crate::goal_command::persist_active_goal_snapshot(&session).await;
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        effective_prompt = kickoff;
                    }
                }
            }
        }
    }

    if !prefix_messages.is_empty() {
        session
            .replace_history_with_arc_messages(
                prior_messages
                    .iter()
                    .chain(prefix_messages.iter())
                    .cloned()
                    .collect(),
            )
            .await;
    }

    // Interrupt the print-mode turn on caller cancellation OR an OS signal
    // (SIGINT/SIGTERM). Without the signal arm, `kill <pid>` during a print
    // turn hits the default terminate action instead of a graceful interrupt
    //.
    let cancel_monitor = {
        let cancel = cancel.clone();
        let client = local_app_server_bridge.connect_local_client();
        let handler = local_app_server_bridge.handler().clone();
        let target = interactive_target.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = crate::shutdown::os_interrupt_signal() => {}
            }
            let _ = client.turn_interrupt(&handler, target).await;
        })
    };

    let completion = local_app_server_bridge
        .start_turn_and_wait_for_end(
            session_id.clone(),
            coco_types::TurnStartParams {
                target: interactive_target,
                prompt: effective_prompt,
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: Some(permission_mode),
                thinking_level: turn_thinking_level,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    cancel_monitor.abort();

    let session_result = completion.session_result;

    // Wait for scheduled turn-end extraction/session-memory work before
    // returning so partial writes aren't dropped on process exit. Auto-dream
    // remains fire-and-forget like TS.
    crate::shutdown::drain_session_memory(&session).await;

    // Persist coordinator mode at end-of-run so a later `--resume` re-derives
    // the role.
    crate::shutdown::persist_session_resume_mode(&session).await;

    let additional_dirs = resolve_additional_dirs(cli, &cwd);
    let tool_filter_summary = summarize_tool_filter(cli);
    let usage_snapshot = session.session_usage_snapshot().await;
    let cost_tracker = CostTracker::from_snapshot(usage_snapshot);
    let final_messages = session.history_messages().await;
    let response_text = session_result.result.clone().unwrap_or_else(|| {
        final_messages
            .iter()
            .rev()
            .find_map(|message| match message.as_ref() {
                coco_messages::Message::Assistant(assistant) => match &assistant.message {
                    coco_messages::LlmMessage::Assistant { content, .. } => {
                        content.iter().find_map(|part| match part {
                            coco_messages::AssistantContent::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_default()
    });
    let budget_exhausted = matches!(
        completion.ended.outcome,
        coco_types::TurnOutcome::BudgetExhausted(_)
    );
    let cancelled = matches!(
        completion.ended.outcome,
        coco_types::TurnOutcome::Interrupted(_)
    );
    let shutdown_timeout = Duration::from_secs(runtime_config.server.shutdown_timeout_secs as u64);
    let shutdown = ShutdownCoordinator::new("headless", shutdown_timeout)
        .drain_app_server_and_event_hub(
            local_app_server_bridge.shutdown_registered_sessions(),
            event_hub_connector,
            event_hub_membership_watcher,
        )
        .await;

    Ok(RunChatOutcome {
        effective_cwd: cwd.clone(),
        additional_dirs,
        tool_filter_summary,
        app_server_shutdown: shutdown.app_server,
        event_hub_shutdown: shutdown.event_hub,
        response_text,
        turns: session_result.total_turns,
        total_usage: session_result.usage,
        cost_tracker,
        model_id,
        provider_api,
        permission_mode,
        bypass_permissions_available,
        permission_notification: startup.notification,
        duration_ms: session_result.duration_ms,
        duration_api_ms: session_result.duration_api_ms,
        budget_exhausted,
        cancelled,
        last_continue_reason: None,
        installed_fallback_count,
        final_messages,
    })
}
