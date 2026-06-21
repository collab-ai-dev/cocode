//! UI state — local TUI state, never sent to the agent.
//!
//! This file keeps the UiState plus the pieces tightly coupled to it: input +
//! history, focus, streaming, interaction pane state, modal state, and toasts.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use ratatui::layout::Size;

use crate::display_settings::DisplaySettings;
use crate::keybinding_resolver::KeybindingHandle;
use crate::state::interaction::AtPopupState;
use crate::state::interaction::ComposerPopupState;
use crate::state::interaction::InteractionPaneState;
use crate::state::interaction::PanePromptState;
use crate::state::interaction::SlashPopupState;
use crate::state::interaction::SymbolPopupState;
use crate::state::modal::ModalQueue;
use crate::state::modal::ModalState;
use crate::theme::Theme;
use crate::theme::ThemeRuntimeState;
use crate::theme::ThemeSetting;
use coco_tui_ui::constants;
use coco_tui_ui::double_press::DoublePressTracker;

/// Exit keys subject to double-press confirmation.
///
/// The variant labels (`"Ctrl-C"` / `"Ctrl-D"`) match the string
/// values used in footer copy and i18n substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKey {
    /// Ctrl+C — interrupt the running task on the first press and exit
    /// on a second press within the window.
    CtrlC,
    /// Ctrl+D — arm-only on the first press, exit on the second.
    CtrlD,
}

impl ExitKey {
    /// Human-readable label used in the "Press X again to exit" hint.
    pub fn label(self) -> &'static str {
        match self {
            Self::CtrlC => "Ctrl-C",
            Self::CtrlD => "Ctrl-D",
        }
    }
}

