//! Prepare tools for Google Generative AI API calls.

use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

use crate::convert_json_schema_to_openapi_schema::convert_json_schema_to_openapi_schema;
use crate::tool::code_execution::CODE_EXECUTION_TOOL_ID;
use crate::tool::enterprise_web_search::ENTERPRISE_WEB_SEARCH_TOOL_ID;
use crate::tool::file_search::FILE_SEARCH_TOOL_ID;
use crate::tool::google_maps::GOOGLE_MAPS_TOOL_ID;
use crate::tool::google_search::GOOGLE_SEARCH_TOOL_ID;
use crate::tool::url_context::URL_CONTEXT_TOOL_ID;
use crate::tool::vertex_rag_store::VERTEX_RAG_STORE_TOOL_ID;

/// Result of preparing tools for the Google API.
#[derive(Debug, Clone, Default)]
pub struct PreparedTools {
    /// Function declarations for Google API.
    pub function_declarations: Option<Value>,
    /// Tool config (mode and allowed function names).
    pub tool_config: Option<Value>,
    /// Additional tool entries (google_search, code_execution, etc.).
    pub tool_entries: Vec<Value>,
    /// Warnings generated during tool preparation.
    pub tool_warnings: Vec<Warning>,
}

/// Prepare tools for a Google API request.
///
/// Converts Vercel AI SDK tool definitions and tool choice into
/// the Google API format.
pub fn prepare_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
    model_id: &str,
) -> PreparedTools {
    let tools = match tools {
        Some(t) if !t.is_empty() => t,
        _ => return PreparedTools::default(),
    };

    let model_lower = model_id.to_lowercase();
    let is_latest = matches!(
        model_lower.as_str(),
        "gemini-flash-latest" | "gemini-flash-lite-latest" | "gemini-pro-latest"
    );
    let is_gemini_2_or_newer = model_lower.contains("gemini-2")
        || model_lower.contains("gemini-3")
        || model_lower.contains("nano-banana")
        || is_latest;
    let supports_file_search =
        model_lower.contains("gemini-2.5") || model_lower.contains("gemini-3");

    let mut function_declarations: Vec<Value> = Vec::new();
    let mut tool_entries: Vec<Value> = Vec::new();
    let mut tool_warnings: Vec<Warning> = Vec::new();
    let mut has_provider_tools = false;

    for tool in tools {
        match tool {
            LanguageModelV4Tool::Function(func_tool) => {
                let mut decl = json!({
                    "name": func_tool.name,
                    "description": func_tool.description.as_deref().unwrap_or(""),
                });

                // Convert JSON Schema to OpenAPI schema
                let schema_value = serde_json::to_value(&func_tool.input_schema)
                    .unwrap_or(Value::Object(Default::default()));
                if let Some(openapi_schema) = convert_json_schema_to_openapi_schema(&schema_value) {
                    decl["parameters"] = openapi_schema;
                }

                function_declarations.push(decl);
            }
            LanguageModelV4Tool::Provider(provider_tool) => match provider_tool.id.as_str() {
                GOOGLE_SEARCH_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.google_search requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "googleSearch": provider_tool.args }));
                }
                URL_CONTEXT_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.url_context requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "urlContext": {} }));
                }
                CODE_EXECUTION_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.code_execution requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "codeExecution": {} }));
                }
                ENTERPRISE_WEB_SEARCH_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.enterprise_web_search requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "enterpriseWebSearch": {} }));
                }
                FILE_SEARCH_TOOL_ID => {
                    if !supports_file_search {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.file_search requires Gemini 2.5 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "fileSearch": provider_tool.args }));
                }
                GOOGLE_MAPS_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.google_maps requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    tool_entries.push(json!({ "googleMaps": {} }));
                }
                VERTEX_RAG_STORE_TOOL_ID => {
                    if !is_gemini_2_or_newer {
                        tool_warnings.push(Warning::unsupported_with_details(
                            "provider-defined tool",
                            "google.vertex_rag_store requires Gemini 2.0 or newer",
                        ));
                        continue;
                    }
                    has_provider_tools = true;
                    let mut config = json!({});
                    if let Some(rag_corpus) = provider_tool.args.get("ragCorpus") {
                        config["rag_resources"] = json!({ "rag_corpus": rag_corpus });
                    }
                    if let Some(top_k) = provider_tool.args.get("topK") {
                        config["similarity_top_k"] = top_k.clone();
                    }
                    tool_entries.push(json!({ "retrieval": { "vertex_rag_store": config } }));
                }
                _ => {
                    tool_warnings.push(Warning::unsupported(format!(
                        "Unsupported provider-defined tool: {}",
                        provider_tool.id
                    )));
                    continue;
                }
            },
        }
    }

    if !function_declarations.is_empty() && has_provider_tools {
        tool_warnings.push(Warning::unsupported(
            "combination of function and provider-defined tools",
        ));
    }

    // Detect strict tools
    let has_strict_tools = tools
        .iter()
        .any(|t| matches!(t, LanguageModelV4Tool::Function(f) if f.strict == Some(true)));

    // Build tool config from tool_choice
    let tool_config = match tool_choice.as_ref() {
        None => {
            if has_strict_tools {
                Some(json!({ "functionCallingConfig": { "mode": "VALIDATED" } }))
            } else {
                None
            }
        }
        Some(LanguageModelV4ToolChoice::Auto) => {
            let mode = if has_strict_tools {
                "VALIDATED"
            } else {
                "AUTO"
            };
            Some(json!({ "functionCallingConfig": { "mode": mode } }))
        }
        Some(LanguageModelV4ToolChoice::None) => {
            Some(json!({ "functionCallingConfig": { "mode": "NONE" } }))
        }
        Some(LanguageModelV4ToolChoice::Required) => {
            let mode = if has_strict_tools { "VALIDATED" } else { "ANY" };
            Some(json!({ "functionCallingConfig": { "mode": mode } }))
        }
        Some(LanguageModelV4ToolChoice::Tool { tool_name }) => {
            let mode = if has_strict_tools { "VALIDATED" } else { "ANY" };
            Some(json!({
                "functionCallingConfig": {
                    "mode": mode,
                    "allowedFunctionNames": [tool_name]
                }
            }))
        }
    };

    let function_declarations = if function_declarations.is_empty() {
        None
    } else {
        Some(json!(function_declarations))
    };

    PreparedTools {
        function_declarations,
        tool_config,
        tool_entries,
        tool_warnings,
    }
}

#[cfg(test)]
#[path = "google_prepare_tools.test.rs"]
mod tests;
