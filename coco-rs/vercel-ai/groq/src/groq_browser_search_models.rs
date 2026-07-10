/// Models that support Groq's browser-search provider tool.
///
/// Based on <https://console.groq.com/docs/browser-search>.
pub const BROWSER_SEARCH_SUPPORTED_MODELS: &[&str] = &["openai/gpt-oss-20b", "openai/gpt-oss-120b"];

/// Check if a model supports browser search.
pub fn is_browser_search_supported_model(model_id: &str) -> bool {
    BROWSER_SEARCH_SUPPORTED_MODELS.contains(&model_id)
}

/// Comma-separated list of supported models, for error messages.
pub fn supported_models_string() -> String {
    BROWSER_SEARCH_SUPPORTED_MODELS.join(", ")
}

#[cfg(test)]
#[path = "groq_browser_search_models.test.rs"]
mod tests;
