use super::*;

#[test]
fn deserialize_chat_response() {
    let json = r#"{
        "id": "chatcmpl-abc",
        "model": "gpt-4o",
        "created": 1700000000,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello!"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    }"#;
    let resp: OpenAIChatResponse = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.id.as_deref(), Some("chatcmpl-abc"));
    assert_eq!(resp.choices.len(), 1);
    assert_eq!(resp.choices[0].message.content.as_deref(), Some("Hello!"));
    assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
}

#[test]
fn deserialize_tool_call_response() {
    let json = r#"{
        "id": "chatcmpl-def",
        "model": "gpt-4o",
        "created": 1700000001,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"SF\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 10,
            "total_tokens": 30
        }
    }"#;
    let resp: OpenAIChatResponse = serde_json::from_str(json).expect("should deserialize");
    let tc = &resp.choices[0]
        .message
        .tool_calls
        .as_ref()
        .expect("tool_calls")[0];
    assert_eq!(tc.id.as_deref(), Some("call_123"));
    assert_eq!(tc.function.name, "get_weather");
}

#[test]
fn deserialize_streaming_chunk() {
    let json = r#"{
        "id": "chatcmpl-ghi",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "content": "Hello"
            },
            "finish_reason": null
        }]
    }"#;
    let chunk: OpenAIChatChunk = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(
        chunk.choices.as_ref().expect("choices")[0]
            .delta
            .as_ref()
            .and_then(|d| d.content.as_deref()),
        Some("Hello")
    );
}
