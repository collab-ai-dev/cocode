//! `HookSuccess` reminder via the **UserPromptSubmit** path.
//!
//! Engine wiring: `QueryEngineRunner::run_turn` calls
//! `runtime.fire_user_prompt_submit_hooks(prompt)` BEFORE building the
//! per-turn engine. Each successful hook with non-empty stdout pushes a
//! `HookEvent::Success { hook_event: UserPromptSubmit, ... }` onto the
//! sync hook buffer; the first reminder pass drains the buffer and the
//! `HookSuccessGenerator` renders it.
//!
//! TS parity: `executeUserPromptSubmitHooks` in
//! `utils/processUserInput/processUserInput.ts:182-263`. The
//! `hook_success` render gate (TS `messages.ts:4099-4115`
//! `normalizeAttachmentForAPI`) only emits text for SessionStart /
//! UserPromptSubmit; this test exercises the latter.

use anyhow::Result;
use coco_types::AttachmentKind;
use coco_types::HookEventType;

use crate::common::reminders;
use crate::sdk_server::harness::BuildOptions;
use crate::sdk_server::harness::build_live_server_with_options;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

const NEEDLE: &str = "OWL-user-prompt-submit-7411";

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-sdk-hook-uprompt-")?;
    let settings_path = workdir.path().join("settings.json");
    // Use structured success JSON so the aggregator records this as a
    // `HookSuccess` (non-empty content). Plain stdout would land under
    // `additional_context` instead.
    let payload = serde_json::json!({
        "continue": true,
        "systemMessage": NEEDLE,
    });
    let cmd = format!("printf %s '{}'", payload.to_string().replace('\'', "'\\''"));
    let body = serde_json::json!({
        "hooks": {
            HookEventType::UserPromptSubmit.as_str(): [{
                "type": "command",
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

    let _ = send_turn(&server, 200, "Reply with one word: ok").await;

    let history = server.history_snapshot().await;
    // `hook_success` body is rendered by HookSuccessGenerator —
    // assert by AttachmentKind presence (the body content is the raw
    // hook stdout, which contains NEEDLE).
    reminders::assert_reminder_contains(
        &history,
        AttachmentKind::HookSuccess,
        NEEDLE,
        &format!("{provider}/{model} hook_user_prompt_submit"),
    );

    server.shutdown().await;
    Ok(())
}
