//! Boundary mapper: durable `coco_goals::GoalSnapshot` → wire
//! `coco_types::GoalSnapshotView` (design §8.1).
//!
//! Keeps `coco-types` free of a `coco-goals` dependency: the protocol/TUI
//! consumers see a bounded projection, mapped here at the agent-host boundary,
//! never reconstructed from transcript messages (design §9.1).

use coco_goals::{
    BlockerEvidence, BudgetKind, GoalLifecycle, GoalSnapshot, PauseReason, WaitCondition,
};
use coco_types::{GoalSnapshotView, GoalStatusKind};

/// Project a durable goal snapshot into its bounded wire view.
pub fn goal_snapshot_view(snapshot: &GoalSnapshot) -> GoalSnapshotView {
    GoalSnapshotView {
        goal_id: snapshot.goal_id.to_string(),
        spec_revision: u64::from(snapshot.spec_revision.get()),
        state_version: snapshot.state_version.get(),
        status: status_kind(&snapshot.lifecycle),
        status_detail: status_detail(&snapshot.lifecycle),
        objective: snapshot.objective.text.to_string(),
        total_turns: snapshot.counters.total_turns as i32,
        autonomous_turns: snapshot.counters.autonomous_turns as i32,
        max_autonomous_turns: snapshot.budget.max_autonomous_turns.get() as i32,
        input_tokens: snapshot.usage.input_tokens as i64,
        output_tokens: snapshot.usage.output_tokens as i64,
        progress_summary: snapshot
            .progress
            .as_ref()
            .map(|progress| progress.summary.to_string()),
        last_rejection: snapshot
            .last_rejection
            .as_ref()
            .map(|rejection| rejection.detail.to_string()),
        plan_digest: snapshot
            .plan
            .as_ref()
            .and_then(|plan| plan.content_digest.as_ref().map(ToString::to_string)),
        created_at_ms: snapshot.created_at.millis(),
        updated_at_ms: snapshot.updated_at.millis(),
    }
}

fn status_kind(lifecycle: &GoalLifecycle) -> GoalStatusKind {
    match lifecycle {
        GoalLifecycle::Active { .. } => GoalStatusKind::Active,
        GoalLifecycle::Waiting { .. } => GoalStatusKind::Waiting,
        GoalLifecycle::Paused { .. } => GoalStatusKind::Paused,
        GoalLifecycle::Blocked { .. } => GoalStatusKind::Blocked,
        GoalLifecycle::UsageLimited { .. } => GoalStatusKind::UsageLimited,
        GoalLifecycle::BudgetLimited { .. } => GoalStatusKind::BudgetLimited,
        GoalLifecycle::Completed { .. } => GoalStatusKind::Completed,
    }
}

/// Bounded human detail for a stopped/waiting status; `None` for a running or
/// completed goal, where the status alone is enough.
fn status_detail(lifecycle: &GoalLifecycle) -> Option<String> {
    match lifecycle {
        GoalLifecycle::Active { .. } | GoalLifecycle::Completed { .. } => None,
        GoalLifecycle::Paused { reason } => Some(pause_reason_detail(*reason).to_string()),
        GoalLifecycle::Blocked { evidence } => Some(blocker_detail(evidence)),
        GoalLifecycle::UsageLimited { reason } => Some(reason.detail.to_string()),
        GoalLifecycle::BudgetLimited { kind, .. } => Some(
            match kind {
                BudgetKind::Turns => "autonomous-turn budget exhausted",
                BudgetKind::Tokens => "token budget exhausted",
            }
            .to_string(),
        ),
        GoalLifecycle::Waiting { wake } => Some(wait_detail(&wake.condition)),
    }
}

fn pause_reason_detail(reason: PauseReason) -> &'static str {
    match reason {
        PauseReason::UserInterrupt => "paused by user",
        PauseReason::NoProgress => "paused: no progress for three turns",
        PauseReason::ContextUnavailable => "paused: goal context unavailable",
        PauseReason::VerificationUnavailable => "paused: completion verification unavailable",
        PauseReason::SchedulerUnavailable => "paused: autonomous scheduler unavailable",
        PauseReason::ApprovalRecoveryFailed => "paused: pending approval could not be recovered",
        PauseReason::RecoveryError => "paused: snapshot recovery error",
    }
}

fn blocker_detail(evidence: &BlockerEvidence) -> String {
    match evidence {
        BlockerEvidence::Dependency {
            required_change, ..
        } => format!("blocked: {required_change}"),
        BlockerEvidence::ExecutionError { message } => format!("blocked: {message}"),
    }
}

fn wait_detail(condition: &WaitCondition) -> String {
    match condition {
        WaitCondition::Task { task_ids } => {
            format!("waiting on {} background task(s)", task_ids.len())
        }
        WaitCondition::Deadline { .. } => "waiting for a deadline".to_string(),
        WaitCondition::Permission { .. } => "waiting on a permission approval".to_string(),
        WaitCondition::ModeGate { .. } => "waiting: plan/review mode gates execution".to_string(),
        WaitCondition::ProviderBackoff { attempt, .. } => {
            format!("waiting: provider backoff (attempt {attempt})")
        }
        WaitCondition::UserAcceptance => "awaiting completion acceptance".to_string(),
        WaitCondition::UsageReset { .. } => "waiting for a usage-limit reset".to_string(),
        WaitCondition::External { description } => format!("waiting: {description}"),
    }
}

#[cfg(test)]
#[path = "goal_view.test.rs"]
mod tests;
