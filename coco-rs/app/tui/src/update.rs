//! TUI update handler — the Update in TEA.
//!
//! Applies [`TuiCommand`]s to [`AppState`]. Side effects (sending to core)
//! are dispatched via the command channel. Complex per-category handlers
//! live in the private submodules (`state`, `show`, `edit`) to keep this
//! dispatcher focused on routing.

use coco_types::TurnAbortReason;
use tokio::sync::mpsc;

use crate::command::ShutdownReason;
use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::FocusTarget;
use crate::state::ModalState;
use crate::state::PanePromptState;
use coco_tui_ui::constants;

use exit::ExitEffect;

mod agents_dialog;
mod background_tasks;
mod clipboard;
pub(crate) use clipboard::sniff_image_path_paste;
mod edit;
mod exit;
mod expanded_view;
mod interaction;
mod plugin_dialog;
// `pub(crate)` so the slash-command dispatcher (in
// `server_notification_handler::tui_only`) can call into `cycle_model`
// when `TuiOnlyEvent::OpenModelPicker` arrives. The other `show::*`
// constructors remain crate-internal helpers.
pub(crate) mod show;
mod skills_dialog;
mod stash;
mod transcript;

#[cfg(test)]
#[path = "update.test.rs"]
mod tests;

/// Route a bracketed paste into the active AskUserQuestion free-text input.
/// Returns `true` if consumed. Lets `app.rs` redirect clipboard / IME-committed
/// paste away from the hidden main input while a question prompt is open.
pub(crate) fn route_question_free_text_paste(state: &mut AppState, text: &str) -> bool {
    crate::bottom_pane::question::question_free_text_paste(state, text)
}

