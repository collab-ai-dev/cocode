//! Keybinding bridge — maps key events to TUI commands.
//!
//! Determines the active keybinding context from state, then resolves
//! key events to commands. Context priority:
//!
//!   state > autocomplete > global > input

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;

/// Keybinding context — determines which key mappings are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindingContext {
    /// Permission/approval state — Y/N/A approval.
    Confirmation,
    /// AskUserQuestion multi-choice state. Distinct from [`Self::Confirmation`]
    /// because a question is answered by selecting an option (Enter), never by
    /// Y/N/A — routing it through the confirmation map would let the first
    /// letter of an answer silently approve/deny and tear the prompt down, and
    /// would leave the question free-text input unreachable.
    Question,
    /// Filterable list state (model picker, command palette, etc.).
    Picker,
    /// Model picker state — filterable list plus effort/role controls.
    ModelPicker,
    /// Teams roster picker — list nav plus ←/→ mode cycling (gap 8).
    TeamRoster,
    /// Scrollable content state (help, diff view, task detail, doctor).
    Scrollable,
    /// Transcript reader state.
    Transcript,
    /// Autocomplete suggestions visible.
    Autocomplete,
    /// Tabbed settings state — Tab/Shift+Tab cycle tabs, Up/Down nav.
    Settings,
    /// Theme tab inside Settings — includes theme-picker-specific actions.
    ThemePicker,
    /// `/permissions` rule-editor overlay. Dedicated context (like
    /// [`Self::Question`]) so the add-form text input is reachable and
    /// list-mode characters aren't hijacked as y/n/a confirmation actions —
    /// the keys map to input-style commands the editor's `intercept`
    /// consumes (Cursor* / InsertChar / SubmitInput).
    PermissionsEditor,
    /// `/add-dir` (no-arg) directory-input overlay. Dedicated context (same
    /// shape as [`Self::PermissionsEditor`]): a single-line text field whose
    /// keys map to `Cursor* / InsertChar / SubmitInput / Cancel` for
    /// `add_directory::intercept`. Global-only `context_stack` keeps chars /
    /// arrows from resolving to filter / Surface* / y-n-a.
    AddDirectory,
    /// Editable "always allow" prefix on a shell permission prompt. Active only
    /// while an allow row is focused (mirrors [`Self::PermissionsEditor`]): plain
    /// chars become `InsertChar` and arrows `Cursor*` so the rule field is
    /// editable, rather than being hijacked as y/n/a confirmation actions.
    PermissionPrefixEdit,
    /// Background-tasks dialog (footer "N agents" pill → full-screen list +
    /// per-task detail). Dedicated context because the generic
    /// [`Self::Confirmation`] map emits `Surface{Prev,Next,Confirm}` and has no
    /// `x`/`←` arms, but `update::background_tasks::intercept` consumes
    /// `Cursor{Up,Down,Left}` / `SubmitInput` / `InsertChar('x')` — so routing
    /// it through confirmation left ↑/↓/Enter/x/← all dead (only Esc closed).
    BackgroundTasks,
    /// Default chat input context.
    Chat,
}

