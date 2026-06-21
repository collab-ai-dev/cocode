//! Linux sandbox implementation using bubblewrap + in-process seccomp.
//!
//! Wraps commands with `bwrap` for namespace isolation (network, PID, IPC, UTS, user)
//! and applies seccomp BPF filters to block sandbox-escaping syscalls.
//!
//! The seccomp filter is compiled at runtime (via `seccompiler`) and passed
//! to the inner stage using the coco binary's `--apply-seccomp` arg0
//! dispatch. This eliminates external binary dependencies.

use std::path::Path;
use std::path::PathBuf;

use tracing::info;
use tracing::warn;

use snafu::OptionExt;

use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::config::WritableRoot;
use crate::error::Result;
use crate::error::sandbox_error::*;
use crate::platform::SandboxPlatform;
use crate::seccomp;

/// Default paths to search for bubblewrap.
const BWRAP_PATHS: &[&str] = &["/usr/bin/bwrap", "/usr/local/bin/bwrap"];

/// Arg1 flag for the seccomp-apply inner stage dispatch.
/// Canonical definition lives in [`crate::inner_stage`].
pub use crate::inner_stage::APPLY_SECCOMP_ARG1;

/// Linux bubblewrap sandbox implementation.
pub struct LinuxSandbox;

impl LinuxSandbox {
    /// Find the bubblewrap binary.
    fn find_bwrap() -> Option<PathBuf> {
        BWRAP_PATHS.iter().map(PathBuf::from).find(|p| p.exists())
    }
}

impl SandboxPlatform for LinuxSandbox {
    fn available(&self) -> bool {
        cfg!(target_os = "linux") && Self::find_bwrap().is_some()
    }

