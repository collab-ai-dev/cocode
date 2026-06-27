//! Tests for output_strategy.rs

use super::*;

#[test]
fn test_object_strategy() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let strategy = ObjectOutputStrategy::object(schema);

    assert!(matches!(strategy, ObjectOutputStrategy::Object { .. }));
    assert!(strategy.schema().is_some());
}

#[test]
fn test_array_strategy() {
    let item_schema = serde_json::json!({
        "type": "string"
    });
    let strategy = ObjectOutputStrategy::array(item_schema);

    assert!(matches!(strategy, ObjectOutputStrategy::Array { .. }));
    assert!(strategy.schema().is_some());
}

#[test]
fn test_enum_strategy() {
    let strategy = ObjectOutputStrategy::enum_values(vec![
        "red".to_string(),
        "green".to_string(),
        "blue".to_string(),
    ]);

    assert!(matches!(strategy, ObjectOutputStrategy::Enum { .. }));
    assert!(strategy.schema().is_none());
}

#[test]
fn test_to_response_format() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let strategy = ObjectOutputStrategy::object(schema);

    let format = strategy.to_response_format(Some("person"));
    assert!(format.is_some());
}
