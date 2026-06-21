//! Pure scheduler core — the I/O-free heart of the cron tick loop.
//!
//! Faithful port of the decision logic in TS `utils/cronScheduler.ts`
//! `check()` + `isRecurringTaskAged()` + `findMissed()`. No clock, no files,
//! no tokio: the caller passes `now_ms` each tick and performs the side effects
//! (enqueue the prompt, mark fired / remove) the returned [`DueFire`]s describe.
//! This keeps the timing logic deterministic and unit-testable.

use crate::next_cron_run_ms;
use std::collections::HashMap;
use std::collections::HashSet;

/// Default recurring-task auto-expiry: 7 days after creation (TS
/// `DEFAULT_CRON_JITTER_CONFIG.recurringMaxAgeMs`). `0` = never expire.
pub const RECURRING_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The timing-only view of a task the scheduler core needs. The owning record
/// (prompt, durable, agent_id) lives in the store layer; the core sees only
/// what it needs to decide *when* to fire.
#[derive(Debug, Clone, Copy)]
pub struct CronTiming<'a> {
    pub id: &'a str,
    pub cron: &'a str,
    pub created_at_ms: i64,
    pub last_fired_at_ms: Option<i64>,
    pub recurring: bool,
    pub permanent: bool,
}

/// One fire decision produced by [`CronTickState::tick`]. The caller turns this
/// into side effects: always enqueue the task's prompt; then if
/// `recurring && !aged` persist `last_fired_at = now`, otherwise remove the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueFire {
    pub id: String,
    pub recurring: bool,
    /// Recurring task that crossed `recurring_max_age_ms` — fires one last time
    /// then is removed (TS aged-out path).
    pub aged: bool,
}

/// `true` when a recurring task is older than `max_age_ms` and not `permanent`.
/// `max_age_ms == 0` means unlimited.
pub fn is_recurring_task_aged(t: &CronTiming<'_>, now_ms: i64, max_age_ms: i64) -> bool {
    if max_age_ms == 0 {
        return false;
    }
    t.recurring && !t.permanent && now_ms - t.created_at_ms >= max_age_ms
}

/// Per-task next-fire schedule, carried across ticks. `i64::MAX` encodes
/// "never" (unparseable cron / no match in the next year), matching TS's use
/// of `Infinity` in the `nextFireAt` map.
#[derive(Debug, Default)]
pub struct CronTickState {
    next_fire_at: HashMap<String, i64>,
}

impl CronTickState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one scheduler tick. Mirrors `cronScheduler.ts` `check()`:
    /// - **First sight** of a task: anchor its next fire from
    ///   `last_fired_at ?? created_at` (recurring) or `created_at` (one-shot),
    ///   so a task that came due while the process was down still fires once.
    /// - **Due** (`now >= next_fire_at`): emit a [`DueFire`]; reschedule a live
    ///   recurring task **from `now`** (not from the matched instant — avoids
    ///   rapid catch-up if a tick was delayed); drop one-shot / aged tasks from
    ///   the schedule.
    /// - **Evict** schedule entries for tasks no longer present.
    ///
    /// Returns the fires for this tick (at most one per task). The caller owns
    /// the side effects.
    pub fn tick(
        &mut self,
        tasks: &[CronTiming<'_>],
        now_ms: i64,
        recurring_max_age_ms: i64,
    ) -> Vec<DueFire> {
        let mut fires = Vec::new();
        let mut seen: HashSet<&str> = HashSet::with_capacity(tasks.len());

        for t in tasks {
            seen.insert(t.id);

            let next = match self.next_fire_at.get(t.id) {
                Some(&n) => n,
                None => {
                    let anchor = if t.recurring {
                        t.last_fired_at_ms.unwrap_or(t.created_at_ms)
                    } else {
                        t.created_at_ms
                    };
                    let n = next_cron_run_ms(t.cron, anchor).unwrap_or(i64::MAX);
                    self.next_fire_at.insert(t.id.to_string(), n);
                    n
                }
            };

            if now_ms < next {
                continue;
            }

            let aged = is_recurring_task_aged(t, now_ms, recurring_max_age_ms);
            if t.recurring && !aged {
                // Reschedule from `now`, not from the matched instant.
                let new_next = next_cron_run_ms(t.cron, now_ms).unwrap_or(i64::MAX);
                self.next_fire_at.insert(t.id.to_string(), new_next);
            } else {
                self.next_fire_at.remove(t.id);
            }

            fires.push(DueFire {
                id: t.id.to_string(),
                recurring: t.recurring,
                aged,
            });
        }

        // Evict schedule entries for tasks that vanished (deleted / completed).
        self.next_fire_at.retain(|id, _| seen.contains(id.as_str()));
        fires
    }

    /// Soonest scheduled fire across all known tasks, or `None` if nothing is
    /// pending (no tasks, or all "never"). Mirrors TS `getNextFireTime`.
    pub fn next_fire_time(&self) -> Option<i64> {
        self.next_fire_at
            .values()
            .copied()
            .filter(|&n| n != i64::MAX)
            .min()
    }
}

/// Startup missed-task scan: **one-shot** tasks whose scheduled time (computed
/// from `created_at`) is already in the past and which never fired. Recurring
/// tasks are excluded — `tick()` fires them on the first pass. Mirrors
/// `cronScheduler.ts` `findMissed` (+ `cronTasks.ts` `findMissedTasks`).
pub fn find_missed(tasks: &[CronTiming<'_>], now_ms: i64) -> Vec<String> {
    tasks
        .iter()
        .filter(|t| !t.recurring && t.last_fired_at_ms.is_none())
        .filter(|t| next_cron_run_ms(t.cron, t.created_at_ms).is_some_and(|next| next < now_ms))
        .map(|t| t.id.to_string())
        .collect()
}

#[cfg(test)]
#[path = "scheduler.test.rs"]
mod tests;
