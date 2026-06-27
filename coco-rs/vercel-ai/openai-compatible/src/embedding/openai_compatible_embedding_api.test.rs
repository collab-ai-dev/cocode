use super::*;

#[test]
fn deserialize_embedding_response() {
    let json = r#"{
        "data": [
            { "embedding": [0.1, 0.2, 0.3], "index": 0 }
        ],
        "model": "text-embedding-3-small",
        "usage": { "prompt_tokens": 5, "total_tokens": 5 }
    }"#;
    let resp: OpenAICompatibleEmbeddingResponse =
        serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.data.len(), 1);
    assert_eq!(resp.data[0].embedding.len(), 3);
    assert_eq!(resp.usage.as_ref().and_then(|u| u.prompt_tokens), Some(5));
}
