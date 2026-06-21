use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use vercel_ai_provider_utils::ExtractExtras;

use crate::provider_options_key::get_effective_provider_options;

/// Provider-specific options for OpenAI-compatible completion models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompatibleCompletionProviderOptions {
    pub echo: Option<bool>,
    pub logit_bias: Option<HashMap<String, f64>>,
    pub suffix: Option<String>,
    pub user: Option<String>,

    // Captures every key not consumed by the typed fields above so the
    // language model can deep-merge them onto the wire body. Replaces
    // the hand-maintained `SCHEMA_KEYS` whitelist.
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ExtractExtras for OpenAICompatibleCompletionProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, Value> {
        std::mem::take(&mut self.extra)
    }
}

/// Extract completion-specific options and passthrough keys from provider options.
pub fn extract_completion_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
    provider_name: &str,
) -> (
    OpenAICompatibleCompletionProviderOptions,
    HashMap<String, Value>,
) {
    let raw = get_effective_provider_options(provider_name, provider_options.as_ref());

    let mut typed = raw
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAICompatibleCompletionProviderOptions>(v).ok())
        .unwrap_or_default();
    let passthrough: HashMap<String, Value> = typed.take_extras().into_iter().collect();

    (typed, passthrough)
}
