//! Budgets, usage accounting, and per-goal counters.
//!
//! Two budgets are tracked and are deliberately different (§11.1):
//!
//! * the **autonomous-continuation** cap bounds *unattended* looping — only a
//!   supervisor-started continuation spends it;
//! * the optional **token** budget covers *all* goal-owned turns (user-guided
//!   and autonomous) because both contribute real cost.

use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Serialize};

/// Default unattended-continuation cap (§11.1).
pub const DEFAULT_MAX_AUTONOMOUS_TURNS: NonZeroU32 = nonzero_u32(20);

/// Default completion-probe cadence: autonomous continuations since the most
/// recent user-guided turn (§12.5).
pub const DEFAULT_PROBE_INTERVAL: NonZeroU32 = nonzero_u32(5);

/// Consecutive signal-free goal turns that trip `paused(no_progress)` (§9.5).
pub const NO_PROGRESS_LIMIT: u32 = 3;

/// Bounded transient-scheduler retry attempts before `paused(scheduler_unavailable)`.
pub const MAX_SCHEDULER_RETRIES: u32 = 3;

const fn nonzero_u32(value: u32) -> NonZeroU32 {
    match NonZeroU32::new(value) {
        Some(n) => n,
        None => panic!("compile-time budget constant must be non-zero"),
    }
}

/// User-authored budget limits. Positive limits are non-zero by construction so a
/// zero budget is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudget {
    /// Unattended-continuation cap across the goal lifetime.
    pub max_autonomous_turns: NonZeroU32,
    /// Optional total-token ceiling across all goal-owned turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<NonZeroU64>,
    /// Completion-probe cadence for goals without deterministic coverage.
    pub probe_interval: NonZeroU32,
}

impl Default for GoalBudget {
    fn default() -> Self {
        Self {
            max_autonomous_turns: DEFAULT_MAX_AUTONOMOUS_TURNS,
            max_tokens: None,
            probe_interval: DEFAULT_PROBE_INTERVAL,
        }
    }
}

/// Committed usage totals. Token accounting uses input+output deltas from session
/// usage; duration is accumulated from monotonic deltas while live (§11.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub active_duration_ms: i64,
}

impl GoalUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn apply(&mut self, delta: UsageDelta) {
        self.input_tokens = self.input_tokens.saturating_add(delta.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(delta.output_tokens);
        self.active_duration_ms = self.active_duration_ms.saturating_add(delta.duration_ms);
    }
}

/// An idempotent usage increment committed at a tool-finish or turn-stop boundary.
/// The host keys application by `(goal_id, lease_id, effect_id)`; the reducer only
/// folds the totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageDelta {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: i64,
}

/// What started a goal-owned turn. Determines whether the autonomous quota and the
/// probe cadence advance (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalTurnTrigger {
    /// Kickoff turn immediately after creation. Goal-owned, spends no quota.
    Creation,
    /// User-started or user-resumed turn. Goal-owned, spends no autonomous quota
    /// and resets the probe cadence.
    UserInput,
    /// Supervisor context-only continuation or a registered wake. Spends the
    /// autonomous quota and advances the probe cadence.
    Autonomous,
}

impl GoalTurnTrigger {
    /// Whether this trigger consumes an autonomous-continuation turn.
    pub fn spends_autonomous_quota(self) -> bool {
        matches!(self, Self::Autonomous)
    }

    /// Whether this trigger resets the completion-probe cadence.
    pub fn resets_probe_cadence(self) -> bool {
        matches!(self, Self::Creation | Self::UserInput)
    }
}

/// Non-durable-spec counters that advance on runtime transitions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCounters {
    /// Every goal-owned turn (audit count).
    pub total_turns: u32,
    /// Autonomous continuations started (spends the turn budget).
    pub autonomous_turns: u32,
    /// Consecutive goal turns with no accepted `ProgressSignal`.
    pub no_progress_streak: u32,
    /// Consecutive goal turns without a `report_goal_turn` call.
    pub unreported_streak: u32,
    /// Autonomous continuations since the most recent user-guided turn.
    pub continuations_since_user_turn: u32,
    /// Suppress back-to-back completion probing after any candidate/audit/probe.
    pub probe_cooldown: bool,
    /// Transient scheduler-retry attempts (reset on resume).
    pub scheduler_retries: u32,
}

impl GoalCounters {
    /// Whether the autonomous-turn budget is exhausted for `budget`.
    pub fn autonomous_exhausted(&self, budget: &GoalBudget) -> bool {
        self.autonomous_turns >= budget.max_autonomous_turns.get()
    }

    /// Whether the no-progress boundary has been reached.
    pub fn no_progress_tripped(&self) -> bool {
        self.no_progress_streak >= NO_PROGRESS_LIMIT
    }

    /// Whether a completion probe is due for `budget` (cadence reached, not in
    /// cooldown).
    pub fn probe_due(&self, budget: &GoalBudget) -> bool {
        !self.probe_cooldown && self.continuations_since_user_turn >= budget.probe_interval.get()
    }
}

#[cfg(test)]
#[path = "budget.test.rs"]
mod tests;
