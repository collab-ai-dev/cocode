//! Token and turn budget tracking for the query engine.
//!
//! Tracks token consumption, turn counts, and detects diminishing returns
//! (model producing less and less output per turn).

use coco_types::TokenUsage;

/// Decision from budget check — whether to continue, stop, or warn.
#[derive(Debug, Clone)]
pub enum BudgetDecision {
    /// Budget allows continued execution.
    Continue,
    /// Budget exhausted, must stop.
    Stop { reason: String },
    /// Approaching budget limit, inject a warning.
    Nudge { message: String },
}

/// Tracks token consumption and turn counts against configured limits.
pub struct BudgetTracker {
    pub max_tokens: Option<i64>,
    /// Turn cap. `None` = unbounded (interactive REPL); `Some(n)` caps at n
    /// (`--print --max-turns`, and every subagent/fork).
    pub max_turns: Option<i32>,
    pub max_continuations: i32,
    pub min_remaining_tokens: i64,
    consumed_tokens: i64,
    continuation_count: i32,
    /// Token output from the previous turn (for diminishing returns).
    last_delta_tokens: i64,
    /// Token output from the current turn.
    current_delta_tokens: i64,
    /// Threshold percentage (0-100) for nudge warnings.
    nudge_threshold_pct: i32,
}

impl BudgetTracker {
    pub fn new(max_tokens: Option<i64>, max_turns: Option<i32>, max_continuations: i32) -> Self {
        Self {
            max_tokens,
            max_turns,
            max_continuations,
            min_remaining_tokens: 500,
            consumed_tokens: 0,
            continuation_count: 0,
            last_delta_tokens: 0,
            current_delta_tokens: 0,
            nudge_threshold_pct: 90,
        }
    }

    /// Record token usage from an LLM response.
    pub fn record_usage(&mut self, usage: &TokenUsage) {
        let delta = usage
            .input_tokens
            .total
            .saturating_add(usage.output_tokens.total);
        self.consumed_tokens = self.consumed_tokens.saturating_add(delta);
        self.last_delta_tokens = self.current_delta_tokens;
        self.current_delta_tokens = usage.output_tokens.total;
    }

    /// Increment the continuation counter (for auto-continue scenarios).
    pub fn record_continuation(&mut self) {
        self.continuation_count += 1;
    }

    /// Reset the continuation counter (after compaction).
    pub fn reset_continuations(&mut self) {
        self.continuation_count = 0;
    }

    /// Check whether the budget allows continuing.
    pub fn check(&self, current_turn: i32) -> BudgetDecision {
        // Check turn limit. `None` = unbounded: the model loops until it stops
        // on its own, the token budget trips, or the user interrupts.
        if let Some(max) = self.max_turns
            && current_turn >= max
        {
            return BudgetDecision::Stop {
                reason: format!("reached maximum turns ({current_turn}/{max})"),
            };
        }

        // Check continuation limit.
        if self.continuation_count >= self.max_continuations {
            return BudgetDecision::Stop {
                reason: format!(
                    "reached maximum continuations ({count}/{max})",
                    count = self.continuation_count,
                    max = self.max_continuations
                ),
            };
        }

        // Check token limit.
        if let Some(max) = self.max_tokens {
            if self.consumed_tokens >= max {
                return BudgetDecision::Stop {
                    reason: format!(
                        "token budget exhausted ({consumed}/{max})",
                        consumed = self.consumed_tokens
                    ),
                };
            }

            let pct_used = (self.consumed_tokens as f64 / max as f64 * 100.0) as i32;

            // Diminishing returns: if we've had 3+ continuations and both
            // last and current deltas are under 500 tokens, stop.
            if pct_used >= self.nudge_threshold_pct
                && self.continuation_count >= 3
                && self.last_delta_tokens < 500
                && self.current_delta_tokens < 500
            {
                return BudgetDecision::Stop {
                    reason: format!(
                        "diminishing returns at {pct_used}% budget ({consumed}/{max})",
                        consumed = self.consumed_tokens
                    ),
                };
            }

            // Nudge if past threshold.
            if pct_used >= self.nudge_threshold_pct {
                return BudgetDecision::Nudge {
                    message: format!(
                        "running low on token budget ({consumed}/{max} tokens used, {pct_used}%)",
                        consumed = self.consumed_tokens
                    ),
                };
            }
        }

        BudgetDecision::Continue
    }

    /// Total tokens consumed so far.
    pub fn total_tokens(&self) -> i64 {
        self.consumed_tokens
    }

    /// Number of forced continuations recorded so far.
    pub fn continuation_count(&self) -> i32 {
        self.continuation_count
    }

    /// Remaining tokens (if max is set).
    pub fn remaining(&self) -> Option<i64> {
        self.max_tokens.map(|max| max - self.consumed_tokens)
    }
}

#[cfg(test)]
#[path = "budget.test.rs"]
mod tests;
