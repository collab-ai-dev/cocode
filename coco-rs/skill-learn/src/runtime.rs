//! Turn-end trigger for the skill-review fork.
//!
//! Mirrors memory's extract throttle: after every `throttle` eligible turns,
//! fire one background review fork (single-flight — never overlap). The heavy
//! work runs on a detached `tokio::spawn`, so the engine's turn-end path stays
//! non-blocking. The `fork_context` snapshot is taken **only** when actually
//! firing, so throttled turns pay no cost.
//!
//! Gating (feature flag, bare-mode, subagent) is decided by the engine before
//! calling in; this runtime owns only the throttle + single-flight counters.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use coco_tool_runtime::{AgentHandleRef, NoOpAgentHandle};
use coco_types::SessionId;
use coco_types::messages::Message;

use crate::curator::SkillCurator;
use crate::review::{AgentSlot, SkillReviewService};

/// Eligible user-prompt cycles between review forks. The counter is
/// in-memory (resets each session), so this must be low enough that typical
/// sessions actually reach it — at 5, any session with five delivered
/// prompts gets at least one learning pass.
pub const DEFAULT_REVIEW_THROTTLE: i32 = 5;

/// What [`SkillReviewRuntime::maybe_review`] decided this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTrigger {
    /// Turn ineligible (undelivered/interrupted, or a subagent turn).
    Skipped,
    /// Below the throttle threshold; the counter advanced.
    Throttled,
    /// A review fork is already running; single-flight suppressed this one.
    InProgress,
    /// A background review fork was spawned.
    Spawned,
}

/// Clears the single-flight flag when the detached review task ends by any
/// path — completion, error, or panic unwind.
struct ClearOnDrop(Arc<AtomicBool>);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Owns the review throttle + single-flight state and spawns review forks.
pub struct SkillReviewRuntime {
    service: Arc<SkillReviewService>,
    /// Shared with `service`; swapped by [`Self::install_agent`] once the real
    /// session `AgentHandle` is ready (mirrors `MemoryRuntime::install_agent`).
    agent: AgentSlot,
    /// For the piggybacked Curator tick (see [`Self::maybe_review`]).
    config_home: PathBuf,
    throttle: i32,
    turns_since: AtomicI32,
    in_progress: Arc<AtomicBool>,
}

impl SkillReviewRuntime {
    /// Build with the default throttle and a no-op agent placeholder. Call
    /// [`Self::install_agent`] at bootstrap once the session handle exists.
    pub fn new(config_home: &Path) -> Self {
        Self::with_throttle(config_home, DEFAULT_REVIEW_THROTTLE)
    }

    /// Build with an explicit throttle (`>= 1`).
    pub fn with_throttle(config_home: &Path, throttle: i32) -> Self {
        let agent: AgentSlot = Arc::new(RwLock::new(Arc::new(NoOpAgentHandle) as AgentHandleRef));
        Self {
            service: Arc::new(SkillReviewService::new(agent.clone(), config_home)),
            agent,
            config_home: config_home.to_path_buf(),
            throttle: throttle.max(1),
            turns_since: AtomicI32::new(0),
            in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Swap in the real session agent handle. Until this is called, review
    /// forks route to the no-op handle (harmless: spawns nothing useful).
    pub fn install_agent(&self, handle: AgentHandleRef) {
        *self.agent.write().unwrap_or_else(PoisonError::into_inner) = handle;
    }

    /// One-time session-start work. Pre-creates the agent skills root — the
    /// skill watcher's `try_watch` no-ops on a non-existent path and never
    /// re-arms, so without this a first-run session (feature on, no prior
    /// skills dir) would get no hot-reload for skills the review fork writes
    /// this session. The review fork also creates it lazily, but that runs
    /// after the watcher spawns. Then kicks a detached curator pass
    /// (time-gated + cross-process locked, so it's cheap when it recently
    /// ran). Must be called from within a Tokio runtime.
    pub fn bootstrap(&self) {
        let agent_root = coco_skills::agent_scope::agent_skills_dir(&self.config_home);
        if let Err(e) = std::fs::create_dir_all(agent_root) {
            tracing::warn!("could not pre-create agent skills dir: {e}");
        }
        let config_home = self.config_home.clone();
        tokio::task::spawn_blocking(move || {
            let outcome = SkillCurator::new(&config_home).maybe_curate();
            tracing::debug!(?outcome, "bootstrap curator pass");
        });
    }

    /// Decide whether to fire a review fork this turn.
    ///
    /// `fork_context` is only invoked when firing, so a throttled/ineligible
    /// turn never pays for the message-history snapshot. Must be called from
    /// within a Tokio runtime (it detaches the fork via `tokio::spawn`).
    pub fn maybe_review(
        &self,
        turn_delivered: bool,
        is_subagent: bool,
        session_id: &SessionId,
        fork_context: impl FnOnce() -> Vec<Arc<Message>>,
    ) -> ReviewTrigger {
        if !turn_delivered || is_subagent {
            return ReviewTrigger::Skipped;
        }
        let n = self.turns_since.fetch_add(1, Ordering::SeqCst) + 1;
        if n < self.throttle {
            return ReviewTrigger::Throttled;
        }
        // Claim single-flight. If a prior review is still running, leave the
        // counter elevated so the next eligible turn retries.
        if self.in_progress.swap(true, Ordering::SeqCst) {
            return ReviewTrigger::InProgress;
        }
        self.turns_since.store(0, Ordering::SeqCst);
        // Only a firing turn pays for the clone (same lazy contract as
        // `fork_context`).
        let session_id = session_id.clone();
        let ctx = fork_context();
        let service = self.service.clone();
        let flag = self.in_progress.clone();
        let config_home = self.config_home.clone();
        tokio::spawn(async move {
            // Drop guard, not a tail store: if `run` panics, the unwind must
            // still clear single-flight or every later review this session
            // returns `InProgress` forever.
            let _clear = ClearOnDrop(flag);
            match service.run(session_id, ctx).await {
                crate::review::SkillReviewOutcome::Completed { paths_written } => {
                    tracing::info!(paths_written, "skill review fork completed");
                }
                crate::review::SkillReviewOutcome::Failed { reason } => {
                    tracing::warn!(reason, "skill review fork failed");
                }
            }
            // Piggybacked Curator tick, so long-lived sessions curate too
            // (bootstrap-only curation would never advance the 24h cadence
            // in a session that stays up for days). The time gate is one
            // fs stat, and this path already runs at most once per
            // `throttle` cycles.
            let curated =
                tokio::task::spawn_blocking(move || SkillCurator::new(&config_home).maybe_curate())
                    .await;
            tracing::debug!(?curated, "turn-end curator pass");
        });
        ReviewTrigger::Spawned
    }
}

#[cfg(test)]
#[path = "runtime.test.rs"]
mod tests;
