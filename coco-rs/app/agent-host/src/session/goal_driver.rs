//! Host-side `GoalSupervisor` driver + concrete [`SessionTurnPort`] (§10.3).
//!
//! This is the missing half of the supervisor model: the domain `GoalSupervisor`
//! and the `SessionTurnPort` trait live in `coco-goal-runtime`, but nothing drove
//! them. This module supplies
//!
//! 1. [`SessionGoalTurnPort`] — runs exactly one promptless, supervisor-owned goal
//!    turn against the session engine (built with
//!    `with_goal_supervisor_owned_finalize`, so the engine defers finalization to
//!    the supervisor) and reports a [`GoalTurnOutcome`];
//! 2. [`GoalDriver`] — a level-triggered loop that, on a goal edge (resume, a fired
//!    wake, or restart reconciliation), advances the supervisor until the goal is no
//!    longer startable, then registers a watcher for any `waiting` state so a
//!    deadline/backoff/reset goal wakes itself.
//!
//! Phase A owns the **cold edges** the engine-hook loop cannot: `waiting`-wake,
//! resume-auto-start, and restart reconciliation. Warm active→active continuation
//! of a freshly created goal still rides the engine-hook loop until the full
//! cut-over; the reducer's lease identity keeps the two from double-driving.

use std::collections::HashSet;
use std::sync::Arc;

use coco_goal_runtime::{
    AdvanceOutcome, AutonomousAdmission, GoalCompletionCoordinator, GoalContextMaterializer,
    GoalRuntimeHandle, GoalSupervisor, GoalTurnCompletion, GoalTurnHandle, GoalTurnOutcome,
    GoalTurnRequest, ProviderErrorKind, SessionTurnPort,
};
use coco_goals::{
    GoalCommand, GoalLeaseId, GoalLifecycle, GoalTurnDisposition, GoalWake, ProgressSignal,
    Timestamp, UsageDelta, WaitCondition, Wake, WakeId,
};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use crate::session::session_runtime::SessionHandle;

/// Milliseconds until a wall-clock wait deadline elapses, clamped to zero.
fn millis_until(deadline: Timestamp, now: Timestamp) -> u64 {
    (deadline.millis() - now.millis()).max(0) as u64
}

fn now_ts() -> Timestamp {
    Timestamp::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default(),
    )
}

/// Registers durable-wake watchers for `waiting` goals. Split behind a trait so the
/// driver loop is unit-testable without real timers. The concrete
/// [`TimerWakeScheduler`] fires a Tokio timer for wall-clock waits; task-completion
/// waits are a follow-up (they need a `TaskManager` subscription seam).
pub trait WakeScheduler: Send + Sync {
    /// Arrange for `on_fire` to run once when `wake`'s condition is satisfied.
    /// A condition with no time/task predicate (permission, mode-gate, user
    /// acceptance, external) is user-driven and left unscheduled.
    fn schedule(&self, wake: GoalWake, on_fire: Box<dyn FnOnce() + Send>);
}

/// Fires a Tokio timer for wall-clock waits (`deadline`, `provider_backoff`,
/// `usage_reset`). Other conditions are left to user/system resume.
pub struct TimerWakeScheduler {
    shutdown: CancellationToken,
}

impl TimerWakeScheduler {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self { shutdown }
    }
}

impl WakeScheduler for TimerWakeScheduler {
    fn schedule(&self, wake: GoalWake, on_fire: Box<dyn FnOnce() + Send>) {
        let delay_ms = match &wake.condition {
            WaitCondition::Deadline { deadline }
            | WaitCondition::ProviderBackoff { deadline, .. }
            | WaitCondition::UsageReset { deadline } => millis_until(*deadline, now_ts()),
            // Task waits need a registry subscription (follow-up); the rest are
            // resolved by explicit user/system action, not a timer.
            _ => return,
        };
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => on_fire(),
            }
        });
    }
}

