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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;

use coco_cli::Cli;
use coco_cli::Commands;
use coco_cli::McpAction;
use coco_cli::headless::build_runtime_config_for_cli;
use coco_cli::headless::resolve_main_model;
use coco_cli::resume_resolver;
use coco_cli::resume_resolver::ResumePlan;
use coco_cli::sdk_server::SdkServer;
use coco_cli::sdk_server::StateQueryEngineRunner;
use coco_cli::sdk_server::StdioTransport;
use coco_cli::sdk_server::cli_bootstrap::CliInitializeBootstrap;
use coco_cli::sdk_server::spawn_app_server_local_outbound_forwarder;
use coco_cli::session_bootstrap::build_engine_resources;
use coco_cli::session_bootstrap::install_session_late_binds;
use coco_cli::tracing_init;
use coco_config::global_config;
use coco_hub_connector::HubConnectorSender;
use coco_session::SessionManager;
use tokio::task::JoinHandle;

mod bin_handlers;
mod tui_runner;
use coco_cli::conversation_export;
use coco_cli::session_runtime;

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
#[cfg(unix)]
const SDK_UNIX_LISTENER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const SDK_WEBSOCKET_LISTENER_CHANNEL_CAPACITY: usize = 256;
const SDK_WEBSOCKET_LISTENER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const SDK_NAMED_PIPE_LISTENER_CHANNEL_CAPACITY: usize = 256;
#[cfg(windows)]
const SDK_NAMED_PIPE_LISTENER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

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
    adapter: coco_app_server::JsonRpcAdapter<coco_cli::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_cli::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_cli::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
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
        coco_cli::sdk_server::AppServerSdkHandler::with_local_app_server(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
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
                target: "coco_cli::sdk",
                socket_path = %task_socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_cli::sdk",
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
    adapter: coco_app_server::JsonRpcAdapter<coco_cli::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_cli::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_cli::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
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
        coco_cli::sdk_server::AppServerSdkHandler::with_local_app_server(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
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
                target: "coco_cli::sdk",
                bind_addr = %task_bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_cli::sdk",
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
    adapter: coco_app_server::JsonRpcAdapter<coco_cli::sdk_server::LocalAppSessionHandle>,
    app_server: Arc<coco_app_server::AppServer<coco_cli::sdk_server::LocalAppSessionHandle>>,
    state: Arc<coco_cli::sdk_server::SdkServerState>,
    hub_connector: Option<HubConnectorSender>,
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
        coco_cli::sdk_server::AppServerSdkHandler::with_local_app_server(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
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
                target: "coco_cli::sdk",
                pipe_name = %task_pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener exited with error"
            );
        }
        result.map_err(anyhow::Error::from)
    });

    tracing::info!(
        target: "coco_cli::sdk",
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
async fn shutdown_sdk_unix_listener(listener: Option<SdkUnixListenerTask>) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(
        SDK_UNIX_LISTENER_SHUTDOWN_TIMEOUT,
        &mut listener.listener_task,
    )
    .await
    {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                socket_path = %listener.socket_path.display(),
                error = %error,
                "SDK AppServer Unix listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                socket_path = %listener.socket_path.display(),
                timeout_secs = SDK_UNIX_LISTENER_SHUTDOWN_TIMEOUT.as_secs(),
                "aborting SDK AppServer Unix listener after shutdown timeout"
            );
            listener.listener_task.abort();
            let _ = listener.listener_task.await;
        }
    }

    listener.outbound_forwarder.abort();
    let _ = listener.outbound_forwarder.await;
}

