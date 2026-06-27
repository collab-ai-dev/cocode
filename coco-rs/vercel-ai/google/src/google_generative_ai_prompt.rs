//! Google Generative AI prompt types.
//!
//! Internal types representing the Google API request format.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// The complete prompt sent to Google's API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerativeAIPrompt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GoogleGenerativeAISystemInstruction>,
    pub contents: Vec<GoogleGenerativeAIContent>,
}

/// System instruction for the prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleGenerativeAISystemInstruction {
    pub parts: Vec<GoogleTextPart>,
}

/// A text part in the system instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTextPart {
    pub text: String,
}

/// Content role in conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoogleContentRole {
    User,
    Model,
}

/// A content entry in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleGenerativeAIContent {
    pub role: GoogleContentRole,
    pub parts: Vec<GoogleGenerativeAIContentPart>,
}

/// Content part variants for Google API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GoogleGenerativeAIContentPart {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought: Option<bool>,
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineDataPart,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought: Option<bool>,
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: FunctionCallPart,
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: FunctionResponsePart,
    },
    FileData {
        #[serde(rename = "fileData")]
        file_data: FileDataPart,
    },
}

/// Inline data (base64-encoded binary data).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineDataPart {
    pub mime_type: String,
    pub data: String,
}

/// Function call part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallPart {
    pub name: String,
    pub args: Value,
}

/// Function response part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionResponsePart {
    pub name: String,
    pub response: Value,
    /// Multimodal inline data parts (Gemini 3+).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<InlineDataPart>>,
}

/// File data reference (URL-based).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDataPart {
    pub mime_type: String,
    pub file_uri: String,
}

/// Provider metadata from Google responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerativeAIProviderMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_feedback: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding_metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_context_metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_ratings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_message: Option<String>,
}
