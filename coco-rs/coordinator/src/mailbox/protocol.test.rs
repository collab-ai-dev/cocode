//! JSON round-trip tests for every `ProtocolMessage` variant + the
//! `is_structured_protocol_message` / `parse_protocol_message`
//! detection helpers.
//!
//! The protocol is wire-stable across teammate sessions (different
//! coco binaries can be reading and writing each other's mailbox
//! files), so the variant tags MUST stay byte-identical to TS. Each
//! test pins the exact `type` literal SDK clients see.

use super::*;
use pretty_assertions::assert_eq;

fn assert_round_trips(json: &str, expected_type: &str) {
    assert!(
        is_structured_protocol_message(json),
        "is_structured_protocol_message must accept: {json}"
    );
    let parsed = parse_protocol_message(json).expect("must parse");
    let serialized = serde_json::to_string(&parsed).expect("must serialise");
    let v: serde_json::Value = serde_json::from_str(&serialized).expect("must round-trip");
    assert_eq!(v["type"].as_str().unwrap(), expected_type);
}

#[test]
fn test_idle_notification_round_trip() {
    let json = r#"{"type":"idle_notification","from":"worker","timestamp":"2026-05-01T00:00:00Z","idleReason":"available","summary":"done"}"#;
    assert_round_trips(json, "idle_notification");
    let m = parse_protocol_message(json).unwrap();
    let ProtocolMessage::IdleNotification {
        from,
        idle_reason,
        summary,
        ..
    } = m
    else {
        panic!("wrong variant");
    };
    assert_eq!(from, "worker");
    assert_eq!(idle_reason.as_deref(), Some("available"));
    assert_eq!(summary.as_deref(), Some("done"));
}

#[test]
fn test_permission_request_round_trip() {
    let json = r#"{"type":"permission_request","request_id":"r1","agent_id":"w@t","tool_name":"Bash","tool_use_id":"tu","description":"rm -rf /","input":{"command":"rm -rf /"}}"#;
    assert_round_trips(json, "permission_request");
}

#[test]
fn test_permission_response_round_trip() {
    let json = r#"{"type":"permission_response","request_id":"r1","subtype":"success","response":{"updated_input":{"path":"/tmp/x"},"permission_updates":[{"type":"addRules","rules":[{"toolName":"Read","ruleContent":"/tmp/**"}],"behavior":"allow","destination":"session"}]}}"#;
    assert_round_trips(json, "permission_response");
}

#[test]
fn test_permission_response_splits_mixed_rule_behaviors() {
    let text = create_permission_response_message_with_payload(
        "r1",
        true,
        None,
        None,
        vec![coco_types::PermissionUpdate::AddRules {
            rules: vec![
                coco_types::PermissionRule {
                    source: coco_types::PermissionRuleSource::Session,
                    behavior: coco_types::PermissionBehavior::Allow,
                    value: coco_types::PermissionRuleValue {
                        tool_pattern: "Read".into(),
                        rule_content: None,
                    },
                },
                coco_types::PermissionRule {
                    source: coco_types::PermissionRuleSource::Session,
                    behavior: coco_types::PermissionBehavior::Deny,
                    value: coco_types::PermissionRuleValue {
                        tool_pattern: "Bash".into(),
                        rule_content: Some("rm *".into()),
                    },
                },
            ],
            destination: coco_types::PermissionUpdateDestination::Session,
        }],
    );
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let updates = value["response"]["permission_updates"].as_array().unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0]["behavior"], "allow");
    assert_eq!(updates[0]["rules"][0]["toolName"], "Read");
    assert_eq!(updates[1]["behavior"], "deny");
    assert_eq!(updates[1]["rules"][0]["toolName"], "Bash");
}

#[test]
fn test_permission_response_rejects_unknown_subtype() {
    let json = r#"{"type":"permission_response","request_id":"r1","subtype":"maybe"}"#;
    assert!(
        parse_protocol_message(json).is_none(),
        "permission_response subtype is a closed TS union"
    );
}

#[test]
fn test_sandbox_permission_request_round_trip() {
    let json = r#"{"type":"sandbox_permission_request","requestId":"sb1","workerId":"w@t","workerName":"worker","hostPattern":{"host":"api.example.com"},"createdAt":1700000000}"#;
    assert_round_trips(json, "sandbox_permission_request");
}

#[test]
fn test_sandbox_permission_response_round_trip() {
    let json = r#"{"type":"sandbox_permission_response","requestId":"sb1","host":"api.example.com","allow":true,"timestamp":"2026-05-01T00:00:00Z"}"#;
    assert_round_trips(json, "sandbox_permission_response");
}

#[test]
fn test_plan_approval_request_round_trip() {
    let json = r#"{"type":"plan_approval_request","from":"worker","timestamp":"t","planFilePath":"","planContent":"plan","requestId":"p1"}"#;
    assert_round_trips(json, "plan_approval_request");
}