/// A future the driver runs as one session-serialized autonomous burst.
type BurstFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Runs the driver's autonomous burst while **holding the session turn slot**, so an
/// autonomous burst and a user turn can never run two engines over the shared
/// history. Split behind a trait so the driver loop is unit-testable without the
/// `TurnCoordinator`.
pub trait BurstScheduler: Send + Sync {
    /// Spawn `burst` holding the turn slot for its duration; returns `false` and
    /// drops `burst` unrun if a turn is already active (a user turn is running).
    /// The slot is released when the burst finishes **or panics** (RAII guard), so a
    /// panicking burst can never wedge the slot and block every future turn.
    fn schedule(&self, burst: BurstFuture) -> bool;
}

/// Session-backed [`BurstScheduler`] over the `TurnCoordinator` slot.
pub struct SessionBurstScheduler {
    session: SessionHandle,
}

impl SessionBurstScheduler {
    pub fn new(session: SessionHandle) -> Self {
        Self { session }
    }
}

/// Releases the session turn slot on drop — including on burst panic, since Tokio
/// unwinds a panicking task and runs its destructors — so a panicking burst can
/// never leave the slot wedged.
struct SlotReleaseGuard {
    session: SessionHandle,
}

impl Drop for SlotReleaseGuard {
    fn drop(&mut self) {
        self.session.mark_active_turn_finishing();
        self.session.complete_finishing_active_turn();
    }
}

impl BurstScheduler for SessionBurstScheduler {
    fn schedule(&self, burst: BurstFuture) -> bool {
        let guard_session = self.session.clone();
        self.session
            .start_active_turn(move |_turn_id, cancel| {
                let turn_task = tokio::spawn(async move {
                    // Held for the whole burst; released on completion or panic.
                    let _slot = SlotReleaseGuard {
                        session: guard_session,
                    };
                    burst.await;
                });
                // No surface forwarder yet: the burst drains the engine event
                // channel and commits durable history; live event routing of
                // autonomous turns is a follow-up.
                let forwarder_task = tokio::spawn(async {});
                crate::session::session_runtime::ActiveTurnHandles {
                    cancel_token: cancel,
                    turn_task,
                    forwarder_task,
                }
            })
            .is_ok()
    }
}

/// Concrete [`SessionTurnPort`] that runs one promptless goal turn on the session
/// engine. The supervisor has already recorded `running(lease, turn_id)`, so the
/// per-turn goal-context reminder re-injects the objective; the engine is built with
/// `with_goal_supervisor_owned_finalize`, so it runs one logical turn and returns
/// without finalizing — the supervisor owns that.
pub struct SessionGoalTurnPort {
    session: SessionHandle,
}

impl SessionGoalTurnPort {
    pub fn new(session: SessionHandle) -> Self {
        Self { session }
    }

    async fn run_one_turn(session: SessionHandle, request: GoalTurnRequest) -> GoalTurnOutcome {
        // The driver holds the turn slot for the whole burst, so this runs within
        // it — no per-turn slot check here (the slot is already ours, and checking
        // `has_active_turn` would see our own burst and wrongly decline).
        let cancel = CancellationToken::new();
        let turn_engine = session
            .build_turn_engine(
                crate::session::session_runtime::SessionTurnEngineConfigRequest {
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                    max_turns: None,
                    system_prompt: None,
                },
                cancel,
            )
            .await;
        let engine = turn_engine.engine.with_goal_supervisor_owned_finalize();
        // Autonomous turn: no new user message; run against the current history.
        let combined = session
            .append_arc_messages_to_history_and_snapshot(Vec::new())
            .await;

        // Drain the engine's event stream; the durable history commit below is what
        // persists the turn (live surface routing of autonomous turns is a
        // follow-up).
        let (event_tx, mut event_rx) = mpsc::channel::<coco_types::CoreEvent>(256);
        let drain = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });

        let result = engine
            .run_with_messages(combined, event_tx, request.turn_id.clone())
            .await;
        drain.abort();

        match result {
            Ok(query_result) => {
                session
                    .commit_engine_turn_history(query_result.final_history)
                    .await;
                if query_result.cancelled {
                    return GoalTurnOutcome::Interrupted;
                }
                let disposition = session
                    .goal_runtime()
                    .take_pending_report()
                    .await
                    .unwrap_or(GoalTurnDisposition::Unreported);
                let usage = UsageDelta {
                    input_tokens: query_result.total_usage.input_tokens.total.max(0) as u64,
                    output_tokens: query_result.total_usage.output_tokens.total.max(0) as u64,
                    duration_ms: query_result.duration_ms.max(0),
                };
                GoalTurnOutcome::Ended {
                    disposition,
                    // A goal-owned turn that reached its natural stop did work; the
                    // coordinator treats one accepted signal as progress.
                    signals: vec![ProgressSignal::ToolObservation],
                    usage,
                }
            }
            Err(err) => GoalTurnOutcome::ProviderError {
                kind: ProviderErrorKind::Retryable,
                message: err.to_string(),
            },
        }
    }
}

