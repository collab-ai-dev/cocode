use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Code execution tool (2025-05-22) — Python only, simple input/output.
pub fn code_execution_20250522() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.code_execution_20250522".into(),
        name: "code_execution_20250522".into(),
        args: HashMap::new(),
    }
}

/// Code execution tool (2025-08-25) — Python + Bash + Text editor with deferred results.
pub fn code_execution_20250825() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.code_execution_20250825".into(),
        name: "code_execution_20250825".into(),
        args: HashMap::new(),
    }
}

/// Code execution tool (2026-01-20) — Adds encrypted output, latest version.
pub fn code_execution_20260120() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.code_execution_20260120".into(),
        name: "code_execution_20260120".into(),
        args: HashMap::new(),
    }
}
