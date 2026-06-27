use super::*;
use pretty_assertions::assert_eq;

#[test]
fn request_round_trips_camelcase() {
    let req = PlanApprovalRequest {
        from: "alice".into(),
        timestamp: "2026-04-19T10:00:00Z".into(),
        plan_file_path: "/tmp/plans/abc.md".into(),
        plan_content: "# plan".into(),
        request_id: "r-1".into(),
    };
    let serialized = serde_json::to_string(&PlanApprovalMessage::PlanApprovalRequest(req.clone()))
        .expect("serialize");
    // Wire format is camelCase.
    assert!(serialized.contains("\"planFilePath\""));
    assert!(serialized.contains("\"requestId\""));
    assert!(serialized.contains("\"planContent\""));
    assert!(serialized.contains("\"type\":\"plan_approval_request\""));

    let parsed: PlanApprovalMessage = serde_json::from_str(&serialized).expect("deserialize");
    match parsed {
        PlanApprovalMessage::PlanApprovalRequest(p) => {
            assert_eq!(p.from, req.from);
            assert_eq!(p.request_id, req.request_id);
            assert_eq!(p.plan_file_path, req.plan_file_path);
        }
        _ => panic!("expected PlanApprovalRequest"),
    }
}

#[test]
fn response_accepts_snake_case_and_camelcase_aliases() {
    // Tests can write either snake or camel — we accept both via alias.
    let snake = r#"{"type":"plan_approval_response","request_id":"r-2","approved":true,"permission_mode":"accept_edits"}"#;
    let camel = r#"{"type":"plan_approval_response","requestId":"r-2","approved":true,"permissionMode":"accept_edits"}"#;
    for input in [snake, camel] {
        let parsed: PlanApprovalMessage = serde_json::from_str(input).expect("deserialize");
        match parsed {
            PlanApprovalMessage::PlanApprovalResponse(r) => {
                assert_eq!(r.request_id, "r-2");
                assert!(r.approved);
                assert_eq!(
                    r.permission_mode,
                    Some(coco_types::PermissionMode::AcceptEdits)
                );
            }
            _ => panic!("expected PlanApprovalResponse"),
        }
    }
}

#[test]
fn response_with_rejection_feedback() {
    let body = r#"{"type":"plan_approval_response","request_id":"r-3","approved":false,"feedback":"refine the security section"}"#;
    let parsed: PlanApprovalMessage = serde_json::from_str(body).expect("deserialize");
    match parsed {
        PlanApprovalMessage::PlanApprovalResponse(r) => {
            assert!(!r.approved);
            assert_eq!(r.feedback.as_deref(), Some("refine the security section"));
            assert_eq!(r.permission_mode, None);
        }
        _ => panic!("expected PlanApprovalResponse"),
    }
}
