//! Text-editing, cursor-history, and word-movement handlers.
//!
//! Extracted from `update.rs` to keep the top-level dispatch lean.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::state::AppState;
use crate::state::PromptMode;
use crate::state::SlashCommandName;

pub(super) fn parse_slash_input(trimmed: &str) -> Option<(SlashCommandName, String)> {
    let stripped = trimmed.strip_prefix('/')?;
    if stripped.is_empty() {
        return None;
    }
    let (name, args) = match stripped.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim_start()),
        None => (stripped, ""),
    };
    Some((SlashCommandName::new(name).ok()?, args.to_string()))
}

/// Mirror an in-memory `add_to_history` into the persistent cross-session
/// store. The driver (`tui_runner`) owns the `coco_session::PromptHistory`
/// file write; a closed channel just means shutdown is in progress, so the
/// dropped persist is harmless.
async fn persist_prompt_history(
    command_tx: &mpsc::Sender<UserCommand>,
    display: String,
    pastes: &[coco_tui_ui::paste::PasteEntry],
) {
    let pasted_contents = pastes
        .iter()
        .filter(|paste| !paste.is_image)
        .filter_map(|paste| paste_id(&paste.pill).map(|id| (id, paste.content.clone())))
        .collect();
    let _ = command_tx
        .send(UserCommand::PersistPromptHistory {
            display,
            pasted_contents,
        })
        .await;
}

fn paste_id(pill: &str) -> Option<i32> {
    let digits = pill
        .split_once('#')?
        .1
        .split(']')
        .next()?
        .split(' ')
        .next()?;
    digits.parse().ok()
}

fn referenced_pastes(state: &AppState, text: &str) -> Vec<coco_tui_ui::paste::PasteEntry> {
    state
        .ui
        .paste_manager
        .entries()
        .iter()
        .filter(|entry| text.contains(&entry.pill))
        .cloned()
        .collect()
}

/// Handle a submission whose leading character is a prompt-mode prefix
/// (`!` bash). Dispatches a typed `UserCommand` for the engine bridge
/// to execute; the bridge's `run_prompt_mode_bash` pushes a single
/// `SystemMessage::LocalCommand { command, output }` via
/// `history_push_and_emit` after the shell call completes, so the
/// transcript view shows the invocation through the standard
/// `MessageAppended` path. The TUI never touches the shell directly —
/// keeps the permission model and side-effect surface in one place.
async fn submit_prefixed(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
    mode: PromptMode,
    text: &str,
) -> bool {
    debug_assert_eq!(mode, PromptMode::Bash);
    let resolved = state.ui.paste_manager.resolve_structured(text);
    let payload = mode.strip_prefix(&resolved.text).to_string();
    if payload.is_empty() {
        // Empty body after stripping the prefix (e.g. user typed just
        // `!` and hit Enter). Don't echo or dispatch — drop silently.
        return true;
    }

    // Record the *full* prefixed text in history so up-arrow recall
    // returns the user to the same mode without forcing them to retype
    // the prefix character.
    let pastes = referenced_pastes(state, text);
    persist_prompt_history(command_tx, text.to_string(), &pastes).await;
    state
        .ui
        .input
        .add_to_history_with_pastes(text.to_string(), pastes);

    let user_message_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(
        target: "coco_tui::submit",
        user_message_id = %user_message_id,
        kind = "bash",
        chars = payload.len(),
        "user submitted bash command",
    );
    if let Err(e) = command_tx
        .send(UserCommand::SubmitBash {
            user_message_id,
            command: payload,
        })
        .await
    {
        tracing::warn!(
            target: "coco_tui::submit",
            error = %e,
            "failed to dispatch SubmitBash (command channel closed)",
        );
    }

    state.ui.paste_manager.clear();
    state.ui.scroll_offset = 0;
    state.ui.user_scrolled = false;
    state.session.last_query_completion_at = None;
    state.session.idle_prompt_fired = false;
    true
}

