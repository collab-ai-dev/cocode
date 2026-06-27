//! Per-content-kind token estimation density.
//!
//! Single source of truth for "what character density should each
//! content part be charged at?" Replaces the magic numbers (100, 50,
//! 20) that used to be sprinkled across multiple estimators.
//!
//! Callers: [`crate::token_estimation::estimate_message_tokens`] (and
//! its slice variant), via [`classify_user`], [`classify_assistant`],
//! [`classify_tool_result`] + [`estimate_part`].
//!
//! Lives in `coco-messages` because token estimation is a Message
//! operation, not a context-assembly concern —
//! [`crate::MessageHistory::tokens_with_last_usage`] is the cohesive
//! method on the history type itself.
//!
//! ## Densities
//!
//! - [`ContentKind::Text`] → `chars / 4` — natural-language prose, code,
//!   reasoning text. Claude/GPT empirically average ~3.5-4 chars/token.
//! - [`ContentKind::Json`] → `chars / 2` — `serde_json::Value::to_string()`
//!   output, JSON-shaped tool inputs/results, `.json/.jsonl/.jsonc`
//!   attachments. Structured data is denser; short keys + braces yield
//!   ~2 chars/token in practice.
//! - [`ContentKind::Image`] → fixed [`IMAGE_MAX_TOKEN_SIZE`]. Max 2000×2000
//!   image = theoretical 5333 tokens via `(w*h)/750`; the conservative 2000
//!   constant ensures auto-compact triggers in time. Used for
//!   image/document/binary file parts of unknown size.

use crate::AssistantContent;
use crate::ToolResultContentPart;
use crate::ToolResultOutput;
use crate::UserContent;

/// Fixed token cost for an image / document / binary attachment part.
///
/// Sized to the theoretical max for a 2000×2000 image (5333 tokens by
/// Anthropic's `(width*height)/750` formula) but reduced to 2000 so
/// auto-compact triggers conservatively rather than late.
pub const IMAGE_MAX_TOKEN_SIZE: i64 = 2_000;

/// What density to charge a content part at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    /// `chars / 4`. Prose, code, reasoning, plain tool-result text.
    Text,
    /// `chars / 2`. Structured JSON: tool-call inputs, JSON tool
    /// results, `.json/.jsonl/.jsonc` attachments.
    Json,
    /// [`IMAGE_MAX_TOKEN_SIZE`] flat. Image / document / unknown binary
    /// part — chars are ignored.
    Image,
}

/// Convert (kind, chars) into a token estimate.
///
/// `chars` is ignored for [`ContentKind::Image`]. For Text/Json it is
/// the raw character count of the rendered payload (usually
/// `value.len()` for `&str` content or `value.to_string().len()` for
/// `serde_json::Value`).
pub fn estimate_part(kind: ContentKind, chars: i64) -> i64 {
    match kind {
        ContentKind::Text => chars / 4,
        ContentKind::Json => chars / 2,
        ContentKind::Image => IMAGE_MAX_TOKEN_SIZE,
    }
}

/// Classify a [`UserContent`] part into a single `(kind, chars)` pair.
///
/// File parts dispatch on filename extension: `.json/.jsonl/.jsonc`
/// → [`ContentKind::Json`] using the filename length as a proxy for
/// stringified body length (full body bytes aren't reachable from this
/// layer). Other extensions → [`ContentKind::Image`] fixed-cost
/// (covers images, PDFs, unknown binary).
pub fn classify_user(part: &UserContent) -> (ContentKind, i64) {
    match part {
        UserContent::Text(t) => (ContentKind::Text, t.text.len() as i64),
        UserContent::File(f) => {
            let lower = f.filename.as_deref().map(str::to_ascii_lowercase);
            let kind = match lower.as_deref() {
                Some(n)
                    if n.ends_with(".json") || n.ends_with(".jsonl") || n.ends_with(".jsonc") =>
                {
                    ContentKind::Json
                }
                _ => ContentKind::Image,
            };
            // For Json variant the filename itself is the only readable
            // measure here; Image variant ignores the char count.
            let chars = f.filename.as_deref().map_or(0, str::len) as i64;
            (kind, chars)
        }
    }
}

/// Classify an [`AssistantContent`] part into one or more
/// `(kind, chars)` pairs.
///
/// `ToolCall` produces two pairs: tool name (Text) + JSON input (Json).
/// Reasoning is text density. File-shaped variants (File, ReasoningFile,
/// Custom, Source) get fixed image cost — they may carry images or
/// arbitrary binary references that we cannot size precisely from
/// this layer.
pub fn classify_assistant(part: &AssistantContent) -> Vec<(ContentKind, i64)> {
    match part {
        AssistantContent::Text(t) => vec![(ContentKind::Text, t.text.len() as i64)],
        AssistantContent::Reasoning(r) => vec![(ContentKind::Text, r.text.len() as i64)],
        AssistantContent::ToolCall(tc) => {
            let input_chars = serde_json::to_string(&tc.input)
                .map(|s| s.len() as i64)
                .unwrap_or(0);
            vec![
                (ContentKind::Text, tc.tool_name.len() as i64),
                (ContentKind::Json, input_chars),
            ]
        }
        AssistantContent::File(_)
        | AssistantContent::ReasoningFile(_)
        | AssistantContent::Custom(_)
        | AssistantContent::Source(_) => vec![(ContentKind::Image, 0)],
        AssistantContent::ToolResult(_) => vec![(ContentKind::Text, 0)],
        // ToolApprovalRequest is a metadata-only handshake — body is
        // the approval prompt text plus tool input echoed verbatim;
        // bill at text density to avoid over-counting.
        AssistantContent::ToolApprovalRequest(_) => vec![(ContentKind::Text, 0)],
    }
}

/// Classify a [`ToolResultOutput`] payload into `(kind, chars)` pairs.
///
/// Text/Error variants → Text density. JSON / ErrorJson → Json
/// density. Mixed Content list: walks parts, Text parts → Text,
/// everything else (file_data, image, etc.) → Image fixed cost. This
/// **closes the previous 100-chars/25-tokens severe underestimate** of
/// image tool-result parts.
///
/// Naming: `ToolResultOutput` is the coco-messages re-export of
/// `coco_llm_types::ToolResultContent` — the inner enum carried by
/// the `output` field of `ToolResultContent` (= `ToolResultPart`).
pub fn classify_tool_result(part: &ToolResultOutput) -> Vec<(ContentKind, i64)> {
    match part {
        ToolResultOutput::Text { value, .. } => {
            vec![(ContentKind::Text, value.len() as i64)]
        }
        ToolResultOutput::Json { value, .. } => {
            vec![(ContentKind::Json, value.to_string().len() as i64)]
        }
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .map(|p| match p {
                ToolResultContentPart::Text { text, .. } => (ContentKind::Text, text.len() as i64),
                _ => (ContentKind::Image, 0),
            })
            .collect(),
        ToolResultOutput::ExecutionDenied { reason, .. } => {
            let chars = reason.as_deref().map_or(20, str::len) as i64;
            vec![(ContentKind::Text, chars)]
        }
        ToolResultOutput::ErrorText { value, .. } => {
            vec![(ContentKind::Text, value.len() as i64)]
        }
        ToolResultOutput::ErrorJson { value, .. } => {
            vec![(ContentKind::Json, value.to_string().len() as i64)]
        }
    }
}

#[cfg(test)]
#[path = "content_kind.test.rs"]
mod tests;
