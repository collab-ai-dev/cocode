//! Goal control-plane request handlers (design §8.1–8.2).
//!
//! Map `session/goal/{create,get,edit,setStatus,clear}` RPCs to
//! `GoalRuntimeHandle` commands, emit `GoalSnapshotChanged` after the durable
//! commit, and return the resulting bounded snapshot view. Session routing and
//! validation are already done by AppServer; these handlers own only the
//! command mapping and event emission.

use std::num::NonZeroU32;

use coco_goals::{
    Clear, CreateGoal, Edit, GoalBudget, GoalCommand, GoalObjective, GoalSnapshot, Pause,
    PauseReason, Resume,
};
use coco_types::{
    CoreEvent, GoalCommandResult, GoalCreateParams, GoalEditParams, GoalSetStatusParams,
    GoalSnapshotChangedParams, GoalStatusRequest, ServerNotification,
};

use crate::app_server_host::outbound::send_session_event;
use crate::app_server_host::{HandlerContext, HandlerResult};
use crate::session::goal_view::goal_snapshot_view;
use crate::session_runtime::SessionHandle;

fn now() -> coco_goals::Timestamp {
    coco_goals::Timestamp::from_millis(crate::goal_command::unix_time_ms())
}

fn mint_goal_id() -> coco_goals::GoalId {
    coco_goals::GoalId::new(format!("goal-{}", uuid::Uuid::new_v4()))
}

fn mint_lease() -> coco_goals::GoalLeaseId {
    coco_goals::GoalLeaseId::new(format!("lease-{}", uuid::Uuid::new_v4()))
}

fn mint_wake() -> coco_goals::WakeId {
    coco_goals::WakeId::new(format!("wake-{}", uuid::Uuid::new_v4()))
}

fn invalid_request(message: impl Into<String>) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: message.into(),
        data: None,
    }
}

async fn runtime_or_err(ctx: &HandlerContext) -> Result<SessionHandle, HandlerResult> {
    ctx.resolve_runtime()
        .await
        .ok_or_else(|| invalid_request("no session runtime installed for this goal request"))
}

/// Emit `GoalSnapshotChanged` for the session and build the RPC result from the
/// post-commit snapshot (`None` after a clear).
async fn commit_and_result(
    session: &SessionHandle,
    ctx: &HandlerContext,
    snapshot: Option<GoalSnapshot>,
) -> HandlerResult {
    let view = snapshot.as_ref().map(goal_snapshot_view);
    let _ = send_session_event(
        &ctx.notif_tx,
        session.session_id().clone(),
        CoreEvent::Protocol(ServerNotification::GoalSnapshotChanged(Box::new(
            GoalSnapshotChangedParams {
                snapshot: view.clone(),
            },
        ))),
    )
    .await;
    HandlerResult::ok(GoalCommandResult { snapshot: view })
}

fn budget_turns(max_autonomous_turns: Option<i32>) -> Option<NonZeroU32> {
    max_autonomous_turns
        .and_then(|turns| u32::try_from(turns).ok())
        .and_then(NonZeroU32::new)
}

pub async fn handle_goal_get(ctx: &HandlerContext) -> HandlerResult {
    let session = match runtime_or_err(ctx).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let view = session
        .goal_runtime()
        .snapshot()
        .await
        .as_ref()
        .map(goal_snapshot_view);
    HandlerResult::ok(GoalCommandResult { snapshot: view })
}

pub async fn handle_goal_create(params: GoalCreateParams, ctx: &HandlerContext) -> HandlerResult {
    let session = match runtime_or_err(ctx).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let goal = session.goal_runtime();
    // Replace any existing unfinished goal (design §9.1 active replacement).
    if let Some(existing) = goal.snapshot().await
        && !existing.is_terminal()
    {
        let _ = goal
            .apply(GoalCommand::Clear(Clear {
                goal_id: existing.goal_id.clone(),
                at: now(),
            }))
            .await;
    }
    let mut budget = GoalBudget::default();
    if let Some(turns) = budget_turns(params.max_autonomous_turns) {
        budget.max_autonomous_turns = turns;
    }
    // Bind the session plan artifact when a plan file exists (design §5.5).
    let plan = crate::session::goal_plan::current_plan_ref(
        session.session_id().as_str(),
        &session.session_plan_file_path(),
        now(),
    );
    let command = GoalCommand::Create(CreateGoal {
        goal_id: mint_goal_id(),
        session_id: session.session_id().clone(),
        lease_id: mint_lease(),
        objective: GoalObjective::new(&params.objective),
        contract: None,
        policy: coco_goals::CompletionPolicy::CandidateWithEvidence,
        budget,
        plan,
        mode_gate: None,
        wake_id: mint_wake(),
        at: now(),
    });
    match goal.apply(command).await {
        Ok(applied) => commit_and_result(&session, ctx, applied.snapshot).await,
        Err(err) => invalid_request(format!("failed to create goal: {err}")),
    }
}

