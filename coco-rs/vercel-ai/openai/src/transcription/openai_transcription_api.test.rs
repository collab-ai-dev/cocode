use super::*;

#[test]
fn deserialize_minimal_response() {
    let json = r#"{"text": "Hello world"}"#;
    let response: OpenAITranscriptionResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.text, "Hello world");
    assert!(response.language.is_none());
    assert!(response.duration.is_none());
    assert!(response.words.is_none());
    assert!(response.segments.is_none());
}

#[test]
fn deserialize_full_response_with_words() {
    let json = r#"{
        "text": "Hello world",
        "language": "en",
        "duration": 2.5,
        "words": [
            {"word": "Hello", "start": 0.0, "end": 0.5},
            {"word": "world", "start": 0.6, "end": 1.0}
        ]
    }"#;
    let response: OpenAITranscriptionResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.text, "Hello world");
    assert_eq!(response.language.as_deref(), Some("en"));
    assert_eq!(response.duration, Some(2.5));
    let words = response.words.unwrap();
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].word, "Hello");
    assert_eq!(words[0].start, 0.0);
    assert_eq!(words[0].end, 0.5);
    assert_eq!(words[1].word, "world");
}

#[test]
fn deserialize_full_response_with_segments() {
    let json = r#"{
        "text": "Hello world",
        "language": "en",
        "duration": 2.5,
        "segments": [
            {
                "id": 0,
                "seek": 0,
                "start": 0.0,
                "end": 2.5,
                "text": "Hello world",
                "tokens": [15339, 1002],
                "temperature": 0.0,
                "avg_logprob": -0.25,
                "compression_ratio": 1.0,
                "no_speech_prob": 0.01
            }
        ]
    }"#;
    let response: OpenAITranscriptionResponse = serde_json::from_str(json).unwrap();
    let segments = response.segments.unwrap();
    assert_eq!(segments.len(), 1);
    let seg = &segments[0];
    assert_eq!(seg.id, Some(0));
    assert_eq!(seg.seek, Some(0));
    assert_eq!(seg.text, "Hello world");
    assert_eq!(seg.start, 0.0);
    assert_eq!(seg.end, 2.5);
    assert_eq!(seg.tokens.as_deref(), Some(&[15339, 1002][..]));
    assert_eq!(seg.temperature, Some(0.0));
    assert_eq!(seg.avg_logprob, Some(-0.25));
    assert_eq!(seg.compression_ratio, Some(1.0));
    assert_eq!(seg.no_speech_prob, Some(0.01));
}

#[test]
fn deserialize_segment_with_minimal_fields() {
    let json = r#"{
        "text": "test",
        "segments": [
            {
                "start": 0.0,
                "end": 1.0,
                "text": "test"
            }
        ]
    }"#;
    let response: OpenAITranscriptionResponse = serde_json::from_str(json).unwrap();
    let segments = response.segments.unwrap();
    assert_eq!(segments.len(), 1);
    let seg = &segments[0];
    assert!(seg.id.is_none());
    assert!(seg.seek.is_none());
    assert!(seg.tokens.is_none());
    assert!(seg.temperature.is_none());
    assert!(seg.avg_logprob.is_none());
    assert!(seg.compression_ratio.is_none());
    assert!(seg.no_speech_prob.is_none());
}