/// Submit current input. Slash commands are sent as typed command requests
/// and resolved by the command layer.
pub(super) async fn submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) -> bool {
    let text = state.ui.input.take_input();
    if text.is_empty() {
        return true;
    }

    // Prompt-mode routing happens BEFORE slash-command checks
    // because `!` and `#` are prefix-only — they can never collide with
    // `/foo` (different leading byte) so this ordering is safe and
    // matches TS's `getModeFromInput → if bash …` dispatch order.
    let mode = PromptMode::from_text(&text);
    if mode != PromptMode::Normal {
        return submit_prefixed(state, command_tx, mode, &text).await;
    }

    let trimmed = text.trim();
    if let Some((name, args)) = parse_slash_input(trimmed) {
        tracing::info!(
            target: "coco_tui::submit",
            kind = "slash",
            command = %name.as_str(),
            args_chars = args.len(),
            "user submitted slash command",
        );
        let pastes = referenced_pastes(state, &text);
        persist_prompt_history(command_tx, text.clone(), &pastes).await;
        state.ui.input.add_to_history_with_pastes(text, pastes);
        state.ui.paste_manager.clear();
        // `/exit` (alias `/quit`) shuts down through the same path as the
        // Ctrl+C/Ctrl+D double-press exit, not the registry handler (which only
        // prints "Exiting…")., where /exit funnels into the shared
        // exit flow.
        if super::is_exit_command(name.as_str()) {
            super::shutdown_via_slash_command(state, command_tx).await;
            return true;
        }
        if let Err(e) = command_tx
            .send(UserCommand::ExecuteSlashCommand { name, args })
            .await
        {
            tracing::warn!(
                target: "coco_tui::submit",
                error = %e,
                "failed to dispatch ExecuteSlashCommand (command channel closed)",
            );
        }
        return true;
    }

    // Snapshot the paste payloads this text references BEFORE the manager
    // is cleared below, so recalling the entry rehydrates its pills.
    let pastes = referenced_pastes(state, &text);
    persist_prompt_history(command_tx, text.clone(), &pastes).await;
    state
        .ui
        .input
        .add_to_history_with_pastes(text.clone(), pastes);
    let resolved = state.ui.paste_manager.resolve_structured(&text);

    // Mint the user-message UUID once at submit time so the agent
    // driver's `Message::User`, the file-history snapshot, and the
    // JSONL transcript all key off the same id. Engine
    // `history_push_and_emit` emits `MessageAppended` carrying this
    // uuid, which the `TranscriptView` then renders.
    let user_message_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(
        target: "coco_tui::submit",
        user_message_id = %user_message_id,
        kind = "prompt",
        chars = resolved.text.len(),
        images = resolved.images.len(),
        display_chars = text.len(),
        "user submitted prompt",
    );

    if let Err(e) = command_tx
        .send(UserCommand::SubmitInput {
            user_message_id,
            content: resolved.text,
            display_text: Some(text),
            images: resolved.images,
        })
        .await
    {
        tracing::warn!(
            target: "coco_tui::submit",
            error = %e,
            "failed to dispatch SubmitInput (command channel closed)",
        );
    }
    state.ui.paste_manager.clear();
    state.ui.scroll_offset = 0;
    state.ui.user_scrolled = false;
    // Reset idle-prompt window: the user has just spoken, so any
    // pending firing must wait for the *next* turn-completion.
    state.session.last_query_completion_at = None;
    state.session.idle_prompt_fired = false;
    true
}

/// Delete one word backwards from the cursor.
/// Delegates to `TextArea::delete_backward_word`, which puts the killed
/// span into the TextArea's kill buffer (yankable via Ctrl+Y).
pub(super) fn delete_word_backward(state: &mut AppState) {
    state.ui.input.textarea.delete_backward_word();
}

/// Delete one word forward from the cursor.
/// Delegates to `TextArea::delete_forward_word` (alt+d / ctrl+delete).
pub(super) fn delete_word_forward(state: &mut AppState) {
    state.ui.input.textarea.delete_forward_word();
}

/// Kill from cursor to end of current line (Emacs Ctrl+K).
/// TextArea owns the single-entry kill buffer; consecutive kills accumulate
/// readline-style so `Ctrl+Y` recovers the full deleted region.
pub(super) fn kill_to_end_of_line(state: &mut AppState) {
    state.ui.input.textarea.kill_to_end_of_line();
}

/// Kill from BOL to cursor (Emacs Ctrl+U / readline `unix-line-discard`).
pub(super) fn kill_to_beginning_of_line(state: &mut AppState) {
    state.ui.input.textarea.kill_to_beginning_of_line();
}

/// Yank (paste) the kill buffer at the cursor (Emacs Ctrl+Y).
pub(super) fn yank(state: &mut AppState) {
    state.ui.input.textarea.yank();
}

