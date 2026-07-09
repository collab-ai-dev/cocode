use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use coco_context::FileHistorySnapshotSink;
use coco_context::FileHistoryState;
use coco_context::FileReadState;
use coco_hooks::HookRegistry;
use coco_messages::MessageHistory;
use coco_query::QueryEngineConfig;
use coco_session::TranscriptStore;
use coco_tool_runtime::MailboxHandleRef;
use coco_types::ModelRole;
use coco_types::SessionId;
use coco_types::ToolAppState;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

use super::SessionAgentCatalogResources;
use super::SessionCatalogResources;
use super::SessionCommandResources;
use super::SessionConfigResources;
use super::SessionEngineConfigResources;
use super::SessionEngineStateResources;
use super::SessionExecutionResources;
use super::SessionHandleResources;
use super::SessionHistoryResources;
use super::SessionHookResources;
use super::SessionIntegrationResources;
use super::SessionLifecycleResources;
use super::SessionMemoryResources;
use super::SessionPermissionResources;
use super::SessionPersistenceResources;
use super::SessionProjectResources;
use super::SessionRuntime;
use super::SessionRuntimeBuildOpts;
use super::SessionSandboxResources;
use super::SessionTitleResources;
use super::SessionTurnResources;
use super::SessionWorkspaceResources;
use super::build_sandbox_state;
use super::hooks::populate_hook_registry;
use super::live_permissions;
use super::state::TranscriptFileHistorySink;
use super::state::file_checkpointing_enabled;

