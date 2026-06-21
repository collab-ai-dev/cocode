//! macOS sandbox implementation using Seatbelt (sandbox-exec).
//!
//! Generates SBPL (Seatbelt Profile Language) profiles at runtime based on
//! the sandbox configuration, then wraps commands with `sandbox-exec`.
//! Uses a static base policy (Chrome-inspired, from codex-rs) plus dynamic
//! path rules for writable roots and network access.

use tracing::info;

use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::error::Result;
use crate::monitor::generate_command_tag;
use crate::platform::SandboxPlatform;

/// Path to the system sandbox-exec binary.
/// Hardcoded to /usr/bin to defend against PATH injection.
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Static base policy: process, sysctl, iokit, mach, pty rules.
/// Inspired by Chrome's sandbox policy and codex-rs seatbelt_base_policy.sbpl.
const SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base.sbpl");

/// Static network policy: TLS services, DNS, AF_SYSTEM sockets.
const SEATBELT_NETWORK_POLICY: &str = include_str!("seatbelt_network.sbpl");

/// macOS Seatbelt sandbox implementation.
///
/// Wraps commands with `sandbox-exec -p <profile>` using a static base
/// policy combined with dynamically generated path rules.
pub struct MacOsSandbox;

impl SandboxPlatform for MacOsSandbox {
    fn available(&self) -> bool {
        cfg!(target_os = "macos") && std::path::Path::new(SANDBOX_EXEC_PATH).exists()
    }

    fn wrap_command(
        &self,
        config: &SandboxConfig,
        command: &str,
        session_tag: &str,
        extra_writable_binds: &[std::path::PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> Result<()> {
        if config.enforcement == EnforcementLevel::Disabled {
            return Ok(());
        }

        let profile = generate_seatbelt_profile(config, command, session_tag, extra_writable_binds);

        info!(
            enforcement = ?config.enforcement,
            writable_roots = config.writable_roots.len(),
            allow_network = config.allow_network,
            profile_len = profile.len(),
            "Wrapping command with macOS Seatbelt sandbox"
        );

        let inner = cmd.as_std();
        let program = inner.get_program().to_os_string();
        let args: Vec<_> = inner
            .get_args()
            .map(std::ffi::OsStr::to_os_string)
            .collect();

        // Rebuild as: sandbox-exec -p <profile> <original_program> <original_args...>
        *cmd = tokio::process::Command::new(SANDBOX_EXEC_PATH);
        cmd.arg("-p").arg(&profile);
        cmd.arg(&program);
        cmd.args(&args);

        // Process hardening: clear dangerous env vars
        cmd.env_remove("DYLD_INSERT_LIBRARIES");
        cmd.env_remove("DYLD_LIBRARY_PATH");
        cmd.env_remove("DYLD_FRAMEWORK_PATH");

        Ok(())
    }
}

/// Escape a path for safe inclusion in SBPL string literals.
///
/// SBPL uses Lisp-style strings where backslash and double-quote are special.
/// Newlines are stripped to prevent rule injection via multi-line paths.
fn escape_sbpl_path(path: &str) -> String {
    path.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], "")
}

