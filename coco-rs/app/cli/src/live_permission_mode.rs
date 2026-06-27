use std::sync::Arc;
use std::sync::atomic::Ordering;

use coco_types::CoreEvent;
use coco_types::PermissionMode;
use coco_types::PermissionModeChangedParams;
use coco_types::PermissionRulesBySource;
use coco_types::ServerNotification;
use coco_types::ToolAppState;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::outbound::OutboundMessage;
use crate::session_runtime::SessionRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePermissionModeChange {
    pub previous: PermissionMode,
    pub changed: bool,
}

pub async fn apply_to_app_state(
    app_state: &Arc<RwLock<ToolAppState>>,
    fallback_mode: PermissionMode,
    mode: PermissionMode,
    live_allow_rules: &PermissionRulesBySource,
) -> LivePermissionModeChange {
    let mut guard = app_state.write().await;
    let previous = guard.permissions.mode.unwrap_or(fallback_mode);
    let changed = coco_permissions::apply_permission_mode_transition_to_app_state(
        &mut guard,
        previous,
        mode,
        live_allow_rules,
    );
    LivePermissionModeChange { previous, changed }
}

pub async fn apply_to_runtime(
    runtime: &Arc<SessionRuntime>,
    mode: PermissionMode,
    event_tx: &mpsc::Sender<CoreEvent>,
    bypass_available: bool,
) -> LivePermissionModeChange {
    let fallback_mode = runtime.current_engine_config().await.permission_mode;
    runtime
        .update_engine_config(move |cfg| cfg.permission_mode = mode)
        .await;
    // The dangerous-rule snapshot the transition strips must come from the LIVE
    // allow rules (the single base the factory reads), not the now-dead config
    // maps. Read them off the shared `ToolAppState.permissions` base.
    let live_allow_rules = runtime
        .app_state
        .read()
        .await
        .permissions
        .allow_rules
        .clone();
    let change =
        apply_to_app_state(&runtime.app_state, fallback_mode, mode, &live_allow_rules).await;
    publish_core_if_changed(event_tx, mode, bypass_available, change.changed).await;
    change
}

pub fn sdk_bypass_available(state: &SdkServerState) -> bool {
    state.bypass_permissions_available.load(Ordering::Relaxed)
}

pub async fn publish_core_if_changed(
    tx: &mpsc::Sender<CoreEvent>,
    mode: PermissionMode,
    bypass_available: bool,
    changed: bool,
) {
    if !changed {
        return;
    }
    let _ = tx
        .send(CoreEvent::Protocol(permission_mode_changed(
            mode,
            bypass_available,
        )))
        .await;
}

pub async fn publish_outbound_if_changed(
    tx: &mpsc::Sender<OutboundMessage>,
    mode: PermissionMode,
    bypass_available: bool,
    changed: bool,
) {
    if !changed {
        return;
    }
    let _ = tx
        .send(OutboundMessage::core_event(CoreEvent::Protocol(
            permission_mode_changed(mode, bypass_available),
        )))
        .await;
}

pub async fn publish_sdk_state_outbound_if_changed(
    state: &SdkServerState,
    mode: PermissionMode,
    changed: bool,
) {
    let Some(tx) = ({ state.outbound_tx.read().await.clone() }) else {
        return;
    };
    publish_outbound_if_changed(&tx, mode, sdk_bypass_available(state), changed).await;
}

fn permission_mode_changed(mode: PermissionMode, bypass_available: bool) -> ServerNotification {
    ServerNotification::PermissionModeChanged(PermissionModeChangedParams {
        mode,
        bypass_available,
    })
}
