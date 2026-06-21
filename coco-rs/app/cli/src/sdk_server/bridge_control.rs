//! Production [`coco_bridge::ControlRequestHandler`] impl backed by
//! the SDK session state.
//!
//! The bridge crate defines the transport + trait; policy lives here
//! because `app/cli` is the layer that owns `SdkServerState` +
//! depends on `coco_permissions` / `coco_types`. Wiring looks like:
//!
//! ```ignore
//! let handler = Arc::new(SdkBridgeControlHandler::new(server.state()));
//! while let Some(msg) = incoming.recv().await {
//!     if let ReplInMessage::ControlRequest { request_id, request } = msg {
//!         let out = coco_bridge::dispatch_control(&*handler, request_id, request).await;
//!         bridge.send(out).await?;
//!     }
//! }
//! ```
//!
//! Security contract: every bypass-origin site
//! (TUI `UserCommand::SetPermissionMode`,
//! SDK `handle_set_permission_mode`, and this bridge handler)
//! enforces the same rule — reject `BypassPermissions` when the
//! session's startup capability gate is off.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use coco_bridge::ControlError;
use coco_bridge::ControlRequest;
use coco_bridge::ControlRequestHandler;

use super::handlers::SdkServerState;

/// Production handler for REPL-bridge control requests. Holds an
/// `Arc<SdkServerState>` so it can read the bypass capability, mutate
/// the active session's permission mode, and propagate to
/// `app_state` — reusing the same code path as
/// `sdk_server::handlers::runtime::handle_set_permission_mode`.
pub struct SdkBridgeControlHandler {
    state: Arc<SdkServerState>,
}

impl SdkBridgeControlHandler {
    pub fn new(state: Arc<SdkServerState>) -> Self {
        Self { state }
    }

    async fn set_permission_mode(
        &self,
        mode: coco_types::PermissionMode,
    ) -> Result<serde_json::Value, ControlError> {
        // Same guard as the SDK handler + TUI runner — keep all three
        // bypass origins enforcing identical rules.
        if mode == coco_types::PermissionMode::BypassPermissions
            && !self
                .state
                .bypass_permissions_available
                .load(Ordering::Relaxed)
        {
            return Err(ControlError::new(
                coco_types::error_codes::PERMISSION_DENIED,
                "Cannot set permission mode to bypassPermissions because \
                 the session was not launched with \
                 --dangerously-skip-permissions (or \
                 --allow-dangerously-skip-permissions).",
            ));
        }

        let mut slot = self.state.session.write().await;
        let Some(session) = slot.as_mut() else {
            return Err(ControlError::new(
                coco_types::error_codes::INVALID_REQUEST,
                "no active session",
            ));
        };
        session.permission_mode = Some(mode);

        // Release the session lock before acquiring app_state — keeps
        // lock order consistent with the SDK handler.
        let app_state = session.app_state.clone();
        drop(slot);
        // Strip provenance from THIS session's live base (the per-SessionHandle
        // base the engine runs against) — the same base `apply_to_app_state`
        // writes, so strip/restore stay coherent.
        let live_allow_rules = app_state.read().await.permissions.allow_rules.clone();
        let change = crate::live_permission_mode::apply_to_app_state(
            &app_state,
            coco_types::PermissionMode::Default,
            mode,
            &live_allow_rules,
        )
        .await;
        crate::live_permission_mode::publish_sdk_state_outbound_if_changed(
            &self.state,
            mode,
            change.changed,
        )
        .await;

        Ok(serde_json::Value::Null)
    }
}

#[async_trait::async_trait]
impl ControlRequestHandler for SdkBridgeControlHandler {
    async fn handle(&self, request: ControlRequest) -> Result<serde_json::Value, ControlError> {
        match request {
            ControlRequest::SetPermissionMode { mode } => self.set_permission_mode(mode).await,
            // Remaining variants live on the SDK dispatcher path;
            // plumbing each through the bridge trait is a separate
            // task. Fail closed so a partially-wired bridge doesn't
            // silently drop requests on the floor.
            other => Err(ControlError::new(
                coco_types::error_codes::METHOD_NOT_FOUND,
                format!(
                    "bridge control request {other:?} not yet routed — \
                     dispatch through the SDK server instead"
                ),
            )),
        }
    }
}

#[cfg(test)]
#[path = "bridge_control.test.rs"]
mod tests;
