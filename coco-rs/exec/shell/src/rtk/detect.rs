//! Binary detection + version gate for the RTK subprocess tier.
//!
//! Resolution order (design §3.1, §4.1): explicit `rtk.binary_path` →
//! `which("rtk")` → `which("rr-rtk")`. The result is cached for the session in
//! the [`super::RtkRewriter`] `OnceCell`; this module runs at most once.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use coco_config::RtkConfig;
use tokio::process::Command;

#[cfg(test)]
#[path = "detect.test.rs"]
mod tests;

/// Minimum rtk version exposing the stable `rtk rewrite` contract (§1.4).
pub(crate) const MIN_VERSION: RtkVersion = RtkVersion {
    major: 0,
    minor: 23,
    patch: 0,
};

/// Time budget for the one-shot `--version` probe. Independent of the
/// per-command rewrite timeout — a hung binary at detection must not wedge the
/// first Bash call either.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_millis(2000);

/// Which binary backs the rewrite. `RrRtk` triggers the §4.5 prefix fixup
/// because its engine still emits hardcoded `rtk ` prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtkFlavor {
    Rtk,
    RrRtk,
}

/// A parsed `major.minor.patch` triple. Pre-release tags (`0.42.3-rr.2`)
/// compare by their base triple — the suffix is dropped during parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RtkVersion {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
}

/// A usable rtk binary discovered on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtkBinary {
    pub path: PathBuf,
    pub flavor: RtkFlavor,
    pub version: RtkVersion,
}

/// Outcome of the one-shot session probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RtkProbe {
    Found(RtkBinary),
    Missing,
    VersionTooOld,
}

/// Probe for a usable rtk binary. Emits exactly one session-scoped `info!`
/// describing the outcome (§6).
pub(crate) async fn probe(config: &RtkConfig) -> RtkProbe {
    let Some((path, flavor)) = resolve_path(config) else {
        tracing::info!("rtk: no binary on PATH; Bash output compression inactive this session");
        return RtkProbe::Missing;
    };

    let Some(version) = query_version(&path).await else {
        tracing::info!(
            path = %path.display(),
            "rtk: `--version` probe failed; Bash output compression inactive this session"
        );
        return RtkProbe::Missing;
    };

    if version < MIN_VERSION {
        tracing::info!(
            path = %path.display(),
            version = ?version,
            "rtk: binary older than {MIN_VERSION:?}; Bash output compression inactive this session"
        );
        return RtkProbe::VersionTooOld;
    }

    tracing::info!(
        path = %path.display(),
        flavor = ?flavor,
        version = ?version,
        "rtk: detected — Bash output compression active this session"
    );
    RtkProbe::Found(RtkBinary {
        path,
        flavor,
        version,
    })
}

/// Resolve the binary path + flavor. `which` is a quick PATH scan + stat; it
/// runs once per session so a direct blocking call is acceptable.
fn resolve_path(config: &RtkConfig) -> Option<(PathBuf, RtkFlavor)> {
    if let Some(configured) = &config.binary_path {
        let path = PathBuf::from(configured);
        let flavor = flavor_from_path(&path);
        return Some((path, flavor));
    }
    if let Ok(path) = which::which("rtk") {
        return Some((path, RtkFlavor::Rtk));
    }
    if let Ok(path) = which::which("rr-rtk") {
        return Some((path, RtkFlavor::RrRtk));
    }
    None
}

/// Infer flavor from a binary path's file stem — an `rr-rtk` binary emits
/// `rtk `-prefixed rewrites that need the §4.5 fixup.
fn flavor_from_path(path: &Path) -> RtkFlavor {
    match path.file_stem().and_then(|s| s.to_str()) {
        Some("rr-rtk") => RtkFlavor::RrRtk,
        _ => RtkFlavor::Rtk,
    }
}

async fn query_version(path: &Path) -> Option<RtkVersion> {
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    let output = tokio::time::timeout(VERSION_PROBE_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_version(&text)
}

/// Extract the first `major.minor.patch` triple from `--version` text
/// (`rtk 0.42.4`, `rr-rtk 0.42.3-rr.2`, …). Pre-release suffixes are ignored.
pub(crate) fn parse_version(text: &str) -> Option<RtkVersion> {
    for token in text.split(|c: char| c.is_whitespace() || c == 'v') {
        // Strip a leading `v` and any pre-release/build suffix (`-rr.2`, `+x`).
        let core = token.trim_start_matches('v');
        let core = core.split(['-', '+']).next().unwrap_or(core);
        let mut parts = core.split('.');
        let major = parts.next()?.parse::<i64>().ok();
        let minor = parts.next().and_then(|p| p.parse::<i64>().ok());
        let patch = parts.next().and_then(|p| p.parse::<i64>().ok());
        if let (Some(major), Some(minor), Some(patch)) = (major, minor, patch) {
            // Reject trailing junk (`1.2.3.4` or `1.2.3abc`).
            if parts.next().is_none() {
                return Some(RtkVersion {
                    major,
                    minor,
                    patch,
                });
            }
        }
    }
    None
}
