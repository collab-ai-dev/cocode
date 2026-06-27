use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a local_shell provider tool for the Responses API.
pub fn openai_local_shell_tool() -> LanguageModelV4ProviderTool {
    let args: HashMap<String, serde_json::Value> = HashMap::new();
    LanguageModelV4ProviderTool {
        id: "openai.local_shell".into(),
        name: "local_shell".into(),
        args,
    }
}
