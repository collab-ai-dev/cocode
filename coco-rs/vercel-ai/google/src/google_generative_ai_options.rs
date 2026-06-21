//! Google Generative AI language model options.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use vercel_ai_provider_utils::ExtractExtras;

/// Response modality for generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResponseModality {
    Text,
    Image,
}

/// Thinking configuration for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
}

/// Thinking level for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

/// Harm category for safety settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmCategory {
    HarmCategoryUnspecified,
    HarmCategoryHateSpeech,
    HarmCategoryDangerousContent,
    HarmCategorySexuallyExplicit,
    HarmCategoryHarassment,
    HarmCategoryCivicIntegrity,
}

/// Harm block threshold for safety settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmBlockThreshold {
    HarmBlockThresholdUnspecified,
    BlockLowAndAbove,
    BlockMediumAndAbove,
    BlockOnlyHigh,
    BlockNone,
    Off,
}

/// Safety setting for content generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetySetting {
    pub category: HarmCategory,
    pub threshold: HarmBlockThreshold,
}

/// Media resolution for content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaResolution {
    MediaResolutionUnspecified,
    MediaResolutionLow,
    MediaResolutionMedium,
    MediaResolutionHigh,
}

/// Aspect ratio for image generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectRatio {
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "2:3")]
    TwoByThree,
    #[serde(rename = "3:2")]
    ThreeByTwo,
    #[serde(rename = "3:4")]
    ThreeByFour,
    #[serde(rename = "4:3")]
    FourByThree,
    #[serde(rename = "4:5")]
    FourByFive,
    #[serde(rename = "5:4")]
    FiveByFour,
    #[serde(rename = "9:16")]
    NineBySixteen,
    #[serde(rename = "16:9")]
    SixteenByNine,
    #[serde(rename = "21:9")]
    TwentyOneByNine,
    #[serde(rename = "1:8")]
    OneByEight,
    #[serde(rename = "8:1")]
    EightByOne,
    #[serde(rename = "1:4")]
    OneByFour,
    #[serde(rename = "4:1")]
    FourByOne,
}

/// Image size for image generation within language model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoogleImageSize {
    #[serde(rename = "512")]
    S512,
    #[serde(rename = "1K")]
    S1K,
    #[serde(rename = "2K")]
    S2K,
    #[serde(rename = "4K")]
    S4K,
}

/// Image configuration for language model image generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_size: Option<GoogleImageSize>,
}

/// Retrieval configuration location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatLng {
    pub latitude: f64,
    pub longitude: f64,
}

/// Dynamic retrieval configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat_lng: Option<LatLng>,
}

/// Google language model options (provider-specific).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleLanguageModelOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modalities: Option<Vec<ResponseModality>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_outputs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_settings: Option<Vec<SafetySetting>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_timestamp: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_resolution: Option<MediaResolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_config: Option<ImageConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_config: Option<RetrievalConfig>,

    // Catches every key not consumed by the typed fields above. The
    // language model's `get_args` deep-merges this into the wire body
    // via `merge_json_value` so users can push extra_body fields
    // (including nested paths) without code changes — and typed-consumed
    // keys like `thinkingConfig` never leak to the body root (Gemini's
    // REST API rejects `thinkingConfig` outside `generationConfig`).
    //
    // The "extras override typed writes at deep-merge final write"
    // doctrine is documented in `services/inference/CLAUDE.md`
    // (Design Notes).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ExtractExtras for GoogleLanguageModelOptions {
    fn take_extras(&mut self) -> BTreeMap<String, serde_json::Value> {
        std::mem::take(&mut self.extra)
    }
}