#[async_trait::async_trait]
impl SessionTurnPort for SessionGoalTurnPort {
    async fn start_goal_turn(
        &self,
        request: GoalTurnRequest,
    ) -> coco_goal_runtime::Result<GoalTurnHandle> {
        let turn_id = request.turn_id.clone();
        let session = self.session.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let outcome = SessionGoalTurnPort::run_one_turn(session, request).await;
            let _ = tx.send(outcome);
        });
        Ok(GoalTurnHandle {
            turn_id,
            completion: GoalTurnCompletion::new(rx),
        })
    }
}

/// Level-triggered driver over the session's [`GoalSupervisor`].
pub struct GoalDriver {
    supervisor: GoalSupervisor,
    goal_runtime: Arc<GoalRuntimeHandle>,
    wake_scheduler: Arc<dyn WakeScheduler>,
    burst_scheduler: Arc<dyn BurstScheduler>,
    edge: Arc<Notify>,
    registered_wakes: tokio::sync::Mutex<HashSet<WakeId>>,
}

impl GoalDriver {
    /// Assemble a driver from the session's goal runtime plus a concrete turn port
    /// and wake scheduler. Shares the supervisor's materializer/coordinator with a
    /// bounded autonomous-admission limit. `burst_scheduler` serializes each
    /// autonomous burst against user turns via the session turn slot.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        goal_runtime: Arc<GoalRuntimeHandle>,
        turn_port: Arc<dyn SessionTurnPort>,
        materializer: Arc<GoalContextMaterializer>,
        coordinator: Arc<GoalCompletionCoordinator>,
        admission: AutonomousAdmission,
        wake_scheduler: Arc<dyn WakeScheduler>,
        burst_scheduler: Arc<dyn BurstScheduler>,
        edge: Arc<Notify>,
    ) -> Self {
        let supervisor = GoalSupervisor::new(
            Arc::clone(&goal_runtime),
            turn_port,
            materializer,
            coordinator,
            admission,
        );
        Self {
            supervisor,
            goal_runtime,
            wake_scheduler,
            burst_scheduler,
            edge,
            registered_wakes: tokio::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Run the driver loop until `shutdown` fires. Reconciles once at start so a
    /// restored `waiting` goal re-registers its watcher and a restored active-queued
    /// goal resumes. Each reconcile runs as one slot-held burst; an edge that fires
    /// mid-burst is not lost — `Notify` stores the permit, so the next loop turn
    /// picks it up.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        self.drive_once_awaiting().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = self.edge.notified() => {}
            }
            self.drive_once_awaiting().await;
        }
    }

    /// Run one reconcile as a slot-held burst and await its completion. When a user
    /// turn holds the slot the burst is skipped; the AppServer turn-completion path
    /// re-signals the edge once the slot frees (Phase B), so continuation resumes.
    async fn drive_once_awaiting(self: &Arc<Self>) {
        let driver = Arc::clone(self);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let scheduled = self.burst_scheduler.schedule(Box::pin(async move {
            driver.reconcile_once().await;
            let _ = done_tx.send(());
        }));
        if scheduled {
            let _ = done_rx.await;
        }
    }

    /// Advance the goal to a stop, then register any needed wake watcher.
    async fn reconcile_once(&self) {
        // Keep advancing while the supervisor started and finalized a turn; any
        // other outcome (not startable / waiting / budget-limited) ends the burst.
        while let Ok(AdvanceOutcome::Advanced) = self.supervisor.advance().await {}
        self.reconcile_wakes().await;
    }

    /// Register a watcher for the current `waiting` state (idempotent per wake id),
    /// so a deadline/backoff/reset goal wakes itself back to `active`.
    async fn reconcile_wakes(&self) {
        let Some(snapshot) = self.goal_runtime.snapshot().await else {
            return;
        };
        let GoalLifecycle::Waiting { wake } = &snapshot.lifecycle else {
            return;
        };
        let mut registered = self.registered_wakes.lock().await;
        if !registered.insert(wake.wake_id.clone()) {
            return;
        }
        let goal_runtime = Arc::clone(&self.goal_runtime);
        let edge = Arc::clone(&self.edge);
        let goal_id = snapshot.goal_id.clone();
        let wake_id = wake.wake_id.clone();
        self.wake_scheduler.schedule(
            wake.clone(),
            Box::new(move || {
                // Fired from a timer thread: apply Wake, then nudge the loop to
                // advance the now-active goal. Best-effort — a failed apply (e.g.
                // the goal was cleared) simply leaves the goal untouched.
                tokio::spawn(async move {
                    let command = GoalCommand::Wake(Wake {
                        goal_id,
                        wake_id,
                        next_lease_id: GoalLeaseId::new(format!("lease-{}", uuid::Uuid::new_v4())),
                        resolution: None,
                        at: now_ts(),
                    });
                    if goal_runtime.apply(command).await.is_ok() {
                        edge.notify_one();
                    }
                });
            }),
        );
    }
}

