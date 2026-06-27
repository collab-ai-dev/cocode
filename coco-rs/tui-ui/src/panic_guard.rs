//! Cooperation between `catch_unwind`-based recovery and the app's global
//! terminal-restoring panic hook.
//!
//! The TUI installs a process-global panic hook that restores the terminal
//! (leaves raw mode, shows the cursor) and prints a backtrace. That hook runs
//! at the panic site — *before* any `catch_unwind` further up the stack gets a
//! chance to recover. So a panic that a caller fully intends to catch (e.g. a
//! pathological mermaid layout) would still tear down the terminal mid-render.
//!
//! This thread-local guard lets the recovering region mark "a panic here is
//! expected and will be caught — do not restore the terminal or dump a
//! backtrace". The hook checks [`suppress_panic_restore`]; the recovering code
//! holds a [`PanicRestoreGuard`] for the duration of its `catch_unwind`.

use std::cell::Cell;

thread_local! {
    /// Nesting depth of live [`PanicRestoreGuard`]s on this thread. A counter
    /// (not a bool) so nested/overlapping recoverable regions compose: an inner
    /// guard dropping must not re-arm restore while an outer guard is still held.
    static SUPPRESS_DEPTH: Cell<i32> = const { Cell::new(0) };
}

/// True while at least one [`PanicRestoreGuard`] is held on this thread.
pub fn suppress_panic_restore() -> bool {
    SUPPRESS_DEPTH.with(Cell::get) > 0
}

/// RAII guard: while held, a panic on this thread is treated as expected and
/// recoverable — the global hook skips terminal restore + backtrace. Dropping
/// it (including during unwind) decrements the depth; suppression ends only when
/// the last live guard drops, so nesting is safe.
#[must_use]
pub struct PanicRestoreGuard(());

impl PanicRestoreGuard {
    pub fn new() -> Self {
        SUPPRESS_DEPTH.with(|c| c.set(c.get() + 1));
        Self(())
    }
}

impl Default for PanicRestoreGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PanicRestoreGuard {
    fn drop(&mut self) {
        SUPPRESS_DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

#[cfg(test)]
#[path = "panic_guard.test.rs"]
mod tests;
