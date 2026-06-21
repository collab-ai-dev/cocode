//! In-memory SDK server harness for live tests.
//!
//! Builds a real `SdkServer` with a real `SessionRuntime` driven by a
//! real `QueryEngineRunner` against the live DeepSeek API, but talks
//! to it over an `InMemoryTransport::pair()` instead of stdio. Tests
//! drive the client end directly.
//!
//! Skips a handful of optional bootstraps that `run_sdk_mode` wires
//! (output-style discovery, agent search paths, auth resolution,
//! agent-team install, fork dispatcher, transcript store) — none of
//! them are exercised by the protocol tests we run, and skipping them
//! keeps the harness short.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use coco_cli::Cli;
use coco_cli::headless;
use coco_cli::sdk_server::CliInitializeBootstrap;
use coco_cli::sdk_server::InMemoryTransport;
use coco_cli::sdk_server::QueryEngineRunner;
use coco_cli::sdk_server::SdkServer;
use coco_cli::sdk_server::SdkTransport;
use coco_cli::session_runtime::SessionRuntime;
use coco_cli::session_runtime::SessionRuntimeBuildOpts;
use coco_commands::CommandRegistry;
use coco_commands::register_extended_builtins;
use coco_session::SessionManager;
use coco_tool_runtime::ToolRegistry;
use coco_types::ClientRequestMethod;
use coco_types::JsonRpcMessage;
use coco_types::JsonRpcNotification;
use coco_types::JsonRpcRequest;
use coco_types::NotificationMethod;
use coco_types::RequestId;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use crate::common;

/// One running SDK server bound to its transport pair. The server
/// runs in a background task; tests drive `client` and call `shutdown`
/// when done.
pub struct LiveSdkServer {
    /// Client end of the transport — `send` to push a request to the
    /// server, `recv` to read responses + notifications.
    pub client: Arc<InMemoryTransport>,
    /// Background task driving the server's dispatch loop.
    pub server_task: JoinHandle<()>,
    /// Tempdir holding the SessionManager database. Kept alive for
    /// the duration of the test so per-session JSONL writes don't
    /// race the cleanup.
    pub _sessions_dir: TempDir,
    /// Cwd tempdir kept alive so per-turn workdir writes survive
    /// assertions. Reminder tests (`skill_listing`) plant files here
    /// pre-build; using `keep()` would leak the dir, so we own it.
    pub _cwd_dir: TempDir,
    /// Reference to the running session runtime. Held to keep the
    /// runtime's per-session subsystems alive for the lifetime of
    /// the harness. Note: the SDK runner's per-turn engine writes
    /// history to `SessionHandle.history` (read via
    /// [`Self::history_snapshot`]), NOT to `runtime.history` — so
    /// reminder assertions go through the server-state path, not
    /// this field directly.
    #[allow(dead_code)]
    pub session_runtime: Arc<coco_cli::session_runtime::SessionRuntime>,
    /// `Arc<SdkServer>` retained so the harness can peek at the active
    /// `SessionHandle.history` (which is what `QueryEngineRunner` writes
    /// per-turn — `runtime.history` is not the live SDK history).
    pub server: Arc<SdkServer>,
    /// Resolved (provider, model) for the harness, for diagnostic use.
    /// Underscore-prefixed because no test reads them today; future
    /// failure messages may want to grab them via field access.
    pub _provider: String,
    pub _model_id: String,
}

impl LiveSdkServer {
    /// Snapshot the **active session's** history — that's the
    /// `SessionHandle.history` mutex `QueryEngineRunner` writes to per
    /// turn. `runtime.history` is a parallel mutex on `SessionRuntime`
    /// that the SDK runner doesn't update; reading it would always
    /// return empty.
    ///
    /// **Race**: the engine emits `turn/completed` before the SDK
    /// runner's post-engine `*h = result.final_messages` write
    /// finishes. Tests that block on the wire-level terminal
    /// notification can race past that write. To make the snapshot
    /// reliable, we poll for up to ~3s for history to grow past the
    /// pre-engine user-message-only state. Returns whatever's there
    /// after the timeout.
    pub async fn history_snapshot(&self) -> Vec<coco_messages::Message> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let snapshot = self.history_snapshot_now().await;
            // Heuristic: if history has anything beyond a single
            // user message, the engine write has landed.
            if snapshot.len() > 1 || tokio::time::Instant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    async fn history_snapshot_now(&self) -> Vec<coco_messages::Message> {
        let state = self.server.state();
        let guard = state.session.read().await;
        let Some(handle) = guard.as_ref() else {
            return Vec::new();
        };
        handle
            .history
            .lock()
            .await
            .iter()
            .map(|a| (**a).clone())
            .collect()
    }
}

