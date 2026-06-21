//! `RealTuiHarness` — drives the real-LLM TUI path in-process.
//!
//! Construction mirrors `coco_cli::tui_runner::run_tui` step-for-step,
//! using the same public bootstrap helpers (`build_runtime_config_for_cli`,
//! `build_engine_resources`, `SessionRuntime::build`,
//! `install_session_late_binds`, `TuiPermissionBridge`). The only
//! production piece we don't reuse is `App::run` — it owns crossterm raw
//! mode and is incompatible with a programmatic harness — and the
//! private slash-command dispatcher inside `tui_runner.rs`.
//!
//! Lifecycle:
//! 1. `RealTuiHarness::builder().build().await?` — parses argv, builds
//!    the full session runtime, wires the permission bridge, spawns the
//!    driver task.
//! 2. `harness.submit("…").await` — sends `UserCommand::SubmitInput`.
//! 3. `harness.pump_until_idle(timeout).await?` — drains every
//!    `CoreEvent` into `AppState` until the engine emits
//!    `SessionResult` (or `timeout` fires).
//! 4. `harness.render_to_string()?` — render `AppState` through the native
//!    surface into a test buffer for substring assertions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use clap::Parser as _;
use coco_cli::Cli;
use coco_cli::headless::build_runtime_config_for_cli;
use coco_cli::session_bootstrap::EngineResources;
use coco_cli::session_bootstrap::build_engine_resources;
use coco_cli::session_bootstrap::install_session_late_binds;
use coco_cli::session_runtime::SessionRuntime;
use coco_cli::session_runtime::SessionRuntimeBuildOpts;
use coco_cli::tui_permission_bridge::PendingApprovals;
use coco_cli::tui_permission_bridge::TuiPermissionBridge;
use coco_cli::tui_permission_bridge::new_pending_map;
use coco_cli::tui_permission_bridge::resolve_pending;
use coco_session::SessionManager;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tui::AppState;
use coco_tui::TuiCommand;
use coco_tui::UserCommand;
use coco_tui::command::ShutdownReason;
use coco_tui::server_notification_handler::handle_core_event;
use coco_tui::update::handle_command;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TuiOnlyEvent;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::common;

/// Knobs for [`RealTuiHarness::builder`]. Most defaults match the
/// `coco -p` headless convention so a one-shot test reads naturally.
pub struct HarnessConfig {
    /// Provider name as used by `--models.main <provider>/<model>`.
    pub provider: String,
    /// Wire-side model id (e.g. `deepseek-v4-flash`).
    pub model: String,
    /// Cap on the agent-loop turns. 8 is enough for tool-chain tests.
    pub max_turns: i32,
    /// Test terminal size (cells). 120×40 mirrors the production
    /// snapshot default; stays wide enough that tool/chat blocks
    /// don't wrap into the assertion text.
    pub width: u16,
    pub height: u16,
    /// `--dangerously-skip-permissions` — set true so file-writing
    /// tools execute without an Ask round-trip (default for tests
    /// that don't exercise the permission bridge).
    pub bypass_permissions: bool,
    /// `--permission-mode <mode>` (e.g. "default", "acceptEdits", "plan").
    /// `None` lets the lib pick the default.
    pub permission_mode: Option<String>,
    /// Optional path to a settings.json passed via `--settings`. Tests
    /// that need hooks installed via prod settings-merge or a tweaked
    /// session config write a file and pass its path here.
    pub settings_path: Option<PathBuf>,
    /// Pre-minted working directory. The tempdir lifetime ties to the
    /// harness; it backs `--cwd` and project_dir for hermetic tests.
    pub workdir: Option<TempDir>,
    /// Extra raw CLI args appended to the constructed argv. Lets a test
    /// flip flags (e.g. `--system-prompt`) without growing the builder
    /// surface for every flag the harness might need.
    pub extra_argv: Vec<String>,
    /// Optional auto-responses for `AskUserQuestion` tool calls. Map of
    /// question text → answer string. When the engine emits an
    /// `ApprovalRequired` for `AskUserQuestion`, the harness builds an
    /// `answers` payload from this map (defaulting to the first option
    /// for any question not in the map) and ships it via the existing
    /// `UserCommand::ApprovalResponse { updated_input: Some(...) }`
    /// channel — same flow the production TUI uses, no custom wiring.
    /// Empty map = auto-pick first option for every question.
    pub auto_answer_questions: Option<HashMap<String, String>>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek-openai".into(),
            model: "deepseek-v4-flash".into(),
            max_turns: 8,
            width: 120,
            height: 40,
            bypass_permissions: true,
            permission_mode: None,
            settings_path: None,
            workdir: None,
            extra_argv: Vec::new(),
            auto_answer_questions: None,
        }
    }
}

