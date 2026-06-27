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
