//! Main TUI application — the event loop driver.
//!
//! Implements the async run loop using `tokio::select!` to multiplex
//! terminal events, agent events, and timer ticks.

#[path = "app_events.rs"]
mod app_events;

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;

use coco_file_search::FileIndex;
use coco_file_search::SharedFileIndex;
use coco_file_search::create_shared_index;
use crossterm::event::Event;
use crossterm::event::EventStream;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio::time::interval_at;
use tokio_stream::StreamExt;

use std::time::Duration;
use std::time::Instant;

use crate::autocomplete::FileSearchEvent;
use crate::autocomplete::FileSearchManager;
use crate::autocomplete::SymbolSearchEvent;
use crate::autocomplete::SymbolSearchManager;
use crate::autocomplete::file_search::create_file_search_channel;
use crate::autocomplete::symbol_search::create_symbol_search_channel;
use crate::command::UserCommand;
use crate::completion::PathCompletionEvent;
use crate::completion::PathCompletionManager;
use crate::completion::create_path_completion_channel;
use crate::events::TuiEvent;
use crate::git_index_watcher;
use crate::keybinding_bridge;
use crate::state::AppState;
use crate::state::SuggestionKind;
use crate::state::Toast;
use crate::state::UiAnimation;
use crate::terminal::Tui;
use crate::update::handle_command;
pub(crate) use app_events::auto_mode_denied_toast_message;
use app_events::*;
use coco_tui_ui::constants;

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TuiOnlyEvent;

use crate::server_notification_handler;

/// Idle threshold for the `idle_prompt` notification.
///
/// Default: 60 s. Wire to `settings.json` later if the cadence proves wrong.
const IDLE_PROMPT_THRESHOLD: Duration = Duration::from_secs(60);
const DEFERRED_CORE_EVENT_LIMIT: usize = 256;

/// Create the TUI ↔ Core communication channels.
///
/// Returns (command_tx, command_rx, event_tx, event_rx):
/// - command: TUI → Core (user actions)
/// - event: Core → TUI (agent CoreEvent stream, 3-layer Protocol/Stream/Tui)
pub fn create_channels() -> (
    mpsc::Sender<UserCommand>,
    mpsc::Receiver<UserCommand>,
    mpsc::Sender<CoreEvent>,
    mpsc::Receiver<CoreEvent>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<UserCommand>(32);
    let (event_tx, event_rx) = mpsc::channel::<CoreEvent>(256);
    (cmd_tx, cmd_rx, event_tx, event_rx)
}

/// Main TUI application.
pub struct App {
    tui: Tui,
    state: AppState,
    command_tx: mpsc::Sender<UserCommand>,
    /// Schedules redraws through a coalescing, 120 FPS-capped actor
    /// (`crate::frame_requester`). Handlers no longer call
    /// [`Self::redraw`] directly — they request a draw via the
    /// requester and the dedicated `draw_rx` branch of [`Self::run`]
    /// performs the actual paint. Lets multiple events in the same
    /// select! iteration coalesce into a single paint and lets idle
    /// frames cost nothing.
    frame_requester: crate::frame_requester::FrameRequester,
    /// Companion of [`Self::frame_requester`]. The scheduler task
    /// broadcasts `()` here when it is time to paint.
    draw_rx: tokio::sync::broadcast::Receiver<()>,
    /// Receives CoreEvent (3-layer: Protocol/Stream/Tui) from the agent loop.
    notification_rx: mpsc::Receiver<CoreEvent>,
    file_search: FileSearchManager,
    file_search_rx: mpsc::Receiver<FileSearchEvent>,
    path_completion: PathCompletionManager,
    path_completion_rx: mpsc::Receiver<PathCompletionEvent>,
    symbol_search: SymbolSearchManager,
    symbol_search_rx: mpsc::Receiver<SymbolSearchEvent>,
    /// Optional channel of keybinding-validation issues. The bootstrap
    /// (in `app/cli::tui_runner`) wires a tokio task that subscribes
    /// to `KeybindingsWatcher` and forwards every reload's warnings
    /// here so the TUI surfaces them as toasts. `None` in tests /
    /// headless paths.
    kb_warnings_rx: Option<mpsc::Receiver<Vec<coco_keybindings::ValidationIssue>>>,
    /// Optional channel of theme reload results from `config home/theme.json`.
    theme_reload_rx: Option<mpsc::Receiver<crate::theme::ThemeLoadResult>>,
    /// Optional channel of display settings derived from settings hot reload.
    display_settings_rx: Option<mpsc::Receiver<crate::display_settings::DisplaySettings>>,
    /// Optional channel of config hot-reload failure messages.
    config_reload_errors_rx: Option<mpsc::Receiver<String>>,
    status_line_tx: mpsc::Sender<crate::status_bar::StatusLineUpdate>,
    status_line_rx: mpsc::Receiver<crate::status_bar::StatusLineUpdate>,
    /// External editor request currently owns the foreground terminal.
    /// While set, terminal input is not polled and unrelated core events
    /// are buffered until the editor completion event restores TUI modes.
    external_editor_active: Option<String>,
    deferred_core_events: VecDeque<CoreEvent>,
    pending_frame_inputs: crate::perf::FrameInputStats,
    frame_index: u64,
    memory_perf: crate::perf::MemoryPerfTracker,
    /// Voice input session (capture + STT engine + state machine). `Some` only
    /// when the CLI bootstrap successfully initialized the voice subsystem
    /// (voice cargo feature compiled + a usable STT backend). Driven by the
    /// `voice:pushToTalk` keybinding via `App::toggle_voice`.
    voice: Option<coco_voice::VoiceSession>,
    /// Isolated stream of `VoiceEvent`s from the voice session — joined into
    /// the run loop's `tokio::select!`. `None` when voice is not initialized.
    voice_rx: Option<mpsc::Receiver<coco_voice::VoiceEvent>>,
}

