//! The skill Curator — periodic lifecycle management of agent-created skills.
//!
//! Mirrors memory's `DreamService` shape (time gate + cross-process CAS lock)
//! but is **write-only**: it never deletes. Aging is an **in-place `disabled`
//! flip** in the skill's frontmatter — the file stays on disk and recovery is
//! a one-line edit — so there is no `.archive/`, no tar.gz, and no `rm` in the
//! fence at all.
//!
//! Scanning is **location-keyed**: every skill directory under the agent
//! skills root is curator-managed, period. Frontmatter is LLM-written and is
//! not consulted for eligibility — keying on an `origin: agent` stamp would
//! make an unstamped (or injection-stripped) artifact immortal. The curator
//! lock and the promotions store both live OUTSIDE the fenced root, so the
//! review fork can neither suppress curation nor self-promote.
//!
//! MVP policy, gated on `total_invocations >= min_invocations`:
//! - **retire** when `success_rate < retire_success_rate` — independent of the
//!   autocomplete recency score, so a frequently-used-but-mostly-failing skill
//!   (high `usage_count`) is still retired.
//! - **promote** when `success_rate >= promote_success_rate` — persisted via
//!   [`coco_skills::agent_scope::save_promotions`], which is what lets the
//!   skill load model-invocable (until then it is quarantined to `/name`
//!   user invocation only).

use std::path::{Path, PathBuf};

use coco_maintenance::{MaintenanceLock, MaintenanceLockOutcome};
use coco_types::{JourneyEvent, SkillRetireReason};

/// Lock file basename. Lives in `<config_home>/skills` — a **sibling** of the
/// fenced `.agent` root, so the review fork cannot write (or unlink) it.
const CURATOR_LOCK_FILENAME: &str = ".skill-curator-lock";

/// Minimum hours between curator passes.
pub const DEFAULT_MIN_HOURS: i64 = 24;
/// Minimum invocations before the failure/promotion gates apply to a skill.
pub const DEFAULT_MIN_INVOCATIONS: i64 = 5;
/// Retire when the success rate over `>= min_invocations` runs is below this.
///
/// Note the failure signal is infrastructure-level (spawn/dispatch errors) —
/// a skill that runs fine but gives bad guidance never trips this gate. The
/// inactivity gate below is what shrinks the library in that case: unhelpful
/// skills stop being invoked, then age out.
pub const DEFAULT_RETIRE_SUCCESS_RATE: f64 = 0.34;
/// Promote to model-invocability when the success rate is at or above this.
pub const DEFAULT_PROMOTE_SUCCESS_RATE: f64 = 0.8;
/// Retire a previously-used skill after this many days without an invocation.
/// Never-used skills (no telemetry entry) are exempt — a grace floor so a
/// freshly created skill isn't retired before anyone tries it.
pub const DEFAULT_RETIRE_INACTIVE_DAYS: i64 = 90;

/// Why the curator did not run, or what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuratorOutcome {
    /// Not enough time has elapsed since the last pass.
    SkippedTimeGate,
    /// Another process holds the curator lock.
    SkippedLockHeld,
    /// Ran over `scanned` agent skills: `retired` disabled, `promoted`
    /// granted model-invocability.
    Ran {
        retired: usize,
        promoted: usize,
        scanned: usize,
    },
}

/// Periodic write-only lifecycle manager for agent-created skills. Every
/// threshold is resolved from `SkillLearnConfig`; the `DEFAULT_*` consts above
/// are that config's defaults.
pub struct SkillCurator {
    config_home: PathBuf,
    lock: MaintenanceLock,
    min_hours: i64,
    min_invocations: i64,
    /// Retire **below** this success rate (a floor, not a failure ceiling).
    retire_success_rate: f64,
    promote_success_rate: f64,
    retire_inactive_days: i64,
    /// Mirrors `SkillLearnConfig::journal_enabled`.
    journal_enabled: bool,
}

impl SkillCurator {
    /// Build a curator over `<config_home>/skills/.agent` with default
    /// thresholds.
    pub fn new(config_home: &Path) -> Self {
        Self::with_config(config_home, &coco_config::SkillLearnConfig::default())
    }

    /// Build a curator from resolved config (thresholds from `SkillLearnConfig`).
    pub fn with_config(config_home: &Path, config: &coco_config::SkillLearnConfig) -> Self {
        // The lock is a sibling of the fenced root — geometry owned by
        // `agent_scope` so it can never drift inside the fence.
        let lock = MaintenanceLock::new(
            &coco_skills::agent_scope::skills_root(config_home),
            CURATOR_LOCK_FILENAME,
        );
        Self {
            config_home: config_home.to_path_buf(),
            lock,
            min_hours: config.curator_min_hours,
            min_invocations: config.promote_min_invocations,
            retire_success_rate: config.retire_success_rate,
            promote_success_rate: config.promote_success_rate,
            retire_inactive_days: config.retire_inactive_days,
            journal_enabled: config.journal_enabled,
        }
    }

