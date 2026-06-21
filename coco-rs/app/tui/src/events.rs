//! TUI events and commands — the Message layer in TEA.
//!
//! - [`TuiEvent`]: low-level events from terminal, agent, and timers
//! - [`TuiCommand`]: high-level user actions derived from events

use crossterm::event::KeyEvent;

use crate::state::SlashCommandName;

/// Low-level TUI events from all sources.
///
/// The main event loop receives these via `tokio::select!` multiplexing.
///
/// Note: no mouse events. coco-tui deliberately leaves mouse input to the
/// terminal emulator so the native drag-to-select / Cmd+C flow keeps working
/// (same choice as codex-rs). `terminal.rs` never calls `EnableMouseCapture`.
#[derive(Debug)]
pub enum TuiEvent {
    // ── Terminal events ──
    /// Keyboard input.
    Key(KeyEvent),
    /// Terminal resized.
    Resize { width: u16, height: u16 },
    /// Terminal focus changed.
    FocusChanged { focused: bool },

    // ── Timing events ──
    /// Redraw request.
    Draw,
    /// Status tick (250 ms) — toast expiry, idle detection, chord and
    /// double-press auto-cancel. The 50 ms spinner cadence used to be
    /// a separate event; it is now driven by `redraw()` self-arming
    /// via [`crate::frame_requester::FrameRequester::schedule_frame_in`]
    /// while a turn / stream is active.
    Tick,

    // ── Input events ──
    /// Bracketed paste content.
    Paste(String),

    // ── Process control ──
    /// User requested process suspend (Ctrl+Z on Unix).
    ///
    /// In raw mode the terminal no longer translates Ctrl+Z into a
    /// SIGTSTP automatically, so [`App::convert_crossterm_event`]
    /// intercepts the key and emits this event. The handler calls
    /// `Tui::trigger_suspend`, which delivers `SIGTSTP` to the process
    /// group via `libc::kill` and blocks until SIGCONT.
    ///
    /// No-op on non-Unix platforms (key falls through as a normal
    /// `Key` event).
    Suspend,

    // ── Permission events ──
    /// Background classifier approved a pending permission request.
    ///
    /// When the classifier approves before the user responds, auto-dismiss
    /// the state with this event.
    ClassifierApproved {
        request_id: String,
        /// The matched rule name from the classifier.
        matched_rule: Option<String>,
    },
    /// Background classifier denied or was unavailable.
    ClassifierDenied { request_id: String, reason: String },
}

/// High-level user commands derived from keyboard/event processing.
///
/// Each variant maps to a state mutation in `update.rs::handle_command()`.
#[derive(Debug, Clone)]
pub enum TuiCommand {
    /// Intentionally do nothing.
    Noop,
    // ── Mode toggles ──
    /// Toggle plan mode.
    TogglePlanMode,
    /// Cycle thinking level (off → low → medium → high).
    CycleThinkingLevel,
    /// Toggle thinking content visibility.
    ToggleThinking,
    /// Cycle model (show model picker).
    CycleModel,
    /// Cycle permission mode.
    CyclePermissionMode,
    /// Toggle fast mode.
    ToggleFastMode,

    // ── Input actions ──
    /// Submit current input (or queue if streaming).
    SubmitInput,
    /// Submit current input by queueing it and interrupting cancelable tools.
    SubmitInterrupt,
    /// Queue input during streaming.
    QueueInput,
    /// Interrupt current operation.
    Interrupt,
    /// Cancel current action / close state.
    Cancel,
    /// Clear screen.
    ClearScreen,

    // ── Text editing ──
    /// Insert a character at cursor.
    InsertChar(char),
    /// Insert a newline.
    InsertNewline,
    /// Delete character before cursor.
    DeleteBackward,
    /// Delete character at cursor.
    DeleteForward,
    /// Delete word before cursor.
    DeleteWordBackward,
    /// Delete word after cursor.
    DeleteWordForward,
    /// Kill to end of line (Ctrl+K).
    KillToEndOfLine,
    /// Kill from beginning of line to cursor (Ctrl+U).
    KillToBeginningOfLine,
    /// Yank killed text (Ctrl+Y).
    Yank,

    // ── Cursor movement ──
    /// Move cursor left.
    CursorLeft,
    /// Move cursor right.
    CursorRight,
    /// Move cursor up (history previous).
    CursorUp,
    /// Move cursor down (history next).
    CursorDown,
    /// Move cursor to start of line.
    CursorHome,
    /// Move cursor to end of line.
    CursorEnd,
    /// Move cursor one word left.
    WordLeft,
    /// Move cursor one word right.
    WordRight,