/// Builder for [`RealTuiHarness`]. Methods listed here are used by
/// the in-tree scenarios; future tests can extend the surface by
/// adding more `with_*` helpers.
#[allow(dead_code)] // some setters are reserved for upcoming scenarios
pub struct RealTuiHarnessBuilder {
    cfg: HarnessConfig,
}

#[allow(dead_code)]
impl RealTuiHarnessBuilder {
    pub fn new() -> Self {
        Self {
            cfg: HarnessConfig::default(),
        }
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.cfg.provider = provider.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.cfg.model = model.into();
        self
    }

    pub fn with_max_turns(mut self, n: i32) -> Self {
        self.cfg.max_turns = n;
        self
    }

    pub fn with_size(mut self, width: u16, height: u16) -> Self {
        self.cfg.width = width;
        self.cfg.height = height;
        self
    }

    /// Default is true. Flip to `false` to make the engine consult the
    /// permission bridge on every tool call — used by the
    /// permission-round-trip suites.
    pub fn with_bypass_permissions(mut self, on: bool) -> Self {
        self.cfg.bypass_permissions = on;
        self
    }

    pub fn with_permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.cfg.permission_mode = Some(mode.into());
        self
    }

    pub fn with_settings_path(mut self, path: PathBuf) -> Self {
        self.cfg.settings_path = Some(path);
        self
    }

    pub fn with_workdir(mut self, dir: TempDir) -> Self {
        self.cfg.workdir = Some(dir);
        self
    }

    pub fn with_extra_argv<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.cfg.extra_argv.extend(args.into_iter().map(Into::into));
        self
    }

    /// Enable auto-handling of `AskUserQuestion` approvals. The harness
    /// will intercept `ApprovalRequired { tool_name == "AskUserQuestion" }`
    /// events during `pump_until_*` loops, build a TS-shaped `answers`
    /// payload (looking up explicit answers in `answers` by question
    /// text, defaulting to the question's first option when no answer
    /// is supplied), and send `UserCommand::ApprovalResponse` with
    /// `updated_input: Some({ ...input, answers })` — the same channel
    /// the production TUI overlay uses. Pass an empty map to enable
    /// the "always pick first option" default.
    pub fn with_auto_answer_questions(mut self, answers: HashMap<String, String>) -> Self {
        self.cfg.auto_answer_questions = Some(answers);
        self
    }

    pub async fn build(self) -> Result<RealTuiHarness> {
        RealTuiHarness::build(self.cfg).await
    }
}

/// In-process driver for a real-LLM TUI session. See module docs.
pub struct RealTuiHarness {
    pub state: AppState,
    terminal_width: u16,
    terminal_height: u16,
    pub command_tx: mpsc::Sender<UserCommand>,
    pub event_rx: mpsc::Receiver<CoreEvent>,
    pub events: Vec<CoreEvent>,
    pub workdir: TempDir,
    pub provider: String,
    pub model: String,
    pending_approvals: PendingApprovals,
    /// Per-session subsystems. Tests reach in via [`Self::history_snapshot`]
    /// to read injected reminder attachments.
    runtime: Arc<SessionRuntime>,
    /// Sender side of the `event_rx` we hand out via the harness; held
    /// so future tests can hand additional emitters into engine methods
    /// run outside the driver task.
    #[allow(dead_code)]
    event_tx: mpsc::Sender<CoreEvent>,
    driver_task: Option<JoinHandle<()>>,
    cancel: CancellationToken,
    /// When `Some`, the harness auto-resolves `AskUserQuestion` approvals
    /// inside `pump_until_*` loops by replaying the model's question
    /// list and picking answers from this map (default: first option).
    /// `None` = leave AskUserQuestion approvals to the test (legacy
    /// behavior). Populated by `with_auto_answer_questions`.
    auto_answer_questions: Option<HashMap<String, String>>,
}

impl RealTuiHarness {
    pub fn builder() -> RealTuiHarnessBuilder {
        RealTuiHarnessBuilder::new()
    }

