//! Post-turn promptSuggestion service.
//!
//! ## Pipeline
//!
//! After each successful turn, `engine_finalize_turn` builds a
//! [`SuggestionContext`] from the parent app_state +
//! `last_cache_safe_params` and calls [`try_generate_suggestion`].
//! The function runs the 9-step guard sequence, and when accepted,
//! the model's reply goes through the 12-rule filter
//! ([`should_filter_suggestion`]). Surviving suggestions are emitted
//! to the TUI via `ServerNotification::PromptSuggestion`.
//!
//! ## Cache
//!
//! The fork dispatcher passes the parent's `CacheSafeParams` so the
//! API request prefix matches byte-for-byte. The 9-step guard's
//! `cache_cold` check ([`MAX_PARENT_UNCACHED_TOKENS`]) suppresses
//! when the parent's last assistant turn would force more than 10k
//! uncached tokens — without this guard, every prompt-suggestion
//! fork would burn 10k+ tokens just warming up the cache.
//!
//! ## Filter rationale
//!
//! 12 filters keep the model's output usable as a TUI placeholder:
//! evaluative ("looks good") and Claude-voice ("Let me…") replies
//! aren't things a user would type. Single-sentence + 2-12 word
//! constraints match the TUI input area.

use std::collections::HashSet;

use coco_messages::Message;
use coco_types::TokenUsage;
use once_cell::sync::Lazy;
use regex::Regex;

// ── Constants (verbatim TS) ────────────────────────────────────

/// Suppression threshold for cache-cold parent turns.
///
/// When the parent's last assistant message would force more than 10k
/// uncached tokens (normalized input minus cache read, plus output),
/// suppress the fork — the re-warm cost dwarfs any suggestion benefit.
pub const MAX_PARENT_UNCACHED_TOKENS: i64 = 10_000;

/// Words that bypass the [`SuggestionFilter::TooFewWords`] rule.
///
/// Single-word user inputs like "yes" / "commit" / "deploy" are valid
/// commands the user would actually type, so we allow them through
/// the < 2 word filter.
pub const ALLOWED_SINGLE_WORDS: &[&str] = &[
    // Affirmatives
    "yes", "yeah", "yep", "yea", "yup", "sure", "ok", "okay", // Actions
    "push", "commit", "deploy", "stop", "continue", "check", "exit", "quit", // Negation
    "no",
];

/// System prompt handed to the suggestion fork.
///
/// Loaded from a sibling `.txt` file.
pub const SUGGESTION_PROMPT: &str = include_str!("prompt_suggestion_prompt.txt");

/// Legacy alias — `engine_finalize_turn` referenced this name from
/// the pre-port days. New callers should use [`SUGGESTION_PROMPT`]
/// directly.
pub fn build_suggestion_system_prompt() -> &'static str {
    SUGGESTION_PROMPT
}

// ── Outcome types ──────────────────────────────────────────────

/// Outcome of [`try_generate_suggestion`].
///
/// Five code paths: `Accepted` records a suggestion; `Suppressed` /
/// `Filtered` / `Empty` / `Aborted` skip rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionOutcome {
    /// Suggestion passed all guards + filters. Caller renders it
    /// behind the user's TUI cursor.
    Accepted {
        text: String,
        prompt_id: String,
        request_id: Option<String>,
    },
    /// One of the 9 guards short-circuited.
    Suppressed { reason: SuppressReason },
    /// All 9 guards passed and generation produced text, but the
    /// 12-rule filter rejected it.
    Filtered { rule: SuggestionFilter },
    /// Generation succeeded but produced empty / `NONE` text.
    Empty,
    /// `currentAbortController` was cancelled mid-flight.
    Aborted,
    /// Underlying fork dispatcher errored (network, cancellation,
    /// engine-level error). Not the model's fault — caller logs
    /// and skips rendering.
    Error { kind: String },
}

