use std::collections::HashMap;

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a shell provider tool for the Responses API.
///
/// # Arguments
/// - `environment` - Optional environment configuration (containerAuto, containerReference, or local)
pub fn openai_shell_tool(environment: Option<serde_json::Value>) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(env) = environment {
        args.insert("environment".into(), env);
    }
    LanguageModelV4ProviderTool {
        id: "openai.shell".into(),
        name: "shell".into(),
        args,
    }
}
