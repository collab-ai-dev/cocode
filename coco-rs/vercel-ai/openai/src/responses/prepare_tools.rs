use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

/// Result of preparing tools for the Responses API.
pub struct PreparedResponsesTools {
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// Convert SDK tools + tool_choice to OpenAI Responses API format.
///
/// Unlike Chat, the Responses API supports provider-specific tools
/// (web_search, file_search, code_interpreter, shell, etc.).
pub fn prepare_responses_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
) -> PreparedResponsesTools {
    let warnings = Vec::new();

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
                    if !params.is_object() {
                        params = json!({ "type": "object", "properties": {} });
                    }
                    let mut tool_obj = json!({
                        "type": "function",
                        "name": ft.name,
                        "parameters": params,
                    });
                    if let Some(ref desc) = ft.description {
                        tool_obj["description"] = Value::String(desc.clone());
                    }
                    if let Some(strict) = ft.strict {
                        tool_obj["strict"] = Value::Bool(strict);
                    }
                    // Read defer_loading from providerOptions.openai.deferLoading
                    if let Some(ref opts) = ft.provider_options
                        && let Some(openai_opts) = opts.get("openai")
                        && let Some(defer_val) = openai_opts.get("deferLoading")
                        && let Some(defer) = defer_val.as_bool()
                    {
                        tool_obj["defer_loading"] = Value::Bool(defer);
                    }
                    openai_tools.push(tool_obj);
                }
                LanguageModelV4Tool::Provider(pt) => {
                    // Determine the API tool type from the provider tool ID.
                    // The id format is "openai.<tool_type>" (e.g., "openai.file_search").
                    // For custom tools, the id is "openai.<custom_name>" and the API
                    // expects type="custom" with a separate "name" field.
                    let tool_type = if pt.id.starts_with("openai.") {
                        &pt.id["openai.".len()..]
                    } else {
                        &pt.name
                    };

                    // Known built-in tool types
                    let is_builtin = matches!(
                        tool_type,
                        "file_search"
                            | "web_search"
                            | "web_search_preview"
                            | "code_interpreter"
                            | "shell"
                            | "local_shell"
                            | "apply_patch"
                            | "image_generation"
                            | "mcp"
                            | "tool_search"
                    );

                    let api_type = if is_builtin { tool_type } else { "custom" };
                    let mut tool_obj = json!({ "type": api_type });

                    // For custom tools, set the name field explicitly
                    if !is_builtin {
                        tool_obj["name"] = Value::String(pt.name.clone());
                    }

                    // Merge args into the tool object in sorted key order.
                    // `pt.args` is a `HashMap` whose iteration order varies per
                    // instance (per-`HashMap` `RandomState` seed), and
                    // `serde_json::Value` preserves insertion order
                    // (`preserve_order` is enabled via the dep tree). Inserting
                    // in HashMap order would make the tool-definition bytes —
                    // which sit at the request prefix — non-deterministic across
                    // turns and silently break prompt caching. Sorting pins it.
                    let mut keys: Vec<&String> = pt.args.keys().collect();
                    keys.sort();
                    for k in keys {
                        tool_obj[k] = pt.args[k].clone();
                    }
                    openai_tools.push(tool_obj);
                }
            }
        }

        if openai_tools.is_empty() {
            None
        } else {
            Some(openai_tools)
        }
    });

    // Collect custom provider tool names for tool_choice resolution
    let custom_tool_names: std::collections::HashSet<&str> = tools
        .as_ref()
        .map(|ts| {
            ts.iter()
                .filter_map(|t| match t {
                    LanguageModelV4Tool::Provider(pt) => {
                        let tool_type = if pt.id.starts_with("openai.") {
                            &pt.id["openai.".len()..]
                        } else {
                            pt.name.as_str()
                        };
                        let is_builtin = matches!(
                            tool_type,
                            "file_search"
                                | "web_search"
                                | "web_search_preview"
                                | "code_interpreter"
                                | "shell"
                                | "local_shell"
                                | "apply_patch"
                                | "image_generation"
                                | "mcp"
                        );
                        if !is_builtin {
                            Some(pt.name.as_str())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    let tool_choice_value = tool_choice.as_ref().map(|tc| match tc {
        LanguageModelV4ToolChoice::Auto => json!("auto"),
        LanguageModelV4ToolChoice::None => json!("none"),
        LanguageModelV4ToolChoice::Required => json!("required"),
        LanguageModelV4ToolChoice::Tool { tool_name } => {
            // Provider tool types use { "type": "<tool_type>" }
            let provider_tool_types = [
                "code_interpreter",
                "file_search",
                "image_generation",
                "web_search_preview",
                "web_search",
                "mcp",
                "apply_patch",
                "tool_search",
            ];
            if provider_tool_types.contains(&tool_name.as_str()) {
                json!({ "type": tool_name })
            } else if custom_tool_names.contains(tool_name.as_str()) {
                json!({ "type": "custom", "name": tool_name })
            } else {
                json!({ "type": "function", "name": tool_name })
            }
        }
    });

    PreparedResponsesTools {
        tools: tools_value,
        tool_choice: tool_choice_value,
        warnings,
    }
}

#[cfg(test)]
#[path = "prepare_tools.test.rs"]
mod tests;
