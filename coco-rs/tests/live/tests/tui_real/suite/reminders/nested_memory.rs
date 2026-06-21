//! `NestedMemory` reminder — fires when the Read tool reads a file
//! inside a subdirectory that has its own `CLAUDE.md` (or any
//! recognized memory-file name). The engine's
//! `drain_nested_memory_triggers` pass runs at end-of-tool-batch and
//! traverses cwd→file, loading each subdir's memory file and emitting
//! a `NestedMemory` reminder.
//!
//! Setup:
//! 1. Workdir with a top-level `CLAUDE.md` (eagerly loaded into the
//!    system prompt — these don't fire NestedMemory).
//! 2. A subdir `subproject/` with its own `CLAUDE.md` that the eager
//!    pass does NOT walk into.
//! 3. A target file `subproject/notes.md` to Read.
//! 4. Submit a prompt that asks the model to use Read on `subproject/notes.md`.

use std::time::Duration;

use anyhow::Result;
use coco_types::AttachmentKind;

use crate::common::reminders;
use crate::tui_real::harness::RealTuiHarness;

const NESTED_MARKER: &str = "🦉nested-memory-marker-7129";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-nested-")?;
    let workdir_path = workdir.path().to_path_buf();
    // Top-level CLAUDE.md (eagerly loaded; not what we're testing).
    std::fs::write(
        workdir_path.join("CLAUDE.md"),
        "# Project\nNothing nested here.\n",
    )?;
    // Nested project with its own CLAUDE.md — this is what NestedMemory
    // surfaces.
    let subproject = workdir_path.join("subproject");
    std::fs::create_dir(&subproject)?;
    std::fs::write(
        subproject.join("CLAUDE.md"),
        format!("# Subproject\n\n{NESTED_MARKER} — read me when in this dir.\n"),
    )?;
    let target = subproject.join("notes.md");
    std::fs::write(&target, "Routine notes file.\n")?;

    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(6)
        .with_workdir(workdir)
        .build()
        .await?;

    let target_str = target.to_string_lossy().into_owned();
    let prompt = format!(
        "Your first action MUST be a Read tool call with the absolute path \
         `{target_str}`. Do NOT respond with text first; do NOT explain; just \
         issue the Read tool call. Once the file content arrives in the tool \
         result, reply `done` and stop."
    );
    harness.submit(&prompt).await;

    let _ = harness.pump_until_idle(Duration::from_secs(180)).await?;

    let history = harness.history_snapshot().await;
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::NestedMemory,
        NESTED_MARKER,
        &format!("{provider}/{model} nested_memory"),
    );

    harness.shutdown().await;
    Ok(())
}
