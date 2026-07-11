use std::collections::BTreeMap;

use serde::Deserialize;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ProviderOptions;

/// Operation mode for xAI video calls. Mirrors the `mode` provider option
/// from `xai-video-model-options.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum XaiVideoMode {
    /// Video editing (`POST /videos/edits`), requires `videoUrl`.
    EditVideo,
    /// Video extension (`POST /videos/extensions`), requires `videoUrl`.
    ExtendVideo,
    /// Reference-to-video generation (`POST /videos/generations`), requires
    /// `referenceImageUrls`.
    ReferenceToVideo,
}

/// Resolution tier accepted by xAI video generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum XaiVideoResolution {
    #[serde(rename = "480p")]
    P480,
    #[serde(rename = "720p")]
    P720,
}

impl XaiVideoResolution {
    /// Wire value for the `resolution` request field.
    pub fn as_str(&self) -> &'static str {
        match self {
            XaiVideoResolution::P480 => "480p",
            XaiVideoResolution::P720 => "720p",
        }
    }
}

/// Provider-specific options for xAI video models (namespace `"xai"`).
///
/// Mirrors the runtime schema in `xai-video-model-options.ts`: an explicit
/// `mode` selects the operation, and the legacy auto-detect shapes (bare
/// `videoUrl` → edit, bare `referenceImageUrls` → reference-to-video) remain
/// supported. The upstream schema is a loose object — unknown keys pass
/// through to the request body — captured here via `extra`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct XaiVideoProviderOptions {
    /// Operation mode (edit / extend / reference-to-video).
    pub mode: Option<XaiVideoMode>,
    /// Source video URL for edit / extend modes.
    pub video_url: Option<String>,
    /// Reference image URLs (1-7) for reference-to-video generation.
    pub reference_image_urls: Option<Vec<String>>,
    /// Polling interval override in milliseconds (default 5000).
    pub poll_interval_ms: Option<u64>,
    /// Polling timeout override in milliseconds (default 600000).
    pub poll_timeout_ms: Option<u64>,
    /// Resolution tier (`480p` | `720p`).
    pub resolution: Option<XaiVideoResolution>,

    /// Unknown keys pass through to the request body verbatim (the upstream
    /// schema is a loose object). Typed-consumed keys never appear here.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Extract xAI video options from the generic provider-options map (the
/// `"xai"` namespace). Returns defaults when absent; malformed values (e.g. a
/// non-string `videoUrl`) surface as an error rather than being silently
/// dropped, since they change which endpoint is called.
pub fn extract_xai_video_options(
    provider_options: &Option<ProviderOptions>,
) -> Result<XaiVideoProviderOptions, AISdkError> {
    let Some(ns) = provider_options.as_ref().and_then(|opts| opts.0.get("xai")) else {
        return Ok(XaiVideoProviderOptions::default());
    };
    let value = serde_json::to_value(ns)
        .map_err(|e| AISdkError::new(format!("Invalid xAI video provider options: {e}")))?;
    serde_json::from_value::<XaiVideoProviderOptions>(value)
        .map_err(|e| AISdkError::new(format!("Invalid xAI video provider options: {e}")))
}

/// Resolve the effective operation mode and validate mode-specific
/// requirements. Mirrors `resolveVideoMode` plus the schema constraints
/// (non-empty `videoUrl`, 1-7 `referenceImageUrls`).
pub fn resolve_video_mode(
    options: &XaiVideoProviderOptions,
) -> Result<Option<XaiVideoMode>, AISdkError> {
    if let Some(ref urls) = options.reference_image_urls
        && (urls.is_empty() || urls.len() > 7 || urls.iter().any(String::is_empty))
    {
        return Err(AISdkError::new(
            "providerOptions.xai.referenceImageUrls must contain 1-7 non-empty URLs",
        ));
    }
    if let Some(ref url) = options.video_url
        && url.is_empty()
    {
        return Err(AISdkError::new(
            "providerOptions.xai.videoUrl must be a non-empty URL",
        ));
    }

    let mode = match options.mode {
        Some(mode) => Some(mode),
        None if options.video_url.is_some() => Some(XaiVideoMode::EditVideo),
        None if options.reference_image_urls.is_some() => Some(XaiVideoMode::ReferenceToVideo),
        None => None,
    };

    match mode {
        Some(XaiVideoMode::EditVideo | XaiVideoMode::ExtendVideo)
            if options.video_url.is_none() =>
        {
            Err(AISdkError::new(
                "providerOptions.xai.videoUrl is required for edit-video and extend-video modes",
            ))
        }
        Some(XaiVideoMode::ReferenceToVideo) if options.reference_image_urls.is_none() => {
            Err(AISdkError::new(
                "providerOptions.xai.referenceImageUrls is required for reference-to-video mode",
            ))
        }
        other => Ok(other),
    }
}

#[cfg(test)]
#[path = "xai_video_options.test.rs"]
mod tests;
