use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Metadata returned in provider_metadata for Anthropic responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessageMetadata {
    /// Raw usage data from the API.
    pub usage: Option<Value>,
    /// Cache creation input tokens (if prompt caching was used).
    pub cache_creation_input_tokens: Option<u64>,
    /// Stop sequence that triggered the stop (if any).
    pub stop_sequence: Option<String>,
    /// Usage breakdown by iteration when compaction is triggered.
    pub iterations: Option<Vec<AnthropicUsageIteration>>,
    /// Container information (for code execution tool).
    pub container: Option<AnthropicResponseContainer>,
    /// Context management response data.
    pub context_management: Option<Value>,
}

/// A single iteration in the usage breakdown (compaction).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsageIteration {
    #[serde(rename = "type")]
    pub iteration_type: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Container metadata from the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicResponseContainer {
    pub id: String,
    pub expires_at: String,
    pub skills: Option<Vec<AnthropicContainerSkill>>,
}

/// A skill loaded in a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicContainerSkill {
    #[serde(rename = "type")]
    pub skill_type: String,
    pub skill_id: String,
    pub version: String,
}
