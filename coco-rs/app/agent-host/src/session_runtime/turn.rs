use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use tokio_util::sync::CancellationToken;

pub(crate) struct ActiveTurnHandles {
    pub cancel_token: CancellationToken,
    pub turn_task: tokio::task::JoinHandle<()>,
    pub forwarder_task: tokio::task::JoinHandle<()>,
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
    active: Mutex<Option<ActiveTurnHandles>>,
    accounting: Mutex<SessionAccounting>,
}

impl Default for SessionTurnCoordinator {
    fn default() -> Self {
        Self {
            next_turn: AtomicU64::new(0),
            active: Mutex::new(None),
            accounting: Mutex::new(SessionAccounting::default()),
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
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.is_some() {
            return Err(());
        }
        let turn_id = self.next_turn_id(session_id);
        let cancel = CancellationToken::new();
        *active = Some(build(turn_id.clone(), cancel));
        Ok(turn_id)
    }

    pub(crate) fn cancel_token(&self) -> Option<CancellationToken> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|active| active.cancel_token.clone())
    }

    pub(crate) fn clear(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .is_some()
    }

    pub(crate) fn take(&self) -> Option<ActiveTurnHandles> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
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
