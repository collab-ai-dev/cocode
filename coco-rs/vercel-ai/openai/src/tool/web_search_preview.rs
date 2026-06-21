use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a web_search_preview provider tool for the Responses API.
///
/// # Arguments
/// - `search_context_size` - "low", "medium", or "high" (default: "medium")
/// - `user_location` - Optional user location for search context
pub fn openai_web_search_preview_tool(
    search_context_size: Option<&str>,
    user_location: Option<serde_json::Value>,
) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(size) = search_context_size {
        args.insert("search_context_size".into(), json!(size));
    }
    if let Some(loc) = user_location {
        args.insert("user_location".into(), loc);
    }
    LanguageModelV4ProviderTool {
        id: "openai.web_search_preview".into(),
        name: "web_search_preview".into(),
        args,
    }
}
