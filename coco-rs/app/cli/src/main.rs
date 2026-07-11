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

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;

use coco_agent_host::{
    headless::{build_runtime_config_for_cli, resolve_main_model},
    resume_resolver,
    resume_resolver::ResumePlan,
    sdk_server::{
        SdkServer, SessionTurnExecutor, StdioTransport, cli_bootstrap::CliInitializeBootstrap,
        spawn_app_server_local_outbound_forwarder,
    },
    session_bootstrap::{build_engine_resources, install_session_late_binds},
};
use coco_cli::{Cli, Commands, McpAction, tracing_init};
use coco_config::global_config;
use coco_hub_connector::HubConnectorSender;
use coco_session::SessionManager;
use tokio::task::JoinHandle;

mod bin_handlers;
mod tui_runner;
use coco_agent_host::{conversation_export, session_runtime};

/// Stack size for tokio worker + blocking threads (default: 2 MiB).
///
/// Workspace crates compile at opt-level 0 in dev builds, so the
/// engine's nested `async fn` poll chains carry very large stack
/// frames — the tokio default has overflowed in practice (`/compact`
/// fork pipeline on a `tokio-rt-worker`). 8 MiB costs virtual address
/// space only; pages commit on touch.
const TOKIO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;
#[cfg(unix)]
const SDK_UNIX_LISTENER_CHANNEL_CAPACITY: usize = 256;
const SDK_WEBSOCKET_LISTENER_CHANNEL_CAPACITY: usize = 256;
#[cfg(windows)]
const SDK_NAMED_PIPE_LISTENER_CHANNEL_CAPACITY: usize = 256;

fn server_config_usize(value: i64, fallback: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn server_config_duration_secs(value: i64, fallback: Duration) -> Duration {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

fn shutdown_drain_result(
    component: &str,
    outcome: &coco_agent_host::shutdown::ShutdownDrainOutcome,
) -> Result<()> {
    if outcome.is_clean() {
        return Ok(());
    }
    Err(anyhow::anyhow!("{component} shutdown drain {outcome}"))
}

fn server_config_surface_limits(
    server_config: &coco_config::ServerConfig,
) -> coco_app_server::SurfaceLimits {
    coco_app_server::SurfaceLimits {
        max_surfaces_per_connection: server_config_usize(
            server_config.max_surfaces_per_connection,
            8,
        ),
        max_passive_surfaces_per_session: server_config_usize(
            server_config.max_passive_surfaces_per_session,
            16,
        ),
    }
}

#[cfg(unix)]
struct SdkUnixListenerTask {
    socket_path: PathBuf,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<()>>,
    outbound_forwarder: JoinHandle<()>,
}

struct SdkWebSocketListenerTask {
    bind_addr: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<()>>,
    outbound_forwarder: JoinHandle<()>,
}

#[cfg(windows)]
struct SdkNamedPipeListenerTask {
    pipe_name: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    listener_task: JoinHandle<Result<()>>,
    outbound_forwarder: JoinHandle<()>,
}

#[cfg(unix)]
fn sdk_unix_socket_path(runtime_config: &coco_config::RuntimeConfig) -> Option<PathBuf> {
    runtime_config
        .server
        .unix_socket_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn sdk_websocket_bind(runtime_config: &coco_config::RuntimeConfig) -> Option<String> {
    runtime_config
        .server
        .websocket_bind
        .as_deref()
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
        .map(str::to_string)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn sdk_named_pipe_name(runtime_config: &coco_config::RuntimeConfig) -> Option<String> {
    runtime_config
        .server
        .named_pipe_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

#[cfg(unix)]
fn start_sdk_unix_listener(
    socket_path: Option<PathBuf>,
    adapter: coco_app_server::JsonRpcAdapter<coco_agent_host::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_agent_host::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_agent_host::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
    turn_drain_timeout: Duration,
) -> Result<Option<SdkUnixListenerTask>> {
    let Some(socket_path) = socket_path else {
        return Ok(None);
    };

    let listener = coco_app_server_transport::bind_ndjson_unix_listener(&socket_path)
        .with_context(|| {
            format!(
                "failed to bind SDK AppServer Unix socket at {}",
                socket_path.display()
            )
        })?;
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(SDK_UNIX_LISTENER_CHANNEL_CAPACITY);
    let handler = Arc::new(
        coco_agent_host::sdk_server::AppServerSdkHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        ),
    );
    let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
        app_server,
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(hub_connector)),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task_socket_path = socket_path.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_unix_listener_until_shutdown(listener, handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                socket_path = %task_socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_agent_host::sdk",
        socket_path = %socket_path.display(),
        "SDK AppServer Unix listener started"
    );
    Ok(Some(SdkUnixListenerTask {
        socket_path,
        shutdown_tx,
        listener_task,
        outbound_forwarder,
    }))
}

async fn start_sdk_websocket_listener(
    bind_addr: Option<String>,
    adapter: coco_app_server::JsonRpcAdapter<coco_agent_host::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_agent_host::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_agent_host::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
    turn_drain_timeout: Duration,
) -> Result<Option<SdkWebSocketListenerTask>> {
    let Some(bind_addr) = bind_addr else {
        return Ok(None);
    };

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| {
            format!("failed to bind SDK AppServer WebSocket listener at {bind_addr}")
        })?;
    let local_addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| bind_addr.clone());
    let (outbound_tx, outbound_rx) =
        tokio::sync::mpsc::channel(SDK_WEBSOCKET_LISTENER_CHANNEL_CAPACITY);
    let handler = Arc::new(
        coco_agent_host::sdk_server::AppServerSdkHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        ),
    );
    let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
        app_server,
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(hub_connector)),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let adapter = adapter.clone();
    let task_bind_addr = local_addr.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_websocket_listener_until_shutdown(listener, handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                bind_addr = %task_bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_agent_host::sdk",
        bind_addr = %local_addr,
        "SDK AppServer WebSocket listener started"
    );
    Ok(Some(SdkWebSocketListenerTask {
        bind_addr: local_addr,
        shutdown_tx,
        listener_task,
        outbound_forwarder,
    }))
}

