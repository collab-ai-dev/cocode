//! Enterprise Web Search tool.

use serde_json::Value;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for Enterprise Web Search.
pub const ENTERPRISE_WEB_SEARCH_TOOL_ID: &str = "google.enterprise_web_search";

/// Create an Enterprise Web Search provider tool.
pub fn google_enterprise_web_search() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(ENTERPRISE_WEB_SEARCH_TOOL_ID, "enterprise_web_search")
}

/// Create an Enterprise Web Search provider tool with a search engine ID.
pub fn google_enterprise_web_search_with_engine(
    search_engine_id: impl Into<String>,
) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(ENTERPRISE_WEB_SEARCH_TOOL_ID, "enterprise_web_search")
        .with_arg("searchEngineId", Value::String(search_engine_id.into()))
}
