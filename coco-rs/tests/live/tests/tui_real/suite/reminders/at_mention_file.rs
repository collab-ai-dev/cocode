//! `AtMentionedFiles` reminder — the engine's reminder pipeline
//! parses `@…` tokens out of the latest user message and emits this
//! reminder whenever a `MentionType::FilePath` mention is present
//! (path containing `/` or `.`).

use std::time::Duration;

use anyhow::Result;
use coco_types::AttachmentKind;

use crate::common::reminders;
use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-atmention-")?;
    let workdir_path = workdir.path().to_path_buf();
    // Plant a small file the model can be asked to "consider".
    let file_path = workdir_path.join("notes.md");
    std::fs::write(&file_path, "# Notes\n\nNothing important.\n")?;

    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(2)
        .with_workdir(workdir)
        .build()
        .await?;

    let prompt = "Consider the file @./notes.md and reply with one word: ok";
    harness.submit(prompt).await;

    let _ = harness.pump_until_idle(Duration::from_secs(60)).await?;

    let history = harness.history_snapshot().await;
    // AttachmentType::AtMentionedFiles maps to AttachmentKind::File
    // in the system-reminder→message bridge (see
    // `coco-system-reminder/src/types.rs` `From<AttachmentType>`).
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::File,
        "notes.md",
        &format!("{provider}/{model} at_mentioned_files"),
    );

    harness.shutdown().await;
    Ok(())
}
