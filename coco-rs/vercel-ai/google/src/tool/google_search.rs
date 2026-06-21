//! Google Search tool for grounding.

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for Google Search.
pub const GOOGLE_SEARCH_TOOL_ID: &str = "google.google_search";

/// Create a Google Search provider tool for grounding.
pub fn google_search() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(GOOGLE_SEARCH_TOOL_ID, "google_search")
}
