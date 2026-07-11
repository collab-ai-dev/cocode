use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use coco_types::SessionId;
use tokio::sync::watch;

/// Lost-wakeup-safe activity clock shared by AppServer lifecycle owners and
/// host-level supervisors.
pub struct SessionActivityTracker {
    last_activity: Mutex<HashMap<SessionId, Instant>>,
    revision: watch::Sender<u64>,
}

impl Default for SessionActivityTracker {
    fn default() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            last_activity: Mutex::new(HashMap::new()),
            revision,
        }
    }
}

impl SessionActivityTracker {
    pub fn touch(&self, session_id: SessionId) {
        self.last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id, Instant::now());
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn forget(&self, session_id: &SessionId) {
        self.last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn last_activity(&self, session_id: &SessionId) -> Option<Instant> {
        self.last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .copied()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }
}
