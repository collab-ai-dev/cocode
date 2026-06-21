//! `SkillListing` reminder via the **runtime SkillsSource** path.
//!
//! Engine wiring: `SessionRuntime::wire_engine` installs the session's
//! `Arc<SkillManager>` as `ReminderSources.skills`. The
//! `SkillListingGenerator` calls `SkillsSource::listing(...)` which
//! returns `Some("- name: desc\n…")` whenever the manager isn't empty,
//! and the reminder pipeline injects the listing once per session.
//!
//! TS parity: `getSkillListingAttachments(ctx)`
//! (`utils/attachments.ts:875`) reads `ctx.skillManager`. The
//! complementary `SessionBootstrap.skills` path (covered in the CLI
//! suite) drives the same reminder kind through a different source —
//! both must work.

use std::fs;

use anyhow::Result;
use coco_types::AttachmentKind;

use crate::common::reminders;
use crate::sdk_server::harness::BuildOptions;
use crate::sdk_server::harness::build_live_server_with_options;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

const SKILL_NAME: &str = "test-skill-runtime-foo";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-sdk-skill-listing-")?;
    let skill_dir = workdir.path().join(".coco").join("skills").join(SKILL_NAME);
    fs::create_dir_all(&skill_dir)?;
    // `parse_skill_markdown` (skills/src/lib.rs) requires `# Name`
    // FIRST then frontmatter. Wrong order → "expected `# Name` heading
    // as first non-empty line, got: ---".
    let skill_md = format!(
        "# {SKILL_NAME}\n---\ndescription: Marker skill for runtime listing test\n---\n\nbody.\n",
    );
    fs::write(skill_dir.join("SKILL.md"), skill_md)?;

    let server = build_live_server_with_options(
        provider,
        model,
        BuildOptions {
            cwd: Some(workdir),
            settings_path: None,
        },
    )
    .await?;

    let _ = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;
    let _ = send_turn(&server, 200, "Reply with one word: ok").await;

    let history = server.history_snapshot().await;
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::SkillListing,
        SKILL_NAME,
        &format!("{provider}/{model} skill_listing_runtime"),
    );

    server.shutdown().await;
    Ok(())
}
