use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Tool search (regex) tool (2025-11-19).
pub fn tool_search_regex_20251119() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.tool_search_regex_20251119".into(),
        name: "tool_search_regex_20251119".into(),
        args: HashMap::new(),
    }
}

/// Tool search (BM25) tool (2025-11-19).
pub fn tool_search_bm25_20251119() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.tool_search_bm25_20251119".into(),
        name: "tool_search_bm25_20251119".into(),
        args: HashMap::new(),
    }
}