/// Apply a TUI command to the state.
///
/// Returns `true` if the state changed and a redraw is needed.
pub async fn handle_command(
    state: &mut AppState,
    cmd: TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    // Breadcrumb every dispatch so user-bug repros include which TuiCommand
    // ran. InsertChar / SurfaceFilter fire per-keystroke and would flood
    // debug at typing rate — drop them to trace.
    match &cmd {
        TuiCommand::InsertChar(_) | TuiCommand::SurfaceFilter(_) => {
            tracing::trace!(target: "coco_tui::command", cmd = ?cmd, "TuiCommand dispatch");
        }
        _ => {
            tracing::debug!(
                target: "coco_tui::command",
                cmd = ?cmd,
                is_streaming = state.is_streaming(),
                has_modal = state.ui.modal.is_some(),
                has_prompt = state.ui.interaction.active_prompt.is_some(),
                "TuiCommand dispatch",
            );
        }
    }
    // Snapshot input before the command so we can reactively refresh the
    // autocomplete popup whenever input text or cursor moves, without
    // threading a refresh call through every editing arm.
    let text_before = state.ui.input.text().to_string();
    let cursor_before = state.ui.input.textarea.cursor();

    // Intercept editable-dialog keys before the main dispatch.
    // The skills dialog has a richer state machine (select / filter
    // modes) than the generic modal cancel/submit path; deferring to
    // it here keeps the per-arm InsertChar/Backspace/etc. branches
    // free of dialog-specific logic.
    if let skills_dialog::Handled::Yes(changed) =
        skills_dialog::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    // The `/agents` 2-tab dialog has its own ←/→ tab cycle + `x`
    // cancel-task path that the generic modal dispatch doesn't model.
    // Same fall-through contract as the skills dialog.
    if let agents_dialog::Handled::Yes(changed) =
        agents_dialog::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    if let plugin_dialog::Handled::Yes(changed) =
        plugin_dialog::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    // The background-tasks dialog (full-screen list + per-task detail) owns
    // its own ↑/↓ select, Enter drill, `x` stop, ←/Esc back-or-close.
    if let background_tasks::Handled::Yes(changed) =
        background_tasks::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    // The `/permissions` editor has its own tab cycle, inline add form,
    // and delete confirmation that the generic modal dispatch can't model.
    if let crate::modal_pane::permissions_editor::Handled::Yes(changed) =
        crate::modal_pane::permissions_editor::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    // The `/add-dir` overlay is a single directory-path text input with inline
    // validation — same dedicated text-input routing as the permissions editor.
    if let crate::modal_pane::add_directory::Handled::Yes(changed) =
        crate::modal_pane::add_directory::intercept(state, &cmd, command_tx).await
    {
        return changed;
    }

    // Inline edits to a shell prompt's editable "always allow" prefix are
    // consumed here so the keystrokes don't leak into the chat composer.
    if crate::bottom_pane::permission::intercept_prefix_edit(state, &cmd) {
        return true;
    }

    // When the footer background-tasks pill holds focus (parked there by
    // pressing Down at the bottom of the composer), Enter opens the dialog
    // and Up/Esc/Left return focus to the input. Any other key drops focus
    // back to the input and falls through so the keystroke isn't lost. If the
    // pill vanished (all tasks finished) while parked here, release focus so
    // the user isn't stuck on an invisible target.
    // When the agent switcher holds focus (Shift+↑/↓ parked it there), the
    // switcher lists agents only — no `◯ main` row. Shift+↑/↓ (or plain ↑/↓)
    // move the selection, Enter views the selected agent, `x` interrupts it,
    // and `←` returns to main + input (a natural "back" gesture that can't
    // interrupt the turn). Esc is a SAFE no-op: it's swallowed here so a stray
    // press can't fall through to the input and interrupt the running turn.
    // Any other key drops back to the input. If the list emptied (all agents
    // finished) while focused, release focus.
    if state.ui.focus == FocusTarget::AgentSwitcher {
        let agents: Vec<String> = state
            .session
            .switcher_agents()
            .iter()
            .map(|a| a.agent_id.clone())
            .collect();
        // Release focus if the list emptied or we entered coordinator mode
        // (its panel isn't switcher-wired) — the user isn't stuck on a target
        // that no longer renders a cursor.
        if agents.is_empty() || state.ui.coordinator_mode_active {
            state.ui.focus = FocusTarget::Input;
            state.session.viewing_agent_id = None;
        } else {
            let last = agents.len() - 1;
            let sel = state.ui.agent_switcher_selected.min(last);
            match &cmd {
                TuiCommand::AgentSwitcherNav(delta) => {
                    state.ui.agent_switcher_selected =
                        (sel as i32 + delta).clamp(0, last as i32) as usize;
                    return true;
                }
                TuiCommand::CursorUp => {
                    state.ui.agent_switcher_selected = sel.saturating_sub(1);
                    return true;
                }
                TuiCommand::CursorDown => {
                    state.ui.agent_switcher_selected = (sel + 1).min(last);
                    return true;
                }
                TuiCommand::SubmitInput => {
                    // View the selected agent (read-only overlay). Keep focus on
                    // the switcher so Shift+↑/↓ can hop between agents.
                    state.session.viewing_agent_id = Some(agents[sel].clone());
                    return true;
                }
                TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'x') => {
                    // `x` = interrupt the selected agent (fires its cancel token).
                    let _ = command_tx
                        .send(UserCommand::CancelSubagent {
                            task_id: agents[sel].clone(),
                        })
                        .await;
                    return true;
                }
                TuiCommand::CursorLeft => {
                    // `←` = back to main + input (the safe exit).
                    state.session.viewing_agent_id = None;
                    state.ui.focus = FocusTarget::Input;
                    return true;
                }
                TuiCommand::Cancel => {
                    // Esc swallowed: neither exits nor interrupts. `←` is the exit.
                    return true;
                }
                _ => {
                    // Any other key drops focus back to the input and falls
                    // through so the keystroke isn't lost (mirrors FooterShells).
                    state.session.viewing_agent_id = None;
                    state.ui.focus = FocusTarget::Input;
                }
            }
        }
    }

    if state.ui.focus == FocusTarget::FooterShells {
        if crate::status_bar::background_pill_label(state).is_none() {
            state.ui.focus = FocusTarget::Input;
        } else {
            match &cmd {
                TuiCommand::SubmitInput => {
                    state.ui.focus = FocusTarget::Input;
                    background_tasks::open(state);
                    return true;
                }
                TuiCommand::CursorUp | TuiCommand::Cancel | TuiCommand::CursorLeft => {
                    state.ui.focus = FocusTarget::Input;
                    return true;
                }
                TuiCommand::CursorDown => return true,
                _ => state.ui.focus = FocusTarget::Input,
            }
        }
    }

    let changed = match cmd {
        TuiCommand::Noop => false,
        // ── Mode toggles ──
        TuiCommand::TogglePlanMode => {
            state.toggle_plan_mode();
            let mode = state.session.permission_mode;
            let _ = command_tx
                .send(UserCommand::SetPermissionMode { mode })
                .await;
            // No toast — the status bar already reflects the active mode.
            true
        }
        TuiCommand::CyclePermissionMode => {
            // Compute the next mode without committing — the cycle helper
            // applies eagerly, so we'd lose the chance to intercept the
            // high-stakes BypassPermissions target and surface
            // `BypassPermissionsModeDialog` before the mode actually flips.
            // Auto (like every other mode) switches directly: it's
            // classifier-gated, not dangerous, so a per-switch confirmation
            // would just be noise.
            let next = state.session.permission_mode.next_in_cycle(
                state.session.plan_mode_available,
                state.session.bypass_permissions_available,
                state.session.auto_mode_available,
            );
            match next {
                coco_types::PermissionMode::BypassPermissions => {
                    let current_label = format!("{:?}", state.session.permission_mode);
                    state.ui.show_modal(ModalState::BypassPermissions(
                        crate::state::BypassPermissionsState {
                            current_mode: current_label,
                        },
                    ));
                }
                _ => {
                    // No toast on switch — the status bar already shows the
                    // active permission mode, so a transient hint is redundant.
                    state.session.permission_mode = next;
                    let _ = command_tx
                        .send(UserCommand::SetPermissionMode { mode: next })
                        .await;
                }
            }
            true
        }
        TuiCommand::CycleThinkingLevel => {
            // Find the catalog entry for the current Main role's
            // (provider, model_id) pair and cycle through ITS declared
            // `supported_efforts`. This honors per-model declarations
            // (DeepSeek's 4-state ladder is different from Anthropic's
            // 4-budget ladder is different from OpenAI's 5-level
            // ladder) — Ctrl+T is no longer a hardcoded ordering.
            let supported: Vec<coco_types::ReasoningEffort> = state
                .session
                .model_catalog
                .iter()
                .find(|e| e.provider == state.session.provider && e.model_id == state.session.model)
                .map(|e| e.supported_efforts.clone())
                .unwrap_or_default();

            // No declared support → silent no-op. Common when the
            // active model is registered without a
            // `supported_thinking_levels` entry (e.g. a user-added
            // model in `~/.coco/models.json` without metadata) or when
            // `state.session.{provider, model}` haven't been seeded
            // yet (pre-bootstrap mock paths).
            if supported.is_empty() {
                return true;
            }

            // Current effort not in supported list → restart at index 0.
            // Self-correcting: a stale `thinking_effort` (e.g. from a
            // prior model that supported XHigh) snaps back to a legal
            // value on the next Ctrl+T press instead of going nowhere.
            let current_idx = supported
                .iter()
                .position(|e| *e == state.session.thinking_effort)
                .unwrap_or(0);
            let next = supported[(current_idx + 1) % supported.len()];

            state.session.thinking_effort = next;
            let _ = command_tx
                .send(UserCommand::SetThinkingLevel {
                    level: next.to_string(),
                })
                .await;
            true
        }
        TuiCommand::ToggleThinking => {
            state.ui.show_thinking = !state.ui.show_thinking;
            true
        }
        TuiCommand::CycleModel => {
            show::cycle_model(state);
            true
        }
        TuiCommand::ToggleFastMode => {
            state.session.fast_mode = !state.session.fast_mode;
            let _ = command_tx.send(UserCommand::ToggleFastMode).await;
            true
        }

        // ── Input actions ──
        TuiCommand::SubmitInput => {
            // Steer (queue) whenever a turn is in flight — the whole turn,
            // not just the token-stream window. Mirrors TS, which gates on
            // `queryGuard.isActive` (active for the entire query lifecycle,
            // including tool / subagent execution). Gating on `is_streaming()`
            // here would treat a submit during the post-stream tool/subagent
            // phase as a fresh turn, hard-preempting (`SystemPreempt`) instead
            // of steering.
            if state.ui.ephemeral.turn_active() {
                let handled = queue_current_input(state, command_tx).await;
                if state.session.has_submit_interruptible_tool_in_progress {
                    let _ = command_tx
                        .send(UserCommand::Interrupt(TurnAbortReason::SubmitInterrupt))
                        .await;
                }
                return handled;
            }
            // A trailing backslash + Enter inserts a newline instead of
            // submitting (poor-man's line-continuation). Match here so the
            // heredoc-style escape works in both ordinary and vim-Insert mode.
            if state.ui.input.textarea.text().ends_with('\\') {
                let len = state.ui.input.textarea.text().len();
                state.ui.input.textarea.replace_range(len - 1..len, "\n");
                return true;
            }
            edit::submit(state, command_tx).await
        }
        TuiCommand::SubmitInterrupt => {
            let handled = queue_current_input(state, command_tx).await;
            let _ = command_tx
                .send(UserCommand::Interrupt(TurnAbortReason::SubmitInterrupt))
                .await;
            handled
        }
        TuiCommand::QueueInput => queue_current_input(state, command_tx).await,
        TuiCommand::Interrupt => {
            let now = std::time::Instant::now();
            let timing =
                ExitTiming::from_pending_until(state.ui.ctrl_c_tracker.pending_until(), now);
            let effect = exit::on_interrupt(state, now);
            apply_exit_effect(state, command_tx, ExitSource::CtrlC, timing, effect).await;
            true
        }
        TuiCommand::RequestExit => {
            let now = std::time::Instant::now();
            let timing =
                ExitTiming::from_pending_until(state.ui.ctrl_d_tracker.pending_until(), now);
            let effect = exit::on_request_exit(state, now);
            apply_exit_effect(state, command_tx, ExitSource::CtrlD, timing, effect).await;
            true
        }
        TuiCommand::Cancel => {
            // Vim insert-mode Esc → transition to Normal mode and walk
            // the cursor back one grapheme (vim convention). Mirrors
            // codex-rs textarea.rs:654-660. This wins over every other
            // Cancel branch because Esc in Insert is a mode transition,
            // not a UI dismissal. Gated on `vim.enabled` so non-vim
            // users keep the standard Esc → Cancel behavior.
            if state.ui.input.vim.insert_escape_active()
                && crate::vim::wiring::handle_insert_escape(
                    &mut state.ui.input.textarea,
                    &mut state.ui.input.vim,
                )
            {
                return true;
            }
            // Esc dismisses autocomplete first (so the user can escape out
            // of a trigger without losing their typed input) before
            // touching any state.
            if !state.ui.has_blocking_interaction() && state.ui.completion.active.is_some() {
                state.ui.completion.dismiss_active();
                state.ui.sync_popup_from_active_suggestions();
                return true;
            }
            // Escape while viewing the teammate activity pane interrupts the
            // focused teammate's current turn only. Ctrl+C / KillAllAgents
            // remains the lifecycle stop path.
            if !state.ui.has_blocking_interaction()
                && matches!(
                    state.session.expanded_view,
                    coco_types::ExpandedView::Teammates
                )
                && let Some(index) = state.session.focused_subagent_index
                && let Some(agent) = state.session.subagents.get(index as usize)
                && matches!(agent.status, crate::state::session::SubagentStatus::Running)
            {
                let _ = command_tx
                    .send(UserCommand::InterruptAgentCurrentWork {
                        agent_id: agent.agent_id.clone(),
                    })
                    .await;
                return true;
            }
            if !state.ui.has_blocking_interaction()
                && (state.is_streaming() || state.session.is_busy())
            {
                state.ui.esc_tracker.reset();
                // Flip the spinner verb to "Interrupting…" on this frame —
                // the abort propagates async and the turn only clears on
                // the engine's terminal event, which can be seconds away
                // for worktree-isolated subagents.
                state.ui.ephemeral.mark_interrupting();
                let _ = command_tx
                    .send(UserCommand::Interrupt(TurnAbortReason::UserCancel))
                    .await;
                return true;
            }
            if !state.ui.has_blocking_interaction()
                && state.session.queued_commands.iter().any(|q| q.editable)
            {
                let _ = command_tx
                    .send(UserCommand::EditQueuedCommands {
                        current_input: state.ui.input.text().to_string(),
                        current_cursor: state.ui.input.textarea.cursor(),
                    })
                    .await;
                return true;
            }
            // No state + active suggestions + text present → ESC
            // double-press clears input + saves to history. Single Esc
            // shows a toast; second Esc within `DOUBLE_PRESS_TIMEOUT` clears.
            if !state.ui.has_blocking_interaction()
                && state.ui.completion.active.is_none()
                && !state.ui.input.is_empty()
            {
                use coco_tui_ui::double_press::Outcome;
                if state.ui.esc_tracker.poll((), std::time::Instant::now()) == Outcome::Double {
                    let taken = state.ui.input.take_input();
                    // Move the draft's paste payloads into the history entry
                    // (and out of the live manager): the cleared draft no
                    // longer references them, and stale image entries would
                    // otherwise still attach to the next unrelated submit.
                    let pastes: Vec<_> = state
                        .ui
                        .paste_manager
                        .entries()
                        .iter()
                        .filter(|e| taken.contains(&e.pill))
                        .cloned()
                        .collect();
                    state.ui.paste_manager.clear();
                    state.ui.input.add_to_history_with_pastes(taken, pastes);
                    state.ui.input.history_index = None;
                } else {
                    state.ui.add_toast(crate::state::ui::Toast::info(
                        crate::i18n::t!("toast.esc_again_to_clear").to_string(),
                    ));
                }
                return true;
            }
            // No state + no suggestions + idle conditions met → run
            // the double-Esc tracker so a second Esc opens the rewind
            // picker. The poll lives here (not in `keybinding_dispatch`)
            // because dispatch only has `&AppState`; the tracker needs a
            // mutable borrow.
            if state.rewind_available_from_input() {
                use coco_tui_ui::double_press::Outcome;
                if state.ui.esc_tracker.poll((), std::time::Instant::now()) == Outcome::Double {
                    show::rewind(state, command_tx).await;
                    return true;
                }
            }
            if !crate::modal_pane::rewind_cancel(state) {
                return true; // phase-back; keep state
            }
            // Every picker reports its own dismissal to the transcript on
            // close. The theme picker / settings route Esc through `Deny`
            // instead, so the shared helper also runs there
            // (`interaction::deny`).
            if state.ui.modal.is_some() {
                crate::modal_pane::close_modal_with_feedback(state, command_tx).await;
            } else if state.has_active_surface() {
                state.ui.dismiss_prompt();
            }
            true
        }
        TuiCommand::ClearScreen => {
            // Phase 3d (§5): clear the engine-derived transcript so the
            // visible chat empties. The engine retains the full
            // conversation; future `MessageAppended` events repopulate
            // the cell view from the next turn forward.
            state.session.transcript.on_session_reset();
            // Dropping messages also invalidates the copy cache — without this
            // /copy after /clear would surface the response the user just
            // wiped, which is surprising. Matches codex-rs's clear-on-reset.
            state.session.last_agent_markdown = None;
            state.ui.scroll_offset = 0;
            true
        }

        // ── Text editing ──
        TuiCommand::InsertChar(c) => {
            state.ui.input.clear_inline_hint();
            // Route into the rewind summarize-feedback box when that
            // state phase is active so typing builds the feedback
            // string instead of leaking to the input bar.
            if let Some(ModalState::Rewind(r)) = state.ui.modal.as_mut()
                && r.phase == crate::state::rewind::RewindPhase::SummarizeFeedback
            {
                r.summarize_feedback.push(c);
            } else if state.ui.input.vim.normal_dispatch_active() {
                // Vim Normal mode: route the printable key through the
                // vim state machine (h/j/k/l/i/a/o/w/b/d/y/p/x/...).
                // Mirrors codex-rs textarea.rs:518-530 pattern. Gated on
                // `vim.enabled` so non-vim users insert characters as
                // typed instead of triggering vim motions.
                let action = crate::vim::wiring::dispatch_vim_key(
                    c,
                    &mut state.ui.input.textarea,
                    &mut state.ui.input.vim,
                );
                let should_submit = crate::vim::wiring::apply_action(
                    action,
                    &mut state.ui.input.textarea,
                    &mut state.ui.input.vim,
                );
                if should_submit {
                    // Vim `Enter` in Normal mode submits — delegate to
                    // the same path Enter takes in non-vim mode.
                    edit::submit(state, command_tx).await;
                }
            } else {
                let mut buf = [0u8; 4];
                state.ui.input.textarea.insert_str(c.encode_utf8(&mut buf));
            }
            true
        }
        TuiCommand::InsertNewline => {
            state.ui.input.clear_inline_hint();
            state.ui.input.textarea.insert_str("\n");
            true
        }
        TuiCommand::DeleteBackward => {
            state.ui.input.clear_inline_hint();
            if let Some(ModalState::Rewind(r)) = state.ui.modal.as_mut()
                && r.phase == crate::state::rewind::RewindPhase::SummarizeFeedback
            {
                r.summarize_feedback.pop();
            } else {
                state.ui.input.textarea.delete_backward(1);
            }
            true
        }
        TuiCommand::DeleteForward => {
            state.ui.input.clear_inline_hint();
            state.ui.input.textarea.delete_forward(1);
            true
        }
        TuiCommand::DeleteWordBackward => {
            state.ui.input.clear_inline_hint();
            edit::delete_word_backward(state);
            true
        }
        TuiCommand::DeleteWordForward => {
            state.ui.input.clear_inline_hint();
            edit::delete_word_forward(state);
            true
        }
        TuiCommand::KillToEndOfLine => {
            state.ui.input.clear_inline_hint();
            edit::kill_to_end_of_line(state);
            true
        }
        TuiCommand::KillToBeginningOfLine => {
            state.ui.input.clear_inline_hint();
            edit::kill_to_beginning_of_line(state);
            true
        }
        TuiCommand::Yank => {
            state.ui.input.clear_inline_hint();
            edit::yank(state);
            true
        }

        // ── Cursor movement ──
        TuiCommand::CursorLeft => {
            state.ui.input.clear_inline_hint();
            state.ui.input.textarea.move_cursor_left();
            true
        }
        TuiCommand::CursorRight => {
            state.ui.input.clear_inline_hint();
            state.ui.input.textarea.move_cursor_right();
            true
        }
        TuiCommand::CursorUp => {
            state.ui.input.clear_inline_hint();
            if state.ui.input.is_empty() && state.session.queued_commands.iter().any(|q| q.editable)
            {
                let _ = command_tx
                    .send(UserCommand::EditQueuedCommands {
                        current_input: String::new(),
                        current_cursor: 0,
                    })
                    .await;
                return true;
            }
            edit::history_up(state);
            true
        }
        TuiCommand::CursorDown => {
            state.ui.input.clear_inline_hint();
            edit::history_down(state);
            true
        }
        TuiCommand::HistorySearchStart => {
            edit::history_search_start(state);
            true
        }
        TuiCommand::HistorySearchInput(c) => {
            edit::history_search_input(state, c);
            true
        }
        TuiCommand::HistorySearchBackspace => {
            edit::history_search_backspace(state);
            true
        }
        TuiCommand::HistorySearchOlder => {
            edit::history_search_older(state);
            true
        }
        TuiCommand::HistorySearchNewer => {
            edit::history_search_newer(state);
            true
        }
        TuiCommand::HistorySearchAccept => {
            edit::history_search_accept(state);
            true
        }
        TuiCommand::HistorySearchCancel => {
            edit::history_search_cancel(state);
            true
        }
        TuiCommand::CursorHome => {
            state.ui.input.clear_inline_hint();
            state
                .ui
                .input
                .textarea
                .move_cursor_to_beginning_of_line(coco_tui_ui::widgets::BolBehavior::StayPut);
            true
        }
        TuiCommand::CursorEnd => {
            state.ui.input.clear_inline_hint();
            state
                .ui
                .input
                .textarea
                .move_cursor_to_end_of_line(coco_tui_ui::widgets::EolBehavior::StayPut);
            true
        }
        TuiCommand::WordLeft => {
            state.ui.input.clear_inline_hint();
            edit::word_left(state);
            true
        }
        TuiCommand::WordRight => {
            state.ui.input.clear_inline_hint();
            edit::word_right(state);
            true
        }

        // ── Scrolling ──
        TuiCommand::ScrollUp => {
            state.ui.scroll_offset += constants::SCROLL_LINE_STEP;
            state.ui.user_scrolled = true;
            true
        }
        TuiCommand::ScrollDown => {
            state.ui.scroll_offset = (state.ui.scroll_offset - constants::SCROLL_LINE_STEP).max(0);
            if state.ui.scroll_offset == 0 {
                state.ui.user_scrolled = false;
            }
            true
        }
        TuiCommand::PageUp => {
            state.ui.scroll_offset += constants::SCROLL_PAGE_STEP;
            state.ui.user_scrolled = true;
            true
        }
        TuiCommand::PageDown => {
            state.ui.scroll_offset = (state.ui.scroll_offset - constants::SCROLL_PAGE_STEP).max(0);
            if state.ui.scroll_offset == 0 {
                state.ui.user_scrolled = false;
            }
            true
        }

        // ── Focus ──
        TuiCommand::FocusNext | TuiCommand::FocusPrevious => {
            state.ui.focus = match state.ui.focus {
                FocusTarget::Input => FocusTarget::Chat,
                FocusTarget::Chat | FocusTarget::FooterShells | FocusTarget::AgentSwitcher => {
                    FocusTarget::Input
                }
            };
            true
        }
        TuiCommand::FocusNextAgent => {
            let max = state.session.subagents.len() as i32;
            if max > 0 {
                let idx = state.session.focused_subagent_index.unwrap_or(-1);
                state.session.focused_subagent_index = Some((idx + 1).min(max - 1));
            }
            true
        }
        TuiCommand::FocusPrevAgent => {
            if let Some(idx) = state.session.focused_subagent_index {
                state.session.focused_subagent_index = if idx > 0 { Some(idx - 1) } else { None };
            }
            true
        }
        TuiCommand::AgentSwitcherNav(_) => {
            // Reached only when the switcher isn't focused yet (the focus
            // intercept above consumes it once focused). Shift+↑/↓ parks focus
            // on the switcher — but only when there are agents to switch
            // between AND we're not in coordinator mode (that panel has its own
            // renderer and isn't wired to the switcher); otherwise it's an
            // inert no-op (arrows stay history).
            let count = state.session.switcher_agents().len();
            if count > 0 && !state.ui.coordinator_mode_active {
                state.ui.focus = FocusTarget::AgentSwitcher;
                // First press just parks focus on the switcher and lands on the
                // first agent — there is no synthetic `◯ main`/leader row to
                // step off (unlike TS `stepTeammateSelection`, which parks on
                // the leader), so entry must select a real agent. Direction is
                // ignored on entry; once focused, `↑/↓` step from index 0.
                state.ui.agent_switcher_selected = 0;
            }
            true
        }

        // ── Surface actions ──
        TuiCommand::Approve => {
            interaction::approve(state, command_tx).await;
            true
        }
        TuiCommand::Deny => {
            interaction::deny(state, command_tx).await;
            true
        }
        TuiCommand::ApproveAll => {
            crate::bottom_pane::permission::approve_all(state, command_tx).await;
            true
        }
        TuiCommand::ApproveSession => {
            crate::bottom_pane::permission::approve_session(state, command_tx).await;
            true
        }
        TuiCommand::PermissionDigit(digit) => {
            crate::bottom_pane::route_permission_digit(state, digit, command_tx).await;
            true
        }
        TuiCommand::ClassifierAutoApprove {
            request_id,
            matched_rule: _,
        } => {
            crate::bottom_pane::permission::classifier_auto_approve(state, command_tx, request_id)
                .await;
            true
        }
        TuiCommand::AutocompleteAccept => {
            let _ = crate::completion::accept_suggestion(
                state,
                crate::completion::AcceptMode::ExtendCommonPrefix,
            );
            true
        }
        TuiCommand::AcceptPromptSuggestion => {
            accept_prompt_suggestion(state);
            true
        }
        TuiCommand::SubmitPromptSuggestion => {
            if accept_prompt_suggestion(state) {
                edit::submit(state, command_tx).await
            } else {
                true
            }
        }
        TuiCommand::AutocompleteSubmit => {
            if crate::completion::accept_suggestion(
                state,
                crate::completion::AcceptMode::SubmitSelected,
            )
            .is_some_and(|a| a.should_submit)
            {
                edit::submit(state, command_tx).await
            } else {
                true
            }
        }

        // ── Surface navigation ──
        TuiCommand::SurfaceFilter(c) => {
            let handled_question_digit = if let Some(digit) = c.to_digit(10) {
                matches!(
                    state.ui.interaction.active_prompt,
                    Some(PanePromptState::Question(_))
                ) && digit > 0
                    && crate::bottom_pane::question::question_select_digit_and_confirm(
                        state,
                        digit as i32,
                        command_tx,
                    )
                    .await
            } else {
                false
            };
            if !handled_question_digit {
                interaction::filter(state, c);
            }
            true
        }
        TuiCommand::SurfaceFilterBackspace => {
            interaction::filter_backspace(state);
            true
        }
        TuiCommand::SurfaceNext => {
            interaction::nav(state, 1);
            crate::modal_pane::request_diff_stats_if_rewind(state, command_tx).await;
            true
        }
        TuiCommand::SurfacePrev => {
            interaction::nav(state, -1);
            crate::modal_pane::request_diff_stats_if_rewind(state, command_tx).await;
            true
        }
        TuiCommand::SurfaceJumpStart => {
            interaction::nav(state, i32::MIN / 2);
            crate::modal_pane::request_diff_stats_if_rewind(state, command_tx).await;
            true
        }
        TuiCommand::SurfaceJumpEnd => {
            interaction::nav(state, i32::MAX / 2);
            crate::modal_pane::request_diff_stats_if_rewind(state, command_tx).await;
            true
        }
        TuiCommand::SurfaceConfirm => {
            interaction::confirm(state, command_tx).await;
            true
        }
        TuiCommand::PermissionScrollUp => {
            let max = permission_body_scroll_bound(state);
            state
                .ui
                .interaction
                .scroll_permission(-constants::SCROLL_PAGE_STEP, max);
            true
        }
        TuiCommand::PermissionScrollDown => {
            let max = permission_body_scroll_bound(state);
            state
                .ui
                .interaction
                .scroll_permission(constants::SCROLL_PAGE_STEP, max);
            true
        }
        TuiCommand::CopyPickerWriteToFile => {
            match state.ui.take_modal() {
                Some(ModalState::CopyPicker(cp)) => {
                    if let Some(message) = crate::copy::write_picker_selection_to_file(state, cp) {
                        crate::copy::enqueue_copy_output(message, command_tx);
                    }
                    state.ui.finish_taken_modal();
                }
                Some(other) => state.ui.restore_modal(other),
                None => {}
            }
            true
        }

        // ── Commands & surfaces ──
        TuiCommand::ShowHelp => {
            state.ui.show_modal(ModalState::Help);
            true
        }
        TuiCommand::ShowCommandPalette => {
            show::command_palette(state);
            true
        }
        TuiCommand::ShowSessionBrowser => {
            // Same flow as `/resume` with no args: the engine lists sessions
            // and replies with `TuiOnlyEvent::OpenSessionBrowser`, which opens
            // the modal with fresh data. Reading `state.session.saved_sessions`
            // here showed an empty browser on a fresh start (its only writer is
            // that event).
            if let Ok(name) = crate::state::SlashCommandName::new("resume") {
                let _ = command_tx
                    .send(UserCommand::ExecuteSlashCommand {
                        name,
                        args: String::new(),
                    })
                    .await;
            }
            true
        }
        TuiCommand::OpenTeamRoster => {
            show::team_roster(state);
            true
        }
        TuiCommand::ShowGlobalSearch => {
            show::global_search(state);
            true
        }
        TuiCommand::ShowQuickOpen => {
            show::quick_open(state);
            true
        }
        TuiCommand::ShowRewind => {
            show::rewind(state, command_tx).await;
            true
        }
        TuiCommand::ShowRewindFor { target_uuid } => {
            show::rewind_for(state, command_tx, target_uuid).await;
            true
        }
        TuiCommand::ShowDoctor => {
            show::doctor(state);
            true
        }
        TuiCommand::ShowSettings => {
            show::settings(state);
            true
        }
        TuiCommand::ToggleSyntaxHighlighting => {
            crate::modal_pane::settings::toggle_syntax_highlighting(state);
            true
        }
        TuiCommand::SettingsNextTab => {
            // Tab cycles between contexts depending on the active state.
            // Settings state → next tab. Question state → cycle focus
            // (questions → footer items). ModelPicker → cycle the
            // role pill. Other surfaces ignore Tab.
            if let Some(ModalState::Settings(s)) = state.ui.modal.as_mut() {
                s.next_tab();
            } else if matches!(
                state.ui.interaction.active_prompt,
                Some(PanePromptState::Question(_))
            ) {
                crate::bottom_pane::question::question_cycle_focus(state, 1);
            } else if matches!(state.ui.modal, Some(ModalState::ModelPicker(_))) {
                show::cycle_model_role(state, 1);
            }
            true
        }
        TuiCommand::SettingsPrevTab => {
            if let Some(ModalState::Settings(s)) = state.ui.modal.as_mut() {
                s.prev_tab();
            } else if matches!(
                state.ui.interaction.active_prompt,
                Some(PanePromptState::Question(_))
            ) {
                crate::bottom_pane::question::question_cycle_focus(state, -1);
            } else if matches!(state.ui.modal, Some(ModalState::ModelPicker(_))) {
                show::cycle_model_role(state, -1);
            }
            true
        }
        TuiCommand::ModelPickerCycleEffort(delta) => {
            crate::modal_pane::model_picker::cycle_effort(state, delta);
            true
        }
        TuiCommand::QuestionSwitchQuestion(delta) => {
            crate::bottom_pane::question::question_switch_question(state, delta);
            true
        }
        TuiCommand::TeamRosterCycleMode(delta) => {
            // Cycle the focused teammate's mode and apply it immediately.
            if let Some((name, mode)) = crate::modal_pane::team_roster::cycle_mode(state, delta) {
                let _ = command_tx
                    .send(UserCommand::SetTeammateMode { name, mode })
                    .await;
            }
            true
        }
        TuiCommand::TeamRosterCycleAllModes(delta) => {
            // Cycle ALL teammates in tandem and persist in one batch.
            let updates = crate::modal_pane::team_roster::cycle_all_modes(state, delta);
            if !updates.is_empty() {
                let _ = command_tx
                    .send(UserCommand::SetTeammateModes { updates })
                    .await;
            }
            true
        }
        TuiCommand::ModelPickerCycleRole(delta) => {
            show::cycle_model_role(state, delta);
            true
        }
        TuiCommand::ExecuteSkill(name) => {
            let _ = command_tx
                .send(UserCommand::ExecuteSkill { name, args: None })
                .await;
            true
        }
        TuiCommand::ExecuteSlashCommand(name) => {
            // `/exit` / `/quit` bound to a key (or chosen from the palette) take
            // the shared shutdown path, same as the typed form in `edit::submit`.
            if is_exit_command(name.as_str()) {
                shutdown_via_slash_command(state, command_tx).await;
                return true;
            }
            let _ = command_tx
                .send(UserCommand::ExecuteSlashCommand {
                    name,
                    args: String::new(),
                })
                .await;
            true
        }

        // ── Task management ──
        TuiCommand::BackgroundAllTasks => {
            // Background every foreground BgAgent on single-press Ctrl+B.
            // There is no wire event for the foreground→background
            // transition, so the TUI flips its own rows optimistically
            // before dispatching the engine command. `is_backgrounded` is a
            // sticky UI flag; the eventual `TaskCompleted` carries the real
            // terminal status into `agent.status`.
            if has_foreground_tasks(state) {
                for agent in state.session.subagents.iter_mut() {
                    if matches!(agent.kind, crate::state::SubagentKind::Subagent)
                        && matches!(agent.status, crate::state::SubagentStatus::Running)
                    {
                        agent.is_backgrounded = true;
                    }
                }
                let _ = command_tx.send(UserCommand::BackgroundAllTasks).await;
            }
            true
        }
        TuiCommand::KillAllAgents => {
            let _ = command_tx.send(UserCommand::KillAllAgents).await;
            true
        }

        // ── Display toggles ──
        TuiCommand::ToggleToolCollapse => {
            if state.ui.collapsed_tools.is_empty() {
                for tool in &state.session.tool_executions {
                    state.ui.collapsed_tools.insert(tool.call_id.clone());
                }
            } else {
                state.ui.collapsed_tools.clear();
            }
            true
        }
        TuiCommand::ToggleSystemReminders => {
            state.ui.show_system_reminders = !state.ui.show_system_reminders;
            true
        }
        TuiCommand::TogglePermissionExplanation => {
            // Ctrl+E on a permission prompt: toggle the risk-explainer panel.
            // On first open, kick off the lazy LLM fetch; the runner
            // replies with `TuiOnlyEvent::PermissionExplanationReady`.
            let Some(crate::state::PanePromptState::Permission(p)) =
                state.ui.interaction.active_prompt.as_mut()
            else {
                return false;
            };
            p.explanation_visible = !p.explanation_visible;
            if p.explanation_visible
                && matches!(p.explanation, crate::state::ExplainerFetch::NotFetched)
            {
                p.explanation = crate::state::ExplainerFetch::Loading;
                let request_id = p.request_id.clone();
                let tool_name = p.tool_name.clone();
                let tool_input = p.original_input.clone().unwrap_or(serde_json::Value::Null);
                let _ = command_tx
                    .send(UserCommand::RequestPermissionExplanation {
                        request_id,
                        tool_name,
                        tool_input,
                    })
                    .await;
            }
            true
        }

        // ── External editor / clipboard ──
        TuiCommand::OpenExternalEditor => {
            if state.session.is_busy() || state.ui.streaming.is_some() {
                state.ui.add_toast(crate::state::ui::Toast::warning(
                    "External editor is unavailable while a turn is running",
                ));
                return true;
            }
            let _ = command_tx
                .send(UserCommand::OpenPromptEditor {
                    initial_content: state.ui.input.text().to_string(),
                })
                .await;
            true
        }
        TuiCommand::OpenPlanEditor => {
            let _ = command_tx.send(UserCommand::OpenPlanEditor).await;
            true
        }
        TuiCommand::PasteFromClipboard => {
            clipboard::paste_from_clipboard(state).await;
            true
        }
        TuiCommand::CopyLastMessage => {
            clipboard::copy_last_message(state);
            true
        }

        // ── Application ──
        TuiCommand::Quit => {
            tracing::info!(
                exit_case = %ShutdownReason::ImmediateQuit,
                "immediate quit requested"
            );
            let _ = command_tx
                .send(UserCommand::Shutdown {
                    reason: ShutdownReason::ImmediateQuit,
                })
                .await;
            state.quit();
            true
        }

        // ── Stash ──
        TuiCommand::StashInputDraft => {
            stash::swap_input_draft(state);
            true
        }

        // ── Expanded right-rail view ──
        TuiCommand::ToggleExpandedTasksView => {
            expanded_view::cycle(state);
            true
        }
        TuiCommand::ToggleTeammateMessagePreview => {
            state.ui.show_teammate_message_preview = !state.ui.show_teammate_message_preview;
            true
        }
        TuiCommand::ToggleTranscript => {
            transcript::toggle(state);
            true
        }
        TuiCommand::TranscriptSelectNext => {
            transcript::select_expandable(state, 1);
            true
        }
        TuiCommand::TranscriptToggleCell => {
            transcript::toggle_selected_cell(state);
            true
        }
        TuiCommand::TranscriptScrollLines(delta) => {
            transcript::scroll_lines(state, delta);
            true
        }
        TuiCommand::TranscriptPage(delta) => {
            transcript::page(state, delta);
            true
        }
        TuiCommand::TranscriptJumpStart => {
            transcript::jump_start(state);
            true
        }
        TuiCommand::TranscriptJumpEnd => {
            transcript::jump_end(state);
            true
        }
    };

    if state.ui.input.text() != text_before || state.ui.input.textarea.cursor() != cursor_before {
        crate::autocomplete::refresh_suggestions(state);
    }
    changed
}

