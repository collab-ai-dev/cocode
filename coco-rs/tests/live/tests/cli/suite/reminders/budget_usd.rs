//! `BudgetUsd` reminder — fires every turn whenever
//! `QueryEngineConfig::max_budget_usd` is set. No additional gate; on
//! by default in `AttachmentSettings`.

use anyhow::Result;
use coco_types::AttachmentKind;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(2),
        max_budget_usd: Some(1.0), // generous; we just want the reminder to fire
        ..SessionConfig::default()
    };
    let outcome = run_session(provider, model, cfg, "Reply with one word: ok").await?;

    reminders::assert_reminder_present(
        &outcome.result.final_messages,
        AttachmentKind::BudgetUsd,
        &format!("{provider}/{model} budget_usd"),
    );
    Ok(())
}
