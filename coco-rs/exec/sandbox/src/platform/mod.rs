//! Platform-specific sandbox implementations.
//!
//! Provides a `SandboxPlatform` trait with platform-gated implementations
//! for macOS (Seatbelt), Linux (bubblewrap + seccomp), and Windows
//! (restricted token + ACL enforcement).

use crate::config::SandboxConfig;
use crate::error::Result;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

pub mod windows;

/// Platform-specific sandbox enforcement.
///
/// Implementations wrap child process commands with OS-level restrictions
/// (Seatbelt on macOS, bubblewrap + seccomp on Linux).
pub trait SandboxPlatform: Send + Sync {
    /// Returns true if this sandbox implementation is available on the current OS.
    fn available(&self) -> bool;

    /// Wraps a command to run under sandbox enforcement.
    ///
    /// Modifies the command to execute within the platform-specific sandbox,
    /// applying filesystem, network, and process isolation according to the config.
    ///
    /// `command` is the original shell command string (used on macOS for
    /// violation correlation via `CMD64_` tags). `session_tag` is the
    /// session-unique tag for log filtering (format: `_<hex>_SBX`).
    ///
    /// `extra_writable_binds` — per-command host paths that should be
    /// bind-mounted writable inside the sandbox. The shell executor
    /// passes a freshly-allocated tmpdir (`/tmp/coco-sbx-<rand>`) here
    /// so the inner command can write the cwd-tracking file there and
    /// the parent process can read it after exec.
    fn wrap_command(
        &self,
        config: &SandboxConfig,
        command: &str,
        session_tag: &str,
        extra_writable_binds: &[std::path::PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> Result<()>;
}

/// Returns the platform-appropriate sandbox implementation.
#[cfg(target_os = "macos")]
pub fn create_platform() -> Box<dyn SandboxPlatform> {
    Box::new(macos::MacOsSandbox)
}

/// Returns the platform-appropriate sandbox implementation.
#[cfg(target_os = "linux")]
pub fn create_platform() -> Box<dyn SandboxPlatform> {
    Box::new(linux::LinuxSandbox)
}

/// Returns the Windows sandbox implementation.
#[cfg(target_os = "windows")]
pub fn create_platform() -> Box<dyn SandboxPlatform> {
    Box::new(windows::WindowsSandbox)
}

/// Returns a no-op sandbox for unsupported platforms.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn create_platform() -> Box<dyn SandboxPlatform> {
    Box::new(NoopSandbox)
}

/// No-op sandbox for unsupported platforms.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
struct NoopSandbox;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
impl SandboxPlatform for NoopSandbox {
    fn available(&self) -> bool {
        false
    }

    fn wrap_command(
        &self,
        _config: &SandboxConfig,
        _command: &str,
        _session_tag: &str,
        _extra_writable_binds: &[std::path::PathBuf],
        _cmd: &mut tokio::process::Command,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
