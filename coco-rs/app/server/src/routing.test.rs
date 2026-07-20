use coco_types::{
    CoreEvent, ServerNotification, ServerRequest, ServerRequestUserInputParams, SessionState,
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

fn server_request() -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: "payload-id".to_string(),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

struct ConnectionReceivers {
    events: tokio::sync::mpsc::Receiver<SessionDelivery>,
    requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
    _lifecycle: tokio::sync::mpsc::Receiver<SessionLifecycleEffect>,
}

fn connect(
    routing: &mut RoutingState,
    connection: ConnectionKey,
    capacity: usize,
) -> ConnectionReceivers {
    let (event_tx, events) = tokio::sync::mpsc::channel(capacity);
    let (request_tx, requests) = tokio::sync::mpsc::channel(capacity);
    let (lifecycle_tx, lifecycle) = tokio::sync::mpsc::channel(capacity);
    routing.connect_with_request_and_lifecycle_senders(
        connection,
        event_tx,
        request_tx,
        lifecycle_tx,
    );
    ConnectionReceivers {
        events,
        requests,
        _lifecycle: lifecycle,
    }
}

#[test]
fn subscribe_replays_then_receives_live_events_with_read_only_access() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-replay");
    routing.route_envelope(envelope(session_id.clone(), 1));
    routing.route_envelope(envelope(session_id.clone(), 2));
    let connection = ConnectionKey::generate();
    let mut receivers = connect(&mut routing, connection, 8);

    let Ok(SubscribeReplay::Replayed(replayed)) = routing.subscribe(
        connection,
        session_id.clone(),
        Some(1),
        AttachSessionOptions::read_only(),
    ) else {
        panic!("expected replay");
    };

    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].session_seq, Some(2));
    assert_eq!(
        routing
            .grant(connection, &session_id)
            .map(|grant| grant.access),
        Some(SessionAccess::ReadOnly)
    );
    assert!(matches!(
        routing.require_full(connection, &session_id),
        Err(SessionAccessError::ReadOnly { .. })
    ));

    assert_eq!(routing.route_envelope(envelope(session_id, 3)).delivered, 1);
    assert_eq!(
        receivers
            .events
            .try_recv()
            .expect("live event")
            .envelope
            .session_seq,
        Some(3)
    );
}

#[test]
fn snapshot_required_does_not_create_a_ghost_attachment() {
    let mut routing = RoutingState::new(1);
    let session_id = session("session-snapshot");
    routing.route_envelope(envelope(session_id.clone(), 2));
    let connection = ConnectionKey::generate();
    let _receivers = connect(&mut routing, connection, 8);

    assert!(matches!(
        routing.subscribe(
            connection,
            session_id.clone(),
            None,
            AttachSessionOptions::read_only(),
        ),
        Ok(SubscribeReplay::SnapshotRequired)
    ));
    assert!(routing.attachment(connection, &session_id).is_none());
}

#[test]
fn connection_limits_bound_resources_without_single_writer_semantics() {
    let mut routing = RoutingState::new_with_connection_limits(
        8,
        ConnectionLimits {
            max_attached_sessions_per_connection: 1,
            max_connections_per_session: 1,
        },
    );
    let first_connection = ConnectionKey::generate();
    let second_connection = ConnectionKey::generate();
    let _first_receivers = connect(&mut routing, first_connection, 8);
    let _second_receivers = connect(&mut routing, second_connection, 8);
    let first_session = session("session-limit-first");
    let second_session = session("session-limit-second");

    routing
        .attach_session(
            first_connection,
            first_session.clone(),
            AttachSessionOptions::full(),
        )
        .expect("first attachment");
    routing
        .attach_session(
            first_connection,
            first_session.clone(),
            AttachSessionOptions::full(),
        )
        .expect("idempotent attachment does not consume capacity");
    assert!(matches!(
        routing.attach_session(
            first_connection,
            second_session,
            AttachSessionOptions::full(),
        ),
        Err(AttachError::ConnectionAttachmentLimit { .. })
    ));
    assert!(matches!(
        routing.attach_session(
            second_connection,
            first_session.clone(),
            AttachSessionOptions::read_only(),
        ),
        Err(AttachError::SessionConnectionLimit { .. })
    ));

    routing.detach_session_for_connection(first_connection, &first_session);
    routing
        .attach_session(
            second_connection,
            first_session,
            AttachSessionOptions::full(),
        )
        .expect("released capacity can be reused");
}

