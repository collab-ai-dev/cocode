//! `GoalHandle` — the tool-facing seam onto the session's goal runtime.
//!
//! Definition here; implementation in `coco-agent-host` over the session's
//! `GoalRuntimeHandle`. Mirrors the `AgentHandle` / `MailboxHandle` pattern:
//! `coco-tool-runtime` gains a one-way leaf dep on `coco-goals` for the domain
//! types, and never depends on `coco-goal-runtime`. Tools reach it via
//! `ToolUseContext.goal`.

use std::sync::Arc;

use coco_goals::{GoalSnapshot, GoalTurnDisposition};

/// A worker's create-goal request (the `create_goal` tool). Kept minimal — full
/// contract/policy/budget authoring is a control-plane concern (`/goal edit`).
#[derive(Debug, Clone)]
pub struct GoalCreateRequest {
    pub objective: String,
}

/// A durable tool result the runtime accepted during a goal-owned turn, from which
/// it mints runtime-owned evidence provenance (design §10.2 #9). Provenance is
/// captured when the result is produced — not when it is cited — so a report-time
/// wrapper around an old result cannot acquire fresh ownership. The worker may
/// later cite the minted `EvidenceId` via `report_goal_turn` but can never mint or
/// rebind one.
#[derive(Debug, Clone)]
pub struct ToolEvidenceObservation {
    /// The tool call's id. The minted `EvidenceId` derives deterministically from
    /// it, and the durable tool result is already indexed under it.
    pub tool_use_id: String,
    /// The tool that produced the result (e.g. `Bash`, `Edit`).
    pub tool_name: String,
}

/// Whether the goal wants another logical turn after finalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalContinuation {
    /// The goal queued another turn; the engine should continue the loop.
    Continue,
    /// The goal reached a terminal/stopped/waiting state; end the run.
    Stop,
}

/// Result of finalizing a goal-owned turn: the continuation decision plus an
/// optional concise transcript cell for the durable transition it enacted
/// (completed / blocked / paused / budget / usage), so the transcript carries a
/// permanent record of each goal transition (design §9.2). `None` for ordinary
/// progress continuation, which needs no cell.
#[derive(Debug, Clone)]
pub struct GoalTurnFinalization {
    pub continuation: GoalContinuation,
    pub transition: Option<coco_types::GoalStatusPayload>,
}

impl GoalTurnFinalization {
    /// A stop with no transcript cell (the non-goal-owned / no-op case).
    pub fn stop() -> Self {
        Self {
            continuation: GoalContinuation::Stop,
            transition: None,
        }
    }
}

/// Tool-facing operations on the session's goal runtime.
#[async_trait::async_trait]
pub trait GoalHandle: Send + Sync {
    /// The current durable goal snapshot, or `None` when no goal exists
    /// (`get_goal`).
    async fn snapshot(&self) -> Option<GoalSnapshot>;

    /// Whether a live (non-terminal) goal currently exists. Gates
    /// `report_goal_turn` / `get_goal` visibility.
    async fn has_live_goal(&self) -> bool;

    /// Record the worker's disposition for the current goal-owned turn. The
    /// coordinator evaluates it at turn finalization; this only stores the
    /// candidate. Errors when the current turn is not goal-owned.
    async fn report_turn(&self, disposition: GoalTurnDisposition) -> Result<(), String>;

    /// Create a goal on explicit user/system request (`create_goal`).
    async fn create_goal(&self, request: GoalCreateRequest) -> Result<(), String>;

    /// Mint runtime-owned evidence for a durable tool result accepted during a
    /// goal-owned turn (design §10.2 #9). A no-op unless a goal turn is running
    /// (and for goal-control tools, which are not evidence). The engine calls this
    /// for every successful tool completion; the handle self-gates. The worker
    /// later cites the resulting id via `report_goal_turn`, and the completion gate
    /// resolves it to prove ownership — a fabricated id fails closed.
    async fn record_tool_evidence(&self, _observation: ToolEvidenceObservation) {}

    /// Bind a starting turn to the active goal if one is queued (applies the
    /// domain `StartTurn`). Idempotent; a no-op when no goal is queued. Returns
    /// whether the turn is now goal-owned.
    async fn bind_turn(&self, _turn_id: String) -> bool {
        false
    }

    /// Finalize a goal-owned turn at its natural stop: drain the worker report,
    /// run the completion coordinator, apply the domain `FinishTurn`, and queue
    /// the next turn if the goal continues. `signals_present` is whether the turn
    /// produced accepted progress signals (tool activity). Returns whether the
    /// engine should continue the loop.
    async fn finalize_goal_turn(
        &self,
        _input_tokens: u64,
        _output_tokens: u64,
        _signals_present: bool,
    ) -> GoalTurnFinalization {
        GoalTurnFinalization::stop()
    }

    /// The full bounded snapshot view of the current goal for the protocol/TUI
    /// `GoalSnapshotChanged` event (detail view, composed footer, resume
    /// prompts), or `None` when no goal exists (design §8.1).
    async fn goal_snapshot_view(&self) -> Option<coco_types::GoalSnapshotView> {
        None
    }

    /// The bounded, re-materialized goal-context reminder body for a goal-owned
    /// turn — objective, budget, progress, plan digest, and the
    /// `report_goal_turn` protocol — or `None` when no goal turn is running
    /// (design §5.5). The engine injects it as a per-turn `goal_context`
    /// reminder so compaction cannot erase the objective. Untrusted objective
    /// text is escaped by the renderer, kept separate from static instructions.
    async fn goal_context_fragment(&self) -> Option<String> {
        None
    }

    /// Cheap sync availability check for `Tool::is_enabled` gating: whether this
    /// session owns a usable goal runtime at all.
    fn is_available(&self) -> bool {
        true
    }
}

/// Shared tool-facing handle.
pub type GoalHandleRef = Arc<dyn GoalHandle>;

/// No-op handle for contexts without a goal runtime (tests, non-goal surfaces).
/// Reports no goal and rejects mutations, so goal tools stay hidden via
/// `is_available()`.
pub struct NoOpGoalHandle;

#[async_trait::async_trait]
impl GoalHandle for NoOpGoalHandle {
    async fn snapshot(&self) -> Option<GoalSnapshot> {
        None
    }

    async fn has_live_goal(&self) -> bool {
        false
    }

    async fn report_turn(&self, _disposition: GoalTurnDisposition) -> Result<(), String> {
        Err("goal runtime is not available in this context".to_string())
    }

    async fn create_goal(&self, _request: GoalCreateRequest) -> Result<(), String> {
        Err("goal runtime is not available in this context".to_string())
    }

    fn is_available(&self) -> bool {
        false
    }
}
