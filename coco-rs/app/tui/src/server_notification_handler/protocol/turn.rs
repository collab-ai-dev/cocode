//! Turn-lifecycle handlers — the `TurnEnded` outcome family
//! (completed / failed / interrupted / max-turns / budget-exhausted),
//! the auto-restore rewind, and the session-boundary cleanup shared by
//! `/clear` and resume.
//!
//! Split from `protocol.rs` so the flat 62-arm match stays readable;
//! `protocol::handle` routes `TurnEnded` here via [`on_turn_ended`].

use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::session::SubagentKind;
use crate::state::session::SubagentStatus;
use crate::state::session::TokenUsage;
use crate::state::ui::Toast;

/// Clear UI projections that are scoped to the old conversation tail.
/// Persistent background activity is kept deliberately: teammates are
/// process-owned rows, and backgrounded running subagents continue after
/// `/clear` / resume. Everything else is transcript-adjacent and cannot
/// safely survive a session boundary.
pub(super) fn clear_session_boundary_state(state: &mut AppState) {
    let retained_subagent_ids: std::collections::HashSet<String> = state
        .session
        .subagents
        .iter()
        .filter(|agent| {
            matches!(agent.kind, SubagentKind::Teammate)
                || (matches!(agent.kind, SubagentKind::Subagent)
                    && matches!(agent.status, SubagentStatus::Running)
                    && agent.is_backgrounded)
        })
        .map(|agent| agent.agent_id.clone())
        .collect();
    state
        .session
        .subagents
        .retain(|agent| retained_subagent_ids.contains(&agent.agent_id));
    state
        .session
        .active_tasks
        .retain(|task| retained_subagent_ids.contains(&task.task_id));

    state.session.set_busy(false);
    state.session.current_turn_number = None;
    state.session.session_state = coco_types::SessionState::Idle;
    state.session.is_compacting = false;
    state.session.compaction_started_at = None;
    state.session.compaction_phase = None;
    state.session.stream_stall = false;
    state.session.tool_executions.clear();
    state.session.tool_group_summaries.clear();
    state.session.clear_reasoning_metadata();
    state.session.subagent_summaries.clear();
    state.session.session_usage = None;
    state.session.token_usage = crate::state::session::TokenUsage::default();
    state.session.queued_commands.clear();
    state.session.active_hooks.clear();
    state.session.prompt_suggestions.clear();
    state.session.local_command_output.clear();
    state.session.plan_tasks.clear();
    state.session.todos_by_agent.clear();
    state.session.expanded_view = coco_types::ExpandedView::None;
    state.session.verification_nudge_pending = false;
    state.session.last_agent_markdown = None;

    state.ui.streaming = None;
    state.ui.collapsed_tools.clear();
    state.ui.clear_surfaces();
    state.ui.interaction.delayed_permissions.clear();
    state.ui.ephemeral = crate::state::ui_ephemeral::UiEphemeralState::new();
}

/// Handle `TurnCompleted`: finalize usage, flush streaming buffer into the
/// message list, prune completed tools.
///
/// Dispatcher for the unified `TurnEnded` event. Routes to one of five
/// per-outcome handlers; preserves the auto-restore path on `Interrupted`
/// (see `on_turn_interrupted_outcome`). Pairs 1:1 with `TurnStarted`.
pub(super) fn on_turn_ended(
    state: &mut AppState,
    p: coco_types::TurnEndedParams,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) -> bool {
    match &p.outcome {
        coco_types::TurnOutcome::Completed(_) => on_turn_completed_outcome(state, &p),
        coco_types::TurnOutcome::Failed(data) => on_turn_failed_outcome(state, &data.error),
        coco_types::TurnOutcome::Interrupted(data) => {
            on_turn_interrupted_outcome(state, data.abort_reason, command_tx)
        }
        coco_types::TurnOutcome::MaxTurnsReached(data) => {
            on_max_turns_reached_outcome(state, Some(data.max_turns))
        }
        coco_types::TurnOutcome::BudgetExhausted(data) => {
            on_budget_exhausted_outcome(state, data.used_tokens, data.budget_tokens)
        }
    }
}
// end on_turn_ended

