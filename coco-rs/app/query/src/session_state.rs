//! Session-state emission tracker.
//!
//! Ensures every `CoreEvent::Protocol(SessionStateChanged)` transition goes
//! through a single choke point that dedupes consecutive identical emissions.
//! Without this, the engine emits `Running` on every permission-branch exit
//! — the wire stream sees a storm of duplicate `session/stateChanged`
//! notifications that violate the "exactly one emission per edge" contract
//! SDK consumers depend on.
//!
//! TS reference: `notifySessionStateChanged()` in `print.ts` guards against
//! re-emitting the same state.
//!
//! See `event-system-design.md` §7.2 (SessionStateChanged semantics) and
//! plan file `zippy-roaming-eich.md` WS-4.

use std::sync::Mutex;

use coco_types::SessionState;

use crate::CoreEvent;
use crate::ServerNotification;

/// Tracks the last emitted session state and emits only on real transitions.
///
/// Owned by the session loop. Uses `std::sync::Mutex` rather than `Cell`
/// because the session loop's future is `Send`-bounded (QueryEngine adapters
/// box it as `Send`). The mutex is never held across an `.await`, so it
/// cannot deadlock; it only guards the compare-and-set.
pub(crate) struct SessionStateTracker {
    last: Mutex<Option<SessionState>>,
}

impl SessionStateTracker {
    pub(crate) fn new() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }

    /// Emit a transition to `new` if it differs from the last observed state.
    ///
    /// No-op when `tx` is `None` (headless/test callers) or when the new
    /// state matches the previous one.
    pub(crate) async fn transition_to(
        &self,
        new: SessionState,
        tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        {
            // A poisoned lock only happens if a prior holder panicked
            // mid-update. Recovering the inner value is safe here because
            // the guarded state is a plain `Option<SessionState>` with no
            // invariants beyond "last observed state" — we simply
            // overwrite it.
            let mut guard = match self.last.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if *guard == Some(new) {
                return;
            }
            *guard = Some(new);
        }
        if let Some(sender) = tx {
            let _ = sender
                .send(CoreEvent::Protocol(
                    ServerNotification::SessionStateChanged { state: new },
                ))
                .await;
        }
    }

    /// Return the last emitted state, for assertions in tests.
    #[cfg(test)]
    pub(crate) fn last(&self) -> Option<SessionState> {
        match self.last.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
#[path = "session_state.test.rs"]
mod tests;
