//! Process-wide durable `session_seq` allocation.
//!
//! One allocator per process provides the multi-session single stamping seam:
//! every durable-envelope producer draws from the same
//! per-session counters, so the union of all forwarder paths stays strictly
//! monotonic per session. Restart continuity comes from two
//! halves owned here:
//!
//! - a persist hook fired at bounded intervals so a crash loses at most
//!   [`WATERMARK_PERSIST_INTERVAL`] seqs of watermark staleness, and
//! - skip-ahead initialization on resume: the counter restarts at
//!   `watermark + skip_ahead_window + 1`, which is strictly above anything
//!   the previous process epoch can have emitted. Replay is `seq > cursor`
//!   everywhere, so the resulting hole is benign.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use coco_types::SessionId;

/// Persist a session's seq watermark. Invoked outside the allocator lock;
/// implementations must not block (spawn their own IO).
pub type SessionSeqPersistHook = Arc<dyn Fn(&SessionId, i64) + Send + Sync>;

/// Persist the watermark at least every this many allocated seqs. The first
/// allocation for a session always persists, so watermark staleness is
/// bounded by this interval plus in-flight writes.
pub const WATERMARK_PERSIST_INTERVAL: i64 = 32;

const DEFAULT_SKIP_AHEAD_WINDOW: i64 = 1024 + WATERMARK_PERSIST_INTERVAL;

/// Shared per-process allocator for durable `session_seq` values.
pub struct SessionSeqAllocator {
    inner: Mutex<AllocatorState>,
    /// Highest watermark handed to the persist hook per session. Hook calls
    /// happen outside the allocator lock, so two due allocations can race to
    /// it in either order; this gate drops the older one so the on-disk
    /// watermark never regresses (the skip-ahead safety proof assumes
    /// monotone persists).
    persist_gate: Mutex<HashMap<SessionId, i64>>,
}

struct AllocatorState {
    next: HashMap<SessionId, i64>,
    /// Last seq actually handed out by `next()` this epoch. Distinct from
    /// `next` so `high_water` reports issued reality: after a resume
    /// skip-ahead with no allocation, `next` is `watermark + window + 1` but
    /// nothing was issued, so persisting `next - 1` would inflate the
    /// watermark by ~`window` every idle resume→close cycle.
    issued: HashMap<SessionId, i64>,
    last_persisted: HashMap<SessionId, i64>,
    persist_hook: Option<SessionSeqPersistHook>,
    skip_ahead_window: i64,
}

impl Default for SessionSeqAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionSeqAllocator {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(AllocatorState {
                next: HashMap::new(),
                issued: HashMap::new(),
                last_persisted: HashMap::new(),
                persist_hook: None,
                skip_ahead_window: DEFAULT_SKIP_AHEAD_WINDOW,
            }),
            persist_gate: Mutex::new(HashMap::new()),
        }
    }

    /// Lock the state, recovering the guard through a poisoned mutex. The
    /// allocator only ever holds counters, so a prior panic can't leave a
    /// torn invariant — continuing past the poison is the correct behavior.
    fn state(&self) -> std::sync::MutexGuard<'_, AllocatorState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bind the skip-ahead window to the configured retention ring size.
    /// Clamped so it always exceeds the persist interval — the safety proof
    /// (`emitted_max <= watermark + staleness < watermark + window`) needs it.
    pub fn set_skip_ahead_window(&self, event_retention_per_session: i64) {
        let mut inner = self.state();
        inner.skip_ahead_window = event_retention_per_session.max(WATERMARK_PERSIST_INTERVAL)
            + WATERMARK_PERSIST_INTERVAL;
    }

    pub fn set_persist_hook(&self, hook: SessionSeqPersistHook) {
        let mut inner = self.state();
        inner.persist_hook = Some(hook);
    }

    /// Allocate the next durable seq for `session_id`, firing the persist
    /// hook when the watermark is due (first allocation, then every
    /// [`WATERMARK_PERSIST_INTERVAL`]).
    pub fn next(&self, session_id: &SessionId) -> i64 {
        let (seq, persist) = {
            let mut inner = self.state();
            let next = inner.next.entry(session_id.clone()).or_insert(1);
            let seq = *next;
            *next += 1;
            inner.issued.insert(session_id.clone(), seq);
            let due = match inner.last_persisted.get(session_id) {
                Some(last) => seq - *last >= WATERMARK_PERSIST_INTERVAL,
                None => true,
            };
            if due {
                inner.last_persisted.insert(session_id.clone(), seq);
            }
            (seq, due.then(|| inner.persist_hook.clone()).flatten())
        };
        if let Some(hook) = persist {
            // Serialize hook invocation and skip stale watermarks: without
            // this, hooks for seqs 32 and 64 could run in reverse order and
            // leave 32 on disk.
            let mut gate = self
                .persist_gate
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let last = gate.entry(session_id.clone()).or_insert(0);
            if seq > *last {
                *last = seq;
                hook(session_id, seq);
            }
        }
        seq
    }

    /// Restore counter state for a resumed session from its persisted
    /// watermark: the next allocated seq is at least
    /// `watermark + skip_ahead_window + 1`. Never moves a counter backwards.
    pub fn initialize_after_watermark(&self, session_id: &SessionId, watermark: i64) {
        let mut inner = self.state();
        let floor = watermark
            .saturating_add(inner.skip_ahead_window)
            .saturating_add(1);
        let next = inner.next.entry(session_id.clone()).or_insert(floor);
        if *next < floor {
            *next = floor;
        }
        // Force a persist on the first post-resume allocation so the new
        // epoch's watermark lands quickly. The skip-ahead bumped `next` but
        // issued nothing yet, so `issued` stays untouched.
        inner.last_persisted.remove(session_id);
    }

    /// Last seq actually issued this epoch, for close-time persistence.
    /// Returns `None` when nothing was allocated (e.g. a resume that closed
    /// without emitting), so the close-time persister leaves the prior
    /// watermark standing instead of inflating it by the skip-ahead window
    ///. Counter state is deliberately kept for the process lifetime so
    /// a same-process close-then-resume continues without a hole.
    pub fn high_water(&self, session_id: &SessionId) -> Option<i64> {
        let inner = self.state();
        inner.issued.get(session_id).copied()
    }
}

#[cfg(test)]
#[path = "session_seq.test.rs"]
mod tests;
