use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::ProviderOptions;

#[test]
fn maps_media_type_to_extension() {
    assert_eq!(extension_from_media_type("audio/wav"), "wav");
    assert_eq!(extension_from_media_type("audio/mpeg"), "mp3");
    assert_eq!(extension_from_media_type("audio/flac"), "flac");
    assert_eq!(extension_from_media_type("audio/unknown"), "bin");
}

#[test]
fn parses_verbose_response_with_segments() {
    let json = r#"{
        "text": "hello world",
        "x_groq": {"id": "req_1"},
        "task": "transcribe",
        "language": "english",
        "duration": 1.5,
        "segments": [
            {"id": 0, "seek": 0, "start": 0.0, "end": 1.5, "text": "hello world",
             "tokens": [1,2], "temperature": 0.0, "avg_logprob": -0.3,
             "compression_ratio": 1.2, "no_speech_prob": 0.01}
        ]
    }"#;
    let resp: GroqTranscriptionResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.text, "hello world");
    assert_eq!(resp.language.as_deref(), Some("english"));
    assert_eq!(resp.duration, Some(1.5));
    let segments = resp.segments.expect("segments");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "hello world");
    assert_eq!(segments[0].end, 1.5);
}

#[test]
fn parses_minimal_json_response() {
    let json = r#"{"text": "hi", "x_groq": {"id": "req_2"}}"#;
    let resp: GroqTranscriptionResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.text, "hi");
    assert!(resp.segments.is_none());
}

#[test]
fn extracts_options_from_groq_namespace() {
    let ns: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({
            "language": "en",
            "responseFormat": "verbose_json",
            "temperature": 0.0,
            "timestampGranularities": ["segment", "word"]
        }))
        .unwrap();
    let mut map = std::collections::HashMap::new();
    map.insert("groq".to_string(), ns);
    let opts = extract_transcription_options(&Some(ProviderOptions(map)));
    assert_eq!(opts.language.as_deref(), Some("en"));
    assert_eq!(opts.response_format.as_deref(), Some("verbose_json"));
    assert_eq!(opts.temperature, Some(0.0));
    assert_eq!(
        opts.timestamp_granularities,
        Some(vec!["segment".to_string(), "word".to_string()])
    );
}
