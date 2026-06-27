use super::*;
use serde_json::json;

#[test]
fn test_provider_tool_new() {
    let tool = LanguageModelV4ProviderTool::new("anthropic", "code_interpreter");

    assert_eq!(tool.id, "anthropic.code_interpreter");
    assert_eq!(tool.name, "code_interpreter");
    assert!(tool.args.is_empty());
}

#[test]
fn test_provider_tool_from_id() {
    let tool = LanguageModelV4ProviderTool::from_id("openai.file_search", "file_search");

    assert_eq!(tool.id, "openai.file_search");
    assert_eq!(tool.name, "file_search");
}

#[test]
fn test_provider_tool_with_args() {
    let tool = LanguageModelV4ProviderTool::new("anthropic", "code_interpreter")
        .with_arg("language", json!("python"))
        .with_arg("timeout", json!(30));

    assert_eq!(tool.args.len(), 2);
    assert_eq!(tool.args.get("language").unwrap(), &json!("python"));
    assert_eq!(tool.args.get("timeout").unwrap(), &json!(30));
}

#[test]
fn test_provider_tool_serialization() {
    let tool = LanguageModelV4ProviderTool::new("anthropic", "code_interpreter")
        .with_arg("enabled", json!(true));

    let json_str = serde_json::to_string(&tool).unwrap();
    assert!(json_str.contains("\"id\":\"anthropic.code_interpreter\""));
    assert!(json_str.contains("\"name\":\"code_interpreter\""));

    // When wrapped in LanguageModelV4Tool, the "type" tag is included
    let wrapped = super::super::tool::LanguageModelV4Tool::provider(tool);
    let json_str = serde_json::to_string(&wrapped).unwrap();
    assert!(json_str.contains("\"type\":\"provider\""));
}

#[test]
fn test_provider_tool_deserialization() {
    // Deserialize via LanguageModelV4Tool enum (which handles the "type" tag)
    let json = r#"{
        "type": "provider",
        "id": "test.my_tool",
        "name": "my_tool",
        "args": {"key": "value"}
    }"#;

    let tool: super::super::tool::LanguageModelV4Tool = serde_json::from_str(json).unwrap();
    match tool {
        super::super::tool::LanguageModelV4Tool::Provider(t) => {
            assert_eq!(t.id, "test.my_tool");
            assert_eq!(t.name, "my_tool");
            assert_eq!(t.args.get("key").unwrap(), &json!("value"));
        }
        _ => panic!("Expected Provider variant"),
    }
}

#[test]
fn test_provider_tool_standalone_deserialization() {
    // Standalone deserialization (without "type" tag)
    let json = r#"{
        "id": "test.my_tool",
        "name": "my_tool",
        "args": {"key": "value"}
    }"#;

    let tool: LanguageModelV4ProviderTool = serde_json::from_str(json).unwrap();
    assert_eq!(tool.id, "test.my_tool");
    assert_eq!(tool.name, "my_tool");
    assert_eq!(tool.args.get("key").unwrap(), &json!("value"));
}
