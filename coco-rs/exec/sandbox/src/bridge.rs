//! Optional async approval bridge for interactive permission prompts.
//!
//! When a sandboxed operation hits a deny rule, [`PermissionChecker`]
//! normally returns `Err` immediately. A caller (TUI, SDK, leader UI)
//! that wants the user to *approve* a denied operation can install an
//! implementation of [`SandboxApprovalBridge`]. The async variant
//! [`crate::checker::PermissionChecker::check_path_async`] consults
//! the bridge before returning the deny error: an `Approved` decision
//! converts the result back to `Ok(())`; `Rejected` (or no bridge
//! installed) preserves the original error.
//!
//! TS parity: Claude Code's sandbox surfaces a "Allow this write?" /
//! "Allow this network call?" prompt. Coco-rs keeps the underlying
//! deny semantics deterministic — the bridge is opt-in and lives at
//! a clearly-labelled seam, so the threat model stays auditable.
//!
//! ## Layering
//!
//! This trait lives in `coco-sandbox` (L2). It deliberately does NOT
//! depend on `coco-tool-runtime`'s [`ToolPermissionBridge`] — the
//! sandbox is at a *physical* enforcement layer, while
//! `ToolPermissionBridge` covers *semantic* tool permission. A higher
//! layer (e.g. `coco-coordinator` or the CLI) can adapt one to the
//! other when both seams need to share an approval UI.
//!
//! [`PermissionChecker`]: crate::checker::PermissionChecker
//! [`ToolPermissionBridge`]: ../../core/tool-runtime/src/permission_bridge.rs

use std::sync::Arc;

/// Kind of operation that triggered the deny.
///
/// `#[non_exhaustive]` — future operation kinds (subprocess spawn,
/// network listen, raw socket) can be added without a major-version
/// bump. Bridge implementations must use a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SandboxOperation {
    /// File read.
    Read,
    /// File / directory write.
    Write,
    /// Outbound network access.
    Network,
}

impl SandboxOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Network => "network",
        }
    }
}

/// Request payload handed to a [`SandboxApprovalBridge`] when the
/// sandbox is about to deny an operation.
#[derive(Debug, Clone)]
pub struct SandboxApprovalRequest {
    /// Operation kind.
    pub operation: SandboxOperation,
    /// Filesystem path (when applicable; empty for network requests).
    pub path: String,
    /// Human-readable reason for the deny — surfaced verbatim to the
    /// approval UI so the user understands why approval is being
    /// asked. Mirrors the `reason=` field on the existing
    /// `tracing::info!("sandbox.permission_check", …)` emissions in
    /// [`crate::checker::PermissionChecker::check_path`].
    pub reason: String,
}

/// User decision returned by a bridge.
///
/// `#[non_exhaustive]` — future decisions (e.g. `ApproveAndPersist`
/// for "always allow this path") can be added without a major-version
/// bump. Callers must use a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SandboxApprovalDecision {
    /// Approve this operation. The caller's `Result` becomes `Ok`.
    Approved,
    /// Reject this operation. The caller's original deny error stands.
    Rejected,
}

/// Trait for forwarding sandbox deny events to an approval surface.
///
/// Async because production approvals typically round-trip through a
/// UI / IPC channel. The default `NoOpSandboxApprovalBridge` always
/// rejects — installing it is equivalent to leaving the bridge unset.
#[async_trait::async_trait]
pub trait SandboxApprovalBridge: Send + Sync {
    /// Ask the approval surface whether the denied operation should
    /// proceed. Implementations must respond fail-closed
    /// (`Rejected`) on cancellation / IO failure so a flaky bridge
    /// can't silently approve.
    async fn request_approval(&self, request: SandboxApprovalRequest) -> SandboxApprovalDecision;
}

/// Shared handle type.
pub type SandboxApprovalBridgeRef = Arc<dyn SandboxApprovalBridge>;

/// No-op implementation that always rejects. Useful for tests and as
/// the explicit default — installing this signals "the bridge seam is
/// wired but the user-facing approval UI is not, so behave exactly as
/// the unbridged path".
#[derive(Debug, Clone, Default)]
pub struct NoOpSandboxApprovalBridge;

#[async_trait::async_trait]
impl SandboxApprovalBridge for NoOpSandboxApprovalBridge {
    async fn request_approval(&self, _request: SandboxApprovalRequest) -> SandboxApprovalDecision {
        SandboxApprovalDecision::Rejected
    }
}

#[cfg(test)]
#[path = "bridge.test.rs"]
mod tests;
