//! SDK-layer live tests for the builtin `google` provider via the
//! Gemini API (`coco-inference model runtime`, no agent loop).
//!
//! ```text
//! builtin_providers()  →  RuntimeConfig (overlay = COCO_LIVE_TEST_GOOGLE_*)
//!     →  ModelRegistry  →  model_factory::build_api_client
//!     →  coco_inference::ModelRuntimeClient.query / .query_stream
//!     →  <COCO_LIVE_TEST_GOOGLE_BASE_URL
//!         or generativelanguage.googleapis.com/v1beta>
//!         /models/<id>:generateContent
//! ```
//!
//! The builtin `google` provider pins `api: Gemini` and a default
//! `https://generativelanguage.googleapis.com/v1beta` base URL (see
//! common/config/src/provider/builtin.rs:46). Point
//! `COCO_LIVE_TEST_GOOGLE_BASE_URL` at any Gemini-API-compatible gateway
//! (e.g. the Bytedance AIDP modelhub mirror) to retarget without
//! editing builtins. The provider sends the credential as the
//! `x-goog-api-key` header.
//!
//! Skips with a one-line message when `GOOGLE_API_KEY` /
//! `COCO_LIVE_TEST_GOOGLE_API_KEY` is unset or the provider /
//! capability is excluded by `COCO_LIVE_TEST_*` gates.
//!
//! # Running
//!
//! ```bash
//! cargo test -p coco-tests-live --test sdk_google -- --test-threads=1
//! cargo test -p coco-tests-live --test sdk_google streaming
//! cargo test -p coco-tests-live --test sdk_google tools
//! ```

// `cross_protocol` suite is shared with `sdk_deepseek` and not exercised
// here; silence per-binary `dead_code` warnings without touching the
// shared module.
#![allow(dead_code, unused_imports)]

mod common;

#[path = "sdk/suite/mod.rs"]
mod suite;

use anyhow::Result;

const PROVIDER: &str = "google";

#[tokio::test]
async fn test_basic_google() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run(&target).await
}

#[tokio::test]
async fn test_token_usage_google() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_token_usage(&target).await
}

#[tokio::test]
async fn test_multi_turn_google() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_multi_turn(&target).await
}

#[tokio::test]
async fn test_long_multi_turn_google() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_long_multi_turn(&target).await
}

#[tokio::test]
async fn test_streaming_google() -> Result<()> {
    let target = require_live!(PROVIDER, "streaming");
    suite::streaming::run(&target).await
}

#[tokio::test]
async fn test_streaming_tools_google() -> Result<()> {
    let target = require_live!(PROVIDER, "tools");
    // Gemini is flaky under `tool_choice: None` (sometimes answers in prose),
    // so force the call. Other providers keep the unforced variant — DeepSeek's
    // thinking model rejects a forced `tool_choice` (HTTP 400).
    suite::streaming::run_with_tools_forced(&target).await
}

/// Regression: thinking_level=medium + tool schema with `Option<i64>`
/// and per-variant string enum. Covers two Gemini-3 bugs:
/// (1) `thinkingConfig` root-leak via the `provider_options` shallow
/// merge; (2) schema converter emitting `anyOf`/`oneOf` with siblings,
/// rejected by Gemini-3 strict mode. See suite/streaming.rs doc comment
/// on `run_thinking_with_option_typed_tools` for the full story.
#[tokio::test]
async fn test_streaming_thinking_with_option_typed_tools_google() -> Result<()> {
    let target = require_live!(PROVIDER, "tools");
    suite::streaming::run_thinking_with_option_typed_tools(&target).await
}

#[tokio::test]
async fn test_tools_google() -> Result<()> {
    let target = require_live!(PROVIDER, "tools");
    suite::tools::run(&target).await
}

#[tokio::test]
async fn test_inference_smoke_google() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::inference_smoke::run(target.provider, &target.model).await
}

#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("sdk_google")?;
    Ok(())
}

#[test]
fn list_configured_google_provider() {
    let configured = common::provider_has_credentials(PROVIDER);
    eprintln!("[coco-tests-live] google provider configured: {configured}");
}
