//! `SessionGoalHandle` — the concrete `GoalHandle` bridging the goal tools and the
//! engine turn loop to the session's `GoalRuntimeHandle`. Installed on the engine
//! via `with_goal_handle`.
//!
//! It owns the after-turn completion pipeline (coordinator + gate + evidence +
//! verifier). At a goal-owned turn's natural stop the engine calls
//! [`GoalHandle::finalize_goal_turn`]; this drains the worker report, runs the
//! coordinator, applies the domain `FinishTurn`, and — when the goal continues —
//! queues and binds the next turn so the engine loop drives autonomous progress
//! under the domain's budget/completion authority.

use std::sync::Arc;

use coco_goal_runtime::{
    AlwaysVerified, CompletionVerifier, EvidenceStore, GoalCompletionCoordinator,
    GoalRuntimeHandle, GoalTurnResult,
};
use coco_goals::{
    CompletionPolicy, CreateGoal, DurableResultRef, EvidenceId, EvidenceSource, FinishTurn,
    GoalBudget, GoalCommand, GoalEvidenceRecord, GoalId, GoalLease, GoalLeaseId, GoalLifecycle,
    GoalObjective, GoalSnapshot, GoalTurnDisposition, GoalTurnTrigger, ProgressSignal, StartTurn,
    Timestamp, UsageDelta, VerificationAttemptId, WakeId,
};
use coco_tool_runtime::{
    GoalContinuation, GoalCreateRequest, GoalHandle, GoalTurnFinalization, ToolEvidenceObservation,
};
use coco_types::TurnId;

/// How many recently-minted evidence records the goal-context reminder surfaces as
/// citable ids. Bounds the reminder; the store retains all records.
const GOAL_EVIDENCE_REMINDER_LIMIT: usize = 8;

/// Wraps the session's goal runtime plus the completion pipeline for the tool and
/// engine layers.
pub struct SessionGoalHandle {
    runtime: Arc<GoalRuntimeHandle>,
    evidence: Arc<dyn EvidenceStore>,
    verifier: Arc<dyn CompletionVerifier>,
    materializer: coco_goal_runtime::GoalContextMaterializer,
    /// Cold-edge signal for the goal driver (§10.3). Nudged when an engine-hook
    /// turn ends in a stopped state so the driver reconciles wakes — a warm goal
    /// that reports `waiting` would otherwise not register its timer until the
    /// next resume/restart.
    driver_edge: Arc<tokio::sync::Notify>,
}

impl SessionGoalHandle {
    /// `plan_source` resolves the goal's plan binding for the per-turn context
    /// (the session runtime passes a `SessionPlanSource` over its plan file;
    /// tests pass `NoPlanSource`). `evidence` is the session-scoped store, shared
    /// with the goal driver's coordinator so minted provenance survives across
    /// turns — a per-turn store would lose it every turn.
    pub fn new(
        runtime: Arc<GoalRuntimeHandle>,
        plan_source: Arc<dyn coco_goal_runtime::PlanSource>,
        evidence: Arc<dyn EvidenceStore>,
        driver_edge: Arc<tokio::sync::Notify>,
    ) -> Self {
        // Completion: the gate always runs the deterministic structural precheck
        // (coverage + evidence ownership + plan-drift), so `AlwaysVerified` means
        // "complete once structure holds" — the design's default
        // `candidate_with_evidence` behavior.
        Self {
            runtime,
            evidence,
            verifier: Arc::new(AlwaysVerified),
            materializer: coco_goal_runtime::GoalContextMaterializer::new(plan_source),
            driver_edge,
        }
    }

    fn coordinator(&self) -> GoalCompletionCoordinator {
        GoalCompletionCoordinator::new(Arc::clone(&self.evidence), Arc::clone(&self.verifier))
    }
}

fn now() -> Timestamp {
    Timestamp::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default(),
    )
}

fn mint_lease() -> GoalLeaseId {
    GoalLeaseId::new(format!("lease-{}", uuid::Uuid::new_v4()))
}

fn mint_wake() -> WakeId {
    WakeId::new(format!("wake-{}", uuid::Uuid::new_v4()))
}

fn mint_attempt() -> VerificationAttemptId {
    VerificationAttemptId::new(format!("va-{}", uuid::Uuid::new_v4()))
}

/// The deterministic `EvidenceId` the runtime issues for a tool result. The worker
/// cites this exact id — surfaced in the goal-context reminder — in
/// `report_goal_turn`; the completion gate resolves it to prove ownership.
fn evidence_id_for(tool_use_id: &str) -> EvidenceId {
    EvidenceId::new(format!("ev-{tool_use_id}"))
}

