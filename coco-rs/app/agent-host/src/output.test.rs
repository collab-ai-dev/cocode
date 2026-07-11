use super::*;

#[test]
fn test_format_usage() {
    let usage = TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 1000,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 500,
            ..Default::default()
        },
    };
    let s = format_usage(&usage, 0.05);
    assert!(s.contains("1000↓"));
    assert!(s.contains("500↑"));
    assert!(s.contains("$0.0500"));
}

#[test]
fn test_format_turn_summary() {
    assert_eq!(format_turn_summary(1, 3, 2500), "Turn 1 (3 tools, 2.5s)");
    assert_eq!(format_turn_summary(2, 0, 1000), "Turn 2 (1.0s)");
}

#[test]
fn test_sdk_message_serialization() {
    let msg = SdkMessage::Result {
        text: "done".to_string(),
        turns: 3,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"result\""));
    assert!(json.contains("\"turns\":3"));
}