/// `TurnEnded(Completed)` handler. Folds usage, end-of-turn UI state,
/// notifications, and tool-execution cleanup.
///
/// Does NOT handle auto-restore — that lives in
/// [`on_turn_interrupted_outcome`]. `Completed` fires only on natural
/// turn end; cancel paths take the `Interrupted` branch.
fn on_turn_completed_outcome(state: &mut AppState, p: &coco_types::TurnEndedParams) -> bool {
    state.session.set_busy(false);
    // `updateLastInteractionTime(true)` fires here so the idle window
    // starts ticking from "user has had a chance to read the response",
    // not "agent stopped".
    let now = state.clock.now();
    state.session.last_query_completion_at = Some(now);
    state.session.last_user_interaction_at = now;
    state.session.idle_prompt_fired = false;
    // `p.usage` is `Option<TokenUsage>` since the refactor — `None`
    // means the emitter didn't have access to accumulated usage
    // (runner-side bail, late-cancel). Skip the token fold in that
    // case; the SessionUsage emit path is the authoritative live
    // counter when no per-turn snapshot is available.
    if state.session.session_usage.is_none()
        && let Some(usage) = p.usage.as_ref()
    {
        state.session.update_tokens(TokenUsage {
            input_tokens: usage.input_tokens.total,
            output_tokens: usage.output_tokens.total,
            reasoning_tokens: usage.output_tokens.reasoning,
            cache_read_tokens: usage.input_tokens.cache_read,
            cache_creation_tokens: usage.input_tokens.cache_write,
        });
    }
    // Emit a terminal notification when the user has switched away — they
    // typically want a ping when a long turn finishes in the background.
    // Skips when the terminal is focused to avoid pointless noise.
    if !state.ui.terminal_focused {
        coco_tui_ui::widgets::notification::notify(
            &t!("notification.app_name"),
            &t!("notification.turn_complete"),
        );
    }
    // Telemetry-only — reasoning duration is now attached by the
    // engine via `ReasoningMetadataAttached`. Keep the local
    // computation for now in case future telemetry needs it.
    let _token_only_duration_ms: Option<i64> = state
        .ui
        .ephemeral
        .turn_started_at()
        .and_then(|started_at| started_at.elapsed().as_millis().try_into().ok())
        .or_else(|| {
            state.ui.streaming.as_ref().and_then(|streaming| {
                streaming
                    .segment_started_at
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .ok()
            })
        });
    crate::server_notification_handler::projection::flush_streaming_to_messages(state);
    // F3 — reasoning aggregates are stamped by the dedicated
    // `ReasoningMetadataAttached` handler (engine emits with the
    // assistant message UUID), so no cell-walk anchoring here.
    state.ui.ephemeral.end_turn();
    // Drop resolved rows (now folded into transcript ToolResult cells) and any
    // in-flight orphan. `Streaming`/uncommitted-`Queued` rows that never reached
    // a committed call (e.g. a provider that truncated the tool-call args mid
    // stream — `engine.rs` drops the incomplete call, so no `ToolUseQueued`
    // arrives) would otherwise persist forever as a ghost activity row; only
    // genuinely-executing (`Running`) or committed (`Queued` + stamped) rows survive.
    use crate::state::session::ToolStatus;
    state.session.tool_executions.retain(|t| match t.status {
        ToolStatus::Running => true,
        ToolStatus::Queued => t.message_uuid.is_some(),
        ToolStatus::Streaming | ToolStatus::Completed | ToolStatus::Failed => false,
    });
    true
}

