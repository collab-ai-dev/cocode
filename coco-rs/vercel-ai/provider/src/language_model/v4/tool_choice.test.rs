use super::*;

#[test]
fn test_tool_choice_auto() {
    let choice = LanguageModelV4ToolChoice::auto();
    assert!(matches!(choice, LanguageModelV4ToolChoice::Auto));
}

#[test]
fn test_tool_choice_none() {
    let choice = LanguageModelV4ToolChoice::none();
    assert!(matches!(choice, LanguageModelV4ToolChoice::None));
}

#[test]
fn test_tool_choice_required() {
    let choice = LanguageModelV4ToolChoice::required();
    assert!(matches!(choice, LanguageModelV4ToolChoice::Required));
}

#[test]
fn test_tool_choice_tool() {
    let choice = LanguageModelV4ToolChoice::tool("get_weather");
    match choice {
        LanguageModelV4ToolChoice::Tool { tool_name } => {
            assert_eq!(tool_name, "get_weather");
        }
        _ => panic!("Expected Tool variant"),
    }
}

#[test]
fn test_tool_choice_default() {
    let choice = LanguageModelV4ToolChoice::default();
    assert!(matches!(choice, LanguageModelV4ToolChoice::Auto));
}

#[test]
fn test_tool_choice_serialization() {
    let choice = LanguageModelV4ToolChoice::auto();
    let json = serde_json::to_string(&choice).unwrap();
    assert_eq!(json, r#"{"type":"auto"}"#);

    let choice = LanguageModelV4ToolChoice::tool("my_tool");
    let json = serde_json::to_string(&choice).unwrap();
    assert!(json.contains(r#""type":"tool"#));
    assert!(json.contains(r#""toolName":"my_tool"#));
}

#[test]
fn test_tool_choice_deserialization() {
    let choice: LanguageModelV4ToolChoice = serde_json::from_str(r#"{"type":"auto"}"#).unwrap();
    assert!(matches!(choice, LanguageModelV4ToolChoice::Auto));

    let choice: LanguageModelV4ToolChoice = serde_json::from_str(r#"{"type":"none"}"#).unwrap();
    assert!(matches!(choice, LanguageModelV4ToolChoice::None));

    let choice: LanguageModelV4ToolChoice =
        serde_json::from_str(r#"{"type":"tool","toolName":"search"}"#).unwrap();
    match choice {
        LanguageModelV4ToolChoice::Tool { tool_name } => {
            assert_eq!(tool_name, "search");
        }
        _ => panic!("Expected Tool variant"),
    }
}
