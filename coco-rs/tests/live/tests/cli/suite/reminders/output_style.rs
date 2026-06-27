//! `OutputStyle` reminder — re-injects the active output-style guideline
//! pointer every turn. Sourced from `SessionBootstrap.output_style`.

use anyhow::Result;
use coco_query::SessionBootstrap;
use coco_types::AttachmentKind;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

const STYLE_NAME: &str = "concise-test-style";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let bootstrap = SessionBootstrap {
        output_style: Some(STYLE_NAME.to_string()),
        ..SessionBootstrap::default()
    };
    let cfg = SessionConfig {
        max_turns: Some(2),
        session_bootstrap: Some(bootstrap),
        ..SessionConfig::default()
    };
    let outcome = run_session(provider, model, cfg, "Reply with one word: ok").await?;

    reminders::assert_reminder_contains(
        &outcome.result.final_messages,
        AttachmentKind::OutputStyle,
        STYLE_NAME,
        &format!("{provider}/{model} output_style"),
    );
    Ok(())
}
