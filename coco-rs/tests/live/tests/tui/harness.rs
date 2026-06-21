//! `TuiHarness` — in-process TUI driver.
//!
//! What this is:
//! - A real `coco_tui::AppState` (the TUI's TEA model)
//! - A real `coco_query::QueryEngine` driven by a `ScriptedModel`
//! - Real `coco_tools` builtin tools (Bash / Read / Write / Edit / Glob)
//! - A real `coco_hooks::HookRegistry` (caller decides whether to install
//!   any hook definitions)
//! - Native-surface test rendering that captures the visible terminal buffer
//!   so tests can assert on what the user would see
//!
//! What this is **not**:
//! - It does not call `coco_tui::App::run`. `App::run` opens a crossterm
//!   `EventStream` that owns stdin in raw mode — incompatible with a test
//!   harness that needs to inject events programmatically. Instead, the
//!   harness runs the same three building blocks `App::run` orchestrates
//!   (`handle_core_event` for engine→TUI, `update::handle_command` /
//!   `keybinding_bridge::map_key` for keystrokes, and the native surface
//!   test renderer for the view) but drives them on its own clock. The
//!   pipeline under test is identical; only the I/O edges are stubbed.
//!
//! Lifecycle:
//! 1. `TuiHarness::builder().build()` — wires channels, spawns the agent
//!    driver task, and returns a ready-to-drive harness.
//! 2. `harness.submit("…")` — pushes a user message into AppState and
//!    sends `UserCommand::SubmitInput` on the command channel.
//! 3. `harness.pump_until_idle(timeout)` — drains every `CoreEvent` the
//!    engine emits into AppState until the engine signals
//!    `SessionResult` (or the timeout fires, which surfaces as an error).
//! 4. `harness.render_to_string()` — paints AppState through the native
//!    surface into a buffer and returns the screen as a newline-separated
//!    string for substring assertions.
//! 5. `harness.shutdown()` (drop runs the same path) — closes the
//!    command channel so the driver task exits cleanly.
//!
//! Hermetic by construction: each harness owns a `tempfile::TempDir`
//! that backs the engine's cwd / project_dir, so any tool that writes to
//! disk lands inside `/tmp/coco-tests-tui-<rand>/...` and is cleaned up
//! when the harness drops.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use coco_hooks::HookDefinition;
use coco_hooks::HookRegistry;
use coco_inference::LanguageModel;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::ToolRegistry;
use coco_tui::AppState;
use coco_tui::TuiCommand;
use coco_tui::UserCommand;
use coco_tui::keybinding_bridge;
use coco_tui::server_notification_handler::handle_core_event;
use coco_tui::update::handle_command;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::Features;
use coco_types::PermissionMode;
use coco_types::ServerNotification;
use coco_types::ToolOverrides;
use coco_types::TuiOnlyEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::common;
use crate::tui::scripted_model::ScriptedModel;

/// Builds a model-spawnable [`AgentHandle`](coco_tool_runtime::AgentHandle)
/// from the harness's own scripted model + tool registry. Installed via
/// [`TuiHarnessBuilder::with_agent_handle_factory`]; lets the
/// agent-team suite spawn a real child `QueryEngine` that shares the
/// parent's deterministic model. `FnOnce` because the harness builds the
/// handle exactly once, after the registry exists.
type AgentHandleFactory = Box<
    dyn FnOnce(Arc<dyn LanguageModel>, Arc<ToolRegistry>) -> Arc<dyn coco_tool_runtime::AgentHandle>
        + Send,
>;

