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

#[derive(Default)]
pub(crate) struct SessionTurnCoordinator {
    next_turn: AtomicU64,
    active: Mutex<Option<ActiveTurnHandles>>,
}

impl SessionTurnCoordinator {
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
        let sequence = self.next_turn.fetch_add(1, Ordering::Relaxed) + 1;
        let turn_id = coco_types::TurnId::from(format!("turn-{session_id}-{sequence}"));
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
}