/// Why the 9-step guard suppressed a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressReason {
    /// Fewer than 2 assistant turns — no suggestion context yet.
    TooFewTurns,
    /// Last assistant turn was an API error — model probably
    /// won't produce useful text.
    ApiError,
    /// Parent's last turn would force > [`MAX_PARENT_UNCACHED_TOKENS`]
    /// of cache warm-up.
    CacheCold,
    /// Disabled via settings / env.
    Disabled,
    /// Permission prompt is on screen — overlay race.
    PendingPermission,
    /// Elicitation queue is non-empty.
    ElicitationActive,
    /// Permission mode is `Plan` — user is reviewing a plan.
    PlanMode,
    /// Rate-limited (provider-side capacity).
    RateLimit,
    /// `--bare` mode skips all post-turn forks.
    BareMode,
    /// Non-interactive session (SDK / print mode) has no input area
    /// to render the suggestion into.
    NonInteractive,
    /// Swarm teammate session — only the leader should show
    /// suggestions, not workers / teammates.
    SwarmTeammate,
    /// Awaiting plan approval — the user is reviewing a plan, not
    /// composing the next prompt.
    AwaitingPlanApproval,
}

/// 12 output-filter rules from `shouldFilterSuggestion`.
///
/// Each rule has regex / predicate logic in [`should_filter_suggestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionFilter {
    /// Single word "done" — user wouldn't type this as a follow-up.
    Done,
    /// "nothing found", "no suggestion", "silence is …", bare
    /// "silence" — meta-text spelling out the silence instruction.
    MetaText,
    /// Wrapped meta `(silence — ...)` / `[no suggestion]`.
    MetaWrapped,
    /// Starts with "api error:" / "prompt is too long" / "request
    /// timed out" / "invalid api key" / "image was too large".
    ErrorMessage,
    /// `^\w+:\s` — model emitted a label prefix like "Suggestion: ".
    PrefixedLabel,
    /// Fewer than 2 words and not in [`ALLOWED_SINGLE_WORDS`] /
    /// not a `/`-prefixed slash command.
    TooFewWords,
    /// More than 12 words.
    TooManyWords,
    /// >= 100 characters.
    TooLong,
    /// `[.!?]\s+[A-Z]` — multiple sentences detected.
    MultipleSentences,
    /// Contains `\n` / `*` / `**` — model returned formatting.
    HasFormatting,
    /// Evaluative phrases ("looks good", "thanks", "perfect").
    Evaluative,
    /// Claude-voice openers ("Let me", "I'll", "Here's").
    ClaudeVoice,
}

// ── Guard input / output context ───────────────────────────────

/// Inputs to [`try_generate_suggestion`] — gates that need to be
/// evaluated *before* paying for the fork.
pub struct SuggestionContext {
    /// Number of assistant turns in the parent history. Forks
    /// wait until 2+ turns exist so suggestion has context.
    pub assistant_turn_count: u32,
    /// Whether the parent's last assistant turn was an API error.
    pub last_response_was_api_error: bool,
    /// Parent's last turn `input - cache_read + output` tokens.
    pub parent_uncached_tokens: i64,
    /// Promptsuggestion master switch (settings / env).
    pub disabled: bool,
    /// User has a permission overlay on screen.
    pub pending_permission: bool,
    /// Elicitation queue has pending items.
    pub elicitation_active: bool,
    /// Permission mode is `Plan`.
    pub plan_mode: bool,
    /// Provider-side rate limit triggered.
    pub rate_limit: bool,
    /// `--bare` mode (post-turn forks skipped).
    pub bare_mode: bool,
    /// Non-interactive session.
    pub non_interactive: bool,
    /// Swarm teammate session (only leader shows suggestions).
    pub is_teammate: bool,
    /// Plan mode + the user has already exited plan mode but the
    /// plan hasn't been approved/rejected yet.
    pub awaiting_plan_approval: bool,
}

