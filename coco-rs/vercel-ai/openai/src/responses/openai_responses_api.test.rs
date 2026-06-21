use super::*;

#[test]
fn deserialize_responses_response() {
    let json = r#"{
        "id": "resp_abc",
        "model": "gpt-4o",
        "created_at": 1700000000,
        "output": [{
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "Hello!",
                "annotations": []
            }]
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5
        },
        "status": "completed"
    }"#;
    let resp: OpenAIResponsesResponse = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.id.as_deref(), Some("resp_abc"));
    assert_eq!(resp.output.len(), 1);
    match &resp.output[0] {
        ResponseOutputItem::Message { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("expected Message"),
    }
}

#[test]
fn deserialize_function_call_output() {
    let json = r#"{
        "id": "resp_def",
        "model": "gpt-4o",
        "output": [{
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_123",
            "name": "get_weather",
            "arguments": "{\"city\":\"SF\"}"
        }],
        "status": "completed"
    }"#;
    let resp: OpenAIResponsesResponse = serde_json::from_str(json).expect("should deserialize");
    match &resp.output[0] {
        ResponseOutputItem::FunctionCall {
            name, arguments, ..
        } => {
            assert_eq!(name.as_deref(), Some("get_weather"));
            assert_eq!(arguments.as_deref(), Some("{\"city\":\"SF\"}"));
        }
        _ => panic!("expected FunctionCall"),
    }
}

#[test]
fn deserialize_streaming_text_delta() {
    let json = r#"{"type":"response.output_text.delta","item_id":"item_1","delta":"Hello"}"#;
    let event: ResponsesStreamEvent = serde_json::from_str(json).expect("should deserialize");
    match event {
        ResponsesStreamEvent::OutputTextDelta { delta, .. } => {
            assert_eq!(delta.as_deref(), Some("Hello"));
        }
        _ => panic!("expected OutputTextDelta"),
    }
}

#[test]
fn deserialize_unknown_event() {
    let json = r#"{"type":"response.some_future_event","data":"whatever"}"#;
    let event: ResponsesStreamEvent = serde_json::from_str(json).expect("should deserialize");
    assert!(matches!(event, ResponsesStreamEvent::Unknown));
}
