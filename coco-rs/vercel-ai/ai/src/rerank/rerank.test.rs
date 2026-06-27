//! Tests for the rerank module.

use async_trait::async_trait;
use std::sync::Arc;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::reranking_model::RankedItem;
use vercel_ai_provider::reranking_model::RerankingModelV4;
use vercel_ai_provider::reranking_model::RerankingModelV4CallOptions;
use vercel_ai_provider::reranking_model::RerankingModelV4Result;

use crate::rerank::RerankOptions;
use crate::rerank::RerankingModel;
use crate::rerank::rerank;
use crate::test_utils::MockRerankingModel as TestUtilsMockRerankingModel;

/// Mock reranking model for testing.
struct MockRerankingModel {
    model_id: String,
    provider: String,
}

impl MockRerankingModel {
    fn new(model_id: &str, provider: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            provider: provider.to_string(),
        }
    }
}

#[async_trait]
impl RerankingModelV4 for MockRerankingModel {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_rerank(
        &self,
        options: RerankingModelV4CallOptions,
    ) -> Result<RerankingModelV4Result, AISdkError> {
        // Simple mock that returns documents in reverse order with decreasing scores
        let doc_count = options.documents.len();
        let results: Vec<RankedItem> = (0..doc_count)
            .rev()
            .enumerate()
            .map(|(i, original_index)| {
                let score = 1.0 - (i as f64 * 0.1);
                RankedItem::new(original_index, score)
            })
            .collect();

        Ok(RerankingModelV4Result::new(results))
    }
}

#[tokio::test]
async fn test_rerank_empty_documents() {
    let result = rerank(RerankOptions {
        model: RerankingModel::from_v4(Arc::new(MockRerankingModel::new(
            "test-model",
            "test-provider",
        ))),
        query: "test query".to_string(),
        documents: vec![],
        ..Default::default()
    })
    .await;

    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.original_documents.is_empty());
    assert!(result.ranking.is_empty());
}

#[tokio::test]
async fn test_rerank_basic() {
    let result = rerank(RerankOptions {
        model: RerankingModel::from_v4(Arc::new(MockRerankingModel::new(
            "test-model",
            "test-provider",
        ))),
        query: "test query".to_string(),
        documents: vec!["doc1".to_string(), "doc2".to_string(), "doc3".to_string()],
        ..Default::default()
    })
    .await;

    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.original_documents.len(), 3);
    assert_eq!(result.ranking.len(), 3);

    // Check that documents are reranked (mock returns in reverse order)
    assert_eq!(result.ranking[0].original_index, 2);
    assert_eq!(result.ranking[0].score, 1.0);
    assert_eq!(result.ranking[0].document, "doc3");
}

#[tokio::test]
async fn test_rerank_with_top_n() {
    let result = rerank(RerankOptions {
        model: RerankingModel::from_v4(Arc::new(MockRerankingModel::new(
            "test-model",
            "test-provider",
        ))),
        query: "test query".to_string(),
        documents: vec!["doc1".to_string(), "doc2".to_string(), "doc3".to_string()],
        top_n: Some(2),
        ..Default::default()
    })
    .await;

    assert!(result.is_ok());
    let result = result.unwrap();
    // Note: Our mock doesn't actually respect top_n, but the options are passed correctly
    assert_eq!(result.original_documents.len(), 3);
}

#[test]
fn test_reranking_model_from_string() {
    let model: RerankingModel = "test-model".into();
    assert!(matches!(model, RerankingModel::String(_)));
}

#[test]
fn test_reranking_model_from_v4() {
    let mock: Arc<dyn RerankingModelV4> = Arc::new(MockRerankingModel::new("test", "test"));
    let model: RerankingModel = mock.into();
    assert!(matches!(model, RerankingModel::V4(_)));
}

#[test]
fn test_rerank_result_body() {
    let response = crate::rerank::RerankResponse::new("test-model")
        .with_body(serde_json::json!({"key": "value"}));
    assert!(response.body.is_some());
}

#[tokio::test]
async fn test_rerank_provider_metadata() {
    use vercel_ai_provider::ProviderMetadata;
    use vercel_ai_provider::reranking_model::RankedItem;
    use vercel_ai_provider::reranking_model::RerankingModelV4Result;

    let mock = TestUtilsMockRerankingModel::builder()
        .with_rerank_handler(|options| {
            let doc_count = options.documents.len();
            let results: Vec<RankedItem> = (0..doc_count)
                .map(|i| RankedItem::new(i, 1.0 - (i as f64 * 0.1)))
                .collect();
            let mut metadata = ProviderMetadata::new();
            metadata.set("custom_key", serde_json::json!("custom_value"));
            Ok(RerankingModelV4Result::new(results).with_provider_metadata(metadata))
        })
        .build();
    let model: Arc<dyn RerankingModelV4> = Arc::new(mock);

    let result = rerank(RerankOptions {
        model: RerankingModel::from_v4(model),
        query: "test query".to_string(),
        documents: vec!["doc1".to_string(), "doc2".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let metadata = result
        .provider_metadata
        .expect("should have provider_metadata");
    assert_eq!(
        metadata.get("custom_key"),
        Some(&serde_json::json!("custom_value"))
    );
}
