use std::sync::Mutex;

use super::*;
use coco_goal_runtime::{
    AlwaysVerified, GoalCompletionCoordinator, InMemoryEvidenceStore, InMemoryGoalStore,
    NoPlanSource,
};
use coco_goals::{
    CompletionPolicy, CreateGoal, GoalBudget, GoalId, GoalObjective, GoalStatus, WaitCondition,
};
use coco_types::SessionId;
use pretty_assertions::assert_eq;

fn ts(n: i64) -> Timestamp {
    Timestamp::from_millis(n)
}

/// A port whose turns report a configured disposition, so the driver can be exercised
/// without a real engine.
struct ScriptedPort {
    disposition: Mutex<Option<GoalTurnDisposition>>,
}

impl ScriptedPort {
    fn new(disposition: GoalTurnDisposition) -> Self {
        Self {
            disposition: Mutex::new(Some(disposition)),
        }
    }
}

#[async_trait::async_trait]
impl SessionTurnPort for ScriptedPort {
    async fn start_goal_turn(
        &self,
        request: GoalTurnRequest,
    ) -> coco_goal_runtime::Result<GoalTurnHandle> {
        let disposition = self
            .disposition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or(GoalTurnDisposition::Unreported);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(GoalTurnOutcome::Ended {
            disposition,
            signals: vec![ProgressSignal::ToolObservation],
            usage: UsageDelta::default(),
        });
        Ok(GoalTurnHandle {
            turn_id: request.turn_id,
            completion: GoalTurnCompletion::new(rx),
        })
    }
}

/// Records every wake the driver asks to schedule.
#[derive(Default)]
struct RecordingScheduler {
    scheduled: Mutex<Vec<WaitCondition>>,
}

impl WakeScheduler for RecordingScheduler {
    fn schedule(&self, wake: GoalWake, _on_fire: Box<dyn FnOnce() + Send>) {
        self.scheduled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(wake.condition);
    }
}

/// A burst scheduler that runs the burst immediately without a real turn slot. The
/// driver-loop tests call `reconcile_once` directly, so this only satisfies the
/// constructor.
struct ImmediateBurstScheduler;

impl BurstScheduler for ImmediateBurstScheduler {
    fn schedule(
        &self,
        burst: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    ) -> bool {
        tokio::spawn(burst);
        true
    }
}

async fn active_goal_runtime() -> Arc<GoalRuntimeHandle> {
    let store = Arc::new(InMemoryGoalStore::new());
    let sid = SessionId::try_new("goal-driver-test").expect("session id");
    let runtime = Arc::new(GoalRuntimeHandle::new(sid.clone(), store, None));
    runtime
        .apply(GoalCommand::Create(CreateGoal {
            goal_id: GoalId::new("goal-1"),
            session_id: sid,
            lease_id: coco_goals::GoalLeaseId::new("lease-0"),
            objective: GoalObjective::new("ship it"),
            contract: None,
            policy: CompletionPolicy::CandidateWithEvidence,
            budget: GoalBudget::default(),
            plan: None,
            mode_gate: None,
            wake_id: coco_goals::WakeId::new("wake-0"),
            at: ts(1),
        }))
        .await
        .expect("create goal");
    runtime
}

fn driver(
    runtime: Arc<GoalRuntimeHandle>,
    port: Arc<dyn SessionTurnPort>,
    scheduler: Arc<RecordingScheduler>,
) -> GoalDriver {
    GoalDriver::new(
        runtime,
        port,
        Arc::new(GoalContextMaterializer::new(Arc::new(NoPlanSource))),
        Arc::new(GoalCompletionCoordinator::new(
            Arc::new(InMemoryEvidenceStore::new()),
            Arc::new(AlwaysVerified),
        )),
        AutonomousAdmission::new(1),
        scheduler,
        Arc::new(ImmediateBurstScheduler),
        Arc::new(tokio::sync::Notify::new()),
    )
}

#[tokio::test]
async fn test_reconcile_advances_then_registers_deadline_wake() {
    let runtime = active_goal_runtime().await;
    // The one autonomous turn reports a deadline wait, so the goal parks waiting.
    let port = Arc::new(ScriptedPort::new(GoalTurnDisposition::Waiting {
        condition: WaitCondition::Deadline { deadline: ts(9999) },
    }));
    let scheduler = Arc::new(RecordingScheduler::default());
    let driver = driver(runtime.clone(), port, scheduler.clone());

    driver.reconcile_once().await;

    // The goal advanced one turn and parked in `waiting`.
    let snapshot = runtime.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.status(), GoalStatus::Waiting);
    // The driver registered exactly one wake watcher for the deadline.
    let scheduled = scheduler
        .scheduled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(scheduled.len(), 1);
    assert!(matches!(scheduled[0], WaitCondition::Deadline { .. }));
}

#[tokio::test]
async fn test_reconcile_is_idempotent_for_the_same_wake() {
    let runtime = active_goal_runtime().await;
    let port = Arc::new(ScriptedPort::new(GoalTurnDisposition::Waiting {
        condition: WaitCondition::Deadline { deadline: ts(9999) },
    }));
    let scheduler = Arc::new(RecordingScheduler::default());
    let driver = driver(runtime.clone(), port, scheduler.clone());

    driver.reconcile_once().await;
    // A second reconcile over the same waiting state must not double-register.
    driver.reconcile_once().await;

    let scheduled = scheduler
        .scheduled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(scheduled.len(), 1);
}

#[test]
fn test_millis_until_is_clamped() {
    assert_eq!(millis_until(ts(1_500), ts(1_000)), 500);
    // A past deadline never yields a negative delay.
    assert_eq!(millis_until(ts(500), ts(1_000)), 0);
}