    async fn build(cfg: HarnessConfig) -> Result<Self> {
        crate::common::env::ensure_env_loaded();

        let workdir = match cfg.workdir {
            Some(d) => d,
            None => common::tmpdir::make("coco-tests-tui-real-")
                .with_context(|| "create cwd tempdir under /tmp")?,
        };
        let cwd = workdir.path().to_path_buf();

        // Construct the same Cli the binary parses. `--cwd` pins the
        // session's view; the build helper threads it through
        // RuntimeConfig.paths and into engine config_dir.
        let mut argv: Vec<String> = vec!["coco".into()];
        argv.push("--models.main".into());
        argv.push(format!("{}/{}", cfg.provider, cfg.model));
        argv.push("--max-turns".into());
        argv.push(cfg.max_turns.to_string());
        argv.push("--cwd".into());
        argv.push(cwd.to_string_lossy().into_owned());
        if cfg.bypass_permissions {
            argv.push("--dangerously-skip-permissions".into());
        }
        if let Some(mode) = &cfg.permission_mode {
            argv.push("--permission-mode".into());
            argv.push(mode.clone());
        }
        if let Some(path) = &cfg.settings_path {
            argv.push("--settings".into());
            argv.push(path.to_string_lossy().into_owned());
        }
        argv.extend(cfg.extra_argv);

        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let cli = Cli::parse_from(&argv_refs);

        // Stage 1: layered runtime config (settings.json + env + flags).
        let runtime_config = build_runtime_config_for_cli(&cli, &cwd)
            .with_context(|| "build_runtime_config_for_cli")?;

        // Stage 2: shared engine resources (full ToolRegistry, system
        // prompt with CLAUDE.md, command registry, startup permission
        // state).
        let EngineResources {
            tools,
            system_prompt,
            model_id,
            provider_api: _,
            startup,
            command_registry,
            skill_manager,
            output_style_manager: _,
        } = build_engine_resources(&cli, &runtime_config, &cwd)
            .with_context(|| "build_engine_resources")?;

        // SessionManager — sessions_dir mirrors prod (`~/.coco/sessions`).
        // Tests touch the user's home like the existing coco_cli_deepseek
        // suite does; it's a known minor cost vs full prod-fidelity.
        let session_manager = Arc::new(SessionManager::new(coco_cli::paths::sessions_dir()));
        let _ = session_manager.create(&model_id, &cwd);

        // Channels — same shape `create_channels` produces. Capacity
        // mirrors prod (32 cmd / 256 evt) so back-pressure profiles
        // match.
        let (command_tx, command_rx) = mpsc::channel::<UserCommand>(32);
        let (event_tx, event_rx) = mpsc::channel::<CoreEvent>(256);

        // Permission bridge: production `TuiPermissionBridge` reused
        // verbatim so `PermissionDecision::Ask` flows the same way it
        // does in real interactive sessions.
        let pending_approvals: PendingApprovals = new_pending_map();
        let bridge: ToolPermissionBridgeRef = Arc::new(TuiPermissionBridge::new(
            event_tx.clone(),
            pending_approvals.clone(),
        ));

        // Stage 3: per-session subsystems. SessionRuntime owns hook
        // registry (settings.json + plugins), file-history, ToolAppState,
        // session memory, mailbox, etc.
        let runtime = SessionRuntime::build(SessionRuntimeBuildOpts {
            cli: &cli,
            runtime_config: Arc::new(runtime_config),
            cwd: cwd.clone(),
            model_id: model_id.clone(),
            system_prompt,
            bypass_permissions_available: startup.bypass_available,
            permission_mode: startup.mode,
            model_runtimes: None,
            tools,
            session_manager,
            fast_model_spec: None,
            permission_bridge: Some(bridge),
            command_registry,
            skill_manager,
            agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            session_id_override: None,
            is_non_interactive: false,
        })
        .await
        .with_context(|| "SessionRuntime::build")?;

        // SessionRuntime's engine_config doesn't set `cwd_override` —
        // production TUI/SDK rely on `std::env::current_dir()` for tool
        // context's cwd resolution. Tests run inside cargo so the
        // process cwd points at the workspace root, not the harness's
        // tempdir. Pin cwd_override so tools (Read/Write/Bash) and the
        // nested-memory pipeline (`drain_nested_memory_triggers` reads
        // `ctx.cwd_override` first) all see the test workdir.
        runtime
            .update_engine_config(|c| c.cwd_override = Some(cwd.clone()))
            .await;

        // Stage 4: late-binds shared with TUI/SDK runners (task runtime,
        // transcript store, fork dispatcher, agent-team).
        install_session_late_binds(
            runtime.clone(),
            &cwd,
            /*mcp*/ None,
            /*lsp*/ None,
            /*event_sink*/ Some(event_tx.clone()),
        )
        .await
        .with_context(|| "install_session_late_binds")?;
        // Fire SessionStart hooks to mirror `tui_runner::run_tui` so
        // settings.json hook entries surface as `hook_*` reminders on
        // the first turn.
        runtime.fire_session_start_hooks("startup").await;

        // Spawn the driver. Handles SubmitInput / ApprovalResponse /
        // Interrupt / Shutdown — the subset every real-LLM scenario in
        // this crate uses.
        let cancel = CancellationToken::new();
        let runtime_for_driver = runtime.clone();
        let event_tx_for_driver = event_tx.clone();
        let pending_for_driver = pending_approvals.clone();
        let driver_task = tokio::spawn(run_real_agent_driver(
            command_rx,
            event_tx_for_driver,
            runtime_for_driver,
            pending_for_driver,
        ));

        // AppState seeded the same way `App::new` would (modes, model
        // label) so render output reads the same as production.
        let mut state = AppState::new();
        state.session.permission_mode = startup.mode;
        state.session.bypass_permissions_available = startup.bypass_available;
        state.session.model = model_id.clone();

        Ok(Self {
            state,
            terminal_width: cfg.width,
            terminal_height: cfg.height,
            command_tx,
            event_rx,
            events: Vec::new(),
            workdir,
            provider: cfg.provider.clone(),
            model: cfg.model.clone(),
            pending_approvals,
            runtime,
            event_tx,
            driver_task: Some(driver_task),
            cancel,
            auto_answer_questions: cfg.auto_answer_questions,
        })
    }

