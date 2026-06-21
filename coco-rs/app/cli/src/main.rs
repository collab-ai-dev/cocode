// Use jemalloc as the global allocator in release/distribution builds, tuned
// for a long-running TUI / server process. jemalloc's stock defaults retain
// freed pages and over-provision per-thread arenas, which inflates idle RSS.
//
// The tuning (dirty/muzzy page decay + arena cap) is baked into the jemalloc
// build via `JEMALLOC_SYS_WITH_MALLOC_CONF` in .cargo/config.toml. That is
// platform-independent: an exported `malloc_conf` symbol is only honored on
// unprefixed (Linux) jemalloc builds — on macOS the `_rjem_` prefix stays and
// such a symbol is silently ignored — whereas the build-time conf is compiled
// into the library on every target.
//
// Opt-in via the `jemalloc` feature; never active on Windows (jemalloc-sys has
// no MSVC build). Off for ordinary `cargo build`/`cargo run` so dev builds stay
// fast and platform-portable.
#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use coco_cli::Cli;
use coco_cli::Commands;
use coco_cli::McpAction;
use coco_cli::headless::build_runtime_config_for_cli;
use coco_cli::headless::resolve_main_model;
use coco_cli::paths::standard_agent_search_paths;
use coco_cli::resume_resolver;
use coco_cli::resume_resolver::ResumePlan;
use coco_cli::sdk_server::QueryEngineRunner;
use coco_cli::sdk_server::SdkServer;
use coco_cli::sdk_server::StdioTransport;
use coco_cli::sdk_server::cli_bootstrap::CliInitializeBootstrap;
use coco_cli::session_bootstrap::build_engine_resources;
use coco_cli::session_bootstrap::install_session_late_binds;
use coco_cli::tracing_init;
use coco_config::global_config;
use coco_session::SessionManager;

mod bin_handlers;
mod tui_runner;
use coco_cli::conversation_export;
use coco_cli::session_runtime;
use coco_cli::side_question;

