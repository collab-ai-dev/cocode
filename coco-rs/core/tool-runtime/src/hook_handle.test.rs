use super::HookHandle;
use super::HookPermission;
use super::NoOpHookHandle;
use super::PostToolUseOutcome;
use super::PreToolUseOutcome;
use serde_json::json;

#[test]
fn test_pre_outcome_default_is_not_blocked() {
    let out = PreToolUseOutcome::default();
    assert!(!out.is_blocked());
    assert!(out.updated_input.is_none());
    assert!(out.permission_override.is_none());
    // suppress_output defaults to false — outputs are shown by default.
    assert!(!out.suppress_output);
}

#[test]
fn test_outcomes_have_suppress_output_field() {
    // Regression guard: both outcome types carry suppress_output
    let pre = PreToolUseOutcome {
        suppress_output: true,
        ..Default::default()
    };
    assert!(pre.suppress_output);

    let post = PostToolUseOutcome {
        suppress_output: true,
        ..Default::default()
    };
    assert!(post.suppress_output);
}

#[test]
fn test_pre_outcome_blocked_via_reject() {
    let out = PreToolUseOutcome {
        blocking_reason: Some("hook rejected".into()),
        ..Default::default()
    };
    assert!(out.is_blocked());
}

#[test]
fn test_pre_outcome_blocked_via_deny_override() {
    let out = PreToolUseOutcome {
        permission_override: Some(HookPermission::Deny),
        ..Default::default()
    };
    assert!(out.is_blocked());
}

#[test]
fn test_pre_outcome_allow_override_is_not_blocked() {
    let out = PreToolUseOutcome {
        permission_override: Some(HookPermission::Allow),
        ..Default::default()
    };
    assert!(!out.is_blocked());
}

#[test]
fn test_pre_outcome_ask_override_is_not_blocked() {
    // Ask means "force user prompt", not hard block.
    let out = PreToolUseOutcome {
        permission_override: Some(HookPermission::Ask),
        ..Default::default()
    };
    assert!(!out.is_blocked());
}

#[test]
fn test_post_outcome_default_does_not_interrupt() {
    let out = PostToolUseOutcome::default();
    assert!(!out.should_interrupt());
    assert!(out.updated_output.is_none());
}

#[test]
fn test_post_outcome_prevent_continuation_interrupts() {
    let out = PostToolUseOutcome {
        prevent_continuation: true,
        stop_reason: Some("loop limit reached".into()),
        ..Default::default()
    };
    assert!(out.should_interrupt());
}

#[test]
fn test_post_outcome_blocking_reason_interrupts() {
    let out = PostToolUseOutcome {
        blocking_reason: Some("output rejected by audit hook".into()),
        ..Default::default()
    };
    assert!(out.should_interrupt());
}

#[tokio::test]
async fn test_noop_handle_returns_empty_outcomes() {
    let h = NoOpHookHandle;
    let pre = h
        .run_pre_tool_use("Bash", "tu-1", &json!({"command": "ls"}))
        .await;
    assert!(!pre.is_blocked());
    assert!(pre.additional_contexts.is_empty());

    let post_ok = h
        .run_post_tool_use("Bash", "tu-1", &json!({"command": "ls"}), &json!("output"))
        .await;
    assert!(!post_ok.should_interrupt());

    let post_err = h
        .run_post_tool_use_failure("Bash", "tu-1", &json!({"command": "ls"}), "boom")
        .await;
    assert!(!post_err.should_interrupt());
}
