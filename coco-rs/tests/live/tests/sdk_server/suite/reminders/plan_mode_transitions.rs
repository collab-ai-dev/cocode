//! `PlanModeExit` (and adjacent transition reminders) — fire when the
//! engine detects a permission-mode transition out of Plan, by
//! comparing the current mode to the previous-turn mode tracked on
//! `ToolAppState`.
//!
//! Setup:
//! 1. Boot in default mode.
//! 2. `control/setPermissionMode` → Plan, run a turn (sees PlanMode).
//! 3. `control/setPermissionMode` → Default, run a turn (sees PlanModeExit).
//!
//! The reminder source is `app_state.has_exited_plan_mode`, set by the
//! plan-reminder side-effect pre-pass when it detects an unannounced
//! Plan→Default transition in history.

use anyhow::Result;
use coco_cli::sdk_server::SdkTransport;
use coco_types::AttachmentKind;
use coco_types::ClientRequestMethod;

use crate::common::reminders;
use crate::sdk_server::harness::build_live_server;
use crate::sdk_server::harness::drive_until_response;
use crate::sdk_server::harness::req;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let server = build_live_server(provider, model).await?;

    let _ = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    // Switch into Plan mode and run a turn — installs PlanMode reminder
    // and marks the session as plan-active.
    server
        .client
        .send(req(
            300,
            ClientRequestMethod::SetPermissionMode.as_str(),
            serde_json::json!({ "mode": "plan" }),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send setPermissionMode plan: {e:?}"))?;
    let _ = drive_until_response(&server.client, 300, std::time::Duration::from_secs(10)).await?;

    let _ = send_turn(&server, 301, "Reply with one word: ok").await?;

    // Now switch back to default and run another turn — engine should
    // emit PlanModeExit on this turn (the unannounced Plan→Default
    // transition is detected by the plan-reminder side-effect pass).
    server
        .client
        .send(req(
            302,
            ClientRequestMethod::SetPermissionMode.as_str(),
            serde_json::json!({ "mode": "default" }),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send setPermissionMode default: {e:?}"))?;
    let _ = drive_until_response(&server.client, 302, std::time::Duration::from_secs(10)).await?;

    let _ = send_turn(&server, 303, "Reply with one word: yes").await?;

    let history = server.history_snapshot().await;

    // Both reminders should be present in the cumulative history
    // (PlanMode from turn 1, PlanModeExit from turn 2).
    reminders::assert_reminder_present(
        &history,
        AttachmentKind::PlanMode,
        &format!("{provider}/{model} plan_mode (pre-exit)"),
    );
    reminders::assert_reminder_present(
        &history,
        AttachmentKind::PlanModeExit,
        &format!("{provider}/{model} plan_mode_exit"),
    );

    server.shutdown().await;
    Ok(())
}
