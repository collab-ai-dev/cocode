//! Cross-App Access (XAA) / Enterprise Managed Authorization (SEP-990, ID-JAG).
//!
//! Obtains an MCP access token WITHOUT a browser consent screen by chaining:
//!   1. RFC 8693 Token Exchange at the IdP: `id_token` → ID-JAG
//!   2. RFC 7523 JWT Bearer Grant at the AS: ID-JAG → `access_token`
//!
//! [`perform_cross_app_access`] is the orchestrator: it discovers the MCP
//! server's PRM (RFC 9728) and the authorization server's metadata (RFC 8414),
//! then runs the two legs with the spec-correct parameters — `audience` is the
//! **AS issuer** (not a client id) and the leg-1 request carries the
//! `resource` (the MCP server URL). Leg 2 authenticates with
//! `client_secret_basic` by default (the SEP-990 conformance expectation),
//! falling back to `client_secret_post` only when the AS advertises post-only.
//!
//! Tokens are redacted from debug logs via [`redact_tokens`] — failing to do so
//! leaks credentials if a misbehaving AS echoes the `subject_token` in an error.
//!
//! HTTP/SSE MCP configs with `oauth.xaa` call this flow before connecting and
//! persist the result through `coco-rmcp-client` OAuth storage.

use base64::Engine as _;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use tracing::debug;
use tracing::warn;

use crate::xaa_idp_login::discover_authorization_server;
use crate::xaa_idp_login::discover_prm;

/// Default HTTP timeout for XAA requests.
pub const XAA_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// `urn:ietf:params:oauth:grant-type:token-exchange` (RFC 8693).
pub const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// `urn:ietf:params:oauth:grant-type:jwt-bearer` (RFC 7523).
pub const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// ID-JAG token type (`draft-ietf-oauth-identity-assertion-authz-grant`).
pub const ID_JAG_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:id-jag";

/// `urn:ietf:params:oauth:token-type:id_token` — id_token subject type.
pub const ID_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:id_token";

/// Client authentication method at the AS token endpoint (leg 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsAuthMethod {
    /// Base64 `Authorization: Basic` header (SEP-990 conformance default).
    ClientSecretBasic,
    /// `client_id` / `client_secret` in the form body.
    ClientSecretPost,
}

/// Static inputs to the XAA orchestrator.
#[derive(Debug, Clone)]
pub struct XaaConfig {
    /// Client ID registered at the MCP server's authorization server.
    pub client_id: String,
    /// Client secret for the MCP server's authorization server.
    pub client_secret: String,
    /// Client ID registered at the IdP (for the token-exchange request).
    pub idp_client_id: String,
    /// Optional IdP client secret (`client_secret_post`) — some IdPs require it.
    pub idp_client_secret: Option<String>,
    /// The user's OIDC `id_token` from IdP login.
    pub idp_id_token: String,
    /// IdP token endpoint (RFC 8693 token-exchange target).
    pub idp_token_endpoint: String,
    /// Optional scope requested in both legs.
    pub scope: Option<String>,
}

/// Parameters for the RFC 8693 token exchange at the IdP (leg 1).
#[derive(Debug, Clone)]
pub struct JwtGrantRequest {
    pub token_endpoint: String,
    /// The AS **issuer URL** (not a client id).
    pub audience: String,
    /// The MCP server resource URL (PRM `resource`).
    pub resource: String,
    pub id_token: String,
    /// IdP-registered client id.
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scope: Option<String>,
}

/// Parameters for the RFC 7523 jwt-bearer grant at the AS (leg 2).
#[derive(Debug, Clone)]
pub struct JwtBearerRequest {
    pub token_endpoint: String,
    pub assertion: String,
    /// AS-registered client id.
    pub client_id: String,
    pub client_secret: String,
    pub auth_method: AsAuthMethod,
    pub scope: Option<String>,
}

/// Successful XAA orchestration result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XaaResult {
    /// AS-issued access token (the bearer token for subsequent MCP requests).
    pub access_token: String,
    /// Token type — almost always `"Bearer"`.
    #[serde(default)]
    pub token_type: String,
    /// Seconds until `access_token` expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// AS issuer URL discovered via PRM. Callers persist this so refresh /
    /// revocation can locate the token endpoint (the MCP URL is not the AS URL).
    #[serde(default)]
    pub authorization_server_url: String,
}

