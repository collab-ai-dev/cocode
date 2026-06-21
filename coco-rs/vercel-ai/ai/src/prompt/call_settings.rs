//! Call settings for model invocation.
//!
//! This module provides configuration types for controlling how
//! language models are called.

use std::collections::HashMap;

use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ReasoningLevel;

/// Timeout configuration for model calls.
#[derive(Debug, Clone, Default)]
pub struct TimeoutConfiguration {
    /// Total timeout for the entire operation in milliseconds.
    pub total_ms: Option<u64>,
    /// Timeout for each step in multi-step generation in milliseconds.
    pub step_ms: Option<u64>,
    /// Timeout for receiving a chunk during streaming in milliseconds.
    /// Only applicable to streaming operations.
    pub chunk_ms: Option<u64>,
}

impl TimeoutConfiguration {
    /// Create new timeout configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the total timeout in milliseconds.
    pub fn with_total_ms(mut self, ms: u64) -> Self {
        self.total_ms = Some(ms);
        self
    }

    /// Set the step timeout in milliseconds.
    pub fn with_step_ms(mut self, ms: u64) -> Self {
        self.step_ms = Some(ms);
        self
    }

    /// Set the chunk timeout in milliseconds.
    pub fn with_chunk_ms(mut self, ms: u64) -> Self {
        self.chunk_ms = Some(ms);
        self
    }
}

/// Call settings for model invocation.
#[derive(Debug, Clone, Default)]
pub struct CallSettings {
    /// Maximum number of tokens to generate.
    pub max_tokens: Option<u64>,
    /// Temperature for sampling.
    pub temperature: Option<f32>,
    /// Top-p for nucleus sampling.
    pub top_p: Option<f32>,
    /// Top-k for sampling.
    pub top_k: Option<u64>,
    /// Stop sequences.
    pub stop_sequences: Option<Vec<String>>,
    /// Frequency penalty.
    pub frequency_penalty: Option<f32>,
    /// Presence penalty.
    pub presence_penalty: Option<f32>,
    /// Seed for deterministic sampling.
    pub seed: Option<u64>,
    /// Provider-agnostic reasoning effort level.
    pub reasoning: Option<ReasoningLevel>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Maximum number of retries for transient failures.
    pub max_retries: Option<u32>,
    /// Timeout configuration.
    pub timeout: Option<TimeoutConfiguration>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
}

impl CallSettings {
    /// Create new default call settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum tokens.
    pub fn with_max_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set the top-p.
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set the top-k.
    pub fn with_top_k(mut self, top_k: u64) -> Self {
        self.top_k = Some(top_k);
        self
    }

    /// Set the stop sequences.
    pub fn with_stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(stop_sequences);
        self
    }

    /// Set the frequency penalty.
    pub fn with_frequency_penalty(mut self, frequency_penalty: f32) -> Self {
        self.frequency_penalty = Some(frequency_penalty);
        self
    }

    /// Set the presence penalty.
    pub fn with_presence_penalty(mut self, presence_penalty: f32) -> Self {
        self.presence_penalty = Some(presence_penalty);
        self
    }

    /// Set the seed.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set the reasoning level.
    pub fn with_reasoning(mut self, reasoning: ReasoningLevel) -> Self {
        self.reasoning = Some(reasoning);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set the timeout configuration.
    pub fn with_timeout(mut self, timeout: TimeoutConfiguration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, provider_options: ProviderOptions) -> Self {
        self.provider_options = Some(provider_options);
        self
    }
}

#[cfg(test)]
#[path = "call_settings.test.rs"]
mod tests;
