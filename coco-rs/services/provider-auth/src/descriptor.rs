//! Data-driven OAuth flow descriptors. Each `OAuthFlowId` maps to exactly one
//! `OAuthFlowDescriptor`; the flow engine (`flow.rs`) is a `match` over the
//! composed strategy enums, so adding a provider is descriptor data + one wire
//! mode in the provider crate — no new engine code.
//!
//! Wired today: `OPENAI_CHATGPT` (loopback + Form exchange / JSON refresh,
//! rotating token, id_token account claim) and `GEMINI_CODE_ASSIST` (loopback +
//! Form exchange/refresh, persistent token, client_secret, userinfo email).
//! The strategy enums also carry variants reserved for Claude Max
//! (`VerifierAsState`, `LoopbackOrPaste`) so that flow slots in as pure data +
//! one engine arm later; until its `OAuthFlowId` lands, the engine returns
//! `Internal` for the unimplemented `LoopbackOrPaste` combination.

use coco_config::EnvKey;
use coco_types::OAuthFlowId;

/// How the authorization code is delivered back to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectStrategy {
    /// Bind a loopback listener; `redirect_uri` is derived from the bound port.
    Loopback {
        default_port: u16,
        fallback_port: Option<u16>,
        callback_path: &'static str,
    },
    /// Try loopback first; fall back to a hosted-paste redirect (Claude/Gemini).
    LoopbackOrPaste {
        default_port: u16,
        fallback_port: Option<u16>,
        callback_path: &'static str,
        paste_redirect_uri: &'static str,
    },
}

/// What goes in the `state` param and how it is validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateStrategy {
    /// Separate random CSRF state, validated against the callback. (OpenAI/Gemini)
    SeparateRandom,
    /// The PKCE verifier is reused as `state` (Claude).
    VerifierAsState,
}

/// Request-body encoding for token exchange / refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyEncoding {
    Form,
    Json,
}

/// Whether the refresh token rotates (single-use) or persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenRotation {
    /// New refresh token each refresh; old is single-use (OpenAI/Claude) — refresh
    /// must be serialized.
    Rotates,
    /// Refresh token persists; response may omit it → keep the old one (Gemini).
    Persists,
}

/// Where the durable account identifier comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountIdSource {
    /// JWT id_token claim path (OpenAI: `https://api.openai.com/auth` → `chatgpt_account_id`).
    IdTokenClaim { path: &'static [&'static str] },
    /// Fetch a string field from a userinfo endpoint with the access token
    /// (Google: `userinfo` → `email`). Populates the credential's `email`.
    UserInfoEndpoint {
        url: &'static str,
        field: &'static str,
    },
    /// None carried; account id is not used by this provider.
    None,
}

/// Fully describes one subscription OAuth flow as data.
#[derive(Debug, Clone, Copy)]
pub struct OAuthFlowDescriptor {
    pub flow: OAuthFlowId,
    pub display_name: &'static str,
    pub client_id: &'static str,
    /// OAuth client secret for desktop-app flows (Google). `None` for pure
    /// PKCE-public clients (OpenAI). Sent in the token exchange + refresh body.
    pub client_secret: Option<&'static str>,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    /// RFC 7009 revocation endpoint. `logout` best-effort POSTs the token here.
    pub revoke_url: Option<&'static str>,
    pub scope: &'static str,
    pub redirect: RedirectStrategy,
    pub state: StateStrategy,
    pub exchange_encoding: BodyEncoding,
    pub refresh_encoding: BodyEncoding,
    pub refresh_rotation: RefreshTokenRotation,
    /// Extra static authorize-URL query params.
    pub authorize_extra: &'static [(&'static str, &'static str)],
    /// Extra static refresh-body params (e.g. Claude `scope`).
    pub refresh_extra: &'static [(&'static str, &'static str)],
    pub account_id: AccountIdSource,
    /// Test/diagnostic override for the token endpoint host (wiremock seam).
    pub token_url_env: Option<EnvKey>,
    /// Test/diagnostic override for the revocation endpoint (wiremock seam).
    pub revoke_url_env: Option<EnvKey>,
}

