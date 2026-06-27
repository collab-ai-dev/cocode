use super::*;

fn make_config() -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.embedding".into(),
        base_url: "https://api.openai.com/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
        full_url: None,
        chatgpt_subscription: false,
        reasoning_store: Default::default(),
    })
}

#[test]
fn creates_model() {
    let model = OpenAIEmbeddingModel::new("text-embedding-3-small", make_config());
    assert_eq!(model.model_id(), "text-embedding-3-small");
    assert_eq!(model.provider(), "openai.embedding");
    assert_eq!(model.max_embeddings_per_call(), 2048);
    assert!(model.supports_parallel_calls());
}
