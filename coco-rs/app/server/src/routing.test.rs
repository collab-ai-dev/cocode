use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::ServerRequest;
use coco_types::ServerRequestUserInputParams;
use coco_types::SessionState;
use coco_types::TuiOnlyEvent;

use super::*;

fn test_session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid test session id")
}

fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
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

fn ephemeral_envelope(session_id: SessionId) -> SessionEnvelope {
    SessionEnvelope::ephemeral(
        session_id,
        None,
        None,
        CoreEvent::Tui(TuiOnlyEvent::QuestionAsked {
            request_id: "question-1".to_string(),
            input: serde_json::json!({ "question": "continue?" }),
        }),
    )
}

fn request_id_strings(request_ids: Vec<RequestId>) -> Vec<String> {
    let mut request_ids = request_ids
        .into_iter()
        .map(|request_id| request_id.as_display())
        .collect::<Vec<_>>();
    request_ids.sort();
    request_ids
}

fn test_server_request() -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: "payload-request-id".to_string(),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

#[test]
fn subscribe_replays_ring_then_receives_live_events() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    routing.route_envelope(durable_envelope(session_id.clone(), 1));
    routing.route_envelope(durable_envelope(session_id.clone(), 2));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);

    let replay = routing
        .subscribe(connection, surface_id.clone(), session_id.clone(), Some(1))
        .expect("subscribe");

    let SubscribeReplay::Replayed(events) = replay else {
        panic!("expected replay");
    };
    assert_eq!(
        events
            .iter()
            .map(|event| event.session_seq.expect("seq"))
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(routing.surface_session(&surface_id), Some(&session_id));

    let outcome = routing.route_envelope(durable_envelope(session_id, 3));
    assert_eq!(outcome.delivered, 1);
    let delivered = rx.try_recv().expect("live delivery");
    assert_eq!(delivered.surface_id, surface_id);
    assert_eq!(delivered.envelope.session_seq, Some(3));
}

#[test]
fn subscribe_requires_snapshot_when_cursor_falls_out_of_ring() {
    let mut routing = RoutingState::new(2);
    let session_id = test_session_id("sess-1");
    routing.route_envelope(durable_envelope(session_id.clone(), 1));
    routing.route_envelope(durable_envelope(session_id.clone(), 2));
    routing.route_envelope(durable_envelope(session_id.clone(), 3));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);

    let replay = routing
        .subscribe(connection, surface_id.clone(), session_id.clone(), Some(0))
        .expect("subscribe");

    assert!(matches!(replay, SubscribeReplay::SnapshotRequired));
    assert_eq!(routing.surface_session(&surface_id), None);
    let outcome = routing.route_envelope(durable_envelope(session_id, 4));
    assert_eq!(outcome.delivered, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn missing_cursor_requires_snapshot_and_does_not_attach() {
    let mut routing = RoutingState::new(8);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    let session_id = test_session_id("sess-1");
    routing.connect(connection, tx);

    let replay = routing
        .subscribe(connection, surface_id.clone(), session_id, None)
        .expect("subscribe");

    assert!(matches!(replay, SubscribeReplay::SnapshotRequired));
    assert_eq!(routing.surface_session(&surface_id), None);
}

#[test]
fn ephemeral_events_deliver_live_without_entering_replay_ring() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, surface_id.clone(), session_id.clone())
        .expect("attach");

    let outcome = routing.route_envelope(ephemeral_envelope(session_id.clone()));

    assert_eq!(outcome.delivered, 1);
    let delivered = rx.try_recv().expect("ephemeral delivery");
    assert_eq!(delivered.surface_id, surface_id);
    assert_eq!(delivered.envelope.session_seq, None);

    let replay = routing
        .subscribe(
            connection,
            SurfaceId::from("surface-2"),
            session_id,
            Some(0),
        )
        .expect("subscribe");
    let SubscribeReplay::Replayed(events) = replay else {
        panic!("expected empty replay");
    };
    assert!(events.is_empty());
}

