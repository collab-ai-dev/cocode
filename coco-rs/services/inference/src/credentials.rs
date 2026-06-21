//! Provider-credential resolution seam (dependency inversion).
//!
//! `coco-inference` owns this trait + the coco-neutral [`SubscriptionCreds`]
//! carrier. The implementor (`coco-provider-auth`) depends on this crate; this
//! crate does **not** depend back — the auth service stays out of the inference
//! dependency graph and tests can use a fake resolver.
//!
//! The carrier is intentionally provider-neutral (no `vercel-ai-*` type crosses
//! the seam — that would trip `scripts/check-vercel-ai-seam.sh`). `model_factory`
//! (which already depends on `vercel-ai-openai`) adapts it into the provider
//! crate's wire-auth mode (`vercel_ai_openai::OpenAIAuth::ChatGptSubscription`).

use std::fmt;
use std::sync::Arc;

/// Live subscription credentials, supplied per request. Fields cover the union
/// of what subscription providers need; each provider's `model_factory` arm
/// reads only the fields its wire mode uses:
/// - OpenAI ChatGPT → `access_token` + `account_id` (→ `ChatGPT-Account-ID`).
/// - Anthropic (future) → `access_token` + `subscription_type` (cache-TTL).
/// - Gemini (future) → `access_token` + `project_id`.
///
/// `Debug` is hand-rolled to redact the bearer — this carrier crosses into
/// `model_factory` and the provider header closures, so a future
/// `tracing::debug!(?creds)` must not dump the token (matches the redacting
/// `Debug` on `coco-provider-auth`'s `TokenSnapshot` / `StoredCredential`).
#[derive(Clone, Default)]
pub struct SubscriptionCreds {
    pub access_token: String,
    pub account_id: Option<String>,
    pub subscription_type: Option<String>,
    pub project_id: Option<String>,
}

impl fmt::Debug for SubscriptionCreds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionCreds")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .field("subscription_type", &self.subscription_type)
            .field("project_id", &self.project_id)
            .finish()
    }
}

/// Per-request supplier of [`SubscriptionCreds`]. Returns `None` when the user
/// is not logged in for that provider (the wire mode then emits no
/// `Authorization`, surfacing a clear 401 rather than failing at build time).
pub type SubscriptionCredsSupplier = Arc<dyn Fn() -> Option<SubscriptionCreds> + Send + Sync>;

/// Future returned by [`ProviderCredentialResolver::refresh_now`]. Resolves to
/// `true` when a refresh was actually performed (token possibly updated),
/// `false` otherwise (not OAuth / not logged in / refresh failed). Boxed +
/// `'static` so it can be driven from the private client retry loop.
pub type RefreshFuture = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// A bound reactive-refresh callback for one provider instance, installed on a
/// private runtime slot so a 401/403 can force a token refresh + retry.
pub type RefreshHook = Arc<dyn Fn() -> RefreshFuture + Send + Sync>;

/// Resolves live subscription credentials for OAuth-backed provider instances.
/// Implemented by `coco-provider-auth`.
pub trait ProviderCredentialResolver: Send + Sync {
    /// Returns a live credential supplier for the named provider instance, or
    /// `None` when that provider is not OAuth-backed / not logged in.
    fn subscription_creds(&self, provider_name: &str) -> Option<SubscriptionCredsSupplier>;

    /// Force a token refresh for an OAuth provider instance — the reactive-401
    /// recovery path. The default is a no-op (`false`) so api-key resolvers and
    /// test doubles need not implement it. The returned future updates the live
    /// token cell out-of-band; the caller then retries, and the provider's sync
    /// header closure reads the fresh token.
    fn refresh_now(&self, provider_name: &str) -> RefreshFuture {
        let _ = provider_name;
        Box::pin(async { false })
    }
}
