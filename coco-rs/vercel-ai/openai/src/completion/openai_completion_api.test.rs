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
    let resp: OpenAICompletionResponse = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.choices[0].text.as_deref(), Some("Hello world"));
}

#[test]
fn deserialize_chunk_with_logprobs() {
    let json = r#"{
        "id": "cmpl-abc",
        "model": "gpt-3.5-turbo-instruct",
        "created": 1700000000,
        "choices": [{
            "text": "Hello",
            "index": 0,
            "finish_reason": null,
            "logprobs": {
                "tokens": ["Hello"],
                "token_logprobs": [-0.5]
            }
        }]
    }"#;
    let chunk: OpenAICompletionChunk = serde_json::from_str(json).expect("should deserialize");
    let choice = &chunk.choices.unwrap()[0];
    assert!(
        choice.logprobs.is_some(),
        "chunk choice should have logprobs"
    );
}

#[test]
fn deserialize_chunk_without_logprobs() {
    let json = r#"{
        "id": "cmpl-abc",
        "choices": [{
            "text": "Hello",
            "index": 0
        }]
    }"#;
    let chunk: OpenAICompletionChunk = serde_json::from_str(json).expect("should deserialize");
    let choice = &chunk.choices.unwrap()[0];
    assert!(
        choice.logprobs.is_none(),
        "chunk choice logprobs should be None when absent"
    );
}
