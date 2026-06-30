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
use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::ToolResultOutput;
use coco_messages::UserContent;
use coco_types::TokenUsage;
use std::borrow::Borrow;

use crate::types::AUTOCOMPACT_BUFFER_TOKENS;
use crate::types::ERROR_THRESHOLD_BUFFER_TOKENS;
use crate::types::MANUAL_COMPACT_BUFFER_TOKENS;
use crate::types::MAX_OUTPUT_TOKENS_FOR_SUMMARY;
use crate::types::TokenWarningState;
use crate::types::WARNING_THRESHOLD_BUFFER_TOKENS;

pub const AUTO_COMPACT_WINDOW_MIN_TOKENS: i64 = 100_000;
pub const AUTO_COMPACT_WINDOW_MAX_TOKENS: i64 = 1_000_000;
pub const MODEL_DEFAULT_AUTO_COMPACT_WINDOW_TOKENS: i64 = 200_000;
pub const DEFAULT_PRECOMPUTE_BUFFER_FRACTION: f64 = 0.2;

const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

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

/// Diagnostic returned when the fixed prompt prefix is already larger
/// than the auto-compact threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixOverflowReport {
    pub prefix_tokens: i64,
    pub threshold_tokens: i64,
    pub total_input_tokens: i64,
    pub messages_estimate: i64,
    pub snip_tokens_freed: i64,
    pub document_block_count: i32,
    pub image_block_count: i32,
}

/// Source chosen by the auto-compact window resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCompactWindowSource {
    Env,
    Settings,
    ClientData,
    Experiment,
    ModelDefault,
    Auto,
}

impl AutoCompactWindowSource {
    #[must_use]
    pub fn is_configured(self) -> bool {
        matches!(
            self,
            Self::Env | Self::Settings | Self::ClientData | Self::ModelDefault
        )
    }
}

/// Explicit user/env configured window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredAutoCompactWindow {
    pub window: i64,
    pub source: AutoCompactWindowSource,
}

/// Server-pushed window plus whether the dedicated cache key was present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientDataAutoCompactWindow {
    pub window: Option<i64>,
    pub replaces_model_default: bool,
}

/// Inputs for Claude Code's six-source auto-compact window resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoCompactWindowInputs<'a> {
    pub hard_cap: i64,
    pub configured_override: Option<ConfiguredAutoCompactWindow>,
    pub clientdata: ClientDataAutoCompactWindow,
    pub experiment_window: Option<i64>,
    pub model_id: Option<&'a str>,
    pub one_million_credits_clamped: bool,
    pub model_default_window: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactWindowResolution {
    pub window: i64,
    pub configured: i64,
    pub source: AutoCompactWindowSource,
}

/// Surface-specific precompute tuning target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecomputeSurface {
    Repl,
    Sdk,
}

/// Source selected by the precompute buffer fraction resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecomputeArmSource {
    Scalar,
    Malformed,
    TableNoMatch,
    TableExact,
    TableDefault,
}

/// Inputs for Claude Code's `tengu_amber_moleskin` arm-table resolver.
#[derive(Debug, Clone, Copy)]
pub struct PrecomputeArmInputs<'a> {
    pub resolved_window: i64,
    pub surface: PrecomputeSurface,
    pub scalar_fraction: Option<f64>,
    pub arm_table: Option<&'a serde_json::Value>,
}

