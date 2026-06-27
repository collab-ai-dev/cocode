//! Windows sandbox implementation using restricted tokens + ACL enforcement.
//!
//! Wraps commands with a helper binary that creates a restricted process token
//! (via `CreateRestrictedToken`) and applies ACL deny/allow entries to enforce
//! filesystem isolation, then spawns the command with `CreateProcessAsUserW`.
//!
//! Adopted from codex-rs's `windows-sandbox-rs` crate. The architecture uses:
//! - **Token restriction**: `DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED`
//!   to strip the process of elevated rights
//! - **ACL enforcement**: Adds deny-write ACEs to protected paths and allow-write
//!   ACEs to writable roots, using per-workspace capability SIDs
//! - **Firewall rules**: Optional network isolation via Windows Firewall outbound
//!   block rules (requires elevated setup)
//!
//! ## Two-Stage Sandbox Pattern
//!
//! Similar to Linux's bwrap + seccomp two-stage approach, Windows uses:
//! - Stage 1 (outer): The parent process serializes config for the inner stage
//! - Stage 2 (inner): `coco --apply-windows-sandbox <config> -- <program> <args>`
//!   creates the restricted token, applies ACLs, and spawns the command
//!
//! ## Implementation Status
//!
//! The inner stage (Windows API calls for token restriction, ACL manipulation,
//! `CreateProcessAsUserW`) requires `unsafe` for FFI. Per project convention
//! (no `unsafe` in coco-rs crates), these calls must live in a separate
//! dependency crate (`coco-windows-sandbox`) that wraps Windows APIs in
//! safe abstractions — following the same pattern as codex-rs's
//! `windows-sandbox-rs` crate. The outer stage (config serialization and
//! command wrapping) is implemented here.

use base64::Engine;

use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::error::Result;
use crate::platform::SandboxPlatform;

use crate::error::sandbox_error::*;

/// Arg1 flag for the Windows sandbox inner stage dispatch.
/// Canonical definition lives in [`crate::inner_stage`].
pub use crate::inner_stage::APPLY_WINDOWS_SANDBOX_ARG1;

/// Windows sandbox implementation using restricted tokens + ACL enforcement.
///
/// Uses the coco binary's arg0 dispatch for the inner stage, passing sandbox
/// configuration as a base64-encoded JSON argument. The inner stage (in a
/// separate `coco-windows-sandbox` crate) handles:
/// 1. Deserializing the config
/// 2. Creating a restricted token via `CreateRestrictedToken`
/// 3. Applying ACL entries to filesystem paths
/// 4. Spawning the command via `CreateProcessAsUserW`
pub struct WindowsSandbox;

impl SandboxPlatform for WindowsSandbox {
    fn available(&self) -> bool {
        // coco-rs ships no in-tree native Windows sandbox backend (TS's
        // sandbox-runtime is the closed npm package; the inner stage here is a
        // stub that exits 1). Report unavailable so the enable gate fail-closes
        // to unsandboxed rather than emitting an `--apply-windows-sandbox`
        // re-exec the stub would reject. TS only claims macOS/Linux/WSL2.
        false
    }

    fn wrap_command(
        &self,
        config: &SandboxConfig,
        _command: &str,
        _session_tag: &str,
        _extra_writable_binds: &[std::path::PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> Result<()> {
        if config.enforcement == EnforcementLevel::Disabled {
            return Ok(());
        }

        // Serialize sandbox config for the inner stage
        let config_json = serde_json::to_string(config).map_err(|e| {
            PlatformNotAvailableSnafu {
                message: format!("Failed to serialize sandbox config: {e}"),
            }
            .build()
        })?;
        let config_b64 = base64::engine::general_purpose::STANDARD.encode(config_json.as_bytes());

        tracing::info!(
            enforcement = ?config.enforcement,
            writable_roots = config.writable_roots.len(),
            allow_network = config.allow_network,
            "Wrapping command with Windows restricted token sandbox"
        );

        // Extract current program and args from the command
        let inner = cmd.as_std();
        let program = inner.get_program().to_os_string();
        let args: Vec<_> = inner
            .get_args()
            .map(std::ffi::OsStr::to_os_string)
            .collect();

        // Find coco binary for inner stage dispatch.
        // Fallback to "coco" on PATH if current_exe() fails (e.g., /proc not mounted).
        let coco_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("coco"));

        // Rebuild command: coco --apply-windows-sandbox <config-b64> -- <program> <args>
        *cmd = tokio::process::Command::new(&coco_exe);
        cmd.arg(APPLY_WINDOWS_SANDBOX_ARG1);
        cmd.arg(&config_b64);
        cmd.arg("--");
        cmd.arg(&program);
        cmd.args(&args);

        Ok(())
    }
}

/// Apply Windows sandbox restrictions and exec the command.
///
/// This is the inner stage handler called via arg0 dispatch when the binary
/// receives `--apply-windows-sandbox <config-b64> -- <program> <args>`.
///
/// On Windows, delegates to the `coco-windows-sandbox` crate for actual
/// token restriction and ACL enforcement (which wraps Windows APIs in safe
/// abstractions per project convention).
///
/// On non-Windows platforms, prints an error and exits.
#[cfg(target_os = "windows")]
pub fn apply_windows_sandbox_and_exec(_config_b64: &str, _program: &str, _args: &[String]) -> ! {
    // When the `coco-windows-sandbox` crate is available, this will
    // delegate to it for token restriction + ACL enforcement + process
    // spawning. The crate wraps Windows APIs (`CreateRestrictedToken`,
    // `SetNamedSecurityInfoW`, `CreateProcessAsUserW`) in safe
    // abstractions following the codex-rs `windows-sandbox-rs` pattern.
    eprintln!("Windows sandbox: inner stage not yet connected to coco-windows-sandbox crate");
    std::process::exit(1);
}

/// On non-Windows platforms, prints an error and exits.
#[cfg(not(target_os = "windows"))]
pub fn apply_windows_sandbox_and_exec(_config_b64: &str, _program: &str, _args: &[String]) -> ! {
    eprintln!("Windows sandbox: not available on this platform");
    std::process::exit(1);
}

#[cfg(test)]
#[path = "windows.test.rs"]
mod tests;
