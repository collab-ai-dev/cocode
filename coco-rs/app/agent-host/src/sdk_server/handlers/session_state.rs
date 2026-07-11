use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use coco_types::SessionId;

use super::SessionHandoffState;
use super::SessionMetadata;

#[derive(Default)]
pub(super) struct ScopedSessionState {
    handoffs: StdMutex<HashMap<SessionId, SessionHandoffState>>,
    metadata: StdMutex<HashMap<SessionId, SessionMetadata>>,
    plan_mode_instructions: StdMutex<HashMap<SessionId, String>>,
}

impl ScopedSessionState {
    pub(super) fn set_handoff(&self, session_id: SessionId, handoff: SessionHandoffState) {
        let mut handoffs = match self.handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.insert(session_id, handoff);
    }

    pub(super) fn handoff_snapshot(&self, session_id: &SessionId) -> Option<SessionHandoffState> {
        let handoffs = match self.handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.get(session_id).cloned()
    }

    pub(super) fn sole_handoff_snapshot(&self) -> Option<(SessionId, SessionHandoffState)> {
        let handoffs = match self.handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        if handoffs.len() == 1 {
            handoffs
                .iter()
                .next()
                .map(|(session_id, handoff)| (session_id.clone(), handoff.clone()))
        } else {
            None
        }
    }

    pub(super) fn has_handoffs(&self) -> bool {
        let handoffs = match self.handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        !handoffs.is_empty()
    }

    pub(super) fn sole_handoff_id(&self) -> Option<SessionId> {
        self.sole_handoff_snapshot()
            .map(|(session_id, _)| session_id)
    }

    pub(super) fn clear_handoff(&self, session_id: &SessionId) {
        let mut handoffs = match self.handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.remove(session_id);
    }

    pub(super) fn set_metadata(&self, session_id: SessionId, metadata: SessionMetadata) {
        let mut all_metadata = match self.metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.insert(session_id, metadata);
    }

    pub(super) fn metadata_snapshot(&self, session_id: &SessionId) -> Option<SessionMetadata> {
        let all_metadata = match self.metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.get(session_id).cloned()
    }

    pub(super) fn update_model(&self, session_id: &SessionId, model: String) -> Option<String> {
        let mut all_metadata = match self.metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        let metadata = all_metadata.get_mut(session_id)?;
        Some(std::mem::replace(&mut metadata.model, model))
    }

    pub(super) fn clear_metadata(&self, session_id: &SessionId) {
        let mut all_metadata = match self.metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.remove(session_id);
    }

    pub(super) fn set_plan_mode_instructions(
        &self,
        session_id: SessionId,
        instructions: Option<String>,
    ) {
        let mut all_instructions = match self.plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(instructions) = instructions {
            all_instructions.insert(session_id, instructions);
        } else {
            all_instructions.remove(&session_id);
        }
    }

    pub(super) fn plan_mode_instructions(&self, session_id: &SessionId) -> Option<String> {
        let all_instructions = match self.plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_instructions.get(session_id).cloned()
    }

    pub(super) fn clear_plan_mode_instructions(&self, session_id: &SessionId) {
        let mut all_instructions = match self.plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_instructions.remove(session_id);
    }
}