#[test]
fn two_full_connections_receive_the_same_session_event() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-shared");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let mut first_rx = connect(&mut routing, first, 8);
    let mut second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");

    let outcome = routing.route_envelope(envelope(session_id.clone(), 1));

    assert_eq!(outcome.delivered, 2);
    assert_eq!(
        first_rx
            .events
            .try_recv()
            .expect("first")
            .envelope
            .session_id,
        session_id
    );
    assert_eq!(
        second_rx
            .events
            .try_recv()
            .expect("second")
            .envelope
            .session_seq,
        Some(1)
    );
    assert_eq!(
        routing.connection_counts_for_session(&session("session-shared")),
        SessionConnectionCounts {
            full: 2,
            read_only: 0,
        }
    );
}

#[test]
fn read_only_subscribe_does_not_downgrade_existing_full_grant() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-grant");
    let connection = ConnectionKey::generate();
    let _receivers = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach full");

    let _ = routing.subscribe(
        connection,
        session_id.clone(),
        Some(0),
        AttachSessionOptions::read_only(),
    );

    assert_eq!(
        routing
            .grant(connection, &session_id)
            .map(|grant| grant.access),
        Some(SessionAccess::Full)
    );
}

#[test]
fn server_request_is_broadcast_to_full_connections_only_and_first_reply_wins() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-request");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let reader = ConnectionKey::generate();
    let mut first_rx = connect(&mut routing, first, 8);
    let mut second_rx = connect(&mut routing, second, 8);
    let mut reader_rx = connect(&mut routing, reader, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");
    routing
        .attach_session(
            reader,
            session_id.clone(),
            AttachSessionOptions::read_only(),
        )
        .expect("attach reader");

    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");

    assert_eq!(routed.delivered, 2);
    assert_eq!(
        first_rx
            .requests
            .try_recv()
            .expect("first request")
            .request_id,
        routed.pending.request_id
    );
    assert!(second_rx.requests.try_recv().is_ok());
    assert!(reader_rx.requests.try_recv().is_err());
    assert!(
        routing
            .complete_server_request(
                first,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
    assert!(matches!(
        routing.complete_server_request(
            second,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
    let cancellation = second_rx.requests.try_recv().expect("loser cancellation");
    assert!(matches!(
        cancellation.request,
        ServerRequest::CancelRequest(ref params)
            if params.request_id == routed.pending.request_id.as_display()
    ));
}

#[test]
fn one_full_connection_can_withdraw_without_cancelling_its_peers_request() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-request-withdraw");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let mut first_rx = connect(&mut routing, first, 8);
    let mut second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");
    assert!(first_rx.requests.try_recv().is_ok());
    assert!(second_rx.requests.try_recv().is_ok());

    assert_eq!(
        routing
            .cancel_server_request_for_connection(&routed.pending.request_id, first)
            .expect("withdraw first"),
        CancelServerRequestOutcome::Withdrawn
    );
    assert!(matches!(
        routing.complete_server_request(
            first,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotRecipient { .. })
    ));
    assert!(
        routing
            .complete_server_request(
                second,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn connection_owned_request_is_not_broadcast_or_resolvable_by_a_peer() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-owned-request");
    let owner = ConnectionKey::generate();
    let peer = ConnectionKey::generate();
    let mut owner_rx = connect(&mut routing, owner, 8);
    let mut peer_rx = connect(&mut routing, peer, 8);
    routing
        .attach_session(owner, session_id.clone(), AttachSessionOptions::full())
        .expect("attach owner");
    routing
        .attach_session(peer, session_id.clone(), AttachSessionOptions::full())
        .expect("attach peer");

    let routed = routing
        .route_server_request_to(
            ServerRequestAudience::Connection(owner),
            session_id.clone(),
            None,
            server_request(),
        )
        .expect("route owned request");

    assert_eq!(routed.delivered, 1);
    assert!(owner_rx.requests.try_recv().is_ok());
    assert!(peer_rx.requests.try_recv().is_err());
    assert!(matches!(
        routing.complete_server_request(
            peer,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotRecipient { .. })
    ));
    assert!(matches!(
        routing.cancel_server_request_for_connection(&routed.pending.request_id, peer),
        Err(CompleteServerRequestError::NotRecipient { .. })
    ));
    assert!(
        routing
            .complete_server_request(
                owner,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn detaching_a_session_removes_targeted_requests_and_callback_ownership() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-detach-cleanup");
    let owner = ConnectionKey::generate();
    let _owner_rx = connect(&mut routing, owner, 8);
    routing
        .attach_session(owner, session_id.clone(), AttachSessionOptions::full())
        .expect("attach owner");
    let callback = ConnectionCallback::Hook("callback-1".to_string());
    routing
        .register_connection_callback(owner, session_id.clone(), callback.clone())
        .expect("register callback owner");
    let pending = routing
        .route_server_request_to(
            ServerRequestAudience::Connection(owner),
            session_id.clone(),
            None,
            server_request(),
        )
        .expect("route owned request")
        .pending;

    let outcome = routing.detach_session_for_connection(owner, &session_id);

    assert!(outcome.detached);
    assert_eq!(outcome.cancelled_requests, vec![pending.request_id]);
    assert!(
        routing
            .connection_callback_owner(&session_id, &callback)
            .is_none()
    );
    assert!(
        routing
            .pending_server_request_replays_for_session(&session_id)
            .is_empty()
    );
}

#[test]
fn callback_registration_requires_a_live_full_attachment() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-callback-access");
    let connection = ConnectionKey::generate();
    let _receivers = connect(&mut routing, connection, 8);
    let callback = ConnectionCallback::Hook("callback-1".to_string());

    routing
        .attach_session(
            connection,
            session_id.clone(),
            AttachSessionOptions::read_only(),
        )
        .expect("attach read-only");
    assert!(matches!(
        routing.register_connection_callback(connection, session_id.clone(), callback.clone(),),
        Err(SessionAccessError::ReadOnly { .. })
    ));

    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("upgrade to full");
    routing.close_session_attachments(&session_id);
    assert_eq!(
        routing.session_access(connection, &session_id),
        Some(SessionAccess::Full),
        "close intentionally preserves the grant"
    );
    assert!(matches!(
        routing.register_connection_callback(connection, session_id, callback),
        Err(SessionAccessError::NotAttached { .. })
    ));
}

#[test]
fn wrong_reply_kind_does_not_consume_pending_request() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-reply-kind");
    let connection = ConnectionKey::generate();
    let _rx = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach full");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");

    assert!(matches!(
        routing.complete_server_request(
            connection,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::Approval),
        ),
        Err(CompleteServerRequestError::WrongReplyKind { .. })
    ));
    assert!(
        routing
            .complete_server_request(
                connection,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn idempotent_full_attach_does_not_replay_pending_request_twice() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-idempotent-attach");
    let connection = ConnectionKey::generate();
    let mut rx = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach full");
    routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");
    assert!(rx.requests.try_recv().is_ok());

    routing
        .attach_session(connection, session_id, AttachSessionOptions::full())
        .expect("reattach full");

    assert!(rx.requests.try_recv().is_err());
}

#[test]
fn disconnecting_one_full_connection_does_not_cancel_shared_pending_request() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-request-disconnect");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let _first_rx = connect(&mut routing, first, 8);
    let _second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");

    routing.disconnect(first);

    assert!(
        routing
            .complete_server_request(
                second,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn reconnecting_with_full_access_replays_pending_server_requests() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-request-reconnect");
    let original = ConnectionKey::generate();
    let mut original_rx = connect(&mut routing, original, 8);
    routing
        .attach_session(original, session_id.clone(), AttachSessionOptions::full())
        .expect("attach original");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");
    assert_eq!(
        original_rx
            .requests
            .try_recv()
            .expect("original delivery")
            .request_id,
        routed.pending.request_id
    );
    routing.disconnect(original);

    let reconnected = ConnectionKey::generate();
    let mut reconnected_rx = connect(&mut routing, reconnected, 8);
    routing
        .attach_session(
            reconnected,
            session_id.clone(),
            AttachSessionOptions::full(),
        )
        .expect("attach replacement responder");

    assert_eq!(
        reconnected_rx
            .requests
            .try_recv()
            .expect("pending request replay")
            .request_id,
        routed.pending.request_id
    );
    assert!(
        routing
            .complete_server_request(
                reconnected,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn a_slow_connection_is_disconnected_without_affecting_its_peer() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-slow");
    let slow = ConnectionKey::generate();
    let healthy = ConnectionKey::generate();
    let _slow_rx = connect(&mut routing, slow, 1);
    let mut healthy_rx = connect(&mut routing, healthy, 8);
    routing
        .attach_session(slow, session_id.clone(), AttachSessionOptions::full())
        .expect("attach slow");
    routing
        .attach_session(healthy, session_id.clone(), AttachSessionOptions::full())
        .expect("attach healthy");

    assert_eq!(
        routing
            .route_envelope(envelope(session_id.clone(), 1))
            .delivered,
        2
    );
    let second = routing.route_envelope(envelope(session_id.clone(), 2));

    assert_eq!(second.delivered, 1);
    assert_eq!(second.disconnected, vec![slow]);
    assert!(routing.attachment(slow, &session_id).is_none());
    assert!(routing.require_full(healthy, &session_id).is_ok());
    assert!(healthy_rx.events.try_recv().is_ok());
    assert!(healthy_rx.events.try_recv().is_ok());
}

#[test]
fn replacing_a_session_repoints_only_the_calling_connection() {
    let mut routing = RoutingState::new(8);
    let old = session("session-old");
    let new = session("session-new");
    let caller = ConnectionKey::generate();
    let peer = ConnectionKey::generate();
    let _caller_rx = connect(&mut routing, caller, 8);
    let _peer_rx = connect(&mut routing, peer, 8);
    routing
        .attach_session(caller, old.clone(), AttachSessionOptions::full())
        .expect("attach caller");
    routing
        .attach_session(peer, old.clone(), AttachSessionOptions::full())
        .expect("attach peer");

    let outcome = routing
        .replace_calling_attachment(caller, &old, new.clone())
        .expect("replace");

    assert_eq!(outcome.calling_connection, caller);
    assert!(outcome.detached_connections.contains(&caller));
    assert!(outcome.detached_connections.contains(&peer));
    assert!(routing.require_full(caller, &new).is_ok());
    assert!(routing.attachment(peer, &old).is_none());
}

#[test]
fn failed_replace_replay_keeps_old_session_peers_attached() {
    let mut routing = RoutingState::new(8);
    let old = session("session-replace-replay-old");
    let new = session("session-replace-replay-new");
    let caller = ConnectionKey::generate();
    let peer = ConnectionKey::generate();
    let destination_owner = ConnectionKey::generate();
    let _caller_receivers = connect(&mut routing, caller, 1);
    let _peer_receivers = connect(&mut routing, peer, 2);
    let _destination_receivers = connect(&mut routing, destination_owner, 2);
    routing
        .attach_session(caller, old.clone(), AttachSessionOptions::full())
        .expect("attach caller");
    routing
        .attach_session(peer, old.clone(), AttachSessionOptions::full())
        .expect("attach peer");
    routing
        .attach_session(destination_owner, new.clone(), AttachSessionOptions::full())
        .expect("attach destination owner");
    routing
        .route_server_request(old.clone(), None, server_request())
        .expect("fill caller request queue");
    routing
        .route_server_request(new.clone(), None, server_request())
        .expect("create destination pending request");

    assert!(matches!(
        routing.replace_calling_attachment(caller, &old, new.clone()),
        Err(ReplaceAttachmentError::Attach(
            AttachError::ReplayQueueUnavailable { .. }
        ))
    ));
    assert!(routing.attachment(peer, &old).is_some());
    assert!(routing.attachment(destination_owner, &new).is_some());
    assert!(routing.attachment(caller, &old).is_none());
    assert!(routing.attachment(caller, &new).is_none());
}

#[test]
fn closing_a_session_cancels_pending_requests_and_detaches_every_connection() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-close");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let _first_rx = connect(&mut routing, first, 8);
    let _second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(
            second,
            session_id.clone(),
            AttachSessionOptions::read_only(),
        )
        .expect("attach second");
    routing.route_envelope(envelope(session_id.clone(), 1));
    assert!(routing.rings.contains_key(&session_id));
    let pending = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("request")
        .pending;

    let outcome = routing.close_session_attachments(&session_id);

    assert_eq!(outcome.detached_connections.len(), 2);
    assert_eq!(outcome.cancelled_requests, vec![pending.request_id]);
    assert_eq!(
        routing.connection_counts_for_session(&session_id),
        SessionConnectionCounts::default()
    );
    assert_eq!(
        routing.session_access(first, &session_id),
        Some(SessionAccess::Full),
        "close preserves the full grant for durable operations"
    );
    assert_eq!(
        routing.session_access(second, &session_id),
        Some(SessionAccess::ReadOnly),
        "close preserves the read-only sharing grant"
    );
    assert!(routing.attachment(first, &session_id).is_none());
    assert!(routing.attachment(second, &session_id).is_none());
    assert!(!routing.rings.contains_key(&session_id));

    routing.disconnect(first);
    routing.disconnect(second);
    assert!(routing.grant(first, &session_id).is_none());
    assert!(routing.grant(second, &session_id).is_none());
}

#[test]
fn error_reply_on_broadcast_withdraws_only_the_sender() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-error-withdraw");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let mut first_rx = connect(&mut routing, first, 8);
    let mut second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");
    assert!(first_rx.requests.try_recv().is_ok());
    assert!(second_rx.requests.try_recv().is_ok());

    assert_eq!(
        routing
            .resolve_error_reply(first, &session_id, &routed.pending.request_id)
            .expect("error reply withdraws"),
        ErrorReplyDisposition::Withdrawn
    );
    // The peer keeps the pending request and can still answer it.
    assert!(
        routing
            .complete_server_request(
                second,
                &session_id,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}

#[test]
fn error_reply_from_last_broadcast_recipient_cancels_the_request() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-error-last");
    let only = ConnectionKey::generate();
    let mut only_rx = connect(&mut routing, only, 8);
    routing
        .attach_session(only, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");
    let routed = routing
        .route_server_request(session_id.clone(), None, server_request())
        .expect("route request");
    assert!(only_rx.requests.try_recv().is_ok());

    let disposition = routing
        .resolve_error_reply(only, &session_id, &routed.pending.request_id)
        .expect("error reply cancels");
    assert!(matches!(
        disposition,
        ErrorReplyDisposition::CancelledLast(pending)
            if pending.request_id == routed.pending.request_id
    ));
    assert!(matches!(
        routing.complete_server_request(
            only,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn error_reply_completes_a_connection_targeted_request() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-error-targeted");
    let owner = ConnectionKey::generate();
    let peer = ConnectionKey::generate();
    let mut owner_rx = connect(&mut routing, owner, 8);
    let _peer_rx = connect(&mut routing, peer, 8);
    routing
        .attach_session(owner, session_id.clone(), AttachSessionOptions::full())
        .expect("attach owner");
    routing
        .attach_session(peer, session_id.clone(), AttachSessionOptions::full())
        .expect("attach peer");
    let routed = routing
        .route_server_request_to(
            ServerRequestAudience::Connection(owner),
            session_id.clone(),
            None,
            server_request(),
        )
        .expect("route targeted request");
    assert!(owner_rx.requests.try_recv().is_ok());

    let disposition = routing
        .resolve_error_reply(owner, &session_id, &routed.pending.request_id)
        .expect("error reply completes targeted");
    assert!(matches!(
        disposition,
        ErrorReplyDisposition::CompletedTargeted(pending)
            if pending.request_id == routed.pending.request_id
    ));
    assert!(matches!(
        routing.complete_server_request(
            owner,
            &session_id,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn cancellation_notifications_are_not_routable_as_requests() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-cancel-not-routable");
    let connection = ConnectionKey::generate();
    let _rx = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");

    let result = routing.route_server_request(
        session_id.clone(),
        None,
        ServerRequest::CancelRequest(ServerCancelRequestParams {
            request_id: "bogus".to_string(),
            reason: None,
        }),
    );
    assert!(matches!(
        result,
        Err(ServerRequestRouteError::CancellationNotRoutable { session_id: ref rejected })
            if *rejected == session_id
    ));
}

#[test]
fn internal_disconnect_records_orphaned_waiter_cancellations() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-orphaned-waiters");
    let connection = ConnectionKey::generate();
    // Event queue of 1: the second routed envelope overflows and disconnects
    // the connection from inside `route_envelope`.
    let _rx = connect(&mut routing, connection, 1);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");
    let routed = routing
        .route_server_request_to(
            ServerRequestAudience::Connection(connection),
            session_id.clone(),
            None,
            server_request(),
        )
        .expect("route targeted request");

    routing.route_envelope(envelope(session_id.clone(), 1));
    let outcome = routing.route_envelope(envelope(session_id, 2));
    assert_eq!(outcome.disconnected, vec![connection]);

    let orphaned = routing.take_orphaned_waiter_cancellations();
    assert!(
        orphaned.contains(&routed.pending.request_id),
        "the internal disconnect must surface the cancelled request id for waiter cleanup"
    );
    assert!(routing.take_orphaned_waiter_cancellations().is_empty());
}

#[test]
fn callback_owner_falls_back_to_prior_registrant_on_disconnect() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-owner-stack");
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let _first_rx = connect(&mut routing, first, 8);
    let _second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    routing
        .attach_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("attach second");
    let callback = ConnectionCallback::Hook("hook-1".to_string());
    routing
        .register_connection_callback(first, session_id.clone(), callback.clone())
        .expect("register first");
    routing
        .register_connection_callback(second, session_id.clone(), callback.clone())
        .expect("register second");
    assert_eq!(
        routing.connection_callback_owner(&session_id, &callback),
        Some(second),
        "most recent registrant owns the callback"
    );

    routing.disconnect(second);
    assert_eq!(
        routing.connection_callback_owner(&session_id, &callback),
        Some(first),
        "ownership falls back to the prior still-attached registrant"
    );

    routing.disconnect(first);
    assert_eq!(
        routing.connection_callback_owner(&session_id, &callback),
        None
    );
}

#[test]
fn attach_replays_pending_broadcasts_in_mint_order() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-replay-order");
    let first = ConnectionKey::generate();
    let mut first_rx = connect(&mut routing, first, 8);
    routing
        .attach_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("attach first");
    let mut routed_order = Vec::new();
    for _ in 0..3 {
        routed_order.push(
            routing
                .route_server_request(session_id.clone(), None, server_request())
                .expect("route request")
                .pending
                .request_id,
        );
    }
    while first_rx.requests.try_recv().is_ok() {}

    let second = ConnectionKey::generate();
    let mut second_rx = connect(&mut routing, second, 8);
    routing
        .attach_session(second, session_id, AttachSessionOptions::full())
        .expect("attach second");

    let replayed: Vec<_> = std::iter::from_fn(|| second_rx.requests.try_recv().ok())
        .map(|delivery| delivery.request_id)
        .collect();
    assert_eq!(
        replayed, routed_order,
        "pending broadcasts replay oldest-first to a newly attached Full connection"
    );
}

#[test]
fn turn_transition_cancels_only_that_turns_pending_requests() {
    let mut routing = RoutingState::new(8);
    let session_id = session("session-turn-cancel");
    let connection = ConnectionKey::generate();
    let mut rx = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");
    let first_turn = TurnId::from("turn-1");
    let second_turn = TurnId::from("turn-2");
    let first_routed = routing
        .route_server_request(
            session_id.clone(),
            Some(first_turn.clone()),
            server_request(),
        )
        .expect("route first");
    let second_routed = routing
        .route_server_request(session_id.clone(), Some(second_turn), server_request())
        .expect("route second");
    while rx.requests.try_recv().is_ok() {}

    let cancelled = routing.cancel_turn_server_requests(&first_turn);
    assert_eq!(cancelled, vec![first_routed.pending.request_id.clone()]);
    // The other turn's request survives and remains completable.
    assert!(
        routing
            .complete_server_request(
                connection,
                &session_id,
                &second_routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
    // The cancelled turn's request is gone.
    assert!(matches!(
        routing.complete_server_request(
            connection,
            &session_id,
            &first_routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn reply_for_the_wrong_session_is_rejected_and_the_request_stays_pending() {
    let mut routing = RoutingState::new(8);
    let session_a = session("session-reply-a");
    let session_b = session("session-reply-b");
    let connection = ConnectionKey::generate();
    let mut rx = connect(&mut routing, connection, 8);
    routing
        .attach_session(connection, session_a.clone(), AttachSessionOptions::full())
        .expect("attach a");
    routing
        .attach_session(connection, session_b.clone(), AttachSessionOptions::full())
        .expect("attach b");
    let routed = routing
        .route_server_request(session_a.clone(), None, server_request())
        .expect("route request");
    while rx.requests.try_recv().is_ok() {}

    // A Full grant on another session must not let a reply cross sessions.
    assert!(matches!(
        routing.complete_server_request(
            connection,
            &session_b,
            &routed.pending.request_id,
            Some(ServerRequestReplyKind::UserInput),
        ),
        Err(CompleteServerRequestError::WrongSession { .. })
    ));
    // Same for error replies.
    assert!(matches!(
        routing.resolve_error_reply(connection, &session_b, &routed.pending.request_id),
        Err(CompleteServerRequestError::WrongSession { .. })
    ));
    // The rejected replies left the request pending and resolvable.
    assert!(
        routing
            .complete_server_request(
                connection,
                &session_a,
                &routed.pending.request_id,
                Some(ServerRequestReplyKind::UserInput),
            )
            .is_ok()
    );
}