/// Resolved precompute fraction plus the source that won.
#[derive(Debug, Clone, PartialEq)]
pub struct PrecomputeArmResolution {
    pub fraction: f64,
    pub source: PrecomputeArmSource,
    pub matched_window_key: Option<i64>,
    pub malformed_payload_type: Option<&'static str>,
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

/// Resolve the raw auto-compact context window before output-token reserve.
///
/// Mirrors Claude Code's source precedence:
/// env/settings override > clientdata > experiment > model-default > auto.
/// The function is pure: config/env/clientdata/provider state must already
/// be converted into [`AutoCompactWindowInputs`] by the caller.
#[must_use]
pub fn resolve_auto_compact_window(
    inputs: AutoCompactWindowInputs<'_>,
) -> AutoCompactWindowResolution {
    let hard_cap = inputs.hard_cap.max(0);

    if let Some(configured) = inputs
        .configured_override
        .filter(|configured| configured.window > 0)
    {
        return resolved_window(hard_cap, configured.window, configured.source);
    }

    if let Some(window) = valid_window_source_value(inputs.clientdata.window) {
        return resolved_window(hard_cap, window, AutoCompactWindowSource::ClientData);
    }

    if let Some(window) = valid_window_source_value(inputs.experiment_window) {
        return resolved_window(hard_cap, window, AutoCompactWindowSource::Experiment);
    }

    if hard_cap < AUTO_COMPACT_WINDOW_MAX_TOKENS
        && (is_static_model_default_window_model(inputs.model_id)
            || inputs.one_million_credits_clamped)
    {
        return resolved_window(
            hard_cap,
            MODEL_DEFAULT_AUTO_COMPACT_WINDOW_TOKENS,
            AutoCompactWindowSource::ModelDefault,
        );
    }

    if !inputs.clientdata.replaces_model_default
        && let Some(window) = valid_window_source_value(inputs.model_default_window)
    {
        return resolved_window(hard_cap, window, AutoCompactWindowSource::ModelDefault);
    }

    resolved_window(hard_cap, hard_cap, AutoCompactWindowSource::Auto)
}

/// Resolve the precompute buffer fraction.
///
/// Mirrors Claude Code's table layer: absent table uses the scalar path,
/// malformed table falls back to scalar with a diagnostic source, exact
/// `windowSize` beats `default`, and a valid table with no match falls back
/// to scalar. The function is pure: callers thread any feature-flag payload
/// and can emit telemetry from `malformed_payload_type` if desired.
#[must_use]
pub fn resolve_precompute_arm(inputs: PrecomputeArmInputs<'_>) -> PrecomputeArmResolution {
    let scalar = scalar_precompute_fraction(inputs.scalar_fraction);
    let Some(raw_table) = inputs.arm_table else {
        return PrecomputeArmResolution {
            fraction: scalar,
            source: PrecomputeArmSource::Scalar,
            matched_window_key: None,
            malformed_payload_type: None,
        };
    };

    let Some(table) = parse_precompute_arm_table(raw_table) else {
        return PrecomputeArmResolution {
            fraction: scalar,
            source: PrecomputeArmSource::Malformed,
            matched_window_key: None,
            malformed_payload_type: Some(json_payload_type(raw_table)),
        };
    };

    if let Some(entry) = table
        .entries
        .iter()
        .find(|entry| entry.window_size == inputs.resolved_window)
    {
        return PrecomputeArmResolution {
            fraction: entry.fraction_for(inputs.surface),
            source: PrecomputeArmSource::TableExact,
            matched_window_key: Some(entry.window_size),
            malformed_payload_type: None,
        };
    }

    if let Some(entry) = table.default_entry {
        return PrecomputeArmResolution {
            fraction: entry.fraction_for(inputs.surface),
            source: PrecomputeArmSource::TableDefault,
            matched_window_key: None,
            malformed_payload_type: None,
        };
    }

    PrecomputeArmResolution {
        fraction: scalar,
        source: PrecomputeArmSource::TableNoMatch,
        matched_window_key: None,
        malformed_payload_type: None,
    }
}

fn scalar_precompute_fraction(fraction: Option<f64>) -> f64 {
    fraction
        .filter(|fraction| is_valid_precompute_fraction(*fraction))
        .unwrap_or(DEFAULT_PRECOMPUTE_BUFFER_FRACTION)
}

#[derive(Debug, Clone, Copy)]
struct PrecomputeArmEntry {
    window_size: i64,
    repl: f64,
    sdk: f64,
}

impl PrecomputeArmEntry {
    fn fraction_for(self, surface: PrecomputeSurface) -> f64 {
        match surface {
            PrecomputeSurface::Repl => self.repl,
            PrecomputeSurface::Sdk => self.sdk,
        }
    }
}

#[derive(Debug, Clone)]
struct PrecomputeArmTable {
    entries: Vec<PrecomputeArmEntry>,
    default_entry: Option<PrecomputeArmEntry>,
}

fn parse_precompute_arm_table(raw: &serde_json::Value) -> Option<PrecomputeArmTable> {
    let object = raw.as_object()?;
    if object.is_empty() {
        return None;
    }

    let mut entries = Vec::new();
    let mut default_entry = None;
    for (key, value) in object {
        let parsed = parse_precompute_arm_entry(value)?;
        if key == "default" {
            default_entry = Some(parsed);
            continue;
        }
        let window_size = parse_js_safe_integer_key(key)?;
        if window_size <= 0 || window_size > JS_MAX_SAFE_INTEGER {
            return None;
        }
        entries.push(PrecomputeArmEntry {
            window_size,
            ..parsed
        });
    }

    if entries.is_empty() && default_entry.is_none() {
        return None;
    }

    Some(PrecomputeArmTable {
        entries,
        default_entry,
    })
}

fn parse_js_safe_integer_key(key: &str) -> Option<i64> {
    let value = key.trim().parse::<f64>().ok()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    if value < i64::MIN as f64 || value > i64::MAX as f64 {
        return None;
    }
    Some(value as i64)
}

fn parse_precompute_arm_entry(value: &serde_json::Value) -> Option<PrecomputeArmEntry> {
    let object = value.as_object()?;
    let repl = object
        .get("repl")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| is_valid_precompute_fraction(*value))?;
    let sdk = object
        .get("sdk")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| is_valid_precompute_fraction(*value))?;
    Some(PrecomputeArmEntry {
        window_size: 0,
        repl,
        sdk,
    })
}