/// Per-session cap on concurrent autonomous goal turns. One keeps a session's
/// autonomous work strictly sequential; process-wide fairness is a follow-up.
const GOAL_AUTONOMOUS_CONCURRENCY: usize = 1;

/// Construct and spawn the goal continuation driver for a session (§10.3).
///
/// Shares the session-scoped evidence store (so the driver's completion
/// coordinator resolves the same provenance the per-turn goal tools mint) and the
/// session's cold-edge signal (so `/goal resume` and fired wakes nudge it). The
/// task is session-owned, so close aborts and joins it. Safe from any entrypoint:
/// the loop idles until a goal exists.
pub fn spawn(session: SessionHandle) {
    let plan_source = Arc::new(crate::session::goal_plan::SessionPlanSource::new(
        session.session_plan_file_path(),
    ));
    let materializer = Arc::new(GoalContextMaterializer::new(plan_source));
    let coordinator = Arc::new(GoalCompletionCoordinator::new(
        session.goal_evidence(),
        Arc::new(coco_goal_runtime::AlwaysVerified),
    ));
    let port: Arc<dyn SessionTurnPort> = Arc::new(SessionGoalTurnPort::new(session.clone()));
    let shutdown = session.shutdown_child_token();
    let wake_scheduler: Arc<dyn WakeScheduler> =
        Arc::new(TimerWakeScheduler::new(shutdown.clone()));
    let burst_scheduler: Arc<dyn BurstScheduler> =
        Arc::new(SessionBurstScheduler::new(session.clone()));
    let driver = Arc::new(GoalDriver::new(
        session.goal_runtime(),
        port,
        materializer,
        coordinator,
        AutonomousAdmission::new(GOAL_AUTONOMOUS_CONCURRENCY),
        wake_scheduler,
        burst_scheduler,
        session.goal_driver_edge(),
    ));
    session.spawn_session_task(driver.run(shutdown));
}

#[cfg(test)]
#[path = "goal_driver.test.rs"]
mod tests;