impl App {
    /// Create a new TUI application.
    pub fn new(
        command_tx: mpsc::Sender<UserCommand>,
        notification_rx: mpsc::Receiver<CoreEvent>,
        cwd: PathBuf,
    ) -> io::Result<Self> {
        crate::i18n::init();
        let tui = Tui::new()?;
        let mut state = AppState::new();
        // Stamp the process id so the header can surface it and users can
        // match this session to its per-PID log file. Set only on the real
        // app path (`with_terminal` test ctor leaves the `0` sentinel).
        state.session.pid = std::process::id();
        if let Ok(size) = tui.size() {
            state.ui.terminal_size = size;
        }
        apply_terminal_compatibility_status(&mut state, &tui);
        let index = create_shared_index(cwd.clone());
        // Pre-warm the file index so the first `@` keystroke gets results
        // without waiting for the initial git ls-files / ripgrep walk.
        FileIndex::refresh_background(index.clone());
        // Watch `.git/index` mtime — invalidates the cache when the user
        // commits or checks out a different branch.
        git_index_watcher::spawn(cwd, index.clone());
        let (file_tx, file_rx) = create_file_search_channel();
        let (path_tx, path_rx) = create_path_completion_channel();
        let (sym_tx, sym_rx) = create_symbol_search_channel();

        let (draw_tx, draw_rx) = tokio::sync::broadcast::channel(1);
        let frame_requester = crate::frame_requester::FrameRequester::new(draw_tx);
        let (status_line_tx, status_line_rx) = mpsc::channel(16);
        Ok(Self {
            tui,
            state,
            command_tx,
            frame_requester,
            draw_rx,
            notification_rx,
            file_search: FileSearchManager::new(index, file_tx),
            file_search_rx: file_rx,
            path_completion: PathCompletionManager::new(path_tx),
            path_completion_rx: path_rx,
            symbol_search: SymbolSearchManager::new(sym_tx),
            symbol_search_rx: sym_rx,
            kb_warnings_rx: None,
            theme_reload_rx: None,
            display_settings_rx: None,
            config_reload_errors_rx: None,
            status_line_tx,
            status_line_rx,
            external_editor_active: None,
            deferred_core_events: VecDeque::new(),
            pending_frame_inputs: crate::perf::FrameInputStats::default(),
            frame_index: 0,
            memory_perf: crate::perf::MemoryPerfTracker::default(),
            voice: None,
            voice_rx: None,
        })
    }

    /// Create with an existing terminal (for testing).
    pub fn with_terminal(
        tui: Tui,
        command_tx: mpsc::Sender<UserCommand>,
        notification_rx: mpsc::Receiver<CoreEvent>,
        cwd: PathBuf,
    ) -> Self {
        let index = create_shared_index(cwd);
        let (file_tx, file_rx) = create_file_search_channel();
        let (path_tx, path_rx) = create_path_completion_channel();
        let (sym_tx, sym_rx) = create_symbol_search_channel();
        let mut state = AppState::new();
        apply_terminal_compatibility_status(&mut state, &tui);
        let (draw_tx, draw_rx) = tokio::sync::broadcast::channel(1);
        let frame_requester = crate::frame_requester::FrameRequester::new(draw_tx);
        let (status_line_tx, status_line_rx) = mpsc::channel(16);
        Self {
            tui,
            state,
            command_tx,
            frame_requester,
            draw_rx,
            notification_rx,
            file_search: FileSearchManager::new(index, file_tx),
            file_search_rx: file_rx,
            path_completion: PathCompletionManager::new(path_tx),
            path_completion_rx: path_rx,
            symbol_search: SymbolSearchManager::new(sym_tx),
            symbol_search_rx: sym_rx,
            kb_warnings_rx: None,
            theme_reload_rx: None,
            display_settings_rx: None,
            config_reload_errors_rx: None,
            status_line_tx,
            status_line_rx,
            external_editor_active: None,
            deferred_core_events: VecDeque::new(),
            pending_frame_inputs: crate::perf::FrameInputStats::default(),
            frame_index: 0,
            memory_perf: crate::perf::MemoryPerfTracker::default(),
            voice: None,
            voice_rx: None,
        }
    }

    /// Allow callers to swap in their own pre-built index (used by tests
    /// and by the CLI that already runs `discover_files` for other panels).
    pub fn with_file_index(mut self, index: SharedFileIndex) -> Self {
        self.file_search.set_index(index);
        self
    }

    /// Wire a channel of keybinding-validation issues into the
    /// running TUI. Each `recv()` produces the **full** set of
    /// warnings from the most recent load (defaults-only sessions
    /// emit empty vectors); the App surfaces non-empty vectors as
    /// toasts. Bootstrap (in `app/cli::tui_runner`) creates the tx
    /// half and the forwarding task that reads from
    /// `KeybindingsWatcher::subscribe`.
    pub fn with_keybinding_warnings(
        mut self,
        rx: mpsc::Receiver<Vec<coco_keybindings::ValidationIssue>>,
    ) -> Self {
        self.kb_warnings_rx = Some(rx);
        self
    }

    pub fn with_theme_reload(mut self, rx: mpsc::Receiver<crate::theme::ThemeLoadResult>) -> Self {
        self.theme_reload_rx = Some(rx);
        self
    }

    pub fn with_display_settings_reload(
        mut self,
        rx: mpsc::Receiver<crate::display_settings::DisplaySettings>,
    ) -> Self {
        self.display_settings_rx = Some(rx);
        self
    }

    pub fn with_config_reload_errors(mut self, rx: mpsc::Receiver<String>) -> Self {
        self.config_reload_errors_rx = Some(rx);
        self
    }

    /// Install the voice-input subsystem: the `VoiceSession` plus the receiver
    /// half of its event stream. Called by the CLI bootstrap (`tui_runner`)
    /// when `Feature::Voice` is available and a backend initialized. `enabled`
    /// seeds the live gate (`/voice` toggles it thereafter).
    pub fn with_voice(
        mut self,
        session: coco_voice::VoiceSession,
        rx: mpsc::Receiver<coco_voice::VoiceEvent>,
        enabled: bool,
    ) -> Self {
        self.state.ui.voice.enabled = enabled;
        self.state.ui.voice.engine_label = Some(session.engine_name().to_string());
        self.voice = Some(session);
        self.voice_rx = Some(rx);
        self
    }

