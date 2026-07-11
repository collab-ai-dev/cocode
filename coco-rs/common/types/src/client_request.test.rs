use super::*;

fn session_target() -> SessionTarget {
    SessionTarget {
        session_id: crate::SessionId::try_new("session-a").unwrap(),
    }
}

fn interactive_target() -> InteractiveTarget {
    InteractiveTarget {
        session_id: session_target().session_id,
        surface_id: crate::SurfaceId::new("surface-a"),
    }
}

#[test]
fn canonical_targets_round_trip() {
    let session = session_target();
    let interactive = interactive_target();
    assert_eq!(
        serde_json::from_value::<SessionTarget>(serde_json::to_value(&session).unwrap())
            .unwrap()
            .session_id,
        session.session_id
    );
    let decoded =
        serde_json::from_value::<InteractiveTarget>(serde_json::to_value(&interactive).unwrap())
            .unwrap();
    assert_eq!(decoded.session_id, interactive.session_id);
    assert_eq!(decoded.surface_id, interactive.surface_id);
}

#[test]
fn archive_target_round_trips_both_authority_cases() {
    for target in [
        ArchiveTarget::Interactive(interactive_target()),
        ArchiveTarget::Orphaned(session_target()),
    ] {
        let value = serde_json::to_value(&target).unwrap();
        let decoded: ArchiveTarget = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.session_id(), target.session_id());
    }
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
        ConfigWriteTarget::Project(interactive_target()),
        ConfigWriteTarget::Local(interactive_target()),
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
        serde_json::json!({"method":"session/archive","params":{}}),
        serde_json::json!({"method":"config/read","params":{}}),
    ] {
        assert!(serde_json::from_value::<ClientRequest>(request).is_err());
    }
}

#[test]
fn interactive_and_session_requests_serialize_authority() {
    let interactive = ClientRequest::TurnInterrupt(interactive_target());
    let session = ClientRequest::McpStatus(session_target());
    assert_eq!(
        serde_json::to_value(interactive).unwrap()["params"]["surface_id"],
        "surface-a"
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
        request_scope(ClientRequestMethod::SessionList),
        RequestScope::Process
    );
    assert_eq!(
        request_scope(ClientRequestMethod::SessionRead),
        RequestScope::SessionRead
    );
    assert_eq!(
        request_scope(ClientRequestMethod::TurnStart),
        RequestScope::Interactive
    );
    assert_eq!(
        request_scope(ClientRequestMethod::ConfigRead),
        RequestScope::Configuration
    );
}

#[test]
fn connection_profile_normalizes_and_freezes_callback_requirements() {
    let mut params = InitializeParams {
        sdk_mcp_servers: Some(vec![" beta ".into(), "alpha".into(), "alpha".into()]),
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
        profile.initialize().sdk_mcp_servers.as_ref().unwrap(),
        &["alpha".to_string(), "beta".to_string()]
    );
    let requirements = profile.callback_requirements();
    assert!(requirements.hook_callback_ids.contains("callback-a"));
    assert!(requirements.is_satisfied_by(&profile));
    assert!(
        !requirements
            .is_satisfied_by(&ConnectionProfile::try_from(InitializeParams::default()).unwrap())
    );
}

#[test]
fn replace_serializes_source_and_typed_destination() {
    let request = ClientRequest::SessionReplace(Box::new(SessionReplaceParams {
        source: interactive_target(),
        destination: SessionReplacement::Resume(session_target()),
    }));
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["method"], "session/replace");
    assert_eq!(value["params"]["source"]["surface_id"], "surface-a");
    assert_eq!(
        value["params"]["destination"]["resume"]["session_id"],
        "session-a"
    );
}
