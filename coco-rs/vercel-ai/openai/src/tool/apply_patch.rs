use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create an apply_patch provider tool for the Responses API.
pub fn openai_apply_patch_tool() -> LanguageModelV4ProviderTool {
    let args: HashMap<String, serde_json::Value> = HashMap::new();
    LanguageModelV4ProviderTool {
        id: "openai.apply_patch".into(),
        name: "apply_patch".into(),
        args,
    }
}
