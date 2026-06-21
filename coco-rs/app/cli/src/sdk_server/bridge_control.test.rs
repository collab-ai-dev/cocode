use std::sync::Arc;
use std::sync::atomic::Ordering;

use coco_bridge::ControlRequest;
use coco_bridge::ControlRequestHandler;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use tokio::sync::mpsc;

use super::SdkBridgeControlHandler;
use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::SessionHandle;
use crate::sdk_server::outbound::OutboundMessage;

fn state_with_session() -> Arc<SdkServerState> {
    let state = Arc::new(SdkServerState::default());
    {
        let mut slot = state.session.try_write().unwrap();
        *slot = Some(SessionHandle::new(
            "sess-1".into(),
            "/tmp".into(),
            "mock-model".into(),
        ));
    }
    state
}

#[tokio::test]
async fn bridge_handler_rejects_bypass_without_capability() {
    // Startup capability defaults to false — the bridge handler
    // must refuse to escalate into BypassPermissions.
    let state = state_with_session();
    assert!(!state.bypass_permissions_available.load(Ordering::Relaxed));

    let handler = SdkBridgeControlHandler::new(state.clone());
    let err = handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, coco_types::error_codes::PERMISSION_DENIED);
    assert!(err.message.contains("bypassPermissions"));

    // Session was not mutated.
    let slot = state.session.read().await;
    assert!(
        !matches!(
            slot.as_ref().unwrap().permission_mode,
            Some(coco_types::PermissionMode::BypassPermissions)
        ),
        "rejected bridge request must not write session.permission_mode",
    );
}

#[tokio::test]
async fn bridge_handler_accepts_bypass_when_capability_on() {
    // Flipping the capability flag at startup allows bridge-origin
    // escalation. Verifies the handler reads the live AtomicBool
    // (not a cached value).
    let state = state_with_session();
    state
        .bypass_permissions_available
        .store(true, Ordering::Relaxed);

    let handler = SdkBridgeControlHandler::new(state.clone());
    let ok = handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap();
    assert_eq!(ok, serde_json::Value::Null);

    let slot = state.session.read().await;
    let session = slot.as_ref().unwrap();
    assert_eq!(
        session.permission_mode,
        Some(coco_types::PermissionMode::BypassPermissions),
    );
    // app_state propagation — engine's live source of truth.
    let app_state = session.app_state.read().await;
    assert_eq!(
        app_state.permissions.mode,
        Some(coco_types::PermissionMode::BypassPermissions),
    );
}

#[tokio::test]
async fn bridge_handler_allows_non_bypass_modes_unconditionally() {
    // Non-bypass transitions never touch the killswitch gate.
    let state = state_with_session();
    assert!(!state.bypass_permissions_available.load(Ordering::Relaxed));

    let handler = SdkBridgeControlHandler::new(state.clone());
    handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        })
        .await
        .unwrap();

    let slot = state.session.read().await;
    assert_eq!(
        slot.as_ref().unwrap().permission_mode,
        Some(coco_types::PermissionMode::AcceptEdits),
    );
}

#[tokio::test]
async fn bridge_handler_enter_plan_applies_plan_transition_state() {
    let state = state_with_session();
    {
        let slot = state.session.read().await;
        let session = slot.as_ref().unwrap();
        session.app_state.write().await.permissions.mode =
            Some(coco_types::PermissionMode::AcceptEdits);
    }

    let handler = SdkBridgeControlHandler::new(state.clone());
    handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::Plan,
        })
        .await
        .unwrap();

    let slot = state.session.read().await;
    let session = slot.as_ref().unwrap();
    let app_state = session.app_state.read().await;
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
    let state = state_with_session();
    let (tx, mut rx) = mpsc::channel(4);
    {
        let mut outbox = state.outbound_tx.write().await;
        *outbox = Some(tx);
    }

    let handler = SdkBridgeControlHandler::new(state);
    handler
        .handle(ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::Plan,
        })
        .await
        .unwrap();

    let msg = rx.recv().await.expect("permission mode notification");
    match msg {
        OutboundMessage::CoreEvent(event) => match *event {
            CoreEvent::Protocol(ServerNotification::PermissionModeChanged(params)) => {
                assert_eq!(params.mode, coco_types::PermissionMode::Plan);
                assert!(!params.bypass_available);
            }
            other => panic!("expected PermissionModeChanged, got {other:?}"),
        },
        other => panic!("expected CoreEvent outbound, got {other:?}"),
    }
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
async fn bridge_handler_rejects_unrouted_variants() {
    // Only SetPermissionMode is routed through the bridge trait
    // today. Other variants must fail closed rather than silently
    // no-op, so the caller knows to dispatch via the SDK path.
    let state = state_with_session();
    let handler = SdkBridgeControlHandler::new(state);
    let err = handler.handle(ControlRequest::Interrupt).await.unwrap_err();
    assert_eq!(err.code, coco_types::error_codes::METHOD_NOT_FOUND);
    assert!(err.message.contains("not yet routed"));
}