/// Loose upper bound for the active permission prompt's body scroll offset.
/// The render pass clamps precisely against the wrapped body height; this only
/// keeps the raw counter from running away on repeated `PageDown`.
fn permission_body_scroll_bound(state: &AppState) -> u16 {
    match state.ui.interaction.active_prompt.as_ref() {
        Some(crate::state::PanePromptState::Permission(p)) => {
            let lines = crate::presentation::request::permission_body_line_count(p);
            u16::try_from(lines.saturating_mul(2)).unwrap_or(u16::MAX)
        }
        _ => 0,
    }
}

fn accept_prompt_suggestion(state: &mut AppState) -> bool {
    if !state.ui.input.is_empty() || !state.session.queued_commands.is_empty() {
        return false;
    }
    let Some(suggestion) = state.session.prompt_suggestions.last().cloned() else {
        return false;
    };
    if suggestion.is_empty() {
        return false;
    }
    state.ui.input.set_text(&suggestion);
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.session.prompt_suggestions.clear();
    true
}

#[derive(Debug, Clone, Copy)]
enum ExitSource {
    CtrlC,
    CtrlD,
}

impl ExitSource {
    fn label(self) -> &'static str {
        match self {
            Self::CtrlC => "Ctrl-C",
            Self::CtrlD => "Ctrl-D",
        }
    }

    fn shutdown_reason(self) -> ShutdownReason {
        match self {
            Self::CtrlC => ShutdownReason::DoublePressCtrlC,
            Self::CtrlD => ShutdownReason::DoublePressCtrlD,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ExitTiming {
    expired_by_ms: Option<u128>,
}

impl ExitTiming {
    fn from_pending_until(
        pending_until: Option<std::time::Instant>,
        now: std::time::Instant,
    ) -> Self {
        Self {
            expired_by_ms: pending_until
                .and_then(|until| now.checked_duration_since(until))
                .map(|d| d.as_millis()),
        }
    }
}

/// Translate an [`ExitEffect`] (pure decision from `update::exit`) into
/// the matching side effects: `UserCommand` sends + terminal
/// `state.quit()`. **Does not** decide auto-restore — that's the
/// `TurnInterrupted` event handler's job in
/// `server_notification_handler::protocol::on_turn_interrupted`,
/// where `restoreMessageSync` runs after the abort completes.
async fn apply_exit_effect(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
    source: ExitSource,
    timing: ExitTiming,
    effect: ExitEffect,
) {
    match effect {
        ExitEffect::InterruptOnly => {
            tracing::info!(
                key = source.label(),
                exit_case = "interrupt_active_turn",
                "exit key interrupted active turn"
            );
            state.ui.ephemeral.mark_interrupting();
            let _ = command_tx
                .send(UserCommand::Interrupt(TurnAbortReason::UserCancel))
                .await;
        }
        ExitEffect::ClearInput => {
            // Idle Ctrl+C with text in the input: clear + save to history.
            // The exit hint is already armed by `on_interrupt`, so the
            // *next* Ctrl+C within the window goes through the Quit path.
            let taken = state.ui.input.take_input();
            state.ui.input.add_to_history(taken);
            state.ui.input.history_index = None;
            let prompt = state
                .ui
                .pending_exit_hint()
                .map(|key| t!("status.exit_prompt", key = key.label()).to_string())
                .unwrap_or_else(|| t!("status.exit_prompt", key = source.label()).to_string());
            tracing::info!(
                key = source.label(),
                exit_case = "clear_input",
                prompt,
                "exit key cleared draft input"
            );
        }
        ExitEffect::ArmOnly => {
            // First idle Ctrl+C / Ctrl+D: no interrupt, no state.
            // Tracker already updated by `exit::*`; renderer reads
            // `state.ui.pending_exit_hint()` to show the footer hint.
            let prompt = state
                .ui
                .pending_exit_hint()
                .map(|key| t!("status.exit_prompt", key = key.label()).to_string())
                .unwrap_or_else(|| t!("status.exit_prompt", key = source.label()).to_string());
            tracing::info!(
                key = source.label(),
                exit_case = "arm_exit_prompt",
                rearmed_after_timeout = timing.expired_by_ms.is_some(),
                expired_by_ms = timing.expired_by_ms.unwrap_or(0),
                window_ms = coco_tui_ui::constants::DOUBLE_PRESS_TIMEOUT.as_millis(),
                prompt,
                "exit prompt armed"
            );
        }
        ExitEffect::Quit => {
            let reason = source.shutdown_reason();
            tracing::info!(
                key = source.label(),
                exit_case = %reason,
                "exit confirmed; shutting down"
            );
            let _ = command_tx.send(UserCommand::Shutdown { reason }).await;
            state.quit();
        }
    }
}

/// Built-in exit command names (`exit` + its `quit` alias). Typed `/exit`
/// shuts the REPL down through the SAME path as the Ctrl+C / Ctrl+D
/// double-press exit — the registry handler only prints "Exiting…" and never
/// tears the TUI down. Mirrors TS `commands/exit` (`immediate: true` →
/// `gracefulShutdown`), whose own comment notes /exit, /quit, ctrl+c and
/// ctrl+d all funnel through one exit path.
pub(super) fn is_exit_command(name: &str) -> bool {
    matches!(name, "exit" | "quit")
}

/// Shut down in response to a typed `/exit` / `/quit`, reusing the
/// double-press exit mechanism (`UserCommand::Shutdown` + `state.quit()`) so
/// the in-flight turn is drained and the terminal restored cleanly.
pub(super) async fn shutdown_via_slash_command(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    tracing::info!(
        exit_case = %ShutdownReason::SlashCommand,
        "exit confirmed via slash command; shutting down"
    );
    let _ = command_tx
        .send(UserCommand::Shutdown {
            reason: ShutdownReason::SlashCommand,
        })
        .await;
    state.quit();
}

async fn queue_current_input(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) -> bool {
    let text = state.ui.input.take_input();
    if text.is_empty() {
        return true;
    }
    let resolved = state.ui.paste_manager.resolve_structured(&text);
    let _ = command_tx
        .send(UserCommand::QueueCommand {
            prompt: resolved.text,
            images: resolved.images,
        })
        .await;
    state.ui.paste_manager.clear();
    true
}

/// Whether any foreground tools / subagents are still running. Drives
/// the live Ctrl+B path in `TuiCommand::BackgroundAllTasks`. A subagent
/// flipped to `is_backgrounded` no longer counts; `Queued` tool
/// executions are excluded.
fn has_foreground_tasks(state: &AppState) -> bool {
    let any_running_subagent =
        state.session.subagents.iter().any(|s| {
            matches!(s.status, crate::state::SubagentStatus::Running) && !s.is_backgrounded
        });
    let any_running_tool = state
        .session
        .tool_executions
        .iter()
        .any(|t| matches!(t.status, crate::state::ToolStatus::Running));
    any_running_subagent || any_running_tool
}

/// Localised label for a permission mode, used in toasts/banners so the
/// user sees the same wording the help state and status row use.
/// `pub(crate)` so the state deny handler can reuse it without
/// duplicating the match — keeps mode wording consistent across the
/// TogglePlanMode / Cycle / state-decline surfaces.
pub(crate) fn permission_mode_label(mode: coco_types::PermissionMode) -> String {
    let key = match mode {
        coco_types::PermissionMode::Default => "permission_mode.default",
        coco_types::PermissionMode::Plan => "permission_mode.plan",
        coco_types::PermissionMode::AcceptEdits => "permission_mode.accept_edits",
        coco_types::PermissionMode::BypassPermissions => "permission_mode.bypass",
        coco_types::PermissionMode::Auto => "permission_mode.auto",
        coco_types::PermissionMode::DontAsk => "permission_mode.dont_ask",
        coco_types::PermissionMode::Bubble => "permission_mode.bubble",
    };
    t!(key).to_string()
}
