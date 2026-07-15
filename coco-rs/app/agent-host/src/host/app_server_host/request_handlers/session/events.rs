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
    turn_id: coco_types::TurnId,
    app_server: Option<
        std::sync::Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    >,
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
                if turn_slot_cleared {
                    // A terminal was already forwarded for this turn. A second
                    // terminal is a bug (the executor owns exactly one); drop it
                    // without touching the coordinator slot — a fast next turn
                    // may already own it — so it cannot demote that turn to
                    // Finishing or reach the wire/ring twice.
                    tracing::warn!(
                        session_id = %owner_session_id,
                        "dropping duplicate terminal TurnEnded for turn"
                    );
                    continue;
                }
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
        // The runner's channel closed without any terminal being forwarded. The
        // executor owns a terminal on every real path, so this is a defensive
        // backstop for a custom runner: synthesize one so a waiter
        // (`start_turn_and_wait_for_end`, the TUI completion monitor, an SDK
        // stream consumer) sees a complete cycle instead of hanging, then free
        // the slot. `forward_terminal_event` clears the slot as it forwards.
        tracing::warn!(
            session_id = %owner_session_id,
            %turn_id,
            "turn runner exited without a terminal; synthesizing a failed TurnEnded"
        );
        session.mark_active_turn_finishing();
        let _ = forward_terminal_event(
            &tx,
            &owner_session_id,
            coco_types::TurnEndedParams::failed(
                turn_id.clone(),
                /*usage*/ None,
                coco_types::ErrorPayload {
                    message: "turn runner exited without a terminal".to_string(),
                    code: coco_types::ErrorCode::Unknown,
                },
            ),
            &session,
            &mut turn_slot_cleared,
        )
        .await;
    }
    // The turn has ended. Cancel any server->client requests still pending for
    // it (e.g. an approval abandoned when the turn was interrupted) so their
    // pending entries + retained payloads are reclaimed now rather than leaking
    // until the surface detaches or the session closes.
    if let Some(app_server) = &app_server {
        app_server.cancel_turn_server_requests(&turn_id);
    }
}

async fn forward_terminal_event(
    tx: &mpsc::Sender<OutboundMessage>,
    owner_session_id: &coco_types::SessionId,
    ended: coco_types::TurnEndedParams,
    session: &crate::session_runtime::SessionHandle,
    turn_slot_cleared: &mut bool,
) -> bool {
    // Belt against a duplicate terminal reaching the wire/ring. The active-turn
    // slot is cleared exactly once, on the first terminal; a second terminal is
    // dropped-with-warn rather than forwarded (it would otherwise be delivered
    // to passive/replay/Hub consumers as a contradictory outcome).
    if *turn_slot_cleared {
        tracing::warn!(
            session_id = %owner_session_id,
            "dropping duplicate terminal TurnEnded before forward"
        );
        return true;
    }
    // Free the per-session turn slot BEFORE forwarding the terminal
    // `TurnEnded`, so the client's next `turn/start` (sent the instant it sees
    // `TurnEnded`) finds the slot free instead of racing the turn task's own
    // clear and hitting `TurnAlreadyRunning`. This is now the sole clear site
    // for the active-turn record; the turn/compact tasks no longer clear it.
    session.complete_finishing_active_turn();
    *turn_slot_cleared = true;
    // §10.3: a user-started goal turn just freed the slot. Nudge the goal
    // continuation driver so it advances any queued autonomous turn (or
    // registers a wake) now that the slot is free — no mid-turn race. Cheap
    // no-op when no goal is live. The early-return guard above guarantees this
    // runs exactly once, on the first terminal.
    if session.goal_runtime().has_live_goal_sync() {
        session.goal_driver_edge().notify_one();
    }
    send_session_event(
        tx,
        owner_session_id.clone(),
        CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(ended)),
    )
    .await
    .is_ok()
}
