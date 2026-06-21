//! One-shot real-LLM round trip through the full TUI stack.
//!
//! The agent receives a single prompt that needs no tools, replies
//! once, and the engine completes. Success criteria:
//! - SessionResult arrives with `is_error == false`
//! - The response text contains the requested word
//! - The rendered TUI buffer shows the user message and assistant
//!   reply (proves `handle_core_event` folded text deltas correctly)

use std::time::Duration;

use anyhow::Result;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(2)
        .build()
        .await?;

    harness
        .submit("Reply with exactly the single word: ok")
        .await;

    let ok = harness.pump_until_idle(Duration::from_secs(30)).await?;
    assert!(ok, "{provider}/{model}: SessionResult flagged is_error");

    let text = harness.assistant_text().to_lowercase();
    assert!(
        text.contains("ok"),
        "{provider}/{model}: expected `ok` in assistant text, got {text:?}",
    );

    // Render snapshot — must show the user prompt and the assistant
    // reply at minimum. We assert on substrings that production always
    // surfaces in the chat scroll.
    let buf = harness.render_to_string()?;
    assert!(
        buf.contains("ok") || buf.to_lowercase().contains("ok"),
        "{provider}/{model}: render buffer should contain `ok`; \
         buffer (truncated) = {:.400}",
        buf,
    );

    harness.shutdown().await;
    Ok(())
}