/// Configuration knobs for [`TuiHarness::builder`]. Keep these tight
/// to the testable surface — anything bigger belongs in production
/// `QueryEngineConfig` plumbing, not in tests.
pub struct HarnessConfig {
    /// Width of the synthetic terminal in cells. 120×40 mirrors the
    /// default `coco-tui` snapshot size — wide enough that tool blocks
    /// and chat messages don't wrap surprises into the assertions.
    pub width: u16,
    pub height: u16,
    /// Engine permission mode. Tests default to `BypassPermissions` so
    /// file-writing tools execute without an Ask round-trip; flip to
    /// `Default` to assert the permission-prompt overlay appears.
    pub permission_mode: PermissionMode,
    /// Engine ceiling on agent loop turns. 8 is enough for the multi-
    /// turn / tool-chain suites without giving an underspecified
    /// `ScriptedModel` queue room to spin forever. `None` = unbounded.
    pub max_turns: Option<i32>,
    /// Hooks installed before the engine is built. The harness owns
    /// the `HookRegistry`; production code paths fire through the
    /// engine's `Option<Arc<HookRegistry>>` field.
    pub hooks: Vec<HookDefinition>,
    /// Pre-scripted assistant replies. Pop-front per `do_generate` /
    /// `do_stream` call. See [`ScriptedModel`] for the shape.
    pub replies: Vec<crate::tui::scripted_model::Reply>,
    /// Pre-minted working directory. When `Some`, the harness uses
    /// this path as the engine's cwd / project_dir instead of minting
    /// a fresh tempdir. Tests that need to bake an absolute path into
    /// a `Reply::tool_call` (e.g. tool_chain's `Write` example) pass
    /// the dir here so they can compute the path *before* the harness
    /// boots. Ownership of cleanup transfers to the harness.
    pub workdir: Option<TempDir>,
    /// When set, the harness builds an `AgentHandle` from this factory,
    /// installs it on the engine, and registers the `Agent` tool — so the
    /// scripted model can spawn a real child `QueryEngine`. `None` keeps
    /// the default `NoOpAgentHandle` (spawn returns an error).
    pub agent_handle_factory: Option<AgentHandleFactory>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            width: 120,
            height: 40,
            permission_mode: PermissionMode::BypassPermissions,
            max_turns: Some(8),
            hooks: Vec::new(),
            replies: Vec::new(),
            workdir: None,
            agent_handle_factory: None,
        }
    }
}

/// Builder for [`TuiHarness`]. Set the canned replies / hooks / mode you
/// need, then `build()`.
pub struct TuiHarnessBuilder {
    cfg: HarnessConfig,
}

impl TuiHarnessBuilder {
    pub fn new() -> Self {
        Self {
            cfg: HarnessConfig::default(),
        }
    }

    pub fn with_replies(
        mut self,
        replies: impl IntoIterator<Item = crate::tui::scripted_model::Reply>,
    ) -> Self {
        self.cfg.replies = replies.into_iter().collect();
        self
    }

    pub fn with_hooks(mut self, hooks: impl IntoIterator<Item = HookDefinition>) -> Self {
        self.cfg.hooks = hooks.into_iter().collect();
        self
    }

    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.cfg.permission_mode = mode;
        self
    }

    pub fn with_max_turns(mut self, max_turns: i32) -> Self {
        self.cfg.max_turns = Some(max_turns);
        self
    }

    pub fn with_size(mut self, width: u16, height: u16) -> Self {
        self.cfg.width = width;
        self.cfg.height = height;
        self
    }

    /// Use a pre-minted tempdir as the engine's cwd. Lets a test compute
    /// an absolute path *before* the harness boots so it can bake that
    /// path into a `Reply::tool_call` input.
    pub fn with_workdir(mut self, dir: TempDir) -> Self {
        self.cfg.workdir = Some(dir);
        self
    }

    /// Install a model-spawnable `AgentHandle` and register the `Agent`
    /// tool. The factory receives the harness's scripted model (as
    /// `Arc<dyn LanguageModel>`) and tool registry so the handle can spawn
    /// child `QueryEngine`s that share the same deterministic model — the
    /// seam the agent-team e2e drives instead of the default
    /// `NoOpAgentHandle`.
    pub fn with_agent_handle_factory(
        mut self,
        factory: impl FnOnce(
            Arc<dyn LanguageModel>,
            Arc<ToolRegistry>,
        ) -> Arc<dyn coco_tool_runtime::AgentHandle>
        + Send
        + 'static,
    ) -> Self {
        self.cfg.agent_handle_factory = Some(Box::new(factory));
        self
    }

    pub async fn build(self) -> Result<TuiHarness> {
        TuiHarness::build(self.cfg).await
    }
}