fn is_valid_precompute_fraction(value: f64) -> bool {
    value.is_finite() && (0.0..1.0).contains(&value)
}

fn json_payload_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "object",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn resolved_window(
    hard_cap: i64,
    configured: i64,
    source: AutoCompactWindowSource,
) -> AutoCompactWindowResolution {
    AutoCompactWindowResolution {
        window: hard_cap.min(configured).max(0),
        configured,
        source,
    }
}

fn valid_window_source_value(window: Option<i64>) -> Option<i64> {
    window.filter(|window| {
        *window >= AUTO_COMPACT_WINDOW_MIN_TOKENS && *window <= AUTO_COMPACT_WINDOW_MAX_TOKENS
    })
}

fn is_static_model_default_window_model(model_id: Option<&str>) -> bool {
    let Some(model_id) = model_id.map(str::trim) else {
        return false;
    };
    matches!(model_id, "claude-sonnet-4-6" | "claude-opus-4-6")
}

/// Generic "model-card max wins" clamp.
///
/// A configured / over-large context window (from `QueryEngineConfig` or
/// the `COCO_COMPACT_AUTO_WINDOW` override) can never exceed the active
/// model's authoritative per-model cap (`ModelInfo.context_window`). When
/// the model max is unknown (`None`) or non-positive the configured value
/// passes through unchanged — the clamp only ever tightens, never widens.
///
/// Provider-agnostic by construction: the only input is the model-card
/// token count threaded in by the caller. Any provider-billing-specific
/// clamp-back (e.g. Anthropic 1M-credits latch) belongs in
/// `vercel-ai-anthropic`, not here.
#[must_use]
pub fn clamp_to_model_max(context_window: i64, model_max: Option<i64>) -> i64 {
    match model_max.filter(|v| *v > 0) {
        Some(max) => context_window.min(max),
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

/// Detect a fixed-prefix overflow before attempting auto-compaction.
///
/// This mirrors Claude Code's non-blocking prefix-overflow probe: use
/// the latest assistant usage as the authoritative billed input, subtract
/// the locally-estimated message payload and any already-freed snip
/// tokens, then compare the remaining fixed prefix against the same
/// threshold used by the auto-compact gate. A returned report is
/// diagnostic only; callers should log it and continue with the normal
/// breaker/compaction flow.
#[must_use]
pub fn prefix_overflow_check<M: Borrow<Message>>(
    messages: &[M],
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
    snip_tokens_freed: i64,
) -> Option<PrefixOverflowReport> {
    let usage = latest_assistant_usage(messages)?;
    let total_input_tokens = total_input_tokens_for_prefix(usage);
    let messages_estimate = coco_messages::estimate_tokens_for_messages(messages);
    let prefix_tokens = total_input_tokens
        .saturating_sub(snip_tokens_freed.max(0))
        .saturating_sub(messages_estimate)
        .max(0);
    let threshold_tokens = auto_compact_threshold(context_window, max_output_tokens, cfg);

    if prefix_tokens <= threshold_tokens {
        return None;
    }

    let mut media_counts = MediaBlockCounts::default();
    for message in messages {
        tally_message_media(message.borrow(), &mut media_counts);
    }

    Some(PrefixOverflowReport {
        prefix_tokens,
        threshold_tokens,
        total_input_tokens,
        messages_estimate,
        snip_tokens_freed: snip_tokens_freed.max(0),
        document_block_count: media_counts.documents,
        image_block_count: media_counts.images,
    })
}

fn latest_assistant_usage<M: Borrow<Message>>(messages: &[M]) -> Option<TokenUsage> {
    messages
        .iter()
        .rev()
        .find_map(|message| match message.borrow() {
            Message::Assistant(assistant) => assistant.usage,
            Message::User(_)
            | Message::ToolResult(_)
            | Message::System(_)
            | Message::Progress(_)
            | Message::Attachment(_)
            | Message::Tombstone(_) => None,
        })
}

fn total_input_tokens_for_prefix(usage: TokenUsage) -> i64 {
    let buckets = usage
        .input_tokens
        .no_cache
        .saturating_add(usage.input_tokens.cache_read)
        .saturating_add(usage.input_tokens.cache_write);
    usage.input_tokens.total.max(buckets)
}

#[derive(Default)]
struct MediaBlockCounts {
    documents: i32,
    images: i32,
}

fn tally_message_media(message: &Message, counts: &mut MediaBlockCounts) {
    match message {
        Message::User(user) => tally_llm_message_media(&user.message, counts),
        Message::Assistant(assistant) => tally_llm_message_media(&assistant.message, counts),
        Message::ToolResult(tool) => tally_llm_message_media(&tool.message, counts),
        Message::Attachment(attachment) => {
            if let Some(message) = attachment.as_api_message() {
                tally_llm_message_media(message, counts);
            }
        }
        Message::System(_) | Message::Progress(_) | Message::Tombstone(_) => {}
    }
}

fn tally_llm_message_media(message: &LlmMessage, counts: &mut MediaBlockCounts) {
    match message {
        LlmMessage::System { content, .. }
        | LlmMessage::Developer { content, .. }
        | LlmMessage::User { content, .. } => tally_user_content_media(content, counts),
        LlmMessage::Assistant { content, .. } => tally_assistant_content_media(content, counts),
        LlmMessage::Tool { content, .. } => tally_tool_content_media(content, counts),
    }
}

fn tally_user_content_media(content: &[UserContent], counts: &mut MediaBlockCounts) {
    for part in content {
        if let UserContent::File(file) = part {
            tally_media_type(&file.media_type, counts);
        }
    }
}

fn tally_assistant_content_media(content: &[AssistantContent], counts: &mut MediaBlockCounts) {
    for part in content {
        match part {
            AssistantContent::File(file) => tally_media_type(&file.media_type, counts),
            AssistantContent::ReasoningFile(file) => tally_media_type(&file.media_type, counts),
            AssistantContent::ToolResult(tool_result) => {
                tally_tool_result_output_media(&tool_result.output, counts);
            }
            AssistantContent::Text(_)
            | AssistantContent::Reasoning(_)
            | AssistantContent::ToolCall(_)
            | AssistantContent::Custom(_)
            | AssistantContent::Source(_)
            | AssistantContent::ToolApprovalRequest(_) => {}
        }
    }
}

fn tally_tool_content_media(content: &[ToolContent], counts: &mut MediaBlockCounts) {
    for part in content {
        if let ToolContent::ToolResult(tool_result) = part {
            tally_tool_result_output_media(&tool_result.output, counts);
        }
    }
}

fn tally_tool_result_output_media(output: &ToolResultOutput, counts: &mut MediaBlockCounts) {
    if let ToolResultOutput::Content { value, .. } = output {
        for part in value {
            match part {
                coco_messages::ToolResultContentPart::FileData { media_type, .. }
                | coco_messages::ToolResultContentPart::FileUrl { media_type, .. } => {
                    tally_media_type(media_type, counts);
                }
                coco_messages::ToolResultContentPart::Text { .. }
                | coco_messages::ToolResultContentPart::FileReference { .. }
                | coco_messages::ToolResultContentPart::Custom { .. } => {}
            }
        }
    }
}

fn tally_media_type(media_type: &str, counts: &mut MediaBlockCounts) {
    if media_type.starts_with("image/") || media_type == "image" {
        counts.images += 1;
    } else {
        counts.documents += 1;
    }
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