impl LiveSdkServer {
    /// Stop the server and join its task. Failing to call this leaks
    /// the join handle but is harmless across tests because each test
    /// gets its own server.
    pub async fn shutdown(self) {
        let _ = self.client.close().await;
        // Server's `run` loop exits on `recv` returning Err(Closed).
        // Bound the wait so a wedged server can't hang the test.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.server_task).await;
    }
}

/// Build a minimal Cli pointing at `(provider, model)`. Adds optional
/// extra argv entries (e.g. `--max-turns 4`). Top-level flags must
/// appear BEFORE the `sdk` subcommand because clap routes them on
/// the parent parser.
pub fn cli_for(provider: &str, model: &str, extra: &[&str]) -> Cli {
    use clap::Parser;
    let model_arg = format!("{provider}/{model}");
    let mut argv: Vec<&str> = vec!["coco", "--models.main", &model_arg];
    for e in extra {
        argv.push(e);
    }
    argv.push("sdk");
    Cli::parse_from(&argv)
}

/// Spin up an `SdkServer` driven by a real `QueryEngineRunner` against
/// `(provider, model)`. The runner uses DeepSeek live; `DEEPSEEK_API_KEY`
/// must be set or the harness errors before reaching the server.
pub async fn build_live_server(provider: &str, model: &str) -> Result<LiveSdkServer> {
    build_live_server_with_options(provider, model, BuildOptions::default()).await
}

/// Optional knobs for [`build_live_server_with_options`].
#[derive(Default)]
pub struct BuildOptions {
    /// Pre-minted cwd. When `None` the harness creates one. Tests that
    /// need to plant files (`.coco/skills/*.md`, nested `CLAUDE.md`,
    /// `settings.json`) before the runtime boots pass the dir here so
    /// they can compute paths in advance.
    pub cwd: Option<TempDir>,
    /// Path to a settings.json passed via `--settings`. Used by reminder
    /// tests to install hooks (`session_start`, `user_prompt_submit`,
    /// `stop`) and toggle `system_reminder.attachments.*` flags through
    /// the production settings-merge path.
    pub settings_path: Option<std::path::PathBuf>,
}

