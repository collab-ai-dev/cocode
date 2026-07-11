use std::collections::HashMap;
use std::sync::Arc;

/// Shared configuration passed to each xAI model instance.
///
/// Mirrors the `XaiChatConfig` type from `@ai-sdk/xai`, adapted to coco-rs's
/// transport (a shared `reqwest::Client` instead of a pluggable `fetch`).
pub struct XaiConfig {
    /// Provider identifier (e.g., "xai.chat").
    pub provider: String,
    /// Base URL for the API (e.g., "https://api.x.ai/v1").
    pub base_url: String,
    /// Lazy header supplier — called per-request to get auth + custom headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

impl XaiConfig {
    /// Build a full URL from a path segment (e.g., "/chat/completions").
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Get the current headers by invoking the lazy supplier.
    pub fn get_headers(&self) -> HashMap<String, String> {
        (self.headers)()
    }
}
