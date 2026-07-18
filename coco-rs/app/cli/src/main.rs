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

use std::{io::Read, path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;

use coco_agent_host::{
    headless::{build_runtime_config_for_cli, resolve_main_model},
    remote_host::{HostBuilder, RemoteHostOptions},
    resume_resolver,
    resume_resolver::ResumePlan,
};
use coco_app_runtime::ProcessRuntime;
use coco_cli::{
    Cli, Commands, McpAction,
    execution_plan::{ExecutionMode, ExecutionPlan, IoCapabilities, build_execution_plan},
    tracing_init,
};
use coco_config::global_config;
use coco_sdk_server::SdkSidecarConfig;

mod bin_handlers;
mod headless;
mod sdk;
mod tui;
use coco_agent_host::session_runtime;

/// Convert a component shutdown-drain outcome into a process result.
fn shutdown_drain_result(
    component: &str,
    outcome: &coco_agent_host::shutdown::ShutdownDrainOutcome,
) -> Result<()> {
    if outcome.is_clean() {
        return Ok(());
    }
    Err(anyhow::anyhow!("{component} shutdown drain {outcome}"))
}

/// Stack size for tokio worker + blocking threads (default: 2 MiB).
///
/// Workspace crates compile at opt-level 0 in dev builds, so the
/// engine's nested `async fn` poll chains carry very large stack
/// frames — the tokio default has overflowed in practice (`/compact`
/// fork pipeline on a `tokio-rt-worker`). 8 MiB costs virtual address
/// space only; pages commit on touch.
const TOKIO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

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

struct ProcessRuntimeShutdownGuard {
    runtime: Arc<ProcessRuntime>,
}

impl ProcessRuntimeShutdownGuard {
    fn new(runtime: Arc<ProcessRuntime>) -> Self {
        Self { runtime }
    }
}

impl Drop for ProcessRuntimeShutdownGuard {
    fn drop(&mut self) {
        self.runtime.shutdown_background_tasks();
    }
}

fn main() -> Result<()> {
    // Tier 1 of the fault handler: snapshot the cooked termios and install
    // async-signal-safe SIGSEGV/SIGBUS/… handlers before anything else runs, so
    // even an early-startup hard fault leaves the terminal usable. The TUI later
    // arms the full RESTORE_SEQ write via `arm_tui_restore`.
    coco_utils_crash_handler::install_terminal_restore_only();
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
    let execution_plan = build_execution_plan(&cli, IoCapabilities::detect())?;

    // Bind the handle for the lifetime of `main` so the non-blocking
    // file appender flushes on drop. `Mode::Skip` (status/doctor/etc.)
    // returns `None` and never installs a global subscriber.
    let _tracing_handle = tracing_init::install(&cli, &startup_cwd, &execution_plan)?;
    coco_cli::startup_profile::mark("subscriber_installed");
    let process_runtime = ProcessRuntime::global();
    let _process_runtime_shutdown = ProcessRuntimeShutdownGuard::new(process_runtime.clone());

    tracing::info!(
        target: "coco_agent_host::startup",
        version = env!("CARGO_PKG_VERSION"),
        subcommand = ?cli.command.as_ref().map(std::mem::discriminant),
        has_prompt = cli.prompt.is_some(),
        "coco entry"
    );

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
                return tui::run_tui(
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
                return headless::run_chat(
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
                return headless::run_chat(
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
            Commands::ReleaseNotes => {
                let version = env!("CARGO_PKG_VERSION");
                println!("Release Notes — v{version}");
                println!();
                println!("See full changelog at:");
                println!("https://github.com/anthropics/claude-code/releases");
                return Ok(());
            }
            Commands::Sdk => {
                let host = HostBuilder::new(
                    RemoteHostOptions {
                        agent_host_options: cli.agent_host_options(),
                        max_turns: cli.max_turns,
                    },
                    startup_cwd.clone(),
                    process_runtime.clone(),
                )
                .prepare()
                .await?;
                let sidecar_config = sdk_sidecar_config_from_runtime_config(host.runtime_config());
                sdk::run_sdk_mode(host, sidecar_config).await?;
                return Ok(());
            }
        }
    }

    match execution_plan.mode {
        ExecutionMode::Headless => {
            let prompt = resolve_headless_prompt(&cli, execution_plan)?;
            tracing::info!(
                target: "coco_agent_host::startup",
                mode = "headless",
                stdout_is_terminal = execution_plan.io.stdout_is_terminal,
                stdin_is_terminal = execution_plan.io.stdin_is_terminal,
                prompt_len = prompt.len(),
                "running headless chat"
            );
            headless::run_chat(
                &cli,
                Some(&prompt),
                startup_cwd.clone(),
                process_runtime.clone(),
            )
            .await
        }
        ExecutionMode::Tui => {
            // Resolve `--resume` / `--continue` / `--fork-session` once
            // and hand off to the TUI runner. `None` keeps the default
            // fresh-session bootstrap.
            let cwd = startup_cwd.clone();
            let runtime_paths = coco_agent_host::paths::runtime_paths();
            let plan: Option<ResumePlan> = resume_resolver::resolve(
                &cli.agent_host_options(),
                runtime_paths.memory_base(),
                &cwd,
            )?;
            coco_cli::startup_profile::mark("resume_resolved");
            tracing::info!(
                target: "coco_agent_host::startup",
                mode = "tui",
                resuming = plan.is_some(),
                "launching interactive TUI"
            );
            tui::run_tui(cli.agent_host_options(), plan, cwd, process_runtime.clone()).await
        }
        ExecutionMode::Skip | ExecutionMode::Sdk => {
            unreachable!("non-command execution plan cannot be skip or sdk")
        }
    }
}

fn resolve_headless_prompt(cli: &Cli, plan: ExecutionPlan) -> Result<String> {
    if let Some(prompt) = &cli.prompt {
        return Ok(prompt.clone());
    }
    if !plan.io.stdin_is_terminal {
        let mut prompt = String::new();
        std::io::stdin().read_to_string(&mut prompt)?;
        return Ok(prompt);
    }
    Ok("Hello!".to_string())
}

#[cfg(test)]
#[path = "main.test.rs"]
mod tests;
