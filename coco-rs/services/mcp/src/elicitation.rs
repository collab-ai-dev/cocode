//! URL/form elicitation handling.
//!
//! Elicitation allows MCP servers to collect user input during tool execution
//! via forms or URL-based flows. The server sends an `ElicitationRequest` and
//! the client returns an `ElicitationResult` with the collected values.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

/// High-level elicitation type: form-based or URL-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationType {
    Form,
    Url,
}

/// A field type within an elicitation form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElicitationFieldType {
    Text,
    Number,
    Boolean,
    Select { options: Vec<String> },
}

/// A single field in an elicitation form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationField {
    pub name: String,
    pub field_type: ElicitationFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
}

/// An elicitation request from an MCP server.
///
/// Carries either form fields or a URL the user should visit to complete an
/// action (e.g. OAuth consent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    pub server_name: String,
    pub request_id: String,
    pub elicitation_type: ElicitationType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: Vec<ElicitationField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Legacy free-form message (prefer `description`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The mode of elicitation (wire-format variant).
///
/// Kept for backward compatibility with existing serialized messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElicitationMode {
    Url { url: String },
    Form { schema: serde_json::Value },
}

/// Result of a completed elicitation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResult {
    pub approved: bool,
    #[serde(default)]
    pub values: HashMap<String, serde_json::Value>,
}

/// Elicitation outcome with status discrimination (wire-format variant).
///
/// Kept for backward compatibility with existing serialized messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ElicitResult {
    Completed { data: serde_json::Value },
    Cancelled,
    Timeout,
}

#[cfg(test)]
#[path = "elicitation.test.rs"]
mod tests;
