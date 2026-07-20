use super::*;

fn session_target() -> SessionTarget {
    SessionTarget {
        session_id: crate::SessionId::try_new("session-a").unwrap(),
    }
}

#[test]
fn canonical_targets_round_trip() {
    let session = session_target();
    assert_eq!(
        serde_json::from_value::<SessionTarget>(serde_json::to_value(&session).unwrap())
            .unwrap()
            .session_id,
        session.session_id
    );
}

#[test]
fn config_targets_round_trip_without_string_scopes() {
    let reads = [
        ConfigReadTarget::Process,
        ConfigReadTarget::Session(session_target()),
    ];
    for target in reads {
        let decoded: ConfigReadTarget =
            serde_json::from_value(serde_json::to_value(&target).unwrap()).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{target:?}"));
    }
    let writes = [
        ConfigWriteTarget::User,
        ConfigWriteTarget::Project(session_target()),
        ConfigWriteTarget::Local(session_target()),
    ];
    for target in writes {
        let decoded: ConfigWriteTarget =
            serde_json::from_value(serde_json::to_value(&target).unwrap()).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{target:?}"));
    }
}

#[test]
fn missing_targets_fail_deserialization() {
    for request in [
        serde_json::json!({"method":"turn/interrupt"}),
        serde_json::json!({"method":"mcp/status"}),
        serde_json::json!({"method":"session/read","params":{"limit":1}}),
        serde_json::json!({"method":"session/close","params":{}}),
        serde_json::json!({"method":"session/delete","params":{}}),
        serde_json::json!({"method":"config/read","params":{}}),
    ] {
        assert!(serde_json::from_value::<ClientRequest>(request).is_err());
    }
}

#[test]
fn full_and_read_requests_serialize_session_target() {
    let full = ClientRequest::TurnInterrupt(session_target());
    let session = ClientRequest::McpStatus(session_target());
    assert_eq!(
        serde_json::to_value(full).unwrap()["params"]["session_id"],
        "session-a"
    );
    assert_eq!(
        serde_json::to_value(session).unwrap()["params"]["session_id"],
        "session-a"
    );
}

#[test]
fn request_scope_classifies_representative_methods() {
    assert_eq!(
        request_scope(ClientRequestMethod::Initialize),
        RequestScope::Connection
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionReplace),
        RequestScope::Lifecycle
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionClose),
        RequestScope::Lifecycle
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionDelete),
        RequestScope::Lifecycle
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionList),
        RequestScope::Process
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionRead),
        RequestScope::SessionRead
    );
    assert_eq!(
        request_scope(ClientRequestMethod::TurnStart),
        RequestScope::SessionFull
    );
    assert_eq!(
        request_scope(ClientRequestMethod::ConfigRead),
        RequestScope::Configuration
    );
}

#[test]
fn connection_profile_normalizes_and_freezes_initialize_data() {
    let mut params = InitializeParams {
        client_mcp_servers: Some(vec![" beta ".into(), "alpha".into(), "alpha".into()]),
        ..Default::default()
    };
    params.hooks = Some(std::collections::HashMap::from([(
        crate::HookEventType::PreToolUse,
        vec![HookCallbackMatcher {
            matcher: None,
            hook_callback_ids: vec!["callback-a".into()],
            timeout: None,
        }],
    )]));
    let profile = ConnectionProfile::try_from(params).unwrap();
    assert_eq!(
        profile.initialize().client_mcp_servers.as_ref().unwrap(),
        &["alpha".to_string(), "beta".to_string()]
    );
    assert_eq!(
        profile.initialize().hooks.as_ref().unwrap()[&crate::HookEventType::PreToolUse][0]
            .hook_callback_ids,
        ["callback-a"]
    );
}

#[test]
fn replace_serializes_source_and_typed_destination() {
    let request = ClientRequest::SessionReplace(Box::new(SessionReplaceParams {
        source: session_target(),
        destination: SessionReplacement::Resume(session_target()),
    }));
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["method"], "session/replace");
    assert_eq!(value["params"]["source"]["session_id"], "session-a");
    assert_eq!(
        value["params"]["destination"]["resume"]["session_id"],
        "session-a"
    );
}