#[test]
fn test_plan_approval_response_round_trip_with_feedback() {
    let json = r#"{"type":"plan_approval_response","requestId":"p1","approved":false,"feedback":"missing tests","timestamp":"t","permissionMode":"plan"}"#;
    assert_round_trips(json, "plan_approval_response");
    let m = parse_protocol_message(json).unwrap();
    let ProtocolMessage::PlanApprovalResponse {
        approved,
        feedback,
        permission_mode,
        ..
    } = m
    else {
        panic!("wrong variant");
    };
    assert!(!approved);
    assert_eq!(feedback.as_deref(), Some("missing tests"));
    assert_eq!(permission_mode.as_deref(), Some("plan"));
}

#[test]
fn test_shutdown_request_round_trip() {
    let json = r#"{"type":"shutdown_request","requestId":"s1","from":"team-lead","reason":"team disbanded","timestamp":"t"}"#;
    assert_round_trips(json, "shutdown_request");
}

#[test]
fn test_shutdown_approved_round_trip() {
    let json = r#"{"type":"shutdown_approved","requestId":"s1","from":"worker","timestamp":"t","paneId":"p","backendType":"in-process"}"#;
    assert_round_trips(json, "shutdown_approved");
}

#[test]
fn test_shutdown_rejected_round_trip() {
    let json = r#"{"type":"shutdown_rejected","requestId":"s1","from":"worker","reason":"mid task","timestamp":"t"}"#;
    assert_round_trips(json, "shutdown_rejected");
}

#[test]
fn test_create_shutdown_approved_carries_pane_coords() {
    // Pane-based teammate: pane id + backend round-trip so the leader can
    // kill the right pane.
    let text =
        create_shutdown_approved_message("req-9", "worker-1", Some("%3"), Some(BackendType::Tmux));
    let ProtocolMessage::ShutdownApproved {
        request_id,
        from,
        pane_id,
        backend_type,
        ..
    } = parse_protocol_message(&text).expect("must parse")
    else {
        panic!("wrong variant");
    };
    assert_eq!(request_id, "req-9");
    assert_eq!(from, "worker-1");
    assert_eq!(pane_id.as_deref(), Some("%3"));
    assert_eq!(backend_type, Some(BackendType::Tmux));
}

#[test]
fn test_create_shutdown_approved_empty_pane_is_none() {
    // In-process teammate: empty pane id collapses to None so the leader
    // skips kill_pane and only removes membership.
    let text = create_shutdown_approved_message(
        "req-1",
        "ip-worker",
        Some(""),
        Some(BackendType::InProcess),
    );
    let ProtocolMessage::ShutdownApproved {
        pane_id,
        backend_type,
        ..
    } = parse_protocol_message(&text).expect("must parse")
    else {
        panic!("wrong variant");
    };
    assert_eq!(pane_id, None);
    assert_eq!(backend_type, Some(BackendType::InProcess));
}

#[test]
fn test_task_assignment_round_trip() {
    let json = r#"{"type":"task_assignment","taskId":"t1","subject":"refactor","description":"split agent_handle.rs","assignedBy":"team-lead","timestamp":"t"}"#;
    assert_round_trips(json, "task_assignment");
}

#[test]
fn test_team_permission_update_round_trip() {
    let json = r#"{"type":"team_permission_update","permissionUpdate":{"type":"addRules","rules":[{"toolName":"WebFetch","ruleContent":"/proj/**"}],"behavior":"allow","destination":"session"},"directoryPath":"/proj","toolName":"WebFetch"}"#;
    assert_round_trips(json, "team_permission_update");
}

#[test]
fn test_team_permission_update_uses_ts_wire_fields() {
    let json = r#"{"type":"team_permission_update","permissionUpdate":{"type":"addRules","rules":[{"toolName":"Edit","ruleContent":"/proj/**"}],"behavior":"allow","destination":"session"},"directoryPath":"/proj","toolName":"Edit"}"#;
    let parsed = parse_protocol_message(json).expect("must parse TS team permission update");
    match parsed {
        ProtocolMessage::TeamPermissionUpdate {
            permission_update,
            directory_path,
            tool_name,
        } => {
            assert_eq!(directory_path, "/proj");
            assert_eq!(tool_name, "Edit");
            match permission_update {
                WireTeamPermissionUpdate::AddRules {
                    rules,
                    behavior,
                    destination,
                } => {
                    assert_eq!(rules[0].tool_name, "Edit");
                    assert_eq!(rules[0].rule_content.as_deref(), Some("/proj/**"));
                    assert_eq!(behavior, coco_types::PermissionBehavior::Allow);
                    assert!(matches!(
                        destination,
                        WireTeamPermissionUpdateDestination::Session
                    ));
                }
            }
        }
        other => panic!("expected team permission update, got {other:?}"),
    }
}

#[test]
fn test_team_permission_update_rejects_legacy_snake_case_fields() {
    let json = r#"{"type":"team_permission_update","permission_update":{"type":"add_rules","rules":[{"tool_name":"Edit","rule_content":"/proj/**"}],"behavior":"allow","destination":"session"},"directory_path":"/proj","tool_name":"Edit"}"#;
    assert!(
        parse_protocol_message(json).is_none(),
        "mailbox protocol intentionally accepts only the TS wire shape"
    );
}

