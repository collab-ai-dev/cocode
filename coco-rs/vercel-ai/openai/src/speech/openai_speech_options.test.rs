use std::collections::HashMap;

use vercel_ai_provider::ProviderOptions;

use super::*;

fn make_provider_opts(openai_map: HashMap<String, serde_json::Value>) -> Option<ProviderOptions> {
    let mut map = HashMap::new();
    map.insert("openai".into(), openai_map);
    Some(ProviderOptions(map))
}

#[test]
fn extract_speech_options_returns_default_when_none() {
    let opts = extract_speech_options(&None);
    assert!(opts.instructions.is_none());
    assert!(opts.speed.is_none());
}

#[test]
fn extract_speech_options_returns_default_when_no_openai_key() {
    let mut map = HashMap::new();
    let mut other_map = HashMap::new();
    other_map.insert("speed".into(), serde_json::json!(2.0));
    map.insert("other".into(), other_map);
    let provider_opts = Some(ProviderOptions(map));
    let opts = extract_speech_options(&provider_opts);
    assert!(opts.speed.is_none());
}

#[test]
fn extract_speech_options_extracts_all_fields() {
    let mut openai_map = HashMap::new();
    openai_map.insert("instructions".into(), serde_json::json!("Speak slowly"));
    openai_map.insert("speed".into(), serde_json::json!(0.5));
    let provider_opts = make_provider_opts(openai_map);
    let opts = extract_speech_options(&provider_opts);
    assert_eq!(opts.instructions.as_deref(), Some("Speak slowly"));
    assert_eq!(opts.speed, Some(0.5));
}