    fn wrap_command(
        &self,
        config: &SandboxConfig,
        _command: &str,
        _session_tag: &str,
        extra_writable_binds: &[PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> Result<()> {
        if config.enforcement == EnforcementLevel::Disabled {
            return Ok(());
        }

        let bwrap_path = Self::find_bwrap().context(PlatformNotAvailableSnafu {
            message: "bubblewrap (bwrap) not found",
        })?;

        let mut bwrap_args = build_bwrap_args(config);
        // Per-command writable binds — shell-executor passes the
        // freshly-allocated tmpdir so the inner command can persist
        // its cwd-tracking file there and the parent can read it.
        // TS parity: `bashProvider.ts:235-247` sandboxTmpDir.
        for path in extra_writable_binds {
            let path_str = path.display().to_string();
            bwrap_args.extend(["--bind".to_string(), path_str.clone(), path_str]);
        }
        let seccomp_mode =
            seccomp::determine_seccomp_mode(config.allow_network, config.proxy_active);

        info!(
            enforcement = ?config.enforcement,
            writable_roots = config.writable_roots.len(),
            allow_network = config.allow_network,
            bwrap_args_count = bwrap_args.len(),
            seccomp_mode = ?seccomp_mode,
            "Wrapping command with Linux bubblewrap sandbox"
        );

        // Extract current program and args
        let inner = cmd.as_std();
        let program = inner.get_program().to_os_string();
        let args: Vec<_> = inner
            .get_args()
            .map(std::ffi::OsStr::to_os_string)
            .collect();

        // Two-stage sandbox pattern:
        //   Stage 1 (outer): bwrap provides namespace isolation
        //   Stage 2 (inner): in-process seccomp via arg0 dispatch
        //
        // Without seccomp:  bwrap [args] -- <program> <args>
        // With seccomp:     bwrap [args] -- <coco> --apply-seccomp <mode> -- <program> <args>
        *cmd = tokio::process::Command::new(&bwrap_path);
        for arg in &bwrap_args {
            cmd.arg(arg);
        }
        cmd.arg("--");

        if let Some(mode) = seccomp_mode {
            // Binary is visible inside bwrap via the read-only root bind.
            // Fallback to "coco" on PATH if current_exe() fails
            // (e.g., /proc not mounted inside bwrap).
            let coco_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("coco"));
            cmd.arg(&coco_exe);
            cmd.arg(APPLY_SECCOMP_ARG1);
            cmd.arg(mode.as_str_arg());
            cmd.arg("--");
        }

        cmd.arg(&program);
        cmd.args(&args);

        Ok(())
    }
}

/// Build bubblewrap arguments from the sandbox configuration.
///
/// Follows codex-rs mount ordering best practices:
/// 1. Safety flags and namespace isolation
/// 2. Base filesystem (read-only root)
/// 3. Minimal device and process filesystems
/// 4. Writable roots (skip missing paths gracefully)
/// 5. Re-apply read-only protections on subpaths
/// 6. Symlink attack masking
/// 7. Extra bind mounts (proxy bridges)
/// 8. Process hardening (clear dangerous env vars)
fn build_bwrap_args(config: &SandboxConfig) -> Vec<String> {
    let mut args = Vec::new();

    // Safety flags (from codex-rs):
    // --new-session: isolate from process group (prevents TIOCSTI escape)
    // --die-with-parent: kill sandbox if parent exits (prevents orphans)
    args.extend_from_slice(&["--new-session".into(), "--die-with-parent".into()]);

    // Namespace isolation
    // --unshare-user: user namespace (required for uid 0 mapping, prevents priv-esc)
    args.push("--unshare-user".into());
    if !config.allow_network {
        args.push("--unshare-net".into());
    }
    args.extend_from_slice(&[
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
    ]);

    // Base filesystem: read-only bind of the entire root
    args.extend_from_slice(&["--ro-bind".into(), "/".into(), "/".into()]);

    // Minimal device filesystem
    args.push("--dev".into());
    args.push("/dev".into());

    // /proc mount: some restrictive container runtimes block this.
    // Adopted from codex-rs: probe first, skip if unavailable.
    if proc_mount_available() {
        args.extend_from_slice(&["--proc".into(), "/proc".into()]);
    } else {
        info!("/proc mount unavailable (restrictive container?); skipping");
    }

    // Writable temp directory
    args.extend_from_slice(&["--tmpfs".into(), "/tmp".into()]);

    // Writable roots — skip missing paths gracefully (codex-rs pattern:
    // allows mixed-platform configs where some paths don't exist).
    for root in &config.writable_roots {
        if !root.path.exists() {
            tracing::debug!(
                path = %root.path.display(),
                "Skipping non-existent writable root"
            );
            continue;
        }
        let root_path = root.path.display().to_string();
        args.extend(["--bind".to_string(), root_path.clone(), root_path]);
        // Re-apply read-only for protected subpaths
        for sub in &root.readonly_subpaths {
            let sub_str = root.path.join(sub).display().to_string();
            args.extend(["--ro-bind-try".to_string(), sub_str.clone(), sub_str]);
        }
    }

    // Symlink attack prevention: mask dangerous symlinks with /dev/null.
    // Uses component-by-component walk (adopted from codex-rs) for deeper
    // detection than just checking immediate children.
    for root in &config.writable_roots {
        for symlink_path in find_attack_symlinks(root) {
            warn!(
                symlink = %symlink_path.display(),
                root = %root.path.display(),
                "Masking symlink in protected subpath to prevent escape attack"
            );
            let path_str = symlink_path.display().to_string();
            args.extend(["--ro-bind".to_string(), "/dev/null".to_string(), path_str]);
        }
    }

    // Extra read-only bind mounts (e.g., proxy bridge UDS sockets)
    for path in &config.extra_bind_ro {
        let path_str = path.display().to_string();
        args.extend(["--ro-bind".to_string(), path_str.clone(), path_str]);
    }

    // Deny-read enforcement on Linux is implemented by mounting /dev/null
    // over each denied path — same trick codex-rs uses for symlink attack
    // mitigation. Globs are expanded under the writable roots first
    // (bounded by `glob_scan_max_depth`).
    //
    // `allow_read` carve-outs: bwrap can't do precision per-file deny
    // within a directory bind, so if an allow_read path is the same as or
    // a descendant of a deny path, we skip the deny (the broader allow
    // wins). This matches the user intent ("re-allow this subtree") at the
    // cost of also exposing siblings of the allow path. macOS Seatbelt
    // gets precise carve-outs via rule order — Linux trades a strict
    // narrow allow for a loose broader allow.
    let writable_root_paths: Vec<PathBuf> = config
        .writable_roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let glob_expanded = crate::glob_expansion::expand(
        &writable_root_paths,
        &config.denied_read_globs,
        config.glob_scan_max_depth.max(0) as usize,
    );
    let allow_read_overrides_deny = |deny_path: &Path| -> bool {
        config
            .allowed_read_paths
            .iter()
            .any(|allow| allow.starts_with(deny_path))
    };
    for path in config
        .denied_read_paths
        .iter()
        .chain(&config.denied_paths)
        .chain(&glob_expanded)
    {
        if !path.exists() {
            continue;
        }
        if allow_read_overrides_deny(path) {
            tracing::debug!(
                deny = %path.display(),
                "Skipping deny-read on Linux: allow_read carves out a descendant; \
                 broader allow wins because bwrap can't precision-deny"
            );
            continue;
        }
        let path_str = path.display().to_string();
        args.extend([
            "--ro-bind-try".to_string(),
            "/dev/null".to_string(),
            path_str,
        ]);
    }

    // Process hardening: clear dangerous env vars
    for var in &["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT"] {
        args.extend(["--unsetenv".to_string(), var.to_string()]);
    }

    // Set CWD to the first existing writable root (preserves symlink aliases)
    if let Some(first_root) = config.writable_roots.iter().find(|r| r.path.exists()) {
        let cwd_str = first_root.path.display().to_string();
        args.extend_from_slice(&["--chdir".into(), cwd_str]);
    }

    args
}

/// Probe whether `--proc /proc` is available in the current environment.
///
/// Some restrictive container runtimes (Docker with `--security-opt=no-new-privileges`,
/// certain Kubernetes pod security policies) block the proc mount. Adopted from
/// codex-rs's preflight detection: run a minimal bwrap command and check if it
/// succeeds. The result is cached per-process via `OnceLock` (not safe across
/// fork — forked children inherit the parent's cached result).
fn proc_mount_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();

