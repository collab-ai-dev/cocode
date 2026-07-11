use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;

fn provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("xai".to_string(), ns);
    ProviderOptions(map)
}

#[test]
fn defaults_when_absent() {
    let opts = extract_xai_chat_options(&None);
    assert_eq!(opts.reasoning_effort, None);
    assert_eq!(opts.logprobs, None);
    assert_eq!(opts.top_logprobs, None);
    assert_eq!(opts.parallel_function_calling, None);
}

#[test]
fn parses_keys() {
    let po = provider_options(serde_json::json!({
        "reasoningEffort": "high",
        "logprobs": true,
        "topLogprobs": 5,
        "parallel_function_calling": false
    }));
    let opts = extract_xai_chat_options(&Some(po));
    assert_eq!(opts.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(opts.logprobs, Some(true));
    assert_eq!(opts.top_logprobs, Some(5));
    assert_eq!(opts.parallel_function_calling, Some(false));
}

#[test]
fn ignores_other_namespaces() {
    let mut ns = HashMap::new();
    ns.insert("logprobs".to_string(), serde_json::json!(true));
    let mut map = HashMap::new();
    map.insert("openai".to_string(), ns);
    let opts = extract_xai_chat_options(&Some(ProviderOptions(map)));
    assert_eq!(opts.logprobs, None);
}
