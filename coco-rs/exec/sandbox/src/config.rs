//! Sandbox runtime configuration types.
//!
//! The user-facing settings type ([`SandboxSettings`]) and its inner config
//! types ([`FilesystemConfig`], [`NetworkConfig`], [`SandboxBypass`], etc.)
//! live in `coco-config` so settings.json deserialization and the sandbox
//! runtime share a single source of truth. This module re-exports them for ergonomic
//! callsite access (`coco_sandbox::SandboxSettings`).
//!
//! What stays here is the **runtime/adapter output**:
//! - [`SandboxConfig`] — what platform wrappers (Seatbelt/bwrap) actually
//!   consume after the adapter resolves rules + paths.
//! - [`EnforcementLevel`] — runtime enforcement posture.
//! - [`WritableRoot`] — writable directory + read-only subpath protections,
//!   with the `.git`-pointer-file detection used by worktrees/submodules.

use std::path::Path;
use std::path::PathBuf;

use coco_types::SandboxMode;
use serde::Deserialize;
use serde::Serialize;

// Re-export the user-facing settings types so existing callsites that import
// from `coco_sandbox::*` keep compiling. The canonical location is
// `coco_config::sandbox_settings`.
pub use coco_config::FilesystemConfig;
pub use coco_config::IgnoreViolationsConfig;
pub use coco_config::MitmProxyConfig;
pub use coco_config::NetworkConfig;
pub use coco_config::NetworkMode;
pub use coco_config::RipgrepConfig;
pub use coco_config::SandboxBypass;
pub use coco_config::SandboxSettings;

/// Sandbox enforcement level controlling filesystem and network access.
///
/// Distinct from [`SandboxMode`] which represents the user's intent
/// (ReadOnly/WorkspaceWrite/FullAccess). This enum represents the actual
/// enforcement behavior applied at runtime.
///
/// Convert from user-facing mode via `EnforcementLevel::from(sandbox_mode)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementLevel {
    /// No sandbox enforcement; all operations are allowed.
    #[default]
    Disabled,
    /// Read-only mode; file writes are blocked.
    ReadOnly,
    /// Workspace-write mode; writes allowed to configured writable roots.
    WorkspaceWrite,
    /// Strict mode; only explicitly allowed paths are accessible,
    /// and network is blocked unless explicitly allowed.
    Strict,
}

impl From<SandboxMode> for EnforcementLevel {
    fn from(mode: SandboxMode) -> Self {
        match mode {
            SandboxMode::ReadOnly => Self::ReadOnly,
            SandboxMode::WorkspaceWrite => Self::WorkspaceWrite,
            SandboxMode::FullAccess => Self::Disabled,
            // ExternalSandbox: workspace-write enforcement for permission checks,
            // but platform wrapping (bwrap/Seatbelt) is skipped by SandboxState.
            SandboxMode::ExternalSandbox => Self::WorkspaceWrite,
        }
    }
}

/// A writable root directory with read-only subpath protections.
///
/// Certain subpaths (e.g., `.git`, `.coco`) remain read-only even when
/// the parent directory is writable. This prevents the agent from modifying
/// version control or configuration state within otherwise writable projects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WritableRoot {
    /// The root directory that is writable.
    pub path: PathBuf,
    /// Subpaths that remain read-only even under this writable root.
    #[serde(default = "default_readonly_subpaths")]
    pub readonly_subpaths: Vec<String>,
}

impl WritableRoot {
    /// Creates a writable root with default read-only subpaths.
    ///
    /// If `.git` is a pointer file (git worktrees/submodules), the actual
    /// gitdir is also added to the read-only subpaths (adopted from codex-rs).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut subpaths = default_readonly_subpaths();

        // Detect git pointer files: in worktrees and submodules, `.git` is a
        // file containing `gitdir: <path>`. The actual gitdir must also be
        // protected as read-only (codex-rs pattern).
        let git_path = path.join(".git");
        if git_path.is_file()
            && let Some(gitdir) = resolve_git_pointer(&git_path)
        {
            // `resolve_git_pointer` canonicalizes the gitdir, so canonicalize
            // the writable root too — otherwise on macOS the
            // `/var/folders/...` symlink prefix won't match the resolved
            // `/private/var/folders/...` and `strip_prefix` fails.
            let canonical_root = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            match gitdir.strip_prefix(&canonical_root) {
                Ok(rel) => {
                    let rel_str = rel.display().to_string();
                    if !subpaths.contains(&rel_str) {
                        subpaths.push(rel_str);
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        gitdir = %gitdir.display(),
                        root = %path.display(),
                        "Git pointer resolves outside writable root; cannot protect"
                    );
                }
            }
        }

        Self {
            path,
            readonly_subpaths: subpaths,
        }
    }

    /// Creates a writable root with no read-only subpath protections.
    pub fn unprotected(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            readonly_subpaths: Vec::new(),
        }
    }

    /// Check if a path is writable under this root.
    ///
    /// Returns `true` if the path is under this root AND not under any
    /// of the read-only subpaths.
    pub fn is_writable(&self, path: &Path) -> bool {
        path.starts_with(&self.path)
            && !self
                .readonly_subpaths
                .iter()
                .any(|sub| path.starts_with(self.path.join(sub)))
    }

    /// Check if a path is under this root (regardless of write permission).
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.path)
    }

    /// Resolve read-only subpaths to absolute paths (root + subpath).
    ///
    /// Used by platform enforcement (bwrap, Seatbelt) to generate
    /// mount/deny rules with full paths.
    pub fn resolved_readonly_subpaths(&self) -> Vec<PathBuf> {
        self.readonly_subpaths
            .iter()
            .map(|sub| self.path.join(sub))
            .collect()
    }
}

