use super::*;

#[test]
fn surface_id_serializes_as_plain_string() {
    let id = SurfaceId::from("surface-1");

    let json = serde_json::to_value(&id).unwrap();
    assert_eq!(json, serde_json::json!("surface-1"));

    let back: SurfaceId = serde_json::from_value(json).unwrap();
    assert_eq!(back.as_str(), "surface-1");
}

#[test]
fn generated_surface_id_is_non_empty() {
    let id = SurfaceId::generate();

    assert!(!id.as_str().is_empty());
}

#[test]
fn generated_session_id_is_uuid_string() {
    let id = SessionId::generate();

    uuid::Uuid::parse_str(id.as_str()).expect("generated session id is uuid");
}

#[test]
fn session_id_try_new_rejects_unsafe_path_components() {
    assert_eq!(
        SessionId::try_new("").unwrap_err(),
        IdValidationError::Empty
    );
    assert_eq!(
        SessionId::try_new(".").unwrap_err(),
        IdValidationError::DotSegment
    );
    assert_eq!(
        SessionId::try_new("..").unwrap_err(),
        IdValidationError::DotSegment
    );
    assert_eq!(
        SessionId::try_new("a/b").unwrap_err(),
        IdValidationError::PathSeparator
    );
    assert_eq!(
        SessionId::try_new("a\\b").unwrap_err(),
        IdValidationError::PathSeparator
    );
}

#[test]
fn session_id_try_new_uuid_requires_uuid_shape() {
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(SessionId::try_new_uuid(&id).unwrap().as_str(), id);
    assert_eq!(
        SessionId::try_new_uuid("session-not-a-uuid").unwrap_err(),
        IdValidationError::InvalidUuid
    );
}

#[test]
fn session_id_deserialize_rejects_unsafe_path_components() {
    let err = serde_json::from_value::<SessionId>(serde_json::json!("a/b")).unwrap_err();

    assert!(err.to_string().contains("path separator"));
}

#[test]
fn agent_id_try_new_rejects_unsafe_path_components() {
    assert_eq!(AgentId::try_new("agent_1").unwrap().as_str(), "agent_1");
    assert_eq!(AgentId::try_new("").unwrap_err(), IdValidationError::Empty);
    assert_eq!(
        AgentId::try_new("../agent").unwrap_err(),
        IdValidationError::PathSeparator
    );
}

#[test]
fn agent_id_try_new_generated_accepts_canonical_shape() {
    assert_eq!(
        AgentId::try_new_generated("a0123456789abcdef")
            .unwrap()
            .as_str(),
        "a0123456789abcdef"
    );
    assert_eq!(
        AgentId::try_new_generated("aworker_1-0123456789abcdef")
            .unwrap()
            .as_str(),
        "aworker_1-0123456789abcdef"
    );
}

#[test]
fn agent_id_try_new_generated_rejects_non_canonical_shape() {
    assert_eq!(
        AgentId::try_new_generated("agent_1").unwrap_err(),
        IdValidationError::InvalidAgentId
    );
    assert_eq!(
        AgentId::try_new_generated("a0123").unwrap_err(),
        IdValidationError::InvalidAgentId
    );
    assert_eq!(
        AgentId::try_new_generated("ahelper-0123456789abcdeg").unwrap_err(),
        IdValidationError::InvalidAgentId
    );
    assert_eq!(
        AgentId::try_new_generated("aHelper-0123456789abcdef").unwrap_err(),
        IdValidationError::InvalidAgentLabel
    );
    assert_eq!(
        AgentId::try_new_generated("a-0123456789abcdef").unwrap_err(),
        IdValidationError::InvalidAgentLabel
    );
}

#[test]
fn agent_id_generate_uses_canonical_shape() {
    let unlabeled = AgentId::generate(None);
    assert!(AgentId::try_new_generated(unlabeled.as_str()).is_ok());
    assert_eq!(unlabeled.as_str().len(), 17);
    assert!(unlabeled.as_str().starts_with('a'));

    let labeled = AgentId::try_generate(Some("worker_1")).unwrap();
    assert!(AgentId::try_new_generated(labeled.as_str()).is_ok());
    assert!(labeled.as_str().starts_with("aworker_1-"));
}

#[test]
fn agent_id_try_generate_rejects_invalid_labels() {
    assert_eq!(
        AgentId::try_generate(Some("Worker")).unwrap_err(),
        IdValidationError::InvalidAgentLabel
    );
    assert_eq!(
        AgentId::try_generate(Some("")).unwrap_err(),
        IdValidationError::InvalidAgentLabel
    );
    assert_eq!(
        AgentId::try_generate(Some("../worker")).unwrap_err(),
        IdValidationError::InvalidAgentLabel
    );
}

#[test]
fn agent_id_deserialize_rejects_unsafe_path_components() {
    let err = serde_json::from_value::<AgentId>(serde_json::json!("agent\\id")).unwrap_err();

    assert!(err.to_string().contains("path separator"));
}
