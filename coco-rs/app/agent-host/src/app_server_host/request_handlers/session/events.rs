use coco_types::CoreEvent;
use tokio::sync::mpsc;

use crate::app_server_host::outbound::{OutboundMessage, send_session_event};

/// Drain per-turn CoreEvents and forward to the outbound notification
/// channel, intercepting session envelope events.
///
/// Specifically:
/// - `SessionResult` events are **not** forwarded. Instead, their stats
///   are folded into the selected runtime's accounting. The aggregated
///   `SessionResult` is emitted once
///   when `session/archive` runs.
/// - `SessionStarted` events are also swallowed (defensive — the current
///   runner doesn't emit them, but if a future runner enables the
///   bootstrap path, we still want exactly one per session from the remote
///   bridge side, not one per turn).
/// - All other events pass through unchanged.
///
/// `owner_session_id` is the session this forwarder was created for and is
/// used to stamp every outbound envelope.
pub(in crate::app_server_host::request_handlers) async fn forward_turn_events(
    mut rx: mpsc::Receiver<CoreEvent>,
    tx: mpsc::Sender<OutboundMessage>,
    session: crate::session_runtime::SessionHandle,
    owner_session_id: coco_types::SessionId,
) {
    use coco_types::ServerNotification;
    // Clear the active-turn slot on the FIRST terminal `TurnEnded` only, so a
    // stray second terminal event can never wipe a fast next turn's slot.
    let mut turn_slot_cleared = false;
    while let Some(event) = rx.recv().await {
        match event {
            CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
                session.accumulate_session_result(&params);
                // Swallow — aggregated result is emitted by session/archive.
            }
            CoreEvent::Protocol(ServerNotification::SessionStarted(_)) => {
                // Swallow: SessionStarted is owned by the remote client server, not the engine.
            }
            other => {
                // free the per-session turn slot BEFORE forwarding the
                // terminal `TurnEnded`, so the client's next `turn/start` (sent
                // the instant it sees `TurnEnded`) finds the slot free instead of
                // racing the turn task's own clear and hitting
                // `TurnAlreadyRunning`. This is now the sole clear site for the
                // active-turn record; the turn/compact tasks no longer clear it.
                if !turn_slot_cleared
                    && matches!(
                        &other,
                        CoreEvent::Protocol(ServerNotification::TurnEnded(_))
                    )
                {
                    session.clear_active_turn();
                    turn_slot_cleared = true;
                }
                if send_session_event(&tx, owner_session_id.clone(), other)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    if !turn_slot_cleared {
        // Returning from TurnRunner closes this channel and is also a clean
        // completion signal. A custom runner that omits TurnEnded must not
        // permanently occupy the session's active-turn slot.
        session.clear_active_turn();
    }
}