/// Determine the active keybinding context from state.
pub fn active_context(state: &AppState) -> KeybindingContext {
    if let Some(modal) = state.ui.modal.as_ref() {
        return match modal {
            // Filterable list modals
            ModalState::ModelPicker(_) => KeybindingContext::ModelPicker,
            ModalState::TeamRoster(_) => KeybindingContext::TeamRoster,

            // Standalone theme picker — reuses the ThemePicker context so the
            // `theme:toggleSyntaxHighlighting` (ctrl+t) binding is active.
            ModalState::ThemePicker(_) => KeybindingContext::ThemePicker,

            // Filterable list modals
            ModalState::SessionBrowser(_)
            | ModalState::GlobalSearch(_)
            | ModalState::QuickOpen(_)
            | ModalState::Export(_)
            | ModalState::Feedback(_)
            | ModalState::McpServerSelect(_)
            | ModalState::CopyPicker(_)
            | ModalState::PluginHint(_)
            | ModalState::PluginDialog(_)
            | ModalState::Rewind(_) => KeybindingContext::Picker,

            // Scrollable read-only modals
            ModalState::Help
            | ModalState::DiffView(_)
            | ModalState::TaskDetail(_)
            | ModalState::Doctor(_) => KeybindingContext::Scrollable,
            ModalState::Transcript(_) => KeybindingContext::Transcript,

            // Tabbed settings state. The Display tab gets the ThemePicker
            // context so `theme:toggleSyntaxHighlighting` (ctrl+t) toggles
            // the syntax-highlighting row.
            ModalState::Settings(s)
                if s.active_tab == crate::widgets::settings_panel::SettingsTab::Display =>
            {
                KeybindingContext::ThemePicker
            }
            ModalState::Settings(_) => KeybindingContext::Settings,

            // `/permissions` editor — dedicated context for text input +
            // distinct nav (see the enum-variant doc).
            ModalState::PermissionsEditor(_) => KeybindingContext::PermissionsEditor,

            // `/add-dir` overlay — dedicated single-text-input context (see the
            // enum-variant doc); routes to `add_directory::map_key`.
            ModalState::AddDirectory(_) => KeybindingContext::AddDirectory,

            // Background-tasks list/detail — dedicated nav context so ↑/↓/Enter/
            // x/← reach the dialog's intercept (see the enum-variant doc).
            ModalState::BackgroundTasks(_) => KeybindingContext::BackgroundTasks,

            // All others are confirmation/approval surfaces
            _ => KeybindingContext::Confirmation,
        };
    }

    if matches!(
        state.ui.interaction.active_prompt,
        Some(PanePromptState::Question(_))
    ) {
        return KeybindingContext::Question;
    }

    // A shell permission prompt with an allow row focused: editing the rule
    // prefix takes over the keys (chars → InsertChar, not y/n/a).
    if let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_ref()
        && crate::permission_options::prefix_editing(p, state.session.permission_mode)
    {
        return KeybindingContext::PermissionPrefixEdit;
    }

    if matches!(
        state.ui.interaction.active_prompt,
        Some(
            PanePromptState::Permission(_)
                | PanePromptState::SandboxPermission(_)
                | PanePromptState::CostWarning(_)
                | PanePromptState::PlanEntry(_)
                | PanePromptState::PlanApproval(_)
                | PanePromptState::McpServerApproval(_)
        )
    ) {
        return KeybindingContext::Confirmation;
    }

    // Autocomplete popup active: Up/Down/Tab/Enter/Esc route to
    // suggestion navigation; all other keys fall through to normal input
    // editing.
    //
    // Gate on non-empty items — async triggers (File/Symbol) install the
    // query before search results arrive, and we must not hijack arrow
    // keys during that window.
    if state
        .ui
        .completion
        .active
        .as_ref()
        .is_some_and(|s| !s.items.is_empty())
    {
        return KeybindingContext::Autocomplete;
    }

    KeybindingContext::Chat
}

/// Map a key event to a TUI command based on the active context.
///
/// Resolution order:
///
/// 1. Run the [`coco_keybindings`] resolver against the TS default
///    bindings. If it fires an action with a TUI handler in
///    [`crate::keybinding_dispatch`], use it.
/// 2. If the resolver explicitly consumed the keystroke (chord
///    cancelled, null unbind), return `None` so it doesn't fall through.
/// 3. Otherwise, dispatch through the residual hardcoded arms — only keys
///    that CANNOT be rebindable defaults (`?`-on-empty-composer, per-surface
///    navigation maps, readline editing, PageUp/PageDown, F6); every former
///    "TUI-only shortcut" arm is folded into `coco-keybindings` defaults.
pub fn map_key(state: &AppState, key: KeyEvent) -> Option<TuiCommand> {
    let ctx = active_context(state);
    let (cmd, source) = resolve_key(state, key, ctx);
    if let Some(c) = cmd.as_ref()
        && should_log_key_command(ctx, c)
    {
        tracing::debug!(
            target: "coco_tui::keybinding",
            key = ?key.code,
            mods = ?key.modifiers,
            ctx = ?ctx,
            source,
            cmd = ?c,
            "key → TuiCommand",
        );
    }
    cmd
}

// In the `Chat` context the user is typing into the input editor — Backspace,
// arrows, char inserts, Enter-to-submit happen many times per message and
// drown out everything else at DEBUG. Suppress those; keep every command in
// modals / overlays / autocomplete where the same keys carry control intent.
fn should_log_key_command(ctx: KeybindingContext, cmd: &TuiCommand) -> bool {
    if ctx == KeybindingContext::Chat {
        return !matches!(
            cmd,
            TuiCommand::InsertChar(_)
                | TuiCommand::InsertNewline
                | TuiCommand::DeleteBackward
                | TuiCommand::DeleteForward
                | TuiCommand::DeleteWordBackward
                | TuiCommand::DeleteWordForward
                | TuiCommand::CursorLeft
                | TuiCommand::CursorRight
                | TuiCommand::CursorUp
                | TuiCommand::CursorDown
                | TuiCommand::CursorHome
                | TuiCommand::CursorEnd
                | TuiCommand::WordLeft
                | TuiCommand::WordRight
        );
    }
    !matches!(cmd, TuiCommand::InsertChar(_))
}

