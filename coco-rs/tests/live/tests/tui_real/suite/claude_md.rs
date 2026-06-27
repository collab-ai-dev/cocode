//! CLAUDE.md surfacing — proves the prod system-prompt assembly fed
//! the discovered project memory into the model.
//!
//! Production reads CLAUDE.md from cwd via `coco_context::discover_memory_files`
//! and splices its content into the system prompt. The test plants a
//! file with a unique magic word in the workdir cwd, then asks the
//! model what the magic word is. If the assembly is wired correctly,
//! the model can answer.

use std::time::Duration;

use anyhow::Result;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-claudemd-")?;
    let workdir_path = workdir.path().to_path_buf();

    // Plant a CLAUDE.md with a unique nonsense word — anything common
    // (e.g. "rust", "elephant") risks the model confidently answering
    // it without the prompt context.
    let claude_md = workdir_path.join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        "# Project notes\nThe magic word for this session is `flamingo`.\n",
    )?;

    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(2)
        .with_workdir(workdir)
        .build()
        .await?;

    harness
        .submit(
            "What is the magic word for this session, according to the project notes \
             (CLAUDE.md) you were briefed on? Reply with exactly that word and nothing else.",
        )
        .await;

    let ok = harness.pump_until_idle(Duration::from_secs(60)).await?;
    assert!(ok, "{provider}/{model}: SessionResult flagged is_error");

    let text = harness.assistant_text().to_lowercase();
    assert!(
        text.contains("flamingo"),
        "{provider}/{model}: CLAUDE.md content should reach the model; \
         response was: {text:?}",
    );

    harness.shutdown().await;
    Ok(())
}