async fn shutdown_sdk_websocket_listener(listener: Option<SdkWebSocketListenerTask>) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(
        SDK_WEBSOCKET_LISTENER_SHUTDOWN_TIMEOUT,
        &mut listener.listener_task,
    )
    .await
    {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                bind_addr = %listener.bind_addr,
                error = %error,
                "SDK AppServer WebSocket listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                bind_addr = %listener.bind_addr,
                timeout_secs = SDK_WEBSOCKET_LISTENER_SHUTDOWN_TIMEOUT.as_secs(),
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
async fn shutdown_sdk_named_pipe_listener(listener: Option<SdkNamedPipeListenerTask>) {
    let Some(mut listener) = listener else {
        return;
    };

    let _ = listener.shutdown_tx.send(());
    match tokio::time::timeout(
        SDK_NAMED_PIPE_LISTENER_SHUTDOWN_TIMEOUT,
        &mut listener.listener_task,
    )
    .await
    {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener stopped with error"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                pipe_name = %listener.pipe_name,
                error = %error,
                "SDK AppServer named-pipe listener task failed"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "coco_cli::sdk",
                pipe_name = %listener.pipe_name,
                timeout_secs = SDK_NAMED_PIPE_LISTENER_SHUTDOWN_TIMEOUT.as_secs(),
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
    // downstream `is_env_truthy(CocoBareMode)` read — session bootstrap
    // and the per-turn finalize — observes it.
    if cli.bare {
        // SAFETY: set once at startup, single-threaded, before any task spawn.
        unsafe {
            std::env::set_var(coco_config::EnvKey::CocoBareMode.as_str(), "1");
        }
    }
    coco_cli::startup_profile::init();

    let startup_cwd = std::env::current_dir()?;

    // Bind the handle for the lifetime of `main` so the non-blocking
    // file appender flushes on drop. `Mode::Skip` (status/doctor/etc.)
    // returns `None` and never installs a global subscriber.
    let _tracing_handle = tracing_init::install(&cli, &startup_cwd)?;
    coco_cli::startup_profile::mark("subscriber_installed");
    let process_runtime = coco_cli::process_runtime::ProcessRuntime::global();

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
                let cwd = startup_cwd.clone();
                let runtime_paths = coco_cli::paths::runtime_paths();
                let plan =
                    resume_resolver::resolve(&cli_for_resume, runtime_paths.memory_base(), &cwd)?;
                if plan.is_none() {
                    println!("No sessions to resume.");
                    return Ok(());
                }
                return tui_runner::run_tui(&cli_for_resume, plan, cwd, process_runtime.clone())
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
                import,
            } => {
                let cwd = startup_cwd.clone();
                return coco_cli::provider_login::run_login(
                    provider.clone(),
                    *no_browser,
                    import.clone(),
                    &cwd,
                )
                .await;
            }
            Commands::Logout { provider } => {
                return coco_cli::provider_login::run_logout(provider.clone()).await;
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
                        return coco_cli::mcp_cli::run_login(name, *no_browser, &cwd).await;
                    }
                    McpAction::Logout { name } => {
                        let cwd = startup_cwd.clone();
                        return coco_cli::mcp_cli::run_logout(name, &cwd).await;
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
            target: "coco_cli::startup",
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
        let runtime_paths = coco_cli::paths::runtime_paths();
        let plan: Option<ResumePlan> =
            resume_resolver::resolve(&cli, runtime_paths.memory_base(), &cwd)?;
        coco_cli::startup_profile::mark("resume_resolved");
        tracing::info!(
            target: "coco_cli::startup",
            mode = "tui",
            resuming = plan.is_some(),
            "launching interactive TUI"
        );
        tui_runner::run_tui(&cli, plan, cwd, process_runtime.clone()).await
    }
}

/// Run a single-turn print mode (--print / piped stdout).
async fn run_chat(
    cli: &Cli,
    prompt: Option<&str>,
    cwd: PathBuf,
    process_runtime: Arc<coco_cli::process_runtime::ProcessRuntime>,
) -> Result<()> {
    // Resolve `--resume` / `--continue` / `--fork-session` once at
    // the boot edge so headless and TUI share identical semantics.
    // `None` means no resume flag was set; fall through to a fresh
    // session.
    let runtime_paths = coco_cli::paths::runtime_paths();
    let plan = resume_resolver::resolve(cli, runtime_paths.memory_base(), &cwd)?;
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
        None => coco_cli::headless::RunChatOptions {
            cwd: Some(cwd.clone()),
            process_runtime: Some(process_runtime.clone()),
            ..Default::default()
        },
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
async fn run_sdk_mode(
    cli: &Cli,
    cwd: PathBuf,
    process_runtime: Arc<coco_cli::process_runtime::ProcessRuntime>,
) -> Result<()> {
    tracing::info!(
        target: "coco_cli::sdk",
        cwd = %cwd.display(),
        "sdk mode starting"
    );
    let runtime_config = coco_cli::headless::build_runtime_config_for_cli(cli, &cwd)?;
    coco_cli::model_card_refresh::spawn_if_enabled(&runtime_config);

    let resources = build_engine_resources(&process_runtime, cli, &runtime_config, &cwd)?;
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
        .with_startup_cwd(cwd.clone())
        .with_session_manager(session_manager)
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

    #[cfg(unix)]
    let sdk_unix_socket_path = sdk_unix_socket_path(&runtime_config);
    let sdk_websocket_bind = sdk_websocket_bind(&runtime_config);
    #[cfg(windows)]
    let sdk_named_pipe_name = sdk_named_pipe_name(&runtime_config);
    let app_server = Arc::new(coco_app_server::AppServer::<
        coco_cli::sdk_server::LocalAppSessionHandle,
    >::new(/*max_sessions*/ 1, /*channel_capacity*/ 256));
    let adapter =
        coco_app_server::JsonRpcAdapter::with_channel_capacity(Arc::clone(&app_server), 256);

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
            tools: resources.tools,
            session_manager: session_manager_for_runtime,
            fast_model_spec: None,
            permission_bridge: Some(bridge),
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
    let registry_session_id = startup_session_id.clone();
    let build_session_id = startup_session_id.clone();
    let mut load_completion =
        match app_server.spawn_load(startup_session_id.clone(), async move {
            let runtime = runtime_factory
                .build_with_session_id(build_session_id)
                .await
                .map_err(|error| coco_app_server::RegistryError::load_failed(error.to_string()))?;
            Ok::<coco_cli::sdk_server::LocalAppSessionHandle, coco_app_server::RegistryError>(
                coco_cli::sdk_server::LocalAppSessionHandle::from_runtime(
                    registry_session_id,
                    runtime,
                ),
            )
        })? {
            coco_app_server::AppLoadStart::Started { completion }
            | coco_app_server::AppLoadStart::Loading(completion) => completion,
            coco_app_server::AppLoadStart::Live(_) => {
                anyhow::bail!("SDK startup AppServer session {startup_session_id} was already live")
            }
            coco_app_server::AppLoadStart::Closing(_) => {
                anyhow::bail!("SDK startup AppServer session {startup_session_id} is closing")
            }
        };
    let loaded_handle = load_completion.wait().await?;
    let session_handle = loaded_handle.runtime().cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "SDK startup AppServer session {} loaded without a runtime handle",
            loaded_handle.session_id()
        )
    })?;
    let session_runtime = session_handle.runtime().clone();
    let mcp_manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new_with_runtime_config(
            global_config::config_home(),
            &session_runtime.runtime_config().mcp,
        ),
    ));
    {
        let mut slot = state.mcp_manager.write().await;
        *slot = Some(mcp_manager.clone());
    }
    let sdk_event_hub_connector = {
        let session_id = session_runtime.current_typed_session_id().await;
        coco_cli::event_hub::RuntimeEventHubConnector::spawn_for_session(
            session_runtime.runtime_config(),
            session_id,
            &cwd,
        )
    };

    // SDK NDJSON is a non-interactive session. Inject the `StructuredOutput`
    // tool and enable the inline enforcement nudge when `--json-schema` is set.
    // TUI never reaches this branch (different code path in `tui_runner`).
    let requires_structured_output =
        coco_cli::headless::inject_structured_output_tool_if_requested(
            cli,
            session_runtime.tools(),
        )?;
    if requires_structured_output {
        session_runtime
            .update_engine_config(|cfg| cfg.requires_structured_output = true)
            .await;
    }

    // Late-binds shared with TUI/headless: task runtime, agent transcript
    // persistence, agent-team wiring, fork dispatcher.
    let lsp_handle = coco_cli::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime.clone(),
        session_runtime.runtime_config(),
        &global_config::config_home(),
        session_runtime.project_root(),
    )
    .await;
    install_session_late_binds(session_handle.clone(), &cwd, None, lsp_handle, None).await?;
    // Unified MCP bootstrap (shared with TUI/headless): registers config-file +
    // plugin MCP servers, attaches the manager + `McpManagerAdapter` handle, and
    // connects + registers tools in the background. Reuses the manager already
    // handed to `SdkServer` (for `mcp/setServers`) so all surfaces share one
    // source of truth.
    coco_cli::session_bootstrap::bootstrap_session_mcp(
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
    coco_cli::leader_inbox_poller::install_leader(session_handle.clone(), None).await;

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

    coco_cli::sdk_server::install_sdk_session_runtime_state(state.clone(), session_handle.clone())
        .await;

    let file_history_for_server = session_runtime.file_history().cloned().unwrap_or_else(|| {
        Arc::new(tokio::sync::RwLock::new(
            coco_context::FileHistoryState::new(),
        ))
    });
    {
        let mut replacement = state.runtime_replacement.write().await;
        *replacement = Some(coco_cli::sdk_server::RuntimeReplacementContext {
            runtime_factory: runtime_replacement_factory,
            process_runtime: process_runtime.clone(),
            cwd: cwd.clone(),
            requires_structured_output,
        });
    }
    let server = server.with_file_history(file_history_for_server, global_config::config_home());
    let server = if let Some(connector) = &sdk_event_hub_connector {
        server.with_hub_connector_sender(connector.sender())
    } else {
        server
    };

    let runner = Arc::new(StateQueryEngineRunner::new(
        state.clone(),
        cli.max_turns,
        system_prompt,
    ));
    server.set_turn_runner(runner).await;

    tracing::info!(
        target: "coco_cli::sdk",
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
            .map(coco_cli::event_hub::RuntimeEventHubConnector::sender),
    )?;
    let sdk_websocket_listener = start_sdk_websocket_listener(
        sdk_websocket_bind,
        adapter.clone(),
        Arc::clone(&app_server),
        state.clone(),
        sdk_event_hub_connector
            .as_ref()
            .map(coco_cli::event_hub::RuntimeEventHubConnector::sender),
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
            .map(coco_cli::event_hub::RuntimeEventHubConnector::sender),
    )?;
    let connection = adapter.connect();
    let dispatch_result = server
        .run_app_server_connection(connection)
        .await
        .map(|_| ());

    #[cfg(unix)]
    shutdown_sdk_unix_listener(sdk_unix_listener).await;
    shutdown_sdk_websocket_listener(sdk_websocket_listener).await;
    #[cfg(windows)]
    shutdown_sdk_named_pipe_listener(sdk_named_pipe_listener).await;

    // Wait for any in-flight auto-memory extraction to complete before
    // we exit so partial writes aren't dropped on process shutdown. Done
    // after the SDK AppServer bridge exits so the dispatch loop has
    // already stopped accepting new turns.
    let session_runtime_guard = state.session_runtime.read().await;
    if let Some(session_runtime) = session_runtime_guard.as_ref() {
        // Persist coordinator mode at exit so a later `--resume` re-derives the
        // role (R2). The SDK leader path previously never wrote it, silently
        // dropping the coordinator role on resume.
        let session_id = session_runtime.current_typed_session_id().await;
        coco_cli::coordinator_mode_resume::persist_session_mode(
            session_runtime.session_manager(),
            &session_id,
            &session_runtime.runtime_config().features,
        );
    }
    if let Some(session_runtime) = session_runtime_guard.as_ref()
        && let Some(memory_runtime) = session_runtime.memory_runtime()
    {
        let _ = memory_runtime
            .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
            .await;
    }
    drop(session_runtime_guard);

    if let Some(connector) = sdk_event_hub_connector {
        connector.shutdown_and_flush().await;
    }

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
