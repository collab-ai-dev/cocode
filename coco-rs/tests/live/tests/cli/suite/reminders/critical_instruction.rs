//! `CriticalSystemReminder` — user-supplied per-turn instruction
//! injected verbatim. Sourced from `system_reminder.critical_instruction`.

use anyhow::Result;
use coco_config::SystemReminderConfig;
use coco_types::AttachmentKind;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

const NEEDLE: &str = "🦁lion-roar-7392";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(2),
        system_reminder: SystemReminderConfig {
            critical_instruction: Some(NEEDLE.to_string()),
            ..SystemReminderConfig::default()
        },
        ..SessionConfig::default()
    };
    let outcome = run_session(provider, model, cfg, "Reply with one word: ok").await?;

    // The reminder body must contain the verbatim user-supplied marker —
    // proves the path from settings → engine config → generator → wrapped
    // attachment is intact.
    reminders::assert_reminder_contains(
        &outcome.result.final_messages,
        AttachmentKind::CriticalSystemReminder,
        NEEDLE,
        &format!("{provider}/{model} critical_instruction"),
    );
    Ok(())
}
