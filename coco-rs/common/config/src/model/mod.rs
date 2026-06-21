pub mod partial;
pub mod registry;
pub mod role_slots;

pub use partial::PartialModelInfo;
pub use registry::ModelRegistry;
pub use registry::ResolvedModel;
pub use registry::build_model_registry;
pub use role_slots::ExhaustedRetryPolicy;
pub use role_slots::FallbackPolicy;
pub use role_slots::RecoveryProbePolicy;
pub use role_slots::RoleSlots;

use crate::error::ConfigError;
use crate::error::ConfigField;
use crate::positive::PositiveCount;
use crate::positive::PositiveTokens;
use coco_types::ApplyPatchToolType;
use coco_types::Capability;
use coco_types::ModelRole;
use coco_types::ModelShellToolType;
use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use coco_types::ToolOverrides;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;

/// Resolved per-model configuration. The on-disk overlay shape is
/// [`PartialModelInfo`]; this is the post-resolution form with required
/// fields concrete.
///
/// Required fields (`context_window`, `max_output_tokens`) are typed
/// `PositiveTokens` so that `as u64` casts are unrepresentable in the
/// downstream call chain — `From<PositiveTokens> for u64` is infallible.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    // === Identity ===
    pub model_id: String,
    pub display_name: Option<String>,

    // === Capacity ===
    pub context_window: PositiveTokens,
    pub max_output_tokens: PositiveTokens,
    /// Optional per-model escalate ceiling for `MaxTokens` finish_reason
    /// recovery. `None` (default) disables Phase-1 escalate for this
    /// model — recovery goes straight to the multi-turn resume-nudge
    /// (Phase-2). `Some(N)` where `N > max_output_tokens` enables a
    /// one-shot Phase-1 retry with `N` as the per-call cap. User opts
    /// in per model via `~/.coco/models.json`. Provider-agnostic.
    pub max_output_tokens_escalate: Option<PositiveTokens>,
    pub timeout_secs: Option<i64>,

    // === Capabilities ===
    pub capabilities: Option<Vec<Capability>>,

    /// Exclude this model from prompt-cache-break detection. A few models
    /// (e.g. Anthropic Haiku) have server-side caching behavior whose
    /// `cache_read` token drops are noise rather than real prefix breaks,
    /// so the detector skips them to avoid false-positive break signals.
    /// Default `false`. Declared on the model registry so
    /// `services/inference` never has to match against a model id.
    pub cache_break_detection_excluded: bool,

    // === Sampling — `Option` carries wire semantics ("let provider default") ===
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<PositiveCount>,

    // === Thinking / Reasoning ===
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    pub default_thinking_level: Option<ReasoningEffort>,

    // === Context Management ===
    pub auto_compact_pct: Option<i32>,

    // === Tools ===
    /// Optional per-model shape for the dedicated `apply_patch` tool
    /// (`None` → `Freeform`). Read by `apply_patch.rs::tool_spec` via
    /// `SchemaContext`. Enablement is separate — driven by
    /// `with_extra(ApplyPatch)` on `tool_overrides`.
    pub apply_patch_tool_type: Option<ApplyPatchToolType>,
    /// Per-model tool-availability adjustments. Layered on top of the
    /// built-in registry. See `docs/coco-rs/feature-gates-and-tool-filtering.md`.
    pub tool_overrides: Option<ToolOverrides>,
    pub shell_tool_type: ModelShellToolType,
    pub max_tool_output_chars: Option<i32>,

    // === Instructions ===
    pub base_instructions: Option<String>,
    pub base_instructions_file: Option<String>,

    /// Layer 1 escape hatch. Provider-agnostic flat keys, **camelCase**
    /// to match each provider's typed-options struct (Layer 3 reads
    /// `#[serde(rename_all = "camelCase")]`). Layer 2 wraps as
    /// `provider_options[<provider_name>]` at call time. snake_case keys
    /// silently fall through Layer 3's typed parser to leftover-merge.
    pub extra_body: BTreeMap<String, serde_json::Value>,
}

