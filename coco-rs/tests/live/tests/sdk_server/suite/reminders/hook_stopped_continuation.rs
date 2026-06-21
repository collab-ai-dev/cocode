//! `HookStoppedContinuation` reminder — fires when a PostToolUse hook
//! returns `{"continue": false, "stop_reason": "<reason>"}`. The
//! reminder body wraps `<hook_name> hook stopped continuation: <reason>`.
//!
//! We force the agent to issue a Bash tool call, then the PostToolUse
//! hook returns the stop-continuation JSON. The reminder lands in
//! history before the next turn would have run.

use anyhow::Result;
use coco_types::AttachmentKind;
use coco_types::HookEventType;

use crate::common::reminders;
use crate::sdk_server::harness::BuildOptions;
use crate::sdk_server::harness::build_live_server_with_options;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

const STOP_REASON: &str = "🛑 sdk-test policy violation";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-sdk-stop-cont-")?;
    let settings_path = workdir.path().join("settings.json");
    // PostToolUse hook returns the stop-continuation JSON. The
    // orchestrator parses `continue: false` + `stop_reason` and the
    // tool_outcome_builder injects the reminder.
    let payload = serde_json::json!({
        "continue": false,
        "stop_reason": STOP_REASON,
    });
    let payload_str = payload.to_string();
    let cmd = format!("printf %s '{payload_str}'");
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
        AttachmentKind::HookStoppedContinuation,
        STOP_REASON,
        &format!("{provider}/{model} hook_stopped_continuation"),
    );

    server.shutdown().await;
    Ok(())
}
