//! File Search tool.

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for File Search.
pub const FILE_SEARCH_TOOL_ID: &str = "google.file_search";

/// Create a File Search provider tool.
pub fn google_file_search() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(FILE_SEARCH_TOOL_ID, "file_search")
}