impl Default for ModelInfo {
    /// Sentinel placeholder for tests and in-process construction. The
    /// `context_window` / `max_output_tokens` values are arbitrary —
    /// they bypass the JSON-boundary validation enforced by
    /// [`ModelInfo::from_partial`].
    ///
    /// **Production paths must not use `Default::default()`** —
    /// always go through `from_partial(provider, model_id, partial)`
    /// so missing required fields surface as
    /// `ConfigError::IncompleteModelEntry { ContextWindow | MaxOutputTokens }`
    /// rather than silently passing through.
    fn default() -> Self {
        Self {
            model_id: String::new(),
            display_name: None,
            context_window: PositiveTokens::new(200_000),
            max_output_tokens: PositiveTokens::new(16_384),
            max_output_tokens_escalate: None,
            timeout_secs: None,
            capabilities: None,
            cache_break_detection_excluded: false,
            temperature: None,
            top_p: None,
            top_k: None,
            supported_thinking_levels: None,
            default_thinking_level: None,
            auto_compact_pct: None,
            apply_patch_tool_type: None,
            tool_overrides: None,
            shell_tool_type: ModelShellToolType::ShellCommand,
            max_tool_output_chars: None,
            base_instructions: None,
            base_instructions_file: None,
            extra_body: BTreeMap::new(),
        }
    }
}

impl ModelInfo {
    /// Resolve a `PartialModelInfo` into a complete `ModelInfo`. The
    /// only public path from JSON; surfaces a typed error when a
    /// required field never appeared anywhere in the merge chain.
    ///
    /// Cross-field invariants enforced here:
    /// - `default_thinking_level` (when set) must match the `.effort`
    ///   of some entry in `supported_thinking_levels` (when set).
    ///   The default exists to identify the picker entry to highlight,
    ///   so an unmatched default would silently degrade `default_thinking()`
    ///   to `None`.
    pub fn from_partial(
        provider: &str,
        model_id: &str,
        partial: PartialModelInfo,
    ) -> Result<Self, ConfigError> {
        if let (Some(default), Some(levels)) = (
            partial.default_thinking_level,
            partial.supported_thinking_levels.as_ref(),
        ) && !levels.iter().any(|l| l.effort == default)
        {
            return Err(ConfigError::DefaultThinkingLevelNotSupported {
                provider: provider.to_string(),
                model: model_id.to_string(),
                default,
                supported: levels.iter().map(|l| l.effort).collect(),
            });
        }

        // M3 — Phase-1 escalate ceiling MUST exceed the baseline; an
        // escalate at-or-below baseline is a silent no-op at the
        // recovery dispatcher and almost certainly a config mistake.
        // Reject at resolution time so the failure surfaces during
        // boot, not on the first MaxTokens stop.
        if let (Some(baseline), Some(escalate)) = (
            partial.max_output_tokens,
            partial.max_output_tokens_escalate,
        ) && i64::from(escalate) <= i64::from(baseline)
        {
            return Err(ConfigError::EscalateBelowBaseline {
                provider: provider.to_string(),
                model: model_id.to_string(),
                baseline: i64::from(baseline),
                escalate: i64::from(escalate),
            });
        }

        Ok(Self {
            model_id: model_id.to_string(),
            display_name: partial.display_name,
            context_window: partial.context_window.ok_or_else(|| {
                ConfigError::IncompleteModelEntry {
                    provider: provider.to_string(),
                    model: model_id.to_string(),
                    field: ConfigField::ContextWindow,
                }
            })?,
            max_output_tokens: partial.max_output_tokens.ok_or_else(|| {
                ConfigError::IncompleteModelEntry {
                    provider: provider.to_string(),
                    model: model_id.to_string(),
                    field: ConfigField::MaxOutputTokens,
                }
            })?,
            max_output_tokens_escalate: partial.max_output_tokens_escalate,
            timeout_secs: partial.timeout_secs,
            capabilities: partial.capabilities,
            cache_break_detection_excluded: partial.cache_break_detection_excluded.unwrap_or(false),
            temperature: partial.temperature,
            top_p: partial.top_p,
            top_k: partial.top_k,
            supported_thinking_levels: partial.supported_thinking_levels,
            default_thinking_level: partial.default_thinking_level,
            auto_compact_pct: partial.auto_compact_pct,
            apply_patch_tool_type: partial.apply_patch_tool_type,
            tool_overrides: partial.tool_overrides,
            shell_tool_type: partial.shell_tool_type.unwrap_or_default(),
            max_tool_output_chars: partial.max_tool_output_chars,
            base_instructions: partial.base_instructions,
            base_instructions_file: partial.base_instructions_file,
            extra_body: partial.extra_body.unwrap_or_default(),
        })
    }

    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities
            .as_ref()
            .is_some_and(|caps| caps.contains(&cap))
    }

    /// Get default ThinkingLevel by looking up default effort in supported levels.
    pub fn default_thinking(&self) -> Option<&ThinkingLevel> {
        let effort = self.default_thinking_level?;
        self.supported_thinking_levels
            .as_ref()?
            .iter()
            .find(|l| l.effort == effort)
    }

    /// Resolve a requested effort to the best matching supported ThinkingLevel.
    ///
    /// Resolution semantics:
    /// - `Some(non-empty)` — exact-effort match wins; otherwise fall
    ///   back to the closest declared level by effort distance.
    /// - `None` (field absent) — pass `requested` through unchanged;
    ///   the model has not declared its thinking surface, so trust
    ///   the caller.
    /// - `Some(vec![])` — also passes `requested` through. An
    ///   explicitly-empty list is treated as "no declared surface,"
    ///   identical to `None`. If a future caller needs an explicit
    ///   "thinking unsupported" signal, prefer omitting `Capability::ExtendedThinking`
    ///   from `capabilities` rather than overloading this field.
    pub fn resolve_thinking_level(&self, requested: &ThinkingLevel) -> ThinkingLevel {
        match &self.supported_thinking_levels {
            Some(levels) if !levels.is_empty() => levels
                .iter()
                .find(|l| l.effort == requested.effort)
                .cloned()
                .unwrap_or_else(|| {
                    levels
                        .iter()
                        .min_by_key(|l| (l.effort as i32 - requested.effort as i32).abs())
                        .cloned()
                        .unwrap_or_else(|| requested.clone())
                }),
            _ => requested.clone(),
        }
    }
}

