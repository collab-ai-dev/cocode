use super::*;

#[test]
fn format_to_content_type_known_formats() {
    assert_eq!(format_to_content_type(Some("mp3")), "audio/mpeg");
    assert_eq!(format_to_content_type(Some("opus")), "audio/opus");
    assert_eq!(format_to_content_type(Some("aac")), "audio/aac");
    assert_eq!(format_to_content_type(Some("flac")), "audio/flac");
    assert_eq!(format_to_content_type(Some("wav")), "audio/wav");
    assert_eq!(format_to_content_type(Some("pcm")), "audio/pcm");
}

#[test]
fn format_to_content_type_unknown_defaults_to_mpeg() {
    assert_eq!(format_to_content_type(Some("ogg")), "audio/mpeg");
    assert_eq!(format_to_content_type(Some("unknown")), "audio/mpeg");
    assert_eq!(format_to_content_type(None), "audio/mpeg");
}

#[test]
fn is_known_speech_format_valid() {
    assert!(is_known_speech_format("mp3"));
    assert!(is_known_speech_format("opus"));
    assert!(is_known_speech_format("aac"));
    assert!(is_known_speech_format("flac"));
    assert!(is_known_speech_format("wav"));
    assert!(is_known_speech_format("pcm"));
}

#[test]
fn is_known_speech_format_invalid() {
    assert!(!is_known_speech_format("ogg"));
    assert!(!is_known_speech_format("mp4"));
    assert!(!is_known_speech_format(""));
}