/// UI-only local state.
#[derive(Debug)]
pub struct UiState {
    /// Multi-line input state.
    pub input: InputState,
    /// Active Ctrl+R reverse-i-search session, or `None` when not searching.
    pub history_search: Option<HistorySearch>,
    /// One-shot flag: the "ctrl+r to search history" hint has been shown
    /// this session (after the user up-arrowed past the newest entry).
    pub search_hint_shown: bool,
    /// Paste pill manager for tracking pasted content (text and images).
    pub paste_manager: coco_tui_ui::paste::PasteManager,
    /// Chat scroll offset (lines from bottom).
    pub scroll_offset: i32,
    /// Current focus target.
    pub focus: FocusTarget,
    /// Selected row in the agent-switcher rail. `0` = `◯ main`; `1..` index
    /// the running subagents in rail order. Only meaningful while
    /// `focus == FocusTarget::AgentSwitcher`.
    pub agent_switcher_selected: usize,
    /// Bottom interaction pane: composer popups and prompt-class surfaces.
    pub interaction: InteractionPaneState,
    /// Active full-screen modal.
    pub modal: Option<ModalState>,
    /// Queued full-screen modals awaiting display.
    pub modal_queue: ModalQueue,
    /// Monotonic identity for the currently active blocking surface.
    ///
    /// Incremented only when the active prompt/modal changes, not when a
    /// renderer mutates selection/filter state inside the same surface.
    surface_generation: u64,
    /// Active streaming content.
    pub streaming: Option<StreamingState>,
    /// Whether thinking content is visible.
    pub show_thinking: bool,
    /// Whether system reminders are visible (debug).
    pub show_system_reminders: bool,
    /// Whether user has manually scrolled.
    pub user_scrolled: bool,
    /// Current theme.
    pub theme: Theme,
    /// Runtime theme registry + persisted setting snapshot.
    pub theme_state: ThemeRuntimeState,
    /// Display preferences derived from settings.json.
    pub display_settings: DisplaySettings,
    /// Runtime state for command-backed `statusLine` output.
    pub(crate) status_line: crate::status_bar::StatusLineRuntime,
    /// Active toast notifications.
    pub toasts: VecDeque<Toast>,
    /// `/skills` dialog Enter → CLI write-local round-trip is in
    /// flight; the `total_edits` count the dialog computed at
    /// dispatch lives here so the [`TuiOnlyEvent::SkillOverridesSaved`]
    /// handler can render the localized "Updated N override(s)"
    /// toast without the count crossing the wire. `None` when no
    /// dialog save is pending.
    pub pending_skills_save_edits: Option<usize>,
    /// Status-bar warning for terminal compatibility downgrades.
    pub terminal_compatibility_warning: Option<String>,
    /// IDs of collapsed tool calls.
    pub collapsed_tools: HashSet<String>,
    /// Help state scroll position.
    pub help_scroll: i32,
    /// Last terminal size reported by crossterm. Used by update logic for
    /// page-size decisions without reading render-derived metrics.
    pub(crate) terminal_size: Size,
    /// Double-press tracker for Ctrl+C → exit. Independent from
    /// [`ctrl_d_tracker`] so a "Ctrl+C, Ctrl+D, Ctrl+C" sequence within
    /// the window still completes the Ctrl+C double-press.
    pub ctrl_c_tracker: DoublePressTracker<()>,
    /// Double-press tracker for Ctrl+D → exit. See [`ctrl_c_tracker`].
    pub ctrl_d_tracker: DoublePressTracker<()>,
    /// Double-press tracker for Esc → Rewind state. The Esc keystroke
    /// itself fires `TuiCommand::Cancel` on every press; this tracker
    /// only controls whether the second Esc opens the rewind picker.
    pub esc_tracker: DoublePressTracker<()>,
    /// Whether the terminal window currently has focus. Used to gate
    /// turn-complete notifications so they only fire when the user has
    /// switched away.
    pub terminal_focused: bool,
    /// Last time the retained native surface was known to be visible.
    ///
    /// This intentionally excludes generic lifecycle timestamps like turn
    /// completion. Native scrollback visibility is only considered known after
    /// an app-directed key/paste interaction or after focus gain is followed by
    /// a successful retained-surface draw.
    pub surface_visibility_known_at: Option<Instant>,
    surface_visibility_confirmation_pending: bool,
    /// Platform clipboard lease held alive for the lifetime of the TUI. On
    /// Linux/X11 and some Wayland setups the clipboard is served by the
    /// process that wrote it, so dropping the `arboard::Clipboard` handle
    /// would wipe the copied text. The lease is `None` on other platforms
    /// and on the OSC 52 path where no in-process ownership is required.
    pub clipboard_lease: Option<coco_tui_ui::clipboard_copy::ClipboardLease>,
    /// Active autocomplete suggestions and request-key bookkeeping.
    ///
    /// `completion.active` drives the keybinding bridge into `Autocomplete`
    /// context and renders a popup above the input. Recomputed after every
    /// input mutation in `autocomplete::refresh_suggestions`.
    pub completion: crate::completion::CompletionState,
    /// Keybinding resolver + warnings + display platform. Cheap to clone
    /// (`Arc` internally). Defaults to a from-defaults handle; the
    /// CLI bootstrap (`tui_runner`) replaces it with a watcher-backed
    /// handle so `~/.coco/keybindings.json` customizations + hot reload
    /// take effect.
    ///
    /// Lives in state (not a process-wide global) so each test gets
    /// its own handle and `cargo test --lib` runs without
    /// `serial_test` guards.
    pub kb_handle: KeybindingHandle,
    /// Whether teammate spinner lines show recent message preview.
    /// Default false. Toggled via `app:toggleTeammatePreview` (Ctrl+Shift+O).
    pub show_teammate_message_preview: bool,
    /// Whether subagent activity renders the coordinator task view.
    /// Resolved by the CLI runner from runtime feature gates and env
    /// before rendering, so the view remains deterministic from state.
    pub coordinator_mode_active: bool,
    /// Stashed input draft from `chat:stash` (Ctrl+S in defaults).
    ///
    /// Single-slot push/pop semantics. Three cases:
    /// * empty input + stash present → pop stash into input
    /// * non-empty input → push to stash (overwriting any prior),
    ///   clear input
    /// * empty input + empty stash → silent no-op
    pub stashed_input: Option<StashedInput>,
    /// UI-only ephemera (spinner verb, status-clock pause accumulators,
    /// task-panel completion timestamps + all-completed anchor). These
    /// previously lived on [`crate::state::SessionState`]; they have no
    /// agent/engine consumer, so keeping them here keeps SessionState
    /// focused on conversation state and lets the renderer find them
    /// next to other UI ephemera.
    pub(crate) ephemeral: crate::state::ui_ephemeral::UiEphemeralState,
}

