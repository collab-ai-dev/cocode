//! Bytes-per-token estimation heuristic.
//!
//! Distinct from the catalog: this is a coarse heuristic for sizing
//! the `/skills` token column. TS `sG(model)` in 2.1.142 hardcodes a
//! Claude-vs-other binary split (4 vs 3 bytes/token) via an explicit
//! id Set; coco-rs uses keyword matching on the Anthropic-branded
//! family names ("claude", "sonnet", "haiku", "opus") so new model
//! IDs (e.g. `claude-opus-5-0`, `claude-sonnet-4-7-fast`) classify
//! correctly without a code change.
//!
//! TS exact-match → coco-rs substring match is a *deliberate*
//! divergence: the maintenance cost of an exact-id Set is a recurring
//! drag (every Claude generation needs an entry, fast/dated variants
//! must be added, otherwise they silently fall back to `3`). The four
//! keywords are Anthropic-distinctive enough that false positives are
//! implausible for the providers coco-rs targets (OpenAI, Google,
//! ByteDance, xAI, generic OpenAI-compatible).

/// Distinctive substrings that mark a model id as Claude-family.
/// Order-insensitive — any single hit promotes the id to 4 bytes/token.
const CLAUDE_KEYWORDS: &[&str] = &["claude", "sonnet", "haiku", "opus"];

/// Estimate how many input bytes correspond to one token for `model_id`.
///
/// Returns `4` for Claude-family ids (denser BPE for English text)
/// and `3` for everything else. Empty id returns the Claude default
/// (`4`) — TS `sG` does the same so the dialog never divides by zero
/// before the model selection settles.
///
/// The value is used **only** for the visual `~N tok` column in the
/// `/skills` dialog. Real context-window accounting goes through the
/// live tokenizer in `services/inference`, never this helper.
pub fn bytes_per_token_for_model(model_id: &str) -> i64 {
    if model_id.is_empty() {
        return 4;
    }
    let lower = model_id.to_ascii_lowercase();
    if CLAUDE_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        4
    } else {
        3
    }
}

#[cfg(test)]
#[path = "bytes_per_token.test.rs"]
mod tests;
