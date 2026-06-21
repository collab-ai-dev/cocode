//! URL Context tool for grounding with web content.

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for URL Context.
pub const URL_CONTEXT_TOOL_ID: &str = "google.url_context";

/// Create a URL Context provider tool.
pub fn google_url_context() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(URL_CONTEXT_TOOL_ID, "url_context")
}
