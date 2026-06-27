use super::*;

#[test]
fn model_id_and_provider() {
    let model = GoogleGenerativeAIEmbeddingModel::new(
        "text-embedding-004",
        GoogleGenerativeAIEmbeddingModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert_eq!(model.model_id(), "text-embedding-004");
    assert_eq!(model.provider(), "google.generative-ai");
    assert_eq!(model.max_embeddings_per_call(), 2048);
    assert!(model.supports_parallel_calls());
}

#[test]
fn single_embed_response_deserialization() {
    let json = r#"{"embedding": {"values": [0.1, 0.2, 0.3]}}"#;
    let response: GoogleEmbedContentResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.embedding.values, vec![0.1, 0.2, 0.3]);
}

#[test]
fn batch_embed_response_deserialization() {
    let json = r#"{"embeddings": [{"values": [0.1, 0.2]}, {"values": [0.3, 0.4]}]}"#;
    let response: GoogleBatchEmbedContentsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.embeddings.len(), 2);
    assert_eq!(response.embeddings[0].values, vec![0.1, 0.2]);
    assert_eq!(response.embeddings[1].values, vec![0.3, 0.4]);
}

#[test]
fn build_content_parts_text_only() {
    let parts = GoogleGenerativeAIEmbeddingModel::build_content_parts("hello", None);
    assert_eq!(parts, serde_json::json!([{ "text": "hello" }]));
}

#[test]
fn build_content_parts_with_multimodal() {
    let multimodal = vec![serde_json::json!({
        "inlineData": { "mimeType": "image/png", "data": "abc123" }
    })];
    let parts = GoogleGenerativeAIEmbeddingModel::build_content_parts("hello", Some(&multimodal));
    assert_eq!(
        parts,
        serde_json::json!([
            { "text": "hello" },
            { "inlineData": { "mimeType": "image/png", "data": "abc123" } }
        ])
    );
}

#[test]
fn build_content_parts_empty_text_with_multimodal() {
    let multimodal = vec![serde_json::json!({
        "inlineData": { "mimeType": "image/png", "data": "abc123" }
    })];
    let parts = GoogleGenerativeAIEmbeddingModel::build_content_parts("", Some(&multimodal));
    assert_eq!(
        parts,
        serde_json::json!([
            { "inlineData": { "mimeType": "image/png", "data": "abc123" } }
        ])
    );
}
