use super::*;

#[test]
fn parse_structured_output_ok_true() {
    let result = parse_structured_output(Some(serde_json::json!({"ok": true}))).unwrap();
    assert!(matches!(result, HookEvaluationResult::Ok));
}

#[test]
fn parse_structured_output_ok_false_blocks_with_prefix() {
    let result =
        parse_structured_output(Some(serde_json::json!({"ok": false, "reason": "bad"}))).unwrap();
    match result {
        // Blocking feedback carries the `Agent hook condition was not met: ` prefix.
        HookEvaluationResult::Blocking { reason } => {
            assert_eq!(reason, "Agent hook condition was not met: bad");
        }
        other => panic!("expected Blocking, got {other:?}"),
    }
}

#[test]
fn parse_structured_output_ok_false_no_reason_omits_colon() {
    let result = parse_structured_output(Some(serde_json::json!({"ok": false}))).unwrap();
    match result {
        HookEvaluationResult::Blocking { reason } => {
            assert_eq!(reason, "Agent hook condition was not met");
        }
        other => panic!("expected Blocking, got {other:?}"),
    }
}

#[test]
fn parse_structured_output_missing_output_cancels() {
    let result = parse_structured_output(None).unwrap();
    assert!(matches!(result, HookEvaluationResult::Cancelled));
}

#[test]
fn agent_hook_disallowed_tools_match_ts_set() {
    // Verify the disallowed-tools set for agent hooks.
    for name in [
        ToolName::TaskOutput,
        ToolName::ExitPlanMode,
        ToolName::EnterPlanMode,
        ToolName::Agent,
        ToolName::AskUserQuestion,
        ToolName::TaskStop,
    ] {
        assert!(
            is_agent_hook_disallowed_tool(&ToolId::Builtin(name)),
            "{name:?} must be withheld from hook agents"
        );
    }
    // A representative allowed tool stays available to the verifier.
    assert!(!is_agent_hook_disallowed_tool(&ToolId::Builtin(
        ToolName::Read
    )));
    assert!(!is_agent_hook_disallowed_tool(&ToolId::Builtin(
        ToolName::StructuredOutput
    )));
}
