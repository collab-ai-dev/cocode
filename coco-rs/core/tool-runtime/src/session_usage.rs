//! Session-wide loop breakers for model-dispatched tools.
//!
//! These counters are intentionally independent of task lifecycle state:
//! completed agents still consume the spawn budget, while a `/clear` creates a
//! new session runtime and therefore a fresh counter set. Atomic check-and-
//! increment keeps concurrent tool batches from overshooting their limits.

use std::sync::atomic::{AtomicI32, Ordering};

/// Result of atomically charging one tool call to a session budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLimitDecision {
    /// The call was charged; `used` includes the newly accepted call.
    Allowed { used: i32, limit: i32 },
    /// The counter was already at its limit and remains unchanged.
    Exhausted { used: i32, limit: i32 },
}

/// Race-free session counters shared by the main agent and child engines.
#[derive(Debug)]
pub struct SessionUsageLimits {
    agent_spawns: AtomicI32,
    web_searches: AtomicI32,
    max_agent_spawns: i32,
    max_web_searches: i32,
}

impl SessionUsageLimits {
    /// Create zeroed counters with limits clamped to at least one.
    pub fn new(max_agent_spawns: i32, max_web_searches: i32) -> Self {
        Self {
            agent_spawns: AtomicI32::new(0),
            web_searches: AtomicI32::new(0),
            max_agent_spawns: max_agent_spawns.max(1),
            max_web_searches: max_web_searches.max(1),
        }
    }

    /// Build from the session's resolved agent-teams config.
    pub fn from_config(config: &coco_config::AgentTeamsConfig) -> Self {
        Self::new(
            config.max_subagents_per_session,
            config.max_web_searches_per_session,
        )
    }

    /// Atomically charge one Agent dispatch.
    pub fn try_record_agent_spawn(&self) -> UsageLimitDecision {
        try_consume(&self.agent_spawns, self.max_agent_spawns)
    }

    /// Atomically charge one WebSearch call.
    pub fn try_record_web_search(&self) -> UsageLimitDecision {
        try_consume(&self.web_searches, self.max_web_searches)
    }

    /// Number of Agent dispatches charged so far.
    pub fn agent_spawns(&self) -> i32 {
        self.agent_spawns.load(Ordering::Acquire)
    }

    /// Number of WebSearch calls charged so far.
    pub fn web_searches(&self) -> i32 {
        self.web_searches.load(Ordering::Acquire)
    }
}

impl Default for SessionUsageLimits {
    fn default() -> Self {
        Self::new(
            coco_config::DEFAULT_MAX_SUBAGENTS_PER_SESSION,
            coco_config::DEFAULT_MAX_WEB_SEARCHES_PER_SESSION,
        )
    }
}

fn try_consume(counter: &AtomicI32, limit: i32) -> UsageLimitDecision {
    match counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
        (used < limit).then_some(used + 1)
    }) {
        Ok(previous) => UsageLimitDecision::Allowed {
            used: previous + 1,
            limit,
        },
        Err(used) => UsageLimitDecision::Exhausted { used, limit },
    }
}

#[cfg(test)]
#[path = "session_usage.test.rs"]
mod tests;
