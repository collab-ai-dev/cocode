use super::*;

#[test]
fn test_assemble_operation_name_with_operation() {
    assert_eq!(
        assemble_operation_name("ai.generateText", Some("my-op")),
        "ai.generateText.my-op"
    );
}

#[test]
fn test_assemble_operation_name_without_operation() {
    assert_eq!(
        assemble_operation_name("ai.generateText", None),
        "ai.generateText"
    );
}

#[test]
fn test_assemble_operation_name_empty_operation() {
    assert_eq!(
        assemble_operation_name("ai.generateText", Some("")),
        "ai.generateText"
    );
}

#[test]
fn test_get_base_telemetry_attributes() {
    let attrs = get_base_telemetry_attributes("claude-3", "anthropic", None);
    assert_eq!(attrs.get("ai.model.id").unwrap(), "claude-3");
    assert_eq!(attrs.get("ai.model.provider").unwrap(), "anthropic");
}

#[test]
fn test_get_base_telemetry_attributes_with_metadata() {
    let mut meta = HashMap::new();
    meta.insert("user_id".to_string(), "123".to_string());
    let attrs = get_base_telemetry_attributes("gpt-4", "openai", Some(&meta));
    assert_eq!(attrs.get("ai.telemetry.metadata.user_id").unwrap(), "123");
}

#[test]
fn test_select_telemetry_attributes_filters_inputs() {
    let mut attrs = HashMap::new();
    attrs.insert("ai.model.id".to_string(), "test".to_string());
    attrs.insert("ai.prompt.text".to_string(), "secret".to_string());
    attrs.insert("ai.response.text".to_string(), "output".to_string());

    let filtered = select_telemetry_attributes(attrs, false, true);
    assert!(!filtered.contains_key("ai.prompt.text"));
    assert!(filtered.contains_key("ai.response.text"));
    assert!(filtered.contains_key("ai.model.id"));
}

#[test]
fn test_select_telemetry_attributes_filters_outputs() {
    let mut attrs = HashMap::new();
    attrs.insert("ai.model.id".to_string(), "test".to_string());
    attrs.insert("ai.prompt.text".to_string(), "input".to_string());
    attrs.insert("ai.output.text".to_string(), "secret".to_string());

    let filtered = select_telemetry_attributes(attrs, true, false);
    assert!(filtered.contains_key("ai.prompt.text"));
    assert!(!filtered.contains_key("ai.output.text"));
    assert!(filtered.contains_key("ai.model.id"));
}
