use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Semantics of OpenAI-compatible `prompt_tokens` when cache-read tokens are
/// also reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTokensTotalSemantics {
    /// `prompt_tokens` already includes `prompt_tokens_details.cached_tokens`.
    #[default]
    Inclusive,
    /// `prompt_tokens` excludes `prompt_tokens_details.cached_tokens`.
    NonInclusive,
}

/// Provider-instance options parsed by the OpenAI-compatible adapter.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct OpenAICompatibleProviderOptionsConfig {
    pub prompt_tokens_total_semantics: PromptTokensTotalSemantics,
}

#[derive(Debug)]
pub struct ProviderOptionsError {
    source: serde_json::Error,
}

impl fmt::Display for ProviderOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid OpenAI-compatible provider options: {}",
            self.source
        )
    }
}

impl std::error::Error for ProviderOptionsError {}

pub fn parse_provider_options(
    options: &BTreeMap<String, Value>,
) -> Result<OpenAICompatibleProviderOptionsConfig, ProviderOptionsError> {
    let value = Value::Object(options.clone().into_iter().collect());
    serde_json::from_value(value).map_err(|source| ProviderOptionsError { source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_inclusive_prompt_tokens() {
        let parsed = parse_provider_options(&BTreeMap::new()).expect("parse defaults");
        assert_eq!(
            parsed.prompt_tokens_total_semantics,
            PromptTokensTotalSemantics::Inclusive
        );
    }

    #[test]
    fn parses_non_inclusive_prompt_tokens() {
        let options = BTreeMap::from([(
            "prompt_tokens_total_semantics".to_string(),
            serde_json::json!("non_inclusive"),
        )]);
        let parsed = parse_provider_options(&options).expect("parse non-inclusive");
        assert_eq!(
            parsed.prompt_tokens_total_semantics,
            PromptTokensTotalSemantics::NonInclusive
        );
    }

    #[test]
    fn rejects_unknown_provider_options() {
        let options = BTreeMap::from([("typo".to_string(), serde_json::json!(true))]);
        assert!(parse_provider_options(&options).is_err());
    }
}
