//! `/compact` — manual full LLM-summarized compaction.
//!
//! Slash-command handlers in this crate are pure functions returning a
//! status string; they don't hold a `QueryEngine` reference. This
//! handler emits a sentinel control prefix so the TUI / SDK runner
//! recognizes the request and dispatches it to
//! [`coco_query::QueryEngine::run_manual_compact`]. The trailing text
//! is shown verbatim to the user as confirmation.
//!
//! The sentinel format is one line:
//!   `__COCO_COMPACT_NOW__ <custom_instructions>\n<status text>`
//! Runners parse the first line, drop it from displayed output, and
//! drive the engine. If a runner doesn't understand the sentinel it
//! falls back to displaying both lines — no crash, just a no-op.

use std::pin::Pin;

/// Sentinel prefix recognised by SDK / TUI runners. The text after the
/// prefix (until newline) is the optional `custom_instructions`.
pub const COMPACT_SENTINEL: &str = "__COCO_COMPACT_NOW__";

/// Parsed compact request extracted from a handler output's sentinel
/// line. Runners obtain this by calling [`parse_compact_sentinel`] on
/// the handler's first line of output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactRequest {
    /// The user's `/compact <instructions>` argument, trimmed; empty
    /// string when no argument was supplied.
    pub custom_instructions: String,
    /// Remaining lines of the handler output (status text shown in the
    /// transcript). Runners typically display this verbatim.
    pub display_text: String,
}

/// Parse a [`COMPACT_SENTINEL`]-prefixed handler output into a
/// `CompactRequest`. Returns `None` when the input does not begin with
/// the sentinel — the runner should then treat the output as ordinary
/// command text.
///
/// Rust uses a sentinel because the slash-command registry
/// returns plain strings. This helper centralizes the parse so both
/// `tui_runner` and `sdk_runner` consume it identically.
#[must_use]
pub fn parse_compact_sentinel(handler_output: &str) -> Option<CompactRequest> {
    let parsed = super::sentinel::parse_sentinel(handler_output, COMPACT_SENTINEL)?;
    Some(CompactRequest {
        custom_instructions: parsed.args.to_string(),
        display_text: parsed.status.to_string(),
    })
}

/// Async handler for `/compact [instructions]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let custom_instructions = args.trim();
        let mut out = format!("{COMPACT_SENTINEL} {custom_instructions}\n");
        out.push_str("Compacting conversation…\n");
        if !custom_instructions.is_empty() {
            out.push_str(&format!("Summarization focus: {custom_instructions}\n"));
        }
        out.push_str("Older messages will be summarized into a compact representation; ");
        out.push_str("the assistant retains key context from the full conversation.");
        Ok(out)
    })
}

#[cfg(test)]
#[path = "compact.test.rs"]
mod tests;
