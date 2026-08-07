//! API-native context management config builder.
//!
//! Produces the `context_management` payload sent to providers that support
//! server-side context editing (today: Anthropic). Multi-provider dispatch
//! lives at the `services/inference` layer via the `ProviderContextEditing`
//! capability trait — providers without server-side support return `None`
//! from `encode_context_management`, and the client-side fallback
//! (`crate::micro_advanced::*`) handles the same effect at the cost of
//! cache invalidation.
//!
//! The two strategies emitted here are:
//!
//! - `clear_tool_uses_20250919` — drops tool result content / tool inputs
//!   for older turns when input tokens exceed the configured trigger.
//! - `clear_thinking_20251015` — preserves thinking blocks in past turns
//!   without re-sending their full content.

use coco_config::CompactApiNativeConfig;
use coco_types::ToolName;

use crate::types::ClearToolInputs;
use crate::types::ContextEditStrategy;
use crate::types::ThinkingKeep;

/// Default trigger threshold for `clear_tool_uses` (input tokens).
/// Matches `CompactApiNativeConfig` default.
pub const DEFAULT_API_MAX_INPUT_TOKENS: i64 = 180_000;

/// Default keep-target for `clear_tool_uses` (input tokens after clearing).
pub const DEFAULT_API_TARGET_INPUT_TOKENS: i64 = 40_000;

/// Tool names whose results are eligible for `clear_tool_inputs`.
///
/// Read/search/web tools that may have produced large but no-longer-essential
/// output.
pub const TOOLS_CLEARABLE_RESULTS: &[ToolName] = &[
    ToolName::Bash,
    ToolName::PowerShell,
    ToolName::Glob,
    ToolName::Grep,
    ToolName::Read,
    ToolName::WebFetch,
    ToolName::WebSearch,
];

/// Tool names that should be **excluded** from `clear_tool_uses`.
///
/// File-mutating tools whose tool_use inputs (the actual edit
/// specifications) carry semantic value beyond the resulting tool_result,
/// so their inputs are kept intact. `ApplyPatch` is included because its
/// input *is* the patch spec.
pub const TOOLS_EXCLUDE_FROM_CLEAR_USES: &[ToolName] = &[
    ToolName::Edit,
    ToolName::Write,
    ToolName::NotebookEdit,
    ToolName::ApplyPatch,
];

/// Per-call overrides driving [`get_api_context_management`].
///
/// Static gates (`clear_tool_results` / `clear_tool_uses` / threshold /
/// target) come from `coco_config::ApiNativeConfig`; the model-state-driven
/// fields (thinking / redact-thinking / clear-all-thinking) are passed
/// per-call by the inference layer.
#[derive(Debug, Clone, Default)]
pub struct ApiContextOptions {
    /// Whether the model has thinking enabled (skip `clear_thinking` when off).
    pub has_thinking: bool,
    /// Whether redact-thinking is active (skip `clear_thinking` — redacted
    /// blocks have no model-visible content).
    pub is_redact_thinking_active: bool,
    /// Force `clear_thinking { keep: 1 }` (triggered on long-idle / >1h gap).
    pub clear_all_thinking: bool,
    /// Whether to enable `clear_tool_uses_20250919` clearing tool result
    /// content. Sourced from `CompactApiNativeConfig.clear_tool_results`.
    pub clear_tool_results: bool,
    /// Whether to enable `clear_tool_uses_20250919` clearing entire tool use
    /// blocks (excluding write/edit tools). Sourced from
    /// `CompactApiNativeConfig.clear_tool_uses`.
    pub clear_tool_uses: bool,
    /// Trigger threshold (input tokens) for `clear_tool_uses_20250919`.
    /// Default `DEFAULT_API_MAX_INPUT_TOKENS`.
    pub trigger_threshold: i64,
    /// Keep target (input tokens) — informational; the API uses it as a
    /// hint for how aggressively to clear. Default `DEFAULT_API_TARGET_INPUT_TOKENS`.
    pub keep_target: i64,
}

impl ApiContextOptions {
    /// Build options from the resolved compact-api-native config and
    /// per-call model state.
    #[must_use]
    pub fn from_config(
        cfg: &CompactApiNativeConfig,
        has_thinking: bool,
        is_redact_thinking_active: bool,
        clear_all_thinking: bool,
    ) -> Self {
        Self {
            has_thinking,
            is_redact_thinking_active,
            clear_all_thinking,
            clear_tool_results: cfg.clear_tool_results,
            clear_tool_uses: cfg.clear_tool_uses,
            trigger_threshold: cfg.max_input_tokens,
            keep_target: cfg.target_input_tokens,
        }
    }
}

/// Build the API-native context management strategy list.
///
/// Returns an empty `Vec` when no strategies are applicable — callers
/// should treat that as "omit `context_management` from the request" so
/// the API falls back to defaults.
///
/// Output order: thinking first, tool_results second, tool_uses third,
/// for stable server-side edit application.
#[must_use]
pub fn get_api_context_management(opts: &ApiContextOptions) -> Vec<ContextEditStrategy> {
    let mut strategies = Vec::new();

    // 1. Clear thinking — skip when redact-thinking is on (no visible content).
    if opts.has_thinking && !opts.is_redact_thinking_active {
        strategies.push(ContextEditStrategy::ClearThinking {
            keep: if opts.clear_all_thinking {
                ThinkingKeep::Recent { turns: 1 }
            } else {
                ThinkingKeep::All
            },
        });
    }

    // Tool clearing strategies require explicit opt-in.
    if !opts.clear_tool_results && !opts.clear_tool_uses {
        return strategies;
    }

    let trigger = if opts.trigger_threshold > 0 {
        opts.trigger_threshold
    } else {
        DEFAULT_API_MAX_INPUT_TOKENS
    };

    // `clear_at_least = trigger - keep_target` so Anthropic frees the
    // gap rather than its default smaller cut. Skip when keep_target ≥ trigger
    // (config error: would request a negative clear).
    let keep_target = if opts.keep_target > 0 {
        opts.keep_target
    } else {
        DEFAULT_API_TARGET_INPUT_TOKENS
    };
    let clear_at_least = (trigger > keep_target).then_some(trigger - keep_target);

    // 2. Clear tool result content — clear_tool_inputs is the per-tool list.
    if opts.clear_tool_results {
        strategies.push(ContextEditStrategy::ClearToolUses {
            trigger: Some(trigger),
            keep_recent: None,
            clear_at_least,
            clear_inputs: ClearToolInputs::SpecificTools(TOOLS_CLEARABLE_RESULTS.to_vec()),
            exclude_tools: Vec::new(),
            exclude_tool_strs: Vec::new(),
        });
    }

    // 3. Clear entire tool_use blocks (excluding write/edit tools).
    if opts.clear_tool_uses {
        strategies.push(ContextEditStrategy::ClearToolUses {
            trigger: Some(trigger),
            keep_recent: None,
            clear_at_least,
            clear_inputs: ClearToolInputs::None,
            exclude_tools: TOOLS_EXCLUDE_FROM_CLEAR_USES.to_vec(),
            exclude_tool_strs: Vec::new(),
        });
    }

    strategies
}

#[cfg(test)]
#[path = "api_compact.test.rs"]
mod tests;