#[cfg(windows)]
fn start_sdk_named_pipe_listener(
    pipe_name: Option<String>,
    adapter: coco_app_server::JsonRpcAdapter<coco_agent_host::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_agent_host::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_agent_host::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
    turn_drain_timeout: Duration,
) -> Result<Option<SdkNamedPipeListenerTask>> {
    let Some(pipe_name) = pipe_name else {
        return Ok(None);
    };

    let listener = coco_app_server_transport::bind_ndjson_named_pipe_listener(&pipe_name)
        .with_context(|| {
            format!("failed to bind SDK AppServer Windows named pipe at {pipe_name}")
        })?;
    let (outbound_tx, outbound_rx) =
        tokio::sync::mpsc::channel(SDK_NAMED_PIPE_LISTENER_CHANNEL_CAPACITY);
    let handler = Arc::new(
        coco_agent_host::sdk_server::AppServerSdkHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        ),
    );
    let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
        app_server,
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(hub_connector)),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let adapter = adapter.clone();
    let task_pipe_name = pipe_name.clone();
    let listener_task = tokio::spawn(async move {
        let result = adapter
            .run_named_pipe_listener_until_shutdown(listener, handler, shutdown_rx)
            .await;
        if let Err(error) = &result {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                pipe_name = %task_pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_agent_host::sdk",
        pipe_name = %pipe_name,
        "SDK AppServer named-pipe listener started"
    );
    Ok(Some(SdkNamedPipeListenerTask {
        pipe_name,
        shutdown_tx,
        listener_task,
        outbound_forwarder,
    }))
}

#[cfg(unix)]
async fn shutdown_sdk_unix_listener(
    listener: Option<SdkUnixListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                socket_path = %listener.socket_path.display(),
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer Unix listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}

async fn shutdown_sdk_websocket_listener(
    listener: Option<SdkWebSocketListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                bind_addr = %listener.bind_addr,
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer WebSocket listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}

#[cfg(windows)]
async fn shutdown_sdk_named_pipe_listener(
    listener: Option<SdkNamedPipeListenerTask>,
    shutdown_timeout: Duration,
) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(shutdown_timeout, &mut listener.listener_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                pipe_name = %listener.pipe_name,
                timeout_secs = shutdown_timeout.as_secs(),
                "aborting SDK AppServer named-pipe listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
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
                return run_sdk_mode(&cli, startup_cwd.clone(), process_runtime.clone()).await;
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

/// Run in SDK mode: NDJSON-over-stdio JSON-RPC control protocol.
async fn run_sdk_mode(
    cli: &Cli,
    cwd: PathBuf,
    process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
) -> Result<()> {
    let agent_host_options = Arc::new(cli.agent_host_options());
    tracing::info!(
        target: "coco_agent_host::sdk",
        cwd = %cwd.display(),
        "sdk mode starting"
    );
    let runtime_config =
        coco_agent_host::headless::build_runtime_config_for_cli(&agent_host_options, &cwd)?;
    coco_agent_host::model_card_refresh::spawn_if_enabled(&runtime_config);

    let resources =
        build_engine_resources(&process_runtime, &agent_host_options, &runtime_config, &cwd)?;
    let is_real_anthropic = resources.provider_api == Some(coco_types::ProviderApi::Anthropic);
    let system_prompt = Some(resources.system_prompt.clone());

    let session_manager = Arc::new(SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        global_config::config_home(),
    ));
    let session_manager_for_runtime = session_manager.clone();

    // Slash-command registry — built once inside `build_engine_resources`
    // with the full load order (builtins → extended → skills →
    // plugin contributions → P1 handlers). Both the SDK
    // `initialize.commands` advertisement and the TUI dispatch chain
    // (`tui_runner::dispatch_slash_command`) read from the same Arc.
    let command_registry = resources.command_registry.clone();

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
    let agent_search_paths = resources
        .project_services
        .agent_search_paths(&global_config::config_home(), &cwd);

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
        .with_cwd(cwd.clone())
        .with_command_registry(command_registry.clone())
        .with_available_output_styles(available_output_styles)
        .with_agent_search_paths(agent_search_paths.clone());
    if let Some(auth) = auth_method {
        bootstrap_builder = bootstrap_builder.with_auth_method(auth);
    }
    let bootstrap: Arc<dyn coco_agent_host::sdk_server::InitializeBootstrap> =
        Arc::new(bootstrap_builder);

    if let Some(msg) = &resources.startup.notification {
        eprintln!("warning: {msg}");
    }
    let bypass_permissions_available = resources.startup.bypass_available;
    let permission_mode = resources.startup.mode;

    let transport = StdioTransport::new();
    let app_server_turn_drain_timeout = server_config_duration_secs(
        runtime_config.server.turn_drain_timeout_secs,
        Duration::from_secs(10),
    );
    // Plugin file watcher → SDK NDJSON: mirrors TUI so SDK clients
    // receive `plugins/changed` notifications.
    let (plugin_notif_tx, plugin_notif_rx) = tokio::sync::mpsc::channel(16);
    let _plugin_watcher_guard =
        coco_agent_host::plugin_watch::spawn(plugin_notif_tx, &cwd, &global_config::config_home());
    // Startup marketplace maintenance (seed/reconcile/delist) on the SDK
    // surface too — mirrors the TUI/headless so delisted-plugin enforcement
    // runs for SDK NDJSON sessions. Background + non-fatal.
    coco_agent_host::session_bootstrap::spawn_marketplace_startup(global_config::config_home());
    let server = SdkServer::new(transport)
        .with_startup_cwd(cwd.clone())
        .with_session_manager(session_manager)
        .with_initialize_bootstrap(bootstrap)
        .with_external_notifications(plugin_notif_rx)
        .with_app_server_turn_drain_timeout(app_server_turn_drain_timeout);
    let state = server.state();
    state.set_bypass_permissions_available(bypass_permissions_available);
    // Durable session_seq: bind skip-ahead to the retention ring and install
    // the transcript watermark persist hook. SDK stdio shares the
    // same allocator across its stdio and sidecar forwarders.
    coco_agent_host::sdk_server::install_session_seq_durability(
        &state,
        server_config_usize(runtime_config.server.event_retention_per_session, 1024) as i64,
    );

    #[cfg(unix)]
    let sdk_unix_socket_path = sdk_unix_socket_path(&runtime_config);
    let sdk_websocket_bind = sdk_websocket_bind(&runtime_config);
    #[cfg(windows)]
    let sdk_named_pipe_name = sdk_named_pipe_name(&runtime_config);
    let app_server = Arc::new(coco_app_server::AppServer::<
        coco_agent_host::sdk_server::LocalAppSessionHandle,
    >::new_with_surface_limits(
        server_config_usize(runtime_config.server.max_sessions, 32),
        server_config_usize(runtime_config.server.event_retention_per_session, 1024),
        server_config_surface_limits(&runtime_config.server),
    ));
    let adapter = coco_app_server::JsonRpcAdapter::with_channel_capacity(
        Arc::clone(&app_server),
        server_config_usize(runtime_config.server.outbound_queue_frames, 256),
    );

    // Optional idle-session auto-archive, off unless
    // `server.idle_session_timeout_secs` is set. SDK multi-session mode is the
    // only place surfaceless sessions accumulate.
    let _idle_session_sweep = runtime_config
        .server
        .idle_session_timeout_secs
        .filter(|secs| *secs > 0)
        .map(|secs| {
            coco_agent_host::sdk_server::spawn_idle_session_sweep(
                Arc::clone(&app_server),
                Arc::clone(&state),
                Duration::from_secs(secs as u64),
                app_server_turn_drain_timeout,
            )
        });

    let runtime_factory_cli = Arc::clone(&agent_host_options);
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
            session_manager: session_manager_for_runtime,
            fast_model_spec: None,
            permission_bridge: None,
            process_runtime: process_runtime.clone(),
            // Interactive sessions get the full built-in roster;
            // SDK noninteractive paths can override.
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // SDK NDJSON: file-history checkpointing defaults OFF.
            is_non_interactive: true,
        },
    );
    let startup_session_id = coco_types::SessionId::generate();
    let runtime_replacement_factory = runtime_factory.clone();
    let loaded_handle = coco_agent_host::sdk_server::load_local_app_server_session_runtime(
        &app_server,
        startup_session_id.clone(),
        runtime_factory,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{}", error.message))?;
    let session_handle = loaded_handle.require_runtime_anyhow("loaded")?;
    let mcp_manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new_with_runtime_config(
            global_config::config_home(),
            &session_handle.runtime_config().mcp,
        ),
    ));
    let sdk_event_hub_connector = {
        let session_id = session_handle.current_typed_session_id().await;
        coco_agent_host::event_hub::RuntimeEventHubConnector::spawn_for_session(
            session_handle.runtime_config(),
            session_id,
            &cwd,
        )
    };

    // SDK NDJSON is a non-interactive session. Inject the `StructuredOutput`
    // tool and enable the inline enforcement nudge when `--json-schema` is set.
    // TUI never reaches this branch (different code path in `tui_runner`).
    let requires_structured_output =
        coco_agent_host::headless::inject_structured_output_tool_if_requested(
            &agent_host_options,
            session_handle.tools(),
        )?;
    if requires_structured_output {
        session_handle
            .update_engine_config(|cfg| cfg.requires_structured_output = true)
            .await;
    }

    // Late-binds shared with TUI/headless: task runtime, agent transcript
    // persistence, agent-team wiring, fork dispatcher.
    let lsp_handle = coco_agent_host::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime.clone(),
        session_handle.runtime_config(),
        &global_config::config_home(),
        session_handle.project_root(),
    )
    .await;
    install_session_late_binds(session_handle.clone(), &cwd, None, lsp_handle, None).await?;
    // Unified MCP bootstrap (shared with TUI/headless): registers config-file +
    // plugin MCP servers, attaches the manager + `McpManagerAdapter` handle, and
    // connects + registers tools in the background. Reuses the manager already
    // handed to `SdkServer` (for `mcp/setServers`) so all surfaces share one
    // source of truth.
    coco_agent_host::session_bootstrap::bootstrap_session_mcp(
        &session_handle,
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
    coco_agent_host::leader_inbox_poller::install_leader(session_handle.clone(), None).await;

    // SessionStart hooks fire once at session bootstrap; output queues
    // onto the shared sync-hook buffer and surfaces as `hook_*` reminders
    // on the first turn's reminder pass.
    session_handle.fire_session_start_hooks("startup").await;

    // Setup hooks fire at every interactive bootstrap to give project
    // setup hooks a chance to refresh state (env files, build artefacts,
    // …). The 'init' trigger is reserved for the explicit `coco init`
    // flow. Failure is logged + tolerated.
    session_handle
        .fire_setup_hooks(coco_hooks::orchestration::SetupTrigger::Maintenance)
        .await;

    coco_agent_host::sdk_server::install_sdk_session_runtime_state(
        state.clone(),
        session_handle.clone(),
        Arc::clone(&app_server),
    )
    .await;

    state
        .install_runtime_replacement(coco_agent_host::sdk_server::RuntimeReplacementContext {
            startup_session_id: startup_session_id.clone(),
            runtime_factory: runtime_replacement_factory,
            process_runtime: process_runtime.clone(),
            cwd: cwd.clone(),
            requires_structured_output,
        })
        .await;
    let server = if let Some(connector) = &sdk_event_hub_connector {
        server.with_hub_connector_sender(connector.sender())
    } else {
        server
    };

    let runner = Arc::new(SessionTurnExecutor::new(cli.max_turns, system_prompt));
    server.set_turn_runner(runner).await;

    tracing::info!(
        target: "coco_agent_host::sdk",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        "sdk server entering AppServer bridge dispatch loop"
    );
    #[cfg(unix)]
    let sdk_unix_listener = start_sdk_unix_listener(
        sdk_unix_socket_path,
        adapter.clone(),
        Arc::clone(&app_server),
        state.clone(),
        sdk_event_hub_connector
            .as_ref()
            .map(coco_agent_host::event_hub::RuntimeEventHubConnector::sender),
        app_server_turn_drain_timeout,
    )?;
    let sdk_websocket_listener = start_sdk_websocket_listener(
        sdk_websocket_bind,
        adapter.clone(),
        Arc::clone(&app_server),
        state.clone(),
        sdk_event_hub_connector
            .as_ref()
            .map(coco_agent_host::event_hub::RuntimeEventHubConnector::sender),
        app_server_turn_drain_timeout,
    )
    .await?;
    #[cfg(windows)]
    let sdk_named_pipe_listener = start_sdk_named_pipe_listener(
        sdk_named_pipe_name,
        adapter.clone(),
        Arc::clone(&app_server),
        state.clone(),
        sdk_event_hub_connector
            .as_ref()
            .map(coco_agent_host::event_hub::RuntimeEventHubConnector::sender),
        app_server_turn_drain_timeout,
    )?;
    let connection = adapter.connect();
    // an OS signal initiates the drain rather than aborting the process.
    // Racing the dispatch loop against the signal also installs the SIGINT/
    // SIGTERM handler for the whole loop duration, so a `kill <pid>` during an
    // active turn drains cleanly instead of taking the default terminate
    // action. A second signal is caught by the bounded drain below.
    let dispatch_result = tokio::select! {
        result = server.run_app_server_connection(connection) => result.map(|_| ()),
        () = coco_agent_host::shutdown::os_interrupt_signal() => {
            tracing::info!(
                target: "coco_agent_host::sdk",
                "received OS shutdown signal; draining SDK AppServer sessions"
            );
            Ok(())
        }
    };

    let app_server_shutdown_timeout =
        Duration::from_secs(runtime_config.server.shutdown_timeout_secs as u64);
    #[cfg(unix)]
    shutdown_sdk_unix_listener(sdk_unix_listener, app_server_shutdown_timeout).await;
    shutdown_sdk_websocket_listener(sdk_websocket_listener, app_server_shutdown_timeout).await;
    #[cfg(windows)]
    shutdown_sdk_named_pipe_listener(sdk_named_pipe_listener, app_server_shutdown_timeout).await;
    let shutdown_runtimes = app_server
        .list_live_sessions()
        .into_iter()
        .filter_map(|summary| app_server.registry().get(&summary.session_id))
        .filter_map(coco_agent_host::sdk_server::LocalAppSessionHandle::into_session)
        .collect::<Vec<_>>();
    let app_server_for_shutdown = Arc::clone(&app_server);
    let state_for_shutdown = state.clone();
    let app_server_shutdown = coco_agent_host::shutdown::drain_with_timeout_or_signal(
        app_server_shutdown_timeout,
        async move {
            coco_agent_host::sdk_server::shutdown_local_app_server_sessions(
                app_server_for_shutdown,
                state_for_shutdown,
                app_server_turn_drain_timeout,
            )
            .await
            .map_err(|error| format!("{}: {}", error.code, error.message))
        },
        coco_agent_host::shutdown::os_interrupt_signal(),
    )
    .await;
    match &app_server_shutdown {
        coco_agent_host::shutdown::ShutdownDrainOutcome::Clean => {}
        coco_agent_host::shutdown::ShutdownDrainOutcome::Failed { message } => tracing::warn!(
            target: "coco_agent_host::sdk",
            message = %message,
            "SDK AppServer shutdown drain failed"
        ),
        coco_agent_host::shutdown::ShutdownDrainOutcome::TimedOut { timeout_secs } => {
            tracing::warn!(
                target: "coco_agent_host::sdk",
                timeout_secs,
                "SDK AppServer shutdown drain timed out"
            )
        }
        coco_agent_host::shutdown::ShutdownDrainOutcome::Interrupted => tracing::warn!(
            target: "coco_agent_host::sdk",
            "SDK AppServer shutdown drain interrupted by signal"
        ),
    }

    // Wait for any in-flight auto-memory extraction to complete before
    // we exit so partial writes aren't dropped on process shutdown. Done
    // after the SDK AppServer bridge exits so the dispatch loop has
    // already stopped accepting new turns.
    for session_runtime in &shutdown_runtimes {
        // Persist coordinator mode at exit so a later `--resume` re-derives the
        // role (R2). The SDK leader path previously never wrote it, silently
        // dropping the coordinator role on resume.
        let session_id = session_runtime.current_typed_session_id().await;
        coco_agent_host::coordinator_mode_resume::persist_session_mode(
            session_runtime.session_manager(),
            &session_id,
            &session_runtime.runtime_config().features,
        );
        if let Some(memory_runtime) = session_runtime.memory_runtime() {
            let _ = memory_runtime
                .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
                .await;
        }
    }

    let event_hub_shutdown = if let Some(connector) = sdk_event_hub_connector {
        connector
            .shutdown_and_flush_with_timeout(app_server_shutdown_timeout)
            .await
    } else {
        coco_agent_host::shutdown::ShutdownDrainOutcome::Clean
    };

    if let Err(e) = dispatch_result {
        tracing::error!(
            target: "coco_agent_host::sdk",
            error = %e,
            "sdk dispatch loop exited with error"
        );
        eprintln!("sdk mode: dispatch loop exited with error: {e}");
        return Err(anyhow::anyhow!("sdk dispatch failed: {e}"));
    }
    shutdown_drain_result("SDK AppServer", &app_server_shutdown)?;
    shutdown_drain_result("SDK Event Hub", &event_hub_shutdown)
}

#[cfg(test)]
#[path = "main.test.rs"]
mod tests;
