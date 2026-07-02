use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

use crate::openai_auth::OpenAIAuth;
use crate::openai_provider::OpenAIProvider;
use crate::openai_provider::OpenAIProviderSettings;

fn mock_provider(server: &MockServer) -> OpenAIProvider {
    OpenAIProvider::new(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".into())),
        ..Default::default()
    })
}

#[tokio::test]
async fn test_list_models_parses_data_array_with_context_window() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {"id": "gpt-5-5", "context_window": 272000},
                {"id": "gpt-5-4"}
            ]
        })))
        .mount(&server)
        .await;

    let provider = mock_provider(&server);
    let models = provider.list_models().await.expect("list ok");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-5-5");
    assert_eq!(models[0].context_window, Some(272_000));
    assert_eq!(models[1].id, "gpt-5-4");
    assert_eq!(models[1].context_window, None);
}

#[tokio::test]
async fn test_list_models_falls_back_to_models_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [ {"slug": "gpt-5-3-codex", "context_length": 200000} ]
        })))
        .mount(&server)
        .await;

    let provider = mock_provider(&server);
    let models = provider.list_models().await.expect("list ok");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-5-3-codex");
    assert_eq!(models[0].context_window, Some(200_000));
}

#[tokio::test]
async fn test_list_models_surfaces_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"message": "unauthorized", "type": "invalid_request_error"}
        })))
        .mount(&server)
        .await;

    let provider = mock_provider(&server);
    assert!(provider.list_models().await.is_err());
}