/// Inner resolution returning both the command and where it came from
/// (`resolver`, `cascade`, `pending`, `consumed`, `unmapped`). The
/// `source` tag is the breadcrumb that distinguishes "resolver knew
/// this and dispatched" from "resolver had nothing → cascade picked it
/// up" — critical when a user reports a customized binding misfiring.
fn resolve_key(
    state: &AppState,
    key: KeyEvent,
    ctx: KeybindingContext,
) -> (Option<TuiCommand>, &'static str) {
    if key.modifiers == KeyModifiers::CONTROL {
        match key.code {
            // Ctrl+C / Ctrl+D are non-rebindable process-level exit
            // keys. Handle them before context bindings so modal
            // shortcuts cannot swallow the second press after the
            // "Press X again to exit" hint is armed.
            KeyCode::Char('c' | 'C') => return (Some(TuiCommand::Interrupt), "reserved_exit"),
            KeyCode::Char('d' | 'D') => return (Some(TuiCommand::RequestExit), "reserved_exit"),
            _ => {}
        }
    }

    // Reverse-i-search owns every key (except the reserved exits above)
    // while active: typing edits the query, Ctrl+R/↑ and ↓ cycle matches,
    // Enter accepts, Esc cancels. Unmapped keys are swallowed so they
    // can't leak into composer editing mid-search.
    if state.ui.history_search.is_some() {
        return (map_history_search_key(key), "history_search");
    }

    if matches!(ctx, KeybindingContext::Transcript) {
        if matches!(key.code, KeyCode::BackTab) {
            return (None, "transcript_backtab");
        }
        if let Some(cmd) = crate::modal_pane::transcript_map_key(key) {
            return (Some(cmd), "transcript");
        }
    }

    if matches!(state.ui.modal, Some(ModalState::CopyPicker(_)))
        && key.modifiers == KeyModifiers::NONE
        && matches!(key.code, KeyCode::Char('w' | 'W'))
    {
        return (Some(TuiCommand::CopyPickerWriteToFile), "copy_picker");
    }

    // A stale prompt suggestion must not claim Enter/Tab/Right while a turn is
    // in flight — during a turn those keys belong to steering (Enter queues via
    // SubmitInput). Suggestions are normally cleared on turn start, so this is a
    // defensive guard; it mirrors TS, where the suggestion affordance is idle
    // while `queryGuard.isActive`.
    if matches!(ctx, KeybindingContext::Chat)
        && key.modifiers == KeyModifiers::NONE
        && !state.ui.ephemeral.turn_active()
        && prompt_suggestion_visible(state)
    {
        match key.code {
            KeyCode::Tab | KeyCode::Right => {
                return (
                    Some(TuiCommand::AcceptPromptSuggestion),
                    "prompt_suggestion",
                );
            }
            KeyCode::Enter => {
                return (
                    Some(TuiCommand::SubmitPromptSuggestion),
                    "prompt_suggestion",
                );
            }
            _ => {}
        }
    }

    // Layer 1: bindings from the resolver.
    match state.ui.kb_handle.resolve_key(key, ctx) {
        crate::keybinding_resolver::ResolverResult::Action(action) => {
            if let Some(cmd) = crate::keybinding_dispatch::dispatch_action(&action, state) {
                return (Some(cmd), "resolver");
            }
            // Resolver knew about this action but the TUI doesn't have
            // a handler yet. Swallow the keystroke instead of falling
            // through to the cascade — that path would do the wrong
            // thing for a user who customized the chord.
            return (None, "resolver_no_handler");
        }
        crate::keybinding_resolver::ResolverResult::Pending => {
            // The status bar reads the pending chord directly from
            // `kb_handle`, so no TuiCommand is needed for this key.
            return (None, "chord_pending");
        }
        crate::keybinding_resolver::ResolverResult::Consumed => return (None, "consumed"),
        crate::keybinding_resolver::ResolverResult::NotResolved => {}
    }

    // Layer 2: per-surface navigation maps. Bottom-pane prompt surfaces own
    // their key maps (`crate::bottom_pane`); modal surface maps live below.
    let cmd = match ctx {
        KeybindingContext::Confirmation => crate::bottom_pane::confirmation_map_key(state, key),
        KeybindingContext::Question => crate::bottom_pane::question::map_key(key),
        KeybindingContext::ModelPicker => crate::modal_pane::model_picker::map_key(key),
        KeybindingContext::TeamRoster => crate::modal_pane::team_roster::map_key(key),
        KeybindingContext::Picker => crate::modal_pane::picker_map_key(key),
        KeybindingContext::Scrollable => crate::modal_pane::scrollable_map_key(key),
        KeybindingContext::Transcript => crate::modal_pane::transcript_map_key(key),
        // Autocomplete intercepts navigation keys only; other keys fall
        // through to input editing so the user keeps typing and the
        // suggestion popup refreshes reactively.
        KeybindingContext::Autocomplete => map_autocomplete_key(key)
            .or_else(|| map_global_key(state, key))
            .or_else(|| map_input_key(state, key)),
        KeybindingContext::Settings | KeybindingContext::ThemePicker => {
            crate::modal_pane::settings::map_key(key)
        }
        KeybindingContext::PermissionsEditor => crate::modal_pane::permissions_editor::map_key(key),
        KeybindingContext::AddDirectory => crate::modal_pane::add_directory::map_key(key),
        KeybindingContext::PermissionPrefixEdit => permission_prefix_edit_map_key(key),
        KeybindingContext::BackgroundTasks => background_tasks_map_key(key),
        KeybindingContext::Chat => map_global_key(state, key).or_else(|| map_input_key(state, key)),
    };
    let source = if cmd.is_some() { "cascade" } else { "unmapped" };
    (cmd, source)
}

