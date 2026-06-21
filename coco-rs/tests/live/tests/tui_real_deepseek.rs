//! Real-LLM TUI integration tests against DeepSeek.
//!
//! Drives the **production TUI bootstrap path** (Cli → headless config
//! build → session_bootstrap → SessionRuntime → late-binds → driver
//! loop → AppState fold → native-surface render) with a real provider HTTP
//! call instead of a scripted model. See `tui_real/mod.rs` for the
//! full pipeline.
//!
//! # Skip behavior
//!
//! Auto-skips when `COCO_LIVE_TEST_DEEPSEEK_OPENAI_MODEL` (or
//! `_DEEPSEEK_ANTHROPIC_MODEL`) is unset, the API key is missing, or
//! the provider / capability is excluded by `COCO_LIVE_TEST_*` gates.
//! Returns `Ok(())` on skip so unconfigured CI stays green.
//!
//! # Capabilities
//!
//! - `text` — `one_shot`, `claude_md`
//! - `tools` — `tool_chain`, `hook_pretooluse`,
//!   `permission_round_trip`, `permission_reject`, `interrupt`
//!
//! # Running
//!
//! ```bash
//! cargo test -p coco-tests-live --test tui_real_deepseek -- --test-threads=1 --nocapture
//! cargo test -p coco-tests-live --test tui_real_deepseek tool_chain
//! ```

mod common;
mod tui_real;

use anyhow::Result;

// ─── deepseek-openai ─────────────────────────────────────────────────

#[tokio::test]
async fn test_tui_real_one_shot_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    tui_real::suite::one_shot::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_tool_chain_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::tool_chain::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_hook_pretooluse_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::hook_pretooluse::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_permission_approve_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::permission_round_trip::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_permission_reject_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::permission_reject::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_claude_md_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    tui_real::suite::claude_md::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_interrupt_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::interrupt::run(target.provider, &target.model).await
}

// ─── Reminder coverage (full TUI prod path) ───────────────────────────

#[tokio::test]
async fn test_tui_real_reminder_at_mention_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    tui_real::suite::reminders::at_mention_file::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_reminder_agent_mention_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    tui_real::suite::reminders::agent_mention::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_reminder_nested_memory_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    tui_real::suite::reminders::nested_memory::run(target.provider, &target.model).await
}

// ─── deepseek-anthropic (smoke set) ──────────────────────────────────

#[tokio::test]
async fn test_tui_real_one_shot_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    tui_real::suite::one_shot::run(target.provider, &target.model).await
}

#[tokio::test]
async fn test_tui_real_tool_chain_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "tools");
    tui_real::suite::tool_chain::run(target.provider, &target.model).await
}

// ─── Token-usage report (alphabetically last) ────────────────────────

#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("tui_real_deepseek")?;
    Ok(())
}