    // ── Ctrl+R reverse history search ──
    /// Enter reverse-i-search mode (Ctrl+R on an idle composer).
    HistorySearchStart,
    /// Append a character to the active search query.
    HistorySearchInput(char),
    /// Delete the last character from the active search query.
    HistorySearchBackspace,
    /// Step to an older match (Ctrl+R / Up while searching).
    HistorySearchOlder,
    /// Step to a newer match (Down while searching).
    HistorySearchNewer,
    /// Accept the previewed match into the composer and exit search.
    HistorySearchAccept,
    /// Cancel search and restore the saved draft (Esc).
    HistorySearchCancel,

    // ── Scrolling ──
    /// Scroll up by line step.
    ScrollUp,
    /// Scroll down by line step.
    ScrollDown,
    /// Scroll up by page.
    PageUp,
    /// Scroll down by page.
    PageDown,

    // ── Focus ──
    /// Focus next panel.
    FocusNext,
    /// Focus previous panel.
    FocusPrevious,
    /// Focus next agent in the activity surface.
    FocusNextAgent,
    /// Focus previous agent in the activity surface.
    FocusPrevAgent,
    /// Enter / navigate the agent switcher with `Shift+↑` (`-1`) / `Shift+↓`
    /// (`+1`). When the switcher isn't focused it parks focus there; when it
    /// is, it moves the selection by the delta.
    AgentSwitcherNav(i32),

    // ── Surface actions ──
    /// Approve (Y in permission dialog).
    Approve,
    /// Deny (N in permission dialog).
    Deny,
    /// Approve all / always allow (A in permission dialog).
    ApproveAll,
    /// Allow the permission for the current session only.
    ApproveSession,
    /// Digit shortcut (1-based) committing the numbered row of a classic
    /// permission dialog directly.
    PermissionDigit(usize),
    /// Classifier auto-approved a pending permission.
    ClassifierAutoApprove {
        request_id: String,
        matched_rule: Option<String>,
    },
    /// Accept the selected autocomplete suggestion without submitting.
    /// Used by Tab in the autocomplete context.
    AutocompleteAccept,
    /// Accept the visible prompt suggestion into the editable input.
    AcceptPromptSuggestion,
    /// Accept the visible prompt suggestion and submit it immediately.
    SubmitPromptSuggestion,
    /// Accept the selected autocomplete suggestion and submit it when the
    /// accepted slash command takes no arguments. Used by Enter in the
    /// autocomplete context.
    AutocompleteSubmit,

    // ── Commands & surfaces ──
    /// Execute a skill.
    ExecuteSkill(String),
    /// Execute a slash command by name (no leading `/`). Triggered by
    /// a `command:foo` keybinding from the user's `keybindings.json`
    /// — fires through the same handler as if the user typed `/foo`
    /// and hit Enter.
    ExecuteSlashCommand(SlashCommandName),
    /// Show help state.
    ShowHelp,
    /// Show command palette state.
    ShowCommandPalette,
    /// Open the teams roster picker (Ctrl+T when a team is active) — the
    /// leader cycles a teammate's permission mode (gap 8).
    OpenTeamRoster,
    /// Show session browser state.
    ShowSessionBrowser,
    /// Show global search (Ctrl+Shift+F).
    ShowGlobalSearch,
    /// Show quick open (Ctrl+O).
    ShowQuickOpen,
    /// Show rewind state (message selector). Triggered by double-Esc
    /// or `/rewind` command.
    ShowRewind,
    /// Open the rewind state pre-anchored to a specific message, jumping
    /// straight to the RestoreOptions confirm screen. Used by
    /// message-actions `edit` and by the non-lossless branch of
    /// auto-restore-on-interrupt.
    ///
    /// `target_uuid` is the parsed user-message UUID. The slash route
    /// does not go through this event: `/rewind` always opens the bare
    /// picker.
    ShowRewindFor { target_uuid: uuid::Uuid },
    /// Show doctor/diagnostics.
    ShowDoctor,
    /// Show the tabbed settings panel.
    ShowSettings,
    /// Toggle language-level syntax highlighting for markdown code blocks.
    ToggleSyntaxHighlighting,
    /// Tab: cycle Settings tab forward (Settings state) OR cycle
    /// question/footer focus (Question state). Handler in update.rs
    /// dispatches per-state.
    SettingsNextTab,
    /// Shift+Tab variant of [`SettingsNextTab`].
    SettingsPrevTab,

