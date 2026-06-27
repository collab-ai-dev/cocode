//! Official marketplace startup auto-install.
//!
//! Fetches the official `anthropics/claude-plugins-official` github marketplace
//! once on startup, with a persisted exponential-backoff retry gate. The
//! Anthropic-internal GCS accelerator is intentionally omitted — a plain github
//! source is the portable behavior.

use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::marketplace::MarketplaceManager;
use crate::marketplace::OFFICIAL_MARKETPLACE_NAME;
use crate::marketplace::official_marketplace_source;

/// Give up after this many failed attempts.
const MAX_ATTEMPTS: u32 = 10;
/// First retry delay; doubles each attempt (1h → 1week backoff).
const BASE_BACKOFF_SECS: i64 = 3600;
const MAX_BACKOFF_SECS: i64 = 7 * 24 * 3600;
const STATE_FILE: &str = "official_marketplace_state.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct AutoInstallState {
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_attempt: Option<DateTime<Utc>>,
}

/// Outcome of an [`ensure_official_marketplace`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficialInstallOutcome {
    /// Opted out via `COCO_PLUGINS_DISABLE_OFFICIAL_MARKETPLACE`.
    Disabled,
    /// Already registered (a prior run or the user added it).
    AlreadyInstalled,
    /// Within the backoff window after a recent failed attempt.
    Backoff,
    /// Max attempts reached; gives up until the state file is cleared.
    Exhausted,
    /// Freshly fetched + registered on this call.
    Installed,
    /// Attempt failed (network / git); retried after backoff.
    Failed(String),
}

/// Ensure the official marketplace is registered + materialized on disk.
///
/// Idempotent, non-fatal, and retry-gated — safe to call fire-and-forget at
/// startup. The attempt count + backoff clock persist in
/// `<plugins_dir>/official_marketplace_state.json`.
pub async fn ensure_official_marketplace(plugins_dir: PathBuf) -> OfficialInstallOutcome {
    if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoPluginsDisableOfficialMarketplace) {
        return OfficialInstallOutcome::Disabled;
    }

    let state_path = plugins_dir.join(STATE_FILE);
    let mut state = load_state(&state_path);

    // Already registered (user or a prior run)? Stamp installed + done.
    let manager = MarketplaceManager::new(plugins_dir.clone());
    if manager
        .load_known_marketplaces()
        .contains_key(OFFICIAL_MARKETPLACE_NAME)
    {
        if !state.installed {
            state.installed = true;
            let _ = save_state(&state_path, &state);
        }
        return OfficialInstallOutcome::AlreadyInstalled;
    }
    if state.installed {
        return OfficialInstallOutcome::AlreadyInstalled;
    }
    if state.attempts >= MAX_ATTEMPTS {
        return OfficialInstallOutcome::Exhausted;
    }
    if let Some(last) = state.last_attempt
        && Utc::now().signed_duration_since(last).num_seconds() < backoff_secs(state.attempts)
    {
        return OfficialInstallOutcome::Backoff;
    }

    // Attempt fetch + register.
    let source = official_marketplace_source();
    let cache_dir = plugins_dir.join("marketplaces");
    let mut manager = MarketplaceManager::new(plugins_dir.clone());
    let result =
        match crate::fetch::fetch_marketplace(&source, OFFICIAL_MARKETPLACE_NAME, &cache_dir).await
        {
            Ok(loc) => manager
                .register_marketplace(OFFICIAL_MARKETPLACE_NAME, source, &loc.to_string_lossy())
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };

    match result {
        Ok(()) => {
            state.installed = true;
            state.last_attempt = Some(Utc::now());
            let _ = save_state(&state_path, &state);
            OfficialInstallOutcome::Installed
        }
        Err(msg) => {
            state.attempts += 1;
            state.last_attempt = Some(Utc::now());
            let _ = save_state(&state_path, &state);
            tracing::debug!(
                attempt = state.attempts,
                "official marketplace auto-install failed: {msg}"
            );
            OfficialInstallOutcome::Failed(msg)
        }
    }
}

/// Exponential backoff: `base * 2^attempts`, capped at one week.
fn backoff_secs(attempts: u32) -> i64 {
    BASE_BACKOFF_SECS
        .saturating_mul(1i64 << attempts.min(20))
        .min(MAX_BACKOFF_SECS)
}

fn load_state(path: &Path) -> AutoInstallState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(path: &Path, state: &AutoInstallState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state).unwrap_or_default();
    std::fs::write(path, json)
}

#[cfg(test)]
#[path = "official.test.rs"]
mod tests;
