use super::*;

#[test]
fn stream_part_serializes_with_kebab_case_type_tags() {
    let part = TranscriptionModelV4StreamPart::TranscriptPartial {
        id: Some("channel-0".to_string()),
        text: "hello".to_string(),
        start_second: Some(1.0),
        duration_in_seconds: Some(0.5),
        channel_index: Some(0),
        provider_metadata: None,
    };
    let value = serde_json::to_value(&part).expect("serialize");
    assert_eq!(value["type"], "transcript-partial");
    assert_eq!(value["text"], "hello");
    assert_eq!(value["channel_index"], 0);
    // Absent optionals are omitted, not null.
    assert!(value.get("end_second").is_none());
}

#[test]
fn finish_part_carries_segments_and_language() {
    let part = TranscriptionModelV4StreamPart::Finish {
        text: "full transcript".to_string(),
        segments: vec![TranscriptionSegmentV4::new("full transcript", 0.0, 2.0)],
        language: Some("en".to_string()),
        duration_in_seconds: Some(2.0),
        provider_metadata: None,
    };
    let value = serde_json::to_value(&part).expect("serialize");
    assert_eq!(value["type"], "finish");
    assert_eq!(value["language"], "en");
    assert_eq!(value["segments"][0]["text"], "full transcript");
}

#[test]
fn input_audio_format_new() {
    let fmt = TranscriptionInputAudioFormat::new("audio/pcm", Some(16000));
    assert_eq!(fmt.media_type, "audio/pcm");
    assert_eq!(fmt.rate, Some(16000));
}