/// Keys for the editable always-allow prefix field on a shell permission
/// prompt. Mirrors the `/permissions` editor: chars insert, arrows move the
/// cursor, Enter commits the focused allow row, Esc denies, ↑/↓ leave the row.
fn permission_prefix_edit_map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Deny),
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Backspace if ctrl || alt => Some(TuiCommand::DeleteWordBackward),
        KeyCode::Backspace => Some(TuiCommand::DeleteBackward),
        KeyCode::Left => Some(TuiCommand::CursorLeft),
        KeyCode::Right => Some(TuiCommand::CursorRight),
        KeyCode::Home => Some(TuiCommand::CursorHome),
        KeyCode::End => Some(TuiCommand::CursorEnd),
        KeyCode::Char('a') if ctrl => Some(TuiCommand::CursorHome),
        KeyCode::Char('e') if ctrl => Some(TuiCommand::CursorEnd),
        KeyCode::Char('w') if ctrl => Some(TuiCommand::DeleteWordBackward),
        KeyCode::Char(c) if !ctrl && !alt => Some(TuiCommand::InsertChar(c)),
        _ => None,
    }
}

/// Keys for the background-tasks dialog. Emits exactly the commands
/// `update::background_tasks::intercept` consumes: ↑/↓ (k/j) move the list
/// cursor, Enter drills into / closes the focused task, ← steps back from the
/// detail view (or closes the list), Esc closes, and `x` stops the focused
/// task. Plain chars otherwise fall through (the dialog has no text input).
fn background_tasks_map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k' | 'K') => Some(TuiCommand::CursorUp),
        KeyCode::Down | KeyCode::Char('j' | 'J') => Some(TuiCommand::CursorDown),
        KeyCode::Enter => Some(TuiCommand::SubmitInput),
        KeyCode::Left => Some(TuiCommand::CursorLeft),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('x' | 'X') => Some(TuiCommand::InsertChar('x')),
        _ => None,
    }
}

/// Keys for the active Ctrl+R reverse-i-search session. Mirrors codex:
/// Ctrl+R / ↑ = older match, Ctrl+S / ↓ = newer, Enter/Tab = accept,
/// Esc = cancel, Backspace edits the query, any other printable char
/// appends to the query.
fn map_history_search_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => Some(TuiCommand::HistorySearchCancel),
        KeyCode::Enter | KeyCode::Tab => Some(TuiCommand::HistorySearchAccept),
        KeyCode::Up => Some(TuiCommand::HistorySearchOlder),
        KeyCode::Char('r' | 'R') if ctrl => Some(TuiCommand::HistorySearchOlder),
        KeyCode::Down => Some(TuiCommand::HistorySearchNewer),
        KeyCode::Char('s' | 'S') if ctrl => Some(TuiCommand::HistorySearchNewer),
        KeyCode::Backspace => Some(TuiCommand::HistorySearchBackspace),
        KeyCode::Char(c) if !ctrl && !alt => Some(TuiCommand::HistorySearchInput(c)),
        _ => None,
    }
}

