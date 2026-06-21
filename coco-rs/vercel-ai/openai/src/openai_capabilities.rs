/// How system messages should be handled for a given model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemMessageMode {
    /// Use `role: "system"`.
    System,
    /// Use `role: "developer"` (reasoning models).
    Developer,
    /// Strip system messages entirely.
    Remove,
}

/// Capabilities of an OpenAI model, inferred from the model ID.
#[derive(Debug, Clone)]
pub struct OpenAIModelCapabilities {
    pub is_reasoning_model: bool,
    pub system_message_mode: SystemMessageMode,
    pub supports_flex_processing: bool,
    pub supports_priority_processing: bool,
    /// Whether the model supports non-reasoning params (temperature, top_p, etc.)
    /// when reasoning_effort is "none".
    pub supports_non_reasoning_params_with_no_effort: bool,
}

/// Detect model capabilities from the model ID string.
///
/// Ported from `openai-language-model-capabilities.ts`.
pub fn get_capabilities(model_id: &str) -> OpenAIModelCapabilities {
    let is_o1 = model_id.starts_with("o1");
    let is_o3 = model_id.starts_with("o3");
    let is_o4_mini = model_id.starts_with("o4-mini");
    let is_gpt5 = model_id.starts_with("gpt-5") && !model_id.starts_with("gpt-5-chat");

    let is_reasoning_model = is_o1 || is_o3 || is_o4_mini || is_gpt5;

    let system_message_mode = if is_reasoning_model {
        SystemMessageMode::Developer
    } else {
        SystemMessageMode::System
    };

    let supports_flex_processing = is_o3 || is_o4_mini || is_gpt5;

    let supports_priority_processing = model_id.starts_with("gpt-4")
        || (is_gpt5
            && !model_id.starts_with("gpt-5-nano")
            && !model_id.starts_with("gpt-5-chat")
            && !model_id.starts_with("gpt-5.4-nano"))
        || is_o3
        || is_o4_mini;

    let supports_non_reasoning_params_with_no_effort = model_id.starts_with("gpt-5.1")
        || model_id.starts_with("gpt-5.2")
        || model_id.starts_with("gpt-5.3")
        || model_id.starts_with("gpt-5.4");

    OpenAIModelCapabilities {
        is_reasoning_model,
        system_message_mode,
        supports_flex_processing,
        supports_priority_processing,
        supports_non_reasoning_params_with_no_effort,
    }
}

#[cfg(test)]
#[path = "openai_capabilities.test.rs"]
mod tests;
