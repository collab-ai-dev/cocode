//! Engine ↔ TUI/SDK history sync helpers.
//!
//! Pairs every `MessageHistory::push` with a `MessageAppended` event so
//! TUI / SDK consumers can maintain a derived view without recomputing
//! engine-internal state. Centralizes the cancel-marker finalization so
//! `for_tool_use` is computed once at the engine and never recomputed
//! downstream.
//!
//! See `engine-tui-unified-transcript-plan.md` §5.
//!
//! Backward compatibility note: `last_message_is_user_interruption`
//! recognizes both the new `SystemMessage::UserInterruption` form
//! emitted by `finalize_user_cancel` and the legacy
//! `INTERRUPT_MESSAGE` / `INTERRUPT_MESSAGE_FOR_TOOL_USE` text marker
//! that older JSONL transcripts may contain. Dedup works on both forms
//! across resume boundaries.

use std::sync::Arc;

use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::SystemMessage;
use coco_messages::create_user_interruption_system_message;
use coco_types::CoreEvent;
use coco_types::ProviderModelSelection;
use coco_types::ServerNotification;
use coco_types::TokenUsage;
use tokio::sync::mpsc::Sender;

use crate::emit::emit_protocol;

/// `tracing` target for engine↔TUI/SDK history sync.
///
/// Use this canonical target on every emit so operators can pivot a
/// single filter (`coco::history_sync=debug`) to trace the full
/// authority round-trip without drowning in unrelated `coco_query`
/// chatter.
const HISTORY_SYNC_TARGET: &str = "coco::history_sync";

/// Read the F9 envelope (session_id + agent_id) off the history and
/// stamp it onto a transcript-lifecycle event payload. AgentTeams
/// consumers demux merged timelines by these two fields; single-session
/// SDK consumers ignore them (forward-compat via `#[serde(default)]`).
///
/// Envelope lives on `MessageHistory` (set by the engine builder) so
/// every helper here picks it up automatically — no per-call threading.
fn envelope_from(history: &MessageHistory) -> (String, Option<String>) {
    (
        history.session_id().to_string(),
        history.agent_id().map(str::to_string),
    )
}

/// Push `msg` into `history` and emit a typed `MessageAppended` protocol
/// notification. The push allocates one `Arc<Message>`; the same Arc
/// is stored in history and forwarded on the wire — no deep `Message`
/// clone (see `engine-tui-unified-transcript-plan.md` §11 F8).
///
/// The wire payload carries `session_id` + `agent_id` pulled from
/// the history (§11 F9) — engine sets these once at history
/// construction.
pub async fn history_push_and_emit(
    history: &mut MessageHistory,
    msg: Message,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    history_push_arc_and_emit(history, Arc::new(msg), event_tx).await;
}

/// Push an already-`Arc`-wrapped message — used for re-committing
/// drained errors (`drain_pushed_since` round-trip) where the engine
/// already owns the Arc and doesn't need to reallocate.
pub async fn history_push_arc_and_emit(
    history: &mut MessageHistory,
    msg: Arc<Message>,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    let (session_id, agent_id) = envelope_from(history);
    let arc = history.push_arc(msg);
    let uuid = arc.uuid().copied();
    let kind = arc.kind();
    tracing::debug!(
        target: HISTORY_SYNC_TARGET,
        ?uuid,
        ?kind,
        history_len = history.len(),
        has_tx = event_tx.is_some(),
        "history append",
    );
    let _delivered = emit_protocol(
        event_tx,
        ServerNotification::MessageAppended {
            message: arc,
            session_id,
            agent_id,
        },
    )
    .await;
}

