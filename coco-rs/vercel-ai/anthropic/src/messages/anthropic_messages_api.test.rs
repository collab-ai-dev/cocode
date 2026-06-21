use super::*;

#[test]
fn deserializes_text_content_block() {
    let json = r#"{"type":"text","text":"Hello, world!"}"#;
    let block: AnthropicResponseContentBlock =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match block {
        AnthropicResponseContentBlock::Text { text, citations } => {
            assert_eq!(text, "Hello, world!");
            assert!(citations.is_none());
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn deserializes_thinking_content_block() {
    let json = r#"{"type":"thinking","thinking":"Let me think...","signature":"abc123"}"#;
    let block: AnthropicResponseContentBlock =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match block {
        AnthropicResponseContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "Let me think...");
            assert_eq!(signature, "abc123");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn deserializes_tool_use_content_block() {
    let json = r#"{"type":"tool_use","id":"tu_1","name":"get_weather","input":{"city":"SF"}}"#;
    let block: AnthropicResponseContentBlock =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match block {
        AnthropicResponseContentBlock::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "tu_1");
            assert_eq!(name, "get_weather");
            assert_eq!(input["city"], "SF");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn deserializes_usage() {
    let json = r#"{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}"#;
    let usage: AnthropicUsage = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_creation_input_tokens, Some(10));
    assert_eq!(usage.cache_read_input_tokens, Some(20));
}

#[test]
fn deserializes_full_response() {
    let json = r#"{
        "id": "msg_123",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "text", "text": "Hello!"}
        ],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {"input_tokens": 10, "output_tokens": 5}
    }"#;
    let resp: AnthropicMessagesResponse =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(resp.id.as_deref(), Some("msg_123"));
    assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(resp.content.len(), 1);
}

#[test]
fn deserializes_citation() {
    let json = r#"{"type":"web_search_result_location","cited_text":"some text","url":"https://example.com","title":"Example","encrypted_index":"enc123"}"#;
    let citation: AnthropicCitation = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match citation {
        AnthropicCitation::WebSearchResultLocation { url, title, .. } => {
            assert_eq!(url, "https://example.com");
            assert_eq!(title, "Example");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn deserializes_text_delta() {
    let json = r#"{"type":"text_delta","text":"Hello"}"#;
    let delta: ContentBlockDelta = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match delta {
        ContentBlockDelta::TextDelta { text } => assert_eq!(text, "Hello"),
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn deserializes_content_block_start_tool_use() {
    let json = r#"{"type":"tool_use","id":"tu_1","name":"get_weather","input":{"city":"London"}}"#;
    let block: ContentBlockStart = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    match block {
        ContentBlockStart::ToolUse { id, name, .. } => {
            assert_eq!(id, "tu_1");
            assert_eq!(name, "get_weather");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}
