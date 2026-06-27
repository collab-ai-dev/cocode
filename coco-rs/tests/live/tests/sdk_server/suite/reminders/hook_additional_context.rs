//! `HookAdditionalContext` reminder via the **PostToolUse** path.
//!
//! Engine wiring: `tool_outcome_builder::render_hook_context_messages`
//! injects `HookAdditionalContext` reminders for every PostToolUse hook
//! that returns a non-empty `additionalContext` (or `additional_context`)
//! field. The synchronous SessionStart / UserPromptSubmit path doesn't
//! surface hook events through `HookEventsSource` today
//! (`coco-hooks/src/reminder_source.rs` documents this scope), so we
//! drive the assertion through the post-tool path which is wired.

use anyhow::Result;
use coco_types::AttachmentKind;
use coco_types::HookEventType;

use crate::common::reminders;
use crate::sdk_server::harness::BuildOptions;
use crate::sdk_server::harness::build_live_server_with_options;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

const CONTEXT_NEEDLE: &str = "TURTLE-context-1284-marker";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-sdk-hook-addctx-")?;
    let settings_path = workdir.path().join("settings.json");
    let payload = serde_json::json!({
        "additionalContext": CONTEXT_NEEDLE,
    });
    let payload_str = payload.to_string();
    let cmd = format!("printf %s '{payload_str}'");
    // PostToolUse hook on Bash → emits the additionalContext into
    // tool_outcome_builder, which calls render_hook_context_messages
    // and injects the HookAdditionalContext reminder.
    let body = serde_json::json!({
        "hooks": {
            HookEventType::PostToolUse.as_str(): [{
                "type": "command",
                "matcher": "Bash",
                "command": cmd,
                "timeout": 5,
            }]
        }
    });
    std::fs::write(&settings_path, serde_json::to_vec_pretty(&body)?)?;

    let server = build_live_server_with_options(
        provider,
        model,
        BuildOptions {
            cwd: Some(workdir),
            settings_path: Some(settings_path),
        },
    )
    .await?;

    let _ = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    let prompt = "You MUST start with one Bash tool call to run `echo hello`. \
                  Then reply with exactly the word `done`.";
    let _ = send_turn(&server, 200, prompt).await;

    let history = server.history_snapshot().await;
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::HookAdditionalContext,
        CONTEXT_NEEDLE,
        &format!("{provider}/{model} hook_additional_context"),
    );

    server.shutdown().await;
    Ok(())
}
