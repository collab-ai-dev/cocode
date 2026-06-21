//! Reasoning-to-provider mapping utilities.
//!
//! Maps the provider-agnostic `ReasoningLevel` to provider-specific effort
//! strings or token budgets.

use std::collections::HashMap;

use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::Warning;

/// Returns `true` when reasoning is set and is not `ProviderDefault`.
///
/// `None` (the Rust `Option`) means the caller did not specify reasoning at all.
/// `ProviderDefault` means the caller wants the model's built-in behavior.
/// Both are treated as "not custom reasoning".
pub fn is_custom_reasoning(reasoning: Option<ReasoningLevel>) -> bool {
    matches!(
        reasoning,
        Some(
            ReasoningLevel::Off
                | ReasoningLevel::Minimal
                | ReasoningLevel::Low
                | ReasoningLevel::Medium
                | ReasoningLevel::High
                | ReasoningLevel::Xhigh
        )
    )
}

/// Maps a `ReasoningLevel` to a provider-specific effort string.
///
/// Pushes a compatibility warning when the mapped string differs from the
/// level name, or an unsupported warning when the level is absent from the map.
pub fn map_reasoning_to_provider_effort(
    reasoning: ReasoningLevel,
    effort_map: &HashMap<ReasoningLevel, &str>,
    warnings: &mut Vec<Warning>,
) -> Option<String> {
    let mapped = effort_map.get(&reasoning).copied();

    match mapped {
        None => {
            warnings.push(Warning::unsupported_with_details(
                "reasoning",
                format!(
                    "reasoning \"{}\" is not supported by this model.",
                    reasoning.as_str()
                ),
            ));
            None
        }
        Some(value) => {
            if value != reasoning.as_str() {
                warnings.push(Warning::compatibility_with_details(
                    "reasoning",
                    format!(
                        "reasoning \"{}\" is not directly supported by this model. mapped to effort \"{value}\".",
                        reasoning.as_str(),
                    ),
                ));
            }
            Some(value.to_string())
        }
    }
}

/// Maps a `ReasoningLevel` to an absolute token budget.
///
/// Multiplies `max_output_tokens` by the percentage for the given level,
/// then clamps the result between `min_reasoning_budget` and `max_reasoning_budget`.
pub fn map_reasoning_to_provider_budget(
    reasoning: ReasoningLevel,
    max_output_tokens: i64,
    max_reasoning_budget: i64,
    min_reasoning_budget: Option<i64>,
    budget_percentages: Option<&HashMap<ReasoningLevel, f64>>,
    warnings: &mut Vec<Warning>,
) -> Option<i64> {
    let defaults = HashMap::from([
        (ReasoningLevel::Minimal, 0.02),
        (ReasoningLevel::Low, 0.1),
        (ReasoningLevel::Medium, 0.3),
        (ReasoningLevel::High, 0.6),
        (ReasoningLevel::Xhigh, 0.9),
    ]);
    let percentages = budget_percentages.unwrap_or(&defaults);
    let min_budget = min_reasoning_budget.unwrap_or(1024);

    let pct = percentages.get(&reasoning).copied();

    match pct {
        None => {
            warnings.push(Warning::unsupported_with_details(
                "reasoning",
                format!(
                    "reasoning \"{}\" is not supported by this model.",
                    reasoning.as_str()
                ),
            ));
            None
        }
        Some(pct) => {
            let raw = (max_output_tokens as f64 * pct).round() as i64;
            Some(raw.max(min_budget).min(max_reasoning_budget))
        }
    }
}

#[cfg(test)]
#[path = "map_reasoning_to_provider.test.rs"]
mod tests;
