use super::*;

#[test]
fn deserialize_completion_response() {
    let json = r#"{
        "id": "cmpl-abc",
        "model": "gpt-3.5-turbo-instruct",
        "created": 1700000000,
        "choices": [{
            "text": "Hello world",
            "index": 0,
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "total_tokens": 8
        }
    }"#;
    let resp: OpenAICompatibleCompletionResponse =
        serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.choices[0].text.as_deref(), Some("Hello world"));
}