#[test]
fn disconnect_removes_surfaces_from_all_indexes() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_1 = SurfaceId::from("surface-1");
    let surface_2 = SurfaceId::from("surface-2");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, surface_1.clone(), session_id.clone())
        .expect("attach surface 1");
    routing
        .attach_surface(connection, surface_2.clone(), session_id.clone())
        .expect("attach surface 2");

    let outcome = routing.disconnect(connection);

    assert_eq!(outcome.detached_surfaces.len(), 2);
    assert!(outcome.cancelled_requests.is_empty());
    assert_eq!(routing.surface_session(&surface_1), None);
    assert_eq!(routing.surface_session(&surface_2), None);
    assert_eq!(routing.connection_surface_count(connection), 0);
    let outcome = routing.route_envelope(durable_envelope(session_id, 1));
    assert_eq!(outcome.delivered, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn slow_consumer_disconnects_connection() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, surface_id.clone(), session_id.clone())
        .expect("attach");

    let first = routing.route_envelope(durable_envelope(session_id.clone(), 1));
    let second = routing.route_envelope(durable_envelope(session_id.clone(), 2));

    assert_eq!(first.delivered, 1);
    assert_eq!(second.delivered, 0);
    assert_eq!(second.disconnected, vec![connection]);
    assert_eq!(routing.surface_session(&surface_id), None);
    assert_eq!(routing.connection_surface_count(connection), 0);

    let queued = rx.try_recv().expect("first delivery remains queued");
    assert_eq!(queued.envelope.session_seq, Some(1));
    let third = routing.route_envelope(durable_envelope(session_id, 3));
    assert_eq!(third.delivered, 0);
}

#[test]
fn second_interactive_surface_is_rejected_with_owner_metadata() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let owner_surface = SurfaceId::from("surface-owner");
    let second_surface = SurfaceId::from("surface-second");
    routing.connect(connection, tx);
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..AttachSurfaceOptions::default()
    };
    routing
        .attach_surface_with_options(
            connection,
            owner_surface.clone(),
            session_id.clone(),
            options.clone(),
        )
        .expect("attach interactive owner");

    let err = routing
        .attach_surface_with_options(connection, second_surface, session_id.clone(), options)
        .expect_err("second interactive should be rejected");

    match err {
        AttachError::InteractiveOwnerConflict {
            session_id: err_session_id,
            owner_surface: err_owner_surface,
            owner_idle,
            ..
        } => {
            assert_eq!(err_session_id, session_id);
            assert_eq!(err_owner_surface, owner_surface);
            assert!(!owner_idle);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(routing.interactive_owner(&session_id), Some(&owner_surface));
}

#[test]
fn passive_surfaces_can_share_session_with_interactive_owner() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let interactive = SurfaceId::from("surface-interactive");
    let passive = SurfaceId::from("surface-passive");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            interactive.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    routing
        .attach_surface(connection, passive.clone(), session_id.clone())
        .expect("attach passive");

    assert_eq!(routing.interactive_owner(&session_id), Some(&interactive));
    assert_eq!(routing.surface_session(&passive), Some(&session_id));
}

#[test]
fn sole_interactive_session_for_connection_requires_unique_attached_interactive_surface() {
    let mut routing = RoutingState::new(8);
    let first_session_id = test_session_id("sess-1");
    let second_session_id = test_session_id("sess-2");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let passive = SurfaceId::from("surface-passive");
    let first_interactive = SurfaceId::from("surface-interactive-1");
    let second_interactive = SurfaceId::from("surface-interactive-2");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, passive, first_session_id.clone())
        .expect("attach passive");
    assert_eq!(
        routing.sole_interactive_session_for_connection(connection),
        None
    );

    routing
        .attach_surface_with_options(
            connection,
            first_interactive,
            first_session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach first interactive");
    assert_eq!(
        routing.sole_interactive_session_for_connection(connection),
        Some(first_session_id)
    );

    routing
        .attach_surface_with_options(
            connection,
            second_interactive,
            second_session_id,
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach second interactive for a different session");
    assert_eq!(
        routing.sole_interactive_session_for_connection(connection),
        None
    );
}

#[test]
fn connection_surface_limit_is_enforced() {
    let mut routing = RoutingState::new_with_limits(
        8,
        SurfaceLimits {
            max_surfaces_per_connection: 1,
            max_passive_surfaces_per_session: 16,
        },
    );
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, SurfaceId::from("surface-1"), session_id.clone())
        .expect("attach first");

    let err = routing
        .attach_surface(connection, SurfaceId::from("surface-2"), session_id)
        .expect_err("second surface should exceed connection limit");

    assert!(matches!(err, AttachError::SurfaceLimit { .. }));
    assert_eq!(err.status_code(), StatusCode::ResourcesExhausted);
}

#[test]
fn passive_surface_limit_is_enforced_per_session() {
    let mut routing = RoutingState::new_with_limits(
        8,
        SurfaceLimits {
            max_surfaces_per_connection: 8,
            max_passive_surfaces_per_session: 1,
        },
    );
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, SurfaceId::from("surface-1"), session_id.clone())
        .expect("attach first passive");

    let err = routing
        .attach_surface(connection, SurfaceId::from("surface-2"), session_id)
        .expect_err("second passive should exceed session passive limit");

    assert!(matches!(err, AttachError::SurfaceLimit { .. }));
}

