use std::collections::HashMap;
use std::sync::Arc;

/// Shared configuration passed to each Groq model instance.
///
/// Mirrors the `GroqConfig` type from `@ai-sdk/groq`, adapted to coco-rs's
/// transport (a shared `reqwest::Client` instead of a pluggable `fetch`).
/// Error handling is owned by each model at its call site rather than shared
/// here, since the chat and transcription transports differ.
pub struct GroqConfig {
    /// Provider identifier (e.g., "groq.chat", "groq.transcription").
    pub provider: String,
    /// Base URL for the API (e.g., "https://api.groq.com/openai/v1").
    pub base_url: String,
    /// Lazy header supplier — called per-request to get auth + custom headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

impl GroqConfig {
    /// Build a full URL from a path segment (e.g., "/chat/completions").
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Get the current headers by invoking the lazy supplier.
    pub fn get_headers(&self) -> HashMap<String, String> {
        (self.headers)()
    }
}
