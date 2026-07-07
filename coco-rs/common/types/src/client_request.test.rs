use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn initialize_has_method_tag() {
    let req = ClientRequest::Initialize(InitializeParams::default());
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "initialize");
}

#[test]
fn client_request_method_accessor_matches_serde_tag() {
    // Typed discriminator accessor must agree with the serde wire tag for
    // representative tuple / unit / boxed variants. The macro drives both
    // from the same `$wire` literal, so any drift here is a bug in the
    // macro itself.
    let cases: &[(ClientRequest, ClientRequestMethod, &str)] = &[
        (
            ClientRequest::Initialize(InitializeParams::default()),
            ClientRequestMethod::Initialize,
            "initialize",
        ),
        (
            ClientRequest::TurnInterrupt,
            ClientRequestMethod::TurnInterrupt,
            "turn/interrupt",
        ),
        (
            ClientRequest::McpStatus,
            ClientRequestMethod::McpStatus,
            "mcp/status",
        ),
        (
            ClientRequest::AgentInterruptCurrentWork(AgentInterruptCurrentWorkParams {
                agent_id: "worker@team".into(),
            }),
            ClientRequestMethod::AgentInterruptCurrentWork,
            "agent/interruptCurrentWork",
        ),
    ];
    for (req, expected, wire) in cases {
        assert_eq!(req.method(), *expected);
        assert_eq!(expected.as_str(), *wire);
        let j = serde_json::to_value(req).unwrap();
        assert_eq!(j["method"], *wire);
    }
}

#[test]
fn turn_interrupt_is_unit_variant() {
    let req = ClientRequest::TurnInterrupt;
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "turn/interrupt");
}

#[test]
fn session_id_params_stay_string_shaped_on_wire() {
    let req = ClientRequest::SessionRead(SessionReadParams {
        session_id: crate::SessionId::try_new("session-1").unwrap(),
        cursor: None,
        limit: None,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "session/read");
    assert_eq!(j["params"]["session_id"], "session-1");

    let back: ClientRequest = serde_json::from_value(json!({
        "method": "session/archive",
        "params": { "session_id": "session-1" }
    }))
    .unwrap();
    match back {
        ClientRequest::SessionArchive(params) => {
            assert_eq!(params.session_id.as_str(), "session-1");
        }
        other => panic!("expected SessionArchive, got {other:?}"),
    }
}

#[test]
fn session_id_params_reject_unsafe_path_components() {
    let err = serde_json::from_value::<ClientRequest>(json!({
        "method": "session/read",
        "params": { "session_id": "../escape" }
    }))
    .unwrap_err();
    assert!(err.to_string().contains("path separator"));
}

#[test]
fn turn_start_carries_prompt_and_overrides() {
    let req = ClientRequest::TurnStart(TurnStartParams {
        prompt: "hello".into(),
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: Some("<command-name>/test</command-name>".into()),
        model_selection: Some(crate::ProviderModelSelection {
            provider: "moa".into(),
            model_id: "balanced".into(),
        }),
        permission_mode: Some(crate::PermissionMode::AcceptEdits),
        thinking_level: None,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "turn/start");
    assert_eq!(j["params"]["prompt"], "hello");
    assert_eq!(
        j["params"]["slash_metadata"],
        "<command-name>/test</command-name>"
    );
    assert_eq!(j["params"]["model_selection"]["provider"], "moa");
    assert_eq!(j["params"]["model_selection"]["model_id"], "balanced");
    // Wire format matches TS `PermissionModeSchema` (camelCase).
    assert_eq!(j["params"]["permission_mode"], "acceptEdits");
}

#[test]
fn approval_resolve_serializes_decision() {
    let req = ClientRequest::ApprovalResolve(ApprovalResolveParams {
        request_id: "req-1".into(),
        decision: ApprovalDecision::Allow,
        permission_update: None,
        feedback: None,
        updated_input: None,
        content_blocks: None,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "approval/resolve");
    assert_eq!(j["params"]["decision"], "allow");
    assert_eq!(j["params"]["request_id"], "req-1");
}

#[test]
fn mcp_status_is_unit_variant() {
    let req = ClientRequest::McpStatus;
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "mcp/status");
}

#[test]
fn mcp_set_servers_carries_server_map() {
    let mut servers = std::collections::HashMap::new();
    servers.insert("github".into(), json!({ "command": "gh-mcp" }));
    let req = ClientRequest::McpSetServers(McpSetServersParams { servers });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "mcp/setServers");
    assert_eq!(j["params"]["servers"]["github"]["command"], "gh-mcp");
}

#[test]
fn mcp_toggle_carries_server_and_enabled() {
    let req = ClientRequest::McpToggle(McpToggleParams {
        server_name: "github".into(),
        enabled: false,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "mcp/toggle");
    assert_eq!(j["params"]["server_name"], "github");
    assert_eq!(j["params"]["enabled"], false);
}

#[test]
fn config_apply_flags_carries_settings_record() {
    let mut settings = std::collections::HashMap::new();
    settings.insert("experimental_x".into(), json!(true));
    let req = ClientRequest::ConfigApplyFlags(ConfigApplyFlagsParams { settings });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "config/applyFlags");
    assert_eq!(j["params"]["settings"]["experimental_x"], true);
}

#[test]
fn plugin_reload_is_unit_variant() {
    let req = ClientRequest::PluginReload;
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "plugin/reload");
}

#[test]
fn context_usage_is_unit_variant() {
    let req = ClientRequest::ContextUsage;
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "context/usage");
}

#[test]
fn agent_interrupt_current_work_carries_agent_id() {
    let req = ClientRequest::AgentInterruptCurrentWork(AgentInterruptCurrentWorkParams {
        agent_id: "worker@team".into(),
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "agent/interruptCurrentWork");
    assert_eq!(j["params"]["agent_id"], "worker@team");
}

#[test]
fn set_permission_mode_carries_mode() {
    let req = ClientRequest::SetPermissionMode(SetPermissionModeParams {
        mode: crate::PermissionMode::Plan,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "control/setPermissionMode");
    assert_eq!(j["params"]["mode"], "plan");
    assert!(j["params"].get("ultraplan").is_none());
}

#[test]
fn client_request_roundtrip_preserves_variant() {
    let req = ClientRequest::TurnStart(TurnStartParams {
        prompt: "test".into(),
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    });
    let s = serde_json::to_string(&req).unwrap();
    let back: ClientRequest = serde_json::from_str(&s).unwrap();
    match back {
        ClientRequest::TurnStart(p) => assert_eq!(p.prompt, "test"),
        _ => panic!("expected TurnStart"),
    }
}

#[test]
fn turn_start_carries_images() {
    let req = ClientRequest::TurnStart(TurnStartParams {
        prompt: "look".into(),
        history_override: Vec::new(),
        images: vec![crate::QueuedCommandEditImage {
            media_type: "image/png".into(),
            data_base64: "aW1n".into(),
        }],
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "turn/start");
    assert_eq!(j["params"]["images"][0]["media_type"], "image/png");
    assert_eq!(j["params"]["images"][0]["data_base64"], "aW1n");
}

#[test]
fn approval_decision_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(ApprovalDecision::Allow).unwrap(),
        json!("allow")
    );
    assert_eq!(
        serde_json::to_value(ApprovalDecision::Deny).unwrap(),
        json!("deny")
    );
}
