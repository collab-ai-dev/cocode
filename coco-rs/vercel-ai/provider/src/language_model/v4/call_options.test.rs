use super::*;

#[test]
fn test_call_options_new() {
    let prompt = vec![];
    let options = LanguageModelV4CallOptions::new(prompt);
    assert_eq!(options.prompt.len(), 0);
    assert!(options.max_output_tokens.is_none());
    assert!(options.temperature.is_none());
}

#[test]
fn test_call_options_with_max_output_tokens() {
    let options = LanguageModelV4CallOptions::new(vec![]).with_max_output_tokens(1024);
    assert_eq!(options.max_output_tokens, Some(1024));
}

#[test]
fn test_call_options_with_temperature() {
    let options = LanguageModelV4CallOptions::new(vec![]).with_temperature(0.7);
    assert_eq!(options.temperature, Some(0.7));
}

#[test]
fn test_call_options_with_top_p() {
    let options = LanguageModelV4CallOptions::new(vec![]).with_top_p(0.9);
    assert_eq!(options.top_p, Some(0.9));
}

#[test]
fn test_call_options_with_stop_sequences() {
    let options = LanguageModelV4CallOptions::new(vec![])
        .with_stop_sequences(vec!["STOP".to_string(), "END".to_string()]);
    assert_eq!(
        options.stop_sequences,
        Some(vec!["STOP".to_string(), "END".to_string()])
    );
}

#[test]
fn test_call_options_builder_chain() {
    let options = LanguageModelV4CallOptions::new(vec![])
        .with_max_output_tokens(2048)
        .with_temperature(0.5);
    assert_eq!(options.max_output_tokens, Some(2048));
    assert_eq!(options.temperature, Some(0.5));
}

#[test]
fn test_call_options_default_reasoning_is_none() {
    let options = LanguageModelV4CallOptions::default();
    assert!(options.reasoning.is_none());
}

#[test]
fn test_call_options_with_reasoning() {
    let options = LanguageModelV4CallOptions::new(vec![]).with_reasoning(ReasoningLevel::High);
    assert_eq!(options.reasoning, Some(ReasoningLevel::High));
}

#[test]
fn test_reasoning_level_serde_round_trip() {
    for (level, expected_str) in [
        (ReasoningLevel::ProviderDefault, "\"provider-default\""),
        (ReasoningLevel::Off, "\"off\""),
        (ReasoningLevel::Minimal, "\"minimal\""),
        (ReasoningLevel::Low, "\"low\""),
        (ReasoningLevel::Medium, "\"medium\""),
        (ReasoningLevel::High, "\"high\""),
        (ReasoningLevel::Xhigh, "\"xhigh\""),
    ] {
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, expected_str, "serialize {level:?}");
        let deserialized: ReasoningLevel = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, level, "deserialize {level:?}");
    }
}

#[test]
fn test_reasoning_level_as_str() {
    assert_eq!(ReasoningLevel::ProviderDefault.as_str(), "provider-default");
    assert_eq!(ReasoningLevel::Off.as_str(), "off");
    assert_eq!(ReasoningLevel::Minimal.as_str(), "minimal");
    assert_eq!(ReasoningLevel::Low.as_str(), "low");
    assert_eq!(ReasoningLevel::Medium.as_str(), "medium");
    assert_eq!(ReasoningLevel::High.as_str(), "high");
    assert_eq!(ReasoningLevel::Xhigh.as_str(), "xhigh");
}
