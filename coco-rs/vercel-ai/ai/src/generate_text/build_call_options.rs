//! Shared call options builder.
//!
//! This module consolidates the duplicated call options building logic
//! that was in both `generate_text.rs` and `stream_text.rs`.

use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;

use crate::prompt::CallSettings;
use crate::types::ProviderOptions;

use super::output::Output;

/// Apply `CallSettings` fields to existing call options.
///
/// This mutates the provided `call_options` in place, setting max_tokens,
/// temperature, top_p, top_k, stop_sequences, frequency_penalty,
/// presence_penalty, seed, and headers from the settings. Note that
/// `abort_signal` is **not** part of `LanguageModelV4CallOptions` — it
/// flows as a separate parameter to `do_generate` / `do_stream`.
pub fn apply_call_settings(call_options: &mut LanguageModelV4CallOptions, settings: &CallSettings) {
    if let Some(max_tokens) = settings.max_tokens {
        call_options.max_output_tokens = Some(max_tokens);
    }
    if let Some(temp) = settings.temperature {
        call_options.temperature = Some(temp);
    }
    if let Some(top_p) = settings.top_p {
        call_options.top_p = Some(top_p);
    }
    if let Some(top_k) = settings.top_k {
        call_options.top_k = Some(top_k);
    }
    if let Some(ref stop) = settings.stop_sequences {
        call_options.stop_sequences = Some(stop.clone());
    }
    if let Some(freq_penalty) = settings.frequency_penalty {
        call_options.frequency_penalty = Some(freq_penalty);
    }
    if let Some(pres_penalty) = settings.presence_penalty {
        call_options.presence_penalty = Some(pres_penalty);
    }
    if let Some(seed) = settings.seed {
        call_options.seed = Some(seed);
    }
    if let Some(ref headers) = settings.headers {
        call_options.headers = Some(headers.clone());
    }
}

/// Build `LanguageModelV4CallOptions` from the shared set of parameters.
///
/// This function applies all settings fields (max_tokens, temperature, top_p,
/// top_k, stop_sequences, frequency_penalty, presence_penalty, seed, headers),
/// tools, tool_choice, provider_options, and output/response_format.
///
/// `abort_signal` is intentionally NOT part of `LanguageModelV4CallOptions`
/// — it flows as a separate parameter to `do_generate` / `do_stream`. The
/// caller threads it independently.
#[allow(clippy::too_many_arguments)]
pub fn build_call_options(
    settings: &CallSettings,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
    provider_options: &Option<ProviderOptions>,
    output: &Option<Output>,
    messages: LanguageModelV4Prompt,
    tool_definitions: &Option<Vec<LanguageModelV4Tool>>,
) -> LanguageModelV4CallOptions {
    let mut call_options = LanguageModelV4CallOptions::new(messages);

    // Apply all settings.
    apply_call_settings(&mut call_options, settings);

    // Add tools
    if let Some(defs) = tool_definitions {
        call_options.tools = Some(defs.clone());
    }
    if let Some(choice) = tool_choice {
        call_options.tool_choice = Some(choice.clone());
    }

    // Add provider options. Per-step/request options override settings, with
    // nested objects merged to preserve provider defaults.
    let effective_provider_options = merge_provider_options(
        settings.provider_options.as_ref(),
        provider_options.as_ref(),
    );
    if let Some(opts) = effective_provider_options {
        call_options.provider_options = Some(opts);
    }

    // Add response format for structured output
    if let Some(out) = output {
        call_options.response_format = Some(out.to_response_format());
    }

    call_options
}

/// Deeply merge provider options.
///
/// Objects are merged recursively, arrays and primitives are replaced, and
/// prototype-polluting keys are ignored for parity with the TS helper.
pub fn merge_provider_options(
    base: Option<&ProviderOptions>,
    overrides: Option<&ProviderOptions>,
) -> Option<ProviderOptions> {
    match (base, overrides) {
        (None, None) => None,
        (Some(base), None) => Some(base.clone()),
        (None, Some(overrides)) => Some(sanitize_provider_options(overrides)),
        (Some(base), Some(overrides)) => {
            let mut merged = base.clone();
            for (provider, override_options) in &overrides.0 {
                if is_polluting_key(provider) {
                    continue;
                }
                let target = merged.0.entry(provider.clone()).or_default();
                for (key, override_value) in override_options {
                    if is_polluting_key(key) {
                        continue;
                    }
                    match target.get(key) {
                        Some(base_value) => {
                            target
                                .insert(key.clone(), merge_json_value(base_value, override_value));
                        }
                        None => {
                            target.insert(key.clone(), override_value.clone());
                        }
                    }
                }
            }
            Some(merged)
        }
    }
}

fn sanitize_provider_options(options: &ProviderOptions) -> ProviderOptions {
    let mut sanitized = ProviderOptions::new();
    for (provider, provider_options) in &options.0 {
        if is_polluting_key(provider) {
            continue;
        }
        let mut sanitized_provider_options = std::collections::HashMap::new();
        for (key, value) in provider_options {
            if !is_polluting_key(key) {
                sanitized_provider_options.insert(key.clone(), value.clone());
            }
        }
        sanitized.set(provider.clone(), sanitized_provider_options);
    }
    sanitized
}

// Deep-merge / prototype-pollution helpers live in
// `vercel-ai-provider-utils::json` so non-AI crates (e.g.
// `coco-inference`) can reuse them without depending on the AI loop
// crate. Re-exported below for ergonomic access from this module.
pub use vercel_ai_provider_utils::is_prototype_polluting_key as is_polluting_key;
pub use vercel_ai_provider_utils::merge_json_value;

/// Filter tool definitions to only include active tools.
pub fn filter_active_tools(
    tool_definitions: &Option<Vec<LanguageModelV4Tool>>,
    active_tools: &Option<Vec<String>>,
) -> Option<Vec<LanguageModelV4Tool>> {
    match (tool_definitions, active_tools) {
        (Some(defs), Some(active)) => {
            let filtered: Vec<LanguageModelV4Tool> = defs
                .iter()
                .filter(|d| active.iter().any(|name| d.name() == name))
                .cloned()
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some(filtered)
            }
        }
        (Some(defs), None) => Some(defs.clone()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "build_call_options.test.rs"]
mod tests;
