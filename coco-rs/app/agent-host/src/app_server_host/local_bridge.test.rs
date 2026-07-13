use std::sync::Arc;

use coco_app_server::{
    JsonRpcConnectionHandlerFactory, JsonRpcRequestContext, JsonRpcRequestHandler,
};
use coco_types::{ClientRequest, InitializeParams, RequestScope};
use tokio::sync::mpsc;

use super::*;

#[tokio::test]
async fn connection_factory_owns_independent_initialize_state() {
    let state = Arc::new(AppServerHostState::default());
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let factory = AppServerHostHandler::new(state, notif_tx);
    let connection_a = coco_app_server::ConnectionKey::generate();
    let connection_b = coco_app_server::ConnectionKey::generate();
    let handler_a = factory.open(connection_a);
    let handler_b = factory.open(connection_b);

    for (handler, connection, prompt) in [
        (&handler_a, connection_a, "prompt-a"),
        (&handler_b, connection_b, "prompt-b"),
    ] {
        handler
            .handle_json_rpc_request(
                JsonRpcRequestContext {
                    connection,
                    scope: RequestScope::Connection,
                },
                ClientRequest::Initialize(InitializeParams {
                    system_prompt: Some(prompt.to_string()),
                    ..Default::default()
                }),
            )
            .await
            .expect("first initialize succeeds");
    }

    let duplicate = handler_a
        .handle_json_rpc_request(
            JsonRpcRequestContext {
                connection: connection_a,
                scope: RequestScope::Connection,
            },
            ClientRequest::Initialize(InitializeParams::default()),
        )
        .await
        .expect_err("second initialize fails");
    assert_eq!(duplicate.data.unwrap()["kind"], "already_initialized");

    handler_b
        .handle_json_rpc_request(
            JsonRpcRequestContext {
                connection: connection_b,
                scope: RequestScope::Connection,
            },
            ClientRequest::KeepAlive,
        )
        .await
        .expect("connection B remains usable");
}

#[tokio::test]
async fn local_bridge_restore_session_seq_from_watermark_restores_replay_and_allocator() {
    let state = Arc::new(AppServerHostState::default());
    let bridge = AppServerLocalBridge::new(Arc::clone(&state));
    let session_id =
        coco_types::SessionId::try_new("sess-local-seq-resume").expect("valid test session id");
    let watermark = 40;

    bridge.restore_session_seq_from_watermark(session_id.clone(), watermark);

    let adapter = coco_app_server::LocalClientAdapter::with_channel_capacity(
        Arc::clone(bridge.app_server()),
        8,
    );
    let stale_connection = adapter.connect();
    let stale_replay = stale_connection
        .subscribe_surface(
            session_id.clone(),
            Some(0),
            coco_app_server::AttachSurfaceOptions::default(),
        )
        .expect("stale subscribe should resolve");
    assert!(matches!(
        stale_replay,
        coco_app_server::LocalClientSubscribeOutcome::SnapshotRequired
    ));

    route_app_server_session_event(
        bridge.app_server(),
        None,
        state.session_seq_allocator(),
        session_id.clone(),
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::SessionStateChanged {
            state: coco_types::SessionState::Running,
        }),
    );

    let high_water = state
        .session_seq_allocator()
        .high_water(&session_id)
        .expect("durable event should allocate a session_seq");
    assert!(high_water > watermark);
}
