use super::*;
use pretty_assertions::assert_eq;

#[test]
fn deserialize_chat_completions_error() {
    let json = r#"{"error":{"message":"Invalid API Key","type":"invalid_request_error"}}"#;
    let data: XaiErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.to_message(), "Invalid API Key");
    match data {
        XaiErrorData::ChatCompletions { error } => {
            assert_eq!(error.error_type.as_deref(), Some("invalid_request_error"));
        }
        XaiErrorData::Responses { .. } => panic!("wrong variant"),
    }
}

#[test]
fn deserialize_chat_completions_error_without_type() {
    let json = r#"{"error":{"message":"Something went wrong"}}"#;
    let data: XaiErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.to_message(), "Something went wrong");
}

#[test]
fn deserialize_responses_error_prefixes_code() {
    let json = r#"{"code":"Some/Error","error":"Live search is deprecated"}"#;
    let data: XaiErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.to_message(), "Some/Error: Live search is deprecated");
}

#[test]
fn error_text_is_verbatim_without_code_prefix() {
    // Soft-error / stream JSON-error paths surface the raw `error` text (unlike
    // `to_message`, which prefixes the code on the Responses shape).
    let responses: XaiErrorData =
        serde_json::from_str(r#"{"code":"X","error":"boom"}"#).expect("deserialize");
    assert_eq!(responses.error_text(), "boom");
    assert_eq!(responses.code(), Some("X"));

    let chat: XaiErrorData =
        serde_json::from_str(r#"{"error":{"message":"nope"}}"#).expect("deserialize");
    assert_eq!(chat.error_text(), "nope");
    assert_eq!(chat.code(), None);
}

#[test]
fn code_matches_service_unavailable_sentinel() {
    let json = format!(r#"{{"code":"{SERVICE_UNAVAILABLE_CODE}","error":"retry me"}}"#);
    let data: XaiErrorData = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(data.code(), Some(SERVICE_UNAVAILABLE_CODE));
}
