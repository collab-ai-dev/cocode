//! Minimal JWT payload reader — base64url-decode the claims segment and pull
//! out string claims (account id) or the `exp` timestamp. No signature
//! verification: these tokens come from our own OAuth exchange over TLS and are
//! used only to read non-sensitive routing metadata.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

/// Decode a JWT's payload (middle segment) into a JSON value.
fn decode_payload(jwt: &str) -> Option<Value> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read a (possibly nested) string claim by path, e.g.
/// `["https://api.openai.com/auth", "chatgpt_account_id"]`.
pub fn read_string_claim(jwt: &str, path: &[&str]) -> Option<String> {
    let mut node = decode_payload(jwt)?;
    for (i, key) in path.iter().enumerate() {
        let next = node.get(*key)?;
        if i + 1 == path.len() {
            return next.as_str().map(str::to_string);
        }
        node = next.clone();
    }
    None
}

/// Read the `exp` claim (seconds since epoch) as epoch milliseconds.
pub fn read_exp_ms(jwt: &str) -> Option<i64> {
    let payload = decode_payload(jwt)?;
    let exp = payload.get("exp")?.as_i64()?;
    Some(exp.saturating_mul(1000))
}

#[cfg(test)]
#[path = "jwt.test.rs"]
mod tests;
