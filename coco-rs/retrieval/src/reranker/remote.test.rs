use super::*;
use crate::config::RerankerProvider;

#[test]
fn test_get_endpoint() {
    let config = RemoteRerankerConfig {
        provider: RerankerProvider::Cohere,
        model: "rerank-english-v3.0".to_string(),
        api_key_env: "COHERE_API_KEY".to_string(),
        base_url: None,
        timeout_secs: 10,
        max_retries: 2,
        top_n: None,
    };

    let reranker = RemoteReranker::new(&config).unwrap();
    assert_eq!(reranker.get_endpoint(), "https://api.cohere.ai/v1/rerank");
}

#[test]
fn test_custom_base_url() {
    let config = RemoteRerankerConfig {
        provider: RerankerProvider::Custom,
        model: "custom-model".to_string(),
        api_key_env: "CUSTOM_API_KEY".to_string(),
        base_url: Some("https://custom.api.com/rerank".to_string()),
        timeout_secs: 10,
        max_retries: 2,
        top_n: None,
    };

    let reranker = RemoteReranker::new(&config).unwrap();
    assert_eq!(reranker.get_endpoint(), "https://custom.api.com/rerank");
}
