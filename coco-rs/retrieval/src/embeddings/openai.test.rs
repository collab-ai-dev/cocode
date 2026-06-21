use super::*;

#[test]
fn test_new() {
    let provider = OpenAIEmbeddings::new("test-key");
    assert_eq!(provider.dimension(), default_embedding_dimension());
    assert_eq!(provider.model, DEFAULT_MODEL);
}

#[test]
fn test_with_dimension() {
    let provider = OpenAIEmbeddings::new("test-key").with_dimension(512);
    assert_eq!(provider.dimension(), 512);
}

#[test]
fn test_with_model() {
    let provider = OpenAIEmbeddings::new("test-key").with_model("text-embedding-3-large");
    assert_eq!(provider.model, "text-embedding-3-large");
}

#[test]
fn test_with_base_url() {
    let provider = OpenAIEmbeddings::new("test-key").with_base_url("https://custom.api.com");
    assert_eq!(provider.base_url, "https://custom.api.com");
}