/// One slot of stashed input: text + cursor + paste-manager state.
#[derive(Debug, Clone)]
pub struct StashedInput {
    /// Stashed text content.
    pub text: String,
    /// Cursor byte offset at stash time. Restored alongside `text` on pop.
    /// In-memory only (no on-disk persistence), so the encoding change
    /// from char-index → byte-offset doesn't require migration.
    pub cursor_byte: usize,
    /// Snapshot of paste-pill entries at stash time. Restored on pop
    /// so pill labels in the stashed `text` (e.g. `[Pasted text #1]`)
    /// still resolve to the original content. Empty `Vec` when the
    /// user hadn't pasted anything.
    pub paste_entries: Vec<coco_tui_ui::paste::PasteEntry>,
}

impl UiState {
    /// Create a new default UI state.
    pub fn new() -> Self {
        let theme_state = ThemeRuntimeState::default();
        Self {
            input: InputState::new(),
            history_search: None,
            search_hint_shown: false,
            paste_manager: coco_tui_ui::paste::PasteManager::new(),
            scroll_offset: 0,
            focus: FocusTarget::Input,
            agent_switcher_selected: 0,
            interaction: InteractionPaneState::new(),
            modal: None,
            modal_queue: ModalQueue::default(),
            surface_generation: 0,
            streaming: None,
            show_thinking: false,
            show_system_reminders: false,
            user_scrolled: false,
            theme: theme_state.theme.clone(),
            theme_state,
            display_settings: DisplaySettings::default(),
            status_line: crate::status_bar::StatusLineRuntime::default(),
            toasts: VecDeque::new(),
            pending_skills_save_edits: None,
            terminal_compatibility_warning: None,
            collapsed_tools: HashSet::new(),
            help_scroll: 0,
            terminal_size: Size::new(80, 24),
            ctrl_c_tracker: DoublePressTracker::new(constants::DOUBLE_PRESS_TIMEOUT),
            ctrl_d_tracker: DoublePressTracker::new(constants::DOUBLE_PRESS_TIMEOUT),
            esc_tracker: DoublePressTracker::new(constants::DOUBLE_PRESS_TIMEOUT),
            terminal_focused: true,
            surface_visibility_known_at: None,
            surface_visibility_confirmation_pending: false,
            clipboard_lease: None,
            completion: crate::completion::CompletionState::default(),
            kb_handle: KeybindingHandle::from_defaults(),
            stashed_input: None,
            show_teammate_message_preview: false,
            coordinator_mode_active: false,
            ephemeral: crate::state::ui_ephemeral::UiEphemeralState::new(),
        }
    }

    /// Install a resolved theme. Single chokepoint: quantize RGB palettes to
    /// the terminal's color capability so every theme path — initial load AND
    /// in-app switches (which resolve a fresh, un-downsampled `Theme`) — gets
    /// adapted. `downsample` is idempotent. `log` is `false` on the live-preview
    /// path so navigating the theme picker doesn't write an INFO line per
    /// keypress; commit/boot paths pass `true`.
    fn install_theme_runtime(&mut self, theme_state: ThemeRuntimeState, log: bool) {
        self.theme = theme_state.theme.clone();
        let capability = coco_tui_ui::color::color_capability();
        self.theme.downsample(capability);
        if log {
            // Record the resolved theme + color depth + terminal env so the
            // active palette is diagnosable from the log. Named theme colors
            // (e.g. `accent`) are NOT downsampled and render via the terminal's
            // own 16-color palette — a custom palette (magenta→red) recolors them.
            tracing::info!(
                theme = %theme_state.active_id,
                setting = theme_state.setting.as_str(),
                color_capability = ?capability,
                term = %std::env::var("TERM").unwrap_or_default(),
                colorterm = %std::env::var("COLORTERM").unwrap_or_default(),
                colorfgbg = %std::env::var("COLORFGBG").unwrap_or_default(),
                "theme applied"
            );
        }
        self.theme_state = theme_state;
    }

    pub fn apply_theme_runtime(&mut self, theme_state: ThemeRuntimeState) {
        self.install_theme_runtime(theme_state, /*log*/ true);
    }

    pub fn apply_theme_setting(&mut self, setting: ThemeSetting) -> anyhow::Result<()> {
        let theme_state = self.theme_state.with_setting(setting)?;
        self.apply_theme_runtime(theme_state);
        Ok(())
    }

    /// Apply a theme for live preview — same install, but no INFO log (called
    /// per keypress while navigating the theme picker).
    pub fn preview_theme_setting(&mut self, setting: ThemeSetting) -> anyhow::Result<()> {
        let theme_state = self.theme_state.with_setting(setting)?;
        self.install_theme_runtime(theme_state, /*log*/ false);
        Ok(())
    }

