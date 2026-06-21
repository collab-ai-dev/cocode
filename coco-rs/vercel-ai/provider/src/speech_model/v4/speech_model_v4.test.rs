use super::*;

#[test]
fn test_call_options_builder() {
    let opts = SpeechModelV4CallOptions::new("Hello world")
        .with_voice("alloy")
        .with_output_format("mp3")
        .with_speed(1.5)
        .with_instructions("Speak clearly")
        .with_language("en");

    assert_eq!(opts.text, "Hello world");
    assert_eq!(opts.voice, Some("alloy".to_string()));
    assert_eq!(opts.output_format, Some("mp3".to_string()));
    assert_eq!(opts.speed, Some(1.5));
    assert_eq!(opts.instructions, Some("Speak clearly".to_string()));
    assert_eq!(opts.language, Some("en".to_string()));
}

#[test]
fn test_result_constructors() {
    let result = SpeechModelV4Result::mp3(vec![0xFF, 0xFB]);
    assert_eq!(result.content_type, "audio/mpeg");
    assert!(result.warnings.is_empty());
    assert!(result.request.is_none());
    assert!(result.provider_metadata.is_none());

    let result = SpeechModelV4Result::wav(vec![0x00]);
    assert_eq!(result.content_type, "audio/wav");
}

#[test]
fn test_result_builders() {
    let response = SpeechModelV4Response::default()
        .with_model_id("tts-1")
        .with_body(serde_json::json!({"key": "val"}));

    let request = SpeechModelV4Request::default().with_body(serde_json::json!({"text": "hello"}));

    let result = SpeechModelV4Result::mp3(vec![0xFF])
        .with_warnings(vec![crate::shared::Warning::other("test warning")])
        .with_response(response)
        .with_request(request);

    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.response.model_id, Some("tts-1".to_string()));
    assert!(result.request.is_some());
}
