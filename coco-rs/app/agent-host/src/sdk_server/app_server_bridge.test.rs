use std::sync::Arc;

use coco_app_server::{
    JsonRpcConnectionHandlerFactory, JsonRpcRequestContext, JsonRpcRequestHandler,
};
use coco_types::{ClientRequest, InitializeParams, RequestScope};
use tokio::sync::mpsc;

use super::*;

#[tokio::test]
async fn connection_factory_owns_independent_initialize_state() {
    let state = Arc::new(SdkServerState::default());
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let factory = AppServerSdkHandler::new(state, notif_tx);
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

#[test]
fn local_app_session_handle_keeps_immutable_registry_identity() {
    let session_id = coco_types::SessionId::try_new("session-a").unwrap();
    let handle = LocalAppSessionHandle::snapshot(session_id.clone());
    assert_eq!(handle.session_id(), &session_id);
    assert!(!handle.has_runtime());
}
