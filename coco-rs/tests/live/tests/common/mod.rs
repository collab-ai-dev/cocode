//! Shared scaffolding for live integration tests.
//!
//! Tests in this crate exercise the **full coco-rs chain**:
//!
//! ```text
//!  builtin_providers()  →  RuntimeConfig  →  ModelRegistry
//!     →  model_factory::build_language_model_from_runtime
//!     →  vercel-ai SDK / coco_inference::ModelRuntimeClient
//!     →  real provider HTTP API
//! ```
//!
//! The framework auto-skips when credentials are missing — tests print a
//! one-line reason and return `Ok(())` so unconfigured CI stays green.

// Each `tests/<runner>.rs` includes this module via `mod common;` and
// only consumes the subset it needs. `dead_code` and `unused_imports`
// are silenced so a runner that doesn't touch `fixtures` / `build_client`
// doesn't produce per-binary warnings.
#![allow(dead_code, unused_imports)]

pub mod env;
pub mod fixtures;
pub mod reminders;
pub mod runtime;
pub mod tmpdir;
pub mod usage_report;

pub use fixtures::*;
pub use runtime::LiveTarget;
pub use runtime::build_client;
pub use runtime::provider_has_credentials;

/// Resolve a `LiveTarget` for `provider` or return `Ok(())` from the
/// calling test with a one-line skip message.
///
/// Two forms:
///
/// - `require_live!(provider, capability)` — model is read from
///   `COCO_LIVE_TEST_<PROVIDER>_MODEL` (preferred; mirrors the
///   `vercel-ai/ai/tests` pattern of declaring provider+model entirely
///   in `.env`).
/// - `require_live!(provider, model, capability)` — explicit model
///   (back-compat; lets one test target multiple model variants without
///   shadowing the env var).
///
/// Skip cases (single-line `eprintln!` then `return Ok(())`):
/// - `COCO_LIVE_TEST_PROVIDERS` allow-list excludes this provider
/// - `COCO_LIVE_TEST_<PROVIDER>_MODEL` (or 3-arg model) is unset
/// - Provider has no API key (neither `COCO_LIVE_TEST_<PROVIDER>_API_KEY`
///   nor the builtin's native `env_key`)
/// - Capability is disabled by per-provider or global `*_CAPABILITIES`
///
/// Usage:
/// ```ignore
/// #[tokio::test]
/// async fn test_basic_openai() -> anyhow::Result<()> {
///     // .env: COCO_LIVE_TEST_OPENAI_MODEL=gpt-5-2025-08-07
///     let target = require_live!("openai", "text");
///     suite::basic::run(&target).await
/// }
/// ```
#[macro_export]
macro_rules! require_live {
    ($provider:expr, $capability:expr) => {{
        if !$crate::common::env::capability_enabled_for($provider, $capability) {
            eprintln!(
                "[skip] capability `{}` disabled for `{}` by COCO_LIVE_TEST_*_CAPABILITIES",
                $capability, $provider
            );
            return Ok(());
        }
        match $crate::common::LiveTarget::try_resolve_env_model($provider) {
            None => {
                eprintln!(
                    "[skip] provider `{}` not available (set {} and the API key, or check \
                     COCO_LIVE_TEST_PROVIDERS allow-list)",
                    $provider,
                    $crate::common::env::provider_model_var($provider),
                );
                return Ok(());
            }
            Some(Err(e)) => {
                return Err(anyhow::anyhow!(
                    "failed to build live target for {}: {e}",
                    $provider,
                ));
            }
            Some(Ok(target)) => target,
        }
    }};
    ($provider:expr, $model:expr, $capability:expr) => {{
        if !$crate::common::env::capability_enabled_for($provider, $capability) {
            eprintln!(
                "[skip] capability `{}` disabled for `{}` by COCO_LIVE_TEST_*_CAPABILITIES",
                $capability, $provider
            );
            return Ok(());
        }
        match $crate::common::LiveTarget::try_resolve($provider, $model) {
            None => {
                eprintln!(
                    "[skip] provider `{}` not available (missing API key or excluded by \
                     COCO_LIVE_TEST_PROVIDERS)",
                    $provider
                );
                return Ok(());
            }
            Some(Err(e)) => {
                return Err(anyhow::anyhow!(
                    "failed to build live target {}/{}: {e}",
                    $provider,
                    $model
                ));
            }
            Some(Ok(target)) => target,
        }
    }};
}
