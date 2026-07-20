use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use coco_types::{
    ServerRequestDelivery, ServerRequestUserInputParams, SessionAccess, SessionTarget,
    UserInputResolveParams,
};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

fn session(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
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

async fn load(server: &Arc<AppServer<TestHandle>>, session_id: SessionId, handle: TestHandle) {
    let AppLoadStart::Started { mut completion } = server
        .spawn_load(session_id, async move { Ok(handle) })
        .expect("start load")
    else {
        panic!("new session must start loading");
    };
    completion.wait().await.expect("load session");
}

struct ConnectionReceivers {
    requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
}

fn connect(server: &AppServer<TestHandle>, connection: ConnectionKey) -> ConnectionReceivers {
    let (events, _event_rx) = tokio::sync::mpsc::channel(8);
    let (requests, request_rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle, _lifecycle_rx) = tokio::sync::mpsc::channel(8);
    server.connect_with_request_and_lifecycle_senders(connection, events, requests, lifecycle);
    ConnectionReceivers {
        requests: request_rx,
    }
}

#[tokio::test]
async fn spawn_load_has_one_owner_even_when_the_original_waiter_is_dropped() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let session_id = session("session-load-owner");
    let factory_runs = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let runs = Arc::clone(&factory_runs);
    let AppLoadStart::Started { completion } = server
        .spawn_load(session_id.clone(), async move {
            runs.fetch_add(1, Ordering::SeqCst);
            release_rx.await.expect("release");
            Ok(TestHandle("loaded"))
        })
        .expect("start load")
    else {
        panic!("expected owner");
    };
    drop(completion);

    let duplicate_runs = Arc::clone(&factory_runs);
    let AppLoadStart::Loading(mut waiter) = server
        .spawn_load(session_id, async move {
            duplicate_runs.fetch_add(10, Ordering::SeqCst);
            Ok(TestHandle("duplicate"))
        })
        .expect("observe load")
    else {
        panic!("expected loading observer");
    };
    release_tx.send(()).expect("release load");

    assert_eq!(waiter.wait().await.expect("load"), TestHandle("loaded"));
    assert_eq!(factory_runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn live_session_supports_multiple_full_connections_and_read_only_observers() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let session_id = session("session-shared");
    load(&server, session_id.clone(), TestHandle("shared")).await;
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let reader = ConnectionKey::generate();
    let _first_rx = connect(&server, first);
    let _second_rx = connect(&server, second);
    let _reader_rx = connect(&server, reader);

    server
        .attach_live_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("first full");
    server
        .attach_live_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("second full");
    server
        .subscribe_live_session(
            reader,
            session_id.clone(),
            Some(0),
            AttachSessionOptions::read_only(),
        )
        .expect("reader");

    assert_eq!(
        server.list_live_sessions()[0].connection_counts,
        SessionConnectionCounts {
            full: 2,
            read_only: 1,
        }
    );
    assert!(
        server
            .validate_session_target(
                first,
                &SessionTarget {
                    session_id: session_id.clone(),
                },
                SessionAccess::Full,
            )
            .is_ok()
    );
    assert!(matches!(
        server.validate_session_target(reader, &SessionTarget { session_id }, SessionAccess::Full,),
        Err(AppServerError::SessionGrantReadOnly { .. })
    ));
}

#[tokio::test]
async fn server_request_reply_is_atomic_first_response_wins() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = session("session-first-wins");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let mut first_rx = connect(&server, first);
    let mut second_rx = connect(&server, second);
    server
        .attach_live_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("first");
    server
        .attach_live_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("second");
    let reply_waiter = server
        .route_server_request_with_reply(session_id.clone(), None, server_request())
        .expect("route");
    let first_delivery = first_rx.requests.recv().await.expect("first delivery");
    let second_delivery = second_rx.requests.recv().await.expect("second delivery");
    assert_eq!(first_delivery.request_id, second_delivery.request_id);
    let request_id = first_delivery.request_id.as_display();
    let target = SessionTarget {
        session_id: session_id.clone(),
    };
    let reply = ServerRequestReply::UserInput(UserInputResolveParams {
        target: target.clone(),
        request_id: request_id.clone(),
        answer: "first".to_string(),
    });

    server
        .resolve_server_request(first, &target, reply)
        .expect("first reply");

    let winner = reply_waiter.await.expect("winner delivered");
    assert!(matches!(
        winner,
        ServerRequestReply::UserInput(UserInputResolveParams { answer, .. }) if answer == "first"
    ));
    assert!(matches!(
        server.resolve_server_request(
            second,
            &target,
            ServerRequestReply::UserInput(UserInputResolveParams {
                target: target.clone(),
                request_id,
                answer: "second".to_string(),
            }),
        ),
        Err(AppServerError::ServerRequestNotFound { .. })
    ));
}

