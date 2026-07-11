use std::sync::Arc;

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
use crate::sdk_server::outbound::send_session_event;
use crate::session_runtime::SessionHandle;

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
    plan_auto_options: coco_permissions::PlanModeAutoOptions,
) -> LivePermissionModeChange {
    let mut guard = app_state.write().await;
    let previous = guard.permissions.mode.unwrap_or(fallback_mode);
    let changed = coco_permissions::apply_permission_mode_transition_to_app_state(
        &mut guard,
        previous,
        mode,
        live_allow_rules,
        plan_auto_options,
    );
    LivePermissionModeChange { previous, changed }
}

pub async fn apply_to_runtime(
    session: &SessionHandle,
    mode: PermissionMode,
    event_tx: &mpsc::Sender<CoreEvent>,
    bypass_available: bool,
) -> LivePermissionModeChange {
    let fallback_mode = session.current_engine_config().await.permission_mode;
    session
        .update_engine_config(move |cfg| cfg.permission_mode = mode)
        .await;
    // The dangerous-rule snapshot the transition strips must come from the LIVE
    // allow rules (the single base the factory reads), not the now-dead config
    // maps. Read them off the shared `ToolAppState.permissions` base.
    let live_allow_rules = session
        .app_state()
        .read()
        .await
        .permissions
        .allow_rules
        .clone();
    let config = session.current_engine_config().await;
    let plan_auto_options = coco_permissions::PlanModeAutoOptions {
        use_auto_mode_during_plan: config.use_auto_mode_during_plan,
        auto_mode_available: config.permission_mode_availability.auto,
    };
    let change = apply_to_app_state(
        session.app_state(),
        fallback_mode,
        mode,
        &live_allow_rules,
        plan_auto_options,
    )
    .await;
    publish_core_if_changed(event_tx, mode, bypass_available, change.changed).await;
    change
}

pub fn sdk_bypass_available(state: &SdkServerState) -> bool {
    state.bypass_permissions_available()
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
    session_id: coco_types::SessionId,
    mode: PermissionMode,
    bypass_available: bool,
    changed: bool,
) {
    if !changed {
        return;
    }
    let _ = send_session_event(
        tx,
        session_id,
        CoreEvent::Protocol(permission_mode_changed(mode, bypass_available)),
    )
    .await;
}

pub async fn publish_sdk_state_outbound_if_changed(
    state: &SdkServerState,
    mode: PermissionMode,
    changed: bool,
) {
    let Some(tx) = state.sdk_outbound_tx_snapshot().await else {
        return;
    };
    let Some(session_id) = state.runtime_or_active_session_id().await else {
        return;
    };
    publish_outbound_if_changed(&tx, session_id, mode, sdk_bypass_available(state), changed).await;
}

fn permission_mode_changed(mode: PermissionMode, bypass_available: bool) -> ServerNotification {
    ServerNotification::PermissionModeChanged(PermissionModeChangedParams {
        mode,
        bypass_available,
    })
}
