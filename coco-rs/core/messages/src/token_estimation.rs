//! Token count estimation utilities.
//!
//! Walks `Message` content parts via [`crate::content_kind`] classifiers
//! to charge each part at its idiomatic density (Text / Json / Image).
//!
//! ## Entry points
//!
//! - [`estimate_text_tokens`] — `chars / 4` over a raw string. Use for
//!   ad-hoc strings (system prompt body, skill markdown, etc.).
//! - [`estimate_tokens_for_messages`] — walk a slice of messages. Generic
//!   over `Borrow<Message>` so it accepts `&[Message]` and `&[Arc<Message>]`.
//! - [`estimate_message_tokens`] — single-message variant.
//!
//! ## Last-usage precision
//!
//! For the precision walk-back ("previous API usage + estimated tail")
//! call [`crate::MessageHistory::tokens_with_last_usage`] — the marker
//! state and the estimator are cohesive on the history type itself.

use std::borrow::Borrow;

use crate::AssistantContent;
use crate::LlmMessage;
use crate::Message;
use crate::ToolContent;
use crate::UserContent;

use crate::content_kind::ContentKind;
use crate::content_kind::classify_assistant;
use crate::content_kind::classify_tool_result;
use crate::content_kind::classify_user;
use crate::content_kind::estimate_part;

/// Estimate tokens for a raw string at the default Text density.
pub fn estimate_text_tokens(text: &str) -> i64 {
    estimate_part(ContentKind::Text, text.len() as i64)
}

/// Estimate tokens for a slice of messages.
///
/// Generic over `Borrow<Message>` so callers can pass `&[Message]` or
/// `&[Arc<Message>]` without a bridge.
pub fn estimate_tokens_for_messages<M: Borrow<Message>>(messages: &[M]) -> i64 {
    messages
        .iter()
        .map(|m| estimate_message_tokens(m.borrow()))
        .sum()
}

/// Conservative token estimate: base × 4/3 (~33% padding).
///
/// Used by compaction to ensure the post-compact budget has headroom
/// for the padded estimate even when the real API charge runs slightly
/// over the chars/4 baseline.
pub fn estimate_tokens_for_messages_conservative<M: Borrow<Message>>(messages: &[M]) -> i64 {
    estimate_tokens_for_messages(messages) * 4 / 3
}

// `estimate_llm_message_tokens` (the LlmMessage walker entry point)
// stays crate-private — exposed only to siblings like
// `estimate_tool_result_message_tokens`. External callers hold a
// `Message` or `ToolResultMessage`, not a bare `LlmMessage`, so there
// is no public-API reason to surface it.

/// Estimate tokens for a `ToolResultMessage` without cloning. Walks
/// the inner [`LlmMessage::Tool`] directly via the shared walker.
pub fn estimate_tool_result_message_tokens(tr: &crate::ToolResultMessage) -> i64 {
    llm_message_tokens(&tr.message)
}

/// Estimate tokens for a single message via the per-content-kind walker.
///
/// Walks message content parts only — no per-message role/formatting
/// overhead. The provider-side serialization overhead (role tags,
/// JSON braces) is a small constant per message that doesn't
/// meaningfully shift compaction thresholds; keeping the estimator
/// content-only simplifies test fixtures and matches the prior
/// `services/compact::estimate_tokens` semantics.
pub fn estimate_message_tokens(msg: &Message) -> i64 {
    match msg {
        // Transcript-only user messages (e.g. a slash-command echo/result
        // with `display: system`) are never sent to the model, so they
        // must not count toward the context-window / auto-compact budget.
        Message::User(u) if u.is_visible_in_transcript_only => 0,
        Message::User(u) => llm_message_tokens(&u.message),
        Message::Assistant(a) => llm_message_tokens(&a.message),
        Message::ToolResult(t) => llm_message_tokens(&t.message),
        Message::Attachment(a) => a.as_api_message().map_or(0, llm_message_tokens),
        Message::System(_) | Message::Progress(_) | Message::Tombstone(_) => 0,
    }
}

/// Check if the current token count exceeds a percentage of the
/// context window.
pub fn is_over_threshold(current_tokens: i64, context_window: i64, threshold_pct: i32) -> bool {
    if context_window <= 0 {
        return false;
    }
    let threshold = context_window * threshold_pct as i64 / 100;
    current_tokens >= threshold
}

// ── Internal walkers ────────────────────────────────────────────────

fn llm_message_tokens(msg: &LlmMessage) -> i64 {
    match msg {
        LlmMessage::System { content, .. } | LlmMessage::Developer { content, .. } => {
            sum_user_parts(content)
        }
        LlmMessage::User { content, .. } => sum_user_parts(content),
        LlmMessage::Assistant { content, .. } => sum_assistant_parts(content),
        LlmMessage::Tool { content, .. } => sum_tool_parts(content),
    }
}

fn sum_user_parts(parts: &[UserContent]) -> i64 {
    parts
        .iter()
        .map(|p| {
            let (kind, chars) = classify_user(p);
            estimate_part(kind, chars)
        })
        .sum()
}

fn sum_assistant_parts(parts: &[AssistantContent]) -> i64 {
    parts
        .iter()
        .flat_map(|p| classify_assistant(p).into_iter())
        .map(|(kind, chars)| estimate_part(kind, chars))
        .sum()
}

fn sum_tool_parts(parts: &[ToolContent]) -> i64 {
    parts
        .iter()
        .map(|p| match p {
            ToolContent::ToolResult(tr) => classify_tool_result(&tr.output)
                .into_iter()
                .map(|(kind, chars)| estimate_part(kind, chars))
                .sum::<i64>(),
            _ => 5, // small overhead for non-result tool content variants
        })
        .sum()
}

#[cfg(test)]
#[path = "token_estimation.test.rs"]
mod tests;