#[test]
fn notification_preferences_filter_delivery_per_surface() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id,
            session_id.clone(),
            AttachSurfaceOptions {
                notification_prefs: NotificationPrefs {
                    protocol: true,
                    stream: true,
                    tui: false,
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach");

    let tui_outcome = routing.route_envelope(ephemeral_envelope(session_id.clone()));
    let protocol_outcome = routing.route_envelope(durable_envelope(session_id, 1));

    assert_eq!(tui_outcome.delivered, 0);
    assert_eq!(protocol_outcome.delivered, 1);
    let delivered = rx.try_recv().expect("protocol delivery");
    assert_eq!(delivered.envelope.session_seq, Some(1));
    assert!(rx.try_recv().is_err());
}

#[test]
fn disconnect_clears_interactive_owner() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    routing.disconnect(connection);

    assert_eq!(routing.interactive_owner(&session_id), None);
    assert_eq!(
        routing.surface_attachment(&surface_id).map(|a| a.state),
        None
    );
}

#[test]
fn server_request_targets_interactive_surface_with_declared_capability() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let interactive = SurfaceId::from("surface-interactive");
    let passive = SurfaceId::from("surface-passive");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, passive.clone(), session_id.clone())
        .expect("attach passive");
    routing
        .attach_surface_with_options(
            connection,
            interactive.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    keychain: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let pending = routing
        .open_server_request(session_id.clone(), SurfaceCapability::Keychain, None)
        .expect("open request");

    assert_eq!(pending.session_id, session_id);
    assert_eq!(pending.surface_id, interactive);
    assert_eq!(pending.capability, SurfaceCapability::Keychain);
    assert!(
        routing
            .pending_server_requests_for_surface(&passive)
            .is_empty()
    );
    assert_eq!(
        routing.pending_server_requests_for_surface(&pending.surface_id),
        vec![pending]
    );
}

#[test]
fn route_server_request_delivers_on_request_channel_and_records_pending() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect_with_request_sender(connection, event_tx, request_tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let outcome = routing
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route request");

    assert_eq!(outcome.pending.session_id, session_id);
    assert_eq!(outcome.pending.surface_id, surface_id);
    assert_eq!(
        routing.pending_server_requests_for_surface(&outcome.pending.surface_id),
        vec![outcome.pending.clone()]
    );
    let delivery = request_rx.try_recv().expect("request delivery");
    assert_eq!(delivery.surface_id, outcome.pending.surface_id);
    assert_eq!(delivery.request_id, outcome.pending.request_id);
    assert!(matches!(
        delivery.request,
        ServerRequest::RequestUserInput(_)
    ));
    assert!(event_rx.try_recv().is_err());
}

#[test]
fn route_server_request_replay_returns_retained_payload_for_surface() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect_with_request_sender(connection, event_tx, request_tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let outcome = routing
        .route_server_request(
            session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route request");

    let replays = routing.pending_server_request_replays_for_surface(&surface_id);
    assert_eq!(replays.len(), 1);
    assert_eq!(replays[0].pending, outcome.pending);
    let ServerRequest::RequestUserInput(params) = &replays[0].request else {
        panic!("expected user input replay");
    };
    assert_eq!(params.request_id, "payload-request-id");
    assert_eq!(params.prompt, "continue?");
}

#[test]
fn completed_routed_server_request_removes_replay_payload() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect_with_request_sender(connection, event_tx, request_tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    let outcome = routing
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            None,
            test_server_request(),
        )
        .expect("route request");

    routing
        .complete_server_request(&outcome.pending.request_id, &session_id)
        .expect("complete request");

    assert!(
        routing
            .pending_server_request_replays_for_surface(&surface_id)
            .is_empty()
    );
}

#[test]
fn route_server_request_requires_request_channel_without_opening_pending() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, event_tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    keychain: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let err = routing
        .route_server_request(
            session_id,
            SurfaceCapability::Keychain,
            None,
            test_server_request(),
        )
        .expect_err("missing request sender");

    assert_eq!(
        err,
        ServerRequestRouteError::NoRequestChannel {
            surface_id: surface_id.clone(),
        }
    );
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
}