/// In-process TUI under test. See module docs.
pub struct TuiHarness {
    pub state: AppState,
    terminal_width: u16,
    terminal_height: u16,
    /// Sender into the agent driver task. Tests usually go through
    /// [`Self::submit`] / [`Self::press_key`] rather than touching this
    /// directly.
    pub command_tx: mpsc::Sender<UserCommand>,
    pub event_rx: mpsc::Receiver<CoreEvent>,
    /// Every `CoreEvent` the engine emitted, in order. Populated by
    /// [`Self::pump_until_idle`] as it folds events into AppState.
    pub events: Vec<CoreEvent>,
    /// Working directory for the engine. Owned so any tool-written
    /// files survive long enough for assertions, then cleaned on drop.
    pub workdir: TempDir,
    /// Reference to the ScriptedModel so tests can read `call_count`.
    pub model: Arc<ScriptedModel>,
    /// Pending permission approvals — request_id → oneshot::Sender.
    /// Same shape production uses for the TUI ↔ engine approval
    /// round-trip. Tests resolve via [`Self::approve`] / [`Self::reject`]
    /// once they observe an `ApprovalRequired` event.
    pending_approvals: coco_cli::tui_permission_bridge::PendingApprovals,
    /// Engine handle exposed for tests that drive engine methods directly
    /// (e.g. `/compact` calls `run_manual_compact`). The driver task holds
    /// its own clone — both refer to the same engine instance.
    engine: Arc<QueryEngine>,
    /// Cloneable sender into the same `event_rx` the driver uses. Tests
    /// running engine methods outside the driver pass this clone so
    /// events still flow into AppState via [`Self::pump_until_idle`] /
    /// [`Self::drain_pending_events`].
    event_tx: mpsc::Sender<CoreEvent>,
    driver_task: Option<JoinHandle<()>>,
    cancel: CancellationToken,
}

impl TuiHarness {
    pub fn builder() -> TuiHarnessBuilder {
        TuiHarnessBuilder::new()
    }

