//! PKCE (S256) + CSRF state generation. Mirrors codex `login/src/pkce.rs`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::Digest;
use sha2::Sha256;

/// A PKCE verifier/challenge pair (method is always S256).
#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate a PKCE pair: 64 random bytes → URL-safe-no-pad verifier, then
/// `challenge = base64url(SHA256(verifier_ascii))`.
pub fn generate_pkce() -> PkceCodes {
    let mut verifier_bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

/// Generate a random CSRF `state` token (32 bytes, URL-safe-no-pad).
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
#[path = "pkce.test.rs"]
mod tests;
