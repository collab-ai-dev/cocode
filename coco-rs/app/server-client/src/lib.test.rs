use coco_app_server_transport::{JsonRpcFrame, JsonRpcId, JsonRpcNotification, JsonRpcSuccess};
use coco_types::{
    CoreEvent, ServerNotification, SessionDelivery, SessionEnvelope, SessionId,
    SessionLifecycleEffect, SessionLifecycleEffectKind, SessionState,
};

use super::*;

fn session(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
    SessionEnvelope::durable(
        session_id,
        None,
        None,
        seq,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    )
}

fn client_parts() -> (
    RemoteJsonRpcClient,
    RemoteJsonRpcIncoming,
    mpsc::Receiver<JsonRpcFrame>,
    RemoteEventDemux,
) {
    let (outbound_tx, outbound_rx) = mpsc::channel(16);
    let (client, incoming, events) = RemoteJsonRpcClient::new(outbound_tx);
    (client, incoming, outbound_rx, RemoteEventDemux::new(events))
}

async fn answer_next(
    outbound: &mut mpsc::Receiver<JsonRpcFrame>,
    incoming: &RemoteJsonRpcIncoming,
    result: serde_json::Value,
) -> coco_app_server_transport::JsonRpcRequest {
    let JsonRpcFrame::Request(request) = outbound.recv().await.expect("outbound request") else {
        panic!("expected request");
    };
    incoming
        .handle_frame(JsonRpcFrame::Success(JsonRpcSuccess::new(
            request.id.clone(),
            result,
        )))
        .await
        .expect("answer request");
    request
}

#[tokio::test]
async fn ordinary_request_round_trip_uses_one_pending_map() {
    let (client, incoming, mut outbound, _demux) = client_parts();
    let request = tokio::spawn({
        let client = client.clone();
        async move { client.keep_alive().await }
    });

    let wire = answer_next(&mut outbound, &incoming, serde_json::Value::Null).await;

    assert_eq!(wire.method, "control/keepAlive");
    request.await.expect("join").expect("keep alive");
}

#[tokio::test]
async fn multiple_session_handles_share_one_remote_connection() {
    let (client, incoming, mut outbound, _demux) = client_parts();
    let first = client.session_handle(session("session-first"));
    let second = client.session_handle(session("session-second"));
    let first_request = tokio::spawn(async move { first.interrupt().await });
    let first_wire = answer_next(&mut outbound, &incoming, serde_json::Value::Null).await;
    first_request.await.expect("join").expect("first interrupt");
    let second_request = tokio::spawn(async move { second.interrupt().await });
    let second_wire = answer_next(&mut outbound, &incoming, serde_json::Value::Null).await;
    second_request
        .await
        .expect("join")
        .expect("second interrupt");

    assert_eq!(first_wire.method, "turn/interrupt");
    assert_eq!(
        first_wire.params.expect("params")["session_id"],
        "session-first"
    );
    assert_eq!(
        second_wire.params.expect("params")["session_id"],
        "session-second"
    );
    assert_ne!(first_wire.id, second_wire.id);
}

#[tokio::test]
async fn session_start_handle_uses_only_the_returned_session_id() {
    let (client, incoming, mut outbound, mut demux) = client_parts();
    let start = tokio::spawn(async move {
        client
            .session_start_handle(&mut demux, coco_types::SessionStartParams::default())
            .await
    });
    let request = answer_next(
        &mut outbound,
        &incoming,
        serde_json::json!({ "session_id": "session-started" }),
    )
    .await;

    let handle = start.await.expect("join").expect("start");
    assert_eq!(request.method, "session/start");
    assert_eq!(handle.session_id(), &session("session-started"));
}

#[tokio::test]
async fn incoming_session_events_demux_by_session_id() {
    let (_client, incoming, _outbound, mut demux) = client_parts();
    let first = session("session-event-first");
    let second = session("session-event-second");
    for (session_id, seq) in [(second.clone(), 2), (first.clone(), 1)] {
        incoming
            .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
                coco_types::SESSION_EVENT_METHOD,
                Some(serde_json::json!({
                    "envelope": {
                        "session_id": session_id,
                        "agent_id": null,
                        "turn_id": null,
                        "session_seq": seq,
                        "event": {
                            "layer": "protocol",
                            "payload": {
                                "method": "session/stateChanged",
                                "params": { "state": "running" }
                            }
                        }
                    }
                })),
            )))
            .await
            .expect("deliver");
    }

    assert_eq!(
        demux
            .next_session_event(&first)
            .await
            .expect("first")
            .session_seq,
        Some(1)
    );
    assert_eq!(
        demux
            .next_session_event(&second)
            .await
            .expect("second")
            .session_seq,
        Some(2)
    );
}

