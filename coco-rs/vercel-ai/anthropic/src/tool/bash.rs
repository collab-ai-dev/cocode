use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Bash tool (2024-10-22) — Basic bash execution.
pub fn bash_20241022() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.bash_20241022".into(),
        name: "bash_20241022".into(),
        args: HashMap::new(),
    }
}

/// Bash tool (2025-01-24) — Enhanced bash execution.
pub fn bash_20250124() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.bash_20250124".into(),
        name: "bash_20250124".into(),
        args: HashMap::new(),
    }
}
