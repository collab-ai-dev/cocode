//! Auto-trigger logic for compaction.
//!
//! Threshold formula:
//!   effectiveWindow = contextWindow - min(maxOutputTokens, 20K)
//!   autoCompactThreshold = effectiveWindow - 13K
//!
//! Env vars are read once at startup by `coco_config::CompactConfig::resolve`
//! and threaded through here as plain fields — this module does not touch
//! the environment.

use coco_config::AutoCompactConfig;
pub use coco_config::TimeBasedMcConfig;

use crate::types::AUTOCOMPACT_BUFFER_TOKENS;
use crate::types::ERROR_THRESHOLD_BUFFER_TOKENS;
use crate::types::MANUAL_COMPACT_BUFFER_TOKENS;
use crate::types::MAX_OUTPUT_TOKENS_FOR_SUMMARY;
use crate::types::TokenWarningState;
use crate::types::WARNING_THRESHOLD_BUFFER_TOKENS;

/// Compaction recursion guard tag identifying the caller's query source.
///
/// Typed enum — `Other` is the catch-all for any source not requiring
/// guarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactQuerySource {
    /// Forked agent extracting session memory; must not auto-compact
    /// (would deadlock the parent).
    SessionMemory,
    /// The compact LLM call itself; must not nest.
    Compact,
    /// ctx-agent (marble_origami) spawn; must not auto-compact when its
    /// own context blows up (would destroy the main-thread commit log
    /// it shares module-level state with).
    MarbleOrigami,
    /// Any other source (main thread, subagents, SDK).
    Other,
}

/// Whether auto-compaction is currently allowed.
///
/// Single predicate that fuses the user toggle (`enabled`) with both env
/// kill switches (`DISABLE_COMPACT` / `DISABLE_AUTO_COMPACT`). Use
/// [`AutoCompactConfig::is_active`] in callers; this wrapper exists so
/// downstream code that only has the bool-ish view stays terse.
#[must_use]
pub fn is_auto_compact_enabled(cfg: &AutoCompactConfig) -> bool {
    cfg.is_active()
}

/// Apply the optional `CLAUDE_CODE_AUTO_COMPACT_WINDOW` cap.
///
/// Pure function: caller threads the resolved override (or `None`).
#[must_use]
pub fn apply_context_window_override(context_window: i64, override_window: Option<i64>) -> i64 {
    match override_window.filter(|v| *v > 0) {
        Some(v) => context_window.min(v),
        None => context_window,
    }
}

/// Compute the effective context window size after reserving space for
/// summary output.
#[must_use]
pub fn effective_context_window(
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
) -> i64 {
    let context_window = apply_context_window_override(context_window, cfg.context_window_override);
    let reserved = max_output_tokens.min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    (context_window - reserved).max(0)
}

/// Compute the auto-compact trigger threshold.
///
/// Honors the percentage override (1-100) when set on the config.
#[must_use]
pub fn auto_compact_threshold(
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
) -> i64 {
    let effective = effective_context_window(context_window, max_output_tokens, cfg);
    let default_threshold = (effective - AUTOCOMPACT_BUFFER_TOKENS).max(0);

    if let Some(pct) = cfg.pct_override.filter(|p| *p > 0.0 && *p <= 100.0) {
        let percentage_threshold = ((effective as f64) * (pct / 100.0)).floor() as i64;
        return percentage_threshold.min(default_threshold);
    }

    default_threshold
}

/// Check if auto-compaction should be triggered.
///
/// Returns true when `tokens >= effectiveWindow - 13K`.
#[must_use]
pub fn should_auto_compact(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
) -> bool {
    if context_window <= 0 {
        return false;
    }
    current_tokens >= auto_compact_threshold(context_window, max_output_tokens, cfg)
}

