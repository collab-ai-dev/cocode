//! ByteDance video model configuration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the ByteDance video model.
pub struct ByteDanceVideoModelConfig {
    /// Provider identifier string.
    pub provider: String,
    /// Base URL for the API.
    pub base_url: String,
    /// Function to generate request headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
    /// Poll interval for task status (default: 3 seconds).
    pub poll_interval: Option<Duration>,
    /// Maximum polling timeout (default: 300 seconds).
    pub poll_timeout: Option<Duration>,
}
