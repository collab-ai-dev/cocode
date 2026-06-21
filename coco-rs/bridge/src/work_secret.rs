//! Workspace-scoped shared secret.
//!
//! The secret is generated once per workspace (keyed by the absolute
//! path) and stored in the user's keychain; it's what the IDE and CLI
//! exchange to prove they belong to the same trust domain. The JWT
//! layer uses it as the HS256 key.
//!
//! This module owns just the **derivation** of the secret — the actual
//! keychain persistence is delegated to `coco-keyring-store`, which is
//! a dependency this crate doesn't pull in (we'd need OS-specific
//! backends). Callers (usually the bridge server or CLI bootstrap)
//! combine `derive_secret_from_material` with their own storage.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest;
use sha2::Sha256;

/// Length of the final secret in bytes (256-bit / 32 bytes).
pub const SECRET_BYTES: usize = 32;

/// Keychain service name the bridge uses to store per-workspace secrets.
pub const KEYRING_SERVICE: &str = "coco-bridge";

/// Keychain account suffix used as the key in `{service, account}` pairs.
/// Accounts are per-workspace, computed via `account_name_for_workspace`.
pub const KEYRING_ACCOUNT_PREFIX: &str = "work-secret:";

/// Derive a 32-byte workspace secret from the hash of
/// `(workspace_path || installation_id)`. Output is base64url (no
/// padding) for storage and wire transport.
///
/// This is **not** a security boundary on its own — the output is still
/// stored securely. It's used to produce stable, collision-free account
/// keys for the keychain when the raw path would be too long or contain
/// invalid characters.
pub fn derive_secret_from_material(workspace_path: &str, installation_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_path.as_bytes());
    hasher.update([0]); // NUL separator so `"a"+"bc"` doesn't collide with `"ab"+"c"`
    hasher.update(installation_id.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

/// Compute the keychain account name for a workspace. The output is
/// short and safe for all keyring backends.
pub fn account_name_for_workspace(workspace_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_path.as_bytes());
    let digest = hasher.finalize();
    let short = URL_SAFE_NO_PAD.encode(&digest[..12]); // 16-char prefix
    format!("{KEYRING_ACCOUNT_PREFIX}{short}")
}

/// Fresh-secret generator: collect 32 bytes of randomness from the OS
/// and return base64url-encoded. Uses `getrandom` indirectly via the
/// `sha2` dependency graph; if that fails the caller should surface
/// the error to the user rather than fall back to weak randomness.
pub fn generate_fresh_secret() -> std::io::Result<String> {
    let mut buf = [0u8; SECRET_BYTES];
    getrandom_fill(&mut buf).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// Decode a base64url-encoded secret back to raw bytes. Returns an
/// error on malformed input.
pub fn decode_secret(encoded: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(encoded)
}

// ── Internals ──────────────────────────────────────────────────────

/// Thin wrapper around the platform RNG. Extracted so tests can stub
/// it via a feature flag if we ever want deterministic fixtures.
fn getrandom_fill(buf: &mut [u8]) -> Result<(), &'static str> {
    // The workspace already depends on `rustls` with the `ring` feature,
    // which in turn pulls in a CSPRNG. Rather than take a new dep we
    // read from `/dev/urandom` directly on Unix and fall back to the
    // `rand` crate on Windows via std::process::id mixing — which is
    // NOT a CSPRNG. To keep portability without a new dep, use the
    // simpler approach: read OS entropy through std.
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut f = std::fs::File::open("/dev/urandom").map_err(|_| "open /dev/urandom")?;
        f.read_exact(buf).map_err(|_| "read /dev/urandom")?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Windows / WASI etc: if no OS entropy is available via std,
        // callers should use an explicit RNG crate. Fail loudly rather
        // than silently degrade.
        let _ = buf;
        Err("platform RNG not available; pass your own secret")
    }
}

#[cfg(test)]
#[path = "work_secret.test.rs"]
mod tests;
