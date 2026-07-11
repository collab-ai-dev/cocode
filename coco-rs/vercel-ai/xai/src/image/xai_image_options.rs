use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for xAI image models.
///
/// Mirrors `xaiImageModelOptions` from `xai-image-model-options.ts`, extracted
/// from `provider_options["xai"]`. Unlike the chat options the upstream schema
/// uses snake_case keys on this surface.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct XaiImageProviderOptions {
    /// Aspect ratio of the generated image (e.g. `"16:9"`). A top-level
    /// `aspect_ratio` call option takes precedence.
    pub aspect_ratio: Option<String>,
    /// Output image format (e.g. `"png"`, `"jpeg"`).
    pub output_format: Option<String>,
    /// Whether to generate synchronously.
    pub sync_mode: Option<bool>,
    /// Resolution tier: `1k` | `2k`.
    pub resolution: Option<XaiImageResolution>,
    /// Quality tier: `low` | `medium` | `high`.
    pub quality: Option<XaiImageQuality>,
    /// End-user identifier passed through to xAI.
    pub user: Option<String>,
}

/// Resolution tier accepted by xAI image generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum XaiImageResolution {
    #[serde(rename = "1k")]
    OneK,
    #[serde(rename = "2k")]
    TwoK,
}

impl XaiImageResolution {
    /// Wire value for the `resolution` request field.
    pub fn as_str(&self) -> &'static str {
        match self {
            XaiImageResolution::OneK => "1k",
            XaiImageResolution::TwoK => "2k",
        }
    }
}

/// Quality tier accepted by xAI image generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XaiImageQuality {
    Low,
    Medium,
    High,
}

impl XaiImageQuality {
    /// Wire value for the `quality` request field.
    pub fn as_str(&self) -> &'static str {
        match self {
            XaiImageQuality::Low => "low",
            XaiImageQuality::Medium => "medium",
            XaiImageQuality::High => "high",
        }
    }
}

/// Extract xAI image options from the generic provider-options map (the
/// `"xai"` namespace). Returns defaults when absent or malformed.
pub fn extract_xai_image_options(
    provider_options: &Option<ProviderOptions>,
) -> XaiImageProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("xai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<XaiImageProviderOptions>(v).ok())
        .unwrap_or_default()
}
