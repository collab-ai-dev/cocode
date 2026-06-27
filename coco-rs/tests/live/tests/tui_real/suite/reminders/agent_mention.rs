//! `AgentMention` reminder — fires when the user prompt contains a
//! `@agent-…` token that resolves to a **known active agent**. The
//! mention parser distinguishes agents from file paths via the `agent-`
//! prefix, but `active_agent_mentions` (engine_turn_reminders.rs) then
//! drops any mention whose type isn't in the wired catalog — so the
//! reminder never tells the model to invoke an agent the spawn would
//! reject. We mention `Explore` (an interactive-catalog builtin) so the
//! mention resolves and the reminder fires.

use std::time::Duration;

use anyhow::Result;
use coco_types::AttachmentKind;

use crate::common::reminders;
use crate::tui_real::harness::RealTuiHarness;

/// A canonical built-in agent present in `BuiltinAgentCatalog::interactive()`
/// (which the harness wires), so the mention resolves to an active type.
const AGENT_TYPE: &str = "Explore";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(2)
        .build()
        .await?;

    let prompt =
        format!("Consider routing this to @agent-{AGENT_TYPE} but for now just reply with: ok",);
    harness.submit(&prompt).await;

    let _ = harness.pump_until_idle(Duration::from_secs(60)).await?;

    let history = harness.history_snapshot().await;
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::AgentMention,
        AGENT_TYPE,
        &format!("{provider}/{model} agent_mention"),
    );

    harness.shutdown().await;
    Ok(())
}
