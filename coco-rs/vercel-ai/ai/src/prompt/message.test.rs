use serde_json::json;

use super::*;
use crate::prompt::PromptReasoningPart;
use crate::prompt::PromptToolCallPart;
use crate::prompt::PromptToolResultOutput;
use crate::prompt::PromptToolResultPart;

#[test]
fn test_system_message_serde_roundtrip() {
    let msg = PromptMessage::System(PromptSystemMessage::new("Be helpful"));
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "system");
    assert_eq!(json["content"], "Be helpful");

    let deserialized: PromptMessage = serde_json::from_value(json).unwrap();
    if let PromptMessage::System(sys) = deserialized {
        assert_eq!(sys.content, "Be helpful");
    } else {
        panic!("Expected System message");
    }
}

#[test]
fn test_user_text_message_serde_roundtrip() {
    let msg = PromptMessage::User(PromptUserMessage::text("Hello"));
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "Hello");

    let deserialized: PromptMessage = serde_json::from_value(json).unwrap();
    if let PromptMessage::User(user) = deserialized {
        assert!(matches!(user.content, PromptUserContent::Text(t) if t == "Hello"));
    } else {
        panic!("Expected User message");
    }
}

#[test]
fn test_user_parts_message() {
    let parts = vec![PromptUserContentPart::Text {
        text: "Hello".to_string(),
        provider_options: None,
    }];
    let msg = PromptUserMessage::parts(parts);
    assert!(matches!(msg.content, PromptUserContent::Parts(_)));
}

#[test]
fn test_assistant_text_message_serde_roundtrip() {
    let msg = PromptMessage::Assistant(PromptAssistantMessage::text("Response"));
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"], "Response");

    let deserialized: PromptMessage = serde_json::from_value(json).unwrap();
    if let PromptMessage::Assistant(asst) = deserialized {
        assert!(matches!(asst.content, PromptAssistantContent::Text(t) if t == "Response"));
    } else {
        panic!("Expected Assistant message");
    }
}

#[test]
fn test_assistant_parts_with_tool_call() {
    let parts = vec![PromptAssistantContentPart::ToolCall(
        PromptToolCallPart::new("tc1", "my_tool", json!({"arg": "value"})),
    )];
    let msg = PromptAssistantMessage::parts(parts);
    if let PromptAssistantContent::Parts(parts) = msg.content {
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], PromptAssistantContentPart::ToolCall(_)));
    } else {
        panic!("Expected Parts content");
    }
}

#[test]
fn test_assistant_parts_with_reasoning() {
    let parts = vec![PromptAssistantContentPart::Reasoning(
        PromptReasoningPart::new("Let me think..."),
    )];
    let msg = PromptAssistantMessage::parts(parts);
    if let PromptAssistantContent::Parts(parts) = msg.content {
        assert!(matches!(parts[0], PromptAssistantContentPart::Reasoning(_)));
    } else {
        panic!("Expected Parts content");
    }
}

#[test]
fn test_tool_message_serialization() {
    let tool_result = PromptToolResultPart::new(
        "tc1",
        "my_tool",
        PromptToolResultOutput::Text {
            value: "result".to_string(),
        },
    );
    let msg = PromptMessage::Tool(PromptToolMessage::new(vec![
        PromptToolContentPart::ToolResult(tool_result),
    ]));

    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "tool");
    assert!(json["content"].is_array());
    assert_eq!(json["content"][0]["type"], "tool-result");
    assert_eq!(json["content"][0]["tool_call_id"], "tc1");
    assert_eq!(json["content"][0]["tool_name"], "my_tool");
}

#[test]
fn test_tool_message_construction() {
    let tool_result = PromptToolResultPart::new(
        "tc1",
        "my_tool",
        PromptToolResultOutput::Text {
            value: "result".to_string(),
        },
    );
    let msg = PromptToolMessage::new(vec![PromptToolContentPart::ToolResult(tool_result)]);
    assert_eq!(msg.content.len(), 1);
    assert!(msg.provider_options.is_none());
    assert!(matches!(
        msg.content[0],
        PromptToolContentPart::ToolResult(_)
    ));
}
