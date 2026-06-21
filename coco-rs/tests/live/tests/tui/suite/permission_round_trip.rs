//! End-to-end permission approval round-trip.
//!
//! Wires the production `TuiPermissionBridge` into the engine and
//! drives the `PermissionDecision::Ask → ApprovalRequired event →
//! resolve_pending` flow that the real TUI uses.
//!
//! ## Forcing the Ask
//!
//! Builtin tools (Bash/Read/Write/...) return `ToolCheckResult::Passthrough`
//! from `Tool::check_permissions` for ordinary inputs, and the central
//! evaluator (`coco_permissions::PermissionEvaluator`) — wired into
//! `tool_call_preparer::resolve_permission_decision` — also resolves
//! to `Allow` when no rule matches and the mode is Default. To force
//! an `Ask` without seeding session rules, this test uses a
//! **PreToolUse hook** that emits `{"permission_decision": "ask"}`;
//! `tool_call_preparer.rs:247` translates that into
//! `PermissionDecision::Ask` ahead of the evaluator, so the rule
//! plumbing is exercised independently elsewhere
//! (`core/permissions/src/evaluate.test.rs` covers rule pipeline
//! semantics).
//!
//! ## Round-trip
//!
//! 1. Tool call → PreToolUse hook fires, returns `ask`.
//! 2. `tool_call_preparer::resolve_decision` builds a
//!    `PermissionDecision::Ask`.
//! 3. `permission_controller::resolve_ask` calls
//!    `bridge.request_permission(req)`. The bridge (production
//!    `TuiPermissionBridge`) (a) registers a `oneshot::Sender` keyed by
//!    `request_id`, (b) emits `TuiOnlyEvent::ApprovalRequired` on
//!    `event_tx`, then (c) awaits the oneshot.
//! 4. Test pumps until that event lands (folding events into AppState
//!    on the way), then routes `approve` / `reject` to the bridge —
//!    same code path as `tui_runner::resolve_pending`.
//! 5. Engine resumes inside the same turn and finishes.
//!
//! Two scenarios run:
//! - Approve path: tool actually executes, file lands on disk.
//! - Reject path: tool is short-circuited with a denial output,
//!   user feedback round-trips into the rejection text.

use std::time::Duration;

use anyhow::Result;
use coco_hooks::HookDefinition;
use coco_hooks::HookHandler;
use coco_types::HookEventType;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

/// Build a PreToolUse hook that always returns `ask` via stdout JSON.
/// The hook orchestrator parses `permission_decision` (and the nested
/// `hookSpecificOutput.permissionDecision`) into `PermissionBehavior::Ask`.
fn force_ask_hook() -> HookDefinition {
    HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo '{\"permission_decision\":\"ask\"}'".into(),
            timeout_ms: Some(5_000),
            shell: None,
        },
        priority: 0,
        scope: Default::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }
}

pub async fn run() -> Result<()> {
    approve_path().await?;
    reject_path().await?;
    Ok(())
}

