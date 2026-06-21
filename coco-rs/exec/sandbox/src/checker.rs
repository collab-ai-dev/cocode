//! Permission checking for sandbox-enforced operations.
//!
//! `PermissionChecker` is a fail-closed validator. When an approval bridge is
//! installed (via [`SandboxState::set_approval_bridge`](crate::SandboxState),
//! propagated through [`SandboxState::permission_checker`](crate::SandboxState)),
//! the async variant [`PermissionChecker::check_path_async`] consults the bridge
//! before returning a deny — mirroring TS's interactive "Allow this read/write?"
//! prompt. Without a bridge it is identical to the sync `check_path`.
//!
//! Production consumers:
//! - Read/Write/Edit pre-flight → [`PermissionChecker::check_path_async`]
//!   (`core/tools/src/tools/sandbox_preflight.rs`).
//! - SDK control channel → `SdkSandboxApprovalBridge`
//!   (`app/cli/src/sdk_server`), installed onto the live `SandboxState`.
//!
//! Network approval is handled at the egress proxy itself (a `NetworkAskCallback`
//! built from the installed bridge in `state.rs::build_network_ask_callback`),
//! not here — a denied CONNECT consults the bridge before refusing, so the
//! checker has no `check_network_async` counterpart.

use std::path::Path;

use crate::bridge::{
    SandboxApprovalBridgeRef, SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::error::Result;
use crate::error::sandbox_error::*;

/// Checks permissions against the sandbox configuration.
#[derive(Clone)]
pub struct PermissionChecker {
    config: SandboxConfig,
    /// Optional async approval bridge — when set,
    /// [`Self::check_path_async`] / [`Self::check_network_async`]
    /// consult the bridge before returning a deny error. Static
    /// callers ([`Self::check_path`] / [`Self::check_network`]) keep
    /// the synchronous, fail-closed semantics for backwards
    /// compatibility. See [`crate::bridge`].
    approval_bridge: Option<SandboxApprovalBridgeRef>,
}

impl std::fmt::Debug for PermissionChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionChecker")
            .field("config", &self.config)
            .field("approval_bridge", &self.approval_bridge.is_some())
            .finish()
    }
}