    /// Mimic the user typing `text` and pressing Enter. Same path the
    /// production TUI takes for a typed prompt.
    pub async fn submit(&mut self, text: &str) {
        self.state.ui.input.textarea.insert_str(text);
        let _ = handle_command(&mut self.state, TuiCommand::SubmitInput, &self.command_tx).await;
    }

    /// Inspect an event for AskUserQuestion lifecycle and auto-handle
    /// it when [`Self::auto_answer_questions`] is configured. Returns
    /// `true` when the event was consumed (caller should `continue`
    /// the pump loop without surfacing the event). Side effects:
    /// - On `Tui::QuestionAsked { input }`, build a TS-shaped `answers`
    ///   map (lookup question text in the configured map; default to
    ///   first option) and resolve the bridge via
    ///   `UserCommand::ApprovalResponse { updated_input: Some({...input,
    ///   answers}) }` — the same channel the production Question
    ///   overlay uses.
    ///
    /// `Stream::ToolUseQueued` is no longer needed as a buffering
    /// signal: the production bridge now embeds the full input in
    /// `QuestionAsked.input`, so we can build the payload directly.
    ///
    /// Always returns `false` when `auto_answer_questions` is `None`,
    /// so this helper is a no-op for tests that don't opt in.
    async fn maybe_auto_handle_question(&mut self, event: &CoreEvent) -> bool {
        if self.auto_answer_questions.is_none() {
            return false;
        }
        match event {
            CoreEvent::Tui(TuiOnlyEvent::QuestionAsked { request_id, input }) => {
                let answers_overrides = self
                    .auto_answer_questions
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                let updated_input = build_question_answers_payload(Some(input), &answers_overrides);
                let request_id = request_id.clone();
                let _ = resolve_pending(
                    &self.pending_approvals,
                    &request_id,
                    true,
                    None,
                    Vec::new(),
                    Some(updated_input),
                    None,
                    None,
                )
                .await;
                true
            }
            _ => false,
        }
    }

