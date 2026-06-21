use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use vercel_ai_provider_utils::ExtractExtras;

use crate::provider_options_key::get_effective_provider_options;

/// Provider-specific options for OpenAI-compatible Chat models.
///
/// Only includes the 4 fields defined in the openai-compatible schema.
/// All other provider-specific keys flow through `extra` (captured by
/// `#[serde(flatten)]`) and are deep-merged into the request body by
/// the language model.
///
/// Extracted from `options.provider_options[provider_name]` (with fallbacks).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompatibleChatProviderOptions {
    pub user: Option<String>,
    /// Reasoning effort level as a string (e.g., "low", "medium", "high").
    pub reasoning_effort: Option<String>,
    /// Text verbosity level as a string (e.g., "low", "medium", "high").
    pub text_verbosity: Option<String>,
    /// Defaults to true when response_format is json_schema.
    pub strict_json_schema: Option<bool>,

    // Catches every key not consumed by the typed fields above so the
    // language model can deep-merge them onto the wire body. Replaces
    // the hand-maintained `SCHEMA_KEYS` whitelist with the idiomatic
    // serde escape hatch — adding a typed field automatically removes
    // it from the pass-through map.
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ExtractExtras for OpenAICompatibleChatProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, Value> {
        std::mem::take(&mut self.extra)
    }
}

/// Extract provider-specific options from the generic provider options map,
/// with fallback key resolution: `providerOptionsName` → `openaiCompatible`.
///
/// Returns `(typed_options, passthrough_map)` where `passthrough_map`
/// contains only the keys not consumed by typed fields (captured by
/// `#[serde(flatten)] extra`).
pub fn extract_compatible_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
    provider_name: &str,
) -> (OpenAICompatibleChatProviderOptions, HashMap<String, Value>) {
    let Some(opts) = provider_options.as_ref() else {
        return (
            OpenAICompatibleChatProviderOptions::default(),
            HashMap::new(),
        );
    };

    let raw = get_effective_provider_options(provider_name, Some(opts));

    let Some(raw) = raw else {
        return (
            OpenAICompatibleChatProviderOptions::default(),
            HashMap::new(),
        );
    };

    let value = match serde_json::to_value(raw) {
        Ok(v) => v,
        Err(_) => {
            return (
                OpenAICompatibleChatProviderOptions::default(),
                HashMap::new(),
            );
        }
    };

    let mut typed: OpenAICompatibleChatProviderOptions =
        serde_json::from_value(value).unwrap_or_default();
    let passthrough: HashMap<String, Value> = typed.take_extras().into_iter().collect();

    (typed, passthrough)
}