async fn approve_path() -> Result<()> {
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-perm-ok-")
        .tempdir_in("/tmp")?;
    let marker_path = workdir.path().join("approved-marker.txt");
    let marker_str = marker_path.to_string_lossy().into_owned();

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        // PreToolUse hook returns `ask` → engine routes through bridge.
        .with_hooks([force_ask_hook()])
        .with_replies([
            Reply::text_then_tool(
                "I'll write the marker once you approve.",
                "call_bash_perm_ok",
                "Bash",
                json!({
                    "command": format!("echo approved > {marker_str}"),
                    "description": "write approved marker",
                }),
            ),
            Reply::text("approved and written"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("write the marker").await;

    // Pump until the bridge surfaces the request. If this returns Err,
    // the engine completed without ever asking — that's a setup bug
    // (e.g. mode/tool combo doesn't trigger Ask), not a behavior bug.
    let req = harness
        .pump_until_approval_request(Duration::from_secs(15))
        .await?;
    assert_eq!(
        req.tool_name, "Bash",
        "approve_path: ApprovalRequired tool_name should be Bash, got {:?}",
        req.tool_name,
    );
    assert!(
        req.input_preview.contains("approved-marker.txt"),
        "approve_path: input_preview should mention the target file, got {}",
        req.input_preview,
    );

    // Approve via the same channel `resolve_pending` uses in production.
    let resolved = harness.approve(&req.request_id).await;
    assert!(
        resolved,
        "approve_path: approve() should match the pending oneshot — \
         was the request_id valid?",
    );

    // Engine resumes inside the same turn → tool runs → SessionResult.
    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "approve_path: SessionResult flagged is_error");

    // Side effect landed: the Bash command actually ran post-approval.
    let body = std::fs::read_to_string(&marker_path)
        .map_err(|e| anyhow::anyhow!("read marker {}: {e}", marker_path.display()))?;
    assert!(
        body.contains("approved"),
        "approve_path: marker file body unexpected: {body:?}",
    );

    // Tool completion was clean.
    let completions = harness.tool_completions();
    assert_eq!(
        completions,
        vec![("Bash", false)],
        "approve_path: expected single clean Bash completion, got {completions:?}",
    );

    // Two LLM calls: pre-tool + post-tool wrap-up. (Approval doesn't
    // count — bridge is not the LLM.)
    assert_eq!(
        harness.model.call_count(),
        2,
        "approve_path: expected 2 LLM calls (pre + post tool), got {}",
        harness.model.call_count(),
    );

    harness.shutdown().await;
    Ok(())
}

async fn reject_path() -> Result<()> {
    let workdir = tempfile::Builder::new()
        .prefix("coco-tests-tui-perm-no-")
        .tempdir_in("/tmp")?;
    let marker_path = workdir.path().join("rejected-marker.txt");
    let marker_str = marker_path.to_string_lossy().into_owned();

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_hooks([force_ask_hook()])
        .with_replies([
            Reply::tool_call(
                "call_bash_perm_no",
                "Bash",
                json!({
                    "command": format!("echo should-not-run > {marker_str}"),
                    "description": "should be rejected",
                }),
            ),
            // Post-rejection turn: the engine still gets to compose a
            // follow-up reply with the denial-as-tool-result in history.
            Reply::text("understood, skipped"),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("try to run something risky").await;

    let req = harness
        .pump_until_approval_request(Duration::from_secs(15))
        .await?;
    assert_eq!(req.tool_name, "Bash");

    let feedback = "user denied: too risky";
    let resolved = harness
        .reject(&req.request_id, Some(feedback.to_string()))
        .await;
    assert!(
        resolved,
        "reject_path: reject() should match the pending oneshot"
    );

    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "reject_path: SessionResult flagged is_error");

    // Side-effect check: the Bash command must NOT have run. The
    // engine short-circuits Ask→Reject before tool dispatch, so the
    // file should not exist on disk.
    assert!(
        !marker_path.exists(),
        "reject_path: marker file should NOT exist — rejection should \
         short-circuit the tool dispatch",
    );

    // The tool's completion event surfaces with `is_error = true` —
    // permission_controller's `complete_tool_call_with_error` wired
    // the denial output through the runtime's tool-call completion
    // path. Output should mention the rejection reason.
    let completions = harness.tool_completions();
    assert_eq!(
        completions.len(),
        1,
        "reject_path: expected exactly one tool completion event, got {completions:?}",
    );
    let (name, is_error) = &completions[0];
    assert_eq!(*name, "Bash");
    assert!(
        *is_error,
        "reject_path: rejected Bash should complete with is_error=true",
    );

    // The denied Bash result captures the user's feedback so the next
    // turn (and rendered transcript) can see why it was denied. The
    // engine surfaces this as a tool-result cell with `is_error=true`.
    let saw_feedback = harness
        .find_tool_result("Bash")
        .is_some_and(|(out, _)| out.contains(feedback));
    assert!(
        saw_feedback,
        "reject_path: rejection feedback `{feedback}` should appear in chat \
         (got cells: {:?})",
        harness.text_cells_in_order(),
    );

    harness.shutdown().await;
    Ok(())
}
