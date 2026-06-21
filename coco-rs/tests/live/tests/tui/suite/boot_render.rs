//! Smoke test: the harness boots, renders an initial frame, and exits
//! cleanly. No engine traffic, no scripted replies — just verifies the
//! TUI can be instantiated headlessly without panicking on the empty
//! AppState. Catches "render fails on a fresh state" regressions cheaply.

use std::time::Duration;

use anyhow::Result;

use crate::tui::harness::TuiHarness;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder().build().await?;

    let rendered = harness.render_to_string()?;
    // The buffer is the full native-surface test size — every line carries
    // `width` cells plus a trailing newline. Some glyphs (icons, box-
    // drawing characters, dim spinners) are multi-byte UTF-8, so the
    // raw byte length can exceed width*height. Assert on line count
    // and per-line cell count instead.
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(
        lines.len(),
        40,
        "expected 40 rendered lines, got {} ({} bytes)",
        lines.len(),
        rendered.len()
    );
    for (i, line) in lines.iter().enumerate() {
        let cells = line.chars().count();
        assert_eq!(
            cells,
            120,
            "row {i} has {cells} chars, expected 120 (line bytes={})",
            line.len()
        );
    }

    // Sanity: an empty AppState should still render *something* —
    // typically the input box and the model identifier strip.
    assert!(
        !rendered.trim().is_empty(),
        "empty-state render produced an all-blank buffer"
    );
    assert!(
        rendered.contains("scripted-model"),
        "boot_render: expected the scripted-model id in the chrome:\n{rendered}"
    );

    // Engine never ran — no events should have been queued.
    assert!(
        harness.events.is_empty(),
        "boot_render: expected zero events, got {}",
        harness.events.len()
    );

    // pump_until_idle would deadlock here (no engine work) — so we just
    // shut down cleanly.
    let _ = tokio::time::timeout(Duration::from_secs(2), harness.shutdown()).await;
    Ok(())
}
