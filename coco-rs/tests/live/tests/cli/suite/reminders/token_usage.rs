//! `TokenUsage` reminder — opt-in per-turn usage report. Defaults
//! off in TS external builds; flip on via
//! `system_reminder.attachments.token_usage = true`.

use anyhow::Result;
use coco_config::SystemReminderConfig;
use coco_config::system_reminder::AttachmentSettings;
use coco_types::AttachmentKind;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(2),
        system_reminder: SystemReminderConfig {
            attachments: AttachmentSettings {
                token_usage: true,
                ..AttachmentSettings::default()
            },
            ..SystemReminderConfig::default()
        },
        ..SessionConfig::default()
    };
    let outcome = run_session(provider, model, cfg, "Reply with one word: ok").await?;

    reminders::assert_reminder_present(
        &outcome.result.final_messages,
        AttachmentKind::TokenUsage,
        &format!("{provider}/{model} token_usage"),
    );
    Ok(())
}
