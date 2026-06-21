//! Google Generative AI image model settings.

use serde::Deserialize;
use serde::Serialize;

/// Settings for the Google image generation model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerativeAIImageSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_images_per_call: Option<usize>,
}