/// Same as [`build_live_server`], with optional knobs. See [`BuildOptions`].
pub async fn build_live_server_with_options(
    provider: &str,
    model: &str,
    opts: BuildOptions,
) -> Result<LiveSdkServer> {
    let mut extra: Vec<String> = Vec::new();
    if let Some(p) = &opts.settings_path {
        extra.push("--settings".to_string());
        extra.push(p.to_string_lossy().into_owned());
    }
    let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    let cli = cli_for(provider, model, &extra_refs);
    let cwd_dir = match opts.cwd {
        Some(d) => d,
        None => common::tmpdir::make("coco-tests-sdk-cwd-")?,
    };
    let cwd = cwd_dir.path().to_path_buf();
    let sessions_dir = common::tmpdir::make("coco-tests-sdk-sessions-")?;

    // Pin `config_home()` at the sessions tempdir so the transcript
    // *writer* and the resume *reader* share one base. The SessionRuntime
    // persists transcripts under `project_paths(cwd)` =
    // `<config_home>/projects/<slug>/<id>.jsonl` (config_home is not
    // injectable — see app/cli/src/paths.rs), while `SessionManager`
    // below reads from `sessions_dir`. Without this they diverge (real
    // `~/.coco` vs tempdir) and `session/resume` fails with
    // `transcript not found`. This also makes every sdk_server live test
    // hermetic from the developer's real `~/.coco`.
    //
    // SAFETY: nextest isolates each test in its own process, so this
    // process-global set is not observed by other tests; the
    // `_sessions_dir` TempDir on `LiveSdkServer` keeps the dir alive for
    // the whole test, and the process exits before the value matters
    // again.
    unsafe { std::env::set_var("COCO_CONFIG_DIR", sessions_dir.path()) };

    let runtime_config = headless::build_runtime_config_for_cli(&cli, &cwd)?;
    let main_model = headless::resolve_main_model(&runtime_config);
    let model_id = main_model.model_id.clone();
    if model_id == "mock-model" {
        return Err(anyhow!(
            "no live provider configured (DEEPSEEK_API_KEY missing or invalid); \
             SDK live tests cannot run against the mock model"
        ));
    }

    // Curated tool subset (same rationale as the cli_deepseek harness:
    // some builtins emit non-strict schemas that DeepSeek rejects).
    let registry = ToolRegistry::new();
    registry.register(Arc::new(coco_tools::BashTool));
    registry.register(Arc::new(coco_tools::ReadTool));
    registry.register(Arc::new(coco_tools::WriteTool));
    registry.register(Arc::new(coco_tools::EditTool));
    registry.register(Arc::new(coco_tools::GlobTool));
    // Statically-typed schema (required `skill` field), safe for
    // DeepSeek. Required for the `skill_listing` reminder, which is
    // gated behind `reminder_skill_tool_loaded` (engine_turn_reminders.rs).
    registry.register(Arc::new(coco_tools::SkillTool));
    let tools = Arc::new(registry);

    let system_prompt = headless::build_system_prompt_for_model(
        &cwd,
        &runtime_config,
        &main_model.provider,
        &model_id,
        None,
        &[],
    );
    let session_manager = Arc::new(SessionManager::new(sessions_dir.path().to_path_buf()));

    let startup =
        headless::resolve_startup_permission_state(&cli, &runtime_config.settings.merged)?;

    let provider_name = main_model.provider.clone();

    // Build the SessionRuntime with the same `SessionRuntimeBuildOpts`
    // shape `run_sdk_mode` uses, minus the optional bootstraps we don't
    // need for protocol tests (agent-team, fork dispatcher, etc.).
    // Bootstrap so `initialize` returns a non-empty CommandRegistry +
    // dirs, matching what the binary's `CliInitializeBootstrap` ships.
    // Built up-front because both the SessionRuntime and the
    // CliInitializeBootstrap consume it.
    let mut command_registry = CommandRegistry::new();
    register_extended_builtins(&mut command_registry);
    let command_registry = Arc::new(tokio::sync::RwLock::new(Arc::new(command_registry)));

    // Mirror `build_session_command_registry`'s skill load so any
    // `<cwd>/.coco/skills/<name>/SKILL.md` planted by the test fixture
    // is discovered and threaded into `ReminderSources.skills`.
    let skill_manager = coco_skills::SkillManager::new();
    skill_manager.load_from_dirs(&[cwd.join(".coco").join("skills")]);
    let skill_manager = Arc::new(skill_manager);

    let session_runtime = SessionRuntime::build(SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        cwd: cwd.clone(),
        model_id: model_id.clone(),
        system_prompt: system_prompt.clone(),
        bypass_permissions_available: startup.bypass_available,
        permission_mode: startup.mode,
        model_runtimes: None,
        tools,
        session_manager: session_manager.clone(),
        fast_model_spec: None,
        permission_bridge: None,
        command_registry: command_registry.clone(),
        skill_manager,
        // Empty search paths keep tests deterministic — only
        // built-ins land in the catalog, so AgentTool's dynamic
        // prompt is reproducible across runs.
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: true,
    })
    .await
    .with_context(|| format!("build SessionRuntime for {provider}/{model_id}"))?;

    // Mirror `run_sdk_mode`: fire SessionStart hooks once at bootstrap
    // so settings.json hook entries surface as `hook_*` reminders on
    // the first turn.
    session_runtime.fire_session_start_hooks("startup").await;

    let bootstrap = Arc::new(
        CliInitializeBootstrap::new("default".to_string()).with_command_registry(command_registry),
    );

    // Wire the in-memory transport pair.
    let (server_end, client_end) = InMemoryTransport::pair(64);
    // Match `run_sdk_mode`'s wiring: install file_history (empty
    // placeholder when the runtime has none) AND `with_session_runtime`
    // BEFORE building the runner. Without `with_session_runtime`, the
    // server's per-turn engine path can't reach the runtime's wired
    // subsystems.
    let file_history_for_server = session_runtime.file_history.clone().unwrap_or_else(|| {
        Arc::new(tokio::sync::RwLock::new(
            coco_context::FileHistoryState::new(),
        ))
    });
    let server = SdkServer::new(server_end)
        .with_session_manager(session_manager)
        .with_initialize_bootstrap(bootstrap)
        .with_file_history(file_history_for_server, std::env::temp_dir())
        .with_session_runtime(session_runtime.clone());

    let runner = Arc::new(QueryEngineRunner::new(
        session_runtime.clone(),
        cli.max_tokens.unwrap_or(2_048),
        cli.max_turns.or(Some(8)),
        Some(system_prompt),
    ));
    server.set_turn_runner(runner).await;

    // Hold an `Arc<SdkServer>` so the harness can peek at server.state()
    // (active SessionHandle) without consuming the server in the spawn.
    let server_arc = Arc::new(server);
    let server_for_task = server_arc.clone();
    let server_task = tokio::spawn(async move {
        let _ = server_for_task.run().await;
    });

    Ok(LiveSdkServer {
        client: client_end,
        server_task,
        server: server_arc,
        _sessions_dir: sessions_dir,
        _cwd_dir: cwd_dir,
        session_runtime,
        _provider: provider_name,
        _model_id: model_id,
    })
}

