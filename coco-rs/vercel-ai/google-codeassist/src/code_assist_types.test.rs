use super::*;
use serde_json::json;

#[test]
fn generate_request_envelope_serializes_with_expected_keys() {
    let req = CodeAssistGenerateRequest {
        model: "gemini-2.5-pro".to_string(),
        project: "proj-1".to_string(),
        user_prompt_id: "uid-1".to_string(),
        request: json!({ "contents": [] }),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["model"], "gemini-2.5-pro");
    assert_eq!(v["project"], "proj-1");
    // snake_case on the wire (jcode parity — no rename on the struct).
    assert_eq!(v["user_prompt_id"], "uid-1");
    assert!(v["request"]["contents"].is_array());
}

#[test]
fn generate_response_unwraps_inner_response() {
    let raw = json!({
        "traceId": "t-1",
        "response": {
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "hi" }] },
                "finishReason": "STOP"
            }]
        }
    });
    let parsed: CodeAssistGenerateResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(parsed.trace_id.as_deref(), Some("t-1"));
    assert!(parsed.response.is_some());
}

#[test]
fn load_response_tolerates_missing_fields() {
    let parsed: LoadCodeAssistResponse = serde_json::from_value(json!({})).unwrap();
    assert!(parsed.current_tier.is_none());
    assert!(parsed.cloudaicompanion_project.is_none());
}

#[test]
fn client_metadata_is_camel_case() {
    let v = serde_json::to_value(ClientMetadata::default()).unwrap();
    assert_eq!(v["ideType"], "IDE_UNSPECIFIED");
    assert_eq!(v["pluginType"], "GEMINI");
}
