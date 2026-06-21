use super::*;

fn make_config() -> Arc<OpenAICompatibleConfig> {
    Arc::new(OpenAICompatibleConfig {
        provider: "xai.embedding".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        query_params: None,
        client: None,
        include_usage: true,
        supports_structured_outputs: false,
        transform_request_body: None,
        metadata_extractor: None,
        supported_urls: None,
        error_handler: Arc::new(
            crate::openai_compatible_error::OpenAICompatibleFailedResponseHandler::new("xai"),
        ),
        prompt_tokens_total_semantics:
            crate::provider_options::PromptTokensTotalSemantics::Inclusive,
        provider_profile: crate::OpenAICompatibleProviderProfile::Generic,
        full_url: None,
    })
}

#[test]
fn creates_model() {
    let model = OpenAICompatibleEmbeddingModel::new("text-embedding-3-small", make_config());
    assert_eq!(model.model_id(), "text-embedding-3-small");
    assert_eq!(model.provider(), "xai.embedding");
    assert_eq!(model.max_embeddings_per_call(), 2048);
    assert!(model.supports_parallel_calls());
}
