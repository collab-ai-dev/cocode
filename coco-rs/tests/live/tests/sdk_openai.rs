//! SDK-layer live tests for the builtin `openai` provider via the
//! OpenAI Responses API (`coco-inference model runtime`, no agent loop).
//!
//! ```text
//! builtin_providers()  →  RuntimeConfig (overlay = COCO_LIVE_TEST_OPENAI_*)
//!     →  ModelRegistry  →  model_factory::build_api_client
//!     →  coco_inference::ModelRuntimeClient.query / .query_stream
//!     →  <COCO_LIVE_TEST_OPENAI_BASE_URL or api.openai.com>/responses
//! ```
//!
//! The builtin `openai` provider already pins `wire_api: Responses`
//! (see common/config/src/provider/builtin.rs:43), so this binary
//! exercises the Responses wire shape end-to-end. Point
//! `COCO_LIVE_TEST_OPENAI_BASE_URL` at any Responses-API-compatible
//! gateway (TikTok GPT proxy, Azure OpenAI, …) to retarget without
//! editing builtins.
//!
//! Skips with a one-line message when `OPENAI_API_KEY` is unset or the
//! provider/capability is excluded by `COCO_LIVE_TEST_*` gates.
//!
//! # Running
//!
//! ```bash
//! cargo test -p coco-tests-live --test sdk_openai -- --test-threads=1
//! cargo test -p coco-tests-live --test sdk_openai streaming
//! cargo test -p coco-tests-live --test sdk_openai tools
//! ```

// `cross_protocol` suite is shared with `sdk_deepseek` and not exercised
// here; silence per-binary `dead_code` warnings without touching the
// shared module.
#![allow(dead_code, unused_imports)]

mod common;

#[path = "sdk/suite/mod.rs"]
mod suite;

use anyhow::Result;

const PROVIDER: &str = "openai";

#[tokio::test]
async fn test_basic_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run(&target).await
}

#[tokio::test]
async fn test_token_usage_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_token_usage(&target).await
}

#[tokio::test]
async fn test_multi_turn_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_multi_turn(&target).await
}

#[tokio::test]
async fn test_long_multi_turn_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::basic::run_long_multi_turn(&target).await
}

#[tokio::test]
async fn test_streaming_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "streaming");
    suite::streaming::run(&target).await
}

#[tokio::test]
async fn test_streaming_tools_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "tools");
    suite::streaming::run_with_tools(&target).await
}

#[tokio::test]
async fn test_tools_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "tools");
    suite::tools::run(&target).await
}

#[tokio::test]
async fn test_inference_smoke_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "text");
    suite::inference_smoke::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_vision_image_url_openai() -> Result<()> {
    let target = require_live!(PROVIDER, "vision");
    suite::vision::run(&target).await
}

#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("sdk_openai")?;
    Ok(())
}

#[test]
fn list_configured_openai_provider() {
    let configured = common::provider_has_credentials(PROVIDER);
    eprintln!("[coco-tests-live] openai provider configured: {configured}");
}