/// Build a `JsonRpcRequest` envelope.
pub fn req(id: i64, method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest {
        request_id: RequestId::Integer(id),
        method: method.into(),
        params,
    })
}

/// Drive the server until a `JsonRpcResponse` for `request_id` arrives;
/// return `(response_result, accumulated_notifications)`. Times out
/// after `timeout` to keep tests bounded.
pub async fn drive_until_response(
    transport: &Arc<InMemoryTransport>,
    request_id: i64,
    timeout: std::time::Duration,
) -> Result<(serde_json::Value, Vec<JsonRpcNotification>)> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut notifications: Vec<JsonRpcNotification> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!(
                "SDK harness: timed out waiting for response to request {request_id} \
                 (collected {} notifications)",
                notifications.len()
            ));
        }
        let next = tokio::time::timeout(remaining, transport.recv())
            .await
            .map_err(|_| anyhow!("SDK harness: recv timeout"))?
            .map_err(|e| anyhow!("SDK transport error: {e:?}"))?;
        let Some(msg) = next else {
            return Err(anyhow!("SDK harness: transport closed"));
        };
        match msg {
            JsonRpcMessage::Response(resp) => {
                if matches!(resp.request_id, RequestId::Integer(n) if n == request_id) {
                    return Ok((resp.result, notifications));
                }
                // Different id — ignore; out-of-order under cancellation.
            }
            JsonRpcMessage::Error(err) => {
                if matches!(err.request_id, RequestId::Integer(n) if n == request_id) {
                    return Err(anyhow!(
                        "SDK request {request_id} returned error: \
                         code={} message={}",
                        err.code,
                        err.message
                    ));
                }
            }
            JsonRpcMessage::Notification(n) => notifications.push(n),
            JsonRpcMessage::Request(_) => {
                // Server-initiated requests (e.g. permission asks).
                // Protocol tests don't simulate the client's reply side.
            }
        }
    }
}

/// Send `initialize` and return the parsed `InitializeResponse`.
pub async fn send_initialize(
    server: &LiveSdkServer,
) -> Result<(serde_json::Value, Vec<JsonRpcNotification>)> {
    let id = 1;
    server
        .client
        .send(req(
            id,
            ClientRequestMethod::Initialize.as_str(),
            serde_json::json!({}),
        ))
        .await
        .map_err(|e| anyhow!("send initialize: {e:?}"))?;
    drive_until_response(&server.client, id, std::time::Duration::from_secs(20)).await
}

