use std::collections::HashMap;

use serde::Deserialize;

/// Provider-specific options for OpenAI image models.
///
/// Mirrors the TS upstream split (#14863):
/// - shared base fields (this struct): apply to both `/images/generations`
///   and `/images/edits`.
/// - generation-only fields: live in [`OpenAIImageGenerationOptions`].
/// - edit-only fields: live in [`OpenAIImageEditOptions`].
///
/// Unknown keys still flow through via `extra` so callers stay forward-
/// compatible with new OpenAI fields between SDK updates.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIImageProviderOptions {
    /// Output quality (`standard` / `hd` / `low` / `medium` / `high` / `auto`).
    pub quality: Option<String>,
    /// Background mode (`transparent` / `opaque` / `auto`). Image-1 family.
    pub background: Option<String>,
    /// Output format (`png` / `jpeg` / `webp`). Image-1 family.
    pub output_format: Option<String>,
    /// Output compression (0-100, JPEG/WebP). Image-1 family.
    pub output_compression: Option<u32>,
    /// Style (`vivid` / `natural`). DALL-E 3 only.
    pub style: Option<String>,
    /// Size override (typed for back-compat — TS spec hoists this to
    /// `ImageModelV4CallOptions.size`, kept here as escape hatch).
    pub size: Option<String>,
    /// User identifier for OpenAI usage tracking.
    pub user: Option<String>,
    /// All other unknown provider options, forwarded to the API body.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// `/images/generations`-only options. Adds `moderation` on top of the
/// shared base.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIImageGenerationOptions {
    /// Moderation strictness (`low` / `auto`). Image-1 family.
    pub moderation: Option<String>,
    /// Inherited base fields plus catch-all `extra`.
    #[serde(flatten)]
    pub base: OpenAIImageProviderOptions,
}

/// `/images/edits`-only options. Adds `inputFidelity` on top of the
/// shared base.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIImageEditOptions {
    /// Input-fidelity mode (`low` / `high`). Image-1 edit endpoint.
    pub input_fidelity: Option<String>,
    /// Inherited base fields plus catch-all `extra`.
    #[serde(flatten)]
    pub base: OpenAIImageProviderOptions,
}

/// Extract the raw OpenAI provider options as a `serde_json::Value::Object`.
///
/// This returns the entire `providerOptions.openai` value so callers can
/// spread it into the request body, matching the TS `...providerOptions.openai`
/// pattern.
pub fn extract_raw_image_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
}

/// Extract image-specific options from provider options.
pub fn extract_image_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAIImageProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAIImageProviderOptions>(v).ok())
        .unwrap_or_default()
}

/// Extract `/images/generations`-specific options (base + `moderation`).
pub fn extract_image_generation_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAIImageGenerationOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAIImageGenerationOptions>(v).ok())
        .unwrap_or_default()
}

/// Extract `/images/edits`-specific options (base + `inputFidelity`).
pub fn extract_image_edit_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAIImageEditOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAIImageEditOptions>(v).ok())
        .unwrap_or_default()
}

/// Get the maximum number of images per call for a given model.
///
/// Matches TS `openai-image-options.ts`: explicit lookup for known models,
/// default `1` for everything else (via `?? 1`).
pub fn model_max_images_per_call(model_id: &str) -> usize {
    match model_id {
        "dall-e-2"
        | "gpt-image-1"
        | "gpt-image-1-mini"
        | "gpt-image-1.5"
        | "chatgpt-image-latest" => 10,
        _ => 1,
    }
}

#[cfg(test)]
#[path = "openai_image_options.test.rs"]
mod tests;

/// Check if a model has a default response format (doesn't need explicit `b64_json`).
///
/// Models matching these prefixes return base64-encoded images by default.
pub fn has_default_response_format(model_id: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "chatgpt-image-",
        "gpt-image-1-mini",
        "gpt-image-1.5",
        "gpt-image-1",
    ];
    PREFIXES.iter().any(|prefix| model_id.starts_with(prefix))
}
