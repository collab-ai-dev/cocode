use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use tokio_util::sync::CancellationToken;

pub(crate) struct ActiveTurnHandles {
    pub cancel_token: CancellationToken,
    pub turn_task: tokio::task::JoinHandle<()>,
    pub forwarder_task: tokio::task::JoinHandle<()>,
}

pub(crate) enum ActiveTurnDrainState {
    Running(ActiveTurnHandles),
    Finishing(ActiveTurnHandles),
}

impl ActiveTurnDrainState {
    pub(crate) fn into_parts(self) -> (ActiveTurnHandles, bool) {
        match self {
            Self::Running(active) => (active, true),
            Self::Finishing(active) => (active, false),
        }
    }
}

#[derive(Default)]
enum TurnLifecycleState {
    #[default]
    Idle,
    /// A synchronous-lifecycle shortcut (`/cost`, `/btw`, …) holds the slot for
    /// its duration. It has no engine turn task or forwarder to drain — only a
    /// cancel token — and is released to `Idle` by its RAII reservation guard.
    Reserved {
        turn_id: coco_types::TurnId,
        cancel: CancellationToken,
    },
    Running {
        turn_id: coco_types::TurnId,
        handles: ActiveTurnHandles,
    },
    Finishing {
        turn_id: coco_types::TurnId,
        handles: ActiveTurnHandles,
    },
}

impl TurnLifecycleState {
    fn is_busy(&self) -> bool {
        match self {
            Self::Idle => false,
            Self::Reserved { .. } | Self::Running { .. } | Self::Finishing { .. } => true,
        }
    }

    fn cancel_token(&self) -> Option<CancellationToken> {
        match self {
            Self::Idle => None,
            Self::Reserved { cancel, .. } => Some(cancel.clone()),
            Self::Running { handles, .. } | Self::Finishing { handles, .. } => {
                Some(handles.cancel_token.clone())
            }
        }
    }

    /// The id of the turn currently owning the slot, if any. Server-request
    /// bridges read this so pending requests are tagged with their turn and can
    /// be cancelled when that turn ends.
    fn turn_id(&self) -> Option<coco_types::TurnId> {
        match self {
            Self::Idle => None,
            Self::Reserved { turn_id, .. }
            | Self::Running { turn_id, .. }
            | Self::Finishing { turn_id, .. } => Some(turn_id.clone()),
        }
    }

    fn into_drain_state(self) -> Option<ActiveTurnDrainState> {
        match self {
            // A reservation has no engine task/forwarder to drain; close just
            // drops it (its cancel token is returned by `close`).
            Self::Idle | Self::Reserved { .. } => None,
            Self::Running { handles, .. } => Some(ActiveTurnDrainState::Running(handles)),
            Self::Finishing { handles, .. } => Some(ActiveTurnDrainState::Finishing(handles)),
        }
    }
}

/// Aggregate protocol accounting owned by one live session runtime.
#[derive(Debug, Clone)]
pub(crate) struct SessionAccounting {
    pub(crate) started_at: std::time::Instant,
    pub(crate) stats: SessionStats,
}

impl Default for SessionAccounting {
    fn default() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            stats: SessionStats::default(),
        }
    }
}

/// Statistics accumulated from every completed turn in one live session.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub total_turns: i32,
    pub total_duration_api_ms: i64,
    pub total_cost_usd: f64,
    pub usage: coco_types::TokenUsage,
    pub model_usage: std::collections::HashMap<String, coco_types::SessionModelUsage>,
    pub permission_denials: Vec<coco_types::PermissionDenialInfo>,
    pub last_result_text: Option<String>,
    pub last_stop_reason: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub had_error: bool,
    pub errors: Vec<String>,
    pub num_api_calls: i32,
}

impl SessionStats {
    fn accumulate(&mut self, params: &coco_types::SessionResultParams) {
        self.total_turns = self.total_turns.saturating_add(1);
        self.total_duration_api_ms = self
            .total_duration_api_ms
            .saturating_add(params.duration_api_ms);
        self.total_cost_usd += params.total_cost_usd;
        self.usage += params.usage;
        for (model, usage) in &params.model_usage {
            let entry = self.model_usage.entry(model.clone()).or_default();
            entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens);
            entry.output_tokens = entry.output_tokens.saturating_add(usage.output_tokens);
            entry.cache_read_input_tokens = entry
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
            entry.cache_creation_input_tokens = entry
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
            entry.web_search_requests = entry
                .web_search_requests
                .saturating_add(usage.web_search_requests);
            entry.cost_usd += usage.cost_usd;
        }
        self.permission_denials
            .extend(params.permission_denials.iter().cloned());
        if params.result.is_some() {
            self.last_result_text = params.result.clone();
        }
        if params.structured_output.is_some() {
            self.structured_output = params.structured_output.clone();
        }
        self.last_stop_reason = Some(params.stop_reason.clone());
        if params.is_error {
            self.had_error = true;
            self.errors.extend(params.errors.iter().cloned());
        }
        if let Some(count) = params.num_api_calls {
            self.num_api_calls = self.num_api_calls.saturating_add(count);
        }
    }
}

pub(crate) struct SessionTurnCoordinator {
    next_turn: AtomicU64,
    lifecycle: Mutex<TurnLifecycleState>,
    accounting: Mutex<SessionAccounting>,
    /// Tombstone set once the session close cascade has drained the active
    /// turn. A turn/start that resolved its target before the close but runs
    /// after it (the validation->execution gap) is rejected here so no new turn
    /// is admitted against a closed session.
    closed: AtomicBool,
}