/// Whether the cursor sits on the first line of the input (no newline
/// before it). Up-arrow recalls history here; otherwise it moves the
/// cursor up a line so multi-line drafts stay editable.
fn cursor_on_first_line(input: &crate::state::InputState) -> bool {
    let cursor = input.textarea.cursor();
    input.text().get(..cursor).is_none_or(|s| !s.contains('\n'))
}

/// Whether the cursor sits on the last line of the input (no newline at
/// or after it). Down-arrow advances history here; otherwise it moves the
/// cursor down a line.
fn cursor_on_last_line(input: &crate::state::InputState) -> bool {
    let cursor = input.textarea.cursor();
    input.text().get(cursor..).is_none_or(|s| !s.contains('\n'))
}

/// Up arrow: recall older history when the cursor is on the first line,
/// otherwise move the cursor up one line (multi-line draft editing).
pub(super) fn history_up(state: &mut AppState) {
    if cursor_on_first_line(&state.ui.input) {
        history_browse_start(state);
    } else {
        state.ui.input.textarea.move_cursor_up();
    }
}

/// Down arrow: recall newer history (toward the live draft) when the
/// cursor is on the last line, otherwise move the cursor down one line.
/// When the cursor is already at the live draft (no newer history) and the
/// draft is empty, a further Down parks focus on the footer background-tasks
/// pill if any task is running —
/// → `selectFooterItem`. Enter then opens the background-tasks dialog.
pub(super) fn history_down(state: &mut AppState) {
    if !cursor_on_last_line(&state.ui.input) {
        state.ui.input.textarea.move_cursor_down();
        return;
    }
    if state.ui.input.history_index.is_some() {
        history_next(state);
        return;
    }
    if state.ui.input.text().trim().is_empty()
        && crate::status_bar::background_pill_label(state).is_some()
    {
        state.ui.focus = crate::state::FocusTarget::FooterShells;
    }
}

/// Down arrow: step back toward the most-relevant entry; leaving the list
/// at index 0 clears the input (matches TS PromptInput behaviour).
pub(super) fn history_next(state: &mut AppState) {
    let Some(idx) = state.ui.input.history_index else {
        return;
    };
    if idx > 0 {
        let new_idx = idx - 1;
        state.ui.input.history_index = Some(new_idx);
        let entry = &state.ui.input.history[new_idx];
        let text = entry.text.clone();
        state.ui.paste_manager.replace_entries(entry.pastes.clone());
        state.ui.input.textarea.set_text(&text);
        state
            .ui
            .input
            .textarea
            .move_cursor_to_end_of_line(coco_tui_ui::widgets::EolBehavior::StayPut);
    } else {
        state.ui.input.history_index = None;
        state.ui.input.textarea.set_text("");
        state.ui.paste_manager.clear();
    }
}

// ─────────────────────── Ctrl+R fuzzy history search ───────────────────────

/// Preview the matched history entry in the composer (text + pastes),
/// cursor at end — mirrors up-arrow recall.
fn apply_search_match(state: &mut AppState, idx: usize) {
    let entry = &state.ui.input.history[idx];
    let text = entry.text.clone();
    let pastes = entry.pastes.clone();
    state.ui.paste_manager.replace_entries(pastes);
    state.ui.input.textarea.set_text(&text);
    state
        .ui
        .input
        .textarea
        .move_cursor_to_end_of_line(coco_tui_ui::widgets::EolBehavior::StayPut);
}

/// Restore the draft snapshotted when the search began (used on no-match).
fn restore_search_draft(state: &mut AppState) {
    let Some(search) = state.ui.history_search.as_ref() else {
        return;
    };
    let text = search.original_text.clone();
    let pastes = search.original_pastes.clone();
    state.ui.paste_manager.replace_entries(pastes);
    state.ui.input.textarea.set_text(&text);
}

/// Re-rank results for the current query, reset the selection to the top match,
/// and preview it (or restore the draft when nothing matches).
fn refresh_results(state: &mut AppState) {
    let Some((query, browse)) = state
        .ui
        .history_search
        .as_ref()
        .map(|s| (s.query.clone(), s.browse))
    else {
        return;
    };
    let mut results =
        crate::autocomplete::history_search::search_history(&state.ui.input.history, &query);
    if browse && query.is_empty() {
        results.items.reverse();
        results.indices.reverse();
    }
    let selected = if browse {
        results.items.len().saturating_sub(1)
    } else {
        0
    };
    let preview = results.indices.get(selected).copied();
    if let Some(s) = state.ui.history_search.as_mut() {
        s.results = results.items;
        s.result_indices = results.indices;
        s.selected = selected;
    }
    match preview {
        Some(idx) => apply_search_match(state, idx),
        None => restore_search_draft(state),
    }
}

