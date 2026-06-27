//! Content part types for prompts.
//!
//! This module defines the various content parts that can appear in messages.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ProviderOptions;

/// Text content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTextPart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The text content.
    pub text: String,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl PromptTextPart {
    /// Create a new text part.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            part_type: "text".to_string(),
            text: text.into(),
            provider_options: None,
        }
    }

    /// Add provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

/// Image content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptImagePart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The image data (base64 string or URL).
    pub image: PromptImageData,
    /// The media type (e.g., "image/png").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// Image data can be a base64 string, URL, or binary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptImageData {
    /// Base64-encoded image data.
    Base64(String),
    /// URL to the image.
    Url(String),
}

impl PromptImagePart {
    /// Create an image part from base64 data.
    pub fn from_base64(data: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self {
            part_type: "image".to_string(),
            image: PromptImageData::Base64(data.into()),
            media_type: Some(media_type.into()),
            provider_options: None,
        }
    }

    /// Create an image part from a URL.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            part_type: "image".to_string(),
            image: PromptImageData::Url(url.into()),
            media_type: None,
            provider_options: None,
        }
    }
}

/// File content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptFilePart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The file data (base64 string or URL).
    pub data: PromptFileData,
    /// The media type (e.g., "application/pdf").
    pub media_type: String,
    /// The filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// File data can be a base64 string, URL, or binary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptFileData {
    /// Base64-encoded file data.
    Base64(String),
    /// URL to the file.
    Url(String),
}

impl PromptFilePart {
    /// Create a file part from base64 data.
    pub fn from_base64(
        data: impl Into<String>,
        media_type: impl Into<String>,
        filename: Option<String>,
    ) -> Self {
        Self {
            part_type: "file".to_string(),
            data: PromptFileData::Base64(data.into()),
            media_type: media_type.into(),
            filename,
            provider_options: None,
        }
    }

    /// Create a file part from a URL.
    pub fn from_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self {
            part_type: "file".to_string(),
            data: PromptFileData::Url(url.into()),
            media_type: media_type.into(),
            filename: None,
            provider_options: None,
        }
    }
}

/// Reasoning content part (for chain-of-thought).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptReasoningPart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The reasoning text.
    pub text: String,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl PromptReasoningPart {
    /// Create a new reasoning part.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            part_type: "reasoning".to_string(),
            text: text.into(),
            provider_options: None,
        }
    }
}

/// Tool call content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptToolCallPart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The tool call ID.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The tool input arguments.
    pub input: serde_json::Value,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl PromptToolCallPart {
    /// Create a new tool call part.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            part_type: "tool-call".to_string(),
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input,
            provider_options: None,
        }
    }
}

/// Tool result content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptToolResultPart {
    /// The type identifier.
    #[serde(rename = "type")]
    pub part_type: String,
    /// The tool call ID this result is for.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The tool result output.
    pub output: PromptToolResultOutput,
    /// Provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl PromptToolResultPart {
    /// Create a new tool result part.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: PromptToolResultOutput,
    ) -> Self {
        Self {
            part_type: "tool-result".to_string(),
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output,
            provider_options: None,
        }
    }
}

/// Tool result output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptToolResultOutput {
    /// Text output.
    Text {
        /// The text value.
        value: String,
    },
    /// JSON output.
    Json {
        /// The JSON value.
        value: serde_json::Value,
    },
    /// Error text output.
    ErrorText {
        /// The error message.
        value: String,
    },
    /// Error JSON output.
    ErrorJson {
        /// The error JSON value.
        value: serde_json::Value,
    },
    /// Execution denied.
    ExecutionDenied {
        /// The reason for denial.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Content output (array of content items).
    Content {
        /// The content items.
        value: Vec<PromptContentItem>,
    },
}

/// Content item for tool result output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptContentItem {
    /// Text content.
    Text {
        /// The text.
        text: String,
    },
    /// Media content (base64).
    Media {
        /// The base64 data.
        data: String,
        /// The media type.
        media_type: String,
    },
    /// Image data content.
    ImageData {
        /// The base64 image data.
        data: String,
        /// The media type.
        media_type: String,
    },
    /// File data content.
    FileData {
        /// The base64 file data.
        data: String,
        /// The media type.
        media_type: String,
        /// The filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

#[cfg(test)]
#[path = "content_part.test.rs"]
mod tests;