/// Goal-control tools are the reporting channel, not completion evidence, so their
/// results are never minted as citable evidence.
fn is_goal_control_tool(tool_name: &str) -> bool {
    use coco_types::ToolName;
    tool_name == ToolName::GetGoal.as_str()
        || tool_name == ToolName::ReportGoalTurn.as_str()
        || tool_name == ToolName::CreateGoal.as_str()
}

/// The running `(lease_id, turn_id)` if the snapshot is running a goal turn.
fn running_turn(snapshot: &GoalSnapshot) -> Option<(GoalLeaseId, TurnId)> {
    match &snapshot.lifecycle {
        GoalLifecycle::Active {
            lease: GoalLease::Running { lease_id, turn_id },
        } => Some((lease_id.clone(), turn_id.clone())),
        _ => None,
    }
}

/// The queued lease id if the snapshot is active and queued.
fn queued_lease(snapshot: &GoalSnapshot) -> Option<GoalLeaseId> {
    match &snapshot.lifecycle {
        GoalLifecycle::Active {
            lease: GoalLease::Queued { lease_id, .. },
        } => Some(lease_id.clone()),
        _ => None,
    }
}

/// A concise transcript cell for a durable goal transition (§9.2), or `None` for
/// an ordinary active continuation (which is not a status change). Reuses the
/// bounded snapshot projection for the condition/reason text.
fn transition_cell(snapshot: &GoalSnapshot) -> Option<coco_types::GoalStatusPayload> {
    use coco_goals::GoalStatus;
    let view = crate::session::goal_view::goal_snapshot_view(snapshot);
    let base = coco_types::GoalStatusPayload {
        condition: view.objective,
        iterations: Some(view.total_turns),
        tokens: Some(view.output_tokens),
        reason: view.status_detail,
        ..Default::default()
    };
    match snapshot.status() {
        // An active goal is continuing, not transitioning — no cell.
        GoalStatus::Active => None,
        GoalStatus::Completed => Some(coco_types::GoalStatusPayload {
            met: true,
            reason: None,
            ..base
        }),
        GoalStatus::Blocked => Some(coco_types::GoalStatusPayload {
            failed: true,
            ..base
        }),
        GoalStatus::Waiting
        | GoalStatus::Paused
        | GoalStatus::BudgetLimited
        | GoalStatus::UsageLimited => Some(base),
    }
}

#[async_trait::async_trait]
impl GoalHandle for SessionGoalHandle {
    async fn snapshot(&self) -> Option<GoalSnapshot> {
        self.runtime.snapshot().await
    }

    async fn has_live_goal(&self) -> bool {
        self.runtime.has_live_goal().await
    }

    async fn report_turn(&self, disposition: GoalTurnDisposition) -> Result<(), String> {
        if !self.runtime.has_live_goal_sync() {
            return Err("no active goal to report against in this turn".to_string());
        }
        self.runtime.set_pending_report(disposition).await;
        Ok(())
    }

