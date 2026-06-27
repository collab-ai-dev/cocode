use super::*;

#[test]
fn test_call_options_builder() {
    let opts = TranscriptionModelV4CallOptions::new(vec![0xFF, 0xFB], "audio/mpeg");

    assert_eq!(opts.audio, vec![0xFF, 0xFB]);
    assert_eq!(opts.media_type, "audio/mpeg");
    assert!(opts.provider_options.is_none());
    assert!(opts.abort_signal.is_none());
}

#[test]
fn test_result_constructors() {
    let result = TranscriptionModelV4Result::new("Hello world")
        .with_language("en")
        .with_duration_in_seconds(5.5);

    assert_eq!(result.text, "Hello world");
    assert_eq!(result.language, Some("en".to_string()));
    assert_eq!(result.duration_in_seconds, Some(5.5));
    assert!(result.warnings.is_empty());
}

#[test]
fn test_segment_v4() {
    let segment = TranscriptionSegmentV4::new("Hello", 0.0, 1.5);
    assert_eq!(segment.text, "Hello");
    assert_eq!(segment.start_second, 0.0);
    assert_eq!(segment.end_second, 1.5);
}

#[test]
fn test_result_with_segments() {
    let segments = vec![
        TranscriptionSegmentV4::new("Hello", 0.0, 1.0),
        TranscriptionSegmentV4::new("world", 1.0, 2.0),
    ];

    let result = TranscriptionModelV4Result::new("Hello world").with_segments(segments);

    assert_eq!(result.segments.as_ref().unwrap().len(), 2);
}

#[test]
fn test_response_metadata() {
    let response = TranscriptionModelV4Response::default()
        .with_model_id("whisper-1")
        .with_body(serde_json::json!({"key": "val"}));

    assert_eq!(response.model_id, Some("whisper-1".to_string()));
    assert!(response.body.is_some());
}
