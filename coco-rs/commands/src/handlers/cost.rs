//! `/cost` — show per-model token usage and USD cost for the session.
//!
//! The handler can't reach the live `CostTracker` (registry handlers return
//! plain strings, no runtime access), so it emits the `__COCO_COST__`
//! sentinel; the TUI / SDK runner intercepts it, reads the live multi-provider
//! [`coco_types::SessionUsageSnapshot`], and renders the breakdown via
//! `coco_messages::format_session_cost`. This replaces the previous
//! stale-session-file + Anthropic-only hardcoded-pricing path, which ignored
//! the live tracker and mispriced every non-Anthropic model as Sonnet.

use std::pin::Pin;

/// Sentinel emitted by `/cost`; the runner replaces it with the rendered live
/// session cost. See [`crate::handlers::sentinel`].
pub const COST_SENTINEL: &str = "__COCO_COST__";

/// Async handler for `/cost`. Emits the cost sentinel plus a fallback status
/// line (only shown verbatim in contexts without a runtime to intercept it).
pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        Ok(format!(
            "{COST_SENTINEL}\nSession cost is unavailable in this context."
        ))
    })
}

/// Parse a `__COCO_COST__` first line. Returns `Some(())` on match.
#[must_use]
pub fn parse_cost_sentinel(handler_output: &str) -> Option<()> {
    crate::handlers::sentinel::parse_sentinel(handler_output, COST_SENTINEL).map(|_| ())
}

#[cfg(test)]
#[path = "cost.test.rs"]
mod tests;