#[tokio::test]
async fn lifecycle_activation_is_keyed_by_the_new_session() {
    let (_client, incoming, _outbound, mut demux) = client_parts();
    let old = session("session-old");
    let new = session("session-new");
    incoming
        .handle_frame(JsonRpcFrame::Notification(JsonRpcNotification::new(
            coco_types::SESSION_LIFECYCLE_METHOD,
            Some(serde_json::json!({
                "effect": {
                    "type": "session_replaced",
                    "old_session_id": old,
                    "new_session_id": new,
                }
            })),
        )))
        .await
        .expect("deliver");

    let effect = demux
        .next_session_activation(&session("session-new"))
        .await
        .expect("activation");
    assert!(matches!(
        effect.kind,
        SessionLifecycleEffectKind::SessionReplaced { new_session_id, .. }
            if new_session_id == session("session-new")
    ));
}

#[tokio::test]
async fn subscribe_returns_a_read_only_handle_with_decoded_replay() {
    let (client, incoming, mut outbound, _demux) = client_parts();
    let subscribe = tokio::spawn(async move {
        client
            .subscribe_session(session("session-read-only"), Some(4))
            .await
    });
    let request = answer_next(
        &mut outbound,
        &incoming,
        serde_json::json!({
            "session_id": "session-read-only",
            "replayed": [{
                "session_id": "session-read-only",
                "session_seq": 5,
                "event": {
                    "layer": "protocol",
                    "payload": {
                        "method": "session/stateChanged",
                        "params": { "state": "running" }
                    }
                }
            }]
        }),
    )
    .await;

    let read_only = subscribe.await.expect("join").expect("subscribe");
    assert_eq!(request.method, "session/subscribe");
    assert_eq!(request.params.expect("params")["after_seq"], 4);
    assert_eq!(read_only.session_id(), &session("session-read-only"));
    assert_eq!(read_only.replayed()[0].session_seq, Some(5));
}

#[tokio::test]
async fn server_request_and_session_event_coexist_on_one_incoming_channel() {
    let (_client, incoming, _outbound, mut demux) = client_parts();
    incoming
        .handle_frame(JsonRpcFrame::Request(
            coco_app_server_transport::JsonRpcRequest::new(
                JsonRpcId::String("approval-1".to_string()),
                "approval/askForApproval",
                Some(serde_json::json!({ "tool_name": "Bash" })),
            ),
        ))
        .await
        .expect("request");
    incoming
        .deliver_event(RemoteJsonRpcEvent::SessionDelivery(Box::new(
            SessionDelivery {
                envelope: envelope(session("session-with-request"), 1),
            },
        )))
        .expect("event");

    assert!(demux.next_server_request().await.is_some());
    assert!(
        demux
            .next_session_event(&session("session-with-request"))
            .await
            .is_some()
    );
}

#[tokio::test]
async fn cancellation_notification_purges_buffered_server_request() {
    let (_client, incoming, _outbound, mut demux) = client_parts();
    incoming
        .handle_frame(JsonRpcFrame::Request(
            coco_app_server_transport::JsonRpcRequest::new(
                JsonRpcId::String("server-request-1".to_string()),
                "approval/askForApproval",
                Some(serde_json::json!({ "tool_name": "Bash" })),
            ),
        ))
        .await
        .expect("request");
    incoming
        .handle_frame(JsonRpcFrame::Notification(
            coco_app_server_transport::JsonRpcNotification::new(
                "control/cancelRequest",
                Some(serde_json::json!({ "request_id": "server-request-1" })),
            ),
        ))
        .await
        .expect("cancellation");

    let cancellation = demux.next_notification().await.expect("notification");
    assert_eq!(cancellation.method, "control/cancelRequest");
    assert!(demux.try_next_server_request().is_none());
}

#[tokio::test]
async fn dropping_incoming_invalidates_pending_requests() {
    let (client, incoming, mut outbound, _demux) = client_parts();
    let pending = tokio::spawn(async move { client.keep_alive().await });
    let _ = outbound.recv().await.expect("outbound request");

    drop(incoming);

    assert!(matches!(
        pending.await.expect("join"),
        Err(ClientError::Disconnected)
    ));
}

#[test]
fn direct_demux_events_are_session_keyed() {
    let (tx, rx) = mpsc::channel(4);
    tx.try_send(RemoteJsonRpcEvent::SessionDelivery(Box::new(
        SessionDelivery {
            envelope: envelope(session("session-direct"), 7),
        },
    )))
    .expect("send");
    tx.try_send(RemoteJsonRpcEvent::SessionLifecycle(
        SessionLifecycleEffect {
            kind: SessionLifecycleEffectKind::SessionEnded {
                session_id: session("session-direct"),
            },
        },
    ))
    .expect("send");
    let mut demux = RemoteEventDemux::new(rx);

    assert_eq!(
        demux
            .try_next_session_event(&session("session-direct"))
            .expect("event")
            .session_seq,
        Some(7)
    );
    assert!(
        demux
            .try_next_lifecycle(&session("session-direct"))
            .is_some()
    );
}