/// Atomic push + LastUsageMarker anchor for the success path of an
/// API call. The combined call site eliminates the prior two-step
/// "push then anchor" sequence — there is no window between the wire
/// `MessageAppended` event and marker installation. `msg` MUST be a
/// `Message::Assistant`; `usage` and `model` come from the
/// `QueryResult` / `StreamEvent::Finish` of the same successful call.
pub async fn history_push_assistant_with_usage_and_emit(
    history: &mut MessageHistory,
    msg: Message,
    usage: TokenUsage,
    model: ProviderModelSelection,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    let (session_id, agent_id) = envelope_from(history);
    let arc = history.push_arc_assistant_with_usage(Arc::new(msg), usage, model);
    let uuid = arc.uuid().copied();
    let kind = arc.kind();
    tracing::debug!(
        target: HISTORY_SYNC_TARGET,
        ?uuid,
        ?kind,
        history_len = history.len(),
        has_tx = event_tx.is_some(),
        "history append (with usage anchor)",
    );
    let _delivered = emit_protocol(
        event_tx,
        ServerNotification::MessageAppended {
            message: arc,
            session_id,
            agent_id,
        },
    )
    .await;
}

/// Clear `history` and emit `MessageTruncated { keep_count: 0 }`. The
/// symmetric companion to [`history_push_and_emit`] — every transcript
/// mutation goes through a wire-visible event so the TUI's
/// `TranscriptView` and SDK NDJSON observers stay coherent with engine
/// state. Use this for plan-mode-exit clears and any other
/// "drop entire history" path that should NOT rotate session_id.
///
/// For `/clear` (which rotates session_id), call
/// [`history_clear_and_emit_session_reset`] instead so consumers
/// also rotate `conversation_id` / re-key the prompt cache.
pub async fn history_clear_and_emit(
    history: &mut MessageHistory,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    let (session_id, agent_id) = envelope_from(history);
    let removed = history.len();
    history.clear();
    tracing::info!(
        target: HISTORY_SYNC_TARGET,
        removed,
        has_tx = event_tx.is_some(),
        "history cleared (no session rotation)",
    );
    let _delivered = emit_protocol(
        event_tx,
        ServerNotification::MessageTruncated {
            keep_count: 0,
            session_id,
            agent_id,
        },
    )
    .await;
}

/// Clear `history` and emit `SessionResetForResume { session_id }`. Use
/// for `/clear` paths that rotate the session id — the same event the
/// resume path uses, since TUI / SDK consumers handle both with the
/// same teardown (wipe transcript, clear overlays, re-key
/// `conversation_id`).
pub async fn history_clear_and_emit_session_reset(
    history: &mut MessageHistory,
    new_session_id: String,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    let agent_id = history.agent_id().map(str::to_string);
    let removed = history.len();
    history.clear();
    tracing::info!(
        target: HISTORY_SYNC_TARGET,
        removed,
        new_session_id = %new_session_id,
        has_tx = event_tx.is_some(),
        "history cleared + session reset",
    );
    let _delivered = emit_protocol(
        event_tx,
        ServerNotification::SessionResetForResume {
            session_id: new_session_id,
            agent_id,
        },
    )
    .await;
}

/// Replace `history.messages` wholesale and emit a single
/// [`ServerNotification::HistoryReplaced`] carrying the new snapshot.
///
/// Used by compaction (partial / session-memory / full / reactive
/// head-trim) and any other "swap the whole transcript" path. The
/// TUI's derived view processes this in one cache-rebuild pass via
/// [`crate::TranscriptView::replace_from_messages`], avoiding the
/// channel-bounded N-event burst the older `MessageTruncated{0}` +
/// N×`MessageAppended` sequence required.
///
/// Empty `new_messages` is allowed — equivalent to emitting
/// `HistoryReplaced { messages: vec![] }`, which clears the derived
/// view exactly like [`history_clear_and_emit`] but without
/// rotating the session id.
pub async fn history_replace_and_emit(
    history: &mut MessageHistory,
    new_messages: Vec<Arc<Message>>,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    let (session_id, agent_id) = envelope_from(history);
    let removed = history.len();
    let incoming = new_messages.len();
    history.clear();
    let mut snapshot: Vec<Arc<Message>> = Vec::with_capacity(incoming);
    for arc in new_messages {
        // `push_arc` stores the same Arc in history — no re-allocation,
        // no `Message::clone`.
        snapshot.push(history.push_arc(arc));
    }
    tracing::info!(
        target: HISTORY_SYNC_TARGET,
        removed,
        incoming,
        has_tx = event_tx.is_some(),
        "history replace: single HistoryReplaced event",
    );
    let _delivered = emit_protocol(
        event_tx,
        ServerNotification::HistoryReplaced {
            messages: snapshot,
            session_id,
            agent_id,
        },
    )
    .await;
}

