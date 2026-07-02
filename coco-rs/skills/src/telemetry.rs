//! Per-skill lifecycle telemetry — success / failure / view / patch counts
//! that let the skill-learning Curator make aging and retire/refine decisions.
//!
//! Deliberately **separate** from [`crate::usage`] (which feeds only the `/`
//! autocomplete recency ranker):
//!
//! - **No debounce.** Every lifecycle event is recorded; a debounce would drop
//!   e.g. a failure that lands within 60s of a prior success — exactly the
//!   bursty "worked once then failed repeatedly" pattern the Curator needs.
//! - **In-process file lock.** The background review fork's `record_patch`
//!   and the main loop's `record_invocation` serialize their read-modify-write
//!   within one process so neither loses an increment. Cross-process races
//!   (two coco sessions) can still lose a counter increment — acceptable,
//!   since the Curator's gates only need order-of-magnitude counts, and the
//!   atomic-rename write keeps the file itself consistent.
//! - **Success counts never mix with the autocomplete ranker**, so failures
//!   can't pollute `usage::score_for`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Outcome of a single skill invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutcome {
    Success,
    Failure,
}

/// Lifecycle counters for one skill, keyed by skill name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillTelemetryStats {
    #[serde(default)]
    pub success_count: i64,
    #[serde(default)]
    pub failure_count: i64,
    #[serde(default)]
    pub patch_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<SkillOutcome>,
    #[serde(default)]
    pub last_used_at_ms: i64,
    #[serde(default)]
    pub last_patched_at_ms: i64,
}

impl SkillTelemetryStats {
    /// Total invocations (success + failure).
    pub fn total_invocations(&self) -> i64 {
        self.success_count.saturating_add(self.failure_count)
    }

    /// Success rate in `[0.0, 1.0]`. Returns `1.0` when there are no
    /// invocations yet — there is nothing to penalize, so a brand-new skill is
    /// never treated as failing.
    pub fn success_rate(&self) -> f64 {
        let total = self.total_invocations();
        if total <= 0 {
            return 1.0;
        }
        self.success_count as f64 / total as f64
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TelemetryFile {
    #[serde(default)]
    skills: HashMap<String, SkillTelemetryStats>,
}

fn telemetry_file_path(config_home: &Path) -> PathBuf {
    config_home.join("skill_telemetry.json")
}

/// Serializes read-modify-write across threads so concurrent records (main
/// loop vs background fork) don't lose updates.
fn file_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Record one skill invocation into BOTH stores with the canonical pairing:
/// [`crate::usage`] (the `/` autocomplete recency ranker) counts successes
/// only, so failures never inflate the ranking; lifecycle telemetry records
/// every outcome for the Curator. Every invocation seam (inline slash, Skill
/// tool, fork) must go through this so the two stores cannot drift.
///
/// **Blocking I/O — wrap in `spawn_blocking` from async contexts**, or use
/// [`record_invocation_outcome_detached`].
pub fn record_invocation_outcome(config_home: &Path, skill_name: &str, outcome: SkillOutcome) {
    if outcome == SkillOutcome::Success {
        crate::usage::record(config_home, skill_name);
    }
    record_invocation(config_home, skill_name, outcome);
}

/// Fire-and-forget variant of [`record_invocation_outcome`] for async
/// dispatchers: runs on a blocking thread so the caller never waits on the
/// read + atomic write. Must be called from within a Tokio runtime.
pub fn record_invocation_outcome_detached(
    config_home: PathBuf,
    skill_name: String,
    outcome: SkillOutcome,
) {
    tokio::task::spawn_blocking(move || {
        record_invocation_outcome(&config_home, &skill_name, outcome);
    });
}

/// Record a skill invocation outcome (success or failure).
///
/// **Blocking I/O — wrap in `spawn_blocking` from async contexts.**
pub fn record_invocation(config_home: &Path, skill_name: &str, outcome: SkillOutcome) {
    mutate(config_home, skill_name, |s, now| {
        match outcome {
            SkillOutcome::Success => s.success_count = s.success_count.saturating_add(1),
            SkillOutcome::Failure => s.failure_count = s.failure_count.saturating_add(1),
        }
        s.last_status = Some(outcome);
        s.last_used_at_ms = now;
    });
}

/// Record that a skill was patched by the learning loop.
pub fn record_patch(config_home: &Path, skill_name: &str) {
    mutate(config_home, skill_name, |s, now| {
        s.patch_count = s.patch_count.saturating_add(1);
        s.last_patched_at_ms = now;
    });
}

/// Load all lifecycle stats. Blocking I/O; callers on a hot path should cache.
pub fn load_all(config_home: &Path) -> HashMap<String, SkillTelemetryStats> {
    let _g = file_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    read_file(config_home).skills
}

fn mutate(config_home: &Path, skill_name: &str, f: impl FnOnce(&mut SkillTelemetryStats, i64)) {
    if skill_name.is_empty() {
        debug_assert!(false, "skill_telemetry: record called with empty name");
        return;
    }
    // Refuse to record on a pre-1970 clock — a `0` timestamp would poison
    // recency/aging math downstream (mirrors the usage-store contract).
    let Some(now) = coco_utils_common::now_epoch_ms() else {
        tracing::warn!(
            skill = %skill_name,
            "skill_telemetry: system clock pre-1970, skipping record"
        );
        return;
    };
    let _g = file_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut file = read_file(config_home);
    let entry = file.skills.entry(skill_name.to_string()).or_default();
    f(entry, now);
    write_file(config_home, &file);
}

fn read_file(config_home: &Path) -> TelemetryFile {
    match std::fs::read_to_string(telemetry_file_path(config_home)) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => TelemetryFile::default(),
    }
}

fn write_file(config_home: &Path, file: &TelemetryFile) {
    let path = telemetry_file_path(config_home);
    let Ok(json) = serde_json::to_string_pretty(file) else {
        return;
    };
    if let Err(e) = coco_utils_common::write_atomic(&path, json) {
        tracing::debug!(?path, error = %e, "skill_telemetry: write failed");
    }
}

#[cfg(test)]
#[path = "telemetry.test.rs"]
mod tests;