/// Preview the currently selected match (or restore the draft if none).
fn preview_selected(state: &mut AppState) {
    let idx = state
        .ui
        .history_search
        .as_ref()
        .and_then(crate::state::HistorySearch::selected_history_index);
    match idx {
        Some(i) => apply_search_match(state, i),
        None => restore_search_draft(state),
    }
}

/// Ctrl+R on an idle composer: snapshot the draft and enter search mode.
pub(super) fn history_search_start(state: &mut AppState) {
    start_history_search(state, false);
}

fn history_browse_start(state: &mut AppState) {
    start_history_search(state, true);
}

fn start_history_search(state: &mut AppState, browse: bool) {
    if state.ui.history_search.is_some() {
        return;
    }
    state.ui.input.clear_inline_hint();
    let original_pastes = state.ui.paste_manager.entries().to_vec();
    state.ui.history_search = Some(crate::state::HistorySearch {
        browse,
        query: String::new(),
        results: Vec::new(),
        result_indices: Vec::new(),
        selected: 0,
        original_text: state.ui.input.text().to_string(),
        original_pastes,
        original_history_index: state.ui.input.history_index,
    });
    refresh_results(state);
}

/// Append a character to the query and re-rank from the top match.
pub(super) fn history_search_input(state: &mut AppState, c: char) {
    if let Some(s) = state.ui.history_search.as_mut() {
        s.browse = false;
        s.query.push(c);
    } else {
        return;
    }
    refresh_results(state);
}

/// Delete the last query character and re-rank from the top match.
pub(super) fn history_search_backspace(state: &mut AppState) {
    if let Some(s) = state.ui.history_search.as_mut() {
        s.browse = false;
        s.query.pop();
    } else {
        return;
    }
    refresh_results(state);
}

/// Move the selection one row DOWN the ranked list (↓ / Ctrl+R).
pub(super) fn history_search_older(state: &mut AppState) {
    if let Some(s) = state.ui.history_search.as_mut() {
        if s.results.is_empty() {
            return;
        }
        if s.browse && s.selected + 1 >= s.results.len() {
            history_search_cancel(state);
            return;
        }
        s.selected = (s.selected + 1).min(s.results.len() - 1);
    } else {
        return;
    }
    preview_selected(state);
}

/// Move the selection one row UP the ranked list (↑ / Ctrl+S).
pub(super) fn history_search_newer(state: &mut AppState) {
    let show_hint = if let Some(s) = state.ui.history_search.as_mut() {
        let before = s.selected;
        s.selected = s.selected.saturating_sub(1);
        s.browse && s.selected < before && !state.ui.search_hint_shown
    } else {
        return;
    };
    if show_hint {
        state.ui.search_hint_shown = true;
        state.ui.add_toast(crate::state::ui::Toast::info(
            crate::i18n::t!("toast.ctrl_r_search_hint").to_string(),
        ));
    }
    preview_selected(state);
}

/// Accept the previewed entry (already in the composer) as the live draft.
pub(super) fn history_search_accept(state: &mut AppState) {
    if state.ui.history_search.take().is_some() {
        state.ui.input.history_index = None;
    }
}

/// Cancel search and restore the draft saved when it began.
pub(super) fn history_search_cancel(state: &mut AppState) {
    let Some(search) = state.ui.history_search.take() else {
        return;
    };
    state
        .ui
        .paste_manager
        .replace_entries(search.original_pastes);
    state.ui.input.textarea.set_text(&search.original_text);
    state.ui.input.history_index = search.original_history_index;
}

/// Move cursor one word to the left (grapheme-aware via TextArea).
pub(super) fn word_left(state: &mut AppState) {
    let target = state.ui.input.textarea.beginning_of_previous_word();
    state.ui.input.textarea.set_cursor(target);
}

/// Move cursor one word to the right (grapheme-aware via TextArea).
pub(super) fn word_right(state: &mut AppState) {
    let target = state.ui.input.textarea.end_of_next_word();
    state.ui.input.textarea.set_cursor(target);
}
