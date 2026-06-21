//! Tool failure path. Scripts a `Read` on a path that doesn't exist —
//! the engine dispatches the tool, the tool returns an error result,
//! and the agent loop *still* completes cleanly with a follow-up
//! assistant message. Verifies:
//!
//! - `AgentStreamEvent::ToolUseCompleted` carries `is_error = true`
//!   (`tool_completions()` surfaces it as `(name, true)`).
//! - A `Message::ToolResult` cell with `is_error = true` lands on the
//!   engine transcript — the user-visible "tool failed" row.
//! - The engine recovers: it re-enters the loop with the tool result
//!   and the next scripted reply lands as a normal assistant text.
//! - `SessionResult.is_error` stays `false` — a failed *tool* is data,
//!   not a session-level error.
//!
//! Mirrors the production path where a model retries / explains after a
//! file-not-found Read instead of crashing the turn.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Pre-mint workdir so we can bake the absolute (non-existent) path
    // into the scripted Read input.
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-err-")
        .tempdir_in("/tmp")?;
    let missing_path = workdir.path().join("does-not-exist.txt");
    let missing_str = missing_path.to_string_lossy().into_owned();

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_replies([
            Reply::tool_call(
                "call_read_missing",
                "Read",
                json!({ "file_path": missing_str }),
            ),
            Reply::text("the file is gone — nothing to show"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("read the missing file").await;
    let ok = harness.pump_until_idle(Duration::from_secs(20)).await?;
    assert!(
        ok,
        "tool_error: SessionResult flagged is_error=true — a failed tool \
         shouldn't promote to a session-level error",
    );

    // Pre + post-tool turns: 2 model calls.
    assert_eq!(
        harness.model.call_count(),
        2,
        "tool_error: expected 2 LLM calls (pre + post tool), got {}",
        harness.model.call_count(),
    );

    // Engine surfaced the failure on the wire.
    let completions = harness.tool_completions();
    assert_eq!(
        completions,
        vec![("Read", true)],
        "tool_error: expected exactly one Read completion with is_error=true, \
         got {completions:?}",
    );

    // AppState surface: a tool-result cell for `Read` with `is_error=true`.
    let read_result = harness.find_tool_result("Read");
    let (error, is_error) = read_result.ok_or_else(|| {
        anyhow::anyhow!(
            "tool_error: missing tool-result cell for Read (cell count {})",
            harness.cell_count(),
        )
    })?;
    assert!(
        is_error && !error.is_empty(),
        "tool_error: Read result should be flagged is_error with non-empty body",
    );

    // Loop continued: the post-tool assistant text landed.
    assert!(
        harness.assistant_text_contains("the file is gone"),
        "tool_error: post-tool assistant recovery message missing — \
         engine should have re-entered the loop after the tool error",
    );

    harness.shutdown().await;
    Ok(())
}
