//! Permission **rejection** round trip.
//!
//! Same setup as `permission_round_trip`, but the test rejects the
//! approval. The engine should short-circuit the tool dispatch and
//! surface the rejection as a tool-error completion. The Bash command
//! must NOT execute.

use std::time::Duration;

use anyhow::Result;
use coco_types::HookEventType;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-perm-no-")?;
    let workdir_path = workdir.path().to_path_buf();
    let marker_path = workdir_path.join("should-not-exist.txt");
    let marker_str = marker_path.to_string_lossy().into_owned();

    let settings_path = workdir_path.join("settings.json");
    let settings_body = serde_json::json!({
        "hooks": {
            HookEventType::PreToolUse.as_str(): [{
                "type": "command",
                "matcher": "Bash",
                "command": "echo '{\"permission_decision\":\"ask\"}'",
                "timeout": 5,
            }]
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&settings_body).expect("settings.json"),
    )?;

    let mut harness = RealTuiHarness::builder()
        .with_provider(provider)
        .with_model(model)
        .with_max_turns(6)
        .with_bypass_permissions(false)
        .with_settings_path(settings_path)
        .with_workdir(workdir)
        .build()
        .await?;

    // Imperative phrasing: tell the model to issue the tool call
    // unconditionally. "If denied" wording can confuse some models
    // into skipping the call entirely. We test the rejection path
    // by having the test reject — not by asking the model to gauge
    // user sentiment.
    let prompt = format!(
        "You MUST start by issuing exactly one Bash tool call to run \
         `echo nope > {marker_str}`. Do not skip this call. After the tool \
         result comes back (whether successful or denied), reply with \
         exactly one word — `acknowledged` — and stop."
    );
    harness.submit(&prompt).await;

    let req = harness
        .pump_until_approval_request(Duration::from_secs(60))
        .await?;
    assert_eq!(req.tool_name, "Bash");

    let feedback = "user denied: too risky";
    let resolved = harness
        .reject(&req.request_id, Some(feedback.to_string()))
        .await;
    assert!(
        resolved,
        "{provider}/{model}: reject() should match the pending oneshot",
    );

    // The session may end with `is_error=true` here — that's actually
    // correct behavior. Real models, on receiving a tool-error, often
    // retry, hit the same hook, get denied again, and the engine
    // eventually surfaces that retry pattern as a non-clean session.
    // What we care about is the *behavior* of the rejection: the
    // command didn't run, the engine emitted a tool-error completion,
    // and the feedback round-tripped into the message history.
    let _is_clean = harness.pump_until_idle(Duration::from_secs(90)).await?;

    // Side-effect check: the Bash command must NOT have run. The
    // engine short-circuits Ask→Reject before dispatch.
    assert!(
        !marker_path.exists(),
        "{provider}/{model}: marker {} should NOT exist after reject",
        marker_path.display(),
    );

    // The tool's completion must surface with `is_error == true`
    // (permission_controller wires the denial output through the
    // runtime's tool-call completion path).
    let completions = harness.tool_completions();
    let bash_failed = completions.iter().any(|(n, e)| *n == "Bash" && *e);
    assert!(
        bash_failed,
        "{provider}/{model}: rejected Bash should complete with is_error=true; \
         got {completions:?}",
    );

    // Rejection feedback should be captured in the engine transcript
    // so the next turn (and any rendered transcript) can see why it
    // was denied. The harness folds `Message::ToolResult` cells from
    // SessionResult / TurnCompleted notifications.
    let saw_feedback = harness
        .find_tool_result("Bash")
        .is_some_and(|(out, _)| out.contains(feedback));
    assert!(
        saw_feedback,
        "{provider}/{model}: rejection feedback `{feedback}` should appear in chat",
    );

    harness.shutdown().await;
    Ok(())
}