    async fn build(mut cfg: HarnessConfig) -> Result<Self> {
        let workdir = match cfg.workdir {
            Some(dir) => dir,
            None => common::tmpdir::make("coco-tests-tui-")
                .with_context(|| "create cwd tempdir under /tmp")?,
        };
        let workdir_path = workdir.path().to_path_buf();

        // Engine plumbing: ScriptedModel → ModelRuntimeRegistry → QueryEngine.
        let model = ScriptedModel::new(cfg.replies);
        let model_runtimes = coco_query::test_support::model_runtime_registry(
            model.clone() as Arc<dyn LanguageModel>
        );

        // Tool registry: the same curated subset the cli/sdk_server live
        // suites use. Keeps the harness focused on agent-loop + TUI
        // behavior; anything wider is covered by `coco-tools` per-tool
        // tests.
        let tool_registry = ToolRegistry::new();
        tool_registry.register(Arc::new(coco_tools::BashTool));
        tool_registry.register(Arc::new(coco_tools::ReadTool));
        tool_registry.register(Arc::new(coco_tools::WriteTool));
        tool_registry.register(Arc::new(coco_tools::EditTool));
        tool_registry.register(Arc::new(coco_tools::GlobTool));
        // Expose the `Agent` tool only when a handle is installed, so the
        // tool list stays identical for every other suite.
        if cfg.agent_handle_factory.is_some() {
            tool_registry.register(Arc::new(coco_tools::AgentTool));
        }
        let tools = Arc::new(tool_registry);

        // Build the model-spawnable agent handle (if any) from the
        // harness's own scripted model + tool registry, so spawned
        // children share the same deterministic model and tools.
        let agent_handle = cfg
            .agent_handle_factory
            .take()
            .map(|f| f(model.clone() as Arc<dyn LanguageModel>, tools.clone()));

        // Hook registry: install caller-supplied definitions. Empty by
        // default — most tests don't need hooks.
        let hooks = if cfg.hooks.is_empty() {
            None
        } else {
            let reg = HookRegistry::new();
            for h in cfg.hooks {
                reg.register(h);
            }
            Some(Arc::new(reg))
        };

        let cancel = CancellationToken::new();
        let engine_cfg = QueryEngineConfig {
            model_id: model.model_id().to_string(),
            permission_mode: cfg.permission_mode,
            bypass_permissions_available: matches!(
                cfg.permission_mode,
                PermissionMode::BypassPermissions
            ),
            context_window: 200_000,
            max_output_tokens: 2_048,
            max_turns: cfg.max_turns,
            total_token_budget: None,
            system_prompt: Some("You are a test scripted model.".into()),
            is_non_interactive: true,
            project_dir: Some(workdir_path.clone()),
            cwd_override: Some(workdir_path.clone()),
            features: Arc::new(Features::with_defaults()),
            tool_overrides: Arc::new(ToolOverrides::none()),
            ..QueryEngineConfig::default()
        };

        // Channels — same shapes `coco_tui::create_channels` produces,
        // sized for the harness rather than a real TUI session.
        let (command_tx, command_rx) = mpsc::channel::<UserCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<CoreEvent>(512);

        // Permission bridge: production `TuiPermissionBridge` reused
        // verbatim — it surfaces `ApprovalRequired` over the same
        // `event_tx` the engine uses, and awaits a oneshot the test
        // resolves via `approve()` / `reject()`. Wiring this even when
        // the test runs in BypassPermissions is harmless — the engine
        // only consults the bridge on `PermissionDecision::Ask`.
        let pending_approvals = coco_cli::tui_permission_bridge::new_pending_map();
        let bridge: Arc<dyn coco_tool_runtime::ToolPermissionBridge> =
            Arc::new(coco_cli::tui_permission_bridge::TuiPermissionBridge::new(
                event_tx.clone(),
                pending_approvals.clone(),
            ));
        let engine = QueryEngine::new(engine_cfg, model_runtimes, tools, cancel.clone(), hooks)
            .with_permission_bridge(bridge);
        // Install the model-spawnable agent handle when the test provided
        // one — replaces the default `NoOpAgentHandle` so `Agent`-tool
        // spawns run a real child engine instead of returning an error.
        let engine = match agent_handle {
            Some(handle) => engine.with_agent_handle(handle),
            None => engine,
        };
        let engine = Arc::new(engine);
        let engine_for_driver = engine.clone();
        let event_tx_for_driver = event_tx.clone();

        // Spawn the agent driver: a stripped-down version of
        // `app/cli/src/tui_runner.rs::run_agent_driver`. Production
        // dispatcher handles ~15 UserCommand variants for slash commands,
        // permissions, rewind, etc. — none of those code paths are under
        // test here, so the harness only handles `SubmitInput` (the path
        // every conversational test exercises) and `Shutdown` (so drop
        // can join cleanly).
        let driver_task = tokio::spawn(run_test_agent_driver(
            engine_for_driver,
            command_rx,
            event_tx_for_driver,
        ));

        // AppState starts empty — production fills it via `app.state_mut()`
        // post-`new`; we don't need any of that bootstrapping for these
        // scenarios.
        let mut state = AppState::new();
        state.session.permission_mode = cfg.permission_mode;
        state.session.bypass_permissions_available =
            matches!(cfg.permission_mode, PermissionMode::BypassPermissions);
        state.session.model = "scripted-model".to_string();

        Ok(Self {
            state,
            terminal_width: cfg.width,
            terminal_height: cfg.height,
            command_tx,
            event_rx,
            events: Vec::new(),
            workdir,
            model,
            pending_approvals,
            engine,
            event_tx,
            driver_task: Some(driver_task),
            cancel,
        })
    }

    /// Mimic the user typing `text` and pressing Enter. Matches what
    /// `update::edit::submit` does in production — sets the input
    /// buffer and sends `UserCommand::SubmitInput` on the command
    /// channel; the engine echoes a `Message::User` back via
    /// `MessageAppended` which the transcript view picks up.
    pub async fn submit(&mut self, text: &str) {
        // Set the input buffer so `TuiCommand::SubmitInput` picks it up.
        self.state.ui.input.textarea.insert_str(text);
        let _ = handle_command(&mut self.state, TuiCommand::SubmitInput, &self.command_tx).await;
    }