/// Errors returned by the XAA exchange path.
#[derive(Debug, thiserror::Error)]
pub enum XaaError {
    #[error("http error: {0}")]
    Http(String),
    #[error("provider returned {status}: {body}")]
    Provider {
        status: u16,
        body: String,
        /// Whether callers should drop any cached id_token because the IdP
        /// deemed it invalid.
        should_clear_id_token: bool,
    },
    #[error("malformed response: {0}")]
    MalformedResponse(String),
    #[error("discovery failed: {0}")]
    Discovery(String),
    #[error("config error: {0}")]
    Config(String),
}

/// Build the leg-1 (token-exchange) form body. Pure — unit-tested.
fn build_token_exchange_form(req: &JwtGrantRequest) -> Vec<(&'static str, String)> {
    let mut form = vec![
        ("grant_type", TOKEN_EXCHANGE_GRANT.to_string()),
        ("requested_token_type", ID_JAG_TOKEN_TYPE.to_string()),
        ("audience", req.audience.clone()),
        ("resource", req.resource.clone()),
        ("subject_token", req.id_token.clone()),
        ("subject_token_type", ID_TOKEN_TYPE.to_string()),
        ("client_id", req.client_id.clone()),
    ];
    if let Some(secret) = &req.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    if let Some(scope) = &req.scope {
        form.push(("scope", scope.clone()));
    }
    form
}

/// RFC 8693 Token Exchange at the IdP: `id_token` → ID-JAG (leg 1).
pub async fn request_jwt_authorization_grant(
    client: &Client,
    req: JwtGrantRequest,
) -> Result<String, XaaError> {
    let form = build_token_exchange_form(&req);

    debug!(endpoint = %req.token_endpoint, "xaa: leg 1 (id_token -> ID-JAG)");
    let response = client
        .post(&req.token_endpoint)
        .timeout(XAA_REQUEST_TIMEOUT)
        .form(&form)
        .send()
        .await
        .map_err(|e| XaaError::Http(redact_tokens(&e.to_string())))?;

    let status = response.status();
    let body_text = response
        .text()
        .await
        .map_err(|e| XaaError::Http(redact_tokens(&e.to_string())))?;

    if !status.is_success() {
        let should_clear = should_clear_id_token_on_status(status.as_u16(), &body_text);
        warn!(
            status = status.as_u16(),
            clear_id = should_clear,
            "xaa: IdP token exchange failed"
        );
        return Err(XaaError::Provider {
            status: status.as_u16(),
            body: redact_tokens(&body_text),
            should_clear_id_token: should_clear,
        });
    }

    let parsed: TokenExchangeResponse = serde_json::from_str(&body_text).map_err(|e| {
        // A protocol violation with a 2xx — safer to drop the cached id_token
        // so we don't keep hitting a broken flow.
        XaaError::MalformedResponse(format!("invalid JSON in 200 body: {e}"))
    })?;

    if parsed.issued_token_type.as_deref() != Some(ID_JAG_TOKEN_TYPE) {
        return Err(XaaError::MalformedResponse(format!(
            "expected issued_token_type={ID_JAG_TOKEN_TYPE}, got {:?}",
            parsed.issued_token_type
        )));
    }
    if parsed.access_token.is_empty() {
        return Err(XaaError::MalformedResponse("empty access_token".into()));
    }
    Ok(parsed.access_token)
}

