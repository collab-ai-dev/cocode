//! Code Assist credentials supplied per request (Bearer OAuth).

use std::sync::Arc;

/// Live Gemini Code Assist credentials, read fresh per request from a
/// synchronous supplier (refreshed out-of-band by the caller, mirroring
/// `vercel_ai_openai::ChatGptCreds`).
#[derive(Clone)]
pub struct CodeAssistCreds {
    /// Current OAuth access token (Bearer).
    pub access_token: String,
    /// Pre-resolved GCP project id, if known. When `None`, the adapter
    /// discovers it lazily via the onboarding handshake and caches it.
    pub project_id: Option<String>,
}

/// Per-request supplier of Code Assist credentials. Returns `None` when the
/// user is not logged in (the request then carries no `Authorization`,
/// surfacing a clear 401 rather than a build-time failure).
pub type CodeAssistCredsSupplier = Arc<dyn Fn() -> Option<CodeAssistCreds> + Send + Sync>;
