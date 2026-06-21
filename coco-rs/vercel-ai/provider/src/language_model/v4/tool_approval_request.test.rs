use super::*;

#[test]
fn test_tool_approval_request_new() {
    let req = LanguageModelV4ToolApprovalRequest::new("approval-1", "call-1");
    assert_eq!(req.approval_id, "approval-1");
    assert_eq!(req.tool_call_id, "call-1");
    assert!(req.provider_metadata.is_none());
}

#[test]
fn test_tool_approval_request_serialization() {
    let req = LanguageModelV4ToolApprovalRequest::new("approval-1", "call-1");
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains(r#""approvalId":"approval-1"#));
    assert!(json.contains(r#""toolCallId":"call-1"#));
}

#[test]
fn test_tool_approval_request_deserialization() {
    let req: LanguageModelV4ToolApprovalRequest = serde_json::from_str(
        r#"{"type":"tool-approval-request","approvalId":"app-1","toolCallId":"tc-1"}"#,
    )
    .unwrap();
    assert_eq!(req.approval_id, "app-1");
    assert_eq!(req.tool_call_id, "tc-1");
}