    /// Override the minimum hours between passes (tests pass `0` to bypass
    /// the time gate; the invocation/rate thresholds keep their defaults).
    pub fn with_min_hours(mut self, min_hours: i64) -> Self {
        self.min_hours = min_hours;
        self
    }

    /// Run a curator pass if the time gate + lock allow. Blocking I/O — call
    /// from `spawn_blocking` in async contexts.
    pub fn maybe_curate(&self) -> CuratorOutcome {
        let now = coco_utils_common::now_epoch_ms().unwrap_or(0);

        // Time gate — cheap stat before acquiring the lock.
        if let Some(last) = self.lock.last_run_at()
            && now.saturating_sub(last) < self.min_hours.saturating_mul(3_600_000)
        {
            return CuratorOutcome::SkippedTimeGate;
        }

        let guard = match self.lock.try_acquire() {
            MaintenanceLockOutcome::Acquired(g) => g,
            MaintenanceLockOutcome::Held | MaintenanceLockOutcome::Error(_) => {
                return CuratorOutcome::SkippedLockHeld;
            }
        };

        let telemetry = coco_skills::telemetry::load_all(&self.config_home);
        let mut promotions = coco_skills::agent_scope::load_promotions(&self.config_home);
        let mut newly_promoted: Vec<(String, i64, i64)> = Vec::new();
        let mut retired = 0usize;
        let mut scanned = 0usize;

        // Single scan implementation shared with `/journey`. Retired skills are
        // included so `scanned` counts every curator-managed directory; the
        // already-disabled ones are skipped from the gates below.
        for scan in coco_skills::agent_scope::scan_agent_skills(
            &self.config_home,
            coco_skills::agent_scope::IncludeDisabled::Yes,
        ) {
            // Location-keyed: living under the agent root IS the
            // curator-managed signal.
            scanned += 1;
            let name = scan.skill.name.clone();
            let skill_md = &scan.skill_md;
            // Never-invoked skills have no telemetry entry — the grace floor.
            let Some(stats) = telemetry.get(&name) else {
                continue;
            };
            if scan.disabled {
                continue;
            }
            // Inactivity aging first: a once-used skill nobody invokes anymore
            // ages out regardless of its success rate (the failure gate below
            // only sees infra errors, so "runs but unhelpful" skills retire
            // through THIS gate).
            let inactive_ms = now.saturating_sub(stats.last_used_at_ms);
            if stats.last_used_at_ms > 0
                && inactive_ms >= self.retire_inactive_days.saturating_mul(86_400_000)
            {
                if coco_skills::set_skill_disabled(skill_md, true).is_ok() {
                    tracing::info!(
                        target: "coco_skill_learn::curator",
                        skill = %name,
                        inactive_days = inactive_ms / 86_400_000,
                        "retired inactive agent skill"
                    );
                    self.append_journal_event(JourneyEvent::SkillRetired {
                        name: name.clone(),
                        reason: SkillRetireReason::Inactivity,
                    });
                    retired += 1;
                }
                continue;
            }
            if stats.total_invocations() < self.min_invocations {
                continue;
            }
            if stats.success_rate() < self.retire_success_rate {
                if coco_skills::set_skill_disabled(skill_md, true).is_ok() {
                    tracing::info!(
                        target: "coco_skill_learn::curator",
                        skill = %name,
                        success = stats.success_count,
                        failure = stats.failure_count,
                        "retired misfiring agent skill"
                    );
                    self.append_journal_event(JourneyEvent::SkillRetired {
                        name: name.clone(),
                        reason: SkillRetireReason::FailureRate,
                    });
                    retired += 1;
                }
            } else if stats.success_rate() >= self.promote_success_rate
                && promotions.insert(name.clone())
            {
                newly_promoted.push((name.clone(), stats.success_count, stats.failure_count));
            }
        }

        // One write for all of this pass's promotions; log only what actually
        // persisted so a failed write can't claim a promotion.
        let mut promoted = 0usize;
        if !newly_promoted.is_empty()
            && coco_skills::agent_scope::save_promotions(&self.config_home, &promotions)
        {
            promoted = newly_promoted.len();
            for (name, success, failure) in &newly_promoted {
                tracing::info!(
                    target: "coco_skill_learn::curator",
                    skill = %name,
                    success,
                    failure,
                    "promoted agent skill to model-invocable"
                );
                self.append_journal_event(JourneyEvent::SkillPromoted { name: name.clone() });
            }
        }

        // Stamp lastConsolidatedAt = now so the time gate resets.
        guard.commit();
        CuratorOutcome::Ran {
            retired,
            promoted,
            scanned,
        }
    }

    /// Append one curator event to the learning journal (best-effort; the
    /// curator carries no session context so `session_id` is `None`). A no-op
    /// when `SkillLearnConfig::journal_enabled` is off.
    fn append_journal_event(&self, event: JourneyEvent) {
        if !self.journal_enabled {
            return;
        }
        crate::journal::append_event(&self.config_home, None, event);
    }
}

#[cfg(test)]
#[path = "curator.test.rs"]
mod tests;