/// Send `session/start` to bootstrap a session. Most SDK control
/// methods (`turn/start`, `control/setPermissionMode`, …) require an
/// active session and reply with `INVALID_REQUEST: no active session`
/// when called before this.
///
/// **Important**: pass the model explicitly. `session/start` defaults
/// the model to `DEFAULT_SDK_MODEL` (Claude Opus) — that becomes the
/// `handoff.model_id` the runner threads into `QueryEngineConfig`,
/// even when the SessionRuntime was built for a different provider.
/// Mismatch silently breaks the turn (engine builds against wrong
/// registry entry, no events fire).
pub async fn send_session_start(
    server: &LiveSdkServer,
) -> Result<(serde_json::Value, Vec<JsonRpcNotification>)> {
    server
        .client
        .send(req(
            100,
            ClientRequestMethod::SessionStart.as_str(),
            serde_json::json!({ "model": &server._model_id }),
        ))
        .await
        .map_err(|e| anyhow!("send session/start: {e:?}"))?;
    drive_until_response(&server.client, 100, std::time::Duration::from_secs(15)).await
}

/// Send `turn/start` with `prompt`, drive until `turn/completed` /
/// `turn/failed` / `turn/interrupted` notification arrives. Returns the
/// `turn/start` response (carries `turn_id`) plus every notification
/// observed up to and including the terminator.
pub async fn send_turn(
    server: &LiveSdkServer,
    request_id: i64,
    prompt: &str,
) -> Result<(serde_json::Value, Vec<JsonRpcNotification>)> {
    server
        .client
        .send(req(
            request_id,
            ClientRequestMethod::TurnStart.as_str(),
            serde_json::json!({ "prompt": prompt }),
        ))
        .await
        .map_err(|e| anyhow!("send turn/start: {e:?}"))?;
    let verbose = std::env::var("COCO_TEST_SDK_VERBOSE").ok().as_deref() == Some("1");
    let (resp, mut notifications) = drive_until_response(
        &server.client,
        request_id,
        std::time::Duration::from_secs(120),
    )
    .await?;
    if verbose {
        eprintln!(
            "[send_turn id={request_id}] turn/start ack received after {} early notifs",
            notifications.len()
        );
        for n in &notifications {
            eprintln!(
                "[send_turn id={request_id}] early notif method={}",
                n.method
            );
        }
    }
    // 360s budget: DeepSeek's reasoning model can spend 200s+ thinking
    // about even trivial prompts before producing a single token of
    // output, then more time on the assistant text.
    let started = tokio::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(360);
    let mut last_recv = started;
    while !notifications
        .iter()
        .any(|n| is_turn_terminal_method(&n.method))
    {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let methods: Vec<&str> = notifications.iter().map(|n| n.method.as_str()).collect();
            return Err(anyhow!(
                "SDK turn: timed out waiting for turn terminal notification \
                 ({} notifications collected, methods={methods:?})",
                notifications.len()
            ));
        }
        let next = tokio::time::timeout(remaining, server.client.recv())
            .await
            .map_err(|_| anyhow!("SDK turn recv timeout"))?
            .map_err(|e| anyhow!("SDK transport error: {e:?}"))?;
        let Some(msg) = next else {
            return Err(anyhow!("SDK transport closed mid-turn"));
        };
        if let JsonRpcMessage::Notification(n) = msg {
            if verbose {
                let now = tokio::time::Instant::now();
                eprintln!(
                    "[send_turn id={request_id} t+{:>5.1}s Δ{:>5.1}s] {}",
                    now.duration_since(started).as_secs_f64(),
                    now.duration_since(last_recv).as_secs_f64(),
                    n.method,
                );
                last_recv = now;
            }
            notifications.push(n);
        }
    }
    Ok((resp, notifications))
}

/// Returns true when the wire-method string is the terminal turn
/// signal (`turn/ended`). Outcome variants (completed / failed /
/// interrupted / max_turns_reached / budget_exhausted) are inside
/// `params.outcome.kind`.
pub fn is_turn_terminal_method(method: &str) -> bool {
    method == NotificationMethod::TurnEnded.as_str()
}
