//! Google Generative AI video model settings.

use serde::Deserialize;
use serde::Serialize;

/// Person generation mode for video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonGeneration {
    DontAllow,
    AllowAdult,
    AllowAll,
}

/// A reference image for video generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceImage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_base64_encoded: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gcs_uri: Option<String>,
}

/// Settings for the Google video generation model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerativeAIVideoSettings {
    /// Person generation mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_generation: Option<PersonGeneration>,
    /// Negative prompt to exclude from generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    /// Reference images for guided generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_images: Option<Vec<ReferenceImage>>,
    /// Polling interval in milliseconds (default: 10000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    /// Polling timeout in milliseconds (default: 600000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_timeout_ms: Option<u64>,
}