/// Resolve a `.git` pointer file to the actual gitdir path.
///
/// Git worktrees and submodules use a `.git` file (not directory) containing
/// `gitdir: <path>`. Returns the resolved absolute path to the actual gitdir,
/// or `None` if the file isn't a valid pointer.
fn resolve_git_pointer(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    // Extract first line only — pointer files have `gitdir: <path>` on line 1.
    let first_line = content.lines().next()?.trim();
    let gitdir = first_line.strip_prefix("gitdir:")?.trim();
    if gitdir.is_empty() {
        return None;
    }
    let gitdir_path = PathBuf::from(gitdir);
    let resolved = if gitdir_path.is_relative() {
        git_file.parent()?.join(&gitdir_path)
    } else {
        gitdir_path
    };
    match std::fs::canonicalize(&resolved) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::debug!(path = %resolved.display(), error = %e, "Failed to resolve gitdir");
            None
        }
    }
}

fn default_readonly_subpaths() -> Vec<String> {
    vec![
        ".git".to_string(),
        ".coco".to_string(),
        ".agents".to_string(),
    ]
}

/// Configuration for the sandbox runtime (adapter output).
///
/// This is the resolved type that platform wrappers consume — distinct from
/// the user-facing [`SandboxSettings`] (re-exported from `coco-config`).
/// `SandboxSettings` describes "what the user wrote in settings.json" while
/// `SandboxConfig` describes "what the kernel-level enforcer will apply"
/// after the adapter folds in permission rules, glob expansion, worktree
/// detection, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// The sandbox enforcement level.
    #[serde(default)]
    pub enforcement: EnforcementLevel,
    /// Writable root directories with subpath protections.
    #[serde(default)]
    pub writable_roots: Vec<WritableRoot>,
    /// Paths that are explicitly denied for both read and write (takes precedence).
    #[serde(default)]
    pub denied_paths: Vec<PathBuf>,
    /// Paths explicitly denied for reading.
    #[serde(default)]
    pub denied_read_paths: Vec<PathBuf>,
    /// Glob patterns explicitly denied for reading. Expanded at wrap time
    /// against [`writable_roots`] using
    /// [`crate::glob_expansion::expand`], bounded by
    /// [`SandboxSettings::mandatory_deny_search_depth`]. Mirrors the
    /// codex-rs `glob_scan_max_depth` behavior — TS delegates this to
    /// `@anthropic-ai/sandbox-runtime` calling `sandbox.ripgrep.command`.
    #[serde(default)]
    pub denied_read_globs: Vec<String>,
    /// Maximum directory walk depth when expanding `denied_read_globs`.
    /// Defaults to 3 (matches `SandboxSettings::default`); platform
    /// wrappers carry this in `SandboxConfig` so a hot-reload of
    /// settings reaches the platform without an out-of-band channel.
    #[serde(default = "default_glob_scan_max_depth")]
    pub glob_scan_max_depth: i32,
    /// Paths to re-allow reading even when shadowed by `denied_read_paths`
    /// or a permission `Read(/foo)` deny rule. TS parity:
    /// `entrypoints/sandboxTypes.ts:71-77` — Seatbelt evaluates rules in
    /// order so allow rules emitted *after* deny rules win for matching
    /// paths; on Linux bwrap, an overlapping `allow_read` causes the
    /// matching deny to be skipped (bwrap can't precision carve-out a
    /// subtree, so we trade the broader deny for the narrower allow).
    #[serde(default)]
    pub allowed_read_paths: Vec<PathBuf>,
    /// Paths explicitly denied for writing (in addition to `denied_paths`).
    #[serde(default)]
    pub deny_write_paths: Vec<PathBuf>,
    /// Whether git config files are writable (`.git/config`, `~/.gitconfig`).
    #[serde(default)]
    pub allow_git_config: bool,
    /// Whether network access is allowed.
    #[serde(default)]
    pub allow_network: bool,
    /// Whether the network proxy is active (runtime-only, not persisted).
    ///
    /// Controls seccomp mode selection: `ProxyRouted` when true, `Restricted` when false.
    /// Synced from `SandboxState::network_active()` in the `config()` snapshot method.
    #[serde(skip)]
    pub proxy_active: bool,
    /// Paths to bind-mount into the sandbox (e.g., proxy bridge UDS sockets).
    #[serde(default)]
    pub extra_bind_ro: Vec<PathBuf>,
    /// Allow `com.apple.trustd.agent` mach lookup for Go TLS cert verification.
    ///
    /// Required for Go programs (gh, gcloud, terraform, kubectl) that verify
    /// TLS certificates through macOS system services rather than bundled CAs.
    #[serde(default)]
    pub weaker_network_isolation: bool,
    /// Allow pseudo-terminal access inside the sandbox (macOS).
    ///
    /// Defaults to `true`. When `false`, PTY rules are excluded from the
    /// Seatbelt profile, preventing sandboxed commands from allocating TTYs.
    #[serde(default = "default_true")]
    pub allow_pty: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enforcement: EnforcementLevel::default(),
            writable_roots: Vec::new(),
            denied_paths: Vec::new(),
            denied_read_paths: Vec::new(),
            denied_read_globs: Vec::new(),
            glob_scan_max_depth: default_glob_scan_max_depth(),
            allowed_read_paths: Vec::new(),
            deny_write_paths: Vec::new(),
            allow_git_config: false,
            allow_network: false,
            proxy_active: false,
            extra_bind_ro: Vec::new(),
            weaker_network_isolation: false,
            // Mirrors the `#[serde(default = "default_true")]` attribute so
            // `..Default::default()` matches the deserialized default.
            allow_pty: true,
        }
    }
}

fn default_glob_scan_max_depth() -> i32 {
    3
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
