//! Session-scoped auto-mode state.
//!
//! Three boolean flags controlling auto-mode lifecycle:
//! - `active`: whether the classifier runs on tool calls
//! - `cli_flag`: whether `--auto` was passed at startup
//! - `circuit_broken`: set by remote config gate check
//!
//! Uses `AtomicBool` for lock-free hot-path reads. The `is_active()` check
//! runs on every tool call; `set_active()` is rare (mode transitions only).

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Session-scoped auto-mode state. Thread-safe (`Send + Sync`).
///
/// Shared via `Arc<AutoModeState>` across the permission pipeline, query
/// engine, and mode transition logic.
pub struct AutoModeState {
    /// Whether auto-mode is currently active (classifier runs on tool calls).
    active: AtomicBool,
    /// Whether `--auto` flag was passed on CLI (immutable after startup).
    cli_flag: AtomicBool,
    /// Circuit breaker: set by config gate check when auto is disabled remotely.
    /// Once set, auto-mode cannot be re-activated until session restart.
    circuit_broken: AtomicBool,
}

impl AutoModeState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            cli_flag: AtomicBool::new(false),
            circuit_broken: AtomicBool::new(false),
        }
    }

    /// Whether auto-mode is currently active.
    ///
    /// Hot path — called on every tool call. `Relaxed` ordering is sufficient
    /// because the flag is set synchronously during mode transitions (same
    /// task context), not concurrently by a different thread.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn set_active(&self, v: bool) {
        self.active.store(v, Ordering::Relaxed);
    }

    /// Whether the circuit breaker has tripped (remote config disabled auto).
    pub fn is_circuit_broken(&self) -> bool {
        self.circuit_broken.load(Ordering::Relaxed)
    }

    pub fn set_circuit_broken(&self, v: bool) {
        self.circuit_broken.store(v, Ordering::Relaxed);
    }

    /// Whether `--auto` was passed on CLI.
    pub fn cli_flag(&self) -> bool {
        self.cli_flag.load(Ordering::Relaxed)
    }

    pub fn set_cli_flag(&self, v: bool) {
        self.cli_flag.store(v, Ordering::Relaxed);
    }

    /// Whether the auto-mode gate is enabled (can enter/stay in auto).
    ///
    /// Checks the circuit breaker; a full implementation would also check settings and model.
    pub fn is_gate_enabled(&self) -> bool {
        !self.is_circuit_broken()
    }
}

impl Default for AutoModeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "auto_mode_state.test.rs"]
mod tests;
