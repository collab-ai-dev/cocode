use serde::Deserialize;

/// Response from an OpenAI-compatible Images API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleImageResponse {
    pub data: Vec<OpenAICompatibleImageData>,
    pub created: Option<u64>,
    pub usage: Option<OpenAICompatibleImageUsage>,
}

/// A single image in the response.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleImageData {
    pub b64_json: Option<String>,
    pub url: Option<String>,
    pub revised_prompt: Option<String>,
}

/// Usage info from the Images API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleImageUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[cfg(test)]
#[path = "openai_compatible_image_api.test.rs"]
mod tests;