/// Keys for autocomplete suggestions.
fn map_autocomplete_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::SurfacePrev)
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::SurfaceNext)
        }
        KeyCode::Tab => Some(TuiCommand::AutocompleteAccept),
        KeyCode::Enter => Some(TuiCommand::AutocompleteSubmit),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        _ => None,
    }
}

/// Residual hardcoded keys for the Chat context — only what CANNOT be a
/// rebindable default. Every former "TUI-only shortcut" arm has been folded
/// into `coco-keybindings` defaults as documented coco-rs extensions
/// (`app:forceQuit`, `app:help`, `app:commandPalette`, `app:settings`,
/// `chat:toggleSystemReminders`, `chat:togglePlanMode`, plus second default
/// bindings on existing actions: ctrl+f → `chat:killAgents`, ctrl+m →
/// `chat:modelPicker`, alt/ctrl+v → `chat:imagePaste`). The arms that the
/// resolver had already shadowed (ctrl+l → `app:redraw`, ctrl+shift+f →
/// `app:globalSearch`, ctrl+s → `chat:stash`, ctrl+g →
/// `chat:externalEditor`, shift+tab → `chat:cycleMode`, plus the
/// platform-primary paste key) were dead code and are simply gone.
///
/// What stays, and why it cannot be a binding:
/// - `?` opens help only on an empty composer; otherwise the key must fall
///   through to typing. The resolver swallows matched keys outright, so a
///   binding would eat `?` mid-sentence.
/// - PageUp/PageDown are viewport scrolling — navigation, not a shortcut
///   (the internal `Scroll` context is not part of the Chat stack).
/// - F6 focus cycling — navigation, same class as the per-surface maps.
fn map_global_key(state: &AppState, key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Char('?') if state.ui.input.is_empty() => Some(TuiCommand::ShowHelp),
        KeyCode::PageUp => Some(TuiCommand::PageUp),
        KeyCode::PageDown => Some(TuiCommand::PageDown),
        KeyCode::F(6) => Some(TuiCommand::FocusNext),
        _ => None,
    }
}

