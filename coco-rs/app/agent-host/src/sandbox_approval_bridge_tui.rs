//! Interactive-TUI sandbox approval bridge.
//!
//! The SDK path installs [`crate::sdk_server::SdkSandboxApprovalBridge`] to
//! surface a denied sandbox operation over the NDJSON control channel. This is
//! the interactive-TUI counterpart: when a sandboxed path/network operation is
//! about to be denied, [`crate::session_runtime::SessionRuntime`]'s
//! `Arc<SandboxState>` consults the installed bridge, and this implementation
//! turns that into a `SandboxApprovalRequired` overlay in the user's terminal.
//!
//! ## Reuses the tool-permission round-trip
//!
//! It deliberately shares the SAME [`PendingApprovals`] map as the
//! [`crate::tui_permission_bridge::TuiPermissionBridge`], so the existing
//! `tui_runner` `UserCommand::ApprovalResponse` arm resolves a sandbox prompt
//! with no extra wiring. Request ids are fresh `Uuid`s, so they never collide
//! with the engine's tool-permission request ids in the shared map.
//!
//! The shared map carries `oneshot::Sender<ToolPermissionResolution>`; this
//! bridge translates the resolved [`ToolPermissionDecision`] into a
//! [`SandboxApprovalDecision`]. The sandbox prompt is a plain allow/deny (no
//! rule persistence — TS sandbox approvals don't add rules either), so the
//! resolution's `applied_updates` / `updated_input` are ignored.
//!
//! Fail-closed by construction: a closed notification channel (TUI shutting
//! down) or a dropped response channel both resolve to
//! [`SandboxApprovalDecision::Rejected`], leaving the underlying deny in place.

use async_trait::async_trait;
use coco_query::CoreEvent;
use coco_sandbox::{
    SandboxApprovalBridge, SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
use coco_tool_runtime::ToolPermissionDecision;
use coco_types::TuiOnlyEvent;
use tokio::sync::{mpsc, oneshot};

use crate::tui_permission_bridge::{PendingApprovalEntry, PendingApprovals};

/// Bridge implementation for the interactive TUI sandbox approval overlay.
pub struct TuiSandboxApprovalBridge {
    notification_tx: mpsc::Sender<CoreEvent>,
    pending: PendingApprovals,
}

impl TuiSandboxApprovalBridge {
    /// Construct the bridge. Pass the SAME `pending` map handed to
    /// [`crate::tui_permission_bridge::TuiPermissionBridge::new`] and the
    /// tui_runner's `ApprovalResponse` arm, plus a clone of the TUI's
    /// `CoreEvent` notification sender.
    pub fn new(notification_tx: mpsc::Sender<CoreEvent>, pending: PendingApprovals) -> Self {
        Self {
            notification_tx,
            pending,
        }
    }

    /// Human-readable prompt line for the overlay. Phrasing mirrors
    /// `createSandboxAskCallback` which surfaces "Allow network connection to
    /// {host}?"); path read/write are the coco-rs filesystem-sandbox extension.
    fn describe(request: &SandboxApprovalRequest) -> String {
        match request.operation {
            SandboxOperation::Network => {
                format!("Allow network connection to {}?", request.path)
            }
            SandboxOperation::Read => format!("Allow sandboxed read of {}?", request.path),
            SandboxOperation::Write => format!("Allow sandboxed write to {}?", request.path),
            // `SandboxOperation` is `#[non_exhaustive]` — describe generically.
            _ => format!("Allow sandboxed {} operation?", request.operation.as_str()),
        }
    }
}

#[async_trait]
impl SandboxApprovalBridge for TuiSandboxApprovalBridge {
    async fn request_approval(&self, request: SandboxApprovalRequest) -> SandboxApprovalDecision {
        let request_id = uuid::Uuid::new_v4().to_string();
        let operation = Self::describe(&request);

        // Register the oneshot BEFORE emitting the event so a fast Approve
        // click can't race ahead of the pending entry. `_guard: None` — the
        // sandbox prompt does not participate in the pending-permission
        // counter (it's an out-of-band physical-layer prompt, not a tool Ask).
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                request_id.clone(),
                PendingApprovalEntry {
                    sender: tx,
                    _guard: None,
                },
            );
        }

        let event = CoreEvent::Tui(TuiOnlyEvent::SandboxApprovalRequired {
            request_id: request_id.clone(),
            operation,
        });
        if self.notification_tx.send(event).await.is_err() {
            // TUI is shutting down — clean up and fail closed.
            self.pending.write().await.remove(&request_id);
            return SandboxApprovalDecision::Rejected;
        }

        match rx.await {
            Ok(resolution) if matches!(resolution.decision, ToolPermissionDecision::Approved) => {
                SandboxApprovalDecision::Approved
            }
            // Rejected, or the response channel closed (TUI exited without
            // resolving) → fail closed. Best-effort pending cleanup.
            _ => {
                self.pending.write().await.remove(&request_id);
                SandboxApprovalDecision::Rejected
            }
        }
    }
}

#[cfg(test)]
#[path = "sandbox_approval_bridge_tui.test.rs"]
mod tests;
