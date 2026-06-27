//! Offline auto-compact *trigger* — drives the full agent loop with a
//! scripted turn whose reported token usage crosses the production
//! auto-compact threshold, then asserts the engine fired the auto path.
//!
//! Contrast with `compact_round_trip` (manual `/compact`, which calls
//! `run_manual_compact` directly): this test never touches a compact
//! entry-point. It lets a normal `submit()` turn finalize and proves
//! `QueryEngine::finalize_turn_post_tools` detects token pressure and
//! emits `ContextCompacted { trigger: Auto }`.
//!
//! The trigger is driven at the `Usage` seam (`Reply::with_usage`) —
//! the in-process analogue of codex's `ev_completed_with_tokens` SSE
//! usage events. Before this seam existed, the only way to cross the
//! threshold was a live provider (skipped in CI), so the production
//! auto-compact gate never ran offline.
//!
//! The triggering turn must call a **tool**: the auto-compact ladder
//! lives in `finalize_turn_post_tools`, which the engine only runs on
//! the tool-execution path (a no-tool turn finalizes via
//! `handle_no_tool_calls_terminal`, which skips the ladder). So we
//! script a `Bash` call carrying the high usage; once the tool result
//! is folded in, finalization sees the token pressure and compacts.

use std::time::Duration;

use anyhow::Result;
use coco_types::CoreEvent;
use coco_types::ServerNotification;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

/// The harness default context window is 200_000 with `max_output_tokens`
/// 2_048, so the production threshold is
/// `(200_000 - min(2_048, 20_000)) - 13_000 = 184_952`
/// (see `coco_compact::auto_compact_threshold`). Report well past it so
/// the post-turn `tokens_with_last_usage()` is unambiguously over.
const SCRIPTED_INPUT_TOKENS: u64 = 250_000;
const AUTO_COMPACT_THRESHOLD: i64 = 184_952;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([
            // Turn 1: a tool call (so finalization runs the auto-compact
            // ladder) whose assistant message reports a huge input-token
            // count — as if the conversation had filled the context
            // window. Once the Bash result is folded in, the post-turn
            // `tokens_with_last_usage()` crosses the threshold.
            Reply::tool_call(
                "call-trigger",
                "Bash",
                serde_json::json!({ "command": "echo auto-compact-trigger" }),
            )
            .with_usage(SCRIPTED_INPUT_TOKENS, 200),
            // Spare replies: the full-compact summarizer fork consumes one,
            // the post-tool continuation turn consumes another. Extra text
            // replies keep the model from returning an empty `stop`
            // mid-compact regardless of exact consumption order.
            Reply::text("compact-summary: prior context distilled to gist."),
            Reply::text("Done — context compacted, continuing."),
            Reply::text("Done."),
        ])
        .build()
        .await?;

    harness.submit("keep going on the long task").await;
    harness.pump_until_idle(Duration::from_secs(5)).await?;

    // The auto path emits `ContextCompacted { trigger: Auto }` the moment
    // post-turn token accounting crosses the threshold (engine_finalize_turn).
    let saw_auto = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::ContextCompacted(p))
                if matches!(p.trigger, coco_types::CompactTrigger::Auto)
        )
    });
    let methods: Vec<String> = harness
        .events
        .iter()
        .map(|e| match e {
            CoreEvent::Protocol(n) => format!("P:{:?}", n.method()),
            CoreEvent::Stream(_) => "S".to_string(),
            CoreEvent::Tui(_) => "T".to_string(),
        })
        .collect();
    assert!(
        saw_auto,
        "auto_compact_trigger: expected `ContextCompacted {{ trigger: Auto }}` \
         after the scripted turn reported {SCRIPTED_INPUT_TOKENS} input tokens — \
         drained {} events, none auto-compacted. events={methods:?}",
        harness.events.len(),
    );

    // Load-bearing: the trigger fired because real token pressure crossed
    // the real threshold, not spuriously. `pre_tokens` is the engine's
    // `tokens_with_last_usage()` at the decision point.
    let pre = harness
        .events
        .iter()
        .find_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::ContextCompacted(p))
                if matches!(p.trigger, coco_types::CompactTrigger::Auto) =>
            {
                p.pre_tokens
            }
            _ => None,
        })
        .expect("auto-compact ContextCompacted should carry pre_tokens");
    assert!(
        pre >= AUTO_COMPACT_THRESHOLD,
        "auto_compact_trigger: pre_tokens ({pre}) should be at/above the \
         auto-compact threshold ({AUTO_COMPACT_THRESHOLD}) — proves the trigger \
         keyed off real token pressure, not an unconditional fire",
    );

    // Auto-compaction happens inside turn finalization, not as an error
    // path — the session must still terminate cleanly.
    let clean = harness.events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::SessionResult(p)) if !p.is_error
        )
    });
    assert!(
        clean,
        "auto_compact_trigger: expected a clean SessionResult after auto-compact",
    );

    harness.shutdown().await;
    Ok(())
}
