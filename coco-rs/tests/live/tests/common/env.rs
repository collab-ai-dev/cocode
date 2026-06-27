//! Test environment loading.
//!
//! Loads `.env.test` (preferred) or `.env` from the crate root exactly once
//! per test process via `OnceLock` + `dotenvy`. Mirrors the convention used
//! by `vercel-ai/ai/tests/common/config.rs` so the same `.env` works for
//! both test runners.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

const ENV_FILE_VAR: &str = "COCO_LIVE_TEST_ENV_FILE";
const DEFAULT_ENV_FILE: &str = ".env.test";
const FALLBACK_ENV_FILE: &str = ".env";

/// Capability gates honored by suites — keep in sync with what the suite
/// modules actually check.
pub const ALL_CAPABILITIES: &[&str] = &[
    "text",
    "streaming",
    "tools",
    "vision",
    "compact",
    "cross_protocol",
];

const CAPABILITIES_VAR: &str = "COCO_LIVE_TEST_CAPABILITIES";
const PROVIDERS_VAR: &str = "COCO_LIVE_TEST_PROVIDERS";

static ENV_LOADED: OnceLock<bool> = OnceLock::new();

/// Load the test `.env` file into the process environment exactly once.
/// Safe to call from every test entry point — subsequent calls are no-ops.
pub fn ensure_env_loaded() {
    ENV_LOADED.get_or_init(|| {
        let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let env_file = std::env::var(ENV_FILE_VAR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let test_env = crate_root.join(DEFAULT_ENV_FILE);
                if test_env.exists() {
                    test_env
                } else {
                    crate_root.join(FALLBACK_ENV_FILE)
                }
            });
        if env_file.exists() && dotenvy::from_path(&env_file).is_ok() {
            eprintln!("[coco-tests-live] loaded env from {}", env_file.display());
        }
        true
    });
}

/// Read the active capability set.
///
/// Resolution: `COCO_LIVE_TEST_CAPABILITIES` (comma list, or `none`) →
/// otherwise all capabilities are enabled.
pub fn capabilities() -> HashSet<String> {
    ensure_env_loaded();
    match std::env::var(CAPABILITIES_VAR) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
                return HashSet::new();
            }
            value
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        }
        Err(_) => ALL_CAPABILITIES.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// `true` when the named capability is currently enabled.
pub fn capability_enabled(capability: &str) -> bool {
    capabilities().contains(capability)
}

/// Optional provider allow-list — if set, providers not in the list are
/// skipped even when their API key is present. Empty / unset = run all.
pub fn provider_allowlist() -> Option<HashSet<String>> {
    ensure_env_loaded();
    let raw = std::env::var(PROVIDERS_VAR).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        raw.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// `true` when the named provider is allowed by `COCO_LIVE_TEST_PROVIDERS`
/// (or the variable is unset).
pub fn provider_allowed(provider_name: &str) -> bool {
    match provider_allowlist() {
        Some(set) => set.contains(&provider_name.to_lowercase()),
        None => true,
    }
}

/// Convert builtin provider name to the env-var token used in
/// `COCO_LIVE_TEST_<TOKEN>_<FIELD>` (uppercase, `-` → `_`).
fn provider_env_token(provider_name: &str) -> String {
    provider_name.to_uppercase().replace('-', "_")
}

/// Per-provider model under test. Reads `COCO_LIVE_TEST_<PROVIDER>_MODEL`.
/// `None` when unset or empty — the calling test should skip with a
/// one-line message rather than fall back to a hardcoded default, so
/// model identity stays visible in `.env` (no surprise routing).
pub fn provider_model(provider_name: &str) -> Option<String> {
    ensure_env_loaded();
    let key = format!("COCO_LIVE_TEST_{}_MODEL", provider_env_token(provider_name));
    std::env::var(&key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `true` when `capability` is enabled for `provider_name`. Resolution
/// order, mirroring `vercel-ai/ai/tests/common/config.rs`:
///
/// 1. `COCO_LIVE_TEST_<PROVIDER>_CAPABILITIES` (per-provider, comma list
///    or `none`)
/// 2. `COCO_LIVE_TEST_CAPABILITIES` (global)
/// 3. All capabilities enabled
pub fn capability_enabled_for(provider_name: &str, capability: &str) -> bool {
    ensure_env_loaded();
    let key = format!(
        "COCO_LIVE_TEST_{}_CAPABILITIES",
        provider_env_token(provider_name)
    );
    if let Ok(value) = std::env::var(&key) {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            return false;
        }
        let set: HashSet<String> = value
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        return set.contains(capability);
    }
    capability_enabled(capability)
}

/// Env-var name suggested in skip messages so users know what to set.
pub fn provider_model_var(provider_name: &str) -> String {
    format!("COCO_LIVE_TEST_{}_MODEL", provider_env_token(provider_name))
}