    /// Drain every queued event into `AppState`, returning when the
    /// engine emits `SessionResult` (turn-loop done) or `timeout`
    /// elapses. Returns `true` ⇒ clean run (`is_error=false`),
    /// `false` ⇒ engine flagged an error.
    pub async fn pump_until_idle(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "RealTuiHarness: timed out after {:?} waiting for SessionResult\n  \
                     {}",
                    timeout,
                    diagnose_event_stall(&self.events),
                ));
            }
            let next = match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Some(evt)) => evt,
                Ok(None) => {
                    return Err(anyhow!(
                        "RealTuiHarness: event channel closed before SessionResult\n  \
                         {}",
                        diagnose_event_stall(&self.events),
                    ));
                }
                Err(_) => continue,
            };
            handle_core_event(&mut self.state, next.clone(), &self.command_tx);
            let auto_handled = self.maybe_auto_handle_question(&next).await;
            let is_terminal = matches!(
                &next,
                CoreEvent::Protocol(ServerNotification::SessionResult(_))
            );
            self.events.push(next);
            if auto_handled {
                continue;
            }
            if is_terminal {
                let is_err = self
                    .events
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        CoreEvent::Protocol(ServerNotification::SessionResult(p)) => {
                            Some(p.is_error)
                        }
                        _ => None,
                    })
                    .unwrap_or(false);
                return Ok(!is_err);
            }
        }
    }

    /// Pump until the next `ApprovalRequired` lands. Returns the
    /// request the test must `approve` / `reject`. Errors if the
    /// session ended without one.
    pub async fn pump_until_approval_request(
        &mut self,
        timeout: Duration,
    ) -> Result<ApprovalRequest> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "RealTuiHarness: timed out after {:?} waiting for ApprovalRequired \
                     ({} events drained)",
                    timeout,
                    self.events.len()
                ));
            }
            let next = match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Some(evt)) => evt,
                Ok(None) => {
                    return Err(anyhow!(
                        "RealTuiHarness: event channel closed before ApprovalRequired"
                    ));
                }
                Err(_) => continue,
            };
            handle_core_event(&mut self.state, next.clone(), &self.command_tx);
            // Auto-handle AskUserQuestion approvals when the harness
            // is configured for it — the test would otherwise see the
            // AskUserQuestion ApprovalRequired and treat it as the
            // approval it was waiting for, which it isn't.
            let auto_handled = self.maybe_auto_handle_question(&next).await;
            if matches!(
                &next,
                CoreEvent::Protocol(ServerNotification::SessionResult(_))
            ) {
                self.events.push(next);
                return Err(anyhow!(
                    "RealTuiHarness: SessionResult arrived before ApprovalRequired — \
                     the engine completed without requesting approval. Check that \
                     bypass is OFF and the hook/tool combo triggers an Ask decision."
                ));
            }
            let approval = if auto_handled {
                None
            } else if let CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired {
                request_id,
                tool_name,
                display_input,
                ..
            }) = &next
            {
                Some(ApprovalRequest {
                    request_id: request_id.clone(),
                    tool_name: tool_name.clone(),
                    input_preview: display_input.as_display_str().to_string(),
                })
            } else {
                None
            };
            self.events.push(next);
            if let Some(req) = approval {
                return Ok(req);
            }
        }
    }

    /// Resolve a pending approval as approved (mirrors
    /// `UserCommand::ApprovalResponse` in prod).
    pub async fn approve(&self, request_id: &str) -> bool {
        resolve_pending(
            &self.pending_approvals,
            request_id,
            true,
            None,
            Vec::new(),
            None,
            None,
            None,
        )
        .await
    }

    /// Resolve a pending approval as rejected, with optional feedback
    /// echoed back to the engine as the rejection reason.
    pub async fn reject(&self, request_id: &str, feedback: Option<String>) -> bool {
        resolve_pending(
            &self.pending_approvals,
            request_id,
            false,
            feedback,
            Vec::new(),
            None,
            None,
            None,
        )
        .await
    }

    /// Render `AppState` through the native surface and return the buffer as
    /// newline-separated text for `assert!(s.contains(…))` checks.
    pub fn render_to_string(&mut self) -> Result<String> {
        Ok(coco_tui::testing::render_native_surface_to_string(
            &self.state,
            self.terminal_width,
            self.terminal_height,
        ))
    }

    /// Tool starts seen in the event stream (in order).
    pub fn tool_starts(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Stream(AgentStreamEvent::ToolUseStarted { name, .. }) => {
                    Some(name.as_str())
                }
                _ => None,
            })
            .collect()
    }

    /// `(tool_name, is_error)` for every tool completion event.
    pub fn tool_completions(&self) -> Vec<(&str, bool)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Stream(AgentStreamEvent::ToolUseCompleted {
                    name, is_error, ..
                }) => Some((name.as_str(), *is_error)),
                _ => None,
            })
            .collect()
    }

    /// First `Message::ToolResult` cell whose tool_name matches `name`.
    /// Returns `(output, is_error)`. Used to probe denied / errored
    /// tool calls without going through the deleted legacy projection.
    pub fn find_tool_result(&self, name: &str) -> Option<(String, bool)> {
        use coco_messages::Message;
        use coco_messages::ToolContent;
        use coco_messages::ToolResultContentPart;
        use coco_messages::ToolResultOutput;
        use coco_tui::transcript::CellKind;
        for cell in self.state.session.transcript.cells_for_test() {
            if !matches!(cell.kind, CellKind::ToolResult { .. }) {
                continue;
            }
            let Message::ToolResult(tr) = cell.source.as_ref() else {
                continue;
            };
            let coco_messages::LlmMessage::Tool { content, .. } = &tr.message else {
                continue;
            };
            let part = content.iter().find_map(|p| match p {
                ToolContent::ToolResult(part) => Some(part),
                _ => None,
            });
            let Some(part) = part else { continue };
            if part.tool_name != name {
                continue;
            }
            let output = match &part.output {
                ToolResultOutput::Text { value, .. } => value.clone(),
                ToolResultOutput::Json { value, .. } => value.to_string(),
                ToolResultOutput::Content { value, .. } => value
                    .iter()
                    .filter_map(|p| match p {
                        ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                ToolResultOutput::ErrorText { value, .. } => value.clone(),
                ToolResultOutput::ErrorJson { value, .. } => value.to_string(),
                ToolResultOutput::ExecutionDenied { reason, .. } => {
                    reason.clone().unwrap_or_default()
                }
            };
            return Some((output, tr.is_error));
        }
        None
    }

    /// Aggregate `TokenUsage` over the session — sourced from the
    /// final `SessionResult` notification (engine's own roll-up).
    /// Returns `None` if no `SessionResult` was emitted.
    pub fn session_total_usage(&self) -> Option<coco_types::TokenUsage> {
        self.events.iter().rev().find_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::SessionResult(p)) => Some(p.usage),
            _ => None,
        })
    }

    /// Path inside the harness workdir.
    pub fn workdir(&self) -> PathBuf {
        self.workdir.path().to_path_buf()
    }

    /// Send `UserCommand::Interrupt` — same path Esc/Ctrl+C takes in
    /// production. The driver fires the active turn's cancel token.
    pub async fn interrupt(&self) -> Result<()> {
        self.command_tx
            .send(UserCommand::Interrupt(
                coco_types::TurnAbortReason::UserCancel,
            ))
            .await
            .map_err(|e| anyhow!("send Interrupt: {e}"))
    }

    /// Snapshot the current `runtime.history` — clones the locked Vec
    /// so callers can run reminder helpers without holding the mutex.
    /// Reminders inject as `Message::Attachment` entries here and are
    /// not surfaced via the wire-protocol notification stream.
    pub async fn history_snapshot(&self) -> Vec<coco_messages::Message> {
        self.runtime
            .history
            .lock()
            .await
            .iter()
            .map(|a| (**a).clone())
            .collect()
    }

    /// The engine's response text, accumulated from streaming `TextDelta`
    /// events. This is what the user actually saw rendered into the
    /// chat scroll. Useful when the answer matters more than the tool
    /// trace.
    pub fn assistant_text(&self) -> String {
        let mut out = String::new();
        for e in &self.events {
            if let CoreEvent::Stream(AgentStreamEvent::TextDelta { delta, .. }) = e {
                out.push_str(delta);
            }
        }
        out
    }

    pub async fn shutdown(mut self) {
        let _ = self
            .command_tx
            .send(UserCommand::Shutdown {
                reason: ShutdownReason::ImmediateQuit,
            })
            .await;
        self.cancel.cancel();
        if let Some(handle) = self.driver_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
        }
    }
}

