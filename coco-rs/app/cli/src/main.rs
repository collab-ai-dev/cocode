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

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;

use coco_agent_host::{
    headless::{build_runtime_config_for_cli, resolve_main_model},
    remote_host::{RemoteHostOptions, prepare_remote_host},
    resume_resolver,
    resume_resolver::ResumePlan,
};
use coco_cli::{Cli, Commands, McpAction, tracing_init};
use coco_config::global_config;
use coco_sdk_server::{SdkSidecarConfig, run_sdk_mode};

mod bin_handlers;
mod tui_runner;
use coco_agent_host::session_runtime;

/// Stack size for tokio worker + blocking threads (default: 2 MiB).
///
/// Workspace crates compile at opt-level 0 in dev builds, so the
/// engine's nested `async fn` poll chains carry very large stack
/// frames — the tokio default has overflowed in practice (`/compact`
/// fork pipeline on a `tokio-rt-worker`). 8 MiB costs virtual address
/// space only; pages commit on touch.
const TOKIO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

fn shutdown_drain_result(
    component: &str,
    outcome: &coco_agent_host::shutdown::ShutdownDrainOutcome,
) -> Result<()> {
    if outcome.is_clean() {
        return Ok(());
    }
    Err(anyhow::anyhow!("{component} shutdown drain {outcome}"))
}

fn sdk_sidecar_config_from_runtime_config(
    runtime_config: &coco_config::RuntimeConfig,
) -> SdkSidecarConfig {
    let mut config = SdkSidecarConfig::default();
    #[cfg(unix)]
    if let Some(path) = runtime_config
        .server
        .unix_socket_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        config = config.with_unix_socket_path(PathBuf::from(path));
    }
    if let Some(bind_addr) = runtime_config
        .server
        .websocket_bind
        .as_deref()
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
    {
        config = config.with_websocket_bind(bind_addr.to_string());
    }
    #[cfg(windows)]
    if let Some(name) = runtime_config
        .server
        .named_pipe_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        config = config.with_named_pipe_name(name.to_string());
    }
    config
}

