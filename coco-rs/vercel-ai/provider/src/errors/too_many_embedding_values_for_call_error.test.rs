use super::*;

#[test]
fn test_too_many_embedding_values_error_new() {
    let error =
        TooManyEmbeddingValuesForCallError::new("openai", "text-embedding-3-small", 2048, 3000);
    assert_eq!(error.provider, "openai");
    assert_eq!(error.model_id, "text-embedding-3-small");
    assert_eq!(error.max_embeddings_per_call, 2048);
    assert_eq!(error.values_count, 3000);
    assert!(error.message.contains("openai"));
    assert!(error.message.contains("text-embedding-3-small"));
    assert!(error.message.contains("2048"));
    assert!(error.message.contains("3000"));
}

#[test]
fn test_too_many_embedding_values_error_display() {
    let error = TooManyEmbeddingValuesForCallError::new("anthropic", "claude-embed", 100, 150);
    let display = format!("{error}");
    assert!(display.contains("Too many values"));
    assert!(display.contains("anthropic"));
}