    /// Start/stop voice recording (the `voice:pushToTalk` toggle). No-op when
    /// voice is disabled or uninitialized. Returns whether a redraw is needed.
    fn toggle_voice(&mut self) -> bool {
        if !self.state.ui.voice.enabled {
            return false;
        }
        let transcribing = matches!(
            self.state.ui.voice.status,
            crate::state::ui::VoiceStatusKind::Transcribing
        );
        let Some(session) = self.voice.as_mut() else {
            self.state.ui.add_toast(crate::state::ui::Toast::warning(
                "Voice input is unavailable (no backend configured).".to_string(),
            ));
            return true;
        };
        if transcribing && !session.is_recording() {
            // A transcription (possibly a stuck first-use model download) is in
            // flight — pressing the key again aborts it rather than starting a
            // new recording.
            session.cancel_transcription();
            self.state.ui.voice.status = crate::state::ui::VoiceStatusKind::Idle;
            self.state.ui.input.clear_inline_hint();
            self.state.ui.add_toast(crate::state::ui::Toast::warning(
                "Voice transcription cancelled.".to_string(),
            ));
        } else if session.is_recording() {
            session.stop();
            self.state.ui.voice.status = crate::state::ui::VoiceStatusKind::Transcribing;
            let engine = self
                .state
                .ui
                .voice
                .engine_label
                .clone()
                .unwrap_or_else(|| "STT".to_string());
            self.state
                .ui
                .input
                .set_inline_hint(format!("transcribing via {engine}…"));
        } else {
            match session.start() {
                Ok(()) => {
                    self.state.ui.voice.status = crate::state::ui::VoiceStatusKind::Recording;
                    let chord = self
                        .state
                        .ui
                        .kb_handle
                        .display_for(
                            &coco_keybindings::KeybindingAction::VoicePushToTalk,
                            crate::keybinding_bridge::KeybindingContext::Chat,
                        )
                        .unwrap_or_else(|| "F3".to_string());
                    self.state
                        .ui
                        .input
                        .set_inline_hint(format!("🎙 recording… ({chord} to stop)"));
                }
                Err(err) => {
                    self.state.ui.voice.status = crate::state::ui::VoiceStatusKind::Idle;
                    self.state
                        .ui
                        .add_toast(crate::state::ui::Toast::warning(voice_error_text(&err)));
                }
            }
        }
        true
    }

