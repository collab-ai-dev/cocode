use super::*;

#[test]
fn test_stream_error_new() {
    let error = StreamError::new("Test error");
    assert_eq!(error.message, "Test error");
    assert!(error.code.is_none());
    assert!(!error.is_retryable);
}

#[test]
fn test_stream_error_retryable() {
    let error = StreamError::retryable("Temporary error");
    assert_eq!(error.message, "Temporary error");
    assert!(error.is_retryable);
}

#[test]
fn test_text_start() {
    let part = LanguageModelV4StreamPart::TextStart {
        id: "text-1".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("textStart"));
}

#[test]
fn test_text_delta() {
    let part = LanguageModelV4StreamPart::TextDelta {
        id: "text-1".to_string(),
        delta: "Hello".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("textDelta"));
    assert!(json.contains("Hello"));
}

#[test]
fn test_text_end() {
    let part = LanguageModelV4StreamPart::TextEnd {
        id: "text-1".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("textEnd"));
}

#[test]
fn test_reasoning_start() {
    let part = LanguageModelV4StreamPart::ReasoningStart {
        id: "reason-1".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("reasoningStart"));
}

#[test]
fn test_reasoning_delta() {
    let part = LanguageModelV4StreamPart::ReasoningDelta {
        id: "reason-1".to_string(),
        delta: "Thinking...".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("reasoningDelta"));
}

#[test]
fn test_tool_input_start() {
    let part = LanguageModelV4StreamPart::ToolInputStart {
        id: "call-1".to_string(),
        tool_name: "get_weather".to_string(),
        provider_executed: None,
        dynamic: None,
        title: None,
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("toolInputStart"));
    assert!(json.contains("get_weather"));
}

#[test]
fn test_tool_input_start_with_fields() {
    let part = LanguageModelV4StreamPart::ToolInputStart {
        id: "call-2".to_string(),
        tool_name: "search".to_string(),
        provider_executed: Some(true),
        dynamic: Some(false),
        title: Some("Search the web".to_string()),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("toolInputStart"));
    assert!(json.contains("search"));
    assert!(json.contains("providerExecuted"));
    assert!(json.contains("Search the web"));
}

#[test]
fn test_finish_part() {
    let part = LanguageModelV4StreamPart::Finish {
        usage: Usage::new(10, 5),
        finish_reason: FinishReason::end_turn(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("finish"));
}

#[test]
fn test_stream_start() {
    let part = LanguageModelV4StreamPart::StreamStart { warnings: vec![] };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("streamStart"));
}

#[test]
fn test_error_part() {
    let part = LanguageModelV4StreamPart::Error {
        error: StreamError::new("Something went wrong"),
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("error"));
}

#[test]
fn test_source_type() {
    let source = Source::url("src-1", "https://example.com");
    let json = serde_json::to_string(&source).unwrap();
    assert!(json.contains("source"));
    assert!(json.contains("url"));

    let doc = Source::document("doc-1", "My Document", "application/pdf");
    let json = serde_json::to_string(&doc).unwrap();
    assert!(json.contains("document"));
    assert!(json.contains("application/pdf"));
}

#[test]
fn test_file() {
    let file = File {
        data: "base64data".to_string(),
        media_type: "image/png".to_string(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&file).unwrap();
    assert!(json.contains("base64data"));
    assert!(json.contains("image/png"));
}

#[test]
fn test_custom_stream_part_serialization() {
    let part = LanguageModelV4StreamPart::Custom {
        kind: "openai-compaction".into(),
        provider_metadata: None,
    };
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains(r#""type":"custom""#));
    assert!(json.contains(r#""kind":"openai-compaction""#));

    let deserialized: LanguageModelV4StreamPart = serde_json::from_str(&json).unwrap();
    assert!(
        matches!(deserialized, LanguageModelV4StreamPart::Custom { kind, .. } if kind == "openai-compaction")
    );
}
