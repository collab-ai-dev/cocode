//! Two-step scripted exchange that exercises a real tool. The model
//! emits a `Write` tool call on turn 1, the engine dispatches it
//! through `coco-tools` (writes a file in the harness tempdir), feeds
//! the result back, and the model returns a final text reply. Verifies:
//! - The agent loop drives a tool call through to completion.
//! - `AgentStreamEvent::ToolUseStarted/Completed` fold into AppState's
//!   `session.tool_executions` and a `Message::ToolResult` cell lands
//!   on the engine transcript.
//! - The TUI's chat-panel surfaces the tool name in the rendered buffer.
//! - The actual file landed on disk inside the harness workdir.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Pre-mint the workdir so we can bake its absolute path into the
    // scripted Write tool call BEFORE the harness boots. The harness's
    // builder takes ownership of the dir via `with_workdir`.
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-")
        .tempdir_in("/tmp")?;
    let target_path = workdir.path().join("greeting.txt");
    let target_path_str = target_path.to_string_lossy().into_owned();

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_replies([
            Reply::text_then_tool(
                "I'll create the file you requested.",
                "call_write_1",
                "Write",
                json!({
                    "file_path": target_path_str,
                    "content": "hello from a scripted tool call",
                }),
            ),
            Reply::text("done — file written"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness
        .submit("write `hello from a scripted tool call` to greeting.txt")
        .await;
    let ok = harness.pump_until_idle(Duration::from_secs(20)).await?;
    assert!(ok, "tool_chain: SessionResult flagged is_error");

    // Two model calls: pre-tool emission and post-tool follow-up.
    assert_eq!(
        harness.model.call_count(),
        2,
        "tool_chain: expected 2 LLM calls (pre + post tool), got {}",
        harness.model.call_count()
    );

    // Tool lifecycle events landed.
    let starts = harness.tool_starts();
    assert_eq!(
        starts,
        vec!["Write"],
        "tool_chain: expected single Write tool start, got {starts:?}"
    );
    let completions = harness.tool_completions();
    assert_eq!(
        completions,
        vec![("Write", false)],
        "tool_chain: expected Write to complete cleanly, got {completions:?}"
    );

    // Side effect: the file actually exists with the expected body.
    let written = std::fs::read_to_string(&target_path)
        .with_context_lazy(|| format!("read back {}", target_path.display()))?;
    assert_eq!(
        written.trim(),
        "hello from a scripted tool call",
        "tool_chain: file body mismatch"
    );

    // AppState surface: a tool-result cell for `Write` lives in the
    // engine transcript. Folded by `server_notification_handler::stream`
    // on `AgentStreamEvent::ToolUseCompleted`.
    let (_, is_error) = harness
        .find_tool_result("Write")
        .ok_or_else(|| anyhow::anyhow!("tool_chain: missing Write tool-result cell"))?;
    assert!(!is_error, "tool_chain: Write result flagged is_error");

    // Render side: the chat panel surfaces the tool name. Don't assert
    // on the assistant prose — different theme widths can wrap it.
    let rendered = harness.render_to_string()?;
    assert!(
        rendered.contains("Write"),
        "tool_chain: rendered buffer missing `Write` tool block:\n{rendered}"
    );

    harness.shutdown().await;
    Ok(())
}

/// Local extension trait so we don't pull `anyhow::Context` into the test
/// crate's prelude (the live suite already uses it differently).
trait ContextLazyExt<T> {
    fn with_context_lazy<F: FnOnce() -> String>(self, f: F) -> Result<T>;
}

impl<T, E: std::fmt::Display> ContextLazyExt<T> for std::result::Result<T, E> {
    fn with_context_lazy<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("{}: {e}", f()))
    }
}