impl Drop for RealTuiHarness {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.driver_task.take() {
            handle.abort();
        }
        // Snapshot usage into the live-tests aggregator so the
        // alphabetically-last `zzz_emit_*` test can write a report.
        // Best-effort: missing data ⇒ skipped; works for both clean
        // shutdowns and panicked tests.
        if let Some(usage) = self.session_total_usage() {
            // LLM call count comes from the cost tracker carried in
            // the SessionResult — for now infer from the number of
            // `TurnEnded(Completed)` notifications the engine emitted.
            let llm_calls = self
                .events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        CoreEvent::Protocol(ServerNotification::TurnEnded(p))
                            if matches!(p.outcome, coco_types::TurnOutcome::Completed(_))
                    )
                })
                .count() as u64;
            common::usage_report::record_with_llm_calls(
                &self.provider,
                &self.model,
                "tui_real",
                &usage,
                llm_calls.max(1),
            );
        }
    }
}

/// Drive the real engine for the subset of `UserCommand` variants this
/// harness exercises. Stripped from `coco_cli::tui_runner::run_agent_driver`
/// — slash interception, `/clear`, `/compact`, rewind, and plan-approval
/// are private to that file or have dedicated coverage elsewhere.
async fn run_real_agent_driver(
    mut command_rx: mpsc::Receiver<UserCommand>,
    event_tx: mpsc::Sender<CoreEvent>,
    runtime: Arc<SessionRuntime>,
    pending_approvals: PendingApprovals,
) {
    // Active turn slot: `Interrupt` reads this to fire the per-turn
    // cancel token. Mirrors `tui_runner::ActiveTurn`.
    struct ActiveTurn {
        cancel: CancellationToken,
        task: JoinHandle<()>,
    }
    let active_turn: Arc<Mutex<Option<ActiveTurn>>> = Arc::new(Mutex::new(None));

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            UserCommand::SubmitInput {
                user_message_id,
                content,
                ..
            } => {
                if content.is_empty() {
                    continue;
                }

                // Defensive last-write-wins: cancel + await the prior
                // turn before starting a new one.
                {
                    let mut g = active_turn.lock().await;
                    if let Some(prev) = g.take() {
                        prev.cancel.cancel();
                        let _ = tokio::time::timeout(Duration::from_secs(2), prev.task).await;
                    }
                }

                let turn_cancel = CancellationToken::new();
                let cancel_for_state = turn_cancel.clone();
                let runtime_t = runtime.clone();
                let event_tx_t = event_tx.clone();

                // Mint user UUID like prod would (used by file-history /
                // rewind keys; we don't exercise those here, but keep the
                // shape).
                let user_uuid = uuid::Uuid::parse_str(&user_message_id)
                    .unwrap_or_else(|_| uuid::Uuid::new_v4());

                let task = tokio::spawn(async move {
                    // Append the user message to runtime.history before
                    // building the engine — same order as
                    // `process_submit_turn`.
                    let new_msgs = build_user_turn_messages(user_uuid, &content);
                    let messages: Vec<std::sync::Arc<coco_messages::Message>> = {
                        let mut h = runtime_t.history.lock().await;
                        for m in new_msgs.iter().cloned() {
                            h.push(m);
                        }
                        h.to_vec()
                    };

                    let engine = runtime_t.build_engine(turn_cancel.clone()).await;

                    // Forward engine events into the harness channel.
                    let (core_event_tx, mut core_event_rx) = mpsc::channel::<CoreEvent>(256);
                    let event_tx_clone = event_tx_t.clone();
                    let forward = tokio::spawn(async move {
                        while let Some(ev) = core_event_rx.recv().await {
                            let _ = event_tx_clone.send(ev).await;
                        }
                    });

                    let harness_cycle_id = coco_types::TurnId::generate();
                    match engine
                        .run_with_messages(messages, core_event_tx, harness_cycle_id.clone())
                        .await
                    {
                        Ok(result) => {
                            let mut h = runtime_t.history.lock().await;
                            h.clear();
                            for arc in result.final_messages {
                                h.push_arc(arc);
                            }
                        }
                        Err(e) => {
                            // No harness-side `TurnEnded(Failed)` emit:
                            // `engine_session::run_internal_with_messages`
                            // is the sole `Failed` emitter on the engine
                            // Err path (it has the partial usage + typed
                            // error code we'd otherwise have to fabricate).
                            // Re-emitting here produced a double-Failed
                            // terminator on every error path before the
                            // refactor.
                            tracing::warn!(
                                error = %e,
                                cycle_turn_id = %harness_cycle_id,
                                "tui_real harness: engine returned Err"
                            );
                        }
                    }
                    let _ = forward.await;
                });

                *active_turn.lock().await = Some(ActiveTurn {
                    task,
                    cancel: cancel_for_state,
                });
            }

            UserCommand::ApprovalResponse {
                request_id,
                approved,
                feedback,
                updated_input,
                ..
            } => {
                let _ = resolve_pending(
                    &pending_approvals,
                    &request_id,
                    approved,
                    feedback,
                    Vec::new(),
                    updated_input,
                    None,
                    None,
                )
                .await;
            }

            UserCommand::Interrupt(_) => {
                if let Some(state) = active_turn.lock().await.as_ref() {
                    state.cancel.cancel();
                }
            }

            UserCommand::Shutdown { .. } => {
                // Drain the active turn so the harness sees its
                // SessionResult before we exit.
                let mut g = active_turn.lock().await;
                if let Some(prev) = g.take() {
                    prev.cancel.cancel();
                    let _ = tokio::time::timeout(Duration::from_secs(2), prev.task).await;
                }
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::SessionEnded(
                        coco_types::SessionEndedParams {
                            reason: "Test shutdown".into(),
                        },
                    )))
                    .await;
                break;
            }

            // Other variants intentionally unhandled — see harness module
            // docs. The handful we don't model (slash-command palette,
            // rewind, plan approval, …) are covered elsewhere.
            _ => {}
        }
    }

    // Final drain so we don't leak a JoinHandle on dropped channel.
    let mut g = active_turn.lock().await;
    if let Some(prev) = g.take() {
        prev.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), prev.task).await;
    }
}

