use super::*;
use crate::prompt::CallSettings;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::language_model::v4::LanguageModelV4FunctionTool;

#[test]
fn test_build_call_options_defaults() {
    let settings = CallSettings::default();
    let opts = build_call_options(&settings, &None, &None, &None, vec![], &None);

    assert!(opts.max_output_tokens.is_none());
    assert!(opts.temperature.is_none());
    assert!(opts.tools.is_none());
}

#[test]
fn test_build_call_options_all_settings() {
    let mut headers = HashMap::new();
    headers.insert("X-Custom".to_string(), "value".to_string());

    let settings = CallSettings::new()
        .with_max_tokens(1000)
        .with_temperature(0.7)
        .with_top_p(0.9)
        .with_top_k(40)
        .with_stop_sequences(vec!["STOP".to_string()])
        .with_frequency_penalty(0.5)
        .with_presence_penalty(0.3)
        .with_seed(42)
        .with_headers(headers);

    let opts = build_call_options(&settings, &None, &None, &None, vec![], &None);

    assert_eq!(opts.max_output_tokens, Some(1000));
    assert_eq!(opts.temperature, Some(0.7));
    assert_eq!(opts.top_p, Some(0.9));
    assert_eq!(opts.top_k, Some(40));
    assert_eq!(opts.stop_sequences, Some(vec!["STOP".to_string()]));
    assert_eq!(opts.frequency_penalty, Some(0.5));
    assert_eq!(opts.presence_penalty, Some(0.3));
    assert_eq!(opts.seed, Some(42));
    assert!(opts.headers.is_some());
}

#[test]
fn test_build_call_options_merges_provider_options() {
    let mut base_openai = HashMap::new();
    base_openai.insert(
        "reasoning".to_string(),
        serde_json::json!({ "effort": "low", "summary": "auto" }),
    );
    base_openai.insert("store".to_string(), serde_json::json!(true));

    let mut base = ProviderOptions::new();
    base.set("openai", base_openai);

    let mut override_openai = HashMap::new();
    override_openai.insert(
        "reasoning".to_string(),
        serde_json::json!({ "effort": "high" }),
    );
    override_openai.insert("metadata".to_string(), serde_json::json!({ "a": 1 }));

    let mut overrides = ProviderOptions::new();
    overrides.set("openai", override_openai);

    let settings = CallSettings::new().with_provider_options(base);
    let opts = build_call_options(&settings, &None, &Some(overrides), &None, vec![], &None);

    let provider_options = opts.provider_options.expect("provider options");
    let openai = provider_options.get("openai").expect("openai options");
    assert_eq!(
        openai.get("reasoning"),
        Some(&serde_json::json!({ "effort": "high", "summary": "auto" }))
    );
    assert_eq!(openai.get("store"), Some(&serde_json::json!(true)));
    assert_eq!(openai.get("metadata"), Some(&serde_json::json!({ "a": 1 })));
}

#[test]
fn test_merge_provider_options_skips_polluting_keys() {
    let mut override_openai = HashMap::new();
    override_openai.insert(
        "__proto__".to_string(),
        serde_json::json!({ "polluted": true }),
    );
    override_openai.insert("safe".to_string(), serde_json::json!(1));

    let mut overrides = ProviderOptions::new();
    overrides.set("openai", override_openai);

    let merged = merge_provider_options(None, Some(&overrides)).expect("merged options");
    let openai = merged.get("openai").expect("openai options");
    assert!(!openai.contains_key("__proto__"));
    assert_eq!(openai.get("safe"), Some(&serde_json::json!(1)));
}

#[test]
fn test_filter_active_tools_none() {
    assert!(filter_active_tools(&None, &None).is_none());
}

#[test]
fn test_filter_active_tools_no_filter() {
    let tools = vec![LanguageModelV4Tool::function(
        LanguageModelV4FunctionTool::new("tool_a", serde_json::json!({})),
    )];
    let result = filter_active_tools(&Some(tools), &None);
    assert_eq!(result.as_ref().map(Vec::len), Some(1));
}

#[test]
fn test_filter_active_tools_with_filter() {
    let tools = vec![
        LanguageModelV4Tool::function(LanguageModelV4FunctionTool::new(
            "tool_a",
            serde_json::json!({}),
        )),
        LanguageModelV4Tool::function(LanguageModelV4FunctionTool::new(
            "tool_b",
            serde_json::json!({}),
        )),
    ];
    let active = vec!["tool_a".to_string()];
    let result = filter_active_tools(&Some(tools), &Some(active));
    assert_eq!(result.as_ref().map(Vec::len), Some(1));
}
