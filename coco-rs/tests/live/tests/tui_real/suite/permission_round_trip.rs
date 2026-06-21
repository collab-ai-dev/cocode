//! Real-LLM permission approval round trip through the production
//! `TuiPermissionBridge`.
//!
//! Forcing the Ask: builtin tools' `check_permissions` returns Allow
//! for ordinary inputs, so to make the engine route through the
//! bridge we install a PreToolUse hook on Bash that emits
//! `{"permission_decision":"ask"}` — the orchestrator translates that
//! into `PermissionBehavior::Ask`, which `tool_call_preparer` lifts
//! into `PermissionDecision::Ask`. Same trick the mock suite uses.
//!
//! Round trip:
//! - Real model issues a Bash tool call.
//! - Hook fires Ask.
//! - Bridge emits `ApprovalRequired` on the event channel.
//! - Test pumps until the event lands, then routes `approve()`.
//! - Engine resumes inside the same turn → tool runs → SessionResult.
//!
//! The bypass-permissions flag MUST be off — otherwise the engine
//! short-circuits and never consults the bridge.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use coco_types::HookEventType;

use crate::tui_real::harness::RealTuiHarness;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let workdir = crate::common::tmpdir::make("coco-tests-tui-real-perm-ok-")?;
    let workdir_path = workdir.path().to_path_buf();
    let marker_path = workdir_path.join("approved.txt");
    let marker_str = marker_path.to_string_lossy().into_owned();

    // Hook that forces Ask on Bash.
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
        .with_bypass_permissions(false) // bypass off → bridge consulted
        .with_settings_path(settings_path)
        .with_workdir(workdir)
        // If the model decides to call AskUserQuestion mid-flow (e.g. to
        // clarify after the Bash approval), the harness auto-resolves
        // the second permission request via the same `UserCommand::
        // ApprovalResponse` channel the production TUI overlay uses,
        // splicing first-option answers into `updated_input`. Without
        // this the engine would block in `permission_controller`'s
        // bridge wait until the test-level 60s deadline tripped.
        .with_auto_answer_questions(HashMap::new())
        .build()
        .await?;

    let prompt = format!(
        "Use the Bash tool to run `echo approved > {marker_str}`. \
         After the tool completes, reply with the single word `done`."
    );
    harness.submit(&prompt).await;

    let req = harness
        .pump_until_approval_request(Duration::from_secs(60))
        .await?;
    assert_eq!(
        req.tool_name, "Bash",
        "{provider}/{model}: expected Bash approval, got {:?}",
        req.tool_name
    );
    assert!(
        req.input_preview.contains("approved.txt") || req.input_preview.contains("approved"),
        "{provider}/{model}: input_preview should reference the marker; got {:?}",
        req.input_preview,
    );

    let resolved = harness.approve(&req.request_id).await;
    assert!(
        resolved,
        "{provider}/{model}: approve() should match the pending oneshot",
    );

    let ok = harness.pump_until_idle(Duration::from_secs(60)).await?;
    assert!(ok, "{provider}/{model}: SessionResult flagged is_error");

    // Real side-effect: the Bash command ran post-approval.
    assert!(
        marker_path.exists(),
        "{provider}/{model}: marker {} should exist after approve",
        marker_path.display(),
    );
    let body = std::fs::read_to_string(&marker_path)?;
    assert!(
        body.contains("approved"),
        "{provider}/{model}: marker body unexpected: {body:?}",
    );

    let completions = harness.tool_completions();
    let bash_clean = completions.iter().any(|(n, e)| *n == "Bash" && !*e);
    assert!(
        bash_clean,
        "{provider}/{model}: expected a clean Bash completion; got {completions:?}",
    );

    harness.shutdown().await;
    Ok(())
}
