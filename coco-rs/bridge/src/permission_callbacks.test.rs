use serde_json::json;

use super::BridgeDecision;
use super::BridgePermissionRequest;
use super::BridgePermissionResponse;
use super::BridgeRiskLevel;

#[test]
fn request_roundtrips_through_json() {
    let request = BridgePermissionRequest {
        id: "req-1".into(),
        tool_name: "Bash".into(),
        description: "Run tests".into(),
        tool_use_id: "tc-1".into(),
        input: json!({"command": "cargo test"}),
        risk: Some(BridgeRiskLevel::High),
        show_always_allow: true,
    };
    let json = serde_json::to_string(&request).unwrap();
    let back: BridgePermissionRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(request, back);
}

#[test]
fn response_roundtrips_through_json() {
    let response = BridgePermissionResponse {
        id: "req-1".into(),
        decision: BridgeDecision::Approved,
        reason: Some("I reviewed it".into()),
        always_allow: true,
    };
    let json = serde_json::to_string(&response).unwrap();
    let back: BridgePermissionResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(response, back);
}

#[test]
fn decision_serializes_lowercase() {
    let approved = serde_json::to_string(&BridgeDecision::Approved).unwrap();
    let rejected = serde_json::to_string(&BridgeDecision::Rejected).unwrap();
    assert_eq!(approved, "\"approved\"");
    assert_eq!(rejected, "\"rejected\"");
}

#[test]
fn risk_levels_serialize_lowercase() {
    assert_eq!(
        serde_json::to_string(&BridgeRiskLevel::Low).unwrap(),
        "\"low\""
    );
    assert_eq!(
        serde_json::to_string(&BridgeRiskLevel::Medium).unwrap(),
        "\"medium\""
    );
    assert_eq!(
        serde_json::to_string(&BridgeRiskLevel::High).unwrap(),
        "\"high\""
    );
}
