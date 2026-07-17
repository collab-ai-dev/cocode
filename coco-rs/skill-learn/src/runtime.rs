//! Turn-end trigger for the skill-review fork.
//!
//! Mirrors memory's extract throttle: after every `throttle` eligible turns
//! *with material signal*, fire one background review fork (single-flight —
//! never overlap). The heavy work runs on a detached `tokio::spawn`, so the
//! engine's turn-end path stays non-blocking. The `fork_context` snapshot is
//! taken **only** when actually firing, so throttled turns pay no cost.
//!
//! Gating (feature flag, bare-mode, subagent) is decided by the engine before
//! calling in; this runtime owns the throttle + single-flight counters, the
//! per-turn signal gate, a failure backoff, and the user-visible notice inbox.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use coco_config::SkillLearnConfig;
use coco_tool_runtime::{AgentHandleRef, NoOpAgentHandle};
use coco_types::SessionId;
use coco_types::messages::Message;

use crate::curator::SkillCurator;
use crate::notice::{SkillLearnInbox, SkillLearnNotice};
use crate::review::{AgentSlot, SkillReviewService};

/// Eligible user-prompt cycles between review forks. The counter is
/// in-memory (resets each session), so this must be low enough that typical
/// sessions actually reach it — at 5, any session with five delivered
/// prompts gets at least one learning pass.
pub const DEFAULT_REVIEW_THROTTLE: i32 = 5;

/// Ceiling on the failure-backoff shift so a run of failures can't push the
/// effective throttle to absurd values.
const MAX_BACKOFF_SHIFT: i32 = 5;

/// Per-turn material-work signal the engine feeds the runtime (L4). An empty
/// signal skips the fork at zero cost, before the throttle even advances.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewSignal {
    /// Tool calls made in the last turn.
    pub tool_calls: i32,
    /// A skill was invoked this turn (strong signal a workflow just ran).
    pub skill_invoked: bool,
}

impl ReviewSignal {
    /// Whether there is enough material this turn to justify a review fork.
    fn is_material(self, min_tool_calls: i32) -> bool {
        self.skill_invoked || self.tool_calls >= min_tool_calls
    }
}

/// What [`SkillReviewRuntime::maybe_review`] decided this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTrigger {
    /// Turn ineligible (undelivered/interrupted, subagent, disabled, or no
    /// material signal this turn).
    Skipped,
    /// Below the throttle threshold; the counter advanced.
    Throttled,
    /// A review fork is already running; single-flight suppressed this one.
    InProgress,
    /// A background review fork was spawned.
    Spawned,
}

/// Why a review fork is running. The presence of a user directive is the only
/// axis that varies; both kinds advance the review cursor on completion, so
/// there is no mode flag to keep in sync.
enum ReviewKind {
    /// Turn-end pass over the unreviewed message delta.
    Auto,
    /// User-initiated `/learn`: the directive leads the prompt and created
    /// skills are stamped `created-by: manual`.
    Manual { directive: String },
}

impl ReviewKind {
    fn into_directive(self) -> Option<String> {
        match self {
            Self::Auto => None,
            Self::Manual { directive } => Some(directive),
        }
    }
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
    /// Resolved skill-learning knobs (throttle, signal threshold, curator).
    config: SkillLearnConfig,
    turns_since: AtomicI32,
    in_progress: Arc<AtomicBool>,
    /// Consecutive spawn failures; shifts the effective throttle (L4 backoff).
    consecutive_failures: Arc<AtomicI32>,
    /// Message index reviewed up to. Forks see only the delta after it, and it
    /// advances only on a `Completed` fork so a failed run re-reviews the same
    /// window (L4 cursor).
    cursor: Arc<AtomicUsize>,
    /// User-visible notice channel drained by the engine each turn (L1).
    notices: SkillLearnInbox,
}

impl SkillReviewRuntime {
    /// Build with default config and a no-op agent placeholder. Call
    /// [`Self::install_agent`] at bootstrap once the session handle exists.
    pub fn new(config_home: &Path) -> Self {
        Self::with_config(config_home, &SkillLearnConfig::default())
    }

