//! OpenAI authentication modes (the wire-contract owner).
//!
//! Two modes: a static API key (the historical default) and a **ChatGPT
//! subscription** OAuth mode that routes to the codex backend with a
//! per-request bearer + `ChatGPT-Account-ID` + `originator` headers. The
//! subscription bearer is read fresh per request from a synchronous supplier
//! closure (refreshed out-of-band by the caller) — there is no async auth path
//! in the provider, so the supplier must be cheap and non-blocking.

use std::borrow::Cow;
use std::sync::Arc;

/// Base URL for the ChatGPT-subscription (codex) Responses backend.
/// `OpenAIConfig::url("/responses")` composes `…/codex/responses` from this.
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Default `originator` header value the codex backend gates first-party
/// access on. Load-bearing client impersonation (analogous to Claude Code's
/// `claude-cli` User-Agent).
pub const DEFAULT_ORIGINATOR: &str = "codex_cli_rs";

/// HTTP header carrying the ChatGPT account id. Exact casing is load-bearing.
pub(crate) const HDR_CHATGPT_ACCOUNT_ID: &str = "ChatGPT-Account-ID";
/// HTTP header carrying the client originator.
pub(crate) const HDR_ORIGINATOR: &str = "originator";

/// Live ChatGPT-subscription credentials, supplied per request.
#[derive(Clone)]
pub struct ChatGptCreds {
    /// Current OAuth access token (Bearer).
    pub access_token: String,
    /// `chatgpt-account-id` claim from the id_token. Header omitted when `None`.
    pub account_id: Option<String>,
}

/// Per-request supplier of ChatGPT-subscription credentials. Returns `None`
/// when the user is not logged in (request then carries no `Authorization`,
/// surfacing a clear 401 upstream rather than a build-time failure).
pub type ChatGptCredsSupplier = Arc<dyn Fn() -> Option<ChatGptCreds> + Send + Sync>;

/// How the OpenAI provider authenticates each request.
#[derive(Clone)]
pub enum OpenAIAuth {
    /// Static API key; falls back to `OPENAI_API_KEY`. The default, historical
    /// behavior.
    ApiKey(Option<String>),
    /// ChatGPT-subscription OAuth. `Authorization: Bearer <access>` +
    /// `ChatGPT-Account-ID` (when present) + `originator`, read fresh per
    /// request from `creds`.
    ChatGptSubscription {
        creds: ChatGptCredsSupplier,
        originator: Cow<'static, str>,
    },
}

impl Default for OpenAIAuth {
    fn default() -> Self {
        Self::ApiKey(None)
    }
}

impl OpenAIAuth {
    /// Whether this is the ChatGPT-subscription mode (drives `store: false` +
    /// the codex base URL default in the provider).
    pub fn is_chatgpt_subscription(&self) -> bool {
        matches!(self, Self::ChatGptSubscription { .. })
    }
}