    pub fn apply_display_settings(&mut self, display_settings: DisplaySettings) {
        let show_thinking_changed =
            self.display_settings.show_thinking != display_settings.show_thinking;
        let show_thinking = display_settings.show_thinking;
        self.display_settings = display_settings;
        if show_thinking_changed {
            self.show_thinking = show_thinking;
        }
        if let Some(ModalState::Settings(settings)) = self.modal.as_mut() {
            settings.set_display_settings(self.display_settings.clone());
        }
    }

    pub fn show_modal(&mut self, modal: ModalState) {
        match self.modal.take() {
            None => {
                self.modal = Some(modal);
                self.bump_surface_generation();
            }
            Some(current) if modal.priority() < current.priority() => {
                self.modal = Some(modal);
                self.modal_queue.push(current);
                self.bump_surface_generation();
            }
            Some(current) => {
                self.modal = Some(current);
                self.modal_queue.push(modal);
            }
        }
    }

    pub fn dismiss_modal(&mut self) {
        if self.modal.is_some() || !self.modal_queue.is_empty() {
            self.modal = self.modal_queue.pop_front();
            self.bump_surface_generation();
        }
    }

    pub fn push_prompt(&mut self, prompt: PanePromptState) {
        let had_active = self.interaction.active_prompt.is_some();
        self.completion.clear_active();
        self.interaction.popup = None;
        self.interaction.push_prompt(prompt);
        if !had_active {
            self.bump_surface_generation();
        }
    }

    pub fn dismiss_prompt(&mut self) {
        if self.interaction.active_prompt.is_some() || !self.interaction.prompt_queue.is_empty() {
            self.interaction.dismiss_active_prompt();
            self.bump_surface_generation();
        }
    }

    pub fn take_modal(&mut self) -> Option<ModalState> {
        self.modal.take()
    }

    pub fn restore_modal(&mut self, modal: ModalState) {
        self.modal = Some(modal);
    }

    pub fn finish_taken_modal(&mut self) {
        self.modal = self.modal_queue.pop_front();
        self.bump_surface_generation();
    }

    pub fn take_prompt(&mut self) -> Option<PanePromptState> {
        self.interaction.active_prompt.take()
    }

    pub fn restore_prompt(&mut self, prompt: PanePromptState) {
        self.interaction.active_prompt = Some(prompt);
    }

    pub fn finish_taken_prompt(&mut self) {
        self.interaction.active_prompt = self.interaction.prompt_queue.pop_front();
        self.bump_surface_generation();
    }

    pub fn has_active_surface(&self) -> bool {
        self.modal.is_some()
            || self.interaction.active_prompt.is_some()
            || self.interaction.popup.is_some()
    }

    pub fn has_blocking_interaction(&self) -> bool {
        self.modal.is_some() || self.interaction.active_prompt.is_some()
    }

    pub fn push_delayed_permission(
        &mut self,
        prompt: crate::state::PermissionPromptState,
        now: Instant,
    ) {
        self.interaction.push_permission(prompt, now);
    }

    pub fn flush_delayed_permissions(&mut self, now: Instant) -> bool {
        let mut changed = false;
        while let Some(prompt) = self.interaction.pop_ready_permission(now) {
            self.push_prompt(PanePromptState::Permission(prompt));
            changed = true;
        }
        changed
    }

    pub fn sync_popup_from_active_suggestions(&mut self) {
        self.interaction.popup = if self.interaction.active_prompt.is_some() {
            None
        } else {
            self.completion
                .active
                .as_ref()
                .map(|suggestions| match suggestions.kind {
                    crate::completion::SuggestionKind::SlashCommand => {
                        ComposerPopupState::Slash(SlashPopupState)
                    }
                    crate::completion::SuggestionKind::At
                    | crate::completion::SuggestionKind::Path
                    | crate::completion::SuggestionKind::Directory
                    | crate::completion::SuggestionKind::CustomTitle => {
                        ComposerPopupState::At(AtPopupState)
                    }
                    crate::completion::SuggestionKind::Symbol => {
                        ComposerPopupState::Symbol(SymbolPopupState)
                    }
                })
        };
    }

    pub fn surface_generation(&self) -> u64 {
        self.surface_generation
    }

