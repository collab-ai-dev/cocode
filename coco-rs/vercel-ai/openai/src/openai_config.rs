use std::collections::HashMap;
use std::sync::Arc;

/// Provider-instance policy for the Responses API `store` field on **reasoning**
/// models when the caller doesn't pass an explicit `store`. A per-provider knob
/// (configured via `provider_options.reasoning_store`) — NOT a hardcoded global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
pub enum ResponsesStorePolicy {
    /// Omit `store` — the server keeps reasoning state, so chain-of-thought
    /// continuity works WITHOUT echoing `encrypted_content`. The conservative
    /// default for plain API keys (matches OpenAI's own server default).
    /// Config value: `"server"`.
    #[default]
    #[serde(rename = "server")]
    ServerDefault,
    /// Force `store: false` and auto-include `reasoning.encrypted_content`.
    /// Stateless / codex-aligned: continuity rides the echoed encrypted blob
    /// (which coco round-trips). ChatGPT-subscription providers always behave
    /// this way regardless of the policy (the codex backend requires it).
    /// Config value: `"stateless"`.
    #[serde(rename = "stateless")]
    Stateless,
}

/// Shared configuration passed to each OpenAI model instance.
pub struct OpenAIConfig {
    /// Provider identifier (e.g., "openai.chat", "openai.responses").
    pub provider: String,
    /// Base URL for the API (e.g., "https://api.openai.com/v1").
    pub base_url: String,
    /// Lazy header supplier — called per-request to get auth + custom headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
    /// When `true`, `base_url` is treated as the complete endpoint URL and
    /// no API path suffix (e.g., `/chat/completions`) is appended.
    /// Default (`None`/`false`): auto-detect — if `base_url` already ends
    /// with the path, appending is skipped automatically.
    pub full_url: Option<bool>,
    /// `true` when this provider authenticates via the ChatGPT subscription
    /// (codex backend). The Responses model defaults `store: false` in this
    /// mode (the codex backend requires it; it also unlocks the
    /// `reasoning.encrypted_content` include). Default `false`.
    pub chatgpt_subscription: bool,
    /// Policy for the Responses `store` field on reasoning models when the
    /// caller doesn't set one. See [`ResponsesStorePolicy`]. Defaults to
    /// `ServerDefault` (omit `store` — server-side reasoning state).
    pub reasoning_store: ResponsesStorePolicy,
}

impl OpenAIConfig {
    /// Build a full URL from a path segment (e.g., "/chat/completions").
    ///
    /// If `full_url` is set, or `base_url` already ends with the path,
    /// returns `base_url` as-is to avoid duplication.
    pub fn url(&self, path: &str) -> String {
        if self.full_url.unwrap_or(false) || self.base_url.ends_with(path) {
            self.base_url.clone()
        } else {
            format!("{}{path}", self.base_url)
        }
    }

    /// Get the current headers by invoking the lazy supplier.
    pub fn get_headers(&self) -> HashMap<String, String> {
        (self.headers)()
    }
}
