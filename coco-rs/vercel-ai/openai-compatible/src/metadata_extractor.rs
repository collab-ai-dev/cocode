use serde_json::Value;
use vercel_ai_provider::ProviderMetadata;

/// Trait for extracting provider-specific metadata from API responses.
///
/// Implementations can extract custom metadata from both streaming and
/// non-streaming responses, making it available to consumers as `ProviderMetadata`.
pub trait MetadataExtractor: Send + Sync {
    /// Extract metadata from a non-streaming response body.
    fn extract_metadata(&self, response: &Value) -> Option<ProviderMetadata>;

    /// Create a stream metadata extractor for processing streaming chunks.
    ///
    /// Returns `None` if this extractor does not support streaming metadata.
    fn create_stream_extractor(&self) -> Option<Box<dyn StreamMetadataExtractor>>;
}

/// Trait for extracting metadata from streaming response chunks.
///
/// Created by [`MetadataExtractor::create_stream_extractor`] and called
/// for each chunk in a streaming response.
pub trait StreamMetadataExtractor: Send {
    /// Process a single streaming chunk.
    fn process_chunk(&mut self, chunk: &Value);

    /// Build the final metadata after all chunks have been processed.
    fn build_metadata(&self) -> Option<ProviderMetadata>;
}
