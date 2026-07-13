use coco_types::CoreEvent;
use tokio::sync::mpsc;

use crate::app_server_host::outbound::{OutboundMessage, send_session_event};

/// Drain per-turn CoreEvents and forward to the outbound notification
/// channel, intercepting session envelope events.
///
/// Specifically:
/// - per-turn `SessionResult` events are **not** forwarded as standalone
///   events. Instead, their stats are folded into the selected runtime's
///   accounting and attached to the terminal `TurnEnded` before that event is
///   forwarded. The aggregated session-level `SessionResult` is still emitted
///   once when `session/close` runs.
/// - `SessionStarted` events are also swallowed (defensive — the current
///   runner doesn't emit them, but if a future runner enables the
///   bootstrap path, we still want exactly one per session from the remote
///   bridge side, not one per turn).
/// - All other events pass through unchanged.
///
/// `owner_session_id` is the session this forwarder was created for and is
/// used to stamp every outbound envelope.
pub(in crate::host::app_server_host::request_handlers) async fn forward_turn_events(
    mut rx: mpsc::Receiver<CoreEvent>,
    tx: mpsc::Sender<OutboundMessage>,
    session: crate::session_runtime::SessionHandle,
    owner_session_id: coco_types::SessionId,
) {
    use coco_types::ServerNotification;
    // Clear the active-turn slot on the FIRST terminal `TurnEnded` only, so a
    // stray second terminal event can never wipe a fast next turn's slot.
    let mut turn_slot_cleared = false;
    let mut pending_terminal: Option<coco_types::TurnEndedParams> = None;
    let mut last_session_result: Option<coco_types::SessionResultParams> = None;
    let mut session_result_accounted = false;
    while let Some(event) = rx.recv().await {
        match event {
            CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
                let result = *params;
                if !session_result_accounted {
                    session.accumulate_session_result(&result);
                    session_result_accounted = true;
                }
                last_session_result = Some(result.clone());
                if let Some(ended) = pending_terminal.take()
                    && !forward_terminal_event(
                        &tx,
                        &owner_session_id,
                        ended.with_session_result(result),
                        &session,
                        &mut turn_slot_cleared,
                    )
                    .await
                {
                    break;
                }
                // Swallow standalone per-turn result — aggregated result is
                // emitted by session/close.
            }
            CoreEvent::Protocol(ServerNotification::SessionStarted(_)) => {
                // Swallow: SessionStarted is owned by the remote client server, not the engine.
            }
            CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) => {
                session.mark_active_turn_finishing();
                if let Some(result) = last_session_result.clone() {
                    if !forward_terminal_event(
                        &tx,
                        &owner_session_id,
                        ended.with_session_result(result),
                        &session,
                        &mut turn_slot_cleared,
                    )
                    .await
                    {
                        break;
                    }
                } else if let Some(result) = ended.session_result.as_deref().cloned() {
                    if !session_result_accounted {
                        session.accumulate_session_result(&result);
                        session_result_accounted = true;
                    }
                    last_session_result = Some(result.clone());
                    if !forward_terminal_event(
                        &tx,
                        &owner_session_id,
                        ended.with_session_result(result),
                        &session,
                        &mut turn_slot_cleared,
                    )
                    .await
                    {
                        break;
                    }
                } else {
                    // Keep the latest terminal event. If a late interrupted
                    // terminal replaces a completed one before the per-turn
                    // result arrives, the latest terminal is authoritative.
                    pending_terminal = Some(ended);
                }
            }
            other => {
                if send_session_event(&tx, owner_session_id.clone(), other)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    if let Some(ended) = pending_terminal {
        let ended = if let Some(result) = last_session_result {
            ended.with_session_result(result)
        } else {
            ended
        };
        let _ = forward_terminal_event(
            &tx,
            &owner_session_id,
            ended,
            &session,
            &mut turn_slot_cleared,
        )
        .await;
    }
    if !turn_slot_cleared {
        // Returning from TurnRunner closes this channel and is also a clean
        // completion signal. A custom runner that omits TurnEnded must not
        // permanently occupy the session's active-turn slot.
        session.mark_active_turn_finishing();
        session.complete_finishing_active_turn();
    }
}

async fn forward_terminal_event(
    tx: &mpsc::Sender<OutboundMessage>,
    owner_session_id: &coco_types::SessionId,
    ended: coco_types::TurnEndedParams,
    session: &crate::session_runtime::SessionHandle,
    turn_slot_cleared: &mut bool,
) -> bool {
    // Free the per-session turn slot BEFORE forwarding the terminal
    // `TurnEnded`, so the client's next `turn/start` (sent the instant it sees
    // `TurnEnded`) finds the slot free instead of racing the turn task's own
    // clear and hitting `TurnAlreadyRunning`. This is now the sole clear site
    // for the active-turn record; the turn/compact tasks no longer clear it.
    if !*turn_slot_cleared {
        session.complete_finishing_active_turn();
        *turn_slot_cleared = true;
    }
    send_session_event(
        tx,
        owner_session_id.clone(),
        CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(ended)),
    )
    .await
    .is_ok()
}
