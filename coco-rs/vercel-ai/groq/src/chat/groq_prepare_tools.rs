use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

use crate::groq_browser_search_models::is_browser_search_supported_model;
use crate::groq_browser_search_models::supported_models_string;
use crate::tool::BROWSER_SEARCH_TOOL_ID;

/// Result of preparing tools for the Groq Chat Completions API.
pub struct PreparedGroqTools {
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// Convert SDK tools + tool_choice into Groq's wire format.
///
/// Mirrors `groq-prepare-tools.ts`. The `groq.browser_search` provider tool
/// is honored on supported models; other provider tools produce a warning.
pub fn prepare_groq_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
    model_id: &str,
) -> PreparedGroqTools {
    let mut warnings = Vec::new();

    // Empty tools array behaves like no tools.
    let non_empty = tools.as_ref().filter(|t| !t.is_empty());
    let Some(tools) = non_empty else {
        return PreparedGroqTools {
            tools: None,
            tool_choice: None,
            warnings,
        };
    };

    let mut groq_tools: Vec<Value> = Vec::new();
    for tool in tools {
        match tool {
            LanguageModelV4Tool::Provider(pt) => {
                if pt.id == BROWSER_SEARCH_TOOL_ID {
                    if is_browser_search_supported_model(model_id) {
                        groq_tools.push(json!({ "type": "browser_search" }));
                    } else {
                        warnings.push(Warning::unsupported_with_details(
                            format!("provider-defined tool {}", pt.id),
                            format!(
                                "Browser search is only supported on the following models: {}. Current model: {model_id}",
                                supported_models_string()
                            ),
                        ));
                    }
                } else {
                    warnings.push(Warning::unsupported(format!(
                        "provider-defined tool {}",
                        pt.id
                    )));
                }
            }
            LanguageModelV4Tool::Function(ft) => {
                let mut params =
                    vercel_ai_provider_utils::to_openai_compatible_schema(&ft.input_schema);
                if !params.is_object() {
                    params = json!({ "type": "object", "properties": {} });
                }
                let mut func = serde_json::Map::new();
                func.insert("name".into(), Value::String(ft.name.clone()));
                if let Some(ref desc) = ft.description {
                    func.insert("description".into(), Value::String(desc.clone()));
                }
                func.insert("parameters".into(), params);
                if let Some(strict) = ft.strict {
                    func.insert("strict".into(), Value::Bool(strict));
                }
                groq_tools.push(json!({ "type": "function", "function": func }));
            }
        }
    }

    // If every requested tool was filtered out, emit neither `tools` nor
    // `tool_choice` — sending `"tools": []` (or a choice with no tools) is
    // noise the API can misinterpret.
    if groq_tools.is_empty() {
        return PreparedGroqTools {
            tools: None,
            tool_choice: None,
            warnings,
        };
    }

    let tool_choice_value = tool_choice.as_ref().map(|tc| match tc {
        LanguageModelV4ToolChoice::Auto => json!("auto"),
        LanguageModelV4ToolChoice::None => json!("none"),
        LanguageModelV4ToolChoice::Required => json!("required"),
        LanguageModelV4ToolChoice::Tool { tool_name } => {
            json!({ "type": "function", "function": { "name": tool_name } })
        }
    });

    PreparedGroqTools {
        tools: Some(groq_tools),
        tool_choice: tool_choice_value,
        warnings,
    }
}

#[cfg(test)]
#[path = "groq_prepare_tools.test.rs"]
mod tests;
