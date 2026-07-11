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
    let opts = extract_xai_responses_options(&None);
    assert_eq!(opts.reasoning_effort, None);
    assert_eq!(opts.reasoning_summary, None);
    assert_eq!(opts.store, None);
    assert_eq!(opts.include, None);
}

#[test]
fn parses_keys() {
    let po = provider_options(serde_json::json!({
        "reasoningEffort": "high",
        "reasoningSummary": "detailed",
        "logprobs": true,
        "topLogprobs": 5,
        "store": false,
        "previousResponseId": "resp_1",
        "include": ["file_search_call.results"],
    }));
    let opts = extract_xai_responses_options(&Some(po));
    assert_eq!(opts.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(opts.reasoning_summary.as_deref(), Some("detailed"));
    assert_eq!(opts.logprobs, Some(true));
    assert_eq!(opts.top_logprobs, Some(5));
    assert_eq!(opts.store, Some(false));
    assert_eq!(opts.previous_response_id.as_deref(), Some("resp_1"));
    assert_eq!(
        opts.include,
        Some(vec!["file_search_call.results".to_string()])
    );
}

#[test]
fn ignores_other_namespaces() {
    let mut ns = HashMap::new();
    ns.insert("store".to_string(), serde_json::json!(false));
    let mut map = HashMap::new();
    map.insert("openai".to_string(), ns);
    let opts = extract_xai_responses_options(&Some(ProviderOptions(map)));
    assert_eq!(opts.store, None);
}