    /// Clear active and queued interaction surfaces.
    pub fn clear_surfaces(&mut self) {
        let had_surface = self.modal.is_some()
            || !self.modal_queue.is_empty()
            || self.interaction.active_prompt.is_some()
            || !self.interaction.prompt_queue.is_empty()
            || self.interaction.popup.is_some();
        self.modal = None;
        self.modal_queue.clear();
        self.interaction.active_prompt = None;
        self.interaction.prompt_queue.clear();
        self.interaction.popup = None;
        self.completion.clear_all();
        if had_surface {
            self.bump_surface_generation();
        }
    }

    /// Record a key/paste interaction routed to the retained surface.
    pub fn record_surface_interaction(&mut self, now: Instant) {
        self.surface_visibility_known_at = Some(now);
        self.surface_visibility_confirmation_pending = false;
    }

    /// Focus gain only proves visibility after a successful retained draw.
    pub fn request_surface_visibility_confirmation(&mut self) {
        self.surface_visibility_confirmation_pending = true;
    }

    pub fn clear_surface_visibility_confirmation(&mut self) {
        self.surface_visibility_confirmation_pending = false;
    }

    pub fn confirm_surface_visibility_after_draw(&mut self, now: Instant) {
        if self.surface_visibility_confirmation_pending {
            self.surface_visibility_known_at = Some(now);
            self.surface_visibility_confirmation_pending = false;
        }
    }

    fn bump_surface_generation(&mut self) {
        self.surface_generation = self.surface_generation.wrapping_add(1);
    }

    /// Whether there are active toasts.
    pub fn has_toasts(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Add a toast notification.
    pub fn add_toast(&mut self, toast: Toast) {
        if self.toasts.len() >= constants::MAX_TOASTS as usize {
            self.toasts.pop_front();
        }
        self.toasts.push_back(toast);
    }

    /// Remove expired toasts.
    pub fn expire_toasts(&mut self) {
        self.toasts.retain(|t| !t.is_expired());
    }

    /// Which exit key is currently armed for double-press confirmation.
    /// When both trackers are armed (uncommon but possible if the user
    /// alternates Ctrl+C/Ctrl+D), the most recently armed key wins so
    /// the hint reflects the latest keystroke — only one prompt visible.
    pub fn pending_exit_hint(&self) -> Option<ExitKey> {
        match (
            self.ctrl_c_tracker.pending_until(),
            self.ctrl_d_tracker.pending_until(),
        ) {
            (Some(cu), Some(du)) => Some(if du > cu {
                ExitKey::CtrlD
            } else {
                ExitKey::CtrlC
            }),
            (Some(_), None) => Some(ExitKey::CtrlC),
            (None, Some(_)) => Some(ExitKey::CtrlD),
            (None, None) => None,
        }
    }

    /// Advance both exit trackers and the Esc tracker. Returns `true`
    /// if any tracker just expired (the caller should request a redraw
    /// so the "press again" hint disappears).
    pub fn tick_double_press(&mut self, now: Instant) -> bool {
        let ctrl_c = self.ctrl_c_tracker.tick(now);
        let ctrl_d = self.ctrl_d_tracker.tick(now);
        let esc = self.esc_tracker.tick(now);
        ctrl_c || ctrl_d || esc
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Current focus target in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusTarget {
    #[default]
    Input,
    Chat,
    /// The footer background-tasks pill ("N shells"). Reached by pressing
    /// Down at the bottom of the composer history; Enter opens the
    /// background-tasks dialog, Up/Esc returns focus to the input.
    FooterShells,
    /// The inline agent-switcher rail above the composer (`◯ main` + running
    /// subagents). Reached with `Shift+↑`; `↑/↓` move the selection, `Enter`
    /// views the selected agent (or returns to main), `x` stops it, and
    /// `Esc` / `Shift+↓` / typing returns focus to the input.
    AgentSwitcher,
}

/// A single composer history entry.
///
/// History is ordered most-recent-first (index 0 = newest), matching
/// shell / codex / claude-code up-arrow recall. Re-submitting an existing
/// text moves it back to the front rather than creating a duplicate.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub text: String,
    /// Paste-pill payloads referenced by `text`, snapshotted at submit so
    /// recalling the entry rehydrates the paste manager — without this a
    /// recalled `[Pasted text #N]` is a dangling token (the manager was
    /// cleared at submit) and resolves to literal text. Mirrors codex
    /// `HistoryEntry.pending_pastes`. Empty for entries hydrated from
    /// persistent cross-session history (text-only, like codex).
    pub pastes: Vec<coco_tui_ui::paste::PasteEntry>,
}

/// Active Ctrl+R reverse-i-search session over the in-memory composer
/// history. Mirrors codex's inline `reverse-i-search:` UX: typing edits
/// `query`, Ctrl+R/↑ steps to older matches, ↓ to newer, Enter accepts
/// the previewed entry into the composer, Esc restores the saved draft.
#[derive(Debug, Clone)]
pub struct HistorySearch {
    /// Search query shown in the composer's bottom-border footer.
    pub query: String,
    /// Index into [`InputState::history`] of the currently previewed
    /// match, or `None` when the query has no match yet.
    pub matched: Option<usize>,
    /// Draft text to restore on cancel / no-match.
    pub original_text: String,
    /// Paste-manager snapshot to restore alongside `original_text`.
    pub original_pastes: Vec<coco_tui_ui::paste::PasteEntry>,
    /// `history_index` to restore on cancel.
    pub original_history_index: Option<usize>,
}

/// Input prefix mode — derived from the leading character of [`InputState::text`].
///
/// * `Bash` (`!` prefix): submit bypasses the model loop and runs the
///   shell directly.
/// * Memory capture uses the `/memory` slash command and file picker.
///   Leading `#` is ordinary chat text.
///
/// The mode is computed on the fly so backspacing past the prefix
/// character returns to `Normal` automatically — no separate state to
/// keep in sync.
///
/// Plan mode is *not* a prompt mode; it's a permission mode set via
/// `Shift+Tab` cycle and shown in the input title, not via prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptMode {
    /// Standard chat input.
    #[default]
    Normal,
    /// Leading `!` — submit runs as a shell command.
    Bash,
}

impl PromptMode {
    /// Compute the mode from the leading character of `text`.
    ///
    /// Returns `Normal` for empty input and for anything that doesn't
    /// match a known prefix. Whitespace before the prefix disqualifies
    /// the mode.
    pub fn from_text(text: &str) -> Self {
        match text.as_bytes().first() {
            Some(b'!') => Self::Bash,
            _ => Self::Normal,
        }
    }

