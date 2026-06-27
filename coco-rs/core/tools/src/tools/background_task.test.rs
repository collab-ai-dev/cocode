use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn from_result_prefers_assistant_auto_then_user_then_explicit() {
    assert_eq!(
        BackgroundKind::from_result(&json!({"assistantAutoBackgrounded": true})),
        BackgroundKind::AssistantAuto
    );
    assert_eq!(
        BackgroundKind::from_result(&json!({"backgroundedByUser": true})),
        BackgroundKind::User
    );
    // Auto wins when both flags are set.
    assert_eq!(
        BackgroundKind::from_result(
            &json!({"assistantAutoBackgrounded": true, "backgroundedByUser": true})
        ),
        BackgroundKind::AssistantAuto
    );
    // No flags → explicit `run_in_background: true`.
    assert_eq!(
        BackgroundKind::from_result(&json!({"backgroundTaskId": "t-1"})),
        BackgroundKind::Explicit
    );
}

#[test]
fn explicit_notice_names_id_and_path() {
    assert_eq!(
        format_background_notice(BackgroundKind::Explicit, "t-1", "/cache/t-1.output"),
        "Command running in background with ID: t-1. Output is being written to: /cache/t-1.output"
    );
}

#[test]
fn user_notice_names_id_and_path() {
    assert_eq!(
        format_background_notice(BackgroundKind::User, "t-2", "/cache/t-2.output"),
        "Command was manually backgrounded by user with ID: t-2. Output is being written to: /cache/t-2.output"
    );
}

#[test]
fn assistant_auto_notice_names_budget_id_path_and_delegation() {
    let text = format_background_notice(BackgroundKind::AssistantAuto, "t-3", "/cache/t-3.output");
    assert!(
        text.contains("assistant-mode blocking budget (15s)"),
        "got: {text}"
    );
    assert!(text.contains("with ID: t-3"), "got: {text}");
    assert!(
        text.contains("Output is being written to: /cache/t-3.output"),
        "got: {text}"
    );
    assert!(text.contains("delegate long-running work"), "got: {text}");
}