    /// Build from resolved config (throttle, signal threshold, curator, max
    /// turns). The one production entry point; `new` / `with_throttle` defer
    /// to it.
    pub fn with_config(config_home: &Path, config: &SkillLearnConfig) -> Self {
        let agent: AgentSlot = Arc::new(RwLock::new(Arc::new(NoOpAgentHandle) as AgentHandleRef));
        let notices = SkillLearnInbox::new();
        let service = Arc::new(
            SkillReviewService::new(agent.clone(), config_home)
                .with_max_turns(config.review_max_turns)
                .with_journal_enabled(config.journal_enabled)
                .with_notices(notices.clone()),
        );
        Self {
            service,
            agent,
            config_home: config_home.to_path_buf(),
            config: config.clone(),
            turns_since: AtomicI32::new(0),
            in_progress: Arc::new(AtomicBool::new(false)),
            consecutive_failures: Arc::new(AtomicI32::new(0)),
            cursor: Arc::new(AtomicUsize::new(0)),
            notices,
        }
    }

    /// Build with an explicit throttle (`>= 1`), other knobs default. Retained
    /// for tests / callers that only vary the throttle.
    pub fn with_throttle(config_home: &Path, throttle: i32) -> Self {
        let config = SkillLearnConfig {
            review_throttle: throttle.max(1),
            ..SkillLearnConfig::default()
        };
        Self::with_config(config_home, &config)
    }

    /// Swap in the real session agent handle. Until this is called, review
    /// forks route to the no-op handle (harmless: spawns nothing useful).
    pub fn install_agent(&self, handle: AgentHandleRef) {
        *self.agent.write().unwrap_or_else(PoisonError::into_inner) = handle;
    }

    /// Drain the user-visible skill notices (called once per turn at finalize).
    pub fn drain_notices(&self) -> Vec<SkillLearnNotice> {
        self.notices.drain()
    }

    /// Effective throttle with failure backoff: `throttle << min(failures, 5)`.
    fn effective_throttle(&self) -> i32 {
        let failures = self
            .consecutive_failures
            .load(Ordering::SeqCst)
            .clamp(0, MAX_BACKOFF_SHIFT);
        self.config
            .review_throttle
            .checked_shl(failures as u32)
            .unwrap_or(i32::MAX)
            .max(1)
    }

    /// One-time session-start work. Pre-creates the agent skills root (the
    /// watcher no-ops on a missing path) and kicks a detached curator pass
    /// (time-gated + cross-process locked). Must run inside a Tokio runtime.
    pub fn bootstrap(&self) {
        let agent_root = coco_skills::agent_scope::agent_skills_dir(&self.config_home);
        if let Err(e) = std::fs::create_dir_all(agent_root) {
            tracing::warn!("could not pre-create agent skills dir: {e}");
        }
        if !self.config.curator_enabled {
            return;
        }
        let curator = self.curator();
        tokio::task::spawn_blocking(move || {
            let outcome = curator.maybe_curate();
            tracing::debug!(?outcome, "bootstrap curator pass");
        });
    }

    /// Build a curator configured from this runtime's knobs.
    fn curator(&self) -> SkillCurator {
        SkillCurator::with_config(&self.config_home, &self.config)
    }

    /// Decide whether to fire a review fork this turn.
    ///
    /// `signal` gates before the throttle: an empty signal skips at zero cost.
    /// `fork_context` is only invoked when firing. Must run inside a Tokio
    /// runtime (it detaches the fork via `tokio::spawn`).
    pub fn maybe_review(
        &self,
        signal: ReviewSignal,
        turn_delivered: bool,
        is_subagent: bool,
        session_id: &SessionId,
        fork_context: impl FnOnce() -> Vec<Arc<Message>>,
    ) -> ReviewTrigger {
        if !turn_delivered || is_subagent || !self.config.enabled {
            return ReviewTrigger::Skipped;
        }
        // L4: no material work this turn → skip before touching the throttle.
        if !signal.is_material(self.config.review_min_tool_calls) {
            return ReviewTrigger::Skipped;
        }
        let n = self.turns_since.fetch_add(1, Ordering::SeqCst) + 1;
        if n < self.effective_throttle() {
            return ReviewTrigger::Throttled;
        }
        if self.in_progress.swap(true, Ordering::SeqCst) {
            return ReviewTrigger::InProgress;
        }
        self.turns_since.store(0, Ordering::SeqCst);
        // L4 cursor: fork only the messages added since the last completed
        // review, so repeat passes don't re-pay for already-reviewed context.
        let full = fork_context();
        let start = self.review_start(full.len());
        let reviewed_upto = full.len();
        let delta = full[start..].to_vec();
        self.spawn_review_inner(session_id.clone(), delta, ReviewKind::Auto, reviewed_upto);
        ReviewTrigger::Spawned
    }