    /// Strip the mode prefix from `text` (including one optional space
    /// after it). Returns `text` unchanged for `Normal`.
    ///
    /// Used at submit time so `!ls -la` becomes the command `ls -la`.
    pub fn strip_prefix(self, text: &str) -> &str {
        match self {
            Self::Normal => text,
            Self::Bash => {
                let stripped = &text[1..];
                stripped.strip_prefix(' ').unwrap_or(stripped)
            }
        }
    }

    /// i18n key for the input title shown when this mode is active.
    pub fn title_i18n_key(self) -> &'static str {
        match self {
            Self::Normal => "input.title",
            Self::Bash => "input.title_bash_mode",
        }
    }
}

/// Multi-line input state.
///
/// Backed by a [`TextArea`] (byte-offset cursor, multi-line wrapped,
/// grapheme + display-width aware) so the cursor renders correctly over
/// CJK / wide characters and multi-line input wraps. Most-recent-first
/// history + vim runtime live alongside it.
#[derive(Debug)]
pub struct InputState {
    /// The editable buffer + cursor. Edit it directly for byte-offset
    /// access; the surrounding `InputState` API only owns history + vim.
    pub textarea: coco_tui_ui::widgets::TextArea,
    /// Dim, non-editable text rendered after the input cursor. Used for
    /// slash-command argument hints after accepting a command completion.
    pub inline_hint: Option<String>,
    /// Dim ghost text rendered at the cursor. It is not part of the
    /// editable buffer; accepting it applies [`InlineGhost::replacement`]
    /// over [`InlineGhost::replace_start`]..[`InlineGhost::replace_end`].
    pub inline_ghost: Option<InlineGhost>,
    /// Composer history ordered most-recent-first (index 0 = newest).
    pub history: Vec<HistoryEntry>,
    /// Current history navigation index into `history` (None = live draft).
    pub history_index: Option<usize>,
    /// Vim modal-editing runtime: state machine + persistent register.
    pub vim: crate::vim::VimRuntime,
}

/// Inline typeahead completion rendered inside the editable input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineGhost {
    pub text: String,
    pub insert_position: usize,
    pub replace_start: usize,
    pub replace_end: usize,
    pub replacement: String,
    pub cursor_after_accept: usize,
}

