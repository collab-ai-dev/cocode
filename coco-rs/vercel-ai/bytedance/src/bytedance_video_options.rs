//! ByteDance video provider options.

use serde::Deserialize;
use std::collections::BTreeMap;
use vercel_ai_provider_utils::ExtractExtras;

/// Provider-specific options for ByteDance video generation.
///
/// Deserialized from the `"bytedance"` namespace in `provider_options`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ByteDanceVideoProviderOptions {
    /// Whether to add a watermark to the generated video.
    pub watermark: Option<bool>,
    /// Whether to generate audio along with the video.
    pub generate_audio: Option<bool>,
    /// Whether the camera should remain fixed during generation.
    pub camera_fixed: Option<bool>,
    /// Whether to return the last frame of the video.
    pub return_last_frame: Option<bool>,
    /// Service tier: `"default"` or `"flex"`.
    pub service_tier: Option<String>,
    /// Whether to use draft mode (faster, lower quality).
    pub draft: Option<bool>,
    /// Base64-encoded last frame image for continuation.
    pub last_frame_image: Option<String>,
    /// Base64-encoded reference images for style guidance.
    pub reference_images: Option<Vec<String>>,
    /// Override poll interval in milliseconds.
    pub poll_interval_ms: Option<u64>,
    /// Override poll timeout in milliseconds.
    pub poll_timeout_ms: Option<u64>,

    // Captures every key not consumed by the typed fields above so
    // the video model can deep-merge them onto the wire body.
    // Replaces the hand-maintained `HANDLED_PROVIDER_OPTIONS`
    // blacklist with the idiomatic serde escape hatch — typed-consumed
    // names auto-strip without an explicit list.
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ExtractExtras for ByteDanceVideoProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, serde_json::Value> {
        std::mem::take(&mut self.extra)
    }
}
