//! Prepare call settings from prompt options.
//!
//! This module provides utilities for preparing language model call settings
//! from prompt and generation options.

use vercel_ai_provider::LanguageModelV4CallOptions;

use crate::prompt::CallSettings;

/// Prepare call settings for a language model request.
///
/// This function applies settings from `CallSettings` to the call options.
///
/// # Arguments
///
/// * `call_options` - The base call options to modify.
/// * `settings` - The call settings to apply.
///
/// # Returns
///
/// Modified call options with settings applied.
pub fn prepare_call_settings(
    mut call_options: LanguageModelV4CallOptions,
    settings: &CallSettings,
) -> LanguageModelV4CallOptions {
    // Apply max tokens
    if let Some(max_tokens) = settings.max_tokens {
        call_options.max_output_tokens = Some(max_tokens);
    }

    // Apply temperature
    if let Some(temp) = settings.temperature {
        call_options.temperature = Some(temp);
    }

    // Apply top_p
    if let Some(top_p) = settings.top_p {
        call_options.top_p = Some(top_p);
    }

    // Apply top_k
    if let Some(top_k) = settings.top_k {
        call_options.top_k = Some(top_k);
    }

    // Apply stop sequences
    if let Some(ref stop) = settings.stop_sequences {
        call_options.stop_sequences = Some(stop.clone());
    }

    // Apply frequency penalty
    if let Some(freq_penalty) = settings.frequency_penalty {
        call_options.frequency_penalty = Some(freq_penalty);
    }

    // Apply presence penalty
    if let Some(pres_penalty) = settings.presence_penalty {
        call_options.presence_penalty = Some(pres_penalty);
    }

    // Apply seed
    if let Some(seed) = settings.seed {
        call_options.seed = Some(seed);
    }

    // Apply reasoning
    if let Some(reasoning) = settings.reasoning {
        call_options.reasoning = Some(reasoning);
    }

    // Apply headers
    if let Some(ref headers) = settings.headers {
        call_options.headers = Some(headers.clone());
    }

    call_options
}

/// Prepare call settings with defaults.
///
/// This function prepares call settings, applying default values for
/// any missing settings.
///
/// # Arguments
///
/// * `call_options` - The base call options to modify.
/// * `settings` - The call settings to apply.
/// * `default_max_tokens` - Optional default max tokens.
/// * `default_temperature` - Optional default temperature.
///
/// # Returns
///
/// Modified call options with settings and defaults applied.
pub fn prepare_call_settings_with_defaults(
    call_options: LanguageModelV4CallOptions,
    settings: &CallSettings,
    default_max_tokens: Option<u32>,
    default_temperature: Option<f32>,
) -> LanguageModelV4CallOptions {
    let mut options = prepare_call_settings(call_options, settings);

    // Apply defaults if not set
    if options.max_output_tokens.is_none() {
        options.max_output_tokens = default_max_tokens.map(|t| t as u64);
    }

    if options.temperature.is_none() && default_temperature.is_some() {
        options.temperature = default_temperature;
    }

    options
}

#[cfg(test)]
#[path = "prepare_call_settings.test.rs"]
mod tests;
