use super::*;
use serde_json::json;

fn make_tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall::new(id, name, json!({}))
}

#[test]
fn test_tool_approval_status() {
    let approved = ToolApprovalStatus::Approved;
    assert!(matches!(approved, ToolApprovalStatus::Approved));

    let denied = ToolApprovalStatus::Denied {
        reason: Some("Not allowed".to_string()),
    };
    assert!(matches!(denied, ToolApprovalStatus::Denied { .. }));
}

#[test]
fn test_tool_approval() {
    let approval = ToolApproval::approved("call_1");
    assert!(approval.is_approved());
    assert!(!approval.is_denied());
    assert_eq!(approval.tool_call_id, "call_1");

    let denied = ToolApproval::denied("call_2", Some("Reason".to_string()));
    assert!(!denied.is_approved());
    assert!(denied.is_denied());
}

#[test]
fn test_tool_approval_request() {
    let tc = make_tool_call("id_1", "tool_a");
    let request = ToolApprovalRequest::new(tc).with_description("A test tool");

    assert_eq!(request.tool_call.tool_call_id, "id_1");
    assert_eq!(request.tool_description, Some("A test tool".to_string()));
}

#[tokio::test]
async fn test_auto_approve_collector() {
    let collector = AutoApproveCollector;
    let requests = vec![
        ToolApprovalRequest::new(make_tool_call("id_1", "tool_a")),
        ToolApprovalRequest::new(make_tool_call("id_2", "tool_b")),
    ];

    let approvals = collector.collect_approvals(requests).await.unwrap();

    assert_eq!(approvals.len(), 2);
    assert!(approvals[0].is_approved());
    assert!(approvals[1].is_approved());
}

#[test]
fn test_all_approved() {
    let approvals = vec![
        ToolApproval::approved("id_1"),
        ToolApproval::approved("id_2"),
    ];
    assert!(all_approved(&approvals));

    let approvals = vec![
        ToolApproval::approved("id_1"),
        ToolApproval::denied("id_2", None),
    ];
    assert!(!all_approved(&approvals));
}

#[test]
fn test_get_denied_approvals() {
    let approvals = vec![
        ToolApproval::approved("id_1"),
        ToolApproval::denied("id_2", Some("reason".to_string())),
        ToolApproval::denied("id_3", None),
    ];

    let denied = get_denied_approvals(&approvals);
    assert_eq!(denied.len(), 2);
}

#[test]
fn test_apply_approvals() {
    let tool_calls = vec![
        make_tool_call("id_1", "tool_a"),
        make_tool_call("id_2", "tool_b"),
        make_tool_call("id_3", "tool_c"),
    ];

    let approvals = vec![
        ToolApproval::approved("id_1"),
        ToolApproval::denied("id_2", None),
        ToolApproval::modified(
            "id_3",
            ToolCall::new("id_3", "tool_c_modified", json!({ "modified": true })),
        ),
    ];

    let result = apply_approvals(tool_calls, &approvals);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].tool_name, "tool_a");
    assert_eq!(result[1].tool_name, "tool_c_modified");
}
