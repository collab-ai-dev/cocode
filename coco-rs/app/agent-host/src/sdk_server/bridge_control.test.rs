use std::sync::Arc;

use coco_bridge::ControlRequest;
use coco_bridge::ControlRequestHandler;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::SdkBridgeControlHandler;
use crate::sdk_server::handlers::ActiveTurnHandles;
use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::SessionMetadata;
use crate::sdk_server::outbound::OutboundMessage;

async fn state_with_session() -> Arc<SdkServerState> {
    let state = Arc::new(SdkServerState::default());
    state
        .install_test_session_state(
            coco_types::SessionId::try_new("sess-1").unwrap(),
            SessionMetadata {
                cwd: "/tmp".into(),
                model: "mock-model".into(),
            },
        )
        .await;
    state
}

fn test_session_id() -> coco_types::SessionId {
    coco_types::SessionId::try_new("sess-1").unwrap()
}

#[tokio::test]
async fn bridge_handler_rejects_bypass_without_capability() {
    // Startup capability defaults to false — the bridge handler
    // must refuse to escalate into BypassPermissions.
    let state = state_with_session().await;
    assert!(!state.bypass_permissions_available());

    let handler = SdkBridgeControlHandler::new(state.clone());
    let err = handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, coco_types::error_codes::PERMISSION_DENIED);
    assert!(err.message.contains("bypassPermissions"));

    // Live app-state mode was not mutated.
    let session_id = test_session_id();
    let handoff = state.session_handoff_snapshot(&session_id).unwrap();
    let app_state = handoff.app_state.read().await;
    assert!(
        !matches!(
            app_state.permissions.mode,
            Some(coco_types::PermissionMode::BypassPermissions)
        ),
        "rejected bridge request must not write app_state.permission_mode",
    );
}

#[tokio::test]
async fn bridge_handler_accepts_bypass_when_capability_on() {
    // Flipping the capability flag at startup allows bridge-origin
    // escalation. Verifies the handler reads the live AtomicBool
    // (not a cached value).
    let state = state_with_session().await;
    state.set_bypass_permissions_available(true);

    let handler = SdkBridgeControlHandler::new(state.clone());
    let ok = handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap();
    assert_eq!(ok, serde_json::Value::Null);

    let session_id = test_session_id();
    let handoff = state.session_handoff_snapshot(&session_id).unwrap();
    // app_state propagation — engine's live source of truth.
    let app_state = handoff.app_state.read().await;
    assert_eq!(
        app_state.permissions.mode,
        Some(coco_types::PermissionMode::BypassPermissions),
    );
}

#[tokio::test]
async fn bridge_handler_allows_non_bypass_modes_unconditionally() {
    // Non-bypass transitions never touch the killswitch gate.
    let state = state_with_session().await;
    assert!(!state.bypass_permissions_available());

    let handler = SdkBridgeControlHandler::new(state.clone());
    handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        })
        .await
        .unwrap();

    let session_id = test_session_id();
    let handoff = state.session_handoff_snapshot(&session_id).unwrap();
    let app_state = handoff.app_state.read().await;
    assert_eq!(
        app_state.permissions.mode,
        Some(coco_types::PermissionMode::AcceptEdits),
    );
}

#[tokio::test]
async fn bridge_handler_enter_plan_applies_plan_transition_state() {
    let state = state_with_session().await;
    {
        let session_id = test_session_id();
        let handoff = state.session_handoff_snapshot(&session_id).unwrap();
        handoff.app_state.write().await.permissions.mode =
            Some(coco_types::PermissionMode::AcceptEdits);
    }

    let handler = SdkBridgeControlHandler::new(state.clone());
    handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::Plan,
        })
        .await
        .unwrap();

    let session_id = test_session_id();
    let handoff = state.session_handoff_snapshot(&session_id).unwrap();
    let app_state = handoff.app_state.read().await;
    assert_eq!(
        app_state.permissions.mode,
        Some(coco_types::PermissionMode::Plan),
    );
    assert_eq!(
        app_state.permissions.pre_plan_mode,
        Some(coco_types::PermissionMode::AcceptEdits),
    );
    assert!(app_state.plan_mode_entry_ms.is_some());
    assert!(!app_state.needs_plan_mode_exit_attachment);
}

#[tokio::test]
async fn bridge_handler_enter_plan_publishes_permission_mode_changed() {
    let state = state_with_session().await;
    let (tx, mut rx) = mpsc::channel(4);
    state.install_sdk_outbound_tx(tx).await;

    let handler = SdkBridgeControlHandler::new(state);
    let request = handler.handle(ControlRequest::SetPermissionMode {
        mode: coco_types::PermissionMode::Plan,
    });
    let notification = async {
        let msg = rx.recv().await.expect("permission mode notification");
        match msg {
            OutboundMessage::SessionEvent { event, routed, .. } => {
                if let Some(routed) = routed {
                    let _ = routed.send(());
                }
                match *event {
                    CoreEvent::Protocol(ServerNotification::PermissionModeChanged(params)) => {
                        assert_eq!(params.mode, coco_types::PermissionMode::Plan);
                        assert!(!params.bypass_available);
                    }
                    other => panic!("expected PermissionModeChanged, got {other:?}"),
                }
            }
            other => panic!("expected session event outbound, got {other:?}"),
        }
    };
    let (result, ()) = tokio::join!(request, notification);
    result.unwrap();
}

#[tokio::test]
async fn bridge_handler_rejects_when_no_active_session() {
    let state = Arc::new(SdkServerState::default());
    let handler = SdkBridgeControlHandler::new(state);
    let err = handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::Plan,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, coco_types::error_codes::INVALID_REQUEST);
    assert!(err.message.contains("no active session"));
}

#[tokio::test]
async fn bridge_handler_routes_set_model_through_sdk_dispatch() {
    let state = state_with_session().await;
    let handler = SdkBridgeControlHandler::new(state.clone());

    let ok = handler
        .handle(ControlRequest::SetModel {
            model: Some("provider/new-model".to_string()),
        })
        .await
        .unwrap();

    assert_eq!(ok, serde_json::Value::Null);
    let session_id = test_session_id();
    assert_eq!(
        state.session_metadata_snapshot(&session_id).unwrap().model,
        "provider/new-model"
    );
}

#[tokio::test]
async fn bridge_handler_routes_interrupt_through_sdk_dispatch() {
    let state = state_with_session().await;
    let token = CancellationToken::new();
    state.install_active_turn(
        coco_types::SessionId::try_new("sess-1").unwrap(),
        ActiveTurnHandles {
            cancel_token: token.clone(),
            turn_task: tokio::spawn(async {}),
            forwarder_task: tokio::spawn(async {}),
        },
    );
    let handler = SdkBridgeControlHandler::new(state);

    let ok = handler.handle(ControlRequest::Interrupt).await.unwrap();

    assert_eq!(ok, serde_json::Value::Null);
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn bridge_handler_routes_mcp_status_through_sdk_dispatch() {
    let state = state_with_session().await;
    let handler = SdkBridgeControlHandler::new(state);

    let value = handler.handle(ControlRequest::McpStatus).await.unwrap();

    assert_eq!(value["mcpServers"], serde_json::json!([]));
}

#[tokio::test]
async fn bridge_handler_returns_sdk_dispatch_errors() {
    let state = state_with_session().await;
    let handler = SdkBridgeControlHandler::new(state);

    let err = handler
        .handle(ControlRequest::GetContextUsage)
        .await
        .unwrap_err();

    assert_eq!(err.code, coco_types::error_codes::INVALID_REQUEST);
    assert!(err.message.contains("active session runtime"));
}
