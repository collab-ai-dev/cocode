//! `UltrathinkEffort` reminder — fires when the user prompt contains
//! the literal keyword `ultrathink` (word-boundary check) AND the
//! settings flag is on.

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
                ultrathink_effort: true,
                ..AttachmentSettings::default()
            },
            ..SystemReminderConfig::default()
        },
        ..SessionConfig::default()
    };

    // Embed the keyword inside a normal prompt — `contains_ultrathink_keyword`
    // matches case-insensitively on word boundaries.
    let outcome = run_session(
        provider,
        model,
        cfg,
        "Please ultrathink about this and reply with the single word: ok",
    )
    .await?;

    reminders::assert_reminder_present(
        &outcome.result.final_messages,
        AttachmentKind::UltrathinkEffort,
        &format!("{provider}/{model} ultrathink_effort"),
    );
    Ok(())
}
