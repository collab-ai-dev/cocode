//! Google Imagen / Gemini image model provider options.
//!
//! These options are passed via `providerOptions.google` and forwarded
//! to the Imagen `:predict` API.

use serde::Deserialize;
use serde::Serialize;

/// Person-generation safety control (Imagen models only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonGeneration {
    DontAllow,
    AllowAdult,
    AllowAll,
}

/// Aspect ratio for generated images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectRatio {
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "3:4")]
    Portrait3x4,
    #[serde(rename = "4:3")]
    Landscape4x3,
    #[serde(rename = "9:16")]
    Portrait9x16,
    #[serde(rename = "16:9")]
    Landscape16x9,
}

/// Provider options for Google image models.
///
/// Currently only applies to Imagen — Gemini multimodal image-output models
/// ignore these fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerativeAIImageOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_generation: Option<PersonGeneration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
}

#[cfg(test)]
#[path = "google_generative_ai_image_options.test.rs"]
mod tests;
