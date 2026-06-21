use serde_json::json;
use tempfile::tempdir;

use super::*;

#[test]
fn test_normalize_observable_tool_input_non_exit_tool_unchanged() {
    let input = json!({"file_path": "/tmp/a"});
    let normalized = normalize_observable_tool_input(
        "Read",
        input.clone(),
        ToolInputNormalizationContext::default(),
    );

    assert_eq!(normalized, input);
}

#[test]
fn test_normalize_observable_tool_input_exit_does_not_inject_plan_and_path() {
    let tmp = tempdir().unwrap();
    let plans_dir = tmp.path().join("plans");
    let session_id = "session-with-plan";
    coco_context::write_plan(session_id, &plans_dir, "## Plan\n- ship it", None).unwrap();

    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::ExitPlanMode.as_str(),
        json!({"outcome": "implementation_plan", "allowedPrompts": []}),
        ToolInputNormalizationContext::default(),
    );

    assert_eq!(normalized, json!({"allowedPrompts": []}));
}

#[test]
fn test_normalize_observable_tool_input_exit_strips_stale_internal_fields() {
    let tmp = tempdir().unwrap();
    let plans_dir = tmp.path().join("plans");
    let session_id = "session-stale-plan";
    coco_context::write_plan(session_id, &plans_dir, "fresh plan", Some("agent-1")).unwrap();

    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::ExitPlanMode.as_str(),
        json!({
            "outcome": "implementation_plan",
            "plan": "stale",
            "planFilePath": "/tmp/stale.md",
            "user_choice": "yes-accept-edits-clear-context"
        }),
        ToolInputNormalizationContext::default(),
    );

    assert_eq!(normalized, json!({}));
}

#[test]
fn test_normalize_observable_tool_input_exit_without_plan_unchanged() {
    let input = json!({"outcome": "implementation_plan", "allowedPrompts": []});

    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::ExitPlanMode.as_str(),
        input,
        ToolInputNormalizationContext::default(),
    );

    assert_eq!(normalized, json!({"allowedPrompts": []}));
}

#[test]
fn test_normalize_observable_tool_input_exit_no_plan_skips_stale_disk_plan() {
    let tmp = tempdir().unwrap();
    let plans_dir = tmp.path().join("plans");
    let session_id = "session-no-plan-outcome";
    coco_context::write_plan(session_id, &plans_dir, "old plan", None).unwrap();
    let input = json!({"outcome": "no_implementation_plan"});

    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::ExitPlanMode.as_str(),
        input,
        ToolInputNormalizationContext::default(),
    );

    assert_eq!(normalized, json!({}));
}

#[test]
fn test_bash_strips_cd_cwd_prefix() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::Bash.as_str(),
        json!({"command": "cd /repo && ls -la"}),
        ToolInputNormalizationContext { cwd: Some("/repo") },
    );
    assert_eq!(normalized, json!({"command": "ls -la"}));
}

#[test]
fn test_bash_skips_strip_when_cwd_unset() {
    let input = json!({"command": "cd /repo && ls -la"});
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::Bash.as_str(),
        input.clone(),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(normalized, input);
}

#[test]
fn test_bash_skips_strip_when_prefix_does_not_match() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::Bash.as_str(),
        json!({"command": "cd /other && ls"}),
        ToolInputNormalizationContext { cwd: Some("/repo") },
    );
    assert_eq!(normalized, json!({"command": "cd /other && ls"}));
}

#[test]
fn test_bash_rewrites_double_backslash_semicolon() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::Bash.as_str(),
        json!({"command": r"find . -name '*.tmp' -exec rm {} \\;"}),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(
        normalized,
        json!({"command": r"find . -name '*.tmp' -exec rm {} \;"})
    );
}

#[test]
fn test_task_output_maps_legacy_agent_id() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::TaskOutput.as_str(),
        json!({"agentId": "agent-42", "block": false}),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(normalized, json!({"task_id": "agent-42", "block": false}));
}

#[test]
fn test_task_output_maps_legacy_bash_id() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::TaskOutput.as_str(),
        json!({"bash_id": "bash-7"}),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(normalized, json!({"task_id": "bash-7"}));
}

#[test]
fn test_task_output_existing_task_id_wins() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::TaskOutput.as_str(),
        json!({"task_id": "modern", "agentId": "ignored"}),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(
        normalized,
        json!({"task_id": "modern", "agentId": "ignored"})
    );
}

#[test]
fn test_task_output_wait_up_to_to_timeout_ms() {
    let normalized = normalize_observable_tool_input(
        coco_types::ToolName::TaskOutput.as_str(),
        json!({"task_id": "t", "wait_up_to": 15}),
        ToolInputNormalizationContext::default(),
    );
    assert_eq!(normalized, json!({"task_id": "t", "timeout": 15_000}));
}