    *AVAILABLE.get_or_init(|| {
        let bwrap = match LinuxSandbox::find_bwrap() {
            Some(p) => p,
            None => return false,
        };
        // Minimal probe: bind / read-only, mount /proc, run /bin/true
        let result = std::process::Command::new(&bwrap)
            .args([
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--unshare-user",
                "--unshare-pid",
                "--",
                "/bin/true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    })
}

/// Find symlinks within protected subpaths that could be used for escape attacks.
///
/// Walks each protected subpath component-by-component (adopted from codex-rs)
/// to detect symlinks at any depth, not just immediate children. A sandboxed
/// process could replace a file in a writable area with a symlink pointing
/// outside the sandbox. Detecting these early lets us mask them with `/dev/null`.
///
/// Returns paths that should be masked with /dev/null.
fn find_attack_symlinks(root: &WritableRoot) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut symlinks = Vec::new();

    for sub in &root.readonly_subpaths {
        let sub_path = root.path.join(sub);

        // Component-by-component walk from root to protected subpath.
        // Detects symlinks at any level, not just the leaf or immediate children.
        // Non-existent paths are skipped (they'll be protected by --ro-bind-try).
        let mut current = root.path.clone();
        for component in std::path::Path::new(sub).components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    if seen.insert(current.clone()) {
                        symlinks.push(current.clone());
                    }
                    break;
                }
                Err(_) => break,
                _ => {}
            }
        }

        // Also check immediate children of the protected subpath directory
        if sub_path.is_dir()
            && let Ok(entries) = std::fs::read_dir(&sub_path)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_symlink() && seen.insert(path.clone()) {
                    symlinks.push(path);
                }
            }
        }
    }
    symlinks
}

#[cfg(test)]
#[path = "linux.test.rs"]
mod tests;
