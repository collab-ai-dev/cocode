use super::*;

#[test]
fn test_call_options_text() {
    let opts = RerankingModelV4CallOptions::new("test query", vec!["doc1".to_string()]);
    assert_eq!(opts.query, "test query");
    assert_eq!(opts.documents.len(), 1);
    assert!(!opts.documents.is_empty());
}

#[test]
fn test_call_options_with_documents_enum() {
    let docs = RerankDocuments::Object(vec![serde_json::json!({"title": "test"})]);
    let opts = RerankingModelV4CallOptions::with_documents("query", docs);
    assert_eq!(opts.documents.len(), 1);
}

#[test]
fn test_rerank_documents_from() {
    let text_docs: RerankDocuments = vec!["a".to_string(), "b".to_string()].into();
    assert_eq!(text_docs.len(), 2);

    let obj_docs: RerankDocuments = vec![serde_json::json!({"a": 1})].into();
    assert_eq!(obj_docs.len(), 1);

    let empty: RerankDocuments = RerankDocuments::default();
    assert!(empty.is_empty());
}

#[test]
fn test_ranked_item() {
    let item = RankedItem::new(2, 0.95);
    assert_eq!(item.index, 2);
    assert_eq!(item.relevance_score, 0.95);
}

#[test]
fn test_result_builders() {
    let items = vec![RankedItem::new(0, 1.0), RankedItem::new(1, 0.5)];
    let response = RerankingModelV4Response::default()
        .with_id("resp-1")
        .with_model_id("rerank-1")
        .with_body(serde_json::json!({"key": "val"}));

    let result = RerankingModelV4Result::new(items)
        .with_response(response)
        .with_warnings(vec![crate::shared::Warning::other("test")]);

    assert_eq!(result.results.len(), 2);
    assert!(result.response.is_some());
    assert!(result.warnings.is_some());
}

#[test]
fn test_reranking_usage() {
    let usage = RerankingUsage::new(100);
    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.total_tokens, 100);
}
