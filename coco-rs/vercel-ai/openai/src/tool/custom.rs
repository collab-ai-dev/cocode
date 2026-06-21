use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create an OpenAI Responses **custom grammar tool** — a provider-defined tool
/// whose body is constrained by a grammar instead of a JSON schema (the
/// mechanism behind freeform tools like `apply_patch`).
///
/// Encapsulates the OpenAI-specific realization so callers (e.g.
/// `app/query::engine_prompt`, via the `coco_inference` re-export) only supply
/// the neutral `(name, description, syntax, definition)` and never spell out
/// the `openai.custom` id or the `{type:"grammar", …}` wire shape. The id
/// prefix makes `prepare_tools` serialize it as `{type:"custom", name, format}`.
pub fn openai_custom_grammar_tool(
    name: impl Into<String>,
    description: impl Into<String>,
    syntax: impl Into<String>,
    definition: impl Into<String>,
) -> LanguageModelV4ProviderTool {
    let args: HashMap<String, serde_json::Value> = HashMap::from([
        ("description".into(), json!(description.into())),
        (
            "format".into(),
            json!({
                "type": "grammar",
                "syntax": syntax.into(),
                "definition": definition.into(),
            }),
        ),
    ]);
    LanguageModelV4ProviderTool {
        id: "openai.custom".into(),
        name: name.into(),
        args,
    }
}