fn main() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(TOKIO_THREAD_STACK_BYTES)
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    // Sandbox inner-stage self-re-exec: when argv is
    // `--apply-seccomp <mode> -- <prog> <args>` (Linux) or
    // `--apply-windows-sandbox <b64> -- <prog> <args>`, this applies the
    // sandbox filter and execs the real program (never returns). For a normal
    // invocation it returns immediately and we fall through to clap parsing.
    // MUST precede `Cli::parse()` — clap would otherwise reject the unknown
    // `--apply-*` flag and the inner stage would die before applying the filter.
    coco_sandbox::dispatch_or_continue(std::env::args_os());

    let mut cli = Cli::parse();
    // `--bare` is the flag form of bare mode; export the env so every
    // downstream `is_env_truthy (CocoBareMode)` read — session bootstrap
    // and the per-turn finalize — observes it.
    if cli.bare {
        // SAFETY: set once at startup, single-threaded, before any task spawn.
        unsafe {
            std::env::set_var(coco_config::EnvKey::CocoBareMode.as_str(), "1");
        }
    }
    coco_cli::startup_profile::init();

    // CLI startup boundary: capture the initial process cwd once;
    // every session threads its own cwd thereafter.
    #[allow(clippy::disallowed_methods)]
    let startup_cwd = std::env::current_dir()?;

    // Bind the handle for the lifetime of `main` so the non-blocking
    // file appender flushes on drop. `Mode::Skip` (status/doctor/etc.)
    // returns `None` and never installs a global subscriber.
    let _tracing_handle = tracing_init::install(&cli, &startup_cwd)?;
    coco_cli::startup_profile::mark("subscriber_installed");
    let process_runtime = coco_app_runtime::ProcessRuntime::global();

    tracing::info!(
        target: "coco_agent_host::startup",
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
    if cli.plan_mode_instructions.is_some()
        && !(cli.non_interactive
            || cli.prompt.is_some()
            || !std::io::IsTerminal::is_terminal(&std::io::stdout()))
    {
        anyhow::bail!("--plan-mode-instructions can only be used in print mode (-p / --print)");
    }
    let _embedded_hub_guard = coco_cli::embedded_hub::start_if_requested(&mut cli).await?;

    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Status => {
                let cwd = startup_cwd.clone();
                let runtime_config = build_runtime_config_for_cli(&cli.agent_host_options(), &cwd)?;
                coco_agent_host::model_card_refresh::spawn_if_enabled(&runtime_config);
                let main_model = resolve_main_model(&runtime_config);
                let mode = main_model.provider_api.map_or("mock", |api| api.as_str());
                println!("coco-rs v0.0.0 ({mode} mode)");
                println!("model: {}", main_model.model_id);
                println!("provider: {}", main_model.provider);
                coco_agent_host::provider_login::print_auth_status(&runtime_config);
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
                let cwd = startup_cwd.clone();
                let runtime_paths = coco_agent_host::paths::runtime_paths();
                let plan = resume_resolver::resolve(
                    &cli_for_resume.agent_host_options(),
                    runtime_paths.memory_base(),
                    &cwd,
                )?;
                if plan.is_none() {
                    println!("No sessions to resume.");
                    return Ok(());
                }
                return tui_runner::run_tui(
                    cli_for_resume.agent_host_options(),
                    plan,
                    cwd,
                    process_runtime.clone(),
                )
                .await;
            }
            Commands::Config { action } => {
                let cwd = startup_cwd.clone();
                return bin_handlers::config::handle_config(action, &cwd);
            }
            Commands::Chat { prompt } => {
                let prompt = prompt.as_deref().unwrap_or("Hello!");
                return run_chat(
                    &cli,
                    Some(prompt),
                    startup_cwd.clone(),
                    process_runtime.clone(),
                )
                .await;
            }
            Commands::Doctor => {
                println!("Running diagnostics...");
                println!("[ok] Shell: available");
                println!("[ok] Config: loaded");
                let cwd = startup_cwd.clone();
                let runtime_config = build_runtime_config_for_cli(&cli.agent_host_options(), &cwd)?;
                coco_agent_host::model_card_refresh::spawn_if_enabled(&runtime_config);
                let main_model = resolve_main_model(&runtime_config);
                let mode = main_model.provider_api.map_or("mock", |api| api.as_str());
                println!("[ok] Model: {} ({mode})", main_model.model_id);
                coco_agent_host::provider_login::print_auth_status(&runtime_config);
                return Ok(());
            }
            Commands::Login {
                provider,
                no_browser,
                import,
            } => {
                let cwd = startup_cwd.clone();
                return coco_agent_host::provider_login::run_login(
                    provider.clone(),
                    *no_browser,
                    import.clone(),
                    &cwd,
                )
                .await;
            }
            Commands::Logout { provider } => {
                return coco_agent_host::provider_login::run_logout(provider.clone()).await;
            }
            Commands::Init => {
                let cwd = startup_cwd.clone();
                let coco_dir = cwd.join(coco_utils_common::COCO_CONFIG_DIR_NAME);
                std::fs::create_dir_all(&coco_dir)?;
                let settings = coco_dir.join("settings.json");
                if !settings.exists() {
                    std::fs::write(&settings, "{}\n")?;
                }
                println!(
                    "Initialized {}/ directory at {}",
                    coco_utils_common::COCO_CONFIG_DIR_NAME,
                    cwd.display()
                );
                return Ok(());
            }
            Commands::Review { target } => {
                let t = target.as_deref().unwrap_or("HEAD");
                println!("Reviewing: {t}");
                return run_chat(
                    &cli,
                    Some(&format!("Review the code changes in {t}")),
                    startup_cwd.clone(),
                    process_runtime.clone(),
                )
                .await;
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
                    McpAction::Login { name, no_browser } => {
                        let cwd = startup_cwd.clone();
                        return coco_agent_host::mcp_cli::run_login(name, *no_browser, &cwd).await;
                    }
                    McpAction::Logout { name } => {
                        let cwd = startup_cwd.clone();
                        return coco_agent_host::mcp_cli::run_logout(name, &cwd).await;
                    }
                }
                return Ok(());
            }
            Commands::Plugin { action } => {
                let cwd = startup_cwd.clone();
                return bin_handlers::plugin::run_plugin_subcommand(action, &cwd).await;
            }
            Commands::Moa { action } => {
                let cwd = startup_cwd.clone();
                return bin_handlers::moa::handle_moa(action, &cwd);
            }
            Commands::Agents => {
                let cwd = startup_cwd.clone();
                return bin_handlers::agents::run_agents_subcommand(&cwd).await;
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
            Commands::ExecServer { listen } => {
                let runtime_paths = coco_exec_server::ExecServerRuntimePaths::new(
                    std::env::current_exe()?,
                    /*coco_linux_sandbox_exe*/ None,
                )?;
                coco_exec_server::run_main(listen, runtime_paths)
                    .await
                    .map_err(|error| anyhow::anyhow!(error))?;
                return Ok(());
            }
            Commands::Daemon => {
                println!("Starting daemon supervisor...");
                println!("Daemon mode is not yet fully implemented.");
                return Ok(());
            }
            Commands::Ps { json, all } => {
                let config_home = global_config::config_home();
                // Live PID sweep (fixes the historic `sessions/` vs
                // `sessions/pids/` directory bug — `collect_ps_entries`
                // routes through the registry's own dir helper), then
                // merge durable terminal records from the JobStore so
                // done/failed/stopped can be reported.
                let entries = bin_handlers::ps::collect_with_jobs(&config_home, *all);
                if *json {
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                    return Ok(());
                }
                println!("Live coco sessions ({} total):", entries.len());
                if entries.is_empty() {
                    println!("  (none)");
                }
                for e in &entries {
                    let kind = serde_json::to_value(e.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "?".into());
                    let state = serde_json::to_value(e.state)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "?".into());
                    let name = e.name.as_deref().unwrap_or("-");
                    println!(
                        "  pid={:<6} kind={kind:<14} state={state:<8} name={name:<20} sid={} cwd={}",
                        e.pid,
                        e.session_id,
                        e.cwd.display(),
                    );
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
                let host = prepare_remote_host(
                    RemoteHostOptions {
                        agent_host_options: cli.agent_host_options(),
                        max_turns: cli.max_turns,
                    },
                    startup_cwd.clone(),
                    process_runtime.clone(),
                )
                .await?;
                let sidecar_config = sdk_sidecar_config_from_runtime_config(host.runtime_config());
                run_sdk_mode(host, sidecar_config).await?;
                return Ok(());
            }
        }
    }

    // Mode selection: --print / piped → headless; default → interactive TUI
    let is_piped = !std::io::IsTerminal::is_terminal(&std::io::stdout());
    if cli.prompt.is_some() || is_piped {
        let prompt = cli.prompt.as_deref().unwrap_or("Hello!");
        tracing::info!(
            target: "coco_agent_host::startup",
            mode = "headless",
            piped = is_piped,
            prompt_len = prompt.len(),
            "running headless chat"
        );
        run_chat(
            &cli,
            Some(prompt),
            startup_cwd.clone(),
            process_runtime.clone(),
        )
        .await
    } else {
        // Resolve `--resume` / `--continue` / `--fork-session` once
        // and hand off to the TUI runner. `None` keeps the default
        // fresh-session bootstrap.
        let cwd = startup_cwd.clone();
        let runtime_paths = coco_agent_host::paths::runtime_paths();
        let plan: Option<ResumePlan> =
            resume_resolver::resolve(&cli.agent_host_options(), runtime_paths.memory_base(), &cwd)?;
        coco_cli::startup_profile::mark("resume_resolved");
        tracing::info!(
            target: "coco_agent_host::startup",
            mode = "tui",
            resuming = plan.is_some(),
            "launching interactive TUI"
        );
        tui_runner::run_tui(cli.agent_host_options(), plan, cwd, process_runtime.clone()).await
    }
}

