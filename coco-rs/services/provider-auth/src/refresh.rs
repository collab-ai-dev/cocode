//! Token-endpoint POST (shared by code-exchange and refresh) + the refresh
//! executor. Refresh tokens rotate (single-use) for OpenAI/Claude, so the
//! caller (`AuthService`) serializes refresh under a per-cell `Semaphore`.

use serde::Deserialize;

use crate::descriptor::BodyEncoding;
use crate::descriptor::OAuthFlowDescriptor;
use crate::descriptor::RefreshTokenRotation;
use crate::error::Result;
use crate::error::SessionExpiredSnafu;
use crate::error::TokenEndpointSnafu;
use crate::jwt;
use crate::token_cell::TokenSnapshot;

/// Common OAuth token-endpoint response shape.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

/// Current wall-clock in epoch milliseconds.
pub(crate) fn now_ms() -> i64 {
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// POST the token endpoint with the descriptor-specified encoding.
pub async fn post_token(
    http: &reqwest::Client,
    url: &str,
    encoding: BodyEncoding,
    params: &[(&str, String)],
) -> Result<TokenResponse> {
    let builder = match encoding {
        BodyEncoding::Form => http.post(url).form(params),
        BodyEncoding::Json => {
            let map: serde_json::Map<String, serde_json::Value> = params
                .iter()
                .map(|(k, v)| ((*k).to_string(), serde_json::Value::String(v.clone())))
                .collect();
            http.post(url).json(&map)
        }
    };
    let resp = builder.send().await.map_err(|e| {
        crate::error::NetworkSnafu {
            message: e.to_string(),
        }
        .build()
    })?;
    let status = resp.status();
    if !status.is_success() {
        // Redact + cap the raw body before it lands in an error (which is
        // printed to the terminal on interactive login AND written to the
        // rotating log on background refresh). RFC 6749 §5.2 error bodies
        // carry `error`/`error_description`, but some IdPs echo the submitted
        // `code`/`refresh_token` in `error_description` — never persist that.
        let raw = resp.text().await.unwrap_or_default();
        let redacted = coco_secret_redact::redact_secrets(&raw);
        let message: String = redacted.chars().take(512).collect();
        return Err(TokenEndpointSnafu {
            status: i32::from(status.as_u16()),
            message,
        }
        .build());
    }
    resp.json::<TokenResponse>().await.map_err(|e| {
        crate::error::InternalSnafu {
            message: format!("decode token response: {e}"),
        }
        .build()
    })
}

/// Derive `expires_at_ms` from the response (`expires_in`) or the access-token
/// JWT `exp` claim, whichever is available.
pub(crate) fn expires_at_ms(tr: &TokenResponse) -> Option<i64> {
    tr.expires_in
        .map(|secs| now_ms() + secs.saturating_mul(1000))
        .or_else(|| jwt::read_exp_ms(&tr.access_token))
}

/// Refresh the access token. `prev` carries the current snapshot (its
/// `refresh_token` + `account_id`). Returns the new snapshot. A 401 maps to
/// `SessionExpired` (refresh token expired/reused/revoked).
pub async fn refresh(
    descriptor: &OAuthFlowDescriptor,
    provider_name: &str,
    prev: &TokenSnapshot,
    http: &reqwest::Client,
) -> Result<TokenSnapshot> {
    refresh_at(
        &descriptor.effective_token_url(),
        descriptor,
        provider_name,
        prev,
        http,
    )
    .await
}

/// `refresh` with the token endpoint supplied explicitly. Separated so wiremock
/// tests can point the refresh at a mock server without the env-override seam
/// (which is debug-gated and process-global).
pub async fn refresh_at(
    token_url: &str,
    descriptor: &OAuthFlowDescriptor,
    provider_name: &str,
    prev: &TokenSnapshot,
    http: &reqwest::Client,
) -> Result<TokenSnapshot> {
    let Some(refresh_token) = prev.refresh_token.clone() else {
        return Err(SessionExpiredSnafu {
            provider: provider_name.to_string(),
        }
        .build());
    };

    let mut params: Vec<(&str, String)> = vec![
        ("client_id", descriptor.client_id.to_string()),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.clone()),
    ];
    if let Some(secret) = descriptor.client_secret {
        params.push(("client_secret", secret.to_string()));
    }
    params.extend(
        descriptor
            .refresh_extra
            .iter()
            .map(|(k, v)| (*k, (*v).to_string())),
    );

    let tr = match post_token(http, token_url, descriptor.refresh_encoding, &params).await {
        Ok(tr) => tr,
        Err(crate::error::ProviderAuthError::TokenEndpoint { status: 401, .. }) => {
            return Err(SessionExpiredSnafu {
                provider: provider_name.to_string(),
            }
            .build());
        }
        Err(e) => return Err(e),
    };

    let new_account_id = tr
        .id_token
        .as_ref()
        .and_then(|jwt| account_id_from_id_token(descriptor, jwt))
        .or_else(|| prev.account_id.clone());

    // `Rotates` flows hand back a fresh single-use refresh token each call; the
    // previous one is now dead. If the server omitted one we keep the old token
    // to avoid losing the session, but that is anomalous for a rotating flow —
    // surface it so a broken endpoint doesn't masquerade as success.
    let next_refresh = match (descriptor.refresh_rotation, &tr.refresh_token) {
        (RefreshTokenRotation::Rotates, None) => {
            tracing::warn!(
                provider = provider_name,
                "rotating-refresh flow returned no new refresh token; reusing the prior token"
            );
            Some(refresh_token)
        }
        // Rotates with a new token, or Persists (response may omit it by design).
        (_, Some(_)) => tr.refresh_token.clone(),
        (RefreshTokenRotation::Persists, None) => Some(refresh_token),
    };

    Ok(TokenSnapshot {
        access_token: tr.access_token.clone(),
        account_id: new_account_id,
        refresh_token: next_refresh,
        subscription_type: prev.subscription_type.clone(),
        expires_at_ms: expires_at_ms(&tr),
        // Refresh never changes identity — carry the epoch through.
        login_epoch: prev.login_epoch,
    })
}

/// Best-effort RFC 7009 token revocation (used by `logout`). POSTs `token`
/// (+ client id/secret) to the descriptor's `revoke_url`. A `None` revoke_url
/// or a non-2xx response is not an error — local logout proceeds regardless.
pub async fn revoke(
    descriptor: &OAuthFlowDescriptor,
    token: &str,
    http: &reqwest::Client,
) -> Result<()> {
    let Some(url) = descriptor.effective_revoke_url() else {
        return Ok(());
    };
    let mut params: Vec<(&str, String)> = vec![
        ("token", token.to_string()),
        ("client_id", descriptor.client_id.to_string()),
    ];
    if let Some(secret) = descriptor.client_secret {
        params.push(("client_secret", secret.to_string()));
    }
    http.post(&url).form(&params).send().await.map_err(|e| {
        crate::error::NetworkSnafu {
            message: e.to_string(),
        }
        .build()
    })?;
    Ok(())
}

pub(crate) fn account_id_from_id_token(
    descriptor: &OAuthFlowDescriptor,
    id_token: &str,
) -> Option<String> {
    match descriptor.account_id {
        crate::descriptor::AccountIdSource::IdTokenClaim { path } => {
            jwt::read_string_claim(id_token, path)
        }
        // Google's account identity (email) comes from a userinfo fetch at login
        // time, not the id_token — handled in `flow::login`.
        crate::descriptor::AccountIdSource::UserInfoEndpoint { .. }
        | crate::descriptor::AccountIdSource::None => None,
    }
}
