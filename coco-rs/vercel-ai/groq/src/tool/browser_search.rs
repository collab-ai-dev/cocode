use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider-tool id for Groq browser search (`<provider>.<tool>` form).
pub const BROWSER_SEARCH_TOOL_ID: &str = "groq.browser_search";

/// Browser search tool for Groq models.
///
/// Provides interactive browser-search capabilities that go beyond traditional
/// web search by navigating websites interactively. Currently supported on
/// `openai/gpt-oss-20b` and `openai/gpt-oss-120b`.
///
/// Add it to a call's `tools` as
/// `LanguageModelV4Tool::Provider(browser_search())`. The tool takes no input
/// parameters — it is activated automatically for supported models.
///
/// Mirrors `tool/browser-search.ts`.
pub fn browser_search() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::new("groq", "browser_search")
}

#[cfg(test)]
#[path = "browser_search.test.rs"]
mod tests;
