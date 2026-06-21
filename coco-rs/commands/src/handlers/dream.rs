//! `/dream` — manual auto-dream consolidation trigger.
//!
//! Forces a memory-consolidation pass over the auto-memory directory
//! regardless of the three-gate scheduler (24h / 5-session / 10-min
//! throttle). The handler emits a sentinel that runners parse and
//! dispatch to [`coco_memory::MemoryRuntime`].
//!
//! Sentinel format (one line):
//!   `__COCO_DREAM_NOW__\n<status text>`
//!
//! Runners that hold a `MemoryRuntime` reference fire
//! `runtime.dream.maybe_consolidate(...)` (with a synthetic "force"
//! flag in a future iteration); runners without one display the
//! handler text and exit cleanly.

use std::pin::Pin;

/// Sentinel prefix runners look for to fire a consolidation pass.
pub const DREAM_SENTINEL: &str = "__COCO_DREAM_NOW__";

/// Parsed dream request — currently nothing besides the prefix line,
/// kept for symmetry with [`super::compact::CompactRequest`] so future
/// arguments (e.g. memory-dir override) can be added here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DreamRequest {
    pub display_text: String,
}

/// Parse a [`DREAM_SENTINEL`]-prefixed handler output. `None` when the
/// input doesn't carry the sentinel — runner falls through to ordinary
/// command-text display.
#[must_use]
pub fn parse_dream_sentinel(handler_output: &str) -> Option<DreamRequest> {
    let parsed = super::sentinel::parse_sentinel(handler_output, DREAM_SENTINEL)?;
    Some(DreamRequest {
        display_text: parsed.status.to_string(),
    })
}

/// Async handler for `/dream`.
pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let mut out = String::from(DREAM_SENTINEL);
        out.push('\n');
        out.push_str("Consolidating memory…\n");
        out.push_str("Running the auto-dream agent over your MEMORY.md and topic files. ");
        out.push_str("This merges related entries, drops duplicates, and prunes stale pointers. ");
        out.push_str("Skipped silently when auto-memory is disabled.");
        Ok(out)
    })
}

#[cfg(test)]
#[path = "dream.test.rs"]
mod tests;
