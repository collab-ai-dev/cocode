//! Sandbox bootstrap lifecycle.
//!
//! Provides the multi-gate enable check that mirrors Claude Code's
//! 4-gate sandbox initialization sequence.

use crate::config::SandboxSettings;
use crate::deps;

/// Result of the 4-gate sandbox enable check.
#[derive(Debug, Clone)]
pub enum EnableCheckResult {
    /// All gates passed; sandbox can be activated.
    Enabled,
    /// User has not enabled sandbox in settings.
    DisabledBySettings,
    /// Current platform is not supported for sandboxing.
    DisabledByPlatform {
        /// Reason the platform is unsupported.
        reason: String,
    },
    /// Required dependencies are missing.
    DisabledByMissingDeps {
        /// Names of missing required dependencies.
        missing: Vec<String>,
    },
    /// Current platform is not in the enabled platforms list.
    DisabledByAllowlist,
}

impl EnableCheckResult {
    /// Returns true if sandbox should be enabled.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Check whether sandbox should be enabled via the 4-gate sequence.
///
/// Gates (in order):
/// 1. Settings enabled
/// 2. Supported platform (macOS or Linux, not WSL1)
/// 3. Platform in enabled platforms list
/// 4. Required dependencies available
pub fn check_enable_gates(settings: &SandboxSettings) -> EnableCheckResult {
    // Gate 1: Settings enabled
    if !settings.enabled {
        return EnableCheckResult::DisabledBySettings;
    }

    // Gate 2: Supported platform
    if !is_supported_platform() {
        return EnableCheckResult::DisabledByPlatform {
            reason: unsupported_reason(),
        };
    }

    // Gate 3: Platform in enabled list
    if !settings.is_platform_enabled() {
        return EnableCheckResult::DisabledByAllowlist;
    }

    // Gate 4: Required dependencies
    let missing = deps::missing_required();
    if !missing.is_empty() {
        return EnableCheckResult::DisabledByMissingDeps {
            missing: missing.iter().map(|s| (*s).to_string()).collect(),
        };
    }

    EnableCheckResult::Enabled
}

/// Public helper: is the current platform supported for sandboxing?
///
/// Used by the CLI bootstrap to feed
/// [`crate::sandbox_unavailable_reason`] without re-deriving the same
/// gate logic.
pub fn current_platform_supported() -> bool {
    is_supported_platform()
}

/// Check if the current platform is supported for sandboxing.
fn is_supported_platform() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    if cfg!(target_os = "linux") {
        // Exclude WSL1 (WSL2 with real kernel is fine)
        return !is_wsl1();
    }
    if cfg!(target_os = "windows") {
        return true;
    }
    false
}

/// Check if running under WSL1 (which lacks namespace support).
///
/// WSL1 doesn't support the namespace syscalls needed by bubblewrap.
/// WSL2 runs a real Linux kernel and is fully supported.
fn is_wsl1() -> bool {
    // WSLInterop only exists on WSL (both v1 and v2).
    if !std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists() {
        return false; // Not WSL at all
    }
    // WSL2 uses "microsoft-standard" kernel; WSL1 uses "Microsoft" (capital M).
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        return !version.contains("microsoft-standard");
    }
    // If we can't read /proc/version, assume WSL1 (conservative).
    tracing::warn!("Cannot read /proc/version on WSL; assuming WSL1 (sandbox disabled)");
    true
}

/// Human-readable reason why the current platform is unsupported.
fn unsupported_reason() -> String {
    if cfg!(target_os = "linux") && is_wsl1() {
        "WSL1 is not supported (missing namespace syscalls); use WSL2".to_string()
    } else {
        format!("unsupported OS: {}", std::env::consts::OS)
    }
}

#[cfg(test)]
#[path = "bootstrap.test.rs"]
mod tests;