impl PermissionChecker {
    /// Creates a new checker with the given configuration. No approval
    /// bridge — denies are immediate. Install one via
    /// [`Self::with_approval_bridge`] / [`Self::set_approval_bridge`].
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            config,
            approval_bridge: None,
        }
    }

    /// Builder-style: install an approval bridge at construction.
    #[must_use]
    pub fn with_approval_bridge(mut self, bridge: SandboxApprovalBridgeRef) -> Self {
        self.approval_bridge = Some(bridge);
        self
    }

    /// Replace the approval bridge after construction.
    pub fn set_approval_bridge(&mut self, bridge: Option<SandboxApprovalBridgeRef>) {
        self.approval_bridge = bridge;
    }

    /// Returns true when an approval bridge is installed. Useful for
    /// callers that want to skip async detours in tight loops where
    /// no bridge is configured.
    pub fn has_approval_bridge(&self) -> bool {
        self.approval_bridge.is_some()
    }

    /// Returns a reference to the underlying configuration.
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Checks whether the given path is accessible.
    ///
    /// Cross-cutting checks (applied before enforcement level logic):
    /// - `denied_read_paths` blocks reads to specific paths
    /// - `deny_write_paths` blocks writes to specific paths
    /// - `allow_git_config` controls write access to git config files
    ///
    /// Enforcement levels:
    /// - `Disabled`: all paths allowed (cross-cutting checks still apply)
    /// - `ReadOnly`: reads allowed; writes denied
    /// - `WorkspaceWrite`: reads allowed; writes only to writable roots
    /// - `Strict`: path must be under a root; writes require writable root
    pub fn check_path(&self, path: &Path, write: bool) -> Result<()> {
        let op = if write { "write" } else { "read" };

        // Cross-cutting: deny-read paths block reads in all non-disabled modes
        if self.config.enforcement != EnforcementLevel::Disabled
            && !write
            && self.is_denied_read_path(path)
        {
            tracing::info!(
                path = %path.display(),
                operation = op,
                enforcement = ?self.config.enforcement,
                decision = "denied",
                reason = "deny_read rule",
                "sandbox.permission_check"
            );
            return PathDeniedSnafu {
                path: path.display().to_string(),
            }
            .fail();
        }

        // Cross-cutting: deny-write paths block writes in all non-disabled modes
        if self.config.enforcement != EnforcementLevel::Disabled
            && write
            && self.is_deny_write_path(path)
        {
            tracing::info!(
                path = %path.display(),
                operation = op,
                enforcement = ?self.config.enforcement,
                decision = "denied",
                reason = "deny_write rule",
                "sandbox.permission_check"
            );
            return WriteDeniedSnafu {
                message: format!("write denied by deny_write rule: {}", path.display()),
            }
            .fail();
        }

        // Cross-cutting: git config protection
        if self.config.enforcement != EnforcementLevel::Disabled
            && write
            && !self.config.allow_git_config
            && is_git_config_path(path)
        {
            tracing::info!(
                path = %path.display(),
                operation = op,
                enforcement = ?self.config.enforcement,
                decision = "denied",
                reason = "git config protection",
                "sandbox.permission_check"
            );
            return WriteDeniedSnafu {
                message: format!(
                    "writing to git config is not allowed (allow_git_config=false): {}",
                    path.display()
                ),
            }
            .fail();
        }

        match self.config.enforcement {
            EnforcementLevel::Disabled => Ok(()),
            EnforcementLevel::ReadOnly => {
                if write {
                    tracing::info!(
                        path = %path.display(),
                        operation = op,
                        enforcement = "read_only",
                        decision = "denied",
                        reason = "read-only mode",
                        "sandbox.permission_check"
                    );
                    return WriteDeniedSnafu {
                        message: format!(
                            "sandbox is in read-only mode, cannot write to: {}",
                            path.display()
                        ),
                    }
                    .fail();
                }
                Ok(())
            }
            EnforcementLevel::WorkspaceWrite => {
                if write {
                    if self.is_denied_path(path) {
                        tracing::info!(
                            path = %path.display(),
                            operation = op,
                            enforcement = "workspace_write",
                            decision = "denied",
                            reason = "denied path",
                            "sandbox.permission_check"
                        );
                        return PathDeniedSnafu {
                            path: path.display().to_string(),
                        }
                        .fail();
                    }
                    if !self.is_writable_path(path) {
                        tracing::info!(
                            path = %path.display(),
                            operation = op,
                            enforcement = "workspace_write",
                            decision = "denied",
                            reason = "outside writable roots",
                            "sandbox.permission_check"
                        );
                        return WriteDeniedSnafu {
                            message: format!(
                                "path is not under any writable root: {}",
                                path.display()
                            ),
                        }
                        .fail();
                    }
                }
                Ok(())
            }
            EnforcementLevel::Strict => {
                if self.is_denied_path(path) {
                    tracing::info!(
                        path = %path.display(),
                        operation = op,
                        enforcement = "strict",
                        decision = "denied",
                        reason = "denied path",
                        "sandbox.permission_check"
                    );
                    return PathDeniedSnafu {
                        path: path.display().to_string(),
                    }
                    .fail();
                }

                if !self.is_under_any_root(path) {
                    tracing::info!(
                        path = %path.display(),
                        operation = op,
                        enforcement = "strict",
                        decision = "denied",
                        reason = "outside all roots",
                        "sandbox.permission_check"
                    );
                    return PathDeniedSnafu {
                        path: path.display().to_string(),
                    }
                    .fail();
                }

                if write && !self.is_writable_path(path) {
                    tracing::info!(
                        path = %path.display(),
                        operation = op,
                        enforcement = "strict",
                        decision = "denied",
                        reason = "read-only subpath",
                        "sandbox.permission_check"
                    );
                    return WriteDeniedSnafu {
                        message: format!("path is under a read-only subpath: {}", path.display()),
                    }
                    .fail();
                }

                Ok(())
            }
        }
    }

    /// Checks whether network access is allowed.
    pub fn check_network(&self) -> Result<()> {
        if self.config.enforcement == EnforcementLevel::Disabled {
            return Ok(());
        }

        if !self.config.allow_network {
            tracing::info!(
                enforcement = ?self.config.enforcement,
                decision = "denied",
                reason = "network access disabled",
                "sandbox.network_check"
            );
            return NetworkDeniedSnafu.fail();
        }

        Ok(())
    }

    // ── Async (bridge-aware) variants — D7 ──
    //
    // The static `check_path` / `check_network` keep their existing
    // fail-closed semantics. Callers that want to surface a denied
    // operation through an interactive approval flow call the `_async`
    // variant; when an approval bridge is installed and grants
    // approval, the deny error is rewritten into `Ok(())`. Without a
    // bridge installed (or with a `Rejected` decision), the original
    // error stands.

    /// Async variant of [`Self::check_path`] that consults the approval
    /// bridge on deny. Behaviour:
    ///
    /// - No bridge installed → identical to [`Self::check_path`].
    /// - Bridge returns [`SandboxApprovalDecision::Approved`] → caller's
    ///   `Result` becomes `Ok(())`. The deny was overridden by user
    ///   consent.
    /// - Bridge returns [`SandboxApprovalDecision::Rejected`] → preserve
    ///   the original deny error.
    /// - The static gate passing (no deny) → return immediately, no
    ///   bridge consultation.
    pub async fn check_path_async(&self, path: &Path, write: bool) -> Result<()> {
        match self.check_path(path, write) {
            Ok(()) => Ok(()),
            Err(e) => {
                let Some(bridge) = self.approval_bridge.as_ref() else {
                    return Err(e);
                };
                let request = SandboxApprovalRequest {
                    operation: if write {
                        SandboxOperation::Write
                    } else {
                        SandboxOperation::Read
                    },
                    path: path.display().to_string(),
                    reason: e.to_string(),
                };
                match bridge.request_approval(request).await {
                    SandboxApprovalDecision::Approved => {
                        tracing::info!(
                            path = %path.display(),
                            operation = if write { "write" } else { "read" },
                            decision = "approved_by_bridge",
                            "sandbox.permission_check"
                        );
                        Ok(())
                    }
                    SandboxApprovalDecision::Rejected => Err(e),
                }
            }
        }
    }

    /// Returns true if the path is under any writable root (respecting subpath restrictions).
    pub fn is_writable_path(&self, path: &Path) -> bool {
        self.config
            .writable_roots
            .iter()
            .any(|root| root.is_writable(path))
    }

    /// Returns true if the path is under any writable root (ignoring subpath restrictions).
    ///
    /// In `Disabled` mode with no configured roots, returns `true` (everything accessible).
    /// In other modes with no roots, returns `false` (nothing accessible).
    pub fn is_under_any_root(&self, path: &Path) -> bool {
        if self.config.writable_roots.is_empty() {
            return self.config.enforcement == EnforcementLevel::Disabled;
        }
        self.config
            .writable_roots
            .iter()
            .any(|root| root.contains(path))
    }

    /// Returns true if the path is under one of the denied paths.
    fn is_denied_path(&self, path: &Path) -> bool {
        self.config
            .denied_paths
            .iter()
            .any(|denied| path.starts_with(denied))
    }

    /// Returns true if the path is under a deny-read path.
    fn is_denied_read_path(&self, path: &Path) -> bool {
        self.config
            .denied_read_paths
            .iter()
            .any(|denied| path.starts_with(denied))
    }

    /// Returns true if the path is under a deny-write path.
    fn is_deny_write_path(&self, path: &Path) -> bool {
        self.config
            .deny_write_paths
            .iter()
            .any(|denied| path.starts_with(denied))
    }
}

/// Check if a path is a git config file.
fn is_git_config_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.ends_with("/.git/config")
        || path_str.ends_with("/.gitconfig")
        || path_str.contains("/.git/config/")
}

#[cfg(test)]
#[path = "checker.test.rs"]
mod tests;
