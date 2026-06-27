use coco_hooks::orchestration::AggregatedHookResult;
use coco_hooks::orchestration::HookBlockingError;
use coco_types::PermissionBehavior;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn test_aggregate_to_pre_outcome_carries_updated_input() {
    let agg = AggregatedHookResult {
        updated_input: Some(json!({"command": "rewritten"})),
        ..Default::default()
    };
    let out = aggregate_to_pre_outcome(&agg);
    assert_eq!(out.updated_input, Some(json!({"command": "rewritten"})));
}

#[test]
fn test_aggregate_to_pre_outcome_maps_permission_behaviors() {
    for (behavior, expected) in [
        (PermissionBehavior::Allow, HookPermission::Allow),
        (PermissionBehavior::Ask, HookPermission::Ask),
        (PermissionBehavior::Deny, HookPermission::Deny),
    ] {
        let agg = AggregatedHookResult {
            permission_behavior: Some(behavior),
            ..Default::default()
        };
        let out = aggregate_to_pre_outcome(&agg);
        assert_eq!(out.permission_override, Some(expected));
    }
}

#[test]
fn test_aggregate_to_pre_outcome_surfaces_blocking_error_reason() {
    let agg = AggregatedHookResult {
        blocking_error: Some(HookBlockingError {
            blocking_error: "rejected by policy".into(),
            source: coco_hooks::orchestration::HookBlockingSource::Command("policy.sh".into()),
        }),
        hook_permission_decision_reason: Some("policy match".into()),
        ..Default::default()
    };
    let out = aggregate_to_pre_outcome(&agg);
    assert_eq!(out.blocking_reason.as_deref(), Some("rejected by policy"));
    assert_eq!(out.permission_reason.as_deref(), Some("policy match"));
    assert!(out.is_blocked());
}

#[test]
fn test_aggregate_to_pre_outcome_copies_context_and_flags() {
    let agg = AggregatedHookResult {
        additional_contexts: vec!["extra one".into(), "extra two".into()],
        system_message: Some("be careful".into()),
        suppress_output: true,
        ..Default::default()
    };
    let out = aggregate_to_pre_outcome(&agg);
    assert_eq!(out.additional_contexts, vec!["extra one", "extra two"]);
    assert_eq!(out.system_message.as_deref(), Some("be careful"));
    assert!(out.suppress_output);
}

#[test]
fn test_aggregate_to_post_outcome_preserves_mcp_output_rewrite() {
    let agg = AggregatedHookResult {
        updated_mcp_tool_output: Some(json!({"redacted": true})),
        prevent_continuation: true,
        stop_reason: Some("policy cut".into()),
        ..Default::default()
    };
    let out = aggregate_to_post_outcome(&agg);
    assert_eq!(out.updated_output, Some(json!({"redacted": true})));
    assert!(out.prevent_continuation);
    assert_eq!(out.stop_reason.as_deref(), Some("policy cut"));
    // Semantic parity: prevent_continuation alone triggers interrupt.
    assert!(out.should_interrupt());
}

#[test]
fn test_aggregate_to_post_outcome_carries_blocking_reason() {
    let agg = AggregatedHookResult {
        blocking_error: Some(HookBlockingError {
            blocking_error: "output flagged".into(),
            source: coco_hooks::orchestration::HookBlockingSource::Command("audit.sh".into()),
        }),
        ..Default::default()
    };
    let out = aggregate_to_post_outcome(&agg);
    assert_eq!(out.blocking_reason.as_deref(), Some("output flagged"));
    assert!(out.should_interrupt());
}
