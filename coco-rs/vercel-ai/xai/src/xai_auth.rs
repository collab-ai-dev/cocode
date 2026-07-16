//! xAI authentication modes owned by the provider wire layer.

use std::sync::Arc;

/// Live Grok-subscription credentials, supplied per request so an out-of-band
/// refresh is visible without rebuilding the model.
#[derive(Clone)]
pub struct GrokCreds {
    pub access_token: String,
}

pub type GrokCredsSupplier = Arc<dyn Fn() -> Option<GrokCreds> + Send + Sync>;

/// How the xAI provider authenticates each request.
#[derive(Clone)]
pub enum XaiConnection {
    /// Static API key sent only to the explicitly configured xAI-compatible
    /// endpoint. The key falls back to `XAI_API_KEY` when absent.
    ApiKey {
        base_url: Option<String>,
        api_key: Option<String>,
    },
    /// Grok subscription token. Its endpoint is intentionally not configurable:
    /// binding the bearer to the production proxy prevents config-based token
    /// exfiltration.
    GrokSubscription { creds: GrokCredsSupplier },
}

impl Default for XaiConnection {
    fn default() -> Self {
        Self::ApiKey {
            base_url: None,
            api_key: None,
        }
    }
}
