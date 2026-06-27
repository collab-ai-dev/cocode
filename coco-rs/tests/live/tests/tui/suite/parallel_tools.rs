//! Multi-tool turn: a single scripted assistant message emits two
//! `Write` tool calls. Exercises the agent loop's handling of
//! batched tool calls — the production engine dispatches them through
//! `ToolExecutor` (concurrent for safe-concurrent tools,
//! queued otherwise). Order of completion isn't load-bearing here;
//! both must finish before the engine re-enters the loop with both
//! tool results and the next scripted reply lands. Verifies:
//!
//! - Both tools start and both complete cleanly (`is_error = false`).
//! - Both target files actually exist on disk afterwards.
//! - The chat surface gets two `ToolSuccess` entries (one per call).
//! - Engine made exactly 2 model calls: turn-1 emitted both tool calls,
//!   turn-2 wrote the post-tool assistant text.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-par-")
        .tempdir_in("/tmp")?;
    let path_a = workdir.path().join("alpha.txt");
    let path_b = workdir.path().join("beta.txt");
    let path_a_str = path_a.to_string_lossy().into_owned();
    let path_b_str = path_b.to_string_lossy().into_owned();

    // Two tool calls in one assistant message — the shape a frontier
    // model produces when it batches independent file writes.
    let multi = Reply::tools([
        (
            "call_write_alpha",
            "Write",
            json!({
                "file_path": path_a_str,
                "content": "alpha-body",
            }),
        ),
        (
            "call_write_beta",
            "Write",
            json!({
                "file_path": path_b_str,
                "content": "beta-body",
            }),
        ),
    ]);

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_replies([multi, Reply::text("both files written")])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("write alpha and beta in parallel").await;
    let ok = harness.pump_until_idle(Duration::from_secs(20)).await?;
    assert!(ok, "parallel_tools: SessionResult flagged is_error");

    // Two LLM calls: one for the dual-tool turn, one for the wrap-up.
    assert_eq!(
        harness.model.call_count(),
        2,
        "parallel_tools: expected 2 LLM calls, got {}",
        harness.model.call_count(),
    );

    // Both tools observed via the wire — assert on a count map so the
    // test isn't sensitive to dispatch order.
    let starts = harness.tool_starts();
    let mut start_counts: HashMap<&str, usize> = HashMap::new();
    for s in &starts {
        *start_counts.entry(*s).or_default() += 1;
    }
    assert_eq!(
        start_counts.get("Write").copied().unwrap_or(0),
        2,
        "parallel_tools: expected 2 Write starts, got starts={starts:?}",
    );

    let completions = harness.tool_completions();
    let write_ok = completions
        .iter()
        .filter(|(name, err)| *name == "Write" && !*err)
        .count();
    assert_eq!(
        write_ok, 2,
        "parallel_tools: expected 2 successful Write completions, got {completions:?}",
    );

    // Side effect: both files landed with the right body.
    let body_a = std::fs::read_to_string(&path_a)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path_a.display()))?;
    let body_b = std::fs::read_to_string(&path_b)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path_b.display()))?;
    assert_eq!(
        body_a.trim(),
        "alpha-body",
        "parallel_tools: alpha.txt body"
    );
    assert_eq!(body_b.trim(), "beta-body", "parallel_tools: beta.txt body");

    // Two `Write` tool-result cells — the chat panel should render
    // both rows, regardless of the order they actually completed in.
    let success_count = harness.tool_result_count("Write");
    assert_eq!(
        success_count, 2,
        "parallel_tools: expected 2 Write tool-result cells, got {success_count}",
    );

    harness.shutdown().await;
    Ok(())
}