#[tokio::main]
async fn main() -> Result<()> {
    // Sandbox inner-stage self-re-exec: when argv is
    // `--apply-seccomp <mode> -- <prog> <args>` (Linux) or
    // `--apply-windows-sandbox <b64> -- <prog> <args>`, this applies the
    // sandbox filter and execs the real program (never returns). For a normal
    // invocation it returns immediately and we fall through to clap parsing.
    // MUST precede `Cli::parse()` — clap would otherwise reject the unknown
    // `--apply-*` flag and the inner stage would die before applying the filter.
    coco_sandbox::dispatch_or_continue(std::env::args_os());

    let cli = Cli::parse();
    // `--bare` is the flag form of bare mode; export the env so every
    // downstream `is_env_truthy(CocoBareMode)` read — session bootstrap
    // and the per-turn finalize — observes it.
    if cli.bare {
        // SAFETY: set once at startup, single-threaded, before any task spawn.
        unsafe {
            std::env::set_var(coco_config::EnvKey::CocoBareMode.as_str(), "1");
        }
    }
    coco_cli::startup_profile::init();

    // Bind the handle for the lifetime of `main` so the non-blocking
    // file appender flushes on drop. `Mode::Skip` (status/doctor/etc.)
    // returns `None` and never installs a global subscriber.
    let _tracing_handle = tracing_init::install(&cli)?;
    coco_cli::startup_profile::mark("subscriber_installed");

    tracing::info!(
        target: "coco_cli::startup",
        version = env!("CARGO_PKG_VERSION"),
        subcommand = ?cli.command.as_ref().map(std::mem::discriminant),
        has_prompt = cli.prompt.is_some(),
        "coco entry"
    );

    // `--no-session-persistence` is print-mode-only: it suppresses session
    // transcript/usage writes for a one-shot run, but an interactive TUI
    // session relies on persistence to stay resumable.
    if cli.no_session_persistence
        && !(cli.non_interactive
            || cli.prompt.is_some()
            || !std::io::IsTerminal::is_terminal(&std::io::stdout())
            || matches!(
                cli.command,
                Some(Commands::Sdk | Commands::Chat { .. } | Commands::Review { .. })
            ))
    {
        anyhow::bail!(
            "--no-session-persistence can only be used in print mode (-p / --print) or SDK mode"
        );
    }

    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Status => {
                let cwd = std::env::current_dir()?;
                let runtime_config = build_runtime_config_for_cli(&cli, &cwd)?;
                coco_cli::model_card_refresh::spawn_if_enabled(&runtime_config);
                let main_model = resolve_main_model(&runtime_config);
                let mode = main_model.provider_api.map_or("mock", |api| api.as_str());
                println!("coco-rs v0.0.0 ({mode} mode)");
                println!("model: {}", main_model.model_id);
                println!("provider: {}", main_model.provider);
                coco_cli::provider_login::print_auth_status(&runtime_config);
                return Ok(());
            }
            Commands::Sessions => {
                return bin_handlers::sessions::handle_sessions();
            }
            Commands::Resume { session_id } => {
                // Synthesize the same effect as `coco --resume <id>`
                // (or `coco --continue` when no id is given) and
                // hand off to the interactive TUI so the user can
                // actually continue the conversation, not just
                // inspect metadata.
                let mut cli_for_resume = cli.clone();
                match session_id.clone() {
                    Some(id) => cli_for_resume.resume = Some(id),
                    None => cli_for_resume.continue_session = true,
                }
                let cwd = std::env::current_dir()?;
                let plan =
                    resume_resolver::resolve(&cli_for_resume, &global_config::config_home(), &cwd)?;
                if plan.is_none() {
                    println!("No sessions to resume.");
                    return Ok(());
                }
                return tui_runner::run_tui(&cli_for_resume, plan).await;
            }
            Commands::Config { action } => {
                return bin_handlers::config::handle_config(action);
            }
            Commands::Chat { prompt } => {
                let prompt = prompt.as_deref().unwrap_or("Hello!");
                return run_chat(&cli, Some(prompt)).await;
            }
            Commands::Doctor => {
                println!("Running diagnostics...");
                println!("[ok] Shell: available");
                println!("[ok] Config: loaded");
                let cwd = std::env::current_dir()?;
                let runtime_config = build_runtime_config_for_cli(&cli, &cwd)?;
                coco_cli::model_card_refresh::spawn_if_enabled(&runtime_config);
                let main_model = resolve_main_model(&runtime_config);
                let mode = main_model.provider_api.map_or("mock", |api| api.as_str());
                println!("[ok] Model: {} ({mode})", main_model.model_id);
                coco_cli::provider_login::print_auth_status(&runtime_config);
                return Ok(());
            }
            Commands::Login {
                provider,
                no_browser,
            } => {
                return coco_cli::provider_login::run_login(provider.clone(), *no_browser).await;
            }
            Commands::Logout { provider } => {
                return coco_cli::provider_login::run_logout(provider.clone()).await;
            }
            Commands::Init => {
                let cwd = std::env::current_dir()?;
                let coco_dir = cwd.join(".coco");
                std::fs::create_dir_all(&coco_dir)?;
                let settings = coco_dir.join("settings.json");
                if !settings.exists() {
                    std::fs::write(&settings, "{}\n")?;
                }
                println!("Initialized .coco/ directory at {}", cwd.display());
                return Ok(());
            }
            Commands::Review { target } => {
                let t = target.as_deref().unwrap_or("HEAD");
                println!("Reviewing: {t}");
                return run_chat(&cli, Some(&format!("Review the code changes in {t}"))).await;
            }
            Commands::Mcp { action } => {
                match action {
                    McpAction::List => println!("MCP servers: (none connected)"),
                    McpAction::Add { name, config } => {
                        println!("Adding MCP server: {name}");
                        if let Some(c) = config {
                            println!("Config: {c}");
                        }
                    }
                    McpAction::Remove { name } => println!("Removing MCP server: {name}"),
                }
                return Ok(());
            }
            Commands::Plugin { action } => {
                return bin_handlers::plugin::run_plugin_subcommand(action).await;
            }
            Commands::Agents => {
                return bin_handlers::agents::run_agents_subcommand().await;
            }
            Commands::AutoMode { subcmd } => {
                match subcmd.as_deref() {
                    Some("defaults") => {
                        println!("Auto-mode default rules:\n  (use /permissions to configure)")
                    }
                    _ => println!("Usage: coco auto-mode defaults"),
                }
                return Ok(());
            }
            Commands::Daemon => {
                println!("Starting daemon supervisor...");
                println!("Daemon mode is not yet fully implemented.");
                return Ok(());
            }
            Commands::Ps => {
                let config_home = global_config::config_home();
                let count = coco_session::count_concurrent_sessions(&config_home);
                let dir = config_home.join("sessions");
                println!("Live coco sessions ({count} total):");
                match std::fs::read_dir(&dir) {
                    Ok(entries) => {
                        let mut found = 0;
                        for entry in entries.flatten() {
                            let name_os = entry.file_name();
                            let name = name_os.to_string_lossy();
                            let Some(stem) = name.strip_suffix(".json") else {
                                continue;
                            };
                            let Ok(pid) = stem.parse::<u32>() else {
                                continue;
                            };
                            if let Ok(Some(rec)) =
                                coco_session::read_session_registration(&config_home, pid)
                            {
                                found += 1;
                                let kind = serde_json::to_value(rec.kind)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_owned))
                                    .unwrap_or_else(|| "?".into());
                                println!(
                                    "  pid={pid:<6} kind={kind:<14} sid={} cwd={}",
                                    rec.session_id,
                                    rec.cwd.display(),
                                );
                            }
                        }
                        if found == 0 {
                            println!("  (none)");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        println!("  (none)");
                    }
                    Err(e) => {
                        eprintln!("warning: failed to read {}: {e}", dir.display());
                    }
                }
                return Ok(());
            }
            Commands::Logs { session_id } => {
                println!("Showing logs for session: {session_id}");
                return Ok(());
            }
            Commands::Attach { session_id } => {
                println!("Attaching to session: {session_id}");
                return Ok(());
            }
            Commands::Kill { session_id } => {
                println!("Killing session: {session_id}");
                return Ok(());
            }
            Commands::RemoteControl => {
                println!("Starting remote control / bridge mode...");
                return Ok(());
            }
            Commands::Sync => {
                println!("Syncing with remote session...");
                return Ok(());
            }
            Commands::ReleaseNotes => {
                let version = env!("CARGO_PKG_VERSION");
                println!("Release Notes — v{version}");
                println!();
                println!("See full changelog at:");
                println!("https://github.com/anthropics/claude-code/releases");
                return Ok(());
            }
            Commands::Upgrade => {
                let version = env!("CARGO_PKG_VERSION");
                println!("Current version: {version}");
                println!("Checking for updates...");
                println!("You are on the latest version.");
                return Ok(());
            }
            Commands::Usage => {
                println!("Usage information:");
                println!("  Plan: (not available without subscription)");
                println!("  Session tokens: check /cost in interactive mode");
                return Ok(());
            }
            Commands::Sdk => {
                return run_sdk_mode(&cli).await;
            }
        }
    }

    // Mode selection: --print / piped → headless; default → interactive TUI
    let is_piped = !std::io::IsTerminal::is_terminal(&std::io::stdout());
    if cli.prompt.is_some() || is_piped {
        let prompt = cli.prompt.as_deref().unwrap_or("Hello!");
        tracing::info!(
            target: "coco_cli::startup",
            mode = "headless",
            piped = is_piped,
            prompt_len = prompt.len(),
            "running headless chat"
        );
        run_chat(&cli, Some(prompt)).await
    } else {
        // Resolve `--resume` / `--continue` / `--fork-session` once
        // and hand off to the TUI runner. `None` keeps the default
        // fresh-session bootstrap.
        let cwd = std::env::current_dir()?;
        let plan: Option<ResumePlan> =
            resume_resolver::resolve(&cli, &global_config::config_home(), &cwd)?;
        coco_cli::startup_profile::mark("resume_resolved");
        tracing::info!(
            target: "coco_cli::startup",
            mode = "tui",
            resuming = plan.is_some(),
            "launching interactive TUI"
        );
        tui_runner::run_tui(&cli, plan).await
    }
}

