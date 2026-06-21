use std::sync::Arc;

use crate::error::AIError;
use crate::test_utils::MockTranscriptionModel;

use super::*;

#[test]
fn test_transcribe_options() {
    let options = TranscribeOptions::new("whisper-1", AudioData::bytes(vec![0, 1, 2]));

    assert!(options.model.is_string());
}

#[test]
fn test_transcribe_options_with_provider_options() {
    let options =
        TranscribeOptions::new("whisper-1", AudioData::bytes(vec![0, 1, 2])).with_max_retries(3);

    assert_eq!(options.max_retries, Some(3));
}

#[test]
fn test_audio_data() {
    let bytes = AudioData::bytes(vec![0, 1, 2, 3]);
    match bytes {
        AudioData::Bytes(data) => assert_eq!(data.len(), 4),
        _ => panic!("Expected Bytes variant"),
    }

    let url = AudioData::url("https://example.com/audio.mp3");
    match url {
        AudioData::Url(u) => assert_eq!(u, "https://example.com/audio.mp3"),
        _ => panic!("Expected Url variant"),
    }
}

#[test]
fn test_transcription_segment() {
    let segment = TranscriptionSegment::new("Hello world", 0.0, 2.5);
    assert_eq!(segment.text, "Hello world");
    assert_eq!(segment.start_second, 0.0);
    assert_eq!(segment.end_second, 2.5);
    assert_eq!(segment.duration(), 2.5);
}

#[test]
fn test_detect_audio_content_type() {
    // MP3 with ID3 tag
    assert_eq!(detect_audio_content_type(b"ID3\x00\x00\x00"), "audio/mpeg");

    // WAV
    assert_eq!(
        detect_audio_content_type(b"RIFF\x00\x00\x00\x00WAVE"),
        "audio/wav"
    );

    // OGG
    assert_eq!(detect_audio_content_type(b"OggS\x00"), "audio/ogg");

    // FLAC
    assert_eq!(detect_audio_content_type(b"fLaC\x00"), "audio/flac");
}

#[test]
fn test_detect_audio_content_type_mp3_sync() {
    // MP3 sync frame: 0xFF followed by byte with top 3 bits set
    assert_eq!(
        detect_audio_content_type(&[0xFF, 0xFB, 0x90, 0x00]),
        "audio/mpeg"
    );
    assert_eq!(
        detect_audio_content_type(&[0xFF, 0xE0, 0x00, 0x00]),
        "audio/mpeg"
    );
}

#[test]
fn test_detect_audio_content_type_m4a() {
    // M4A/MP4: "ftyp" at offset 4
    let mut data = vec![0x00, 0x00, 0x00, 0x20]; // size bytes
    data.extend_from_slice(b"ftyp");
    assert_eq!(detect_audio_content_type(&data), "audio/mp4");
}

#[test]
fn test_detect_audio_content_type_webm() {
    assert_eq!(
        detect_audio_content_type(&[0x1a, 0x45, 0xdf, 0xa3, 0x00]),
        "audio/webm"
    );
}

#[test]
fn test_detect_audio_content_type_unknown() {
    assert_eq!(
        detect_audio_content_type(&[0x00, 0x01, 0x02, 0x03]),
        "application/octet-stream"
    );
}

#[tokio::test]
async fn test_transcribe_with_mock() {
    let mock = MockTranscriptionModel::builder()
        .with_provider("test-provider")
        .with_model_id("test-whisper")
        .build();
    let model: Arc<dyn vercel_ai_provider::TranscriptionModelV4> = Arc::new(mock);

    let result = transcribe(TranscribeOptions {
        model: TranscriptionModel::from_v4(model),
        audio: AudioData::bytes(vec![0xFF, 0xFB, 0x90, 0x00]),
        ..Default::default()
    })
    .await
    .unwrap();

    assert_eq!(result.text, "Hello, world!");
    assert!(!result.responses.is_empty());
}

#[tokio::test]
async fn test_transcribe_empty_text_error() {
    use vercel_ai_provider::TranscriptionModelV4Result;

    let mock = MockTranscriptionModel::builder()
        .with_transcribe_handler(|_| Ok(TranscriptionModelV4Result::new("")))
        .build();
    let model: Arc<dyn vercel_ai_provider::TranscriptionModelV4> = Arc::new(mock);

    let result = transcribe(TranscribeOptions {
        model: TranscriptionModel::from_v4(model),
        audio: AudioData::bytes(vec![0x00, 0x01]),
        ..Default::default()
    })
    .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        AIError::NoTranscriptGenerated
    ));
}
