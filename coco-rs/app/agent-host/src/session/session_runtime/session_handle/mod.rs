use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use coco_types::SessionId;

use super::*;

mod capabilities;
mod controls;
mod engine;
mod history;
mod hooks;
mod late_bind;
mod mcp;
mod tasks;

/// Cheap cloneable capability for a live session runtime.
///
/// The runtime stays private: callers operate through focused capabilities so
/// selecting a session and acting on it remain one explicit boundary.
#[derive(Clone)]
pub struct SessionHandle {
    session_id: SessionId,
    runtime: Arc<SessionRuntime>,
    /// Immutable callback requirements this session was constructed with.
    /// Set once at construction (empty for local surfaces, the connection
    /// profile's set for AppServer/SDK sessions); never installed late.
    callback_requirements: coco_types::SessionCallbackRequirements,
}

pub struct QueuedCommandStatus {
    pub is_empty: bool,
    pub last_changed_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCloseDrainError {
    TurnTaskTimeout { timeout: Duration },
    ForwarderTaskTimeout { timeout: Duration },
}

impl SessionCloseDrainError {
    pub(crate) fn task(self) -> &'static str {
        match self {
            Self::TurnTaskTimeout { .. } => "turn_task",
            Self::ForwarderTaskTimeout { .. } => "forwarder_task",
        }
    }

    pub(crate) fn timeout(self) -> Duration {
        match self {
            Self::TurnTaskTimeout { timeout } | Self::ForwarderTaskTimeout { timeout } => timeout,
        }
    }
}

impl std::fmt::Display for SessionCloseDrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "timed out draining {} after {} ms",
            self.task(),
            self.timeout().as_millis()
        )
    }
}

impl SessionHandle {
    pub async fn fire_session_start_hooks(
        &self,
        source: coco_hooks::orchestration::SessionStartSource,
    ) {
        self.runtime.fire_session_start_hooks(source).await;
    }

    pub(crate) fn start_active_turn(
        &self,
        build: impl FnOnce(
            coco_types::TurnId,
            tokio_util::sync::CancellationToken,
        ) -> super::ActiveTurnHandles,
    ) -> Result<coco_types::TurnId, ()> {
        self.runtime.turn_coordinator.start(&self.session_id, build)
    }

    pub(crate) fn next_turn_id(&self) -> coco_types::TurnId {
        self.runtime.turn_coordinator.next_turn_id(&self.session_id)
    }

    pub(crate) fn reset_session_accounting(&self) {
        self.runtime.turn_coordinator.reset_accounting();
    }

    pub(crate) fn session_accounting_snapshot(&self) -> super::SessionAccounting {
        self.runtime.turn_coordinator.accounting_snapshot()
    }

    pub(crate) fn accumulate_session_result(&self, params: &coco_types::SessionResultParams) {
        self.runtime.turn_coordinator.accumulate_result(params);
    }

    pub(crate) fn active_turn_cancel_token(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.runtime.turn_coordinator.cancel_token()
    }

    pub(crate) fn has_active_turn(&self) -> bool {
        self.active_turn_cancel_token().is_some()
    }

    pub(crate) fn mark_active_turn_finishing(&self) -> bool {
        self.runtime.turn_coordinator.mark_finishing()
    }

    pub(crate) fn complete_finishing_active_turn(&self) -> bool {
        self.runtime.turn_coordinator.complete_finishing()
    }

    pub(crate) fn take_active_turn_for_drain(&self) -> Option<super::ActiveTurnDrainState> {
        self.runtime.turn_coordinator.take_for_drain()
    }

    async fn drain_active_turn(&self, timeout: Duration) -> Result<(), SessionCloseDrainError> {
        let Some(active) = self.take_active_turn_for_drain() else {
            return Ok(());
        };
        let (mut active, cancel_before_drain) = active.into_parts();
        let mut timeout_error = None;
        if cancel_before_drain {
            active.cancel_token.cancel();
        }
        // One absolute deadline for the whole drain: the turn task plus the
        // forwarder must finish within `timeout` total, not `timeout` each, so
        // close cannot silently consume twice the configured budget (CS-3a).
        let deadline = tokio::time::Instant::now() + timeout;
        if tokio::time::timeout_at(deadline, &mut active.turn_task)
            .await
            .is_err()
        {
            active.turn_task.abort();
            let _ = active.turn_task.await;
            timeout_error.get_or_insert(SessionCloseDrainError::TurnTaskTimeout { timeout });
        }
        if tokio::time::timeout_at(deadline, &mut active.forwarder_task)
            .await
            .is_err()
        {
            active.forwarder_task.abort();
            let _ = active.forwarder_task.await;
            timeout_error.get_or_insert(SessionCloseDrainError::ForwarderTaskTimeout { timeout });
        }
        match timeout_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn new(
        runtime: Arc<SessionRuntime>,
        callback_requirements: coco_types::SessionCallbackRequirements,
    ) -> Self {
        let session_id = runtime.current_typed_session_id_snapshot();
        Self {
            session_id,
            runtime,
            callback_requirements,
        }
    }

    pub async fn build(opts: SessionRuntimeBuildOpts<'_>) -> Result<Self> {
        let callback_requirements = opts.callback_requirements.clone();
        let runtime = SessionRuntime::build(opts).await?;
        Ok(Self::new(runtime, callback_requirements))
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Fire `SessionEnd` hooks and request runtime-scoped task shutdown only
    /// when this handle still owns the expected session id.
    ///
    /// Returns the runtime's current session id when the handle is stale.
    pub(crate) async fn close_if_current_session(
        &self,
        expected_session_id: &SessionId,
        reason: coco_hooks::orchestration::ExitReason,
        turn_drain_timeout: Duration,
    ) -> Result<Option<SessionId>, SessionCloseDrainError> {
        let current_session_id = self.runtime.current_typed_session_id().await;
        if current_session_id != *expected_session_id {
            return Ok(Some(current_session_id));
        }

        let drain_result = self.drain_active_turn(turn_drain_timeout).await;
        self.stop_reload_supervisor().await;
        self.runtime.fire_session_end_hooks(reason).await;
        self.runtime.shutdown_signal().cancel();
        // Join session-owned background tasks under the close budget so close
        // proves they terminated, not just that they were signalled (CS-3).
        let task_deadline = tokio::time::Instant::now() + turn_drain_timeout;
        self.runtime.join_session_tasks(task_deadline).await;
        drain_result?;
        Ok(None)
    }

    /// Spawn a session-owned background task tracked for close-time joining
    /// (CS-3 session task supervisor). Prefer this over raw `tokio::spawn` for
    /// tasks that must not outlive the session.
    pub(crate) fn spawn_session_task<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn_session_task(future);
    }

    pub fn orchestration_ctx_factory(
        &self,
    ) -> Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync> {
        self.runtime.orchestration_ctx_factory()
    }
}