    /// Fold an async `VoiceEvent` into UI state. Returns whether a redraw is
    /// needed.
    fn handle_voice_event(&mut self, event: coco_voice::VoiceEvent) -> bool {
        use crate::state::ui::VoiceStatusKind;
        match event {
            coco_voice::VoiceEvent::RecordingStarted => {
                self.state.ui.voice.status = VoiceStatusKind::Recording;
            }
            coco_voice::VoiceEvent::Transcribing { engine } => {
                self.state.ui.voice.status = VoiceStatusKind::Transcribing;
                self.state
                    .ui
                    .input
                    .set_inline_hint(format!("transcribing via {engine}…"));
            }
            coco_voice::VoiceEvent::Download {
                model,
                received,
                total,
            } => {
                // First-use weight download (runs inside transcription); reuse
                // the inline hint to show progress, cleared by Final/Error.
                let hint = match total {
                    Some(total) if total > 0 => {
                        let pct = received.min(total) * 100 / total;
                        format!("downloading {model} model… {pct}%")
                    }
                    _ => format!("downloading {model} model… {} MB", received / 1_048_576),
                };
                self.state.ui.input.set_inline_hint(hint);
            }
            coco_voice::VoiceEvent::Final { text, .. } => {
                self.state.ui.voice.status = VoiceStatusKind::Idle;
                self.state.ui.input.clear_inline_hint();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    // Insert at the cursor (TranscriptMode::Insert). Not
                    // auto-submitted — the user edits/sends.
                    self.state.ui.input.textarea.insert_str(trimmed);
                }
            }
            coco_voice::VoiceEvent::Error(message) => {
                self.state.ui.voice.status = VoiceStatusKind::Idle;
                self.state.ui.input.clear_inline_hint();
                self.state
                    .ui
                    .add_toast(crate::state::ui::Toast::warning(message));
            }
        }
        true
    }

    /// Get a reference to the state.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Get a mutable reference to the state.
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Run the main event loop.
    ///
    /// Uses `tokio::select!` to multiplex:
    /// - Terminal events (key, mouse, resize, paste)
    /// - Tick timer (250 ms — toast expiry, idle detection,
    ///   chord / double-press cancel)
    /// - Draw notifications from `frame_requester` (coalesced
    ///   redraws; the 50 ms spinner cadence rides this path now)
    pub async fn run(&mut self) -> io::Result<()> {
        tracing::info!(
            target: "coco_tui::app",
            terminal_size = ?self.state.ui.terminal_size,
            // Perf-report attribution: debug-build timings are not comparable
            // to release (the 5ms-empty-frame class of misread).
            debug_build = cfg!(debug_assertions),
            "TUI run loop start",
        );
        // Defensive: guarantee the TUI terminal modes (raw mode, bracketed
        // paste, focus reporting) are armed before the event loop. `setup_terminal`
        // arms them at startup, but the theme / sync-update probes briefly borrow
        // raw mode; this idempotent re-arm ensures no probe ordering can leave the
        // session in cooked mode (which echoes focus reports as `^[[O`/`^[[I` and
        // turns Ctrl+C into SIGINT). Best-effort.
        let _ = crate::terminal::enter_tui_modes(&mut std::io::stdout());
        // Compile the syntect grammars off the main loop so the first frame
        // that renders a code block doesn't pay tens of milliseconds of lazy
        // per-grammar regex compilation. Once per process; detached — the
        // warm state lives in the process-global `SyntaxSet`.
        static SYNTAX_PREWARM: std::sync::Once = std::sync::Once::new();
        let syntax_tier = self.state.ui.display_settings.syntax_highlighting;
        SYNTAX_PREWARM.call_once(move || {
            drop(
                std::thread::Builder::new()
                    .name("syntect-prewarm".to_string())
                    .spawn(move || coco_tui_markdown::prewarm_highlighting(syntax_tier)),
            );
        });
        self.refresh_status_line();
        self.log_memory_sample(
            crate::perf::MemoryPhase::Startup,
            crate::perf::MemorySampleKind::Lifecycle,
        );
        crate::jemalloc_purge::sync_heap_profiling(
            self.state
                .ui
                .display_settings
                .performance
                .heap_profile_enabled,
        );
        // Initial render
        self.redraw()?;
        self.log_memory_sample(
            crate::perf::MemoryPhase::FirstDraw,
            crate::perf::MemorySampleKind::Lifecycle,
        );

        let mut event_stream = EventStream::new();
        let mut tick_interval = interval(constants::TICK_INTERVAL);
        // Skip missed ticks rather than bursting them when the gate
        // re-opens — otherwise a long idle period would dump a stream
        // of catch-up ticks the moment the user types again.
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut memory_interval = memory_perf_interval(self.state.ui.display_settings.performance);

        loop {
            let mut needs_redraw = false;

            // Tick is only useful when there's a timer waiting to
            // fire — toasts to expire, a permission prompt waiting
            // to ripen, a chord / double-press hint that might
            // auto-cancel, or the idle-prompt notification that
            // arms after a query completes. Empty across the board
            // ⇒ runtime sleeps until a real event lands. The spinner
            // path used to ride its own 50 ms interval here; it now
            // self-schedules from `redraw()` via
            // `FrameRequester::schedule_frame_in` while a turn or
            // stream is active.
            let tick_active = self.needs_tick();

            tokio::select! {
                // Terminal events
                Some(Ok(evt)) = event_stream.next(), if self.external_editor_active.is_none() => {
                    if let Some(event) = self.convert_crossterm_event(evt) {
                        self.pending_frame_inputs.terminal_inputs += 1;
                        needs_redraw = self.handle_event(event).await;
                    }
                }
                // Agent CoreEvent from core — coalesce pending events before redraw.
                // Under high throughput (e.g. 100+ TextDeltas/sec) this avoids
                // one redraw per token by draining all ready events first.
                Some(event) = self.notification_rx.recv() => {
                    let memory_phase = crate::perf::memory_phase_for_core_event(&event);
                    self.pending_frame_inputs.record_core_event(&event);
                    needs_redraw = self.handle_core_event(event).await?;
                    if let Some(phase) = memory_phase {
                        self.note_lifecycle_memory_phase(phase);
                    }
                    while let Ok(next) = self.notification_rx.try_recv() {
                        let memory_phase = crate::perf::memory_phase_for_core_event(&next);
                        self.pending_frame_inputs.record_core_event(&next);
                        needs_redraw |= self.handle_core_event(next).await?;
                        if let Some(phase) = memory_phase {
                            self.note_lifecycle_memory_phase(phase);
                        }
                    }
                }
                // Async file-search results (from @path triggers).
                Some(evt) = self.file_search_rx.recv() => {
                    needs_redraw = handle_file_search_event(&mut self.state, evt);
                }
                // Async explicit path completion results.
                Some(evt) = self.path_completion_rx.recv() => {
                    needs_redraw = handle_path_completion_event(&mut self.state, evt);
                }
                // Async symbol-search results (from @#symbol triggers).
                Some(evt) = self.symbol_search_rx.recv() => {
                    needs_redraw = handle_symbol_search_event(&mut self.state, evt);
                }
                // Keybinding-config validation issues from hot-reload.
                // Each non-empty batch becomes a stream of toasts so
                // users see new errors after editing keybindings.json
                // without restarting.
                Some(issues) = recv_optional(&mut self.kb_warnings_rx), if self.kb_warnings_rx.is_some() => {
                    needs_redraw = surface_keybinding_warnings(&mut self.state, issues);
                }
                // Theme config reloads from config home/theme.json. Invalid
                // reloads surface as toasts and keep the prior palette.
                Some(result) = recv_optional(&mut self.theme_reload_rx), if self.theme_reload_rx.is_some() => {
                    needs_redraw = apply_theme_reload(&mut self.state, result);
                }
                Some(display_settings) = recv_optional(&mut self.display_settings_rx), if self.display_settings_rx.is_some() => {
                    self.pending_frame_inputs.settings_reloads += 1;
                    self.state.ui.apply_display_settings(display_settings);
                    memory_interval = memory_perf_interval(self.state.ui.display_settings.performance);
                    crate::jemalloc_purge::sync_heap_profiling(
                        self.state.ui.display_settings.performance.heap_profile_enabled,
                    );
                    needs_redraw = true;
                }
                Some(error) = recv_optional(&mut self.config_reload_errors_rx), if self.config_reload_errors_rx.is_some() => {
                    self.pending_frame_inputs.settings_reloads += 1;
                    self.state.ui.add_toast(crate::state::ui::Toast::warning(
                        crate::i18n::t!("toast.config_reload_failed", error = error.as_str()).to_string(),
                    ));
                    needs_redraw = true;
                }
                Some(update) = self.status_line_rx.recv() => {
                    needs_redraw = self.state.ui.status_line.apply_update(update);
                }
                // Async voice events (transcript ready, recording lifecycle,
                // errors). Isolated stream — folded into UI state here, never
                // bridged into CoreEvent.
                Some(event) = recv_optional(&mut self.voice_rx), if self.voice_rx.is_some() => {
                    needs_redraw = self.handle_voice_event(event);
                }
                // Coalesced draw notification — the FrameRequester
                // task fires this when one or more `schedule_frame()`
                // calls have settled. Renders unconditionally; nothing
                // else does.
                Ok(()) = self.draw_rx.recv() => {
                    self.redraw()?;
                    if self.state.should_exit() {
                        break;
                    }
                    continue;
                }
                // Tick timer — gated so idle sessions don't wake the
                // runtime every 250 ms for no-op expiry checks. The
                // 50 ms spinner cadence used to live next to this
                // arm; the spinner self-schedules via
                // `FrameRequester::schedule_frame_in` from
                // `redraw()` now.
                _ = tick_interval.tick(), if self.external_editor_active.is_none() && tick_active => {
                    self.pending_frame_inputs.ticks += 1;
                    needs_redraw = self.handle_event(TuiEvent::Tick).await;
                }
                _ = memory_interval.tick(), if crate::perf::MemoryPerfTracker::periodic_enabled(self.state.ui.display_settings.performance) => {
                    self.log_memory_sample(
                        crate::perf::MemoryPhase::Periodic,
                        crate::perf::MemorySampleKind::Periodic,
                    );
                }
            };

            // After every iteration, refresh async autocomplete dispatches
            // based on the current trigger. Cheap no-op when the query is
            // unchanged since the last dispatch.
            self.dispatch_pending_search();
            self.refresh_status_line();

            // Route every state-mutating handler's redraw signal
            // through the FrameRequester so multiple events in one
            // select! iteration coalesce into a single paint and the
            // 120 FPS rate limiter caps wasted work.
            if needs_redraw {
                self.frame_requester.schedule_frame();
            }

            if self.state.should_exit() {
                break;
            }
        }

        Ok(())
    }

    fn log_memory_sample(
        &mut self,
        phase: crate::perf::MemoryPhase,
        sample_kind: crate::perf::MemorySampleKind,
    ) {
        let config = self.state.ui.display_settings.performance;
        let retained = crate::perf::retained_memory_stats(
            &self.state,
            self.tui.history_replay_cache_estimated_bytes(),
        );
        self.memory_perf
            .maybe_log(config, phase, sample_kind, retained);
    }

    /// Handle a lifecycle-driven memory phase: log the sample, then — at the
    /// end of a turn — reclaim jemalloc's decayed pages. `TurnEnded` is the
    /// natural quiet point to purge (the turn's transient allocations are
    /// freed and the process is about to idle), and on macOS it's the only
    /// thing that advances page decay, since those builds have no
    /// `background_thread`. The same boundary also writes a heap-profile dump
    /// when `tui.performance.heap_profile_enabled` is on. No-op when the
    /// `jemalloc` feature is off.
    fn note_lifecycle_memory_phase(&mut self, phase: crate::perf::MemoryPhase) {
        self.log_memory_sample(phase, crate::perf::MemorySampleKind::Lifecycle);
        if phase == crate::perf::MemoryPhase::TurnEnded {
            crate::jemalloc_purge::spawn_turn_ended_purge(
                self.state
                    .ui
                    .display_settings
                    .performance
                    .heap_profile_enabled,
            );
        }
    }

    fn refresh_status_line(&mut self) {
        let mut runtime = std::mem::take(&mut self.state.ui.status_line);
        runtime.request_refresh(&self.state, &self.status_line_tx);
        self.state.ui.status_line = runtime;
    }

    /// Run a single draw cycle.
    ///
    /// Folds in the per-frame book-keeping that used to live in the
    /// `TuiEvent::SpinnerTick` handler: pause-clock tick + streaming
    /// dot advance. At the end it consults [`AppState::ui_animation`]
    /// and, only if something is actually animating, re-arms the next
    /// frame via [`FrameRequester::schedule_frame_in`] — so the spinner
    /// self-perpetuates without an unconditional timer, and a blocked or
    /// idle TUI re-arms nothing.
    fn redraw(&mut self) -> io::Result<()> {
        self.frame_index = self.frame_index.saturating_add(1);
        let frame_index = self.frame_index;
        let perf_config = self.state.ui.display_settings.performance;
        let frame_start = perf_config.frame_enabled.then(Instant::now);
        let frame_inputs = std::mem::take(&mut self.pending_frame_inputs);
        let now = self.state.clock.now();

        // Drive the pause clock from the actual paint instant so the
        // displayed elapsed value subtracts time spent in tool-permission
        // prompts.
        let blocked = self
            .state
            .ui
            .interaction
            .active_prompt
            .as_ref()
            .is_some_and(crate::state::interaction::PanePromptState::pauses_status_clock);
        self.state.ui.ephemeral.tick_pause_clock(blocked, now);

        // Advance the streaming-cell display state so the animated
        // dots / cursor stay in sync with the paint cadence.
        if let Some(ref mut streaming) = self.state.ui.streaming {
            streaming.advance_display();
        }

        let draw_start = perf_config.frame_enabled.then(Instant::now);
        let outcome = self.tui.draw_with_frame_index(&self.state, frame_index)?;
        let draw_elapsed = draw_start.map(|start| start.elapsed());
        if outcome.retained_surface_visible && self.state.ui.terminal_focused {
            self.state.ui.confirm_surface_visibility_after_draw(now);
        }
        if outcome.attention_requested {
            self.handle_surface_attention_requested();
        }

        // Self-schedule the next animation frame. The decision of
        // *whether* to animate lives in `AppState::ui_animation` (a pure
        // read of the model); this arm only maps that to a cadence.
        //
        // Streaming keeps the fast tick (reveal pacing is one line per
        // painted frame — slowing the cadence slows the text). The
        // spinner ticks at its glyph period. `Idle` arms nothing, so a
        // turn blocked on a user prompt — or a genuinely idle session —
        // lets the frame scheduler sleep until the next real event.
        match self.state.ui_animation() {
            UiAnimation::StreamReveal => self
                .frame_requester
                .schedule_frame_in(constants::SPINNER_TICK_INTERVAL),
            UiAnimation::SpinnerOnly => self
                .frame_requester
                .schedule_frame_in(constants::SPINNER_ONLY_TICK_INTERVAL),
            UiAnimation::Idle => {}
        }

        if let Some(start) = frame_start {
            let elapsed = start.elapsed();
            if crate::perf::should_log_frame(perf_config, frame_index, elapsed) {
                tracing::debug!(
                    target: crate::perf::TARGET,
                    frame_index,
                    duration_ms = crate::perf::duration_ms(elapsed),
                    draw_ms = crate::perf::duration_ms(draw_elapsed.unwrap_or_default()),
                    core_events = frame_inputs.core_events,
                    stream_text_deltas = frame_inputs.stream_text_deltas,
                    stream_thinking_deltas = frame_inputs.stream_thinking_deltas,
                    terminal_inputs = frame_inputs.terminal_inputs,
                    ticks = frame_inputs.ticks,
                    settings_reloads = frame_inputs.settings_reloads,
                    retained_surface_visible = outcome.retained_surface_visible,
                    attention_requested = outcome.attention_requested,
                    "tui frame redraw completed",
                );
            }
        }

        Ok(())
    }

    fn handle_surface_attention_requested(&mut self) {
        let message = crate::i18n::t!("notification.action_required").to_string();
        coco_tui_ui::widgets::notification::notify(coco_config::constants::PRODUCT_NAME, &message);
        self.state.ui.add_toast(Toast::warning(message));
    }

    async fn handle_core_event(&mut self, event: CoreEvent) -> io::Result<bool> {
        if let CoreEvent::Tui(TuiOnlyEvent::ExternalEditorPrepare { request_id }) = event {
            if self.external_editor_active.is_some() {
                tracing::warn!(
                    target: "coco_tui::external_editor",
                    request_id = %request_id,
                    "ExternalEditorPrepare rejected: another editor is already active",
                );
                let _ = self
                    .command_tx
                    .send(UserCommand::ExternalEditorTerminalPrepareFailed {
                        request_id,
                        error: "another external editor is already active".to_string(),
                    })
                    .await;
                return Ok(false);
            }

            match self.tui.prepare_external_process() {
                Ok(()) => {
                    tracing::info!(
                        target: "coco_tui::external_editor",
                        request_id = %request_id,
                        "terminal prepared for external editor",
                    );
                    self.external_editor_active = Some(request_id.clone());
                    if self
                        .command_tx
                        .send(UserCommand::ExternalEditorTerminalReady { request_id })
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            target: "coco_tui::external_editor",
                            "command_tx closed before ExternalEditorTerminalReady could be sent; restoring",
                        );
                        self.external_editor_active = None;
                        self.tui.restore_after_external_process()?;
                        return Ok(true);
                    }
                }
                Err(err) => {
                    tracing::error!(
                        target: "coco_tui::external_editor",
                        request_id = %request_id,
                        error = %err,
                        "prepare_external_process failed",
                    );
                    let _ = self
                        .command_tx
                        .send(UserCommand::ExternalEditorTerminalPrepareFailed {
                            request_id,
                            error: err.to_string(),
                        })
                        .await;
                }
            }
            return Ok(false);
        }

        if self.external_editor_active.is_some() && !is_external_editor_completion(&event) {
            match defer_core_event(&mut self.deferred_core_events, event) {
                DeferredCoreEvent::Buffered => {}
                DeferredCoreEvent::Dropped => {
                    tracing::warn!(
                        target: "coco_tui::external_editor",
                        limit = DEFERRED_CORE_EVENT_LIMIT,
                        "dropped lossy deferred event while editor owns terminal",
                    );
                }
                DeferredCoreEvent::ProcessNow(event) => {
                    let _ = server_notification_handler::handle_core_event(
                        &mut self.state,
                        *event,
                        &self.command_tx,
                    );
                }
            }
            return Ok(false);
        }

        let mut needs_redraw = false;
        if self.external_editor_active.take().is_some() {
            tracing::info!(
                target: "coco_tui::external_editor",
                deferred_events = self.deferred_core_events.len(),
                "external editor completed; restoring terminal",
            );
            self.tui.restore_after_external_process()?;
            needs_redraw = true;
        }

        needs_redraw |= server_notification_handler::handle_core_event(
            &mut self.state,
            event,
            &self.command_tx,
        );
        while let Some(deferred) = self.deferred_core_events.pop_front() {
            needs_redraw |= server_notification_handler::handle_core_event(
                &mut self.state,
                deferred,
                &self.command_tx,
            );
        }
        Ok(needs_redraw)
    }

    /// Fire a file/symbol/path search if the active request key has changed
    /// since the last dispatch. Clears pending when no async trigger is active.
    fn dispatch_pending_search(&mut self) {
        let next = match self.state.ui.completion.active_key.clone() {
            Some(ref sug)
                if matches!(
                    sug.kind,
                    SuggestionKind::At
                        | SuggestionKind::Path
                        | SuggestionKind::BashPath
                        | SuggestionKind::Directory
                        | SuggestionKind::Symbol
                ) =>
            {
                Some(sug.clone())
            }
            _ => None,
        };
        let key = match next {
            Some(v) => v,
            None => {
                if self.state.ui.completion.last_dispatched.is_some() {
                    self.file_search.cancel();
                    self.path_completion.cancel();
                    self.symbol_search.cancel();
                    self.state.ui.completion.last_dispatched = None;
                }
                return;
            }
        };
        let unchanged = self
            .state
            .ui
            .completion
            .last_dispatched
            .as_ref()
            .is_some_and(|dispatched| dispatched == &key);
        if unchanged {
            return;
        }
        match key.kind {
            SuggestionKind::At => {
                // Unified `@` popup dispatches a file search; agent
                // matches are already seeded synchronously into the
                // popup by `unified::seed_agent_items`. MCP resource
                // search would also dispatch here once wired.
                self.symbol_search.cancel();
                self.path_completion.cancel();
                self.file_search.search(key.clone());
            }
            SuggestionKind::Path | SuggestionKind::BashPath | SuggestionKind::Directory => {
                self.file_search.cancel();
                self.symbol_search.cancel();
                self.path_completion.search(key.clone());
            }
            SuggestionKind::Symbol => {
                self.file_search.cancel();
                self.path_completion.cancel();
                self.symbol_search.search(key.clone());
            }
            SuggestionKind::SlashCommand | SuggestionKind::CustomTitle => return,
        }
        self.state.ui.completion.last_dispatched = Some(key);
    }

    /// Convert a crossterm event to a TuiEvent.
    fn convert_crossterm_event(&self, event: Event) -> Option<TuiEvent> {
        convert_crossterm_event(event)
    }

    /// Handle a TUI event, returning true if redraw needed.
    async fn handle_event(&mut self, event: TuiEvent) -> bool {
        match event {
            TuiEvent::Key(key) => {
                // (The old "write last_esc_time before dispatch" path
                // was removed — see `update::exit` + `state.ui.*_tracker`
                // for the new double-press machine.)
                // Every input event bumps the last-interaction timestamp
                // so the idle-prompt timer restarts from "now" rather
                // than firing while the user is actively typing.
                let now = self.state.clock.now();
                self.state.session.last_user_interaction_at = now;
                if self.tui.retained_surface_visible() {
                    self.state.ui.record_surface_interaction(now);
                }
                // Delegate all key mapping to keybinding_bridge
                if let Some(cmd) = keybinding_bridge::map_key(&self.state, key) {
                    // Voice push-to-talk is intercepted here — the VoiceSession
                    // lives on `App`, not `AppState`, so it can't be driven from
                    // `update::handle_command`.
                    if matches!(cmd, crate::events::TuiCommand::VoiceToggle) {
                        self.toggle_voice()
                    } else {
                        handle_command(&mut self.state, cmd, &self.command_tx).await
                    }
                } else {
                    false
                }
            }
            TuiEvent::Tick => {
                let now = self.state.clock.now();
                let had_toasts = self.state.ui.has_toasts();
                self.state.ui.expire_toasts();
                let permission_prompt_ready = self.state.ui.flush_delayed_permissions(now);
                self.maybe_fire_idle_prompt().await;
                // Surface a pending plugin-install hint (zero-cost when the
                // single-slot store is empty, which is the common case).
                let plugin_hint_shown = self.maybe_surface_plugin_hint();
                // Drive the chord-timeout from the tick so a pending
                // chord auto-cancels after the 1 s window without
                // requiring another keypress.
                let chord_cancelled = self.state.ui.kb_handle.tick(now);
                // Expire any armed double-press hint (Ctrl+C / Ctrl+D
                // exit prompt, double-Esc rewind) so the footer text
                // disappears after `DOUBLE_PRESS_TIMEOUT` even if the
                // user never presses another key.
                let pending_exit_before = self.state.ui.pending_exit_hint();
                let double_press_expired = self.state.ui.tick_double_press(now);
                if double_press_expired
                    && let Some(key) = pending_exit_before
                    && self.state.ui.pending_exit_hint().is_none()
                {
                    tracing::info!(key = key.label(), "exit prompt expired before second press");
                }
                (had_toasts && !self.state.ui.has_toasts())
                    || chord_cancelled
                    || double_press_expired
                    || permission_prompt_ready
                    || plugin_hint_shown
            }
            TuiEvent::Paste(text) => {
                let now = self.state.clock.now();
                self.state.session.last_user_interaction_at = now;
                if self.tui.retained_surface_visible() {
                    self.state.ui.record_surface_interaction(now);
                }
                tracing::debug!(
                    target: "coco_tui::input",
                    chars = text.len(),
                    lines = text.lines().count(),
                    "bracketed paste",
                );
                // Route the paste into the active AskUserQuestion free-text
                // input when it is focused; otherwise the main input. Some
                // terminals deliver IME-committed CJK as a bracketed paste, so
                // without this the text would silently land in the hidden
                // background composer instead of the question input.
                if crate::update::route_question_free_text_paste(&mut self.state, &text) {
                    // consumed by the question free-text input
                } else if crate::modal_pane::add_directory::route_paste(&mut self.state, &text) {
                    // consumed by the `/add-dir` overlay input; paste otherwise
                    // bypasses modal interception and leaks to the main composer.
                } else if crate::modal_pane::permissions_editor::route_paste(&mut self.state, &text)
                {
                    // consumed by the `/permissions` add-rule form; same modal
                    // bypass as `/add-dir` above.
                } else if let Some((bytes, mime)) = crate::update::sniff_image_path_paste(&text) {
                    // Drag-and-drop of an image file onto the terminal pastes
                    // its path — attach the image (with bytes; a bytes-less
                    // pill is dropped at submit) instead of inserting the
                    // path text.
                    let size_kb = bytes.len().div_ceil(1024);
                    let pill = self.state.ui.paste_manager.add_image_data(bytes, mime);
                    self.state.ui.input.textarea.insert_str(&pill);
                    self.state.ui.add_toast(crate::state::ui::Toast::success(
                        crate::i18n::t!("toast.image_attached", size_kb = size_kb).to_string(),
                    ));
                    crate::autocomplete::refresh_suggestions(&mut self.state);
                } else if text.chars().count() > coco_tui_ui::paste::LARGE_PASTE_CHAR_THRESHOLD {
                    // Large text paste: store as a pill instead of flooding
                    // the composer; expands back to the full content at
                    // submit (`PasteManager::resolve_structured`).
                    let pill = self.state.ui.paste_manager.add_text(text);
                    self.state.ui.input.textarea.insert_str(&pill);
                    crate::autocomplete::refresh_suggestions(&mut self.state);
                } else {
                    // Batch insertion via TextArea is O(text.len()) and only
                    // recomputes the wrap cache once, vs N times for per-char insert.
                    self.state.ui.input.textarea.insert_str(&text);
                    // Paste bypasses update::handle_command, so refresh the
                    // autocomplete state directly here.
                    crate::autocomplete::refresh_suggestions(&mut self.state);
                }
                true
            }
            TuiEvent::Suspend => {
                tracing::info!(target: "coco_tui::app", "Ctrl+Z suspend requested");
                // Blocks until SIGCONT (typically delivered by `fg` in
                // the parent shell). On return, `Tui::draw` will pick
                // up the pending resume action and clear/repaint the native
                // surface on the next frame. If the suspend/restore path
                // fails, exit instead of continuing in an unknown terminal
                // mode.
                if let Err(err) = self.tui.trigger_suspend() {
                    tracing::error!(error = %err, "trigger_suspend failed; exiting TUI");
                    self.state.quit();
                    return false;
                }
                tracing::info!(target: "coco_tui::app", "TUI resumed after SIGCONT");
                true
            }
            TuiEvent::Resize { width, height } => {
                tracing::debug!(
                    target: "coco_tui::app",
                    width,
                    height,
                    "terminal resized",
                );
                self.state.ui.terminal_size = ratatui::layout::Size::new(width, height);
                true
            }
            TuiEvent::FocusChanged { focused } => {
                tracing::trace!(
                    target: "coco_tui::app",
                    focused,
                    "terminal focus changed",
                );
                // Track focus for turn-complete notification gating.
                self.state.ui.terminal_focused = focused;
                if focused {
                    self.state.ui.request_surface_visibility_confirmation();
                } else {
                    self.state.ui.clear_surface_visibility_confirmation();
                }
                // Force a redraw so the post-draw cursor pin re-asserts
                // the cursor position. Without this, terminals like
                // iTerm2 / Terminal.app re-show the cursor at the last
                // write position (status bar end) on focus-gained.
                true
            }
            TuiEvent::Draw => true,
            TuiEvent::ClassifierApproved {
                request_id,
                matched_rule,
            } => {
                if let Some(crate::state::PanePromptState::Permission(p)) =
                    self.state.ui.interaction.active_prompt.as_mut()
                    && p.request_id == request_id
                {
                    p.classifier_checking = false;
                    p.classifier_auto_approved = Some(matched_rule.unwrap_or_default());
                }
                true
            }
            TuiEvent::ClassifierDenied { request_id, reason } => {
                handle_classifier_denied(&mut self.state, &request_id, &reason)
            }
        }
    }

    /// Fire the `idle_prompt` notification once per turn-completion if
    /// the user has been idle past `IDLE_PROMPT_THRESHOLD`.
    ///
    /// Polls on the existing 250 ms tick — same outcome as a dedicated
    /// timer task, avoids extra spawns. Skips when a modal is open or
    /// the agent is busy.
    /// Does the next `TICK_INTERVAL` tick have anything to do?
    ///
    /// Every condition checked here corresponds to one of the side
    /// effects in `TuiEvent::Tick`: toast expiry, delayed-permission
    /// ripening, idle-prompt firing, and the chord / double-press
    /// hint auto-cancel. When all are false the tick would be a
    /// no-op; gating the `select!` arm lets the runtime sleep until
    /// a real event lands.
    fn needs_tick(&self) -> bool {
        let session = &self.state.session;
        let ui = &self.state.ui;
        ui.has_toasts()
            || !ui.interaction.delayed_permissions.is_empty()
            || ui.kb_handle.has_pending_chord()
            || ui.ctrl_c_tracker.pending().is_some()
            || ui.ctrl_d_tracker.pending().is_some()
            || ui.esc_tracker.pending().is_some()
            || session.active_goal.is_some()
            || (session.last_query_completion_at.is_some() && !session.idle_prompt_fired)
            // A pending plugin-install hint needs a tick to surface its modal.
            || coco_plugins::pending_hint_snapshot().is_some()
    }

    async fn maybe_fire_idle_prompt(&mut self) {
        let session = &self.state.session;
        let Some(qct) = session.last_query_completion_at else {
            return;
        };
        if session.idle_prompt_fired {
            return;
        }
        if session.is_busy() {
            return;
        }
        if self.state.ui.has_active_surface() {
            return;
        }
        if session.last_user_interaction_at > qct {
            return;
        }
        if qct.elapsed() < IDLE_PROMPT_THRESHOLD {
            return;
        }
        tracing::info!(
            target: "coco_tui::idle_prompt",
            idle_secs = qct.elapsed().as_secs(),
            "firing idle_prompt notification hook",
        );
        let _ = self
            .command_tx
            .send(UserCommand::FireIdleNotification {
                message: format!(
                    "{} is waiting for your input",
                    coco_config::constants::PRODUCT_NAME
                ),
            })
            .await;
        self.state.session.idle_prompt_fired = true;
    }

    /// Poll the process-level pending-hint store and, when a hint is waiting
    /// and no blocking interaction is active, resolve it against the
    /// marketplace cache and surface the plugin-recommendation modal.
    ///
    /// Marks shown-this-session on success, then clears the slot. The
    /// pre-store gate (`maybe_record_plugin_hint`) already dropped
    /// installed/shown/capped/policy-blocked hints, so anything that reaches
    /// here is worth surfacing if it resolves against the cache.
    fn maybe_surface_plugin_hint(&mut self) -> bool {
        // Single-slot: nothing pending → nothing to do.
        let Some(hint) = coco_plugins::pending_hint_snapshot() else {
            return false;
        };
        // Don't displace a higher-priority focused dialog / prompt; the slot
        // stays full and we retry on the next idle tick.
        if self.state.ui.has_blocking_interaction() || self.state.ui.modal.is_some() {
            return false;
        }

        let resolved = resolve_pending_plugin_hint(&hint);
        // Clear the slot regardless — a hint that doesn't resolve against the
        // cache is discarded (TS returns null and drops it).
        coco_plugins::hints::clear_pending_hint();
        let Some(rec) = resolved else {
            return false;
        };

        coco_plugins::hints::mark_shown_this_session();
        self.state
            .ui
            .show_modal(crate::state::ModalState::PluginHint(
                crate::state::PluginHintState {
                    plugin_id: rec.plugin_id,
                    plugin_name: rec.plugin_name,
                    marketplace_name: rec.marketplace_name,
                    plugin_description: rec.plugin_description,
                    source_command: rec.source_command,
                    selected: 0,
                },
            ));
        true
    }
}

#[cfg(test)]
#[path = "app.test.rs"]
mod tests;