    /// Synthesize a key event the way `App::handle_event` would —
    /// crossterm `KeyEvent` → `keybinding_bridge::map_key` →
    /// `update::handle_command`. Returns `true` if the keystroke produced
    /// a state change. Tests use this to verify dispatcher routing
    /// without piping through the real crossterm `EventStream`.
    pub async fn press_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let key = KeyEvent::new(code, modifiers);
        if let Some(cmd) = keybinding_bridge::map_key(&self.state, key) {
            handle_command(&mut self.state, cmd, &self.command_tx).await
        } else {
            false
        }
    }

    /// Drain every queued event into AppState, returning when the engine
    /// emits a `SessionResult` (turn-loop done) or `timeout` elapses.
    /// `SessionResult` is the canonical "engine is idle" signal; the
    /// production TUI uses it as the cue to re-enable input.
    ///
    /// Returns the `SessionResult.is_error` flag — `false` ⇒ clean run,
    /// `true` ⇒ engine flagged an error in the result envelope.
    pub async fn pump_until_idle(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "TuiHarness: timed out after {:?} waiting for SessionResult \
                     ({} events drained, last={:?})",
                    timeout,
                    self.events.len(),
                    self.events.last().map(event_summary)
                ));
            }
            let next = match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Some(evt)) => evt,
                Ok(None) => {
                    return Err(anyhow!(
                        "TuiHarness: event channel closed before SessionResult"
                    ));
                }
                Err(_) => continue, // outer-loop guard handles deadline
            };
            // Fold into AppState so render_to_string reflects the
            // post-event view. Then stash for assertion-side
            // introspection.
            handle_core_event(&mut self.state, next.clone(), &self.command_tx);
            let is_terminal = matches!(
                &next,
                CoreEvent::Protocol(ServerNotification::SessionResult(_))
            );
            self.events.push(next);
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

    /// Render AppState through the native surface and return the buffer as a
    /// newline-separated string. Suitable for `assert!(s.contains(...))`
    /// checks. Whitespace at end-of-line is preserved verbatim.
    pub fn render_to_string(&mut self) -> Result<String> {
        Ok(coco_tui::testing::render_native_surface_to_string(
            &self.state,
            self.terminal_width,
            self.terminal_height,
        ))
    }

    /// Convenience: every tool-name that started executing during the
    /// session, in order. Mirrors `cli::events::tool_uses_started`.
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

    /// Convenience: `(tool_name, is_error)` for every completed tool.
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

    /// Number of cells in the engine-authoritative transcript.
    pub fn cell_count(&self) -> usize {
        self.state.session.transcript.cells_for_test().len()
    }

    /// True iff the transcript has no cells.
    pub fn cells_empty(&self) -> bool {
        self.state.session.transcript.is_empty()
    }

    /// True iff any `UserText` cell exists. Synthetic XML wrappers
    /// (`<local-command-stdout>` etc.) still count.
    pub fn has_user_cell(&self) -> bool {
        use coco_tui::transcript::CellKind;
        self.state
            .session
            .transcript
            .cells_for_test()
            .iter()
            .any(|c| matches!(c.kind, CellKind::UserText { .. }))
    }

    /// True iff any `AssistantText` cell exists.
    pub fn has_assistant_text_cell(&self) -> bool {
        use coco_tui::transcript::CellKind;
        self.state
            .session
            .transcript
            .cells_for_test()
            .iter()
            .any(|c| matches!(c.kind, CellKind::AssistantText { .. }))
    }

    /// Find the first `Message::ToolResult` cell whose `tool_name`
    /// matches `name`. Returns `(output, is_error)` extracted from the
    /// wrapped `LlmMessage::Tool` content.
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

    /// Count of `Message::ToolResult` cells whose `tool_name` matches `name`.
    pub fn tool_result_count(&self, name: &str) -> usize {
        use coco_messages::Message;
        use coco_messages::ToolContent;
        use coco_tui::transcript::CellKind;
        self.state
            .session
            .transcript
            .cells_for_test()
            .iter()
            .filter(|cell| {
                if !matches!(cell.kind, CellKind::ToolResult { .. }) {
                    return false;
                }
                let Message::ToolResult(tr) = cell.source.as_ref() else {
                    return false;
                };
                let coco_messages::LlmMessage::Tool { content, .. } = &tr.message else {
                    return false;
                };
                content.iter().any(|p| match p {
                    ToolContent::ToolResult(part) => part.tool_name == name,
                    _ => false,
                })
            })
            .count()
    }

    /// True iff any `AssistantText` cell's body contains `needle`.
    pub fn assistant_text_contains(&self, needle: &str) -> bool {
        use coco_tui::transcript::CellKind;
        self.state.session.transcript.cells_for_test().iter().any(
            |c| matches!(&c.kind, CellKind::AssistantText { text, .. } if text.contains(needle)),
        )
    }

    /// `(role, text)` for every user/assistant text cell, in transcript
    /// order. `role` is `"user"` or `"assistant"`. Tool cells, system
    /// cells, and reasoning cells are skipped.
    pub fn text_cells_in_order(&self) -> Vec<(&'static str, &str)> {
        use coco_tui::transcript::CellKind;
        self.state
            .session
            .transcript
            .cells_for_test()
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::UserText { text } => Some(("user", text.as_str())),
                CellKind::AssistantText { text, .. } => Some(("assistant", text.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Path inside the harness workdir — useful for hooks that need
    /// to write a trace file outside the engine's view.
    pub fn workdir(&self) -> PathBuf {
        self.workdir.path().to_path_buf()
    }

    /// Drain the engine's event stream until the next
    /// `TuiOnlyEvent::ApprovalRequired` arrives, folding intermediate
    /// events into AppState along the way (so render snapshots taken
    /// after this call reflect the full mid-turn state).
    ///
    /// Returns the `(request_id, tool_name)` of the request so the
    /// test can route an `approve` / `reject` to it. If `SessionResult`
    /// arrives first (engine finished without ever asking), returns
    /// an error — that's a test setup bug (e.g. mode/tool combo didn't
    /// trigger an Ask).
    pub async fn pump_until_approval_request(
        &mut self,
        timeout: Duration,
    ) -> Result<ApprovalRequest> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "TuiHarness: timed out after {:?} waiting for ApprovalRequired \
                     ({} events drained)",
                    timeout,
                    self.events.len()
                ));
            }
            let next = match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Some(evt)) => evt,
                Ok(None) => {
                    return Err(anyhow!(
                        "TuiHarness: event channel closed before ApprovalRequired"
                    ));
                }
                Err(_) => continue,
            };
            handle_core_event(&mut self.state, next.clone(), &self.command_tx);
            // Bail early if the session ended before an Ask fired —
            // the test's premise is that an approval *will* be requested.
            if matches!(
                &next,
                CoreEvent::Protocol(ServerNotification::SessionResult(_))
            ) {
                self.events.push(next);
                return Err(anyhow!(
                    "TuiHarness: SessionResult arrived before ApprovalRequired — \
                     the engine completed without requesting an approval. Mode/tool \
                     combo may not trigger an Ask decision."
                ));
            }
            // Capture the approval payload, then push to the events buffer.
            let approval = if let CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired {
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

    /// Resolve a pending approval as **approved**. Mirrors what
    /// `tui_runner.rs`'s `UserCommand::ApprovalResponse` arm does in
    /// production via `tui_permission_bridge::resolve_pending`.
    /// Returns `true` if the request_id matched a pending oneshot.
    pub async fn approve(&self, request_id: &str) -> bool {
        coco_cli::tui_permission_bridge::resolve_pending(
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

    /// Resolve a pending approval as **rejected**.
    pub async fn reject(&self, request_id: &str, feedback: Option<String>) -> bool {
        coco_cli::tui_permission_bridge::resolve_pending(
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

    /// Cloneable engine handle. Tests that exercise engine methods that
    /// the driver doesn't expose (currently: `run_manual_compact`) can
    /// call them directly via this.
    pub fn engine(&self) -> Arc<QueryEngine> {
        self.engine.clone()
    }

    /// Cloneable event sender — for handing into engine methods called
    /// outside the driver so their `CoreEvent`s land on the same channel
    /// `pump_until_idle` / `drain_pending_events` reads from.
    pub fn event_tx(&self) -> mpsc::Sender<CoreEvent> {
        self.event_tx.clone()
    }

    /// Pump every queued event into AppState, returning when the channel
    /// goes quiet for `quiet_for`. Unlike [`Self::pump_until_idle`] this
    /// does NOT block on a `SessionResult` — useful for engine methods
    /// (compact, dream, …) that don't produce a session terminator.
    ///
    /// Returns the number of events drained on this call.
    pub async fn drain_pending_events(&mut self, quiet_for: Duration) -> usize {
        let mut drained = 0;
        loop {
            match tokio::time::timeout(quiet_for, self.event_rx.recv()).await {
                Ok(Some(evt)) => {
                    handle_core_event(&mut self.state, evt.clone(), &self.command_tx);
                    self.events.push(evt);
                    drained += 1;
                }
                Ok(None) => break, // channel closed
                Err(_) => break,   // quiet window elapsed
            }
        }
        drained
    }

    /// Clone of the engine's cancel token. Tests that drive
    /// interrupt/cancel paths fire it from a side task while
    /// `pump_until_idle` is awaiting on the main task.
    ///
    /// One-shot: cancelling permanently disables the engine for the
    /// rest of the harness's lifetime (production rebuilds the engine
    /// per turn with a fresh child token; the harness keeps a single
    /// engine for simplicity). Tests that cancel must not run more
    /// turns afterwards.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Stop the agent driver task and wait for it to exit. `drop` does
    /// the same with a 2s budget; tests that want a deterministic
    /// shutdown order can call this explicitly.
    pub async fn shutdown(mut self) {
        // Closing the command channel makes the driver's `recv()` return
        // `None` — that's the agreed shutdown trigger.
        drop(self.command_tx.clone());
        self.cancel.cancel();
        if let Some(handle) = self.driver_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }
    }
}

impl Drop for TuiHarness {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Best-effort: abort the driver. We can't `await` it from a
        // sync drop, but cancellation + `abort` plus the cancelled
        // channel are enough that the runtime reaps the task on tear
        // down.
        if let Some(handle) = self.driver_task.take() {
            handle.abort();
        }
    }
}

/// Stripped-down agent driver. The production driver
/// (`app/cli/src/tui_runner.rs::run_agent_driver`) handles 15+
/// `UserCommand` variants — slash commands, permission mode changes,
/// rewind, plan-approval, file-history queries, etc. The TUI scenarios
/// in this crate only exercise `SubmitInput` (the conversational path
/// every test uses) and let `Shutdown` / dropped channels close the
/// loop. Anything not covered here either:
///   1. routes through the same `engine.run_with_events` path
///      `SubmitInput` already exercises (e.g. `ExecuteSkill`), or
///   2. needs the full `SessionRuntime` we deliberately don't build
///      (e.g. `Rewind`, `Compact`, `PlanApprovalResponse`).
async fn run_test_agent_driver(
    engine: Arc<QueryEngine>,
    mut command_rx: mpsc::Receiver<UserCommand>,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            UserCommand::SubmitInput { content, .. } => {
                if content.is_empty() {
                    continue;
                }
                // `run_with_events` itself emits SessionStarted /
                // TurnStarted / TurnCompleted / SessionResult into the
                // event channel — the harness folds them into AppState.
                let _ = engine
                    .run_with_events(&content, event_tx.clone(), coco_types::TurnId::generate())
                    .await;
            }
            UserCommand::Shutdown { .. } => break,
            // Other commands are intentionally ignored — see fn docs.
            _ => {}
        }
    }
}

/// What [`TuiHarness::pump_until_approval_request`] returns. Just the
/// fields a test typically asserts on — `request_id` to route the
/// approve/reject, `tool_name` for sanity-checking, and the rendered
/// `input_preview` (JSON-stringified) for substring assertions on
/// what the engine asked about.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub input_preview: String,
}

/// One-line debug summary used in timeout error messages.
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