#[test]
fn test_team_permission_update_rejects_non_add_rules_update() {
    let json = r#"{"type":"team_permission_update","permissionUpdate":{"type":"removeRules","rules":[{"toolName":"Edit","ruleContent":"/proj/**"}],"behavior":"allow","destination":"session"},"directoryPath":"/proj","toolName":"Edit"}"#;
    assert!(
        parse_protocol_message(json).is_none(),
        "team_permission_update only accepts addRules, not removeRules"
    );
}

#[test]
fn test_team_permission_update_serializes_ts_wire_fields() {
    let message = ProtocolMessage::TeamPermissionUpdate {
        permission_update: WireTeamPermissionUpdate::AddRules {
            rules: vec![WirePermissionRuleValue {
                tool_name: "Edit".to_string(),
                rule_content: Some("/proj/**".to_string()),
            }],
            behavior: coco_types::PermissionBehavior::Allow,
            destination: WireTeamPermissionUpdateDestination::Session,
        },
        directory_path: "/proj".to_string(),
        tool_name: "Edit".to_string(),
    };
    let value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();

    assert_eq!(value["permissionUpdate"]["type"], "addRules");
    assert_eq!(value["permissionUpdate"]["rules"][0]["toolName"], "Edit");
    assert_eq!(
        value["permissionUpdate"]["rules"][0]["ruleContent"],
        "/proj/**"
    );
    assert_eq!(value["directoryPath"], "/proj");
    assert_eq!(value["toolName"], "Edit");
    assert!(value.get("permission_update").is_none());
    assert!(
        value["permissionUpdate"]["rules"][0]
            .get("tool_name")
            .is_none()
    );
}

#[test]
fn test_mode_set_request_round_trip() {
    let json = r#"{"type":"mode_set_request","mode":"plan","from":"team-lead"}"#;
    assert_round_trips(json, "mode_set_request");
}

#[test]
fn test_mode_set_request_rejects_invalid_mode() {
    let json = r#"{"type":"mode_set_request","mode":"not-a-mode","from":"team-lead"}"#;
    assert!(
        parse_protocol_message(json).is_none(),
        "mode_set_request.mode must be PermissionMode"
    );
}

#[test]
fn test_is_structured_rejects_plain_text() {
    assert!(!is_structured_protocol_message("hello world"));
    assert!(!is_structured_protocol_message("{not json"));
}

#[test]
fn test_is_structured_rejects_unknown_type_tag() {
    let json = r#"{"type":"something_else","data":1}"#;
    assert!(
        !is_structured_protocol_message(json),
        "unknown `type` tag must not register as a protocol message"
    );
}

#[test]
fn test_parse_returns_none_for_non_protocol_text() {
    assert!(parse_protocol_message("plain message").is_none());
    assert!(parse_protocol_message("").is_none());
}

#[test]
fn test_check_message_type_filter() {
    let json =
        r#"{"type":"plan_approval_response","requestId":"p1","approved":true,"timestamp":"t"}"#;
    assert!(check_message_type(json, "plan_approval_response").is_some());
    assert!(
        check_message_type(json, "permission_response").is_none(),
        "type filter must reject mismatched variants"
    );
}

#[test]
fn test_optional_fields_omitted_on_serialize_when_none() {
    let m = ProtocolMessage::IdleNotification {
        from: "w".into(),
        timestamp: "t".into(),
        idle_reason: None,
        summary: None,
        completed_task_id: None,
        completed_status: None,
        failure_reason: None,
    };
    let s = serde_json::to_string(&m).unwrap();
    assert!(!s.contains("idleReason"), "None fields must skip: {s}");
    assert!(!s.contains("summary"));
}

#[test]
fn test_plan_approval_response_parses_from_leader_writer_codec() {
    // The leader-side writers (TUI human approve + model SendMessage)
    // serialize via `coco_tool_runtime::PlanApprovalResponse`, which has NO
    // `timestamp` field. The teammate consumer (`wait_for_plan_approval`)
    // parses via THIS codec — it must accept the timestamp-less JSON, else
    // an actually-approving leader blocks the teammate forever.
    let writer = coco_tool_runtime::PlanApprovalMessage::PlanApprovalResponse(
        coco_tool_runtime::PlanApprovalResponse {
            request_id: "req-1".to_string(),
            approved: true,
            feedback: None,
            permission_mode: None,
        },
    );
    let json = serde_json::to_string(&writer).expect("writer serialises");
    assert!(
        !json.contains("timestamp"),
        "writer omits timestamp: {json}"
    );

    let parsed = parse_protocol_message(&json).expect("consumer must parse writer JSON");
    let ProtocolMessage::PlanApprovalResponse {
        request_id,
        approved,
        timestamp,
        ..
    } = parsed
    else {
        panic!("wrong variant");
    };
    assert_eq!(request_id, "req-1");
    assert!(approved);
    assert_eq!(timestamp, "", "missing timestamp defaults to empty");
}