/// Input editing keys (lowest priority).
fn map_input_key(state: &AppState, key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // A turn is in flight for its whole lifecycle (stream + tool/subagent
    // execution), mirroring TS `queryGuard.isActive`. While it is, Enter must
    // route to `SubmitInput` (which queues/steers) rather than being claimed by
    // the prompt-suggestion arm below.
    let turn_active = state.ui.ephemeral.turn_active();

    match key.code {
        // Submit / queue — each match arm carries the structured `keymap`
        // entry id it implements; the keymap/ module is the source of
        // truth that `/help`, the help state, and subagent retrieval
        // all read from.
        // keymap = "input:newline"
        KeyCode::Enter if shift || alt => Some(TuiCommand::InsertNewline),
        KeyCode::Enter if turn_active => Some(TuiCommand::SubmitInput),
        KeyCode::Enter if prompt_suggestion_visible(state) => {
            Some(TuiCommand::SubmitPromptSuggestion)
        }
        // keymap = "input:submit"
        KeyCode::Enter => Some(TuiCommand::SubmitInput),

        // Editing
        // keymap = "input:delete_word_backward"
        KeyCode::Backspace if ctrl || alt => Some(TuiCommand::DeleteWordBackward),
        // keymap = "input:delete_backward"
        KeyCode::Backspace => Some(TuiCommand::DeleteBackward),
        // keymap = "input:delete_word_forward"
        KeyCode::Delete if ctrl => Some(TuiCommand::DeleteWordForward),
        // keymap = "input:delete_forward"
        KeyCode::Delete => Some(TuiCommand::DeleteForward),
        // keymap = "input:delete_backward" (alternate combo). Some terminals
        // deliver Ctrl+H as a literal control char rather than Backspace.
        KeyCode::Char('h') if ctrl => Some(TuiCommand::DeleteBackward),

        // Cursor
        // keymap = "input:word_left" (alternate combo)
        KeyCode::Left if ctrl || alt => Some(TuiCommand::WordLeft),
        // keymap = "input:cursor_left" (alternate combo)
        KeyCode::Left => Some(TuiCommand::CursorLeft),
        // keymap = "input:word_right" (alternate combo)
        KeyCode::Right if ctrl || alt => Some(TuiCommand::WordRight),
        KeyCode::Right if prompt_suggestion_visible(state) => {
            Some(TuiCommand::AcceptPromptSuggestion)
        }
        // keymap = "input:cursor_right" (alternate combo)
        KeyCode::Right => Some(TuiCommand::CursorRight),
        // Shift+↑/↓ enter and navigate the agent switcher (no-op when no
        // subagents run — handled in the update layer). Plain ↑/↓ stay
        // history-recall; Shift disambiguates without stealing them.
        KeyCode::Up if shift => Some(TuiCommand::AgentSwitcherNav(-1)),
        KeyCode::Up if alt => Some(TuiCommand::ScrollUp),
        // keymap = "input:history"
        KeyCode::Up => Some(TuiCommand::CursorUp),
        KeyCode::Down if shift => Some(TuiCommand::AgentSwitcherNav(1)),
        KeyCode::Down if alt => Some(TuiCommand::ScrollDown),
        // keymap = "input:history"
        KeyCode::Down => Some(TuiCommand::CursorDown),
        // keymap = "input:cursor_home" (alternate combo)
        KeyCode::Home => Some(TuiCommand::CursorHome),
        // keymap = "input:cursor_end" (alternate combo)
        KeyCode::End => Some(TuiCommand::CursorEnd),

        // Emacs / readline (GNU readline conventions). Each arm is the
        // canonical implementation of the `KeymapEntry` named above.
        // keymap = "input:cursor_home"
        KeyCode::Char('a') if ctrl => Some(TuiCommand::CursorHome),
        // keymap = "input:cursor_end"
        KeyCode::Char('e') if ctrl => Some(TuiCommand::CursorEnd),
        // keymap = "input:cursor_left"
        KeyCode::Char('b') if ctrl => Some(TuiCommand::CursorLeft),
        // keymap = "input:cursor_right"
        KeyCode::Char('f') if ctrl => Some(TuiCommand::CursorRight),
        // keymap = "input:kill_to_eol"
        KeyCode::Char('k') if ctrl => Some(TuiCommand::KillToEndOfLine),
        // keymap = "input:kill_to_bol"
        KeyCode::Char('u') if ctrl => Some(TuiCommand::KillToBeginningOfLine),
        // keymap = "input:delete_word_backward"
        KeyCode::Char('w') if ctrl => Some(TuiCommand::DeleteWordBackward),
        // keymap = "input:yank"
        KeyCode::Char('y') if ctrl => Some(TuiCommand::Yank),
        // keymap = "input:newline" (alternate combo)
        KeyCode::Char('j') if ctrl => Some(TuiCommand::InsertNewline),
        // keymap = "input:word_left" (alternate combo)
        KeyCode::Char('b') if alt => Some(TuiCommand::WordLeft),
        // keymap = "input:word_right" (alternate combo)
        KeyCode::Char('f') if alt => Some(TuiCommand::WordRight),

        // Escape always emits Cancel; the *second* Esc within
        // `DOUBLE_PRESS_TIMEOUT` is what opens rewind, handled inside
        // `update::handle_command`'s Cancel arm via
        // `state.ui.esc_tracker.poll(...)`. The dispatch layer can't
        // poll a tracker through `&AppState`.
        KeyCode::Esc => Some(TuiCommand::Cancel),

        // Character input. A Ctrl+<char> combo that reached here has no binding
        // (the readline arms above are the only Ctrl combos that type); letting
        // it fall through to `InsertChar` typed the literal letter into the
        // composer — the tui-v2 cascade-deletion regression where e.g. Ctrl+S
        // (session browser) degraded to inserting "s", in Chat AND under the
        // autocomplete popup. Swallow unbound Ctrl combos instead. Alt is left
        // alone: on macOS Option-compose delivers accented chars as Alt+<char>.
        KeyCode::Char(c) if !ctrl => Some(TuiCommand::InsertChar(c)),

        _ => None,
    }
}

pub(crate) fn prompt_suggestion_visible(state: &AppState) -> bool {
    state.ui.input.is_empty()
        && state.session.queued_commands.is_empty()
        && state
            .session
            .prompt_suggestions
            .last()
            .is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
#[path = "keybinding_bridge.test.rs"]
mod tests;
