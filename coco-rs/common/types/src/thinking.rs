use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;

/// Unified thinking configuration for all providers.
///
/// `effort` carries provider-agnostic intent — `Disable`, `Auto`, or
/// one of the numeric levels (`Minimal`..`XHigh`). Provider-specific
/// wire toggles (e.g. DeepSeek's `{"thinking":{"type":"enabled"}}`)
/// flow through `options` verbatim.
///
/// Semantic states:
///   * `Disable` — explicit "thinking off"; emit explicit-off signals
///     where the provider supports them, otherwise omit reasoning fields.
///   * `Auto`    — "let the provider decide"; omit reasoning fields
///     so the server-side default applies.
///   * `Minimal`..`XHigh` — explicit numeric efforts; emitted via the
///     provider's typed reasoning channel.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingLevel {
    /// Reasoning effort — universal across all providers.
    pub effort: ReasoningEffort,

    /// Token budget — universal for budget-based providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,

    /// Provider-specific thinking extensions — data-driven passthrough.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub options: HashMap<String, serde_json::Value>,
}

impl ThinkingLevel {
    /// Explicit "thinking off" — convert layer suppresses typed-effort
    /// emission; `options` may carry an explicit-off wire toggle.
    pub fn disable() -> Self {
        Self {
            effort: ReasoningEffort::Off,
            budget_tokens: None,
            options: HashMap::new(),
        }
    }

    /// "Let the provider decide" — convert layer omits all reasoning
    /// fields so the server-side default applies (e.g. DeepSeek defaults
    /// to enabled+high; OpenAI defaults to no reasoning).
    pub fn auto() -> Self {
        Self {
            effort: ReasoningEffort::Auto,
            budget_tokens: None,
            options: HashMap::new(),
        }
    }

    pub fn low() -> Self {
        Self {
            effort: ReasoningEffort::Low,
            ..Self::auto()
        }
    }

    pub fn medium() -> Self {
        Self {
            effort: ReasoningEffort::Medium,
            ..Self::auto()
        }
    }

    pub fn high() -> Self {
        Self {
            effort: ReasoningEffort::High,
            ..Self::auto()
        }
    }

    pub fn xhigh() -> Self {
        Self {
            effort: ReasoningEffort::XHigh,
            ..Self::auto()
        }
    }

    /// Returns `true` for any state where thinking *might* happen on
    /// the wire — i.e. anything other than `Disable`. `Auto` returns
    /// `true` because the user has not opted out, even though the
    /// provider may still resolve it to off-by-default.
    pub fn is_enabled(&self) -> bool {
        self.effort != ReasoningEffort::Off
    }

    pub fn with_budget(effort: ReasoningEffort, budget: i32) -> Self {
        Self {
            effort,
            budget_tokens: Some(budget),
            options: HashMap::new(),
        }
    }
}

impl Default for ThinkingLevel {
    fn default() -> Self {
        Self::auto()
    }
}

/// Parses effort name only (no budget/options).
/// "high" → ThinkingLevel::high(), "auto" → ThinkingLevel::auto()
impl FromStr for ThinkingLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let effort: ReasoningEffort = s.parse()?;
        Ok(Self {
            effort,
            budget_tokens: None,
            options: HashMap::new(),
        })
    }
}

/// Reasoning effort. Ordered from "off" through numeric intensity.
/// Provider-agnostic — `thinking_convert` maps to per-provider wire shapes.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Explicit "thinking off". Convert layer skips typed-effort
    /// emission; `level.options` may still carry an explicit-off
    /// wire toggle (e.g. DeepSeek's `{"thinking":{"type":"disabled"}}`).
    Off,
    /// "Let the provider decide". Convert layer omits reasoning fields;
    /// the server-side default applies. This is the default state when
    /// no thinking level is configured.
    #[default]
    Auto,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningEffort {
    /// Returns `true` only for the explicit numeric efforts that the
    /// convert layer emits via the provider's typed reasoning channel.
    /// `Disable` and `Auto` both return `false` — neither maps to a
    /// concrete level the provider should be told to use.
    pub fn is_explicit_level(self) -> bool {
        matches!(
            self,
            Self::Minimal | Self::Low | Self::Medium | Self::High | Self::XHigh
        )
    }

    /// Canonical short name — matches the serde `snake_case` wire form
    /// and the UI labels rendered in pickers/status bars. `Off` is
    /// `"off"` (not `"disable"`) so users don't confuse it with
    /// `Auto`'s "let the provider default decide".
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "disable" | "disabled" | "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "x_high" | "max" => Ok(Self::XHigh),
            _ => Err(format!("unknown reasoning effort: {s}")),
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "thinking.test.rs"]
mod tests;
