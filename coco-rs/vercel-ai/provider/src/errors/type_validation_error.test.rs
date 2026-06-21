use super::*;

#[test]
fn test_type_validation_context_new() {
    let ctx = TypeValidationContext::new();
    assert!(ctx.field.is_none());
    assert!(ctx.entity_name.is_none());
    assert!(ctx.entity_id.is_none());
}

#[test]
fn test_type_validation_context_builders() {
    let ctx = TypeValidationContext::new()
        .with_field("message.content")
        .with_entity_name("TextPart")
        .with_entity_id("msg-123");
    assert_eq!(ctx.field, Some("message.content".to_string()));
    assert_eq!(ctx.entity_name, Some("TextPart".to_string()));
    assert_eq!(ctx.entity_id, Some("msg-123".to_string()));
}

#[test]
fn test_type_validation_error_new() {
    let value = serde_json::json!({"invalid": true});
    let cause = std::io::Error::other("validation failed");
    let error = TypeValidationError::new(value.clone(), Box::new(cause));
    assert_eq!(error.value, value);
    assert!(error.context.is_none());
    assert!(error.message.contains("Type validation failed"));
}

#[test]
fn test_type_validation_error_with_context() {
    let value = serde_json::json!({"foo": "bar"});
    let cause = std::io::Error::other("bad type");
    let ctx = TypeValidationContext::new()
        .with_field("parts[3]")
        .with_entity_name("ToolCall");
    let error = TypeValidationError::with_context(value.clone(), Box::new(cause), ctx);
    assert_eq!(error.value, value);
    assert!(error.context.is_some());
    let ctx = error.context.unwrap();
    assert_eq!(ctx.field, Some("parts[3]".to_string()));
    assert_eq!(ctx.entity_name, Some("ToolCall".to_string()));
    assert!(error.message.contains("parts[3]"));
}

#[test]
fn test_type_validation_error_display() {
    let value = serde_json::json!({"test": 123});
    let cause = std::io::Error::other("error");
    let error = TypeValidationError::new(value, Box::new(cause));
    let display = format!("{error}");
    assert!(display.contains("Type validation failed"));
}
