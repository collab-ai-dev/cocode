//! Code Execution tool.

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for Code Execution.
pub const CODE_EXECUTION_TOOL_ID: &str = "google.code_execution";

/// Create a Code Execution provider tool.
pub fn google_code_execution() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(CODE_EXECUTION_TOOL_ID, "code_execution")
}