    /// Where this review should start reading.
    ///
    /// The cursor is an index into a *mutable* history: compaction, `/clear`,
    /// and rewind all replace the message list, after which the stored index no
    /// longer identifies the message it was taken at. Detect that by the list
    /// having shrunk past the cursor and fail safe by re-reading from the top —
    /// clamping to `len` instead would fork over an empty delta and then park
    /// the cursor at the new end, permanently skipping the post-compact window.
    fn review_start(&self, history_len: usize) -> usize {
        let cursor = self.cursor.load(Ordering::SeqCst);
        if cursor > history_len {
            tracing::debug!(
                cursor,
                history_len,
                "history shrank below the review cursor (compact/clear/rewind); \
                 resetting to review from the start"
            );
            self.cursor.store(0, Ordering::SeqCst);
            return 0;
        }
        cursor
    }

    /// User-initiated review (`/learn`): bypass the throttle + signal gate but
    /// respect single-flight. `directive` is injected as the top-priority
    /// instruction; skills it creates are stamped `created-by: manual`.
    pub fn manual_review(
        &self,
        directive: String,
        session_id: &SessionId,
        fork_context: Vec<Arc<Message>>,
    ) -> ReviewTrigger {
        if self.in_progress.swap(true, Ordering::SeqCst) {
            return ReviewTrigger::InProgress;
        }
        self.turns_since.store(0, Ordering::SeqCst);
        // A manual review sees the full slice the caller supplied, and advances
        // the cursor exactly like an automatic pass: the material is reviewed
        // either way, so leaving the cursor behind would make the next auto pass
        // re-review (and re-pay for) the window `/learn` just distilled.
        let reviewed_upto = fork_context.len();
        self.spawn_review_inner(
            session_id.clone(),
            fork_context,
            ReviewKind::Manual { directive },
            reviewed_upto,
        );
        ReviewTrigger::Spawned
    }

    /// `reviewed_upto` is stored into the cursor on a `Completed` fork (and
    /// left alone on failure, so the window is re-reviewed).
    fn spawn_review_inner(
        &self,
        session_id: SessionId,
        ctx: Vec<Arc<Message>>,
        kind: ReviewKind,
        reviewed_upto: usize,
    ) {
        let service = self.service.clone();
        let flag = self.in_progress.clone();
        let failures = self.consecutive_failures.clone();
        let cursor = self.cursor.clone();
        let config_home = self.config_home.clone();
        let config = self.config.clone();
        let directive = kind.into_directive();
        tokio::spawn(async move {
            // Drop guard, not a tail store: if `run` panics, the unwind must
            // still clear single-flight or every later review returns
            // `InProgress` forever.
            let _clear = ClearOnDrop(flag);
            match service.run(session_id, ctx, directive).await {
                crate::review::SkillReviewOutcome::Completed { paths_written } => {
                    failures.store(0, Ordering::SeqCst);
                    cursor.store(reviewed_upto, Ordering::SeqCst);
                    tracing::info!(paths_written, "skill review fork completed");
                }
                crate::review::SkillReviewOutcome::Failed { reason } => {
                    failures.fetch_add(1, Ordering::SeqCst);
                    tracing::warn!(reason, "skill review fork failed");
                }
            }
            // Piggybacked Curator tick so long-lived sessions curate too.
            if config.curator_enabled {
                let curated = tokio::task::spawn_blocking(move || {
                    SkillCurator::with_config(&config_home, &config).maybe_curate()
                })
                .await;
                tracing::debug!(?curated, "turn-end curator pass");
            }
        });
    }
}

#[cfg(test)]
#[path = "runtime.test.rs"]
mod tests;