/// Handle `TurnEnded(Interrupted)`: clear streaming state, surface the
/// banner, and run auto-restore when the cancel was user-initiated AND
/// the idle guards + lossless-tail predicate hold.
///
/// Corresponds to the `.finally` block that fires after
/// `abortController.abort('user-cancel')` resolves the query.
pub(super) fn on_turn_interrupted_outcome(
    state: &mut AppState,
    abort_reason: coco_types::TurnAbortReason,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) -> bool {
    state.session.set_busy(false);
    state.ui.ephemeral.end_turn();
    state.ui.streaming = None;
    // Drop in-flight tool widgets — same rationale as `TurnEnded(Failed)`.
    // The cancel aborts the turn before tools could resolve to a
    // `Message::ToolResult`, so any unstamped (= mid-turn) execution
    // would otherwise leak across the interrupt boundary.
    let before = state.session.tool_executions.len();
    state
        .session
        .tool_executions
        .retain(|t| t.message_uuid.is_some());
    let dropped = before.saturating_sub(state.session.tool_executions.len());
    tracing::info!(
        target: "coco_tui::turn",
        abort_reason = ?abort_reason,
        tool_widgets_dropped = dropped,
        tool_widgets_remaining = state.session.tool_executions.len(),
        "TurnEnded(Interrupted)",
    );

    let user_cancel = matches!(abort_reason, coco_types::TurnAbortReason::UserCancel);

    // Auto-restore is gated on:
    // - reason == UserCancel  (treat None/legacy senders as non-user-initiated — conservative)
    // - empty input            (`inputValueRef.current === ''`)
    // - empty queue            (`getCommandQueueLength() === 0`)
    // - no active surface      (not viewing a teammate task, no modal up)
    // - lossless tail          (`messagesAfterAreOnlySynthetic`)
    // Predicates walk the engine-authoritative cell list directly.
    let cells = state.session.transcript.cells();
    let mut auto_restored = false;
    if user_cancel
        && state.ui.input.is_empty()
        && state.session.queued_commands.is_empty()
        && !state.ui.has_active_surface()
        && let Some(idx) = crate::update_rewind::find_last_user_cell_index(cells)
        && crate::update_rewind::cells_after_are_only_synthetic(cells, idx)
    {
        // Snapshot the index so we can mutate state below without
        // reborrowing `cells` (which would conflict with `state.ui`/
        // `state.session` mutations).
        apply_auto_restore(state, idx, command_tx);
        auto_restored = true;
    }

    // The user interruption message renders as the dim
    // `Interrupted · What should Claude do instead?` chat row. Only fires
    // for UserCancel — SystemPreempt means a sibling op
    // (Clear/Compact/Rewind/Shutdown) is about to mutate history anyway.
    // Skipped when auto-restore truncated to the last user prompt: the
    // prompt is now back in the input and adding "you interrupted yourself"
    // would be noise.
    //
    // The engine's `finalize_user_cancel` pushes a typed
    // `SystemMessage::UserInterruption` with the authoritative
    // `for_tool_use`; the MessageAppended event populates `transcript`,
    // and the renderer surfaces it from there.
    let _ = (user_cancel, auto_restored);
    true
}

/// In-place auto-restore. Truncates the message list at `idx` (the last
/// user message), pops the user's text back into the input bar,
/// regenerates `conversation_id` so the next turn starts a fresh cache
/// key, and clears UI state that no longer corresponds to a real
/// conversation tail.
///
/// Dispatches `UserCommand::Rewind { mode: AutoRestore }` directly via
/// `command_tx.try_send`. The engine truncates its authoritative
/// history and emits `ServerNotification::MessageTruncated`, keeping
/// engine + TUI + SDK converged (see
/// `engine-tui-unified-transcript-plan.md` §7.4).
fn apply_auto_restore(
    state: &mut AppState,
    idx: usize,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) {
    let cells = state.session.transcript.cells();
    let Some(cell) = cells.get(idx) else {
        tracing::warn!(
            target: "coco_tui::auto_restore",
            idx,
            cells_len = cells.len(),
            "apply_auto_restore: cell index out of bounds — skipping",
        );
        return;
    };
    let target_message_id = cell.message_uuid.to_string();
    let input_text = match &cell.kind {
        crate::transcript::cells::CellKind::UserText { text } => text.clone(),
        _ => String::new(),
    };
    let perm = match cell.source.as_ref() {
        coco_messages::Message::User(u) => u.permission_mode,
        _ => None,
    };
    // Phase 3d (§5): the renderer reads from `transcript.cells()`
    // directly. The engine emits `MessageTruncated` after our follow-up
    // `UserCommand::Rewind { mode: AutoRestore }` dispatch, which truncates
    // `transcript` to the same boundary.
    tracing::info!(
        target: "coco_tui::auto_restore",
        target_message_id = %target_message_id,
        cell_idx = idx,
        input_chars = input_text.len(),
        permission_mode = ?perm,
        "apply_auto_restore: queueing Rewind AutoRestore dispatch",
    );
    if let Some(mode) = perm {
        state.session.permission_mode = mode;
    }
    if !input_text.is_empty() {
        state.ui.input.textarea.set_text(&input_text);
        let eol = state.ui.input.textarea.end_of_current_line();
        state.ui.input.textarea.set_cursor(eol);
    }
    state.session.conversation_id = Some(uuid::Uuid::new_v4().to_string());
    state.session.prompt_suggestions.clear();
    state.ui.paste_manager.clear();
    state.ui.scroll_offset = 0;
    state.ui.user_scrolled = false;
    // Direct dispatch (no `pending_*` round-trip). `try_send` rather
    // than blocking `send` — the channel has slack; if it's full the
    // event loop is wedged for unrelated reasons and a dropped
    // auto-restore is the right fallback.
    if let Err(e) = command_tx.try_send(crate::command::UserCommand::Rewind {
        message_id: target_message_id.clone(),
        mode: crate::command::RewindMode::AutoRestore,
    }) {
        tracing::warn!(
            target: "coco_tui::auto_restore",
            target_message_id = %target_message_id,
            error = ?e,
            "apply_auto_restore: failed to dispatch Rewind AutoRestore",
        );
    }
}

