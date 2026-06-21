use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use vercel_ai_provider_utils::ExtractExtras;

/// Provider-specific options for OpenAI-compatible image models.
///
/// Only includes the 4 fields defined in the openai-compatible schema.
/// All other provider-specific keys flow through `extra` (captured by
/// `#[serde(flatten)]`) and are deep-merged into the request body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompatibleImageProviderOptions {
    pub quality: Option<String>,
    pub style: Option<String>,
    pub size: Option<String>,
    pub user: Option<String>,

    // Captures every key not consumed by the typed fields above so
    // the image model can deep-merge them onto the wire body.
    // Replaces the hand-maintained `SCHEMA_KEYS` whitelist.
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ExtractExtras for OpenAICompatibleImageProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, Value> {
        std::mem::take(&mut self.extra)
    }
}

/// Extract image-specific options from provider options,
/// with fallback key resolution: `providerOptionsName` → `openaiCompatible`.
///
/// Returns `(typed_options, passthrough_map)` where `passthrough_map`
/// contains only the keys not consumed by typed fields.
pub fn extract_image_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
    provider_name: &str,
) -> (OpenAICompatibleImageProviderOptions, HashMap<String, Value>) {
    let Some(opts) = provider_options.as_ref() else {
        return (
            OpenAICompatibleImageProviderOptions::default(),
            HashMap::new(),
        );
    };

    // Resolve raw value with precedence: providerOptionsName > openaiCompatible
    let raw = opts
        .0
        .get(provider_name)
        .or_else(|| opts.0.get("openaiCompatible"));

    let Some(raw) = raw else {
        return (
            OpenAICompatibleImageProviderOptions::default(),
            HashMap::new(),
        );
    };

    let value = match serde_json::to_value(raw) {
        Ok(v) => v,
        Err(_) => {
            return (
                OpenAICompatibleImageProviderOptions::default(),
                HashMap::new(),
            );
        }
    };

    let mut typed: OpenAICompatibleImageProviderOptions =
        serde_json::from_value(value).unwrap_or_default();
    let passthrough: HashMap<String, Value> = typed.take_extras().into_iter().collect();

    (typed, passthrough)
}
