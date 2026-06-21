//! `HookAdditionalContext` reminder via the **SessionStart** path.
//!
//! Engine wiring: `SessionRuntime::fire_session_start_hooks("startup")`
//! runs `coco_hooks::orchestration::execute_session_start`, which pushes
//! a `HookEvent::AdditionalContext` onto the shared
//! `SyncHookEventBuffer` whenever a hook returns non-empty
//! `additional_context`. The first turn's reminder pass drains the
//! buffer through `CombinedHookEventsSource` and the
//! `HookAdditionalContextGenerator` renders it as a
//! `<system-reminder>` that the model sees.
//!
//! TS parity: `processSessionStartHooks('startup')`
//! (`utils/sessionStart.ts:130-175`) emits
//! `createAttachmentMessage({type: 'hook_additional_context',
//! hookEvent: 'SessionStart', content})`.

use anyhow::Result;
use coco_types::AttachmentKind;
use coco_types::HookEventType;

use crate::common::reminders;
use crate::sdk_server::harness::BuildOptions;
use crate::sdk_server::harness::build_live_server_with_options;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

const NEEDLE: &str = "GIRAFFE-session-start-9281";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-sdk-hook-session-start-")?;
    let settings_path = workdir.path().join("settings.json");
    let body = serde_json::json!({
        "hooks": {
            HookEventType::SessionStart.as_str(): [{
                "type": "command",
                // Plain stdout becomes `additionalContext` after parsing.
                "command": format!("printf %s '{NEEDLE}'"),
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

    // One short turn — enough to drain the SessionStart reminder.
    let _ = send_turn(&server, 200, "Reply with one word: ok").await;

    let history = server.history_snapshot().await;
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::HookAdditionalContext,
        NEEDLE,
        &format!("{provider}/{model} hook_session_start"),
    );

    server.shutdown().await;
    Ok(())
}