/// `TurnEnded(Failed)` handler. Engine-level failure: clear streaming
/// state and drop in-flight tool widgets. The user-facing error renders
/// inline in the transcript — the engine appends a `SystemMessage::ApiError`
/// row (`⚠ <error>`) before emitting this event (TS `SystemAPIErrorMessage`
/// parity), so this handler raises neither a toast nor a blocking modal.
fn on_turn_failed_outcome(state: &mut AppState, error: &coco_types::ErrorPayload) -> bool {
    state.session.set_busy(false);
    state.ui.ephemeral.end_turn();
    state.ui.streaming = None;
    let before = state.session.tool_executions.len();
    state
        .session
        .tool_executions
        .retain(|t| t.message_uuid.is_some());
    let dropped = before.saturating_sub(state.session.tool_executions.len());
    if dropped > 0 {
        tracing::info!(
            target: "coco_tui::turn",
            dropped,
            remaining = state.session.tool_executions.len(),
            error = %error.message,
            code = ?error.code,
            "TurnEnded(Failed): dropped in-flight tool widgets",
        );
    } else {
        tracing::warn!(
            target: "coco_tui::turn",
            error = %error.message,
            code = ?error.code,
            "TurnEnded(Failed)",
        );
    }
    true
}

/// `TurnEnded(MaxTurnsReached)` handler. Turn budget exhausted — show a
/// modal so the user explicitly acknowledges the stop instead of
/// silently continuing on next prompt.
fn on_max_turns_reached_outcome(state: &mut AppState, max_turns: Option<i32>) -> bool {
    state.session.set_busy(false);
    state.ui.ephemeral.end_turn();
    state.ui.streaming = None;
    // Drop in-flight tool widgets (same rationale as Failed/Interrupted): a
    // terminal stop leaves unstamped streaming/queued rows that would ghost
    // the activity strip across the next prompt.
    state
        .session
        .tool_executions
        .retain(|t| t.message_uuid.is_some());
    let msg = match max_turns {
        Some(n) => t!("toast.max_turns_reached", n = n).to_string(),
        None => t!("toast.max_turns_unbounded").to_string(),
    };
    state.ui.add_toast(Toast::warning(msg.clone()));
    let body = crate::widgets::error_dialog::format_error_body(&msg, Some("limit"), false);
    state.ui.show_modal(ModalState::Error(body));
    true
}

/// `TurnEnded(BudgetExhausted)` handler. Token budget (90%/diminishing-
/// returns) exhausted distinctly from max_turns. UI-wise treated as a
/// stop-with-acknowledge, but the toast carries token figures so the
/// user can decide whether to keep going. `budget_tokens` is
/// `Option` because the engine emits `None` when no explicit
/// `config.max_tokens` was set.
fn on_budget_exhausted_outcome(
    state: &mut AppState,
    used_tokens: i64,
    budget_tokens: Option<i64>,
) -> bool {
    state.session.set_busy(false);
    state.ui.ephemeral.end_turn();
    state.ui.streaming = None;
    // Drop in-flight tool widgets (same rationale as Failed/Interrupted).
    state
        .session
        .tool_executions
        .retain(|t| t.message_uuid.is_some());
    let msg = match budget_tokens {
        Some(budget) => {
            format!("Token budget exhausted: used {used_tokens} of {budget}. Stop and acknowledge.")
        }
        None => {
            format!("Token budget exhausted: used {used_tokens} tokens. Stop and acknowledge.")
        }
    };
    state.ui.add_toast(Toast::warning(msg.clone()));
    let body = crate::widgets::error_dialog::format_error_body(&msg, Some("budget"), false);
    state.ui.show_modal(ModalState::Error(body));
    true
}
