//! Bash output capture: scripts a `Bash` tool that prints a unique
//! marker, lets the engine dispatch it through `coco-shell`, and
//! confirms the marker round-trips into both the `ToolSuccess` chat
//! entry *and* the rendered terminal buffer.
//!
//! Differs from `tool_chain.rs` (which exercises `Write`) — this one
//! verifies stdout actually flows from the spawned process back through
//! the tool result envelope and into the user-visible UI. The marker
//! is a high-entropy substring so a positive match can't be a false
//! hit from chrome / status-bar text.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Marker chosen to be improbable in TUI chrome / locale strings.
    const MARKER: &str = "coco-bash-marker-9b3f2e";

    let mut harness = TuiHarness::builder()
        .with_replies([
            Reply::text_then_tool(
                "running echo to capture output",
                "call_bash_capture",
                "Bash",
                json!({
                    "command": format!("echo {MARKER}"),
                    "description": "echo deterministic marker",
                }),
            ),
            Reply::text("captured the marker"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("echo a marker").await;
    let ok = harness.pump_until_idle(Duration::from_secs(20)).await?;
    assert!(ok, "bash_capture: SessionResult flagged is_error");

    // Bash tool ran exactly once and completed cleanly.
    let completions = harness.tool_completions();
    assert_eq!(
        completions,
        vec![("Bash", false)],
        "bash_capture: expected single clean Bash completion, got {completions:?}",
    );

    // Chat surface: the tool-result cell for Bash carries the captured
    // stdout. `is_error` must be false (Bash exited 0).
    let (output, is_error) = harness
        .find_tool_result("Bash")
        .ok_or_else(|| anyhow::anyhow!("bash_capture: missing ToolResult(Bash) cell"))?;
    assert!(!is_error, "bash_capture: Bash result flagged is_error");
    assert!(
        output.contains(MARKER),
        "bash_capture: tool-result output missing marker `{MARKER}`:\n{output}",
    );

    // Render side: marker reaches the terminal buffer too. A pure
    // state-side assertion would miss bugs where the chat-panel widget
    // truncates / hides the output preview.
    let rendered = harness.render_to_string()?;
    assert!(
        rendered.contains(MARKER),
        "bash_capture: rendered buffer missing marker `{MARKER}`:\n{rendered}",
    );

    harness.shutdown().await;
    Ok(())
}
