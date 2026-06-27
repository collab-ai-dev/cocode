//! JWT (HS256) signing + validation for the IDE bridge.
//!
//! Implements just enough of JWS to sign and validate a short-lived
//! session token; not a general-purpose JWT library (use `jsonwebtoken`
//! for general JWT needs).
//!
//! Supports only HS256 (HMAC-SHA256) — the bridge has a shared secret
//! between IDE and CLI, no public-key verification needed.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::Hmac;
use hmac::Mac;
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// The standard HS256 JOSE header, serialized once at compile time.
const HS256_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// A decoded JWT payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    /// Subject — typically the IDE client identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// Issued-at (seconds since epoch).
    #[serde(default)]
    pub iat: i64,
    /// Expiration (seconds since epoch).
    pub exp: i64,
    /// Audience — bridge-scoped resource name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// Workspace scope (work_secret tie-in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Nonce for replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

impl Claims {
    /// Build claims with `exp = now + ttl_secs`.
    pub fn new(ttl_secs: i64) -> Self {
        let iat = now_unix();
        Self {
            sub: None,
            iat,
            exp: iat + ttl_secs,
            aud: None,
            workspace: None,
            nonce: None,
        }
    }

    // Builder method named after the JWT `sub` claim — the trait-name
    // collision with `std::ops::Sub::sub` is intentional here.
    #[allow(clippy::should_implement_trait)]
    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    pub fn aud(mut self, aud: impl Into<String>) -> Self {
        self.aud = Some(aud.into());
        self
    }

    pub fn workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    pub fn nonce(mut self, nonce: impl Into<String>) -> Self {
        self.nonce = Some(nonce.into());
        self
    }
}

/// Errors returned by validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JwtError {
    #[error("malformed token (expected 3 segments)")]
    Malformed,
    #[error("unsupported algorithm (only HS256)")]
    UnsupportedAlg,
    #[error("signature mismatch")]
    BadSignature,
    #[error("token expired at {exp}, current time {now}")]
    Expired { exp: i64, now: i64 },
    #[error("payload decode failed: {0}")]
    Payload(String),
    #[error("header decode failed: {0}")]
    Header(String),
}

/// Sign `claims` with the HS256 secret, returning a compact JWS.
pub fn sign(claims: &Claims, secret: &[u8]) -> String {
    let header_b64 = URL_SAFE_NO_PAD.encode(HS256_HEADER.as_bytes());
    let payload_json = serde_json::to_vec(claims).unwrap_or_else(|_| b"{}".to_vec());
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_json);
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature = hmac_sha256(signing_input.as_bytes(), secret);
    let sig_b64 = URL_SAFE_NO_PAD.encode(&signature);
    format!("{signing_input}.{sig_b64}")
}

/// Validate a JWS and return its claims.
///
/// Checks: 3-segment structure, HS256 alg, constant-time HMAC compare,
/// `exp` vs `now`. Does NOT validate `aud` — callers that need that
/// check the returned claims themselves.
pub fn verify(token: &str, secret: &[u8]) -> Result<Claims, JwtError> {
    let mut parts = token.split('.');
    let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err(JwtError::Malformed),
    };

    let header_bytes = URL_SAFE_NO_PAD
        .decode(h)
        .map_err(|e| JwtError::Header(e.to_string()))?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|e| JwtError::Header(e.to_string()))?;
    if header.get("alg").and_then(|a| a.as_str()) != Some("HS256") {
        return Err(JwtError::UnsupportedAlg);
    }

    let signing_input = format!("{h}.{p}");
    let expected = hmac_sha256(signing_input.as_bytes(), secret);
    let actual = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| JwtError::BadSignature)?;
    if !constant_time_eq(&expected, &actual) {
        return Err(JwtError::BadSignature);
    }

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(p)
        .map_err(|e| JwtError::Payload(e.to_string()))?;
    let claims: Claims =
        serde_json::from_slice(&payload_bytes).map_err(|e| JwtError::Payload(e.to_string()))?;

    let now = now_unix();
    if claims.exp < now {
        return Err(JwtError::Expired {
            exp: claims.exp,
            now,
        });
    }
    Ok(claims)
}

fn hmac_sha256(data: &[u8], secret: &[u8]) -> Vec<u8> {
    // `HmacSha256::new_from_slice` only fails on `InvalidKeyLength` for
    // fixed-length MACs; HS256 accepts any key length, so this branch
    // cannot fail at runtime. Fall back to an empty vec defensively
    // rather than panic — callers will then fail the constant-time
    // compare, which is the right (safe) outcome.
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return Vec::new();
    };
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time comparison to avoid timing leaks on signature mismatch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "jwt_utils.test.rs"]
mod tests;