/// `Authorization: Basic` value for `client_secret_basic`:
/// Base64 of `encodeURIComponent(id):encodeURIComponent(secret)`.
fn basic_auth_header(client_id: &str, client_secret: &str) -> String {
    let raw = format!(
        "{}:{}",
        urlencoding::encode(client_id),
        urlencoding::encode(client_secret)
    );
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

/// Build the leg-2 request: optional `Authorization` header (basic) + form
/// body. Pure — unit-tested.
fn build_jwt_bearer(req: &JwtBearerRequest) -> (Option<String>, Vec<(&'static str, String)>) {
    let mut form = vec![
        ("grant_type", JWT_BEARER_GRANT.to_string()),
        ("assertion", req.assertion.clone()),
    ];
    if let Some(scope) = &req.scope {
        form.push(("scope", scope.clone()));
    }
    match req.auth_method {
        AsAuthMethod::ClientSecretBasic => (
            Some(basic_auth_header(&req.client_id, &req.client_secret)),
            form,
        ),
        AsAuthMethod::ClientSecretPost => {
            form.push(("client_id", req.client_id.clone()));
            form.push(("client_secret", req.client_secret.clone()));
            (None, form)
        }
    }
}

/// RFC 7523 JWT Bearer Grant at the AS: ID-JAG → `access_token` (leg 2).
pub async fn exchange_jwt_auth_grant(
    client: &Client,
    req: JwtBearerRequest,
) -> Result<XaaResult, XaaError> {
    let (auth_header, form) = build_jwt_bearer(&req);

    debug!(endpoint = %req.token_endpoint, "xaa: leg 2 (ID-JAG -> access_token)");
    let mut builder = client
        .post(&req.token_endpoint)
        .timeout(XAA_REQUEST_TIMEOUT)
        .form(&form);
    if let Some(header) = auth_header {
        builder = builder.header(reqwest::header::AUTHORIZATION, header);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| XaaError::Http(redact_tokens(&e.to_string())))?;

    let status = response.status();
    let body_text = response
        .text()
        .await
        .map_err(|e| XaaError::Http(redact_tokens(&e.to_string())))?;

    if !status.is_success() {
        return Err(XaaError::Provider {
            status: status.as_u16(),
            body: redact_tokens(&body_text),
            // A 4xx/5xx on the AS leg does NOT invalidate the id_token; only
            // the IdP owns that decision.
            should_clear_id_token: false,
        });
    }

    let parsed: AccessTokenResponse = serde_json::from_str(&body_text)
        .map_err(|e| XaaError::MalformedResponse(format!("invalid JSON: {e}")))?;

    if parsed.access_token.is_empty() {
        return Err(XaaError::MalformedResponse("empty access_token".into()));
    }
    Ok(XaaResult {
        access_token: parsed.access_token,
        token_type: parsed.token_type.unwrap_or_else(|| "Bearer".into()),
        expires_in: parsed.expires_in,
        scope: parsed.scope,
        refresh_token: parsed.refresh_token,
        authorization_server_url: String::new(),
    })
}

/// Full XAA flow: PRM → AS metadata → token-exchange → jwt-bearer →
/// `access_token`.
pub async fn perform_cross_app_access(
    client: &Client,
    server_url: &str,
    config: &XaaConfig,
) -> Result<XaaResult, XaaError> {
    debug!(server_url, "xaa: discovering PRM");
    let prm = discover_prm(client, server_url)
        .await
        .map_err(|e| XaaError::Discovery(format!("PRM discovery failed: {e}")))?;

    // RFC 9728 §3.3 mix-up protection: advertised resource must match the URL.
    if let Some(resource) = &prm.resource
        && resource.trim_end_matches('/') != server_url.trim_end_matches('/')
    {
        return Err(XaaError::Discovery(format!(
            "PRM resource mismatch: expected {server_url}, got {resource}"
        )));
    }
    let resource = prm
        .resource
        .clone()
        .unwrap_or_else(|| server_url.trim_end_matches('/').to_string());

    // Try each advertised AS until one supports the jwt-bearer grant.
    let mut chosen = None;
    let mut errors = Vec::new();
    for as_url in &prm.authorization_servers {
        match discover_authorization_server(client, as_url).await {
            Ok(meta) => {
                if !meta.grant_types.is_empty()
                    && !meta.grant_types.iter().any(|g| g == JWT_BEARER_GRANT)
                {
                    errors.push(format!("{as_url}: does not advertise jwt-bearer"));
                    continue;
                }
                chosen = Some(meta);
                break;
            }
            Err(e) => errors.push(format!("{as_url}: {e}")),
        }
    }
    let as_meta = chosen.ok_or_else(|| {
        XaaError::Discovery(format!(
            "no authorization server supports jwt-bearer; tried: {}",
            errors.join("; ")
        ))
    })?;

    // Pick auth method from what the AS advertises (SEP-990 default: basic).
    let auth_method = if !as_meta.token_endpoint_auth_methods.is_empty()
        && !as_meta
            .token_endpoint_auth_methods
            .iter()
            .any(|m| m == "client_secret_basic")
        && as_meta
            .token_endpoint_auth_methods
            .iter()
            .any(|m| m == "client_secret_post")
    {
        AsAuthMethod::ClientSecretPost
    } else {
        AsAuthMethod::ClientSecretBasic
    };
    debug!(
        issuer = %as_meta.issuer,
        token_endpoint = %as_meta.token_endpoint,
        "xaa: selected authorization server"
    );

    let id_jag = request_jwt_authorization_grant(
        client,
        JwtGrantRequest {
            token_endpoint: config.idp_token_endpoint.clone(),
            audience: as_meta.issuer.clone(),
            resource,
            id_token: config.idp_id_token.clone(),
            client_id: config.idp_client_id.clone(),
            client_secret: config.idp_client_secret.clone(),
            scope: config.scope.clone(),
        },
    )
    .await?;

    let mut result = exchange_jwt_auth_grant(
        client,
        JwtBearerRequest {
            token_endpoint: as_meta.token_endpoint.clone(),
            assertion: id_jag,
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            auth_method,
            scope: config.scope.clone(),
        },
    )
    .await?;
    result.authorization_server_url = as_meta.issuer;
    Ok(result)
}

// ── Private types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenExchangeResponse {
    access_token: String,
    issued_token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    token_type: Option<String>,
    scope: Option<String>,
    refresh_token: Option<String>,
}

/// Decide whether a non-2xx IdP response should invalidate the cached
/// id_token. 4xx / invalid_grant / invalid_token → clear; 5xx → keep
/// (server-side glitch).
fn should_clear_id_token_on_status(status: u16, body: &str) -> bool {
    if (500..600).contains(&status) {
        return false;
    }
    // 4xx: trust the server
    if (400..500).contains(&status) {
        return true;
    }
    // Other non-2xx: rare, default to clearing
    if body.contains("invalid_grant") || body.contains("invalid_token") {
        return true;
    }
    false
}

/// Redact any token-bearing JSON fields from `raw` — crucial for not leaking
/// the subject_token on 4xx error bodies that echo it back.
pub(crate) fn redact_tokens(raw: &str) -> String {
    const SENSITIVE: &[&str] = &[
        "access_token",
        "refresh_token",
        "id_token",
        "assertion",
        "subject_token",
        "client_secret",
    ];
    let mut out = raw.to_string();
    for key in SENSITIVE {
        // Match the JSON form `"key":"<anything without a quote>"` and
        // replace the value with `[REDACTED]`. This is a best-effort
        // text transform, not a JSON-aware rewrite.
        //
        // Advance `search_from` past each redaction so the scan
        // doesn't re-match the same key on the next iteration (which
        // would be an infinite loop since we only replace the value).
        let pattern = format!("\"{key}\":");
        let mut search_from = 0usize;
        while let Some(rel_start) = out[search_from..].find(&pattern) {
            let start = search_from + rel_start;
            let value_start = start + pattern.len();
            let rest = &out[value_start..];
            if let Some(q1) = rest.find('"') {
                let value_content_start = value_start + q1 + 1;
                if let Some(q2) = out[value_content_start..].find('"') {
                    let value_content_end = value_content_start + q2;
                    out.replace_range(value_content_start..value_content_end, "[REDACTED]");
                    // Continue scanning *after* the redacted region so
                    // subsequent occurrences of the same key still get
                    // found, but the just-replaced one is skipped.
                    search_from = value_content_start + "[REDACTED]".len();
                    continue;
                }
            }
            // Couldn't find a terminating quote; bail on this key
            break;
        }
    }
    out
}

#[cfg(test)]
#[path = "xaa.test.rs"]
mod tests;
