use super::*;

#[test]
fn test_reasoning_new() {
    let reasoning = LanguageModelV4Reasoning::new("Thinking about this...");
    assert_eq!(reasoning.text, "Thinking about this...");
    assert!(reasoning.provider_metadata.is_none());
}

#[test]
fn test_reasoning_from_string() {
    let reasoning: LanguageModelV4Reasoning = "Thought".to_string().into();
    assert_eq!(reasoning.text, "Thought");
}

#[test]
fn test_reasoning_serialization() {
    let reasoning = LanguageModelV4Reasoning::new("Test reasoning");
    let json = serde_json::to_string(&reasoning).unwrap();
    assert!(json.contains(r#""text":"Test reasoning"#));
}
