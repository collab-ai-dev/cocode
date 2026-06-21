use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::CanUseToolCallContext;
use super::CanUseToolDecision;
use super::CanUseToolHandle;
use super::DecisionReason;
use super::NoOpCanUseToolHandle;
use super::SpeculationBoundary;
use super::deny_all_handle;

fn ctx() -> CanUseToolCallContext {
    CanUseToolCallContext {
        tool_use_id: "test-id".into(),
        abort: crate::TurnAbortSignal::from_token(tokio_util::sync::CancellationToken::new()),
        require_can_use_tool: false,
        messages: Arc::new(Vec::new()),
    }
}

#[tokio::test]
async fn test_no_op_returns_ask() {
    let handle = NoOpCanUseToolHandle;
    let decision = handle
        .check("Read", &json!({"path": "/tmp/x"}), &ctx())
        .await;
    match decision {
        CanUseToolDecision::Ask { decision_reason } => match decision_reason {
            DecisionReason::Other { reason } => assert_eq!(reason, "no-op handle"),
            other => panic!("unexpected reason: {other:?}"),
        },
        other => panic!("expected Ask, got {other:?}"),
    }
}

#[tokio::test]
async fn test_deny_all_denies_every_tool() {
    let handle = deny_all_handle("test-only");
    for tool in ["Read", "Write", "Edit", "Bash", "TaskOutput"] {
        let decision = handle.check(tool, &json!({}), &ctx()).await;
        match decision {
            CanUseToolDecision::Deny {
                message,
                decision_reason,
            } => {
                assert!(
                    message.contains("test-only"),
                    "{tool} message should carry reason: {message}"
                );
                match decision_reason {
                    DecisionReason::Other { reason } => assert_eq!(reason, "test-only"),
                    other => panic!("expected Other, got {other:?}"),
                }
            }
            other => panic!("expected Deny for {tool}, got {other:?}"),
        }
    }
}

#[test]
fn test_decision_reason_speculation_carries_boundary() {
    let r = DecisionReason::Speculation {
        boundary: SpeculationBoundary::Bash,
    };
    match r {
        DecisionReason::Speculation { boundary } => {
            assert_eq!(boundary, SpeculationBoundary::Bash);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn test_decision_allow_with_updated_input_round_trip() {
    let original = json!({"file_path": "/main/foo.txt"});
    let rewritten = json!({"file_path": "/overlay/foo.txt"});
    let d = CanUseToolDecision::Allow {
        updated_input: Some(rewritten.clone()),
        decision_reason: DecisionReason::Speculation {
            boundary: SpeculationBoundary::Edit,
        },
    };
    match d {
        CanUseToolDecision::Allow { updated_input, .. } => {
            assert_eq!(updated_input.as_ref(), Some(&rewritten));
            assert_ne!(updated_input.as_ref(), Some(&original));
        }
        other => panic!("expected Allow, got {other:?}"),
    }
}
