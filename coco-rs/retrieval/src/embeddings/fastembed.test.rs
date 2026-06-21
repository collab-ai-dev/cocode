use super::*;

#[test]
fn test_parse_model_name() {
    // Test various name formats
    let (model, dim) = FastembedEmbeddingProvider::parse_model_name("nomic-embed-text-v1.5")
        .expect("should parse");
    assert_eq!(dim, 768);
    assert!(matches!(model, EmbeddingModel::NomicEmbedTextV15));

    let (model, dim) =
        FastembedEmbeddingProvider::parse_model_name("bge-small-en-v1.5").expect("should parse");
    assert_eq!(dim, 384);
    assert!(matches!(model, EmbeddingModel::BGESmallENV15));

    let (model, dim) =
        FastembedEmbeddingProvider::parse_model_name("all-MiniLM-L6-v2").expect("should parse");
    assert_eq!(dim, 384);
    assert!(matches!(model, EmbeddingModel::AllMiniLML6V2));
}

#[test]
fn test_parse_model_name_unknown() {
    let result = FastembedEmbeddingProvider::parse_model_name("unknown-model");
    assert!(result.is_err());
}