#[tokio::test]
async fn cancelling_one_broadcast_recipient_keeps_the_shared_waiter_alive() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = session("session-cancel-one-recipient");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let first = ConnectionKey::generate();
    let second = ConnectionKey::generate();
    let mut first_rx = connect(&server, first);
    let mut second_rx = connect(&server, second);
    server
        .attach_live_session(first, session_id.clone(), AttachSessionOptions::full())
        .expect("first");
    server
        .attach_live_session(second, session_id.clone(), AttachSessionOptions::full())
        .expect("second");
    let reply_waiter = server
        .route_server_request_with_reply(session_id.clone(), None, server_request())
        .expect("route");
    let first_delivery = first_rx.requests.recv().await.expect("first delivery");
    let second_delivery = second_rx.requests.recv().await.expect("second delivery");
    assert_eq!(first_delivery.request_id, second_delivery.request_id);

    assert_eq!(
        server
            .cancel_server_request_for_connection(first, &first_delivery.request_id)
            .expect("withdraw first"),
        CancelServerRequestOutcome::Withdrawn
    );
    let target = SessionTarget { session_id };
    server
        .resolve_server_request(
            second,
            &target,
            ServerRequestReply::UserInput(UserInputResolveParams {
                target: target.clone(),
                request_id: second_delivery.request_id.as_display(),
                answer: "second".to_string(),
            }),
        )
        .expect("second reply");

    let winner = tokio::time::timeout(std::time::Duration::from_secs(1), reply_waiter)
        .await
        .expect("waiter timed out")
        .expect("waiter closed");
    assert!(matches!(
        winner,
        ServerRequestReply::UserInput(UserInputResolveParams { answer, .. })
            if answer == "second"
    ));
}

#[tokio::test]
async fn detaching_callback_owner_closes_its_targeted_request_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = session("session-detach-targeted-waiter");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let owner = ConnectionKey::generate();
    let mut owner_rx = connect(&server, owner);
    server
        .attach_live_session(owner, session_id.clone(), AttachSessionOptions::full())
        .expect("owner");
    let waiter = server
        .route_server_request_with_reply_to_connection(
            owner,
            session_id.clone(),
            None,
            server_request(),
        )
        .expect("route targeted request");
    owner_rx.requests.recv().await.expect("owner delivery");

    let outcome = server.detach_session_for_connection(owner, &session_id);

    assert!(outcome.detached);
    assert_eq!(outcome.cancelled_requests.len(), 1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter timed out")
            .is_err()
    );
}

#[tokio::test]
async fn immediate_client_reply_cannot_beat_waiter_registration() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = session("session-immediate-reply");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let connection = ConnectionKey::generate();
    let mut receiver = connect(&server, connection);
    server
        .attach_live_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");
    let reply_server = Arc::clone(&server);
    let reply_session = session_id.clone();
    let responder = tokio::spawn(async move {
        let delivery = receiver.requests.recv().await.expect("delivery");
        let request_id = delivery.request_id.as_display();
        reply_server
            .resolve_server_request_for_connection(
                connection,
                &reply_session,
                &delivery.request_id,
                ServerRequestReply::UserInput(UserInputResolveParams {
                    target: SessionTarget {
                        session_id: reply_session.clone(),
                    },
                    request_id,
                    answer: "immediate".to_string(),
                }),
            )
            .expect("resolve immediate reply");
    });

    let waiter = server
        .route_server_request_with_reply(session_id, None, server_request())
        .expect("route");
    let reply = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("waiter timed out")
        .expect("waiter closed");
    assert!(matches!(
        reply,
        ServerRequestReply::UserInput(UserInputResolveParams { answer, .. })
            if answer == "immediate"
    ));
    responder.await.expect("responder");
}

#[tokio::test]
async fn unanswered_server_request_expires_and_closes_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::with_server_request_timeout(
        1,
        8,
        std::time::Duration::from_millis(10),
    ));
    let session_id = session("session-request-timeout");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let connection = ConnectionKey::generate();
    let _receiver = connect(&server, connection);
    server
        .attach_live_session(connection, session_id.clone(), AttachSessionOptions::full())
        .expect("attach");
    let waiter = server
        .route_server_request_with_reply(session_id.clone(), None, server_request())
        .expect("route");

    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("timeout task stalled")
            .is_err()
    );
    assert!(
        server
            .pending_server_request_replays_for_session(&session_id)
            .is_empty()
    );
}

#[tokio::test]
async fn read_only_connection_cannot_win_a_server_request_race() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = session("session-read-only-reply");
    load(&server, session_id.clone(), TestHandle("session")).await;
    let writer = ConnectionKey::generate();
    let reader = ConnectionKey::generate();
    let mut writer_rx = connect(&server, writer);
    let _reader_rx = connect(&server, reader);
    server
        .attach_live_session(writer, session_id.clone(), AttachSessionOptions::full())
        .expect("writer");
    server
        .attach_live_session(
            reader,
            session_id.clone(),
            AttachSessionOptions::read_only(),
        )
        .expect("reader");
    let waiter = server
        .route_server_request_with_reply(session_id.clone(), None, server_request())
        .expect("route");
    let delivery = writer_rx.requests.recv().await.expect("writer delivery");
    let request_id = delivery.request_id.as_display();
    let target = SessionTarget { session_id };

    assert!(matches!(
        server.resolve_server_request(
            reader,
            &target,
            ServerRequestReply::UserInput(UserInputResolveParams {
                target: target.clone(),
                request_id: request_id.clone(),
                answer: "reader".to_string(),
            }),
        ),
        Err(AppServerError::SessionGrantReadOnly { .. })
    ));
    server
        .resolve_server_request(
            writer,
            &target,
            ServerRequestReply::UserInput(UserInputResolveParams {
                target: target.clone(),
                request_id,
                answer: "writer".to_string(),
            }),
        )
        .expect("writer reply");
    assert!(waiter.await.is_ok());
}

#[tokio::test]
async fn attach_live_session_rejects_an_unknown_session() {
    let server = AppServer::<TestHandle>::new(1, 8);
    let connection = ConnectionKey::generate();
    let _receivers = connect(&server, connection);

    assert!(matches!(
        server.attach_live_session(
            connection,
            session("session-missing"),
            AttachSessionOptions::full(),
        ),
        Err(AttachError::SessionNotFound { .. })
    ));
}
