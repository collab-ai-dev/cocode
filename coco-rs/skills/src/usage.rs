//! Skill usage tracking — frequency + recency-decayed score that drives
//! the "recently used" section of the `/` autocomplete popup.
//!
//! 7-day half-life, minimum recency factor 0.1, 60-second debounce so
//! a single skill invoked rapidly doesn't pound the filesystem.
//!
//! ## Storage
//!
//! `<config_home>/skill_usage.json`, structure:
//!
//! ```json
//! { "skills": { "name": { "usageCount": 5, "lastUsedAtMs": 1748313600000 } } }
//! ```
//!
//! Stored as i64 millis (epoch ms).
//!
//! ## Writes are atomic
//!
//! Each `record` writes through a sibling `NamedTempFile` and persists
//! via atomic rename. A crash or signal mid-write leaves the previous
//! file intact — single-skill data loss is bounded to the in-flight
//! increment, never the whole history.
//!
//! ## Threading
//!
//! [`record`] is sync and does blocking file I/O. Callers in async
//! contexts MUST wrap it in [`tokio::task::spawn_blocking`]. The 60-
//! second debounce makes the I/O rare in practice, but it's never
//! lock-free.
//!
//! The process-local debounce table is updated AFTER a successful
//! write — a transient I/O failure leaves the debounce window open so
//! the next call retries. This prefers retry-friendly semantics: the
//! debounce window stays open on failure so the next call can retry.
//!
//! Cross-process debounce is best-effort: two coco processes hitting
//! the same skill within 60s each pass the in-memory check and write
//! once. The file lives under the user's config home so concurrent
//! sessions are rare; the atomic-rename write keeps the file shape
//! consistent even when they race.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

/// Skip the disk write when the same skill records within this window.
const DEBOUNCE_MS: i64 = 60_000;

/// Half-life in days for the recency-decay weighting.
/// `recencyFactor = 0.5 ^ (daysSinceUse / 7)`.
const HALF_LIFE_DAYS: f64 = 7.0;

/// Floor for the recency factor so a once-popular skill that hasn't
/// been used in months still beats a never-used skill.
const MIN_RECENCY_FACTOR: f64 = 0.1;

/// Per-skill counters persisted to disk.
///
/// Wire-compatible with the `globalConfig.skillUsage[name]` shape —
/// uses `usageCount` / `lastUsedAt`, accepted via aliases for
/// forward-compat with upstream-generated files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillUsageStats {
    #[serde(default, alias = "usageCount")]
    pub usage_count: i64,
    #[serde(default, alias = "lastUsedAt")]
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UsageFile {
    #[serde(default)]
    skills: HashMap<String, SkillUsageStats>,
}

fn usage_file_path(config_home: &Path) -> PathBuf {
    config_home.join("skill_usage.json")
}

/// Process-lifetime debounce cache. Mirrors the
/// `lastWriteBySkill: Map<string, number>` approach so the 60-second
/// gate is a memory hit, not a file read.
fn last_write_map() -> &'static Mutex<HashMap<String, i64>> {
    static LAST_WRITE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    LAST_WRITE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Best-effort wall-clock millis. Returns `None` only if the system
/// clock is set before 1970 — every healthy machine returns `Some`.
/// Callers that get `None` MUST refuse to record: storing a `0`
/// sentinel would poison every future score read (50+ years of decay
/// flooring everything at 0.1 of usage count).
fn now_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

/// Record a skill invocation. Best-effort: errors are logged via
/// `tracing` but never surface to the caller; the implementation
/// discards failures.
///
/// **Blocking I/O — wrap in `spawn_blocking` from async contexts.**
/// The 60-second debounce makes most calls return immediately without
/// touching disk, but the slow path reads/writes the full JSON file.
pub fn record(config_home: &Path, skill_name: &str) {
    if skill_name.is_empty() {
        debug_assert!(false, "skill_usage::record called with empty name");
        return;
    }
    let Some(now) = now_ms() else {
        tracing::warn!(
            skill = %skill_name,
            "skill_usage: system clock pre-1970, skipping record"
        );
        return;
    };

    // Debounce check — read the in-memory map only. We do NOT update
    // it yet because the write below can fail; updating eagerly would
    // block the retry for 60s.
    {
        let last = lock_debounce_map();
        if let Some(&prev) = last.get(skill_name)
            && now - prev < DEBOUNCE_MS
        {
            return;
        }
    }

    let path = usage_file_path(config_home);
    let mut file: UsageFile = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let entry = file.skills.entry(skill_name.to_string()).or_default();
    entry.usage_count = entry.usage_count.saturating_add(1);
    entry.last_used_at_ms = now;

    if let Err(e) = write_atomic(&path, &file) {
        tracing::debug!(?path, error = %e, "skill_usage: write failed; debounce window unchanged");
        return;
    }

    // Only mark the debounce window after the write committed. A
    // failed write must remain retryable.
    lock_debounce_map().insert(skill_name.to_string(), now);
}

/// Load the entire usage table. Returns an empty map on missing or
/// malformed file — usage tracking is best-effort so the popup must
/// always have something sensible to fall back on.
///
/// **Blocking I/O.** Hot-path callers should cache the result; see
/// [`coco-tui`'s autocomplete state](../../../app/tui/CLAUDE.md).
pub fn load_all(config_home: &Path) -> HashMap<String, SkillUsageStats> {
    let path = usage_file_path(config_home);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<UsageFile>(&text) {
            Ok(file) => file.skills,
            Err(e) => {
                tracing::warn!(?path, error = %e, "skill_usage: malformed file, ignoring");
                HashMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => {
            tracing::debug!(?path, error = %e, "skill_usage: read failed");
            HashMap::new()
        }
    }
}

/// Compute the ranking score for a single skill. Port of
/// `getSkillUsageScore`:
///
/// ```text
/// score = usage_count * max(0.5 ^ (days_since_use / 7), 0.1)
/// ```
///
/// Returns 0 for fresh skills (count = 0) so callers can filter them
/// out of the recently-used section.
pub fn score_for(stats: &SkillUsageStats) -> f64 {
    let Some(now) = now_ms() else {
        // Clock failure: best we can do is report frequency without
        // decay. Slightly inflates old entries but never returns NaN.
        return stats.usage_count as f64 * MIN_RECENCY_FACTOR;
    };
    score_for_at(stats, now)
}

/// Internal version with injectable clock for deterministic tests.
pub(crate) fn score_for_at(stats: &SkillUsageStats, now: i64) -> f64 {
    if stats.usage_count <= 0 {
        return 0.0;
    }
    let elapsed_ms = (now - stats.last_used_at_ms).max(0);
    let days = elapsed_ms as f64 / (1000.0 * 60.0 * 60.0 * 24.0);
    let recency = 0.5_f64.powf(days / HALF_LIFE_DAYS).max(MIN_RECENCY_FACTOR);
    stats.usage_count as f64 * recency
}

/// Acquire the debounce mutex, recovering from poison. A poisoned lock
/// means a previous holder panicked — for non-critical telemetry the
/// safer option is to log and continue rather than escalate.
fn lock_debounce_map() -> std::sync::MutexGuard<'static, HashMap<String, i64>> {
    match last_write_map().lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("skill_usage: debounce mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Write `file` to `path` atomically — never leaves a truncated or
/// partially-serialized JSON on disk. Uses [`tempfile::NamedTempFile`]
/// in the target directory (cross-filesystem rename is a no-go) and
/// `persist` for the atomic swap.
fn write_atomic(path: &Path, file: &UsageFile) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(json.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "usage.test.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn reset_debounce_for_tests() {
    lock_debounce_map().clear();
}
