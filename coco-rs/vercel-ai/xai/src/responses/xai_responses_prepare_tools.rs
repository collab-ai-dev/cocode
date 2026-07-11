use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

use crate::remove_additional_properties::remove_additional_properties_false;

use super::xai_tools::XaiFileSearchArgs;
use super::xai_tools::XaiMcpServerArgs;
use super::xai_tools::XaiWebSearchArgs;
use super::xai_tools::XaiXSearchArgs;
use super::xai_tools::parse_tool_args;

/// Result of preparing tools for the xAI Responses API.
pub struct PreparedXaiResponsesTools {
    /// Wire tool array. Empty when every tool was filtered out; the model omits
    /// the `tools` field in that case.
    pub tools: Vec<Value>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// Convert SDK tools + tool_choice into the xAI Responses wire format.
///
/// Mirrors `xai-responses-prepare-tools.ts`. Provider-defined tools map to the
/// server-side agentic tools by id; function tools are sanitized via
/// [`remove_additional_properties_false`]. Forcing a server-side tool via
/// `tool_choice` is unsupported and warns + drops the choice.
pub fn prepare_responses_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
) -> PreparedXaiResponsesTools {
    let mut warnings = Vec::new();

    let non_empty = tools.as_ref().filter(|t| !t.is_empty());
    let Some(tools) = non_empty else {
        return PreparedXaiResponsesTools {
            tools: Vec::new(),
            tool_choice: None,
            warnings,
        };
    };

    let mut xai_tools: Vec<Value> = Vec::new();
    for tool in tools {
        match tool {
            LanguageModelV4Tool::Provider(pt) => match pt.id.as_str() {
                "xai.web_search" => {
                    xai_tools.push(parse_tool_args::<XaiWebSearchArgs>(&pt.args).to_wire());
                }
                "xai.x_search" => {
                    xai_tools.push(parse_tool_args::<XaiXSearchArgs>(&pt.args).to_wire());
                }
                "xai.code_execution" => {
                    xai_tools.push(json!({ "type": "code_interpreter" }));
                }
                "xai.view_image" => {
                    xai_tools.push(json!({ "type": "view_image" }));
                }
                "xai.view_x_video" => {
                    xai_tools.push(json!({ "type": "view_x_video" }));
                }
                "xai.file_search" => {
                    xai_tools.push(parse_tool_args::<XaiFileSearchArgs>(&pt.args).to_wire());
                }
                "xai.mcp" => {
                    xai_tools.push(parse_tool_args::<XaiMcpServerArgs>(&pt.args).to_wire());
                }
                _ => {
                    warnings.push(Warning::unsupported(format!(
                        "provider-defined tool {}",
                        pt.name
                    )));
                }
            },
            LanguageModelV4Tool::Function(ft) => {
                let params = remove_additional_properties_false(&ft.input_schema);
                let mut func = serde_json::Map::new();
                func.insert("type".into(), Value::String("function".into()));
                func.insert("name".into(), Value::String(ft.name.clone()));
                if let Some(ref desc) = ft.description {
                    func.insert("description".into(), Value::String(desc.clone()));
                }
                func.insert("parameters".into(), params);
                if let Some(strict) = ft.strict {
                    func.insert("strict".into(), Value::Bool(strict));
                }
                xai_tools.push(Value::Object(func));
            }
        }
    }

    let tool_choice_value = tool_choice
        .as_ref()
        .and_then(|tc| resolve_tool_choice(tc, tools, &mut warnings));

    PreparedXaiResponsesTools {
        tools: xai_tools,
        tool_choice: tool_choice_value,
        warnings,
    }
}

/// Resolve `tool_choice` into the xAI wire value. Returns `None` when the
/// choice cannot be honored (unknown name, or a server-side tool that cannot be
/// force-selected — the latter emits a warning).
fn resolve_tool_choice(
    tool_choice: &LanguageModelV4ToolChoice,
    tools: &[LanguageModelV4Tool],
    warnings: &mut Vec<Warning>,
) -> Option<Value> {
    match tool_choice {
        LanguageModelV4ToolChoice::Auto => Some(json!("auto")),
        LanguageModelV4ToolChoice::None => Some(json!("none")),
        LanguageModelV4ToolChoice::Required => Some(json!("required")),
        LanguageModelV4ToolChoice::Tool { tool_name } => {
            let selected = tools.iter().find(|t| t.name() == tool_name)?;
            if let LanguageModelV4Tool::Provider(_) = selected {
                // xAI cannot force a specific server-side tool via tool_choice.
                warnings.push(Warning::unsupported(format!(
                    "toolChoice for server-side tool \"{tool_name}\""
                )));
                return None;
            }
            Some(json!({ "type": "function", "name": tool_name }))
        }
    }
}

#[cfg(test)]
#[path = "xai_responses_prepare_tools.test.rs"]
mod tests;