impl Default for SessionTurnCoordinator {
    fn default() -> Self {
        Self {
            next_turn: AtomicU64::new(0),
            lifecycle: Mutex::new(TurnLifecycleState::Idle),
            accounting: Mutex::new(SessionAccounting::default()),
            closed: AtomicBool::new(false),
        }
    }
}

impl SessionTurnCoordinator {
    pub(crate) fn next_turn_id(&self, session_id: &coco_types::SessionId) -> coco_types::TurnId {
        let sequence = self.next_turn.fetch_add(1, Ordering::Relaxed) + 1;
        coco_types::TurnId::from(format!("turn-{session_id}-{sequence}"))
    }

    pub(crate) fn start(
        &self,
        session_id: &coco_types::SessionId,
        build: impl FnOnce(coco_types::TurnId, CancellationToken) -> ActiveTurnHandles,
    ) -> Result<coco_types::TurnId, ()> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Reject under the lifecycle lock so `close` (which takes the same lock)
        // cannot interleave between this check and the slot install.
        if self.closed.load(Ordering::Acquire) || lifecycle.is_busy() {
            return Err(());
        }
        let turn_id = self.next_turn_id(session_id);
        let cancel = CancellationToken::new();
        *lifecycle = TurnLifecycleState::Running {
            turn_id: turn_id.clone(),
            handles: build(turn_id.clone(), cancel),
        };
        Ok(turn_id)
    }

    /// Reserve the turn slot for a synchronous-lifecycle shortcut. Returns the
    /// minted turn id plus a fresh cancel token; the caller wraps the release in
    /// an RAII guard (see `SessionHandle::reserve_shortcut_turn`). Rejected under
    /// the same `closed || busy` gate as `start`, so a shortcut and a real
    /// `turn/start` cannot both be admitted.
    pub(crate) fn reserve(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Result<(coco_types::TurnId, CancellationToken), ()> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) || lifecycle.is_busy() {
            return Err(());
        }
        let turn_id = self.next_turn_id(session_id);
        let cancel = CancellationToken::new();
        *lifecycle = TurnLifecycleState::Reserved {
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
        };
        Ok((turn_id, cancel))
    }

    /// Release a shortcut reservation back to `Idle`. A no-op if the slot is no
    /// longer `Reserved` (it can only leave `Reserved` via this call, so this is
    /// idempotent and drop-safe).
    pub(crate) fn release_reservation(&self) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*lifecycle, TurnLifecycleState::Reserved { .. }) {
            *lifecycle = TurnLifecycleState::Idle;
        }
    }

    /// The id of the turn currently owning the slot, if any.
    pub(crate) fn active_turn_id(&self) -> Option<coco_types::TurnId> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .turn_id()
    }

    /// Tombstone the coordinator so no further turn can be admitted, and return
    /// the cancel token of a turn admitted in the drain->close race window so
    /// the caller can cancel it. A `Running` turn admitted after the close drain
    /// snapshot would otherwise run detached against a closed session; a
    /// `Finishing` turn is already done (its terminal is in flight) and is
    /// deliberately left alone so close waits for its terminal instead of
    /// issuing a spurious cancel.
    pub(crate) fn close(&self) -> Option<CancellationToken> {
        let lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closed.store(true, Ordering::Release);
        match &*lifecycle {
            TurnLifecycleState::Running { handles, .. } => Some(handles.cancel_token.clone()),
            // A reserved shortcut still in flight when the session closes is
            // cancelled so it cannot run detached against a closed session.
            TurnLifecycleState::Reserved { cancel, .. } => Some(cancel.clone()),
            TurnLifecycleState::Finishing { .. } | TurnLifecycleState::Idle => None,
        }
    }

    pub(crate) fn cancel_token(&self) -> Option<CancellationToken> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel_token()
    }

    pub(crate) fn mark_finishing(&self) -> bool {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::mem::take(&mut *lifecycle);
        match previous {
            TurnLifecycleState::Running { turn_id, handles }
            | TurnLifecycleState::Finishing { turn_id, handles } => {
                *lifecycle = TurnLifecycleState::Finishing { turn_id, handles };
                true
            }
            // A reservation has no engine-turn lifecycle; leave it untouched.
            other @ (TurnLifecycleState::Reserved { .. } | TurnLifecycleState::Idle) => {
                *lifecycle = other;
                false
            }
        }
    }

    pub(crate) fn complete_finishing(&self) -> bool {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::mem::take(&mut *lifecycle);
        match previous {
            TurnLifecycleState::Finishing { .. } => true,
            TurnLifecycleState::Running { turn_id, handles } => {
                *lifecycle = TurnLifecycleState::Running { turn_id, handles };
                false
            }
            other @ (TurnLifecycleState::Reserved { .. } | TurnLifecycleState::Idle) => {
                *lifecycle = other;
                false
            }
        }
    }

    pub(crate) fn take_for_drain(&self) -> Option<ActiveTurnDrainState> {
        std::mem::take(
            &mut *self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_drain_state()
    }

    pub(crate) fn reset_accounting(&self) {
        *self
            .accounting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionAccounting::default();
    }

    pub(crate) fn accounting_snapshot(&self) -> SessionAccounting {
        self.accounting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn accumulate_result(&self, params: &coco_types::SessionResultParams) {
        self.accounting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stats
            .accumulate(params);
    }
}

#[cfg(test)]
#[path = "turn.test.rs"]
mod tests;
