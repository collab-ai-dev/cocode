use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a tool_search provider tool for the Responses API.
///
/// # Arguments
/// - `execution` - Whether tool search runs on server or client (default: server)
/// - `description` - Optional description (only used for client-executed tool search)
/// - `parameters` - Optional JSON Schema for search arguments (only used for client-executed)
pub fn openai_tool_search_tool(
    execution: Option<&str>,
    description: Option<&str>,
    parameters: Option<serde_json::Value>,
) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(exec) = execution {
        args.insert("execution".into(), json!(exec));
    }
    if let Some(desc) = description {
        args.insert("description".into(), json!(desc));
    }
    if let Some(params) = parameters {
        args.insert("parameters".into(), params);
    }
    LanguageModelV4ProviderTool {
        id: "openai.tool_search".into(),
        name: "tool_search".into(),
        args,
    }
}
