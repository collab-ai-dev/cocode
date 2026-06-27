//! Reconcile the live coordinator-mode env flag against a resumed
//! session's stored mode.
//!
//! Called from every resume entry point. The pure decision lives in
//! [`coco_subagent::session_mode_switch_action`]; the env mutation it
//! deliberately defers to the caller lives here, in the bootstrap layer
//! that owns env composition.

use coco_config::EnvKey;
use coco_subagent::SessionMode;
use coco_subagent::SessionModeSwitch;
use coco_subagent::is_coordinator_mode;
use coco_subagent::is_coordinator_mode_env;
use coco_subagent::session_mode_switch_action;
use coco_types::Feature;
use coco_types::Features;

/// Reconcile coordinator mode against a resumed session's stored mode
/// string (`coordinator` / `normal` / absent).
///
/// Gated on [`Feature::AgentTeams`].
/// On a mismatch it flips [`EnvKey::CocoCoordinatorMode`] so the live
/// `coco_subagent::is_coordinator_mode` gate — read per-turn by the
/// system-prompt selector and by the TUI badge — reflects the resumed
/// session, and returns the user-facing warning to surface. Returns
/// `None` when no change is needed or agent-teams is disabled.
pub fn reconcile_on_resume(stored_mode: Option<&str>, features: &Features) -> Option<&'static str> {
    if !features.enabled(Feature::AgentTeams) {
        return None;
    }
    let stored = stored_mode.and_then(SessionMode::from_metadata_str);
    let action = session_mode_switch_action(stored, is_coordinator_mode_env());
    match action {
        SessionModeSwitch::EnterCoordinator => set_coordinator_env(true),
        SessionModeSwitch::ExitCoordinator => set_coordinator_env(false),
        SessionModeSwitch::NoOp => {}
        _ => {}
    }
    action.warning()
}

/// Persist this leader session's coordinator-mode state (`coordinator` /
/// `normal`) into its transcript so a later `--resume` re-derives the role via
/// [`reconcile_on_resume`]. Gated on [`Feature::AgentTeams`] so non-team
/// transcripts stay clean.
///
/// `save_mode` loads the session and requires an existing transcript, so call
/// this AFTER at least one turn (end-of-run is fine). The write side was
/// previously wired ONLY at the TUI exit checkpoint — a headless / SDK leader
/// never persisted the mode, so `--resume` of that session silently dropped
/// the coordinator role. Runs on every leader entrypoint.
///
/// Synchronous IO — callers on an async runtime should wrap in
/// `spawn_blocking`.
pub fn persist_session_mode(
    session_manager: &coco_session::SessionManager,
    session_id: &str,
    features: &Features,
) {
    if !features.enabled(Feature::AgentTeams) {
        return;
    }
    let mode = if is_coordinator_mode(features) {
        "coordinator"
    } else {
        "normal"
    };
    if let Err(e) = session_manager.save_mode(session_id, mode) {
        tracing::warn!(error = %e, mode, "failed to persist coordinator mode to transcript");
    }
}

/// Flip the process-global `COCO_COORDINATOR_MODE` var. `is_coordinator_mode`
/// reads it live, so the change takes effect on the next prompt assembly.
///
/// SAFETY: `std::env::{set_var,remove_var}` are not thread-safe. This runs
/// at session bootstrap / a turn boundary (the resume reconcile), before any
/// concurrent reader thread observes the flag.
fn set_coordinator_env(on: bool) {
    let key = EnvKey::CocoCoordinatorMode.as_str();
    unsafe {
        if on {
            std::env::set_var(key, "1");
        } else {
            std::env::remove_var(key);
        }
    }
}

#[cfg(test)]
#[path = "coordinator_mode_resume.test.rs"]
mod tests;
