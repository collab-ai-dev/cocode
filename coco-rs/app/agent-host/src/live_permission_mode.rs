use std::sync::Arc;

use coco_types::{
    CoreEvent, PermissionMode, PermissionModeChangedParams, PermissionRulesBySource,
    ServerNotification, ToolAppState,
};
use tokio::sync::{RwLock, mpsc};

use crate::{
    app_server_host::outbound::{OutboundMessage, send_session_event},
    session_runtime::SessionHandle,
};

pub use crate::session_runtime::PermissionModeChange as LivePermissionModeChange;

pub struct EnsurePlanModeResult {
    pub session_id: coco_types::SessionId,
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
    let change = session.set_permission_mode(mode).await;
    publish_core_if_changed(event_tx, mode, bypass_available, change.changed).await;
    change
}

pub async fn ensure_plan_mode(
    session: &SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> EnsurePlanModeResult {
    let session_id = session.session_id().clone();
    let previous = session.effective_permission_mode().await;
    if previous == PermissionMode::Plan {
        return EnsurePlanModeResult {
            session_id,
            previous,
            changed: false,
        };
    }

    let change = apply_to_runtime(
        session,
        PermissionMode::Plan,
        event_tx,
        session.bypass_permissions_available().await,
    )
    .await;
    EnsurePlanModeResult {
        session_id,
        previous: change.previous,
        changed: change.changed,
    }
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

fn permission_mode_changed(mode: PermissionMode, bypass_available: bool) -> ServerNotification {
    ServerNotification::PermissionModeChanged(PermissionModeChangedParams {
        mode,
        bypass_available,
    })
}
