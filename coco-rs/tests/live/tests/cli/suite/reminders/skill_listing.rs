//! `SkillListing` reminder — listing-only path, sourced from
//! `SessionBootstrap.skills` when the engine has no `SkillsSource` wired.
//! Fires once per session as a names-only listing.
//!
//! The richer "skill body content" path goes through `SkillsSource` which
//! requires the SDK / TUI's full session runtime — that's covered in the
//! SDK suite. Here we just prove the listing-only path is live.

use anyhow::Result;
use coco_query::SessionBootstrap;
use coco_types::AttachmentKind;

use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;
use crate::common::reminders;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let bootstrap = SessionBootstrap {
        skills: vec!["test-skill-foo".to_string(), "test-skill-bar".to_string()],
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
        AttachmentKind::SkillListing,
        "test-skill-foo",
        &format!("{provider}/{model} skill_listing"),
    );
    Ok(())
}