pub async fn handle_goal_edit(params: GoalEditParams, ctx: &HandlerContext) -> HandlerResult {
    let session = match runtime_or_err(ctx).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let goal = session.goal_runtime();
    let Some(current) = goal.snapshot().await else {
        return invalid_request("no goal to edit");
    };
    // Optimistic concurrency guard (design §9.1 invariant 2). `SpecRevision`
    // has no public constructor — it is minted only by the reducer — so the
    // wire-level check happens here and the current revision is handed on.
    if params.expected_spec_revision != u64::from(current.spec_revision.get()) {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "goal spec revision conflict: expected {}, current {}",
                params.expected_spec_revision,
                current.spec_revision.get(),
            ),
            data: Some(serde_json::json!({ "kind": "spec_revision_conflict" })),
        };
    }
    let budget = budget_turns(params.max_autonomous_turns).map(|turns| GoalBudget {
        max_autonomous_turns: turns,
        ..current.budget
    });
    // A budget raise on a budget-limited goal atomically resumes it (§9.1).
    let next_lease_id = (budget.is_some()
        && current.status() == coco_goals::GoalStatus::BudgetLimited)
        .then(mint_lease);
    let command = GoalCommand::Edit(Edit {
        goal_id: current.goal_id.clone(),
        expected_spec_revision: current.spec_revision,
        objective: params.objective.map(GoalObjective::new),
        contract: None,
        clear_contract: false,
        policy: None,
        budget,
        plan_binding: None,
        next_lease_id,
        at: now(),
    });
    match goal.apply(command).await {
        Ok(applied) => commit_and_result(&session, ctx, applied.snapshot).await,
        Err(err) => invalid_request(format!("failed to edit goal: {err}")),
    }
}

pub async fn handle_goal_set_status(
    params: GoalSetStatusParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let session = match runtime_or_err(ctx).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let goal = session.goal_runtime();
    let Some(current) = goal.snapshot().await else {
        return invalid_request("no goal to update");
    };
    let is_resume = matches!(params.status, GoalStatusRequest::Resume);
    let command = match params.status {
        GoalStatusRequest::Pause => GoalCommand::Pause(Pause {
            goal_id: current.goal_id.clone(),
            reason: PauseReason::UserInterrupt,
            at: now(),
        }),
        GoalStatusRequest::Resume => GoalCommand::Resume(Resume {
            goal_id: current.goal_id.clone(),
            next_lease_id: mint_lease(),
            at: now(),
        }),
    };
    match goal.apply(command).await {
        Ok(applied) => {
            if is_resume {
                // Nudge the continuation driver (§10.3) so the now active+queued
                // goal starts a turn; resume alone is state-only.
                session.goal_driver_edge().notify_one();
            }
            commit_and_result(&session, ctx, applied.snapshot).await
        }
        Err(err) => invalid_request(format!("failed to update goal status: {err}")),
    }
}

pub async fn handle_goal_clear(ctx: &HandlerContext) -> HandlerResult {
    let session = match runtime_or_err(ctx).await {
        Ok(session) => session,
        Err(err) => return err,
    };
    let goal = session.goal_runtime();
    let Some(current) = goal.snapshot().await else {
        return commit_and_result(&session, ctx, None).await;
    };
    match goal
        .apply(GoalCommand::Clear(Clear {
            goal_id: current.goal_id.clone(),
            at: now(),
        }))
        .await
    {
        Ok(applied) => commit_and_result(&session, ctx, applied.snapshot).await,
        Err(err) => invalid_request(format!("failed to clear goal: {err}")),
    }
}
