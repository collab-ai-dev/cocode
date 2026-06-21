use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Memory tool (2025-08-18) — File management (view, create, str_replace, insert, delete, rename).
pub fn memory_20250818() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.memory_20250818".into(),
        name: "memory_20250818".into(),
        args: HashMap::new(),
    }
}
