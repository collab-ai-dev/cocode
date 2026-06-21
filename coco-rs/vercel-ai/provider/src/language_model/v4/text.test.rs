use super::*;

#[test]
fn test_text_new() {
    let text = LanguageModelV4Text::new("Hello, world!");
    assert_eq!(text.text, "Hello, world!");
    assert!(text.provider_metadata.is_none());
}

#[test]
fn test_text_from_string() {
    let text: LanguageModelV4Text = "Hello".to_string().into();
    assert_eq!(text.text, "Hello");
}

#[test]
fn test_text_from_str() {
    let text: LanguageModelV4Text = "Hello".into();
    assert_eq!(text.text, "Hello");
}

#[test]
fn test_text_serialization() {
    let text = LanguageModelV4Text::new("Test");
    let json = serde_json::to_string(&text).unwrap();
    assert!(json.contains(r#""text":"Test"#));
}