/// Result of the underlying fork's text generation. The caller
/// passes this into the last 4 steps of [`try_generate_suggestion`].
pub struct GenerationResult {
    pub text: String,
    pub prompt_id: String,
    pub request_id: Option<String>,
}

/// Text + request id extracted from the fork's emitted messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSuggestion {
    pub text: String,
    pub request_id: Option<String>,
}

// ── 9-step guard sequence ──────────────────────────────────────

/// Returns `Some(reason)` when one of the [`SuppressReason`] variants
/// applies; `None` to proceed.
pub fn get_suggestion_suppress_reason(ctx: &SuggestionContext) -> Option<SuppressReason> {
    if ctx.disabled {
        return Some(SuppressReason::Disabled);
    }
    if ctx.non_interactive {
        return Some(SuppressReason::NonInteractive);
    }
    if ctx.is_teammate {
        return Some(SuppressReason::SwarmTeammate);
    }
    if ctx.bare_mode {
        return Some(SuppressReason::BareMode);
    }
    if ctx.pending_permission {
        return Some(SuppressReason::PendingPermission);
    }
    if ctx.awaiting_plan_approval {
        return Some(SuppressReason::AwaitingPlanApproval);
    }
    if ctx.elicitation_active {
        return Some(SuppressReason::ElicitationActive);
    }
    if ctx.plan_mode {
        return Some(SuppressReason::PlanMode);
    }
    if ctx.rate_limit {
        return Some(SuppressReason::RateLimit);
    }
    None
}

/// Pre-fork guards (steps 1-5).
///
/// Returns `None` to proceed with the fork, or
/// `Some(SuggestionOutcome::{Aborted,Suppressed})` to short-circuit
/// before paying for the API call. Production callers run this
/// **before** dispatching the fork — saving the round-trip for
/// guards that don't need the model's response (TooFewTurns,
/// ApiError, CacheCold, Disabled / PendingPermission / etc.).
pub fn pre_fork_guards(ctx: &SuggestionContext, aborted_before: bool) -> Option<SuggestionOutcome> {
    if aborted_before {
        return Some(SuggestionOutcome::Aborted);
    }
    if ctx.assistant_turn_count < 2 {
        return Some(SuggestionOutcome::Suppressed {
            reason: SuppressReason::TooFewTurns,
        });
    }
    if ctx.last_response_was_api_error {
        return Some(SuggestionOutcome::Suppressed {
            reason: SuppressReason::ApiError,
        });
    }
    if ctx.parent_uncached_tokens > MAX_PARENT_UNCACHED_TOKENS {
        return Some(SuggestionOutcome::Suppressed {
            reason: SuppressReason::CacheCold,
        });
    }
    if let Some(reason) = get_suggestion_suppress_reason(ctx) {
        return Some(SuggestionOutcome::Suppressed { reason });
    }
    None
}

/// Post-fork validation (steps 7-9).
///
/// Caller drives the fork and feeds the model's response text here.
/// Returns `None` to proceed (caller renders the suggestion), or
/// `Some(SuggestionOutcome::{Aborted,Empty,Filtered})` to drop the
/// result.
pub fn post_fork_validation(text: &str, aborted_after: bool) -> Option<SuggestionOutcome> {
    if aborted_after {
        return Some(SuggestionOutcome::Aborted);
    }
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("NONE") {
        return Some(SuggestionOutcome::Empty);
    }
    if let Some(rule) = should_filter_suggestion(trimmed) {
        return Some(SuggestionOutcome::Filtered { rule });
    }
    None
}

