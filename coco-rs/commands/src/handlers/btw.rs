//! `/btw <question>` — by-the-way side-channel question.
//!
//! Asks a quick side question that shares the parent session's prompt
//! cache via [`coco_query::forked_agent`]. The fork is tool-less and
//! one-shot; the parent conversation is never mutated.
//!
//! The answer is surfaced as **model-invisible** content so it never
//! re-enters the LLM's view of the main conversation (the "without
//! interrupting" contract): the TUI renders it as a transcript-only
//! slash result, and the SDK appends the same transcript-only slash
//! messages. Intentional divergence from TS, whose modal is fully
//! ephemeral — coco has no modal, so the answer stays visible in
//! scrollback / SDK notifications (and the JSONL) but out of the model's
//! context.
//!
//! ## Sentinel pattern
//!
//! Slash-command handlers in this crate are pure `fn(&str) -> String` —
//! they don't hold a `QueryEngine` reference, so the actual fork has
//! to happen in the runner. The handler emits:
//!
//! ```text
//! __COCO_BTW_NOW__ <question>
//! ```
//!
//! TUI and SDK surfaces consume it through the AppServer `turn/start` handler
//! shortcut. TUI submits it over the local bridge; SDK submits it over
//! JSON-RPC. The shortcut delegates the fork + answer extraction to
//! `coco_agent_host::side_question`. Headless `-p` mode does not expand registry slash
//! commands, so it never reaches this handler.

/// Sentinel prefix runners recognise on the handler output. Text after
/// the prefix (until newline) is the user's question.
pub const BTW_SENTINEL: &str = "__COCO_BTW_NOW__";

/// Parsed `/btw` request extracted from handler output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwRequest {
    /// The user's `/btw <question>` argument, trimmed.
    pub question: String,
}

/// Parse a [`BTW_SENTINEL`]-prefixed handler output. Returns `None`
/// when the input does not begin with the sentinel or carries no question.
#[must_use]
pub fn parse_btw_sentinel(handler_output: &str) -> Option<BtwRequest> {
    let first = handler_output.lines().next()?;
    let question = first.strip_prefix(BTW_SENTINEL)?.trim().to_string();
    if question.is_empty() {
        return None;
    }
    Some(BtwRequest { question })
}

/// Sync handler — emits the sentinel carrying the question. The runner
/// picks up the sentinel and drives the actual fork.
pub fn handler(args: &str) -> String {
    let question = args.trim();
    if question.is_empty() {
        return "Usage: /btw <your question>".to_string();
    }
    format!("{BTW_SENTINEL} {question}")
}

#[cfg(test)]
#[path = "btw.test.rs"]
mod tests;
