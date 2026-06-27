//! Google Generative AI embedding model options.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Embedding task type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmbeddingTaskType {
    RetrievalQuery,
    RetrievalDocument,
    SemanticSimilarity,
    Classification,
    Clustering,
    QuestionAnswering,
    FactVerification,
    CodeRetrievalQuery,
}

/// Google embedding model options (provider-specific).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleEmbeddingModelOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dimensionality: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_type: Option<EmbeddingTaskType>,
    /// Per-value multimodal content parts for embedding non-text content.
    /// Each entry corresponds to the embedding value at the same index and
    /// its parts are merged with the text value in the request.
    /// Use `null` for entries that are text-only.
    /// The array length must match the number of values being embedded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Option<Vec<Value>>>>,
}