/// Run the full 9-step guard sequence + 12-rule filter end-to-end.
///
/// Convenience for unit tests; production callers should use
/// [`pre_fork_guards`] + dispatch + [`post_fork_validation`] so
/// the API round-trip is skipped when pre-flight fires.
pub fn try_generate_suggestion(
    ctx: &SuggestionContext,
    aborted_before: bool,
    generation: Option<GenerationResult>,
    aborted_after: bool,
) -> SuggestionOutcome {
    if let Some(outcome) = pre_fork_guards(ctx, aborted_before) {
        return outcome;
    }
    let Some(result) = generation else {
        return SuggestionOutcome::Error {
            kind: "generation_returned_none".into(),
        };
    };
    if let Some(outcome) = post_fork_validation(&result.text, aborted_after) {
        return outcome;
    }
    SuggestionOutcome::Accepted {
        text: result.text.trim().to_string(),
        prompt_id: result.prompt_id,
        request_id: result.request_id,
    }
}

// ── Multi-message text walk ────────────────────────────────────

/// Walk the fork's emitted messages and return the first non-empty
/// text block in any assistant message.
///
/// Model may loop (try tool → denied → text in next message);
/// this walk catches the text in turn 2 even when turn 1 was a
/// (denied) tool call.
pub fn extract_suggestion_text(messages: &[std::sync::Arc<Message>]) -> String {
    extract_suggestion_generation(messages).text
}

/// Extract the suggestion text plus the first assistant request id.
///
/// Captures the first assistant `requestId` for dataset joins, then
/// walks messages forward for the first non-empty text block.
pub fn extract_suggestion_generation(messages: &[std::sync::Arc<Message>]) -> ExtractedSuggestion {
    let request_id = messages.iter().find_map(|m| match m.as_ref() {
        coco_messages::Message::Assistant(a) => a.request_id.clone(),
        _ => None,
    });
    let text = messages
        .iter()
        .filter_map(|m| match m.as_ref() {
            coco_messages::Message::Assistant(a) => match &a.message {
                coco_llm_types::LlmMessage::Assistant { content, .. } => Some(content),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|content| content.iter())
        .find_map(|part| match part {
            coco_llm_types::AssistantContentPart::Text(t) => {
                let trimmed = t.text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            _ => None,
        })
        .unwrap_or_default();
    ExtractedSuggestion { text, request_id }
}

// ── 12-rule filter (verbatim regex) ────────────────────────────
//
// Static regex patterns — `expect()` is the established pattern
// for compile-time-validated regex (see
// `core/tools/src/tools/web.rs::DDG_TITLE_PATTERN`). The
// per-static `#[allow(clippy::expect_used)]` is required by the
// workspace clippy policy — every `expect` callsite must explicitly
// opt in.

#[allow(clippy::expect_used)]
static META_TEXT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    // `meta_text` rule
    vec![
        Regex::new(r"\bsilence is\b").expect("static regex compiles"),
        Regex::new(r"\bstay(s|ing)? silent\b").expect("static regex compiles"),
        Regex::new(r"^\W*silence\W*$").expect("static regex compiles"),
    ]
});

#[allow(clippy::expect_used)]
static META_WRAPPED_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\(.*\)$|^\[.*\]$").expect("static regex compiles"));

#[allow(clippy::expect_used)]
static PREFIXED_LABEL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\w+:\s").expect("static regex compiles"));

#[allow(clippy::expect_used)]
static MULTIPLE_SENTENCES_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[.!?]\s+[A-Z]").expect("static regex compiles"));

#[allow(clippy::expect_used)]
static HAS_FORMATTING_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[\n*]|\*\*").expect("static regex compiles"));

#[allow(clippy::expect_used)]
static EVALUATIVE_RE: Lazy<Regex> = Lazy::new(|| {
    // `evaluative` rule
    Regex::new(
        r"thanks|thank you|looks good|sounds good|that works|that worked|that's all|nice|great|perfect|makes sense|awesome|excellent",
    )
    .expect("static regex compiles")
});

#[allow(clippy::expect_used)]
static CLAUDE_VOICE_RE: Lazy<Regex> = Lazy::new(|| {
    // `claude_voice` rule (case-insensitive)
    Regex::new(
        r"(?i)^(let me|i'll|i've|i'm|i can|i would|i think|i notice|here's|here is|here are|that's|this is|this will|you can|you should|you could|sure,|of course|certainly)",
    )
    .expect("static regex compiles")
});

static ERROR_MESSAGE_PREFIXES: &[&str] = &[
    "api error:",
    "prompt is too long",
    "request timed out",
    "invalid api key",
    "image was too large",
];

static ALLOWED_SINGLE_WORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| ALLOWED_SINGLE_WORDS.iter().copied().collect());

