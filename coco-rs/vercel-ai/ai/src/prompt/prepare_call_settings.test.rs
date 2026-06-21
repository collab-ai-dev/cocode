use super::*;
use std::collections::HashMap;

#[test]
fn test_prepare_call_settings_empty() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings::default();

    let result = prepare_call_settings(call_options, &settings);

    // Should remain unchanged
    assert!(result.max_output_tokens.is_none());
    assert!(result.temperature.is_none());
}

#[test]
fn test_prepare_call_settings_max_tokens() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings {
        max_tokens: Some(1000),
        ..Default::default()
    };

    let result = prepare_call_settings(call_options, &settings);

    assert_eq!(result.max_output_tokens, Some(1000));
}

#[test]
fn test_prepare_call_settings_temperature() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings {
        temperature: Some(0.7),
        ..Default::default()
    };

    let result = prepare_call_settings(call_options, &settings);

    assert_eq!(result.temperature, Some(0.7));
}

#[test]
fn test_prepare_call_settings_stop_sequences() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings {
        stop_sequences: Some(vec!["STOP".to_string(), "END".to_string()]),
        ..Default::default()
    };

    let result = prepare_call_settings(call_options, &settings);

    assert_eq!(
        result.stop_sequences,
        Some(vec!["STOP".to_string(), "END".to_string()])
    );
}

#[test]
fn test_prepare_call_settings_all() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let mut headers = HashMap::new();
    headers.insert("X-Custom".to_string(), "value".to_string());

    let settings = CallSettings {
        max_tokens: Some(500),
        temperature: Some(0.5),
        top_p: Some(0.9),
        top_k: Some(40),
        stop_sequences: Some(vec!["STOP".to_string()]),
        frequency_penalty: Some(0.1),
        presence_penalty: Some(0.2),
        seed: Some(42),
        headers: Some(headers),
        ..Default::default()
    };

    let result = prepare_call_settings(call_options, &settings);

    assert_eq!(result.max_output_tokens, Some(500));
    assert_eq!(result.temperature, Some(0.5));
    assert_eq!(result.top_p, Some(0.9));
    assert_eq!(result.top_k, Some(40));
    assert_eq!(result.stop_sequences, Some(vec!["STOP".to_string()]));
    assert_eq!(result.frequency_penalty, Some(0.1));
    assert_eq!(result.presence_penalty, Some(0.2));
    assert_eq!(result.seed, Some(42));
    assert!(result.headers.is_some());
}

#[test]
fn test_prepare_call_settings_with_defaults() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings::default();

    let result =
        prepare_call_settings_with_defaults(call_options, &settings, Some(2000), Some(0.8));

    assert_eq!(result.max_output_tokens, Some(2000));
    assert_eq!(result.temperature, Some(0.8));
}

#[test]
fn test_prepare_call_settings_defaults_not_override() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let settings = CallSettings {
        max_tokens: Some(100),
        temperature: Some(0.1),
        ..Default::default()
    };

    let result =
        prepare_call_settings_with_defaults(call_options, &settings, Some(2000), Some(0.8));

    // Settings should take precedence over defaults
    assert_eq!(result.max_output_tokens, Some(100));
    assert_eq!(result.temperature, Some(0.1));
}
