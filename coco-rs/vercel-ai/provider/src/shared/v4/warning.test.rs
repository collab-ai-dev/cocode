//! Tests for warning types.

use super::*;

#[test]
fn test_unsupported_warning() {
    let warning = Warning::unsupported("streaming");
    assert!(
        matches!(warning, Warning::Unsupported { feature, details: None } if feature == "streaming")
    );
}

#[test]
fn test_unsupported_warning_with_details() {
    let warning =
        Warning::unsupported_with_details("streaming", "Provider does not support streaming");
    assert!(
        matches!(warning, Warning::Unsupported { feature, details: Some(d) } if feature == "streaming" && d == "Provider does not support streaming")
    );
}

#[test]
fn test_compatibility_warning() {
    let warning = Warning::compatibility("tool_choice");
    assert!(
        matches!(warning, Warning::Compatibility { feature, details: None } if feature == "tool_choice")
    );
}

#[test]
fn test_compatibility_warning_with_details() {
    let warning = Warning::compatibility_with_details("tool_choice", "Only 'auto' is supported");
    assert!(
        matches!(warning, Warning::Compatibility { feature, details: Some(d) } if feature == "tool_choice" && d == "Only 'auto' is supported")
    );
}

#[test]
fn test_other_warning() {
    let warning = Warning::other("Something went wrong");
    assert!(matches!(warning, Warning::Other { message } if message == "Something went wrong"));
}

#[test]
fn test_serialize_unsupported() {
    let warning = Warning::unsupported_with_details("streaming", "Not supported");
    let json = serde_json::to_string(&warning).unwrap();
    assert!(json.contains(r#""type":"unsupported""#));
    assert!(json.contains(r#""feature":"streaming""#));
    assert!(json.contains(r#""details":"Not supported""#));
}

#[test]
fn test_serialize_compatibility() {
    let warning = Warning::compatibility("tool_choice");
    let json = serde_json::to_string(&warning).unwrap();
    assert!(json.contains(r#""type":"compatibility""#));
    assert!(json.contains(r#""feature":"tool_choice""#));
    assert!(!json.contains("details")); // Should be omitted when None
}

#[test]
fn test_serialize_other() {
    let warning = Warning::other("Test message");
    let json = serde_json::to_string(&warning).unwrap();
    assert!(json.contains(r#""type":"other""#));
    assert!(json.contains(r#""message":"Test message""#));
}

#[test]
fn test_deserialize_unsupported() {
    let json = r#"{"type":"unsupported","feature":"streaming","details":"Not supported"}"#;
    let warning: Warning = serde_json::from_str(json).unwrap();
    assert!(
        matches!(warning, Warning::Unsupported { feature, details: Some(_) } if feature == "streaming")
    );
}

#[test]
fn test_deserialize_compatibility() {
    let json = r#"{"type":"compatibility","feature":"tool_choice"}"#;
    let warning: Warning = serde_json::from_str(json).unwrap();
    assert!(
        matches!(warning, Warning::Compatibility { feature, details: None } if feature == "tool_choice")
    );
}

#[test]
fn test_deserialize_other() {
    let json = r#"{"type":"other","message":"Test message"}"#;
    let warning: Warning = serde_json::from_str(json).unwrap();
    assert!(matches!(warning, Warning::Other { message } if message == "Test message"));
}