/// Apply the 12 filter rules in order. Returns the first matching
/// rule (or `None` to accept).
pub fn should_filter_suggestion(text: &str) -> Option<SuggestionFilter> {
    let lower = text.to_ascii_lowercase();
    let word_count = text.split_whitespace().count();

    // Rule 1: bare "done"
    if lower == "done" {
        return Some(SuggestionFilter::Done);
    }

    // Rule 2: meta_text
    if lower == "nothing found"
        || lower == "nothing found."
        || lower.starts_with("nothing to suggest")
        || lower.starts_with("no suggestion")
        || META_TEXT_PATTERNS.iter().any(|re| re.is_match(&lower))
    {
        return Some(SuggestionFilter::MetaText);
    }

    // Rule 3: meta_wrapped — `(silence — ...)`, `[no suggestion]`
    if META_WRAPPED_RE.is_match(text) {
        return Some(SuggestionFilter::MetaWrapped);
    }

    // Rule 4: error_message
    if ERROR_MESSAGE_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return Some(SuggestionFilter::ErrorMessage);
    }

    // Rule 5: prefixed_label
    if PREFIXED_LABEL_RE.is_match(text) {
        return Some(SuggestionFilter::PrefixedLabel);
    }

    // Rule 6: too_few_words (with allow-list)
    if word_count < 2 {
        // Slash commands are valid user inputs.
        if !text.starts_with('/') && !ALLOWED_SINGLE_WORDS_SET.contains(lower.as_str()) {
            return Some(SuggestionFilter::TooFewWords);
        }
    }

    // Rule 7: too_many_words
    if word_count > 12 {
        return Some(SuggestionFilter::TooManyWords);
    }

    // Rule 8: too_long
    if text.len() >= 100 {
        return Some(SuggestionFilter::TooLong);
    }

    // Rule 9: multiple_sentences
    if MULTIPLE_SENTENCES_RE.is_match(text) {
        return Some(SuggestionFilter::MultipleSentences);
    }

    // Rule 10: has_formatting
    if HAS_FORMATTING_RE.is_match(text) {
        return Some(SuggestionFilter::HasFormatting);
    }

    // Rule 11: evaluative
    if EVALUATIVE_RE.is_match(&lower) {
        return Some(SuggestionFilter::Evaluative);
    }

    // Rule 12: claude_voice (case-insensitive in TS)
    if CLAUDE_VOICE_RE.is_match(text) {
        return Some(SuggestionFilter::ClaudeVoice);
    }

    None
}

// ── Cache-cold helper ──────────────────────────────────────────

/// Compute parent's last-turn non-cache-read token total.
///
/// Formula is `input + cache_creation + output`, where `input` is
/// the *no-cache* bucket. In coco-rs `TokenUsage::input_tokens` is
/// the normalized total `no_cache + cache_read + cache_write`, so the
/// algebraically-equivalent form `input - cache_read + output` recovers
/// `no_cache + cache_write + output` — same value, expressed against our
/// normalized representation. Caller compares against
/// [`MAX_PARENT_UNCACHED_TOKENS`].
pub fn parent_uncached_tokens(usage: &TokenUsage) -> i64 {
    usage
        .input_tokens
        .total
        .saturating_sub(usage.input_tokens.cache_read)
        .max(0)
        .saturating_add(usage.output_tokens.total)
}

#[cfg(test)]
#[path = "prompt_suggestion.test.rs"]
mod tests;
