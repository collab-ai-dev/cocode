use coco_types::HookCallbackResult;
use coco_types::HookDecision;
use coco_types::HookPermissionDecision;
use coco_types::HookSpecificOutput;
use coco_types::SdkHookOutput;
use pretty_assertions::assert_eq;

/// PreToolUse deny round-trips through the `hookSpecificOutput`
/// shape that orchestration's `aggregate_results_for_event` understands.
#[test]
fn pre_tool_use_deny_round_trips_through_hook_specific_output() {
    let result = HookCallbackResult {
        output: SdkHookOutput {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: Some(HookPermissionDecision::Deny),
                permission_decision_reason: Some("sdk denied".into()),
                updated_input: None,
                additional_context: None,
            }),
            ..Default::default()
        },
    };

    let wire = serde_json::to_value(&result).unwrap();
    // SDK-canonical wire shape: `{output}`, where
    // `output.hookSpecificOutput.hookEventName` discriminates.
    let specific = &wire["output"]["hookSpecificOutput"];
    assert_eq!(specific["hookEventName"], "PreToolUse");
    assert_eq!(specific["permissionDecision"], "deny");
    assert_eq!(specific["permissionDecisionReason"], "sdk denied");

    // Round-trip is lossless: parsing the wire JSON back recovers
    // the typed enum, not a string.
    let parsed: HookCallbackResult = serde_json::from_value(wire).unwrap();
    match parsed.output.hook_specific_output.unwrap() {
        HookSpecificOutput::PreToolUse {
            permission_decision,
            ..
        } => {
            assert_eq!(permission_decision, Some(HookPermissionDecision::Deny));
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

/// Top-level `continue: false` propagates through SdkHookOutput as
/// a sync-mode stop signal for hooks that want to halt the loop
/// without using `hookSpecificOutput`.
#[test]
fn top_level_continue_false_serializes_as_async_omitted() {
    let output = SdkHookOutput {
        r#continue: Some(false),
        stop_reason: Some("policy".into()),
        ..Default::default()
    };
    let wire = serde_json::to_value(&output).unwrap();
    // `async` is omitted (sync-mode default) when not set.
    assert!(wire.get("async").is_none());
    assert_eq!(wire["continue"], false);
    assert_eq!(wire["stopReason"], "policy");
}

/// Async hooks carry `async: true` and optionally `asyncTimeout`;
/// every sync field is omitted by serde when None.
#[test]
fn async_hook_serializes_async_discriminator() {
    let output = SdkHookOutput {
        r#async: Some(true),
        async_timeout: Some(5_000),
        ..Default::default()
    };
    let wire = serde_json::to_value(&output).unwrap();
    assert_eq!(wire["async"], true);
    assert_eq!(wire["asyncTimeout"], 5_000);
    assert!(wire.get("hookSpecificOutput").is_none());
}

/// Top-level `decision: "block"` is wire-canonical (lowercase) for
/// the `HookDecision` enum.
#[test]
fn hook_decision_serializes_lowercase() {
    let output = SdkHookOutput {
        decision: Some(HookDecision::Block),
        reason: Some("nope".into()),
        ..Default::default()
    };
    let wire = serde_json::to_value(&output).unwrap();
    assert_eq!(wire["decision"], "block");
    assert_eq!(wire["reason"], "nope");
}