    async fn create_goal(&self, request: GoalCreateRequest) -> Result<(), String> {
        let command = GoalCommand::Create(CreateGoal {
            goal_id: GoalId::new(format!("goal-{}", uuid::Uuid::new_v4())),
            session_id: self.runtime.session_id().clone(),
            lease_id: mint_lease(),
            objective: GoalObjective::new(request.objective),
            contract: None,
            policy: CompletionPolicy::CandidateWithEvidence,
            budget: GoalBudget::default(),
            plan: None,
            mode_gate: None,
            wake_id: mint_wake(),
            at: now(),
        });
        self.runtime
            .apply(command)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn record_tool_evidence(&self, observation: ToolEvidenceObservation) {
        // Goal-control tools report progress; their results are not evidence.
        if is_goal_control_tool(&observation.tool_name) {
            return;
        }
        let Some(snapshot) = self.runtime.snapshot().await else {
            return;
        };
        // Provenance binds to the running goal-owned turn (goal/lease/turn); with
        // no running goal turn there is nothing to own the record.
        let Some((lease_id, turn_id)) = running_turn(&snapshot) else {
            return;
        };
        let record = GoalEvidenceRecord {
            evidence_id: evidence_id_for(&observation.tool_use_id),
            goal_id: snapshot.goal_id,
            lease_id,
            turn_id,
            source: EvidenceSource::ToolResult {
                tool: observation.tool_name,
            },
            // The durable tool result already lives indexed under its id.
            result_ref: DurableResultRef::new(&observation.tool_use_id),
            content_digest: None,
            observed_at: now(),
        };
        // Best-effort: minting provenance must never fail a turn.
        let _ = self.evidence.record(record);
    }

    async fn bind_turn(&self, turn_id: String) -> bool {
        let Some(snapshot) = self.runtime.snapshot().await else {
            return false;
        };
        let Some(lease_id) = queued_lease(&snapshot) else {
            // Already running, or not active: nothing to bind.
            return snapshot.is_active();
        };
        // A user-initiated goal turn: does not spend the autonomous quota.
        let command = GoalCommand::StartTurn(StartTurn {
            goal_id: snapshot.goal_id.clone(),
            lease_id,
            turn_id: TurnId::new(turn_id),
            trigger: GoalTurnTrigger::UserInput,
            at: now(),
        });
        self.runtime.apply(command).await.is_ok()
    }

    async fn finalize_goal_turn(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        signals_present: bool,
    ) -> GoalTurnFinalization {
        let Some(snapshot) = self.runtime.snapshot().await else {
            return GoalTurnFinalization::stop();
        };
        let Some((lease_id, turn_id)) = running_turn(&snapshot) else {
            // The turn was not goal-owned (or the goal stopped concurrently).
            return GoalTurnFinalization::stop();
        };

        let disposition = self
            .runtime
            .take_pending_report()
            .await
            .unwrap_or(GoalTurnDisposition::Unreported);
        let reported = disposition.is_reported();
        let signals = if signals_present {
            vec![ProgressSignal::ToolObservation]
        } else {
            Vec::new()
        };
        let usage = UsageDelta {
            input_tokens,
            output_tokens,
            duration_ms: 0,
        };

        let result = GoalTurnResult {
            disposition,
            signals: signals.clone(),
            next_lease_id: mint_lease(),
            wake_id: mint_wake(),
            verification_attempt: mint_attempt(),
            at: now(),
        };
        let coordinated = match self.coordinator().coordinate(&snapshot, result).await {
            Ok(outcome) => outcome,
            Err(_) => return GoalTurnFinalization::stop(),
        };

        let finish = GoalCommand::FinishTurn(FinishTurn {
            goal_id: snapshot.goal_id.clone(),
            lease_id,
            turn_id,
            reported,
            signals,
            usage,
            outcome: coordinated.outcome,
            at: now(),
        });
        let applied = match self.runtime.apply(finish).await {
            Ok(applied) => applied,
            Err(_) => return GoalTurnFinalization::stop(),
        };

        // A concise transcript cell for the durable transition this turn enacted
        // (§9.2), read from the post-finish state.
        let transition = applied.snapshot.as_ref().and_then(transition_cell);

        // §10.3: the engine runs exactly one logical turn and never self-continues.
        // The goal driver owns continuation — for a user-started turn the AppServer
        // forwarder nudges the driver once this turn's slot frees, so it advances
        // any queued autonomous turn or registers a wake without a slot race. Nudge
        // here as well so a stop is reconciled promptly; return Stop unconditionally
        // so the engine loop ends after this turn.
        self.driver_edge.notify_one();
        GoalTurnFinalization {
            continuation: GoalContinuation::Stop,
            transition,
        }
    }

    async fn goal_snapshot_view(&self) -> Option<coco_types::GoalSnapshotView> {
        let snapshot = self.runtime.snapshot().await?;
        if snapshot.is_terminal() {
            return None;
        }
        Some(crate::session::goal_view::goal_snapshot_view(&snapshot))
    }

    async fn goal_context_fragment(&self) -> Option<String> {
        let snapshot = self.runtime.snapshot().await?;
        // Only a running goal-owned turn re-injects its context (§5.5).
        snapshot.lifecycle.running_lease_id()?;
        let context = self.materializer.materialize(&snapshot).ok()?;
        let mut fragment = crate::session::goal_reminder::render_goal_context(&context);
        // Surface the ids the runtime issued this goal so the worker can cite them
        // as completion evidence in `report_goal_turn` (§10.2 #9).
        if let Ok(records) = self
            .evidence
            .recent_for_goal(&snapshot.goal_id, GOAL_EVIDENCE_REMINDER_LIMIT)
            && !records.is_empty()
        {
            fragment.push_str(&crate::session::goal_reminder::render_goal_evidence(
                &records,
            ));
        }
        Some(fragment)
    }

    fn is_available(&self) -> bool {
        self.runtime.has_live_goal_sync()
    }
}

#[cfg(test)]
#[path = "goal_tool_handle.test.rs"]
mod tests;
