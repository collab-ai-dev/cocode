//! Mock embedding model for testing.

use std::sync::Arc;
use std::sync::Mutex;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4;
use vercel_ai_provider::EmbeddingModelV4CallOptions;
use vercel_ai_provider::EmbeddingModelV4EmbedResult;
use vercel_ai_provider::EmbeddingUsage;
use vercel_ai_provider::EmbeddingValue;

type EmbedHandler = Arc<
    dyn Fn(EmbeddingModelV4CallOptions) -> Result<EmbeddingModelV4EmbedResult, AISdkError>
        + Send
        + Sync,
>;

/// A configurable mock embedding model for testing.
pub struct MockEmbeddingModel {
    provider_name: String,
    model_id: String,
    max_embeddings_per_call: usize,
    supports_parallel: bool,
    embed_handler: Option<EmbedHandler>,
    call_log: Arc<Mutex<Vec<EmbeddingModelV4CallOptions>>>,
}

impl MockEmbeddingModel {
    /// Create a builder for a mock embedding model.
    pub fn builder() -> MockEmbeddingModelBuilder {
        MockEmbeddingModelBuilder::new()
    }

    /// Get the call log.
    pub fn calls(&self) -> Vec<EmbeddingModelV4CallOptions> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get the number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl EmbeddingModelV4 for MockEmbeddingModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_embeddings_per_call(&self) -> usize {
        self.max_embeddings_per_call
    }

    fn supports_parallel_calls(&self) -> bool {
        self.supports_parallel
    }

    async fn do_embed(
        &self,
        options: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4EmbedResult, AISdkError> {
        self.call_log.lock().unwrap().push(options.clone());

        if let Some(ref handler) = self.embed_handler {
            handler(options)
        } else {
            // Default: return zero vectors
            let count = options.values.len();
            let embeddings = (0..count)
                .map(|_| EmbeddingValue::Dense {
                    vector: vec![0.0; 3],
                })
                .collect();
            Ok(EmbeddingModelV4EmbedResult {
                embeddings,
                usage: EmbeddingUsage::new(count as u64),
                warnings: Vec::new(),
                provider_metadata: None,
                raw_response: None,
            })
        }
    }
}

/// Builder for `MockEmbeddingModel`.
pub struct MockEmbeddingModelBuilder {
    provider_name: String,
    model_id: String,
    max_embeddings_per_call: usize,
    supports_parallel: bool,
    embed_handler: Option<EmbedHandler>,
}

impl MockEmbeddingModelBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            provider_name: "mock".to_string(),
            model_id: "mock-embedding".to_string(),
            max_embeddings_per_call: 1,
            supports_parallel: true,
            embed_handler: None,
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

    /// Set the max embeddings per call.
    pub fn with_max_embeddings_per_call(mut self, max: usize) -> Self {
        self.max_embeddings_per_call = max;
        self
    }

    /// Set a custom embed handler.
    pub fn with_embed_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(EmbeddingModelV4CallOptions) -> Result<EmbeddingModelV4EmbedResult, AISdkError>
            + Send
            + Sync
            + 'static,
    {
        self.embed_handler = Some(Arc::new(handler));
        self
    }

    /// Set a handler that returns an error.
    pub fn with_error(self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.with_embed_handler(move |_| Err(AISdkError::new(&error)))
    }

    /// Build the mock model.
    pub fn build(self) -> MockEmbeddingModel {
        MockEmbeddingModel {
            provider_name: self.provider_name,
            model_id: self.model_id,
            max_embeddings_per_call: self.max_embeddings_per_call,
            supports_parallel: self.supports_parallel,
            embed_handler: self.embed_handler,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for MockEmbeddingModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}
