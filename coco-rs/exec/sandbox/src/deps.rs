//! Sandbox dependency detection.
//!
//! Checks whether required platform-specific binaries are available
//! before attempting to enable sandbox enforcement.

use std::path::PathBuf;

/// Result of checking a single dependency.
#[derive(Debug, Clone)]
pub struct DependencyCheck {
    /// Name of the dependency.
    pub name: &'static str,
    /// Whether the dependency is available.
    pub available: bool,
    /// Path where the dependency was found.
    pub path: Option<PathBuf>,
    /// Whether this dependency is required (vs optional).
    pub required: bool,
}

/// Check all required sandbox dependencies for the current platform.
///
/// Seccomp is handled in-process via `seccompiler` — no external binary
/// dependency needed.
pub fn check_dependencies() -> Vec<DependencyCheck> {
    let mut checks = Vec::new();

    if cfg!(target_os = "macos") {
        checks.push(check_binary(
            "sandbox-exec",
            &["/usr/bin/sandbox-exec"],
            /*required=*/ true,
        ));
    }

    if cfg!(target_os = "linux") {
        checks.push(check_binary(
            "bwrap",
            &["/usr/bin/bwrap", "/usr/local/bin/bwrap"],
            /*required=*/ true,
        ));
        checks.push(check_binary(
            "socat",
            &["/usr/bin/socat", "/usr/local/bin/socat"],
            /*required=*/ false, // Optional: needed for network bridge
        ));
    }

    checks
}

/// Get names of missing required dependencies.
pub fn missing_required() -> Vec<&'static str> {
    check_dependencies()
        .into_iter()
        .filter(|d| d.required && !d.available)
        .map(|d| d.name)
        .collect()
}

/// Check if all required dependencies are available.
pub fn all_required_available() -> bool {
    check_dependencies()
        .iter()
        .filter(|d| d.required)
        .all(|d| d.available)
}

/// Path to the `socat` binary, if installed. Needed to start the Linux netns
/// proxy bridge ([`crate::SandboxState::start_network_proxy_with_bridge`]).
/// Same candidate list as [`check_dependencies`].
pub fn socat_path() -> Option<PathBuf> {
    check_binary(
        "socat",
        &["/usr/bin/socat", "/usr/local/bin/socat"],
        /*required=*/ false,
    )
    .path
}

fn check_binary(name: &'static str, paths: &[&str], required: bool) -> DependencyCheck {
    let found = paths.iter().map(PathBuf::from).find(|p| p.exists());
    DependencyCheck {
        name,
        available: found.is_some(),
        path: found,
        required,
    }
}

#[cfg(test)]
#[path = "deps.test.rs"]
mod tests;
