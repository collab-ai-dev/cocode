//! `/summary` — manual session-memory extraction trigger.
//!
//! Forces a fresh 9-section session-memory update regardless of the
//! token / tool-call gates. The handler emits a sentinel runners parse
//! and dispatch to
//! [`coco_memory::SessionMemoryService::force`].
//!
//! Sentinel format (one line):
//!   `__COCO_SUMMARY_NOW__\n<status text>`

use std::pin::Pin;

/// Sentinel prefix runners look for to fire a session-memory extract.
pub const SUMMARY_SENTINEL: &str = "__COCO_SUMMARY_NOW__";

/// Parsed summary request — empty for now, room for future args.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SummaryRequest {
    pub display_text: String,
}

/// Parse a [`SUMMARY_SENTINEL`]-prefixed handler output.
#[must_use]
pub fn parse_summary_sentinel(handler_output: &str) -> Option<SummaryRequest> {
    let parsed = super::sentinel::parse_sentinel(handler_output, SUMMARY_SENTINEL)?;
    Some(SummaryRequest {
        display_text: parsed.status.to_string(),
    })
}

/// Async handler for `/summary`.
pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let mut out = String::from(SUMMARY_SENTINEL);
        out.push('\n');
        out.push_str("Extracting session memory…\n");
        out.push_str("Updating the 9-section session-memory file with current context. ");
        out.push_str("Skipped silently when auto-memory is disabled.");
        Ok(out)
    })
}

#[cfg(test)]
#[path = "summary.test.rs"]
mod tests;