/// Generate a complete Seatbelt profile from static base + dynamic paths.
///
/// Each invocation produces a unique profile with a `CMD64_` command tag
/// embedded in the `(deny default (with message ...))` rule, enabling
/// macOS violation log correlation back to the specific command.
fn generate_seatbelt_profile(
    config: &SandboxConfig,
    command: &str,
    session_tag: &str,
    extra_writable_binds: &[std::path::PathBuf],
) -> String {
    use std::fmt::Write;

    let mut profile = String::with_capacity(8192);
    let home = dirs_home();

    // Version header + deny-default with command tag for violation correlation.
    // The tag is embedded in the deny message so macOS log stream entries
    // include it, allowing the ViolationMonitor to correlate to specific commands.
    let command_tag = generate_command_tag(command, session_tag);
    profile.push_str("(version 1)\n");
    let _ = writeln!(profile, "(deny default (with message \"{command_tag}\"))");
    let _ = write!(profile, "; LogTag: {command_tag}\n\n");

    // Static base policy (process, sysctl, iokit, mach) — excludes version
    // header (generated above with command tag) and PTY (conditional below).
    profile.push_str(SEATBELT_BASE_POLICY);
    profile.push('\n');

    // Conditional PTY support (Claude Code gates this via allow_pty config).
    // Pattern from codex-rs: separate ptmx (literal) from ttys (regex with [0-9]+).
    if config.allow_pty {
        profile.push_str("; PTY support (enabled by config)\n");
        profile.push_str("(allow pseudo-tty)\n");
        profile.push_str("(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\"))\n");
        profile.push_str("(allow file-read* file-write* (regex #\"^/dev/ttys[0-9]+\"))\n");
        profile.push_str("(allow file-ioctl (regex #\"^/dev/ttys[0-9]+\"))\n");
    } else {
        profile.push_str("; PTY access denied by config\n");
        profile.push_str("(deny pseudo-tty)\n");
    }
    profile.push('\n');

    // System read access (always allowed for basic operation)
    profile.push_str("; System read access\n");
    for path in &[
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/System/Library",
        "/Library/Frameworks",
        "/dev/null",
        "/dev/urandom",
        "/dev/random",
        "/private/var/tmp",
        "/private/tmp",
        "/etc",
    ] {
        let _ = writeln!(profile, "(allow file-read* (subpath \"{path}\"))");
    }

    // Home directory read access
    if let Some(h) = &home {
        let escaped = escape_sbpl_path(h);
        let _ = writeln!(profile, "(allow file-read* (subpath \"{escaped}\"))");
    }
    profile.push('\n');

    // Denied read paths (from config.denied_read_paths + config.denied_paths
    // + glob-expanded `denied_read_globs`).
    //
    // Seatbelt evaluates filter rules bottom-to-top and applies the first
    // matching rule, so emitting `allow_read` carve-outs **after** the deny
    // block lets a more-specific allow win for matching paths while broader
    // deny rules still cover their non-allowed siblings. Matches the TS
    // shape of `entrypoints/sandboxTypes.ts:71-77` where `allowRead`
    // takes precedence over `denyRead` for matching paths.
    let writable_root_paths: Vec<std::path::PathBuf> = config
        .writable_roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let glob_expanded = crate::glob_expansion::expand(
        &writable_root_paths,
        &config.denied_read_globs,
        config.glob_scan_max_depth.max(0) as usize,
    );
    let has_denied_reads = !config.denied_read_paths.is_empty()
        || !config.denied_paths.is_empty()
        || !glob_expanded.is_empty();
    if has_denied_reads {
        profile.push_str("; Explicitly denied read paths\n");
        for path in config
            .denied_read_paths
            .iter()
            .chain(&config.denied_paths)
            .chain(&glob_expanded)
        {
            let escaped = escape_sbpl_path(&path.display().to_string());
            let _ = writeln!(profile, "(deny file-read* (subpath \"{escaped}\"))");
        }
        profile.push('\n');
    }

    // Re-allow reads carved out of the deny regions above. Emitted AFTER
    // the deny block so Seatbelt's last-match-wins semantics let these
    // override the matching deny entries.
    if !config.allowed_read_paths.is_empty() {
        profile.push_str("; allow_read carve-outs (override matching denies)\n");
        for path in &config.allowed_read_paths {
            let escaped = escape_sbpl_path(&path.display().to_string());
            let _ = writeln!(profile, "(allow file-read* (subpath \"{escaped}\"))");
        }
        profile.push('\n');
    }

    // Standard output and device file writes (from Claude Code kx6)
    profile.push_str("; Standard output and device file writes\n");
    for dev in &[
        "/dev/null",
        "/dev/stdout",
        "/dev/stderr",
        "/dev/tty",
        "/dev/dtracehelper",
        "/dev/autofs_nowait",
    ] {
        let _ = writeln!(profile, "(allow file-write* (literal \"{dev}\"))");
    }
    // Branded temp dirs
    profile.push_str("(allow file-write* (subpath \"/tmp/coco\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/tmp/coco\"))\n");
    // Home-relative dirs for logging
    if let Some(h) = &home {
        let escaped = escape_sbpl_path(h);
        let _ = writeln!(
            profile,
            "(allow file-write* (subpath \"{escaped}/.npm/_logs\"))"
        );
        let _ = writeln!(
            profile,
            "(allow file-write* (subpath \"{escaped}/.coco/debug\"))"
        );
    }
    profile.push('\n');

    // Writable roots with subpath protection
    if !config.writable_roots.is_empty() {
        profile.push_str("; Writable roots\n");
        for root in &config.writable_roots {
            let path = escape_sbpl_path(&root.path.display().to_string());
            let _ = writeln!(profile, "(allow file-write* (subpath \"{path}\"))");
            // Re-deny write access to read-only subpaths
            for sub in &root.readonly_subpaths {
                let sub_path = root.path.join(sub);
                let escaped = escape_sbpl_path(&sub_path.display().to_string());
                let _ = writeln!(profile, "(deny file-write* (subpath \"{escaped}\"))");
            }
        }
        profile.push('\n');
    }

    // Temp directory write access with /private variant generation
    profile.push_str("; Temp directory write access\n");
    profile.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    for tmpdir in tmpdir_variants() {
        let escaped = escape_sbpl_path(&tmpdir);
        let _ = writeln!(profile, "(allow file-write* (subpath \"{escaped}\"))");
    }
    // Per-command writable binds — shell-executor's allocated tmpdir
    // for cwd-file tracking and `TMPDIR` injection. Seatbelt treats
    // these as additional writable subpaths; bwrap on Linux binds them.
    for path in extra_writable_binds {
        let escaped = escape_sbpl_path(&path.display().to_string());
        let _ = writeln!(profile, "(allow file-write* (subpath \"{escaped}\"))");
    }
    profile.push('\n');

    // Network access
    profile.push_str("; Network access\n");
    if config.allow_network {
        profile.push_str("(allow network*)\n");
        // Include TLS and DNS services policy for full network access
        profile.push_str(SEATBELT_NETWORK_POLICY);
        // macOS cache dir for TLS/DNS caching
        let cache_dir = std::env::var("DARWIN_USER_CACHE_DIR")
            .ok()
            .or_else(|| home.as_ref().map(|h| format!("{h}/Library/Caches")));
        if let Some(cache_dir) = cache_dir {
            let escaped = escape_sbpl_path(&cache_dir);
            let _ = writeln!(profile, "(allow file-write* (subpath \"{escaped}\"))");
        }
    } else {
        // Allow loopback only (for proxy connections)
        profile.push_str("(allow network* (local ip \"localhost:*\"))\n");
        profile.push_str("(allow network* (remote ip \"localhost:*\"))\n");
    }

    // Weaker network isolation: allow trustd.agent for Go TLS cert verification.
    // Go programs (gh, gcloud, terraform, kubectl) need system TLS services
    // because they don't use Apple's SecureTransport for certificate verification.
    if config.weaker_network_isolation {
        profile.push_str("; Weaker network isolation: Go TLS cert verification\n");
        profile.push_str("(allow mach-lookup (global-name \"com.apple.trustd.agent\"))\n");
    }

    profile
}

