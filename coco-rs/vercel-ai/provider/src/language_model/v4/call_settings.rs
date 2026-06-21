//! Language model call settings (V4).
//!
//! Settings that can be serialized and converted to provider-specific settings.

use super::call_options::LanguageModelV4CallOptions;
use serde::Deserialize;
use serde::Serialize;

/// Settings for language model calls that can be converted to provider-specific settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanguageModelV4CallSettings {
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Temperature for sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top-p for nucleus sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k for sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// Frequency penalty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Seed for deterministic sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

impl From<LanguageModelV4CallOptions> for LanguageModelV4CallSettings {
    fn from(options: LanguageModelV4CallOptions) -> Self {
        Self {
            max_output_tokens: options.max_output_tokens,
            temperature: options.temperature,
            top_p: options.top_p,
            top_k: options.top_k,
            stop_sequences: options.stop_sequences,
            frequency_penalty: options.frequency_penalty,
            presence_penalty: options.presence_penalty,
            seed: options.seed,
        }
    }
}

#[cfg(test)]
#[path = "call_settings.test.rs"]
mod tests;
