use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Text editor tool (2024-10-22).
pub fn text_editor_20241022() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.text_editor_20241022".into(),
        name: "text_editor_20241022".into(),
        args: HashMap::new(),
    }
}

/// Text editor tool (2025-01-24).
pub fn text_editor_20250124() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.text_editor_20250124".into(),
        name: "text_editor_20250124".into(),
        args: HashMap::new(),
    }
}

/// Text editor tool (2025-04-29) — Removes undo_edit support.
pub fn text_editor_20250429() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.text_editor_20250429".into(),
        name: "text_editor_20250429".into(),
        args: HashMap::new(),
    }
}

/// Text editor tool (2025-07-28) — Adds optional maxCharacters parameter.
pub fn text_editor_20250728(max_characters: Option<u32>) -> LanguageModelV4ProviderTool {
    let mut args = HashMap::new();
    if let Some(max) = max_characters {
        args.insert("maxCharacters".into(), json!(max));
    }
    LanguageModelV4ProviderTool {
        id: "anthropic.text_editor_20250728".into(),
        name: "text_editor_20250728".into(),
        args,
    }
}
