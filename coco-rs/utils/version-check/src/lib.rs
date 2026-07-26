//! Best-effort background check for a newer released coco.
//!
//! Three properties shape the whole design, and each is a decision rather than
//! an implementation detail:
//!
//! - **It never blocks a session.** The check is a network call to a third party
//!   that can hang, rate-limit, or be firewalled. Startup reads a cached answer
//!   from the previous run — an instant file read — and refreshes the cache in
//!   the background. A user whose network drops the registry never waits.
//! - **It never nags.** One cached answer per interval, and a version the user
//!   dismissed stays dismissed. The failure mode of update prompts is not being
//!   wrong, it is being repetitive.
//! - **It tells the truth about upgrading.** A user who installed via Homebrew
//!   cannot fix anything with an `npm install` line. The upgrade command is
//!   derived from how this binary was actually installed, and where that cannot
//!   be determined the notice says a new version exists without inventing a
//!   command that would fail.
//!
//! Tier-2 leaf utility (`thiserror`, no `coco-error`).

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

mod install;
mod version;

pub use install::InstallMethod;
pub use version::is_newer;

/// npm dist-tag endpoint for the published CLI. Returns the `latest` manifest,
/// which is a few hundred bytes — not the full packument.
const NPM_LATEST_URL: &str = "https://registry.npmjs.org/@cocode-cli/cocode-cli/latest";

/// How long a cached answer stays fresh. A day: releases are not frequent
/// enough for a tighter interval to inform anyone, and a looser one makes the
/// notice arrive long after the release.
const CACHE_TTL: Duration = Duration::from_secs(20 * 60 * 60);

/// Bound on the whole registry request. Generous enough for a slow link, short
/// enough that a black-holed connection does not keep a task alive for the
/// length of the session.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const CACHE_FILENAME: &str = "version-check.json";

#[derive(Debug, thiserror::Error)]
pub enum VersionCheckError {
    #[error("version check request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("version cache io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("version cache is not valid json: {0}")]
    Decode(#[from] serde_json::Error),
}

/// What the last check learned. Persisted so a session can answer from disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCache {
    pub latest_version: String,
    pub last_checked_at: DateTime<Utc>,
    /// The version the user asked not to be told about again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_version: Option<String>,
}

impl VersionCache {
    fn is_stale(&self, now: DateTime<Utc>) -> bool {
        let Ok(ttl) = chrono::Duration::from_std(CACHE_TTL) else {
            return true;
        };
        self.last_checked_at + ttl <= now
    }
}

/// A newer version worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeNotice {
    pub current_version: String,
    pub latest_version: String,
    /// The command that upgrades *this* installation, or `None` when the
    /// install method is unknown.
    pub upgrade_command: Option<String>,
}

/// Default cache location under the coco home.
pub fn default_cache_path() -> PathBuf {
    coco_utils_common::find_coco_home().join(CACHE_FILENAME)
}

/// The upgrade notice implied by a cache, if any.
///
/// Pure: no io, no clock. The caller supplies the running version so the same
/// logic covers "should the banner show" and the tests.
pub fn notice_from_cache(cache: &VersionCache, current_version: &str) -> Option<UpgradeNotice> {
    if cache.dismissed_version.as_deref() == Some(cache.latest_version.as_str()) {
        return None;
    }
    if !is_newer(&cache.latest_version, current_version) {
        return None;
    }
    Some(UpgradeNotice {
        current_version: current_version.to_string(),
        latest_version: cache.latest_version.clone(),
        upgrade_command: InstallMethod::detect().upgrade_command(),
    })
}

/// Read the cached answer. `None` when there is no cache or it is unreadable —
/// a corrupt cache is not worth surfacing, the next refresh replaces it.
pub fn read_cache(path: &Path) -> Option<VersionCache> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Whether a refresh should run now.
///
/// A source build is excluded outright: it has no released version to compare
/// against, and telling a developer running their own build that they are
/// "behind" is noise.
pub fn should_refresh(
    cache: Option<&VersionCache>,
    current_version: &str,
    now: DateTime<Utc>,
) -> bool {
    if version::is_source_build(current_version) {
        return false;
    }
    cache.is_none_or(|cache| cache.is_stale(now))
}

/// Fetch the latest published version and rewrite the cache.
///
/// Preserves any dismissal already recorded, so a refresh cannot resurrect a
/// notice the user dismissed for a version that is still the latest.
pub async fn refresh_cache(
    path: &Path,
    now: DateTime<Utc>,
) -> Result<VersionCache, VersionCheckError> {
    refresh_cache_from(path, NPM_LATEST_URL, now).await
}

/// [`refresh_cache`] against an explicit endpoint. The cache is written only
/// after a successful fetch, so a failing registry can never replace a good
/// answer with a bad one.
pub async fn refresh_cache_from(
    path: &Path,
    url: &str,
    now: DateTime<Utc>,
) -> Result<VersionCache, VersionCheckError> {
    let latest_version = fetch_latest_version(url).await?;
    let cache = VersionCache {
        dismissed_version: read_cache(path).and_then(|prev| prev.dismissed_version),
        latest_version,
        last_checked_at: now,
    };
    write_cache(path, &cache).await?;
    Ok(cache)
}

/// Record that the user does not want to hear about `version` again.
pub async fn dismiss_version(path: &Path, version: &str) -> Result<(), VersionCheckError> {
    let cache = match read_cache(path) {
        Some(cache) => VersionCache {
            dismissed_version: Some(version.to_string()),
            ..cache
        },
        // No cache to amend: record the dismissal against an epoch timestamp so
        // the next startup still refreshes.
        None => VersionCache {
            latest_version: version.to_string(),
            last_checked_at: DateTime::<Utc>::UNIX_EPOCH,
            dismissed_version: Some(version.to_string()),
        },
    };
    write_cache(path, &cache).await
}

async fn write_cache(path: &Path, cache: &VersionCache) -> Result<(), VersionCheckError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let json = serde_json::to_string(cache)?;
    tokio::fs::write(path, format!("{json}\n")).await?;
    Ok(())
}

#[derive(Deserialize)]
struct NpmDistTag {
    version: String,
}

async fn fetch_latest_version(url: &str) -> Result<String, VersionCheckError> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("coco/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let manifest: NpmDistTag = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(manifest.version)
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
