//! SDK-layer live tests for DeepSeek (`coco-inference model runtime`, no agent loop).
//!
//! Each test exercises the **provider construction chain** plus the inference call:
//!
//! ```text
//! builtin_providers()  →  RuntimeConfig  →  ModelRegistry
//!     →  model_factory::build_api_client
//!     →  coco_inference::ModelRuntimeClient.query / .query_stream
//!     →  api.deepseek.com (real HTTP)
//! ```
//!
//! All AI SDK access flows through the `coco_inference` seam — direct
//! `vercel-ai` use is forbidden (enforced by
//! `scripts/check-vercel-ai-seam.sh`).
//!
//! For end-to-end agent-loop coverage (`QueryEngine` + tools + compaction +
//! `CoreEvent` stream), see the sibling `cli_deepseek` test target.
//!
//! Two builtin providers reach the same model (`deepseek-v4-flash`):
//! - `deepseek-openai`     — OpenAI-compatible at `/v1`
//! - `deepseek-anthropic`  — Anthropic-shaped at `/anthropic/v1`
//!
//! Skips with a one-line message when `DEEPSEEK_API_KEY` is unset or the
//! provider/capability is excluded by `COCO_LIVE_TEST_*` gates.
//!
//! # Running
//!
//! ```bash
//! cargo test -p coco-tests-live --test sdk_deepseek -- --test-threads=1
//! cargo test -p coco-tests-live --test sdk_deepseek streaming
//! cargo test -p coco-tests-live --test sdk_deepseek tools
//! ```

// `vision` suite is openai-only and not exercised here; silence the
// per-binary `dead_code` warnings the shared module would otherwise emit.
#![allow(dead_code, unused_imports)]

mod common;

#[path = "sdk/suite/mod.rs"]
mod suite;

use anyhow::Result;

// ─── deepseek-openai (OpenAI-compatible protocol) ─────────────────────
// Set `COCO_LIVE_TEST_DEEPSEEK_OPENAI_MODEL` (e.g. `deepseek-v4-flash`)
// in `.env` — the macro skips with a one-line message when unset.

#[tokio::test]
async fn test_basic_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    suite::basic::run(&target).await
}

#[tokio::test]
async fn test_token_usage_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    suite::basic::run_token_usage(&target).await
}

#[tokio::test]
async fn test_multi_turn_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    suite::basic::run_multi_turn(&target).await
}

#[tokio::test]
async fn test_long_multi_turn_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    suite::basic::run_long_multi_turn(&target).await
}

#[tokio::test]
async fn test_streaming_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "streaming");
    suite::streaming::run(&target).await
}

#[tokio::test]
async fn test_streaming_tools_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    suite::streaming::run_with_tools(&target).await
}

#[tokio::test]
async fn test_tools_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    suite::tools::run(&target).await
}

// `tools::run_complete_flow` was removed when the SDK suite migrated
// off `vercel-ai`'s executable tool registry — full tool execution is
// now covered end-to-end by `cli_deepseek::test_cli_tool_chain_*`.

#[tokio::test]
async fn test_inference_smoke_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    suite::inference_smoke::run(target.provider, &target.model).await
}

// ─── deepseek-anthropic (Anthropic protocol) ──────────────────────────

#[tokio::test]
async fn test_basic_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    suite::basic::run(&target).await
}

#[tokio::test]
async fn test_token_usage_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    suite::basic::run_token_usage(&target).await
}

#[tokio::test]
async fn test_multi_turn_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    suite::basic::run_multi_turn(&target).await
}

#[tokio::test]
async fn test_long_multi_turn_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    suite::basic::run_long_multi_turn(&target).await
}

#[tokio::test]
async fn test_streaming_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "streaming");
    suite::streaming::run(&target).await
}

#[tokio::test]
async fn test_streaming_tools_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "tools");
    suite::streaming::run_with_tools(&target).await
}

#[tokio::test]
async fn test_tools_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "tools");
    suite::tools::run(&target).await
}

#[tokio::test]
async fn test_inference_smoke_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    suite::inference_smoke::run(target.provider, &target.model).await
}

// ─── Cross-protocol parity ────────────────────────────────────────────

#[tokio::test]
async fn test_cross_protocol_deepseek() -> Result<()> {
    let openai = require_live!("deepseek-openai", "cross_protocol");
    let anthropic = require_live!("deepseek-anthropic", "cross_protocol");
    suite::cross_protocol::run(&openai, &anthropic).await
}

/// Continue one accumulated conversation across both DeepSeek API
/// shapes — turns 1 + 3 ride the OpenAI-compat protocol, turn 2
/// rides the Anthropic-shape protocol with the openai assistant
/// reply embedded in history. Validates `LlmMessage`
/// shape parity at the wire level.
#[tokio::test]
async fn test_cross_protocol_session_switch_deepseek() -> Result<()> {
    let openai = require_live!("deepseek-openai", "cross_protocol");
    let anthropic = require_live!("deepseek-anthropic", "cross_protocol");
    suite::cross_protocol::run_session_switch(&openai, &anthropic).await
}

// ─── Token-usage report (alphabetically last) ─────────────────────────

/// Flush the accumulated token-usage report to
/// `tests/live/last-run/sdk_deepseek/`. Runs after every other test
/// because the test runner sorts test names alphabetically and `zzz_`
/// sorts last. No assertions — purely a side-effecting writer.
#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("sdk_deepseek")?;
    Ok(())
}

// ─── Configuration sanity (always runs) ───────────────────────────────

/// Surfaces which providers/credentials are detected so a user running
/// the test binary with `--nocapture` sees what's enabled.
#[test]
fn list_configured_deepseek_providers() {
    let names = ["deepseek-openai", "deepseek-anthropic"];
    let configured: Vec<&str> = names
        .into_iter()
        .filter(|n| common::provider_has_credentials(n))
        .collect();
    eprintln!("[coco-tests-live] DeepSeek providers with credentials: {configured:?}");
}