#[test]
fn route_server_request_disconnects_full_request_channel_and_cancels_pending() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(1);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect_with_request_sender(connection, event_tx, request_tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let first = routing
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            None,
            test_server_request(),
        )
        .expect("first route");
    let second = routing
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            None,
            test_server_request(),
        )
        .expect_err("request channel full");

    let ServerRequestRouteError::QueueUnavailable { request_id, .. } = second else {
        panic!("expected queue unavailable");
    };
    assert_ne!(request_id, first.pending.request_id);
    assert_eq!(routing.surface_session(&surface_id), None);
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
    assert!(matches!(
        routing.complete_server_request(&first.pending.request_id, &session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
    let queued = request_rx.try_recv().expect("first request remains queued");
    assert_eq!(queued.request_id, first.pending.request_id);
}

#[test]
fn server_request_rejects_missing_interactive_capability() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let interactive = SurfaceId::from("surface-interactive");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            interactive.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    keychain: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");

    let err = routing
        .open_server_request(session_id.clone(), SurfaceCapability::FilePicker, None)
        .expect_err("file picker was not declared");

    assert_eq!(
        err,
        OpenServerRequestError::CapabilityNotDeclared {
            session_id,
            surface_id: interactive,
            capability: SurfaceCapability::FilePicker,
        }
    );
}

#[test]
fn completing_server_request_validates_session_and_clears_indexes() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let wrong_session_id = test_session_id("sess-wrong");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    let pending = routing
        .open_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
        )
        .expect("open request");

    let err = routing
        .complete_server_request(&pending.request_id, &wrong_session_id)
        .expect_err("wrong session should be rejected");
    assert_eq!(
        err,
        CompleteServerRequestError::WrongSession {
            request_id: pending.request_id.clone(),
            expected_session_id: session_id.clone(),
            actual_session_id: wrong_session_id,
        }
    );

    let completed = routing
        .complete_server_request(&pending.request_id, &session_id)
        .expect("complete request");
    assert_eq!(completed, pending);
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
    assert!(matches!(
        routing.complete_server_request(&completed.request_id, &session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn disconnect_cancels_pending_requests_for_connection_surfaces() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    keychain: true,
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    let keychain = routing
        .open_server_request(session_id.clone(), SurfaceCapability::Keychain, None)
        .expect("open keychain");
    let notifications = routing
        .open_server_request(session_id.clone(), SurfaceCapability::Notifications, None)
        .expect("open notifications");

    let outcome = routing.disconnect(connection);

    assert_eq!(outcome.detached_surfaces, vec![surface_id.clone()]);
    assert_eq!(
        request_id_strings(outcome.cancelled_requests),
        request_id_strings(vec![keychain.request_id.clone(), notifications.request_id])
    );
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
    assert!(matches!(
        routing.complete_server_request(&keychain.request_id, &session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn turn_transition_cancels_only_that_turns_pending_requests() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let turn_1 = TurnId::from("turn-1");
    let turn_2 = TurnId::from("turn-2");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    let first = routing
        .open_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            Some(turn_1.clone()),
        )
        .expect("open first");
    let second = routing
        .open_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            Some(turn_2),
        )
        .expect("open second");

    let cancelled = routing.cancel_turn_server_requests(&turn_1);

    assert_eq!(cancelled, vec![first.request_id.clone()]);
    assert_eq!(
        routing.pending_server_requests_for_surface(&surface_id),
        vec![second.clone()]
    );
    assert!(matches!(
        routing.complete_server_request(&first.request_id, &session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
    assert_eq!(
        routing
            .complete_server_request(&second.request_id, &session_id)
            .expect("second still pending"),
        second
    );
}

#[test]
fn replace_repoints_calling_surface_and_closes_peer_surfaces() {
    let mut routing = RoutingState::new(8);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let caller = SurfaceId::from("surface-caller");
    let peer = SurfaceId::from("surface-peer");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            caller.clone(),
            old_session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                last_delivered_seq: 42,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach caller");
    routing
        .attach_surface(connection, peer.clone(), old_session_id.clone())
        .expect("attach peer");

    let outcome = routing
        .replace_calling_surface(&caller, new_session_id.clone())
        .expect("replace");

    assert_eq!(outcome.old_session_id, old_session_id);
    assert_eq!(outcome.new_session_id, new_session_id);
    assert_eq!(outcome.calling_surface, caller);
    assert_eq!(outcome.detached_surfaces, vec![peer.clone()]);
    assert!(outcome.cancelled_requests.is_empty());
    assert_eq!(routing.surface_session(&caller), Some(&new_session_id));
    assert_eq!(routing.surface_session(&peer), None);
    assert_eq!(
        routing.surface_attachment(&peer).map(|a| a.state),
        Some(SurfaceState::SessionClosed)
    );
    assert_eq!(routing.interactive_owner(&old_session_id), None);
    assert_eq!(routing.interactive_owner(&new_session_id), Some(&caller));
    assert_eq!(
        routing
            .surface_attachment(&caller)
            .map(|a| a.last_delivered_seq),
        Some(0)
    );

    let old_outcome = routing.route_envelope(durable_envelope(old_session_id, 1));
    let new_outcome = routing.route_envelope(durable_envelope(new_session_id, 1));
    assert_eq!(old_outcome.delivered, 0);
    assert_eq!(new_outcome.delivered, 1);
    let delivered = rx.try_recv().expect("new-session delivery");
    assert_eq!(delivered.surface_id, caller);
    assert_eq!(delivered.envelope.session_id, test_session_id("sess-new"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn replace_cancels_old_session_pending_requests() {
    let mut routing = RoutingState::new(8);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let caller = SurfaceId::from("surface-caller");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            caller.clone(),
            old_session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    keychain: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach caller");
    let pending = routing
        .open_server_request(old_session_id.clone(), SurfaceCapability::Keychain, None)
        .expect("open request");

    let outcome = routing
        .replace_calling_surface(&caller, new_session_id.clone())
        .expect("replace");

    assert_eq!(outcome.cancelled_requests, vec![pending.request_id.clone()]);
    assert_eq!(routing.surface_session(&caller), Some(&new_session_id));
    assert!(
        routing
            .pending_server_requests_for_surface(&caller)
            .is_empty()
    );
    assert!(matches!(
        routing.complete_server_request(&pending.request_id, &old_session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn archive_session_closes_surfaces_and_removes_fanout() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let interactive = SurfaceId::from("surface-interactive");
    let passive = SurfaceId::from("surface-passive");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            interactive.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    routing
        .attach_surface(connection, passive.clone(), session_id.clone())
        .expect("attach passive");

    let outcome = routing.archive_session(&session_id);

    assert_eq!(outcome.closed_surfaces.len(), 2);
    assert!(outcome.cancelled_requests.is_empty());
    assert_eq!(routing.surface_session(&interactive), None);
    assert_eq!(routing.surface_session(&passive), None);
    assert_eq!(
        routing.surface_attachment(&interactive).map(|a| a.state),
        Some(SurfaceState::SessionClosed)
    );
    assert_eq!(
        routing.surface_attachment(&passive).map(|a| a.state),
        Some(SurfaceState::SessionClosed)
    );
    assert_eq!(routing.interactive_owner(&session_id), None);
    assert_eq!(routing.attached_connection_surface_count(connection), 0);
    assert_eq!(routing.connection_surface_count(connection), 2);

    let route = routing.route_envelope(durable_envelope(session_id, 1));
    assert_eq!(route.delivered, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn archive_session_cancels_pending_requests() {
    let mut routing = RoutingState::new(8);
    let session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let surface_id = SurfaceId::from("surface-1");
    routing.connect(connection, tx);
    routing
        .attach_surface_with_options(
            connection,
            surface_id.clone(),
            session_id.clone(),
            AttachSurfaceOptions {
                role: SurfaceRole::Interactive,
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach interactive");
    let pending = routing
        .open_server_request(session_id.clone(), SurfaceCapability::Notifications, None)
        .expect("open request");

    let outcome = routing.archive_session(&session_id);

    assert_eq!(outcome.cancelled_requests, vec![pending.request_id.clone()]);
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
    assert!(matches!(
        routing.complete_server_request(&pending.request_id, &session_id),
        Err(CompleteServerRequestError::NotFound { .. })
    ));
}

#[test]
fn closed_surfaces_do_not_count_against_connection_limit() {
    let mut routing = RoutingState::new_with_limits(
        8,
        SurfaceLimits {
            max_surfaces_per_connection: 1,
            max_passive_surfaces_per_session: 16,
        },
    );
    let first_session_id = test_session_id("sess-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let first = SurfaceId::from("surface-1");
    let second = SurfaceId::from("surface-2");
    routing.connect(connection, tx);
    routing
        .attach_surface(connection, first.clone(), first_session_id.clone())
        .expect("attach first");
    routing.archive_session(&first_session_id);

    routing
        .attach_surface(connection, second.clone(), test_session_id("sess-2"))
        .expect("closed first surface should not consume live limit");

    assert_eq!(routing.connection_surface_count(connection), 2);
    assert_eq!(routing.attached_connection_surface_count(connection), 1);
    assert_eq!(
        routing.surface_attachment(&first).map(|a| a.state),
        Some(SurfaceState::SessionClosed)
    );
    assert_eq!(
        routing.surface_session(&second),
        Some(&test_session_id("sess-2"))
    );
}
