use super::*;

#[test]
fn test_sdk_item_serialization() {
    let item = SdkItem::AgentMessage {
        text: "Hello".into(),
        model: Some("claude-sonnet-4-6".into()),
    };
    let json = serde_json::to_string(&item).unwrap();
    assert!(json.contains("\"type\":\"agent_message\""));
}

#[test]
fn test_sdk_options_defaults() {
    let opts: SdkQueryOptions = serde_json::from_str("{}").unwrap();
    assert!(opts.model.is_none());
    assert!(!opts.include_hook_events);
}

#[test]
fn test_sdk_session_result_session_id_is_typed_string_on_wire() {
    let result = SdkSessionResult {
        turns: Vec::new(),
        total_turns: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cost_usd: 0.0,
        session_id: coco_types::SessionId::try_new("session-1").unwrap(),
        model: "claude-sonnet-4-6".into(),
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["session_id"], "session-1");

    let back: SdkSessionResult = serde_json::from_value(json).unwrap();
    assert_eq!(back.session_id.as_str(), "session-1");

    let err = serde_json::from_value::<SdkSessionResult>(serde_json::json!({
        "turns": [],
        "total_turns": 0,
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost_usd": 0.0,
        "session_id": "../escape",
        "model": "claude-sonnet-4-6"
    }))
    .unwrap_err();
    assert!(err.to_string().contains("path separator"));
}