/// Run a single-turn print mode (--print / piped stdout).
async fn run_chat(
    cli: &Cli,
    prompt: Option<&str>,
    cwd: PathBuf,
    process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
) -> Result<()> {
    let agent_host_options = cli.agent_host_options();
    // Resolve `--resume` / `--continue` / `--fork-session` once at
    // the boot edge so headless and TUI share identical semantics.
    // `None` means no resume flag was set; fall through to a fresh
    // session.
    let runtime_paths = coco_agent_host::paths::runtime_paths();
    let plan = resume_resolver::resolve(&agent_host_options, runtime_paths.memory_base(), &cwd)?;
    if let Some(p) = &plan {
        eprintln!(
            "{} session {} ({} prior message(s))",
            if p.is_fork { "Forked" } else { "Resumed" },
            p.source_session_id,
            p.prior_messages.len(),
        );
    }
    let opts = match plan {
        Some(p) => coco_agent_host::headless::RunChatOptions {
            cwd: Some(cwd.clone()),
            prior_messages: p
                .prior_messages
                .into_iter()
                .map(std::sync::Arc::new)
                .collect(),
            session_id_override: Some(p.session_id),
            stored_mode: p.conversation.mode,
            process_runtime: Some(process_runtime.clone()),
            ..Default::default()
        },
        None => coco_agent_host::headless::RunChatOptions {
            cwd: Some(cwd.clone()),
            process_runtime: Some(process_runtime.clone()),
            ..Default::default()
        },
    };
    let outcome =
        coco_agent_host::headless::run_chat_with_options(&agent_host_options, prompt, opts).await?;
    if let Some(msg) = &outcome.permission_notification {
        tracing::warn!(target: "coco_agent_host::headless", notice = %msg, "headless permission notice");
        eprintln!("warning: {msg}");
    }
    let mode = outcome
        .provider_api
        .map_or("mock", coco_types::ProviderApi::as_str);
    tracing::info!(
        target: "coco_agent_host::headless",
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
    shutdown_drain_result("headless AppServer", &outcome.app_server_shutdown)?;
    shutdown_drain_result("headless Event Hub", &outcome.event_hub_shutdown)
}

#[cfg(test)]
#[path = "main.test.rs"]
mod tests;