/// The ChatGPT-subscription flow (the only one wired in P1).
pub const OPENAI_CHATGPT: OAuthFlowDescriptor = OAuthFlowDescriptor {
    flow: OAuthFlowId::OpenAiChatGpt,
    display_name: "ChatGPT subscription",
    client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
    client_secret: None,
    authorize_url: "https://auth.openai.com/oauth/authorize",
    token_url: "https://auth.openai.com/oauth/token",
    revoke_url: Some("https://auth.openai.com/oauth/revoke"),
    scope: "openid profile email offline_access api.connectors.read api.connectors.invoke",
    redirect: RedirectStrategy::Loopback {
        default_port: 1455,
        fallback_port: Some(1457),
        callback_path: "/auth/callback",
    },
    state: StateStrategy::SeparateRandom,
    exchange_encoding: BodyEncoding::Form,
    refresh_encoding: BodyEncoding::Json,
    refresh_rotation: RefreshTokenRotation::Rotates,
    authorize_extra: &[
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
    ],
    refresh_extra: &[],
    account_id: AccountIdSource::IdTokenClaim {
        path: &["https://api.openai.com/auth", "chatgpt_account_id"],
    },
    token_url_env: Some(EnvKey::CocoAuthOpenaiTokenUrl),
    revoke_url_env: Some(EnvKey::CocoAuthOpenaiRevokeUrl),
};

/// The Gemini Code Assist flow (Google account OAuth, desktop-app client).
/// Differs from OpenAI: carries a `client_secret`, refreshes form-encoded with a
/// **persistent** refresh token, and derives the account email from a userinfo
/// endpoint rather than a JWT claim. The public desktop client_id/secret are the
/// same ones the Gemini CLI / jcode embed.
pub const GEMINI_CODE_ASSIST: OAuthFlowDescriptor = OAuthFlowDescriptor {
    flow: OAuthFlowId::GeminiCodeAssist,
    display_name: "Gemini Code Assist",
    client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
    client_secret: Some("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"),
    authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    revoke_url: Some("https://oauth2.googleapis.com/revoke"),
    scope: "https://www.googleapis.com/auth/cloud-platform \
            https://www.googleapis.com/auth/userinfo.email \
            https://www.googleapis.com/auth/userinfo.profile",
    redirect: RedirectStrategy::Loopback {
        default_port: 0, // ephemeral — Google desktop clients accept any localhost port
        fallback_port: None,
        callback_path: "/oauth2callback",
    },
    state: StateStrategy::SeparateRandom,
    exchange_encoding: BodyEncoding::Form,
    refresh_encoding: BodyEncoding::Form,
    refresh_rotation: RefreshTokenRotation::Persists,
    authorize_extra: &[("access_type", "offline"), ("prompt", "consent")],
    refresh_extra: &[],
    account_id: AccountIdSource::UserInfoEndpoint {
        url: "https://www.googleapis.com/oauth2/v2/userinfo",
        field: "email",
    },
    token_url_env: Some(EnvKey::CocoAuthGeminiTokenUrl),
    revoke_url_env: Some(EnvKey::CocoAuthGeminiRevokeUrl),
};

/// Resolve the descriptor for a flow id. Returns `None` for flows whose
/// descriptor is not yet populated (Claude in P1).
pub fn descriptor_for(flow: OAuthFlowId) -> Option<&'static OAuthFlowDescriptor> {
    match flow {
        OAuthFlowId::OpenAiChatGpt => Some(&OPENAI_CHATGPT),
        OAuthFlowId::GeminiCodeAssist => Some(&GEMINI_CODE_ASSIST),
    }
}

impl OAuthFlowDescriptor {
    /// Effective token endpoint. The `COCO_AUTH_*_TOKEN_URL` override is a
    /// wiremock/diagnostic seam and is honored **only in debug builds** — a
    /// release binary cannot be redirected (and thus have its refresh token /
    /// client_secret exfiltrated) by setting an env var. When honored it logs
    /// loudly so it can't be active silently.
    pub fn effective_token_url(&self) -> String {
        if cfg!(debug_assertions)
            && let Some(key) = self.token_url_env
            && let Some(v) = coco_config::env::env_opt(key.as_str())
            && !v.trim().is_empty()
        {
            tracing::warn!(
                flow = %self.flow,
                env = key.as_str(),
                "using debug token-endpoint override (not for production)"
            );
            return v;
        }
        self.token_url.to_string()
    }

    /// Effective revocation endpoint (`None` when the flow has none). Honors a
    /// debug-only env override (wiremock seam), mirroring `effective_token_url`.
    pub fn effective_revoke_url(&self) -> Option<String> {
        if cfg!(debug_assertions)
            && let Some(key) = self.revoke_url_env
            && let Some(v) = coco_config::env::env_opt(key.as_str())
            && !v.trim().is_empty()
        {
            return Some(v);
        }
        self.revoke_url.map(str::to_string)
    }
}
