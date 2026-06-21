use super::*;

#[test]
fn test_new_provider() {
    let config = LlmConfig {
        provider: "ollama".to_string(),
        model: "qwen3:0.6b".to_string(),
        base_url: Some("http://localhost:11434/v1".to_string()),
        ..Default::default()
    };

    let provider = OllamaLlmProvider::new(config);
    assert_eq!(provider.name(), "ollama");
    assert_eq!(provider.model, "qwen3:0.6b");
}

#[test]
fn test_default_model_override() {
    // When model is the OpenAI default, use Ollama default instead
    let config = LlmConfig {
        provider: "ollama".to_string(),
        model: "gpt-4o-mini".to_string(),
        ..Default::default()
    };

    let provider = OllamaLlmProvider::new(config);
    assert_eq!(provider.model, "qwen3:0.6b");
}

#[test]
fn test_endpoint() {
    let config = LlmConfig {
        base_url: Some("http://localhost:11434/v1".to_string()),
        ..Default::default()
    };

    let provider = OllamaLlmProvider::new(config);
    assert_eq!(
        provider.endpoint(),
        "http://localhost:11434/v1/chat/completions"
    );
}
