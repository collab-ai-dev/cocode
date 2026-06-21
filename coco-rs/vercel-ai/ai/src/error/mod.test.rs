use super::*;

#[test]
fn test_no_output_generated_error() {
    let err = AIError::NoOutputGenerated;
    assert!(err.is_no_output());
    assert!(!err.is_no_speech());
    assert!(!err.is_no_image());
}

#[test]
fn test_no_speech_generated_error() {
    let err = AIError::NoSpeechGenerated;
    assert!(err.is_no_speech());
    assert!(!err.is_no_output());
}

#[test]
fn test_no_image_generated_error() {
    let err = AIError::NoImageGenerated;
    assert!(err.is_no_image());
    assert!(!err.is_no_output());
}

#[test]
fn test_no_video_generated_error() {
    let err = AIError::NoVideoGeneratedWithResponse(NoVideoGeneratedError::new());
    assert!(err.is_no_video());
    assert!(!err.is_no_output());
}

#[test]
fn test_no_transcript_generated_error() {
    let err = AIError::NoTranscriptGenerated;
    assert!(err.is_no_transcript());
    assert!(!err.is_no_output());
}

#[test]
fn test_no_object_generated_detailed() {
    let err = AIError::NoObjectGeneratedDetailed(NoObjectGeneratedError::new());
    assert!(err.is_no_object());
    assert!(!err.is_no_output());
}

#[test]
fn test_no_object_generated_error_with_metadata() {
    let err = NoObjectGeneratedError::new()
        .with_text("partial output")
        .with_finish_reason(vercel_ai_provider::FinishReason::end_turn())
        .with_raw_response("raw");

    assert_eq!(err.text, Some("partial output".to_string()));
    assert!(err.finish_reason.is_some());
    assert_eq!(err.raw_response, Some("raw".to_string()));

    let display = format!("{err}");
    assert!(display.contains("partial output"));
}

#[test]
fn test_invalid_argument_error() {
    let err = InvalidArgumentError::new("bad value").with_argument("param");
    assert_eq!(err.message, "bad value");
    assert_eq!(err.argument, Some("param".to_string()));

    let display = format!("{err}");
    assert!(display.contains("bad value"));
}

#[test]
fn test_schema_validation_error() {
    let err = SchemaValidationError::new("type mismatch")
        .with_value(serde_json::json!(42))
        .with_schema(serde_json::json!({"type": "string"}));

    assert_eq!(err.value, Some(serde_json::json!(42)));
    assert!(err.schema.is_some());
}

#[test]
fn test_no_such_tool_error_display() {
    let err = NoSuchToolError::new("my_tool");
    let display = format!("{err}");
    assert!(display.contains("my_tool"));
    assert!(display.contains("No tools are available"));

    let err_with_tools =
        NoSuchToolError::new("my_tool").with_available_tools(vec!["a".into(), "b".into()]);
    let display = format!("{err_with_tools}");
    assert!(display.contains("a, b"));
}

#[test]
fn test_missing_tool_results_error() {
    let err = MissingToolResultsError::new(vec!["id1".into(), "id2".into()]);
    let display = format!("{err}");
    assert!(display.contains("id1, id2"));
    assert!(display.contains("results are missing"));
}

#[test]
fn test_retry_error() {
    let err = RetryError::new(vec!["err1".into(), "err2".into()]);
    assert_eq!(err.attempts, 2);
    assert_eq!(err.last_error, "err2");

    let display = format!("{err}");
    assert!(display.contains("2 attempts"));
}

#[test]
fn test_ai_error_display_variants() {
    let err = AIError::MaxStepsExceeded(10);
    let display = format!("{err}");
    assert!(display.contains("10 steps"));

    let err = AIError::ToolExecutionFailed("timeout".to_string());
    let display = format!("{err}");
    assert!(display.contains("timeout"));

    let err = AIError::Timeout("step 0 timed out".to_string());
    let display = format!("{err}");
    assert!(display.contains("step 0"));

    let err = AIError::InvalidConfig("bad config".to_string());
    let display = format!("{err}");
    assert!(display.contains("bad config"));
}
