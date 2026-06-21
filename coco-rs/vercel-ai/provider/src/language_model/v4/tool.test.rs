use super::*;

#[test]
fn test_tool_function_variant() {
    let tool = LanguageModelV4Tool::function(LanguageModelV4FunctionTool::new(
        "test",
        serde_json::json!({"type": "object"}),
    ));
    assert!(tool.is_function());
    assert_eq!(tool.name(), "test");
}

#[test]
fn test_tool_serde_roundtrip() {
    let tool = LanguageModelV4Tool::function(LanguageModelV4FunctionTool::new(
        "test",
        serde_json::json!({"type": "object"}),
    ));
    let json = serde_json::to_value(&tool).unwrap();
    assert_eq!(json["type"], "function");
    assert_eq!(json["name"], "test");

    let parsed: LanguageModelV4Tool = serde_json::from_value(json).unwrap();
    assert_eq!(tool, parsed);
}