impl SessionRuntime {
    /// Build the full session runtime. Constructs every subsystem
    /// `clearConversation` and the per-turn engine assembly need.
    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Arc<Self>> {
        let SessionRuntimeBuildOpts {
            cli,
            runtime_config,
            config_reloader,
            cwd,
            model_id,
            system_prompt,
            permission_mode_availability,
            permission_mode,
            model_runtimes,
            tools,
            session_manager,
            fast_model_spec,
            permission_bridge,
            command_registry,
            skill_manager,
            project_services,
            process_runtime,
            agent_search_paths,
            builtin_agent_catalog,
            session_id_override,
            is_non_interactive,
        } = opts;

        let config_home = coco_config::global_config::config_home();
        let typed_session_id = session_id_override.unwrap_or_else(SessionId::generate);
        let session_id = typed_session_id.to_string();
        // Bare mode (`COCO_BARE_MODE` / `--bare`) disables auto-memory
        // for this session, matching the TS bare-mode behavior.
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
                &typed_session_id,
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
                    session_id: Some(typed_session_id.clone()),
                    cwd: cwd.display().to_string(),
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                }),
            )?),
        };
        // Session workspace anchors — one snapshot shared by every subsystem
        // that needs cwd, storage paths, or the future ProjectServices key.
        let session_workspace = crate::paths::SessionWorkspace::resolve(cwd.clone());
        let project_root = project_services.project_root().to_path_buf();
        let project_paths = session_workspace.storage_paths.clone();

        // Per-project filesystem layout — one `Arc<ProjectPaths>` shared
        // by the memory runtime, the transcript enumerator, and any
        // future subsystem that needs the same canonical slug. Built
        // once via `crate::paths::project_paths` (canonical-git-root
        // + slug).
        // Main-session transcript store, selected by `session.backend`.
        // Constructed once so the per-turn message append, usage accounting,
        // and agent-transcript persistence path share one instance.
        let transcript_store: Arc<dyn coco_session::SessionStore> =
            match runtime_config.settings.merged.session.backend {
                coco_config::SessionBackend::Disk => {
                    Arc::new(TranscriptStore::new(project_paths.clone()))
                }
                coco_config::SessionBackend::Memory => session_manager.store_for(&cwd),
            };
        let usage_accounting = coco_query::usage_accounting::UsageAccounting::new(
            typed_session_id.clone(),
            coco_types::UsageAttribution::session(coco_types::UsageSource::Main),
        )
        .with_persistence(transcript_store.clone(), persist_session);
        usage_accounting
            .load_current_session_tracker_from_store()
            .await;
        let side_query_usage_recorder =
            crate::side_query_impl::SideQueryUsageRecorder::new(usage_accounting.clone());
        let side_query: coco_tool_runtime::SideQueryHandle = Arc::new(
            crate::side_query_impl::SideQueryAdapter::new(model_runtimes.clone(), model_id.clone())
                .with_usage_recorder(side_query_usage_recorder.clone()),
        );

        // ── Auto-memory runtime ──
        // Built once per session, gated on `Feature::AutoMemory`. The
        // runtime owns the three services (extract / dream / session
        // memory) plus the recall ranker state. We hand it the
        // resolved `MemoryConfig` (already merged with env overrides),
        // the shared `Arc<ProjectPaths>` (so the SM file lives at
        // `<projectDir>/<sid>/session-memory/summary.md`), and an
        // `AgentHandle` so the forked extraction / dream subagents
        // spawn against the same swarm runtime that user-facing
        // `Agent` tool spawns use.
        // The handle starts as `NoOpAgentHandle`; the SDK / TUI
        // runner calls `MemoryRuntime::install_agent` once the real
        // `SwarmAgentHandle` is built. Recall + system-prompt
        // rendering work without an agent handle.
        let active_shell_tool =
            crate::shell_tool_selection::active_shell_tool_from_runtime(&runtime_config)?;
        let memory_runtime = if runtime_config.memory_activation.active {
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
            .with_tool_overrides(runtime_config.tool_overrides.clone())
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
            // so turn-end auto-dream can list real prior sessions.
            // Lists sessions touched since `lastAt`, drops the current
            // session. Invoked only after the time + scan throttle
            // gates pass inside `DreamService` so cost is bounded.
            let enumerator: coco_memory::SessionEnumerator = Arc::new(move || {
                let store = coco_session::TranscriptStore::new(enumerator_project_paths.clone());
                let last_ms = coco_memory::lock::consolidate_lock(&enumerator_memory_dir)
                    .last_consolidated_at()
                    .unwrap_or(0);
                match store.list_main_sessions() {
                    Ok(metas) => metas
                        .into_iter()
                        .filter(|m| m.session_id.as_str() != enumerator_session_id)
                        .filter(|m| {
                            m.modified_at
                                .parse::<i64>()
                                .map(|t| t > last_ms)
                                .unwrap_or(false)
                        })
                        .map(|m| m.session_id.into_inner())
                        .collect(),
                    Err(_) => Vec::new(),
                }
            });
            // install_* are one-shot in production (this is the only
            // call site per slot); swallow the duplicate-install Err so
            // a future double-install in tests doesn't blow up startup.
            let _ = runtime_arc.install_session_enumerator(enumerator);
            Some(runtime_arc)
        } else {
            let reason = runtime_config
                .memory_activation
                .disabled_reason
                .unwrap_or(coco_config::MemoryDisabledReason::FeatureGate)
                .as_str();
            tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_memdir_disabled",
                reason,
                "auto-memory disabled"
            );
            None
        };

        // Skill-learning review runtime — the capability-layer analogue of the
        // memory runtime. Feature-gated (default off); the real agent handle is
        // late-bound in `attach_agent_handle`, same pattern as memory.
        let skill_review_runtime = if runtime_config
            .features
            .enabled(coco_types::Feature::SkillLearning)
        {
            let rt = Arc::new(coco_skill_learn::SkillReviewRuntime::new(&config_home));
            // Pre-creates the agent skills dir (the watcher must see it
            // before it spawns) and kicks a time-gated curator pass.
            rt.bootstrap();
            info!("skill-learning review runtime initialized");
            Some(rt)
        } else {
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
        }

        // Warm the session-memory cache so the first compact short-circuit
        // doesn't have to read disk. The handle is derived from
        // `memory_runtime`; SessionRuntime does not store a duplicate.
        if let Some(runtime) = &memory_runtime
            && !bare_mode
        {
            runtime.session_memory.load_from_disk().await;
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
        // etc. hooks. The project plugin set is read from the same
        // `ProjectServices` snapshot used by command/skill bootstrap.
        let hook_registry = {
            let registry = HookRegistry::new();
            populate_hook_registry(&registry, &runtime_config, &project_services);
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
        // Frozen anchor + live tracker. The live tracker is
        // threaded through every `ToolUseContext` so BashTool can
        // read it as the spawn cwd and write back `new_cwd` after
        // each command — `cd /tmp` in turn N survives into turn N+1.
        let session_original_cwd = cwd.clone();
        let session_current_cwd = Arc::new(RwLock::new(cwd.clone()));
        let loop_sentinel_state = Arc::new(Mutex::new(
            coco_skills::bundled::loop_skill::LoopSentinelState::default(),
        ));

        // ── Session-scoped shell provider ──
        // Build once at session start so the provider keeps the same
        // resolved shell binary and session-scoped `/env` store across all
        // shell-tool invocations. Bash additionally keeps snapshot watch +
        // session-env reader state.
        let (shell_provider, session_env_vars): (
            Option<Arc<dyn coco_shell::ShellProvider>>,
            Option<coco_shell::SessionEnvVars>,
        ) = match active_shell_tool {
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
                let shell_provider = Arc::new(coco_shell::BashProvider::new(
                    shell,
                    session_env_reader,
                    session_env_vars.clone(),
                    shell_prefix,
                )) as Arc<dyn coco_shell::ShellProvider>;
                (Some(shell_provider), Some(session_env_vars))
            }
            coco_types::ActiveShellTool::PowerShell => {
                let shell = crate::shell_tool_selection::require_shell(
                    coco_shell::ShellType::PowerShell,
                    "PowerShell tool selected, but neither `pwsh` nor `powershell` was found",
                )?;
                let session_env_vars = coco_shell::SessionEnvVars::new();
                let shell_provider = Arc::new(coco_shell::PowerShellProvider::new(
                    shell,
                    session_env_vars.clone(),
                )) as Arc<dyn coco_shell::ShellProvider>;
                (Some(shell_provider), Some(session_env_vars))
            }
            coco_types::ActiveShellTool::Disabled => (None, None),
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
            permission_mode_availability,
            use_auto_mode_during_plan: runtime_config.settings.use_auto_mode_during_plan_enabled(),
            permission_rule_source_roots: permission_rule_source_roots.clone(),
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
            log_assistant_responses: runtime_config.settings.merged.log.assistant_responses,
            session_id: typed_session_id.clone(),
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
        let transcript_dedup = Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::<
            uuid::Uuid,
        >::new()));
        let terminal_goal_metadata_written = Arc::new(AtomicBool::new(false));
        let tool_result_replacement_state = Arc::new(tokio::sync::RwLock::new(
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX),
        ));

        // ── Agent definition catalog ──
        // Build the per-session [`AgentDefinitionStore`] once at startup
        // so AgentTool's dynamic prompt sees the same set the SDK
        // `initialize.agents` listing returns. The snapshot inspector
        // wires `pending_snapshot_update` per definition so `/agents show`
        // can flag drift without each consumer re-running the
        // `check_agent_memory_snapshot` IO.
        // Errors / missing dirs are non-fatal: the store keeps the
        // built-in roster and the per-turn engine reads the resulting
        // (mostly built-in) catalog. Snapshot is reload-able via
        // [`Self::reload_agent_catalog`]; this initial build lives on
        // the blocking pool because the markdown loader is sync IO.
        let auto_memory_enabled = runtime_config.memory_activation.active;
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

        let orchestration_engine_config = Arc::new(std::sync::RwLock::new(engine_config.clone()));
        // FileHistoryState — backed by JSONL transcript when enabled.
        // The sink reads session id from the same synchronized engine-config
        // mirror used by detached hook factories, so `/clear` regen propagates
        // without a separate file-history identity slot.
        let file_history = if file_checkpointing_enabled(
            runtime_config.settings.merged.file_checkpointing_enabled,
            is_non_interactive,
        ) {
            let sink: Arc<dyn FileHistorySnapshotSink> = Arc::new(TranscriptFileHistorySink::new(
                project_paths.clone(),
                orchestration_engine_config.clone(),
            ));
            let mut state = FileHistoryState::new();
            state.set_sink(sink);
            Some(Arc::new(RwLock::new(state)))
        } else {
            None
        };
        let execution = SessionExecutionResources::new(tools, model_runtimes);
        let hook_resources = SessionHookResources::new(
            hook_registry,
            hook_llm_handle,
            coco_hooks::SyncHookEventBuffer::new(),
            Arc::new(coco_hooks::async_registry::AsyncHookRegistry::new()),
            Arc::new(RwLock::new(None)),
        );
        let persistence = SessionPersistenceResources::new(
            session_manager,
            project_paths,
            transcript_store,
            persist_session,
        );
        let project_resources = SessionProjectResources::new(process_runtime, project_services);
        let config_resources =
            SessionConfigResources::new(config_home, runtime_config, config_reloader);
        let catalog_resources = SessionCatalogResources::new(command_registry, skill_manager);
        let turn_resources = SessionTurnResources::new(
            Arc::new(coco_tool_runtime::DiskBackedScheduleStore::new(
                cwd.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                    .join("scheduled_tasks.json"),
            )),
            side_query,
            usage_accounting,
            mailbox,
            permission_bridge,
        );
        let lifecycle_resources =
            SessionLifecycleResources::new(CancellationToken::new(), pid_registry);
        let command_resources = SessionCommandResources::new(
            session_attachment_tx,
            session_attachment_rx,
            coco_query::CommandQueue::new(),
        );
        let title_resources = SessionTitleResources::new(fast_model_spec, auto_title_enabled);
        let workspace_resources =
            SessionWorkspaceResources::new(session_original_cwd, project_root, session_current_cwd);
        let engine_config_resources = SessionEngineConfigResources::new(
            Arc::new(RwLock::new(engine_config)),
            orchestration_engine_config,
            Arc::new(RwLock::new(HashMap::new())),
        );
        let engine_state_resources = SessionEngineStateResources::new(
            file_read_state,
            file_history,
            app_state,
            session_env_vars,
            loop_sentinel_state,
            Arc::new(coco_tool_runtime::InMemoryPendingMessageStore::new()),
            auto_mode_state,
            denial_tracker,
            transcript_dedup,
            Arc::new(tokio::sync::Mutex::new(None)),
            terminal_goal_metadata_written,
            tool_result_replacement_state,
        );
        let integration_resources = SessionIntegrationResources::new(
            Arc::new(RwLock::new(None)),
            Arc::new(RwLock::new(None)),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Arc::new(RwLock::new(None)),
        );
        let handle_resources = SessionHandleResources::new(swarm_agent_handle);
        let permission_resources = SessionPermissionResources::new();
        let agent_catalog_resources = SessionAgentCatalogResources::new(
            agent_search_paths,
            builtin_agent_catalog,
            agent_catalog,
        );
        let memory_resources = SessionMemoryResources::new(memory_runtime, skill_review_runtime);
        let sandbox_resources = SessionSandboxResources::new(sandbox_state);
        let history_resources = SessionHistoryResources::new(Arc::new(Mutex::new({
            let mut h = MessageHistory::new();
            // Stamp F9 envelope onto history so every history_sync
            // emit carries session_id automatically. agent_id is
            // None for the main session; subagents stamp their own
            // via a separate construction site in `engine_session`.
            h.set_envelope(typed_session_id.clone(), None);
            h
        })));

        let runtime = Self {
            execution,
            catalog_resources,
            config_resources,
            project_resources,
            persistence,
            title_resources,
            turn_resources,
            command_resources,
            lifecycle_resources,
            workspace_resources,
            engine_config_resources,
            engine_state_resources,
            integration_resources,
            handle_resources,
            permission_resources,
            agent_catalog_resources,
            memory_resources,
            sandbox_resources,
            history_resources,
            hook_resources,
        };

        Ok(Arc::new(runtime))
    }
}
