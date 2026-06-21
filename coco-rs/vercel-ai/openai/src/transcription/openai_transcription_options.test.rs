use std::collections::HashMap;

use vercel_ai_provider::ProviderOptions;

use super::*;

fn make_provider_opts(openai_map: HashMap<String, serde_json::Value>) -> Option<ProviderOptions> {
    let mut map = HashMap::new();
    map.insert("openai".into(), openai_map);
    Some(ProviderOptions(map))
}

#[test]
fn extract_transcription_options_returns_default_when_none() {
    let opts = extract_transcription_options(&None);
    assert!(opts.include.is_none());
    assert!(opts.language.is_none());
    assert!(opts.prompt.is_none());
    assert!(opts.temperature.is_none());
    assert!(opts.timestamp_granularities.is_none());
}

#[test]
fn extract_transcription_options_returns_default_when_no_openai_key() {
    let mut map = HashMap::new();
    let mut other_map = HashMap::new();
    other_map.insert("language".into(), serde_json::json!("en"));
    map.insert("other".into(), other_map);
    let provider_opts = Some(ProviderOptions(map));
    let opts = extract_transcription_options(&provider_opts);
    assert!(opts.language.is_none());
}

#[test]
fn extract_transcription_options_extracts_all_fields() {
    let mut openai_map = HashMap::new();
    openai_map.insert("include".into(), serde_json::json!(["logprobs"]));
    openai_map.insert("language".into(), serde_json::json!("en"));
    openai_map.insert("prompt".into(), serde_json::json!("Technical discussion"));
    openai_map.insert("temperature".into(), serde_json::json!(0.5));
    openai_map.insert(
        "timestampGranularities".into(),
        serde_json::json!(["word", "segment"]),
    );
    let provider_opts = make_provider_opts(openai_map);
    let opts = extract_transcription_options(&provider_opts);
    assert_eq!(opts.include.as_deref(), Some(&["logprobs".to_string()][..]));
    assert_eq!(opts.language.as_deref(), Some("en"));
    assert_eq!(opts.prompt.as_deref(), Some("Technical discussion"));
    assert_eq!(opts.temperature, Some(0.5));
    assert_eq!(
        opts.timestamp_granularities.as_deref(),
        Some(&["word".to_string(), "segment".to_string()][..])
    );
}

#[test]
fn extract_transcription_options_partial_fields() {
    let mut openai_map = HashMap::new();
    openai_map.insert("language".into(), serde_json::json!("fr"));
    let provider_opts = make_provider_opts(openai_map);
    let opts = extract_transcription_options(&provider_opts);
    assert_eq!(opts.language.as_deref(), Some("fr"));
    assert!(opts.include.is_none());
    assert!(opts.timestamp_granularities.is_none());
}
