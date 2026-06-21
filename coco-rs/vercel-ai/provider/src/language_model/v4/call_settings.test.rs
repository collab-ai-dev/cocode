use super::*;

#[test]
fn test_call_settings_default() {
    let settings = LanguageModelV4CallSettings::default();
    assert!(settings.max_output_tokens.is_none());
    assert!(settings.temperature.is_none());
    assert!(settings.top_p.is_none());
}

#[test]
fn test_call_settings_from_options() {
    let options = LanguageModelV4CallOptions::new(vec![])
        .with_max_output_tokens(1024)
        .with_temperature(0.7);
    let settings = LanguageModelV4CallSettings::from(options);
    assert_eq!(settings.max_output_tokens, Some(1024));
    assert_eq!(settings.temperature, Some(0.7));
}

#[test]
fn test_call_settings_serde() {
    let settings = LanguageModelV4CallSettings {
        max_output_tokens: Some(100),
        temperature: Some(0.5),
        ..Default::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    // Uses snake_case by default (serde default)
    assert!(json.contains("max_output_tokens") || json.contains("maxOutputTokens"));

    let parsed: LanguageModelV4CallSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.max_output_tokens, Some(100));
    assert_eq!(parsed.temperature, Some(0.5));
}
