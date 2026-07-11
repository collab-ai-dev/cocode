use std::sync::LazyLock;

use regex::Regex;

/// The grok-4.20 reasoning and non-reasoning models (including dated variants
/// such as `grok-4.20-0309-reasoning`) reject the reasoning-effort parameter
/// with an invalid-argument error for every value, including `none`. Other
/// models such as `grok-4.3`, `grok-latest`, and `grok-4.20-multi-agent`
/// accept it.
///
/// Mirrors `supports-reasoning-effort.ts`.
static MODELS_WITHOUT_REASONING_EFFORT: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^grok-4\.20(-\d{4})?-(non-)?reasoning$").ok());

/// Whether the given model accepts the `reasoning_effort` request parameter.
pub fn supports_reasoning_effort(model_id: &str) -> bool {
    match MODELS_WITHOUT_REASONING_EFFORT.as_ref() {
        Some(re) => !re.is_match(model_id),
        // If the pattern failed to compile, assume the parameter is supported.
        None => true,
    }
}

#[cfg(test)]
#[path = "supports_reasoning_effort.test.rs"]
mod tests;
