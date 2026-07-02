//! Generic substrate for fenced background-review loops.
//!
//! Both the memory subsystem (extract / dream) and the skill-learning
//! subsystem (review / curator) run the same shape of work: a periodic or
//! turn-end pass that forks a sandboxed sub-agent under a write fence, guarded
//! by a cross-process consolidation lock. This crate owns the pieces of that
//! machinery that are provably policy-free and security/concurrency-critical,
//! so they exist in exactly one place rather than being copied per subsystem:
//!
//! - [`lock`] — a parameterized, `O_EXCL`-atomic CAS lock with an mtime-based
//!   "last consolidated at" gate and an RAII rollback guard.
//! - [`write_targets`] — tool-input → affected-write-path extraction shared
//!   by the per-fork write fences (memory, skill review).
//!
//! Per-subsystem policy (the review prompt, the target directory, the model
//! role, the consolidation strategy, the per-path write predicate) stays in
//! the consuming crate.

pub mod lock;
pub mod write_targets;

pub use lock::{ConsolidateLock, LockGuard, LockOutcome};
pub use write_targets::{apply_patch_write_targets, input_write_target};
