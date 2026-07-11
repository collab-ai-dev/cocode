use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

use crate::remove_additional_properties::remove_additional_properties_false;

/// Result of preparing tools for the xAI Chat Completions API.
pub struct PreparedXaiTools {
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// Convert SDK tools + tool_choice into xAI's wire format.
///
/// Mirrors `xai-prepare-tools.ts`. Provider-defined tools are unsupported on
/// the Chat Completions surface and produce a warning. Function schemas are
/// sanitized via [`remove_additional_properties_false`].
pub fn prepare_xai_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
) -> PreparedXaiTools {
    let mut warnings = Vec::new();

    // Empty tools array behaves like no tools.
    let non_empty = tools.as_ref().filter(|t| !t.is_empty());
    let Some(tools) = non_empty else {
        return PreparedXaiTools {
            tools: None,
            tool_choice: None,
            warnings,
        };
    };

    let mut xai_tools: Vec<Value> = Vec::new();
    for tool in tools {
        match tool {
            LanguageModelV4Tool::Provider(pt) => {
                warnings.push(Warning::unsupported(format!(
                    "provider-defined tool {}",
                    pt.id
                )));
            }
            LanguageModelV4Tool::Function(ft) => {
                let params = remove_additional_properties_false(&ft.input_schema);
                let mut func = serde_json::Map::new();
                func.insert("name".into(), Value::String(ft.name.clone()));
                // Mirror the TS, which always emits `description` (undefined
                // serializes to absent).
                if let Some(ref desc) = ft.description {
                    func.insert("description".into(), Value::String(desc.clone()));
                }
                func.insert("parameters".into(), params);
                if let Some(strict) = ft.strict {
                    func.insert("strict".into(), Value::Bool(strict));
                }
                xai_tools.push(json!({ "type": "function", "function": func }));
            }
        }
    }

    // If every requested tool was filtered out, emit neither `tools` nor
    // `tool_choice` — sending `"tools": []` is noise the API can misinterpret.
    if xai_tools.is_empty() {
        return PreparedXaiTools {
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

    PreparedXaiTools {
        tools: Some(xai_tools),
        tool_choice: tool_choice_value,
        warnings,
    }
}

#[cfg(test)]
#[path = "xai_prepare_tools.test.rs"]
mod tests;
