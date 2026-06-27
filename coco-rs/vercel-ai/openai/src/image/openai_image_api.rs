use serde::Deserialize;

/// Response from the OpenAI Images API.
#[derive(Debug, Deserialize)]
pub struct OpenAIImageResponse {
    pub data: Vec<OpenAIImageData>,
    pub created: Option<u64>,
    pub usage: Option<OpenAIImageUsage>,
    /// Background setting (gpt-image-1).
    pub background: Option<String>,
    /// Output format used for generation.
    pub output_format: Option<String>,
    /// Size of the generated images.
    pub size: Option<String>,
    /// Quality level used for generation.
    pub quality: Option<String>,
}

/// A single image in the response.
#[derive(Debug, Deserialize)]
pub struct OpenAIImageData {
    pub b64_json: Option<String>,
    pub url: Option<String>,
    pub revised_prompt: Option<String>,
}

/// Usage info from the Images API.
#[derive(Debug, Deserialize)]
pub struct OpenAIImageUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub input_tokens_details: Option<OpenAIImageTokenDetails>,
}

/// Detailed token breakdown for image generation.
#[derive(Debug, Deserialize)]
pub struct OpenAIImageTokenDetails {
    pub image_tokens: Option<u64>,
    pub text_tokens: Option<u64>,
}

#[cfg(test)]
#[path = "openai_image_api.test.rs"]
mod tests;