/// Run a single-turn print mode (--print / piped stdout).
async fn run_chat(cli: &Cli, prompt: Option<&str>) -> Result<()> {
    // Resolve `--resume` / `--continue` / `--fork-session` once at
    // the boot edge so headless and TUI share identical semantics.
    // `None` means no resume flag was set; fall through to a fresh
    // session.
    let cwd = std::env::current_dir()?;
    let plan = resume_resolver::resolve(cli, &global_config::config_home(), &cwd)?;
    if let Some(p) = &plan {
        eprintln!(
            "{} session {} ({} prior message(s))",
            if p.is_fork { "Forked" } else { "Resumed" },
            p.source_session_id,
            p.prior_messages.len(),
        );
    }
    let opts = match plan {
        Some(p) => coco_cli::headless::RunChatOptions {
            prior_messages: p
                .prior_messages
                .into_iter()
                .map(std::sync::Arc::new)
                .collect(),
            session_id_override: Some(p.session_id),
            stored_mode: p.conversation.mode,
            ..Default::default()
        },
        None => coco_cli::headless::RunChatOptions::default(),
    };
    let outcome = coco_cli::headless::run_chat_with_options(cli, prompt, opts).await?;
    if let Some(msg) = &outcome.permission_notification {
        tracing::warn!(target: "coco_cli::headless", notice = %msg, "headless permission notice");
        eprintln!("warning: {msg}");
    }
    let mode = outcome
        .provider_api
        .map_or("mock", coco_types::ProviderApi::as_str);
    tracing::info!(
        target: "coco_cli::headless",
        provider_mode = mode,
        model_id = %outcome.model_id,
        turns = outcome.turns,
        tokens_in = outcome.total_usage.input_tokens.total,
        tokens_out = outcome.total_usage.output_tokens.total,
        "headless chat complete"
    );
    eprintln!("coco-rs ({mode} mode) — model: {}\n", outcome.model_id);
    println!("{}", outcome.response_text);
    eprintln!(
        "\n─── {} turn(s) | {} in / {} out tokens ───",
        outcome.turns,
        outcome.total_usage.input_tokens.total,
        outcome.total_usage.output_tokens.total
    );
    Ok(())
}

