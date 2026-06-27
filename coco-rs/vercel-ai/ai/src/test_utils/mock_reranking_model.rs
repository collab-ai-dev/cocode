//! Mock reranking model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::reranking_model::RankedItem;
use vercel_ai_provider::reranking_model::RerankingModelV4;
use vercel_ai_provider::reranking_model::RerankingModelV4CallOptions;
use vercel_ai_provider::reranking_model::RerankingModelV4Result;

type RerankHandler = Arc<
    dyn Fn(RerankingModelV4CallOptions) -> Result<RerankingModelV4Result, AISdkError> + Send + Sync,
>;

/// A configurable mock reranking model for testing.
pub struct MockRerankingModel {
    provider_name: String,
    model_id: String,
    rerank_handler: Option<RerankHandler>,
    call_log: Arc<Mutex<Vec<RerankingModelV4CallOptions>>>,
}

impl MockRerankingModel {
    /// Create a builder for a mock reranking model.
    pub fn builder() -> MockRerankingModelBuilder {
        MockRerankingModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<RerankingModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl RerankingModelV4 for MockRerankingModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_rerank(
        &self,
        options: RerankingModelV4CallOptions,
    ) -> Result<RerankingModelV4Result, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.rerank_handler {
            handler(options)
        } else {
            // Default: return items in original order with descending scores
            let doc_count = options.documents.len();
            let results = (0..doc_count)
                .map(|i| {
                    let score = 1.0 - (i as f64 * 0.1);
                    RankedItem::new(i, score)
                })
                .collect();
            Ok(RerankingModelV4Result::new(results))
        }
    }
}

/// Builder for `MockRerankingModel`.
pub struct MockRerankingModelBuilder {
    provider_name: String,
    model_id: String,
    rerank_handler: Option<RerankHandler>,
}

impl MockRerankingModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-reranking".to_string(),
            rerank_handler: None,
        }
    }

    /// Set the provider name.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider_name = provider.into();
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Set a custom rerank handler.
    pub fn with_rerank_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(RerankingModelV4CallOptions) -> Result<RerankingModelV4Result, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.rerank_handler = Some(Arc::new(handler));
        self
    }

    /// Set a handler that returns an error.
    pub fn with_error(self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.with_rerank_handler(move |_| Err(AISdkError::new(&error)))
    }

    /// Build the mock model.
    pub fn build(self) -> MockRerankingModel {
        MockRerankingModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            rerank_handler: self.rerank_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockRerankingModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
