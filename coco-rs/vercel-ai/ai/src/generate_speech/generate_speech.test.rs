use std::sync::Arc;

use crate::error::AIError;
use crate::test_utils::MockSpeechModel;

use super::*;

#[test]
fn test_generate_speech_options() {
    let options = GenerateSpeechOptions::new("tts-1", "Hello")
        .with_voice("alloy")
        .with_speed(1.0);

    assert!(options.model.is_string());
    assert_eq!(options.text, "Hello");
    assert_eq!(options.voice, Some("alloy".to_string()));
    assert_eq!(options.speed, Some(1.0));
}

#[test]
fn test_generate_speech_options_all_fields() {
    let options = GenerateSpeechOptions::new("tts-1", "Hello")
        .with_voice("alloy")
        .with_output_format("mp3")
        .with_speed(1.5)
        .with_instructions("Speak clearly")
        .with_language("en")
        .with_max_retries(3);

    assert_eq!(options.voice, Some("alloy".to_string()));
    assert_eq!(options.output_format, Some("mp3".to_string()));
    assert_eq!(options.speed, Some(1.5));
    assert_eq!(options.instructions, Some("Speak clearly".to_string()));
    assert_eq!(options.language, Some("en".to_string()));
    assert_eq!(options.max_retries, Some(3));
}

#[test]
fn test_generated_audio_file() {
    let audio = GeneratedAudioFile::mp3(vec![0u8, 1, 2, 3]);
    assert_eq!(audio.media_type, "audio/mpeg");
    assert_eq!(audio.extension(), "mp3");
    assert!(!audio.data.is_empty());
}

#[test]
fn test_speech_result() {
    let audio = GeneratedAudioFile::wav(vec![0u8; 100]);
    let result = SpeechResult::new(audio);
    assert!(result.warnings.is_empty());
    assert!(result.responses.is_empty());
    assert!(result.provider_metadata.is_none());
}

#[tokio::test]
async fn test_generate_speech_with_mock() {
    let mock = MockSpeechModel::builder()
        .with_provider("test-provider")
        .with_model_id("test-tts")
        .build();
    let model: Arc<dyn vercel_ai_provider::SpeechModelV4> = Arc::new(mock);

    let result = generate_speech(GenerateSpeechOptions {
        model: SpeechModel::from_v4(model),
        text: "Hello, world!".to_string(),
        voice: Some("alloy".to_string()),
        ..Default::default()
    })
    .await
    .unwrap();

    assert!(!result.audio.data.is_empty());
    assert_eq!(result.audio.media_type, "audio/mpeg");
    assert!(!result.responses.is_empty());
}

#[tokio::test]
async fn test_generate_speech_empty_audio_error() {
    use vercel_ai_provider::SpeechModelV4Result;

    let mock = MockSpeechModel::builder()
        .with_generate_handler(|_| Ok(SpeechModelV4Result::new(vec![], "audio/mpeg")))
        .build();
    let model: Arc<dyn vercel_ai_provider::SpeechModelV4> = Arc::new(mock);

    let result = generate_speech(GenerateSpeechOptions {
        model: SpeechModel::from_v4(model),
        text: "Hello".to_string(),
        ..Default::default()
    })
    .await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AIError::NoSpeechGenerated));
}

#[tokio::test]
async fn test_generate_speech_options_passthrough() {
    let mock = MockSpeechModel::builder().build();
    let mock_ref = Arc::new(mock);
    let model: Arc<dyn vercel_ai_provider::SpeechModelV4> = mock_ref.clone();

    let _ = generate_speech(GenerateSpeechOptions {
        model: SpeechModel::from_v4(model),
        text: "Test text".to_string(),
        voice: Some("nova".to_string()),
        output_format: Some("mp3".to_string()),
        speed: Some(1.5),
        instructions: Some("Speak slowly".to_string()),
        language: Some("en".to_string()),
        ..Default::default()
    })
    .await
    .unwrap();

    let calls = mock_ref.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.text, "Test text");
    assert_eq!(call.voice, Some("nova".to_string()));
    assert_eq!(call.output_format, Some("mp3".to_string()));
    assert_eq!(call.speed, Some(1.5));
    assert_eq!(call.instructions, Some("Speak slowly".to_string()));
    assert_eq!(call.language, Some("en".to_string()));
}