impl InputState {
    /// Create empty input.
    pub fn new() -> Self {
        Self {
            textarea: coco_tui_ui::widgets::TextArea::new(),
            inline_hint: None,
            inline_ghost: None,
            history: Vec::new(),
            history_index: None,
            vim: crate::vim::VimRuntime::new(),
        }
    }

    /// Current text content.
    pub fn text(&self) -> &str {
        self.textarea.text()
    }

    /// Replace the entire input. Resets `history_index` so subsequent
    /// Up/Down navigation restarts from the live draft.
    pub fn set_text(&mut self, text: &str) {
        self.textarea.set_text(text);
        self.inline_hint = None;
        self.inline_ghost = None;
        self.history_index = None;
    }

    /// Take the current input, clearing the buffer.
    pub fn take_input(&mut self) -> String {
        self.history_index = None;
        self.inline_hint = None;
        self.inline_ghost = None;
        self.textarea.take_text()
    }

    /// Set a dim inline hint rendered after the cursor. The hint is not
    /// part of the editable buffer or submitted text.
    pub fn set_inline_hint(&mut self, hint: impl Into<String>) {
        let hint = hint.into();
        self.inline_hint = (!hint.is_empty()).then_some(hint);
    }

    /// Clear any dim inline hint shown after the editable input.
    pub fn clear_inline_hint(&mut self) {
        self.inline_hint = None;
    }

    pub fn set_inline_ghost(&mut self, ghost: InlineGhost) {
        self.inline_ghost = (!ghost.text.is_empty()).then_some(ghost);
    }

    pub fn clear_inline_ghost(&mut self) {
        self.inline_ghost = None;
    }

    pub fn active_inline_ghost(&self) -> Option<&InlineGhost> {
        self.inline_ghost
            .as_ref()
            .filter(|ghost| ghost.insert_position == self.textarea.cursor())
    }