/// Get TMPDIR variants with both `/private/var/` and `/var/` paths.
///
/// macOS uses paths like `/private/var/folders/xx/.../T/` for TMPDIR.
/// Some tools resolve the `/private` prefix, others don't, so we
/// generate both variants to avoid sandbox violations.
fn tmpdir_variants() -> Vec<String> {
    let Ok(tmpdir) = std::env::var("TMPDIR") else {
        return vec![];
    };
    let mut dirs = vec![tmpdir.clone()];
    // Generate /private and non-/private variants.
    // macOS uses /private/var/folders/xx/.../T/ — some tools resolve
    // the /private prefix, others don't, so we need both.
    if let Some(rest) = tmpdir.strip_prefix("/private/var/") {
        if !rest.is_empty() {
            dirs.push(format!("/var/{rest}"));
        }
        // Also allow the parent dir (e.g., .../T → .../xx/...)
        if let Some(parent) = std::path::Path::new(&tmpdir).parent() {
            let parent_str = parent.display().to_string();
            if !parent_str.is_empty() && parent_str != "/" {
                dirs.push(parent_str.clone());
                if let Some(stripped) = parent_str.strip_prefix("/private")
                    && !stripped.is_empty()
                {
                    dirs.push(stripped.to_string());
                }
            }
        }
    } else if let Some(rest) = tmpdir.strip_prefix("/var/")
        && !rest.is_empty()
    {
        dirs.push(format!("/private/var/{rest}"));
    }
    dirs
}

/// Get the user's home directory, if available.
fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok()
}

#[cfg(test)]
#[path = "macos.test.rs"]
mod tests;
