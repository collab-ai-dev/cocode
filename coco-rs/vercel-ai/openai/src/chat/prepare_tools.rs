use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

/// Result of preparing tools for the Chat Completions API.
pub struct PreparedChatTools {
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// Convert SDK tools + tool_choice to OpenAI Chat API format.
pub fn prepare_chat_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
) -> PreparedChatTools {
    let mut warnings = Vec::new();

    let tools_value = tools.as_ref().and_then(|ts| {
        if ts.is_empty() {
            return None;
        }

        let mut openai_tools = Vec::new();
        for tool in ts {
            match tool {
                LanguageModelV4Tool::Function(ft) => {
                    let mut params =
                        vercel_ai_provider_utils::to_openai_compatible_schema(&ft.input_schema);
                    // Ensure it's an object type
                    if !params.is_object() {
                        params = json!({ "type": "object", "properties": {} });
                    }
                    let mut func = json!({
                        "name": ft.name,
                        "parameters": params,
                    });
                    if let Some(ref desc) = ft.description {
                        func["description"] = Value::String(desc.clone());
                    }
                    if let Some(strict) = ft.strict {
                        func["strict"] = Value::Bool(strict);
                    }
                    openai_tools.push(json!({
                        "type": "function",
                        "function": func,
                    }));
                }
                LanguageModelV4Tool::Provider(_pt) => {
                    warnings.push(Warning::Unsupported {
                        feature: "provider tools in chat API".into(),
                        details: Some(
                            "Provider tools are only supported in the Responses API".into(),
                        ),
                    });
                }
            }
        }

        if openai_tools.is_empty() {
            None
        } else {
            Some(openai_tools)
        }
    });

    let tool_choice_value = tool_choice.as_ref().map(|tc| match tc {
        LanguageModelV4ToolChoice::Auto => json!("auto"),
        LanguageModelV4ToolChoice::None => json!("none"),
        LanguageModelV4ToolChoice::Required => json!("required"),
        LanguageModelV4ToolChoice::Tool { tool_name } => json!({
            "type": "function",
            "function": { "name": tool_name }
        }),
    });

    PreparedChatTools {
        tools: tools_value,
        tool_choice: tool_choice_value,
        warnings,
    }
}

#[cfg(test)]
#[path = "prepare_tools.test.rs"]
mod tests;