/// Whether a turn-abort reason is *steering* — the user submitted/queued new
/// input while tools were running — rather than a hard cancel (Ctrl+C / ESC).
///
/// On steering, the follow-up queued user message provides conversational
/// continuity, so the standalone [`SystemMessage::UserInterruption`] marker is
/// redundant and suppressed at the `finalize_user_cancel` call sites. This
/// matches the TS implementation, which skips `createUserInterruptionMessage`
/// when `abortController.signal.reason === 'interrupt'` (`query.ts:1046`).
///
/// The per-tool `tool_result`s carrying `INTERRUPT_MESSAGE_FOR_TOOL_USE` are
/// **not** affected — they are required for strict tool_use/tool_result
/// pairing and accurately record that each in-flight tool was interrupted.
pub fn is_steering_interrupt(reason: Option<coco_types::TurnAbortReason>) -> bool {
    matches!(reason, Some(coco_types::TurnAbortReason::SubmitInterrupt))
}

/// Single writer for the user-cancel marker. Reads `in_flight_tool_calls`
/// from the engine (which holds the authoritative view of running tool
/// state at the cancel checkpoint) and pushes a typed
/// `SystemMessage::UserInterruption`; downstream consumers read
/// `for_tool_use` from the field and never recompute it.
///
/// Dedup: returns early if the last history entry is already a
/// `UserInterruption` (or legacy text-form marker from a resumed
/// transcript). Idempotent: double-cancel does not produce a duplicate marker.
pub async fn finalize_user_cancel(
    history: &mut MessageHistory,
    in_flight_tool_calls: bool,
    event_tx: &Option<Sender<CoreEvent>>,
) {
    if last_message_is_user_interruption(history) {
        // Common path on rapid double-Ctrl+C or cancel after a
        // previously-cancelled turn. `debug` (not `warn`) — it's the
        // documented dedup contract, not a bug. Operators chasing
        // "why didn't a second marker appear?" pivot on this line.
        tracing::debug!(
            target: HISTORY_SYNC_TARGET,
            in_flight_tool_calls,
            tail_kind = ?history.last().map(|m| m.kind()),
            "finalize_user_cancel: dedup skipped (tail already an interruption marker)",
        );
        return;
    }
    tracing::info!(
        target: HISTORY_SYNC_TARGET,
        in_flight_tool_calls,
        history_len_before = history.len(),
        "finalize_user_cancel: pushing UserInterruption marker",
    );
    let msg = create_user_interruption_system_message(in_flight_tool_calls);
    history_push_and_emit(history, msg, event_tx).await;
}

/// True when the most recent semantically-meaningful message is a
/// user-cancellation marker — either the typed
/// `SystemMessage::UserInterruption` form emitted by
/// `finalize_user_cancel` or the legacy `INTERRUPT_MESSAGE*` text-form
/// User message preserved for older JSONL transcripts on resume.
///
/// Skips trailing UI-only ephemera (`Progress`, `Tombstone`) when
/// scanning from the tail. A `Progress` row landing between two
/// rapid Ctrl+C presses (e.g. a tool emitting late progress on the
/// cancelled call) must not break dedup: the second cancel would
/// otherwise see the Progress as `last()` and push a duplicate
/// `UserInterruption`.
pub fn last_message_is_user_interruption(history: &MessageHistory) -> bool {
    for msg in history.as_slice().iter().rev() {
        match msg.as_ref() {
            Message::Progress(_) | Message::Tombstone(_) => continue,
            Message::System(SystemMessage::UserInterruption(_)) => return true,
            Message::User(user) => {
                let coco_messages::LlmMessage::User { content, .. } = &user.message else {
                    return false;
                };
                let [coco_llm_types::UserContentPart::Text(text_part)] = content.as_slice() else {
                    return false;
                };
                return text_part.text == coco_messages::INTERRUPT_MESSAGE
                    || text_part.text == coco_messages::INTERRUPT_MESSAGE_FOR_TOOL_USE;
            }
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
#[path = "history_sync.test.rs"]
mod tests;