/// Recursion-guarded variant of [`should_auto_compact`].
///
/// Guards `session_memory`, `compact`, and `marble_origami` query sources
/// to prevent forked agents from re-entering the compaction loop. Returns
/// `false` when auto-compact is disabled (env vars or user setting).
#[must_use]
pub fn should_auto_compact_guarded(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
    source: CompactQuerySource,
) -> bool {
    if matches!(
        source,
        CompactQuerySource::SessionMemory
            | CompactQuerySource::Compact
            | CompactQuerySource::MarbleOrigami
    ) {
        return false;
    }
    if !cfg.is_active() {
        return false;
    }
    should_auto_compact(current_tokens, context_window, max_output_tokens, cfg)
}

/// Variant of [`should_auto_compact_guarded`] that additionally honors
/// the staged-compact mutual exclusion: when `is_collapse_active` is
/// true, autocompact is suppressed so it doesn't race the staged
/// commit/spawn ladder.
#[must_use]
pub fn should_auto_compact_guarded_with_collapse(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
    source: CompactQuerySource,
    is_collapse_active: bool,
) -> bool {
    if is_collapse_active {
        return false;
    }
    should_auto_compact_guarded(
        current_tokens,
        context_window,
        max_output_tokens,
        cfg,
        source,
    )
}

/// Calculate full token warning state.
///
/// `cfg.enabled` (the user toggle) picks the warning denominator: when
/// auto-compact is OFF, the user-visible "context left" is until the
/// effective window, not the autocompact threshold. Honors
/// `cfg.blocking_limit_override` for testing.
#[must_use]
pub fn calculate_token_warning_state(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
) -> TokenWarningState {
    let effective = effective_context_window(context_window, max_output_tokens, cfg);
    let threshold = auto_compact_threshold(context_window, max_output_tokens, cfg);

    let blocking_default = (effective - MANUAL_COMPACT_BUFFER_TOKENS).max(0);
    let blocking_limit = cfg
        .blocking_limit_override
        .filter(|v| *v > 0)
        .unwrap_or(blocking_default);

    let auto_active = cfg.is_active();
    let warning_denominator = if auto_active { threshold } else { effective };

    let percent_left = if warning_denominator > 0 {
        (((warning_denominator - current_tokens).max(0) as f64 / warning_denominator as f64)
            * 100.0)
            .round() as i32
    } else {
        0
    };

    TokenWarningState {
        percent_left,
        is_above_warning_threshold: current_tokens
            >= warning_denominator - WARNING_THRESHOLD_BUFFER_TOKENS,
        is_above_error_threshold: current_tokens
            >= warning_denominator - ERROR_THRESHOLD_BUFFER_TOKENS,
        is_above_auto_compact_threshold: auto_active && current_tokens >= threshold,
        is_at_blocking_limit: current_tokens >= blocking_limit,
    }
}

/// Decision returned by [`evaluate_time_based_trigger`].
#[derive(Debug, Clone)]
pub struct TimeBasedTrigger {
    pub gap_minutes: f64,
    pub config: TimeBasedMcConfig,
}

/// Whether the time-based trigger should fire.
///
/// Returns the measured gap (minutes since last assistant) if the trigger
/// fires, otherwise `None`. Caller threads `now_ms` and `last_assistant_ms`
/// to keep the function pure.
///
/// `is_main_thread` requires an explicit main-thread query source so
/// analysis-only paths (`/context`, `/compact`, `analyzeContext`) don't
/// trigger.
#[must_use]
pub fn evaluate_time_based_trigger(
    config: &TimeBasedMcConfig,
    now_ms: i64,
    last_assistant_ms: Option<i64>,
    is_main_thread: bool,
) -> Option<TimeBasedTrigger> {
    if !config.enabled || !is_main_thread {
        return None;
    }
    let last_ms = last_assistant_ms?;
    let gap_minutes = (now_ms - last_ms) as f64 / 60_000.0;
    if !gap_minutes.is_finite() || gap_minutes < config.gap_threshold_minutes as f64 {
        return None;
    }
    Some(TimeBasedTrigger {
        gap_minutes,
        config: config.clone(),
    })
}

#[cfg(test)]
#[path = "auto_trigger.test.rs"]
mod tests;
