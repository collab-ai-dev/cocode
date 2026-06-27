use super::*;

#[test]
fn test_generate_object_options() {
    let schema = serde_json::json!({ "type": "object" });
    let options: GenerateObjectOptions<serde_json::Value> =
        GenerateObjectOptions::new("gpt-4", "Generate something", schema)
            .with_schema_name("test")
            .with_mode(ObjectGenerationMode::Json);

    assert!(options.model.is_string());
    assert_eq!(options.schema_name, Some("test".to_string()));
    assert_eq!(options.mode, ObjectGenerationMode::Json);
}

#[test]
fn test_parse_partial_json() {
    let partial = r#"{"name": "test"#;
    let result = crate::util::parse_partial_json(partial);
    assert!(result.is_some());

    let complete = r#"{"name": "test"}"#;
    let result = crate::util::parse_partial_json(complete);
    assert!(result.is_some());
}
