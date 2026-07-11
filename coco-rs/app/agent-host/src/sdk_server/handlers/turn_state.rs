use std::{collections::HashMap, sync::Mutex as StdMutex};

use super::SessionAccounting;
use coco_types::SessionId;

#[derive(Default)]
pub(super) struct TurnState {
    counters: StdMutex<HashMap<SessionId, i32>>,
    accounting: StdMutex<HashMap<SessionId, SessionAccounting>>,
}

impl TurnState {
    pub(super) fn next_turn_id(&self, session_id: &SessionId) -> coco_types::TurnId {
        let mut counters = match self.counters.lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        let counter = counters.entry(session_id.clone()).or_insert(0);
        *counter = counter.saturating_add(1);
        coco_types::TurnId::from(format!("turn-{session_id}-{counter}"))
    }

    pub(super) fn clear_turn_counter(&self, session_id: &SessionId) {
        let mut counters = match self.counters.lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        counters.remove(session_id);
    }

    pub(super) fn reset_accounting(&self, session_id: SessionId) {
        let mut accounting = match self.accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting.insert(session_id, SessionAccounting::new());
    }

    pub(super) fn clear_accounting(&self, session_id: &SessionId) {
        let mut accounting = match self.accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting.remove(session_id);
    }

    pub(super) fn accounting_snapshot(&self, session_id: &SessionId) -> SessionAccounting {
        let accounting = match self.accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting
            .get(session_id)
            .cloned()
            .unwrap_or_else(SessionAccounting::new)
    }

    pub(super) fn accumulate_result(
        &self,
        session_id: &SessionId,
        params: &coco_types::SessionResultParams,
    ) {
        let mut accounting = match self.accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = accounting
            .entry(session_id.clone())
            .or_insert_with(SessionAccounting::new);
        entry.stats.accumulate(params);
    }
}