/// Build the user-side messages for a single submit. Mirrors what
/// `tui_runner::build_turn_messages_with_uuid` does sans
/// @-mention/attachment handling — none of these tests use mentions.
fn build_user_turn_messages(uuid: uuid::Uuid, content: &str) -> Vec<coco_messages::Message> {
    use coco_messages::Message;
    use coco_messages::UserMessage;
    vec![Message::User(UserMessage {
        message: coco_messages::LlmMessage::user_text(content),
        uuid,
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })]
}

/// What [`RealTuiHarness::pump_until_approval_request`] returns.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub input_preview: String,
}

/// Build the `updated_input` payload spliced into an `AskUserQuestion`
/// approval. Mirrors the TS `AskUserQuestionPermissionRequest.tsx`
/// flow: clone the original input, add an `answers: { question_text →
/// answer_string }` map, leave everything else untouched.
///
/// `original_input` is the structured tool input captured from
/// `Stream::ToolUseQueued`; `overrides` is the explicit map the test
/// passed via `with_auto_answer_questions`. For each question the
/// model asked, the override wins; otherwise we fall back to the
/// question's first option (the first option).
///
/// Returns a `serde_json::Value` ready to ship as
/// `UserCommand::ApprovalResponse.updated_input`. Returns the bare
/// `{"answers": {}}` payload if `original_input` is missing — keeps
/// the harness usable even if the lifecycle ordering surprises us.
fn build_question_answers_payload(
    original_input: Option<&serde_json::Value>,
    overrides: &HashMap<String, String>,
) -> serde_json::Value {
    let mut answers = serde_json::Map::new();
    if let Some(input) = original_input
        && let Some(questions) = input.get("questions").and_then(|v| v.as_array())
    {
        for q in questions {
            let Some(question_text) = q.get("question").and_then(|v| v.as_str()) else {
                continue;
            };
            let answer = overrides.get(question_text).cloned().or_else(|| {
                q.get("options")
                    .and_then(|v| v.as_array())
                    .and_then(|opts| opts.first())
                    .and_then(|opt| opt.get("label"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
            if let Some(answer) = answer {
                answers.insert(question_text.to_string(), serde_json::Value::String(answer));
            }
        }
    }
    let mut updated = match original_input.cloned() {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    updated.insert("answers".into(), serde_json::Value::Object(answers));
    serde_json::Value::Object(updated)
}

fn event_summary(evt: &CoreEvent) -> String {
    match evt {
        CoreEvent::Protocol(n) => format!("Protocol::{:?}", n.method()),
        CoreEvent::Stream(s) => match s {
            AgentStreamEvent::TextDelta { .. } => "Stream::TextDelta".into(),
            AgentStreamEvent::ThinkingDelta { .. } => "Stream::ThinkingDelta".into(),
            AgentStreamEvent::ToolUseQueued { name, .. } => {
                format!("Stream::ToolUseQueued({name})")
            }
            AgentStreamEvent::ToolUseStarted { name, .. } => {
                format!("Stream::ToolUseStarted({name})")
            }
            AgentStreamEvent::ToolUseCompleted { name, is_error, .. } => {
                format!("Stream::ToolUseCompleted({name}, err={is_error})")
            }
            AgentStreamEvent::McpToolCallBegin { .. } => "Stream::McpToolCallBegin".into(),
            AgentStreamEvent::McpToolCallEnd { .. } => "Stream::McpToolCallEnd".into(),
        },
        CoreEvent::Tui(_) => "Tui".into(),
    }
}

/// Build a multi-line diagnostic for stuck `pump_until_*` loops.
///
/// Includes (a) total event count, (b) a histogram of event variants
/// sorted by frequency, and (c) the last 20 events in chronological
/// order. The histogram surfaces "model kept emitting deltas but never
/// finished a turn" patterns; the tail captures whether tool-call
/// completion + post-approval Resume events arrived.
fn diagnose_event_stall(events: &[CoreEvent]) -> String {
    use std::collections::BTreeMap;

    let mut histogram: BTreeMap<String, usize> = BTreeMap::new();
    for e in events {
        *histogram.entry(event_summary(e)).or_insert(0) += 1;
    }
    let mut sorted: Vec<(&String, &usize)> = histogram.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    let hist_lines: Vec<String> = sorted
        .iter()
        .map(|(k, v)| format!("    {v:>4} × {k}"))
        .collect();

    let tail_n = 20.min(events.len());
    let tail_start = events.len() - tail_n;
    let tail_lines: Vec<String> = events[tail_start..]
        .iter()
        .enumerate()
        .map(|(i, e)| format!("    [{:>4}] {}", tail_start + i, event_summary(e)))
        .collect();

    format!(
        "diagnostics:\n  events_drained = {}\n  histogram (variant × count, descending):\n{}\n  \
         tail (last {} events):\n{}",
        events.len(),
        hist_lines.join("\n"),
        tail_n,
        tail_lines.join("\n"),
    )
}