    // ── Surface navigation ──
    /// Filter text in active filterable state.
    SurfaceFilter(char),
    /// Delete char from state filter.
    SurfaceFilterBackspace,
    /// Select next item in state list.
    SurfaceNext,
    /// Select previous item in state list.
    SurfacePrev,
    /// Jump selection to the first item in the state list
    /// (`Home` / `Shift+Up` / `Meta+Up` / `Shift+K`).
    SurfaceJumpStart,
    /// Jump selection to the last item in the state list
    /// (`End` / `Shift+Down` / `Meta+Down` / `Shift+J`).
    SurfaceJumpEnd,
    /// Confirm selection in state.
    SurfaceConfirm,
    /// Scroll the active permission prompt's command/detail body up one page
    /// (`PageUp`). The action rows stay pinned; only the scrollable body moves.
    PermissionScrollUp,
    /// Scroll the active permission prompt's command/detail body down one page
    /// (`PageDown`).
    PermissionScrollDown,
    /// CopyPicker-only shortcut: write the focused selection to the
    /// fallback temp file without touching the clipboard. Bound to `w`.
    CopyPickerWriteToFile,
    /// Cycle thinking effort in the ModelPicker state by `delta`.
    /// Bound to Left/Right via `modelPicker:decreaseEffort` /
    /// `modelPicker:increaseEffort`. Distinct from `SurfacePrev/Next`
    /// (Up/Down) because the picker has two orthogonal cursors —
    /// model row and effort level.
    ModelPickerCycleEffort(i32),
    /// Cycle the configured role in the ModelPicker state by `delta`.
    /// Tab → +1, Shift+Tab → -1.
    ModelPickerCycleRole(i32),
    /// Switch the focused question in the AskUserQuestion prompt by `delta`
    /// (Left → -1, Right → +1), wrapping among questions only (NOT the footer
    /// actions, which stay on Tab).
    QuestionSwitchQuestion(i32),
    /// Cycle the FOCUSED teammate's mode in the TeamRoster picker by `delta`
    /// (Left/Right) and apply it immediately (cycling persists). gap 8.
    TeamRosterCycleMode(i32),
    /// Cycle ALL teammates' modes in tandem by `delta` (Shift+Left/Right) and
    /// apply immediately. If the members' modes diverge, normalise all to
    /// `Default`; otherwise advance all by `delta`. One atomic `team.json`
    /// write + per-teammate mailbox notify.
    TeamRosterCycleAllModes(i32),

    // ── Task management ──
    /// Background all foreground tasks. Single-press Ctrl+B when at
    /// least one foreground subagent / tool execution is running —
    /// see `update.rs::TuiCommand::BackgroundAllTasks` dispatch.
    BackgroundAllTasks,
    /// Kill all running agents.
    KillAllAgents,

    // ── External editor ──
    /// Open input in external editor ($EDITOR).
    OpenExternalEditor,
    /// Open plan file in external editor.
    OpenPlanEditor,

    // ── Display toggles ──
    /// Toggle tool call collapse.
    ToggleToolCollapse,
    /// Toggle system reminder visibility.
    ToggleSystemReminders,
    /// Toggle the permission-prompt risk-explainer panel (Ctrl+E). Lazily
    /// fetches the LLM explanation on first open. No-op unless a permission
    /// prompt is active.
    TogglePermissionExplanation,

    // ── Clipboard ──
    /// Paste from clipboard (image first, text fallback).
    PasteFromClipboard,
    /// Copy the last agent response to the system clipboard (Ctrl+O / /copy).
    CopyLastMessage,

    // ── Application ──
    /// Quit the application **immediately**. Issued by `/quit` /
    /// `/exit` slash commands and by `exit::ExitEffect::Quit`
    /// (delivered after a double-press has confirmed the user's intent).
    /// Plain Ctrl+C / Ctrl+D do NOT emit `Quit` directly — they go
    /// through [`RequestExit`] or [`Interrupt`] so the double-press
    /// state machine in `update::exit` gets a chance to run.
    Quit,
    /// User pressed an exit key (Ctrl+D in defaults). The handler in
    /// `update::exit::on_request_exit` runs the double-press tracker
    /// and either arms the "Press X again to exit" hint or fires
    /// [`Quit`]. Distinct from [`Interrupt`] (Ctrl+C) because Ctrl+D
    /// has no "cancel running task" semantics — its first press only
    /// arms the prompt.
    RequestExit,

    // ── Stash ──
    /// Push to / pop from the input-draft stash slot. Empty input + stash
    /// present → pop; non-empty input → push (overwrites).
    StashInputDraft,

    // ── Expanded view ──
    /// Cycle the right-rail `expanded_view` between `None`, `Tasks`,
    /// and (when teammates are running) `Teammates`.
    ToggleExpandedTasksView,
    /// Toggle whether teammate spinner lines show recent message preview text.
    ToggleTeammateMessagePreview,
    /// Open / close the transcript state — verbose, scrollable, all-messages view.
    ToggleTranscript,
    /// Select next expandable transcript cell.
    TranscriptSelectNext,
    /// Expand/collapse the selected transcript cell.
    TranscriptToggleCell,
    /// Scroll the transcript reader by rendered rows.
    TranscriptScrollLines(i32),
    /// Scroll the transcript reader by one viewport page.
    TranscriptPage(i32),
    /// Jump the transcript reader to the top.
    TranscriptJumpStart,
    /// Jump the transcript reader to the bottom.
    TranscriptJumpEnd,
}