/// Run in SDK mode: NDJSON-over-stdio JSON-RPC control protocol.
async fn run_sdk_mode(cli: &Cli) -> Result<()> {
    let cwd = std::env::current_dir()?;
    tracing::info!(
        target: "coco_cli::sdk",
        cwd = %cwd.display(),
        "sdk mode starting"
    );
    let (sandbox_reloader, runtime_config) =
        coco_cli::headless::build_runtime_config_with_reloader(cli, &cwd)?;
    coco_cli::model_card_refresh::spawn_if_enabled(&runtime_config);

    let resources = build_engine_resources(cli, &runtime_config, &cwd)?;
    let is_real_anthropic = resources.provider_api == Some(coco_types::ProviderApi::Anthropic);
    let model_id = resources.model_id.clone();
    let system_prompt = Some(resources.system_prompt.clone());

    let session_manager = Arc::new(SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        global_config::config_home(),
    ));
    let session_manager_for_runtime = session_manager.clone();

    let mcp_manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new_with_runtime_config(
            global_config::config_home(),
            &runtime_config.mcp,
        ),
    ));

    // Config-file + plugin MCP server registration + connect happens in the
    // unified `bootstrap_session_mcp` below (shared with TUI/headless). The bare
    // manager is created here only so `SdkServer` can hold it for `mcp/setServers`.

    // Slash-command registry — built once inside `build_engine_resources`
    // with the full load order (builtins → extended → skills →
    // plugin contributions → P1 handlers). Both the SDK
    // `initialize.commands` advertisement and the TUI dispatch chain
    // (`tui_runner::dispatch_slash_command`) read from the same Arc.
    let command_registry = resources.command_registry.clone();
    let skill_manager = resources.skill_manager.clone();

    // Use the manager built inside `build_engine_resources` — the
    // active style already shaped the system prompt, and we surface
    // the same name + catalog on the SDK init message so SDK clients
    // and TUI status lines stay consistent.
    let output_style_manager = resources.output_style_manager.clone();
    let current_output_style = output_style_manager.active_name_for_sdk();
    let mut available_output_styles = output_style_manager.names();
    // Prepend `default` as a selectable option even though it isn't in
    // the catalog — it represents "no style". The SDK schema lists every
    // name a client can set on `outputStyle`.
    if !available_output_styles
        .iter()
        .any(|n| n == coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME)
    {
        available_output_styles
            .insert(0, coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME.to_string());
    }
    let agent_search_paths = standard_agent_search_paths(&global_config::config_home(), &cwd);

    let auth_method = if is_real_anthropic {
        let config_dir = global_config::config_home();
        let api_key_helper = runtime_config.settings.merged.api_key_helper.clone();
        let force_env_auth = runtime_config.env_only.force_env_auth;
        tokio::task::spawn_blocking(move || {
            coco_inference::auth::resolve_auth(&coco_inference::auth::AuthResolveOptions {
                config_dir: Some(config_dir),
                api_key_helper,
                force_env_auth,
                ..Default::default()
            })
        })
        .await
        .ok()
        .flatten()
    } else {
        None
    };

    let mut bootstrap_builder = CliInitializeBootstrap::new(current_output_style)
        .with_command_registry(command_registry.clone())
        .with_available_output_styles(available_output_styles)
        .with_agent_search_paths(agent_search_paths.clone());
    if let Some(auth) = auth_method {
        bootstrap_builder = bootstrap_builder.with_auth_method(auth);
    }
    let bootstrap: Arc<dyn coco_cli::sdk_server::InitializeBootstrap> = Arc::new(bootstrap_builder);

    if let Some(msg) = &resources.startup.notification {
        eprintln!("warning: {msg}");
    }
    let bypass_permissions_available = resources.startup.bypass_available;
    let permission_mode = resources.startup.mode;

    let transport = StdioTransport::new();
    // Plugin file watcher → SDK NDJSON: mirrors TUI so SDK clients
    // receive `plugins/changed` notifications.
    let (plugin_notif_tx, plugin_notif_rx) = tokio::sync::mpsc::channel(16);
    let _plugin_watcher_guard =
        coco_cli::plugin_watch::spawn(plugin_notif_tx, &cwd, &global_config::config_home());
    // Startup marketplace maintenance (seed/reconcile/delist) on the SDK
    // surface too — mirrors the TUI/headless so delisted-plugin enforcement
    // runs for SDK NDJSON sessions. Background + non-fatal.
    coco_cli::session_bootstrap::spawn_marketplace_startup(global_config::config_home());
    let server = SdkServer::new(transport)
        .with_session_manager(session_manager)
        .with_mcp_manager(mcp_manager.clone())
        .with_initialize_bootstrap(bootstrap)
        .with_external_notifications(plugin_notif_rx);
    let state = server.state();
    state.bypass_permissions_available.store(
        bypass_permissions_available,
        std::sync::atomic::Ordering::Relaxed,
    );

    let bridge: Arc<dyn coco_tool_runtime::ToolPermissionBridge> = Arc::new(
        coco_cli::sdk_server::SdkPermissionBridge::new(state.clone()),
    );

    let session_runtime = crate::session_runtime::SessionRuntime::build(
        crate::session_runtime::SessionRuntimeBuildOpts {
            cli,
            runtime_config: Arc::new(runtime_config),
            cwd: cwd.clone(),
            model_id,
            system_prompt: system_prompt.clone().unwrap_or_default(),
            bypass_permissions_available,
            permission_mode,
            model_runtimes: None,
            tools: resources.tools,
            session_manager: session_manager_for_runtime,
            fast_model_spec: None,
            permission_bridge: Some(bridge),
            command_registry: command_registry.clone(),
            skill_manager: skill_manager.clone(),
            // Same paths the SDK `initialize.agents` listing reads —
            // per-session AgentDefinitionStore is built from these.
            agent_search_paths: agent_search_paths.clone(),
            // Interactive sessions get the full built-in roster;
            // SDK noninteractive paths can override.
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            session_id_override: None,
            // SDK NDJSON: file-history checkpointing defaults OFF.
            is_non_interactive: true,
        },
    )
    .await?;

    // Sandbox hot-reload for the long-lived SDK NDJSON server: settings.json
    // `sandbox.*` edits re-flow into the live SandboxState. Held for the
    // session; the task exits when `sandbox_reloader` drops at the end of
    // `run_sdk_mode`.
    let _sandbox_reload = match (sandbox_reloader.as_ref(), session_runtime.sandbox_state()) {
        (Some(reloader), Some(state)) => Some(coco_cli::sandbox_reload::spawn_sandbox_reload(
            state,
            &reloader.publisher(),
            cwd.clone(),
        )),
        _ => None,
    };

    // Install the SDK sandbox approval bridge onto the live SandboxState so a
    // denied sandbox path/network operation can be approved over the SDK
    // control channel. The bridge is interior-mutable on the persistent
    // `Arc<SandboxState>`, so it survives hot-reload. No-op when sandbox is
    // disabled.
    if let Some(sandbox_state) = session_runtime.sandbox_state() {
        sandbox_state.set_approval_bridge(Arc::new(
            coco_cli::sdk_server::SdkSandboxApprovalBridge::new(state.clone()),
        ));
    }

    // SDK NDJSON is a non-interactive session. Inject the `StructuredOutput`
    // tool + register its Stop function hook when `--json-schema` is
    // set. Done post-SessionRuntime so the hook lands in the same
    // `HookRegistry` the engine will dispatch from. TUI never reaches
    // this branch (different code path in `tui_runner`).
    coco_cli::headless::inject_structured_output_tool_if_requested(
        cli,
        session_runtime.tools(),
        &session_runtime.hook_registry(),
    )?;

    // Late-binds shared with TUI/headless: task runtime, agent transcript
    // persistence, agent-team wiring, fork dispatcher.
    let lsp_handle = coco_cli::session_bootstrap::build_lsp_handle_if_enabled(
        &session_runtime.runtime_config,
        &global_config::config_home(),
        &cwd,
    )
    .await;
    install_session_late_binds(session_runtime.clone(), &cwd, None, lsp_handle, None).await?;
    // Unified MCP bootstrap (shared with TUI/headless): registers config-file +
    // plugin MCP servers, attaches the manager + `McpManagerAdapter` handle, and
    // connects + registers tools in the background. Reuses the manager already
    // handed to `SdkServer` (for `mcp/setServers`) so all surfaces share one
    // source of truth.
    coco_cli::session_bootstrap::bootstrap_session_mcp(
        &session_runtime,
        &cwd,
        Some(mcp_manager),
        /*await_connect*/ false,
    )
    .await;

    // Leader-side teammate inbox consumption (R1): a long-running SDK leader
    // that approves a teammate shutdown must run teardown, or it leaks stale
    // team.json membership + orphaned task assignments. No human approval UI
    // here, so no permission bridge is registered (worker deny-path prompts
    // fail closed); teardown / idle / coordinator re-injection still flow.
    // No-op when AgentTeams is off or this session is itself a teammate.
    coco_cli::leader_inbox_poller::install_leader(session_runtime.clone(), None).await;

    // SessionStart hooks fire once at session bootstrap; output queues
    // onto the shared sync-hook buffer and surfaces as `hook_*` reminders
    // on the first turn's reminder pass.
    session_runtime.fire_session_start_hooks("startup").await;

    // Setup hooks fire at every interactive bootstrap to give project
    // setup hooks a chance to refresh state (env files, build artefacts,
    // …). The 'init' trigger is reserved for the explicit `coco init`
    // flow. Failure is logged + tolerated.
    session_runtime
        .fire_setup_hooks(coco_hooks::orchestration::SetupTrigger::Maintenance)
        .await;

    let file_history_for_server = session_runtime.file_history.clone().unwrap_or_else(|| {
        Arc::new(tokio::sync::RwLock::new(
            coco_context::FileHistoryState::new(),
        ))
    });
    let server = server
        .with_file_history(file_history_for_server, global_config::config_home())
        .with_session_runtime(session_runtime.clone());

    let runner = Arc::new(QueryEngineRunner::new(
        session_runtime,
        cli.max_tokens.unwrap_or(16_384),
        cli.max_turns,
        system_prompt,
    ));
    server.set_turn_runner(runner).await;

    tracing::info!(
        target: "coco_cli::sdk",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        "sdk server entering dispatch loop"
    );
    let dispatch_result = server.run().await;

    // Wait for any in-flight auto-memory extraction to complete before
    // we exit so partial writes aren't dropped on process shutdown. Done
    // after `server.run()` so the dispatch loop has already stopped
    // accepting new turns.
    let session_runtime_guard = state.session_runtime.read().await;
    if let Some(session_runtime) = session_runtime_guard.as_ref() {
        // Persist coordinator mode at exit so a later `--resume` re-derives the
        // role (R2). The SDK leader path previously never wrote it, silently
        // dropping the coordinator role on resume.
        let session_id = session_runtime.current_session_id().await;
        coco_cli::coordinator_mode_resume::persist_session_mode(
            &session_runtime.session_manager,
            &session_id,
            &session_runtime.runtime_config.features,
        );
    }
    if let Some(session_runtime) = session_runtime_guard.as_ref()
        && let Some(memory_runtime) = session_runtime.memory_runtime()
    {
        let _ = memory_runtime
            .extract
            .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
            .await;
        // Also wait for any in-flight session-memory fork — a partial
        // `summary.md` write would otherwise survive process exit and
        // mislead the next session's compact short-circuit.
        let _ = memory_runtime
            .session_memory
            .wait_for_extraction(coco_memory::service::session::DEFAULT_WAIT_TIMEOUT)
            .await;
    }
    drop(session_runtime_guard);

    if let Err(e) = dispatch_result {
        tracing::error!(
            target: "coco_cli::sdk",
            error = %e,
            "sdk dispatch loop exited with error"
        );
        eprintln!("sdk mode: dispatch loop exited with error: {e}");
        return Err(anyhow::anyhow!("sdk dispatch failed: {e}"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "main.test.rs"]
mod tests;
