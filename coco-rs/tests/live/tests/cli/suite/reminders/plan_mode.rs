//! `PlanMode` reminder — fires every turn while the engine is in
//! `PermissionMode::Plan`.
//!
//! Setup: switch the engine into Plan mode via `SessionConfig`.
//! Assertion: the `PlanMode` attachment is injected into history
//! (cadence-controlled by `ThrottleManager`; first turn always
//! emits the full content version).

use anyhow::Result;
use coco_types::AttachmentKind;
use coco_types::PermissionMode;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        permission_mode: PermissionMode::Plan,
        max_turns: Some(2),
        ..SessionConfig::default()
    };
    let outcome = run_session(provider, model, cfg, "Reply with one word: ok").await?;

    reminders::assert_reminder_present(
        &outcome.result.final_messages,
        AttachmentKind::PlanMode,
        &format!("{provider}/{model} plan_mode"),
    );
    Ok(())
}
