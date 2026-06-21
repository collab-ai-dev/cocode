//! Vertex RAG Store tool.

use serde_json::Value;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for Vertex RAG Store.
pub const VERTEX_RAG_STORE_TOOL_ID: &str = "google.vertex_rag_store";

/// Create a Vertex RAG Store provider tool.
pub fn google_vertex_rag_store() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(VERTEX_RAG_STORE_TOOL_ID, "vertex_rag_store")
}

/// Create a Vertex RAG Store provider tool with a corpus resource.
pub fn google_vertex_rag_store_with_corpus(
    rag_corpus_resource: impl Into<String>,
) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(VERTEX_RAG_STORE_TOOL_ID, "vertex_rag_store").with_arg(
        "ragCorpusResource",
        Value::String(rag_corpus_resource.into()),
    )
}
