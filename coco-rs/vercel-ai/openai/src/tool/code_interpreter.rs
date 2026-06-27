use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a code_interpreter provider tool for the Responses API.
///
/// # Arguments
/// - `container` - Optional container config: either a string ID or `{ "fileIds": [...] }`
pub fn openai_code_interpreter_tool(
    container: Option<serde_json::Value>,
) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(c) = container {
        args.insert("container".into(), c);
    }
    LanguageModelV4ProviderTool {
        id: "openai.code_interpreter".into(),
        name: "code_interpreter".into(),
        args,
    }
}
