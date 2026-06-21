use super::*;

#[test]
fn test_generate_result_new() {
    let content = vec![AssistantContentPart::text("Hello")];
    let usage = Usage::new(10, 5);
    let result =
        LanguageModelV4GenerateResult::new(content, usage.clone(), FinishReason::end_turn());
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.usage, usage);
    assert!(result.finish_reason.is_stop());
    assert!(result.warnings.is_empty());
    assert!(result.provider_metadata.is_none());
    assert!(result.request.is_none());
    assert!(result.response.is_none());
}

#[test]
fn test_generate_result_text() {
    let usage = Usage::new(10, 5);
    let result = LanguageModelV4GenerateResult::text("Hello, world!", usage);
    assert_eq!(result.content.len(), 1);
    assert!(result.finish_reason.is_stop());
    let text = result.text_content().unwrap();
    assert_eq!(text, "Hello, world!");
}

#[test]
fn test_generate_result_with_warnings() {
    let warnings = vec![Warning::other("Test warning")];
    let result =
        LanguageModelV4GenerateResult::text("test", Usage::empty()).with_warnings(warnings);
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn test_generate_result_with_response() {
    let ts = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let response = LanguageModelV4Response::new()
        .with_model_id("gpt-4")
        .with_timestamp(ts);
    let result =
        LanguageModelV4GenerateResult::text("test", Usage::empty()).with_response(response);
    assert!(result.response.is_some());
    let resp = result.response.unwrap();
    assert_eq!(resp.model_id, Some("gpt-4".to_string()));
}

#[test]
fn test_generate_result_with_request() {
    let request = LanguageModelV4Request::new().with_body(serde_json::json!({"model": "gpt-4"}));
    let result = LanguageModelV4GenerateResult::text("test", Usage::empty()).with_request(request);
    assert!(result.request.is_some());
}

#[test]
fn test_generate_result_text_content_none() {
    let content = vec![
        AssistantContentPart::text("Hello"),
        AssistantContentPart::text("World"),
    ];
    let result =
        LanguageModelV4GenerateResult::new(content, Usage::empty(), FinishReason::end_turn());
    // text_content returns None when there's more than one part
    assert!(result.text_content().is_none());
}

#[test]
fn test_response_builder() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("x-request-id".to_string(), "req-123".to_string());

    let ts = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let response = LanguageModelV4Response::new()
        .with_id("resp_abc")
        .with_timestamp(ts)
        .with_model_id("gpt-4")
        .with_headers(headers.clone());

    assert_eq!(response.id, Some("resp_abc".to_string()));
    assert_eq!(response.timestamp, Some(ts));
    assert_eq!(response.model_id, Some("gpt-4".to_string()));
    assert_eq!(response.headers, Some(headers));
}

#[test]
fn test_request_builder() {
    let request = LanguageModelV4Request::new().with_body(serde_json::json!({"prompt": "Hello"}));

    assert!(request.body.is_some());
}
