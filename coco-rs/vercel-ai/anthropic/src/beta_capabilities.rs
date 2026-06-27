//! Beta-capability translation: typed enum → Anthropic header string.
//!
//! Two-hop translation, deliberate:
//! 1. JSON wire (`"context_1m"`, snake_case) → `AdapterBetaCapability` enum
//!    (parsed by serde in `anthropic_messages_options`).
//! 2. `AdapterBetaCapability` → Anthropic-server beta header
//!    (e.g. `"context-1m-2025-08-07"`, kebab-case + date suffix).
//!
//! Keeping the enum at the typed boundary means coco-rs callers never
//! type the date-suffixed header literal — versions roll forward in one
//! place. The adapter is the only crate that knows the exact header
//! string Anthropic expects.
//!
//! Design §10.4.

use crate::messages::anthropic_messages_options::AdapterBetaCapability;

/// Map a typed beta capability to its Anthropic header string.
///
/// Returned strings are literal header constants; a version-bump
/// (`context-1m-2026-XX-YY`) is a one-line edit here. `None` for
/// capabilities that have no header (i.e. purely typed signaling —
/// none today; reserved for future).
pub fn map_capability(cap: AdapterBetaCapability) -> Option<&'static str> {
    Some(match cap {
        AdapterBetaCapability::Context1m => "context-1m-2025-08-07",
        AdapterBetaCapability::InterleavedThinking => "interleaved-thinking-2025-05-14",
        AdapterBetaCapability::ContextManagement => "context-management-2025-06-27",
        AdapterBetaCapability::StructuredOutputs => "structured-outputs-2025-11-13",
        AdapterBetaCapability::TokenEfficientTools => "token-efficient-tools-2026-03-28",
        AdapterBetaCapability::FastMode => "fast-mode-2026-02-01",
        AdapterBetaCapability::PromptCachingScope => "prompt-caching-scope-2026-01-05",
        AdapterBetaCapability::RedactThinking => "redact-thinking-2026-02-12",
        AdapterBetaCapability::Advisor => "advisor-2025-12-04",
        AdapterBetaCapability::ToolSearch => "tool-search-tool-2025-10-19",
    })
}

/// Baseline beta header sent on every Anthropic request the adapter
/// produces. Returned only when `agentic_query` is true; helper calls
/// (compaction, title generation) skip it.
pub const CLAUDE_CODE_BASELINE: &str = "claude-code-20250219";

#[cfg(test)]
#[path = "beta_capabilities.test.rs"]
mod tests;