impl PartialEq for ModelInfo {
    fn eq(&self, other: &Self) -> bool {
        self.model_id == other.model_id
    }
}

/// Role -> (primary + fallback chain + fallback policy).
///
/// The JSON-facing side uses `RoleSlots<ProviderModelSelection>` (see
/// `ModelSelectionSettings`); this runtime-facing side stores the
/// already-resolved `RoleSlots<ModelSpec>`, produced by
/// `RuntimeConfigBuilder`.
#[derive(Debug, Clone, Default)]
pub struct ModelRoles {
    pub roles: HashMap<ModelRole, RoleSlots<ModelSpec>>,
}

impl ModelRoles {
    /// Primary model for a role. Falls back to `Main`'s primary if
    /// the role is unset.
    pub fn get(&self, role: ModelRole) -> Option<&ModelSpec> {
        self.roles
            .get(&role)
            .map(|s| &s.primary)
            .or_else(|| self.roles.get(&ModelRole::Main).map(|s| &s.primary))
    }

    /// Ordered fallback chain for a role. Strictly per-role.
    pub fn fallbacks(&self, role: ModelRole) -> &[ModelSpec] {
        self.roles
            .get(&role)
            .map(|s| s.fallbacks.as_slice())
            .unwrap_or(&[])
    }

    /// Fallback policy for a role.
    pub fn policy(&self, role: ModelRole) -> Option<FallbackPolicy> {
        self.roles.get(&role).map(|s| s.policy)
    }

    /// Full `RoleSlots` for a role.
    pub fn role_slots(&self, role: ModelRole) -> Option<&RoleSlots<ModelSpec>> {
        self.roles.get(&role)
    }
}

/// JSON-facing role model selections.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSelectionSettings {
    pub main: Option<RoleSlots<ProviderModelSelection>>,
    pub fast: Option<RoleSlots<ProviderModelSelection>>,
    pub plan: Option<RoleSlots<ProviderModelSelection>>,
    pub explore: Option<RoleSlots<ProviderModelSelection>>,
    pub review: Option<RoleSlots<ProviderModelSelection>>,
    pub hook_agent: Option<RoleSlots<ProviderModelSelection>>,
    pub memory: Option<RoleSlots<ProviderModelSelection>>,
    /// Forked-agent spawn model (TS `tools/AgentTool/`). Generic role
    /// for agent/skill subagent dispatch — distinct from `explore`,
    /// which is the investigative subagent type.
    pub subagent: Option<RoleSlots<ProviderModelSelection>>,
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