    pub fn accept_inline_ghost(&mut self) -> bool {
        let Some(ghost) = self.active_inline_ghost().cloned() else {
            self.clear_inline_ghost();
            return false;
        };
        self.textarea
            .replace_range(ghost.replace_start..ghost.replace_end, &ghost.replacement);
        self.textarea.set_cursor(ghost.cursor_after_accept);
        self.clear_inline_ghost();
        true
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    /// Current prompt-prefix mode (derived from leading character).
    pub fn prompt_mode(&self) -> PromptMode {
        PromptMode::from_text(self.textarea.text())
    }

    /// Seed history from persistent cross-session storage at startup.
    ///
    /// `texts` arrives newest-first (as `PromptHistory::get_history`
    /// returns it). Deduped keeping the first (newest) occurrence and
    /// capped at [`constants::MAX_HISTORY_ENTRIES`]. Hydrated entries are
    /// text-only (no paste pills), matching codex's cross-session recall.
    pub fn hydrate_history(&mut self, texts: Vec<String>) {
        let max = constants::MAX_HISTORY_ENTRIES as usize;
        let mut seen = std::collections::HashSet::new();
        self.history = texts
            .into_iter()
            .filter(|t| !t.is_empty() && seen.insert(t.clone()))
            .take(max)
            .map(|text| HistoryEntry {
                text,
                pastes: Vec::new(),
            })
            .collect();
    }

    /// Record a submitted text into history, most-recent-first.
    ///
    /// If the text already exists it is moved back to the front (no
    /// duplicate); otherwise it is prepended. Capped at
    /// [`constants::MAX_HISTORY_ENTRIES`] by dropping the oldest tail.
    pub fn add_to_history(&mut self, text: String) {
        self.add_to_history_with_pastes(text, Vec::new());
    }

    /// [`Self::add_to_history`] carrying the paste-pill payloads referenced
    /// by `text`. A same-text resubmit replaces the stored pastes (pill
    /// numbering restarts after each clear, so equal display text can
    /// reference different payloads).
    pub fn add_to_history_with_pastes(
        &mut self,
        text: String,
        pastes: Vec<coco_tui_ui::paste::PasteEntry>,
    ) {
        if text.is_empty() {
            return;
        }
        // Drop any prior occurrence, then prepend so the most recent
        // submission sits at index 0 for up-arrow recall.
        self.history.retain(|h| h.text != text);
        self.history.insert(0, HistoryEntry { text, pastes });
        let max = constants::MAX_HISTORY_ENTRIES as usize;
        if self.history.len() > max {
            self.history.truncate(max);
        }
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming display state.
#[derive(Debug, Clone)]
pub struct StreamingState {
    /// Accumulated text content.
    pub content: String,
    /// Accumulated thinking content.
    pub thinking: String,
    /// When the *current stream segment* started. NOT the turn start —
    /// `streaming` is re-instantiated across mid-turn segments
    /// (assistant→tool→assistant, retries, etc.) via
    /// `get_or_insert_with(StreamingState::new)`, so this clock resets
    /// inside a single turn. Use `session.current_turn_started_at`
    /// when you want the whole-turn duration.
    pub segment_started_at: Instant,
    /// Current streaming mode.
    pub mode: StreamMode,
    /// Display cursor position for adaptive pacing.
    pub display_cursor: usize,
    /// Identity of the *visible* document (`visible_content`), drawn from a
    /// process-global counter: a fresh value on construction and after every
    /// cursor move. Equal generations therefore guarantee byte-identical
    /// visible content — even across instances (a coalesced turn boundary
    /// swaps the whole state) — letting the stream renderer skip its O(doc)
    /// prefix scan on no-reveal frames (spinner ticks).
    pub visible_generation: u64,
}

/// Next process-globally-unique visible-document generation.
fn next_stream_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Current streaming content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    Text,
    Thinking,
    ToolUse,
}

impl StreamingState {
    /// Create a new streaming state.
    pub fn new() -> Self {
        Self {
            content: String::new(),
            thinking: String::new(),
            segment_started_at: Instant::now(),
            mode: StreamMode::Text,
            display_cursor: 0,
            visible_generation: next_stream_generation(),
        }
    }

    /// Append text delta.
    pub fn append_text(&mut self, delta: &str) {
        self.content.push_str(delta);
        self.mode = StreamMode::Text;
    }

    /// Append thinking delta.
    pub fn append_thinking(&mut self, delta: &str) {
        self.thinking.push_str(delta);
        self.mode = StreamMode::Thinking;
    }

    /// Get visible content up to display cursor.
    pub fn visible_content(&self) -> &str {
        let end = self.display_cursor.min(self.content.len());
        &self.content[..end]
    }

    /// Reveal one COMPLETE (newline-terminated) line per spinner tick; returns
    /// whether the cursor moved.
    ///
    /// The in-progress trailing line is held back until its `\n` arrives. A
    /// partial line has an unknown wrapped height, so revealing it would jitter
    /// the live region (and the viewport bottom) on every delta; gating on the
    /// newline keeps the live region's height changing only at line boundaries.
    /// The final partial line never needs a reveal: at finalize the streaming
    /// overlay is dropped (`MessageAppended`) and the committed cell render
    /// shows the full canonical text.
    pub fn advance_display(&mut self) -> bool {
        match self.content[self.display_cursor..].find('\n') {
            Some(idx) => {
                self.display_cursor += idx + 1;
                self.visible_generation = next_stream_generation();
                true
            }
            None => false,
        }
    }

    /// Reveal all content immediately — test/bench setup helper to skip the
    /// per-tick reveal pacing. Production never calls this; the overlay is
    /// dropped at finalize instead.
    #[cfg(any(test, feature = "testing"))]
    pub fn reveal_all(&mut self) {
        self.display_cursor = self.content.len();
        self.visible_generation = next_stream_generation();
    }
}

impl Default for StreamingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub severity: ToastSeverity,
    pub created_at: Instant,
    pub duration: Duration,
}

/// Toast severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

impl Toast {
    /// Create an info toast.
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Info,
            created_at: Instant::now(),
            duration: constants::TOAST_INFO_DURATION,
        }
    }

    /// Create a success toast.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Success,
            created_at: Instant::now(),
            duration: constants::TOAST_SUCCESS_DURATION,
        }
    }

    /// Create a warning toast.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Warning,
            created_at: Instant::now(),
            duration: constants::TOAST_WARNING_DURATION,
        }
    }

    /// Create an error toast.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Error,
            created_at: Instant::now(),
            duration: constants::TOAST_ERROR_DURATION,
        }
    }

    /// Whether the toast has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration
    }

    /// Remaining percentage (1.0 = full, 0.0 = expired).
    pub fn remaining_pct(&self) -> f64 {
        let elapsed = self.created_at.elapsed().as_secs_f64();
        let total = self.duration.as_secs_f64();
        (1.0 - elapsed / total).max(0.0)
    }
}

#[cfg(test)]
#[path = "ui.test.rs"]
mod tests;
