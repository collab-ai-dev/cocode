use super::*;

#[test]
fn test_language_model_from_string() {
    let model: LanguageModel = "gpt-4".into();
    assert!(model.is_string());
    assert_eq!(model.as_string(), Some("gpt-4"));
    assert!(!model.is_v4());
}

#[test]
fn test_language_model_from_id() {
    let model = LanguageModel::from_id("claude-3-sonnet");
    assert!(model.is_string());
    assert_eq!(model.as_string(), Some("claude-3-sonnet"));
}
