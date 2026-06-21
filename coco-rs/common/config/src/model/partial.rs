//! `PartialModelInfo` — wire format for `ModelInfo`.
//!
//! Every field is `Option<_>` to distinguish "unset" from "explicitly
//! set to zero / empty". Required fields (`context_window`,
//! `max_output_tokens`) become `ConfigError::MissingContextWindow` /
//! `MissingMaxOutputTokens` at `from_partial` time if absent from the
//! whole merge chain.
//!
//! `BTreeMap` for `extra_body` so serialised output is deterministic
//! across snapshots and review diffs.

use crate::positive::PositiveCount;
use crate::positive::PositiveTokens;
use coco_types::ApplyPatchToolType;
use coco_types::Capability;
use coco_types::ModelShellToolType;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use coco_types::ToolOverrides;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

/// Wire format. Identity is the parent map key in `models.json` /
/// `providers.<name>.models`; this struct intentionally has no
/// `model_id` field — `serde(deny_unknown_fields)` rejects it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialModelInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    // === Capacity ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<PositiveTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<PositiveTokens>,
    /// Optional per-model escalate ceiling for `MaxTokens` recovery.
    /// See [`super::ModelInfo::max_output_tokens_escalate`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens_escalate: Option<PositiveTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<i64>,

    // === Capabilities ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<Capability>>,

    /// Exclude this model from prompt-cache-break detection. See
    /// [`super::ModelInfo::cache_break_detection_excluded`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_break_detection_excluded: Option<bool>,

    // === Sampling ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<PositiveCount>,

    // === Thinking / Reasoning ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_thinking_level: Option<ReasoningEffort>,

    // === Context Management ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_compact_pct: Option<i32>,

    // === Tools ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_patch_tool_type: Option<ApplyPatchToolType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_overrides: Option<ToolOverrides>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_tool_type: Option<ModelShellToolType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_output_chars: Option<i32>,

    // === Instructions ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions_file: Option<String>,

    // === Layer 1 escape hatch ===
    /// Provider-agnostic flat keys, **camelCase** to match each
    /// provider's typed-options struct (`#[serde(rename_all = "camelCase")]`
    /// on `AnthropicProviderOptions` / `OpenAIResponsesProviderOptions` / …).
    /// Layer 2 wraps this under `provider_options[<provider_name>]` at
    /// `build_call_options` time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<BTreeMap<String, serde_json::Value>>,
}

impl PartialModelInfo {
    /// Whether the partial holds no overlay information. Used to skip
    /// serialization of an empty `info: { ... }` nested under
    /// [`crate::provider::ProviderModelOverride`]. Pattern-matches every
    /// field exhaustively so adding a field surfaces as a compile
    /// error here rather than silently breaking round-trip stability.
    ///
    /// **Invariant:** `PartialModelInfo::default().is_empty()` must
    /// hold. If a future field gains a non-`None` `Default`, both
    /// [`Self::is_empty`] and the wire-format round-trip must be
    /// updated. The test
    /// `partial_default_is_empty` asserts this invariant.
    pub fn is_empty(&self) -> bool {
        matches!(
            self,
            PartialModelInfo {
                display_name: None,
                context_window: None,
                max_output_tokens: None,
                max_output_tokens_escalate: None,
                timeout_secs: None,
                capabilities: None,
                cache_break_detection_excluded: None,
                temperature: None,
                top_p: None,
                top_k: None,
                supported_thinking_levels: None,
                default_thinking_level: None,
                auto_compact_pct: None,
                apply_patch_tool_type: None,
                tool_overrides: None,
                shell_tool_type: None,
                max_tool_output_chars: None,
                base_instructions: None,
                base_instructions_file: None,
                extra_body: None,
            }
        )
    }

    /// Layer overlay onto `self`: each `Some` field in `overlay` wins;
    /// each `None` field leaves `self` untouched. `extra_body` merges
    /// key-by-key (overlay wins per key).
    pub fn merge_from(&mut self, overlay: &PartialModelInfo) {
        macro_rules! merge_opt {
            ($field:ident) => {
                if overlay.$field.is_some() {
                    self.$field.clone_from(&overlay.$field);
                }
            };
        }
        merge_opt!(display_name);
        merge_opt!(context_window);
        merge_opt!(max_output_tokens);
        merge_opt!(max_output_tokens_escalate);
        merge_opt!(timeout_secs);
        merge_opt!(capabilities);
        merge_opt!(cache_break_detection_excluded);
        merge_opt!(temperature);
        merge_opt!(top_p);
        merge_opt!(top_k);
        merge_opt!(supported_thinking_levels);
        merge_opt!(default_thinking_level);
        merge_opt!(auto_compact_pct);
        merge_opt!(apply_patch_tool_type);
        merge_opt!(tool_overrides);
        merge_opt!(shell_tool_type);
        merge_opt!(max_tool_output_chars);
        merge_opt!(base_instructions);
        merge_opt!(base_instructions_file);
        if let Some(extras) = &overlay.extra_body
            && !extras.is_empty()
        {
            let acc = self.extra_body.get_or_insert_with(BTreeMap::new);
            for (k, v) in extras {
                acc.insert(k.clone(), v.clone());
            }
        }
        // Normalise: an explicitly-empty extra_body collapses to None
        // so round-tripping `Some(empty)` through `merge_from` doesn't
        // change wire-format presence. Stable for snapshots.
        if matches!(&self.extra_body, Some(b) if b.is_empty()) {
            self.extra_body = None;
        }
    }
}

#[cfg(test)]
#[path = "partial.test.rs"]
mod tests;
