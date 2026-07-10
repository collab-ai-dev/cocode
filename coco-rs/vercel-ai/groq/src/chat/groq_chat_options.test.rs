use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;

fn provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("groq".to_string(), ns);
    ProviderOptions(map)
}

#[test]
fn defaults_when_absent() {
    let opts = extract_groq_chat_options(&None);
    assert_eq!(opts.reasoning_format, None);
    assert_eq!(opts.structured_outputs, None);
    assert_eq!(opts.service_tier, None);
}

#[test]
fn parses_camel_case_keys() {
    let po = provider_options(serde_json::json!({
        "reasoningFormat": "parsed",
        "reasoningEffort": "high",
        "parallelToolCalls": false,
        "user": "u-1",
        "structuredOutputs": false,
        "strictJsonSchema": false,
        "serviceTier": "flex"
    }));
    let opts = extract_groq_chat_options(&Some(po));
    assert_eq!(opts.reasoning_format.as_deref(), Some("parsed"));
    assert_eq!(opts.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(opts.parallel_tool_calls, Some(false));
    assert_eq!(opts.user.as_deref(), Some("u-1"));
    assert_eq!(opts.structured_outputs, Some(false));
    assert_eq!(opts.strict_json_schema, Some(false));
    assert_eq!(opts.service_tier.as_deref(), Some("flex"));
}

#[test]
fn ignores_other_namespaces() {
    let mut ns = HashMap::new();
    ns.insert("user".to_string(), serde_json::json!("nope"));
    let mut map = HashMap::new();
    map.insert("openai".to_string(), ns);
    let opts = extract_groq_chat_options(&Some(ProviderOptions(map)));
    assert_eq!(opts.user, None);
}
