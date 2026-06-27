//! SDK-server scenario suites. Hosts reminder coverage that requires
//! the full `SessionRuntime` (hook registry, mode-transition state,
//! command-queue plumbing) — a path the bare-engine CLI tests can't
//! exercise.

pub mod cancel_during_tool;
pub mod reminders;
pub mod session_archive_emits_aggregate;
pub mod session_resume_roundtrip;
pub mod set_model_mid_session;
