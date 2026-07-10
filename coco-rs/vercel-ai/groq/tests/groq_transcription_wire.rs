//! Wire-level test for the Groq transcription model: multipart request +
//! verbose_json response mapping.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use vercel_ai_groq::GroqProviderSettings;
use vercel_ai_groq::create_groq;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn do_transcribe_maps_verbose_json() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "text": "hello world",
        "x_groq": {"id": "req_1"},
        "language": "english",
        "duration": 1.5,
        "segments": [{
            "id": 0, "seek": 0, "start": 0.0, "end": 1.5, "text": "hello world",
            "tokens": [1, 2], "temperature": 0.0, "avg_logprob": -0.3,
            "compression_ratio": 1.2, "no_speech_prob": 0.01
        }]
    });
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let provider = create_groq(GroqProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.transcription("whisper-large-v3");

    let options = TranscriptionModelV4CallOptions::new(vec![0x00, 0x01, 0x02], "audio/wav");
    let result = model.do_transcribe(options).await.expect("do_transcribe");

    assert_eq!(result.text, "hello world");
    assert_eq!(result.language.as_deref(), Some("english"));
    assert_eq!(result.duration_in_seconds, Some(1.5));
    let segments = result.segments.expect("segments");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "hello world");
}

#[tokio::test]
async fn do_transcribe_surfaces_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error": {"message": "bad audio", "type": "invalid_request_error"}}),
        ))
        .mount(&server)
        .await;

    let provider = create_groq(GroqProviderSettings {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.transcription("whisper-large-v3");
    let options = TranscriptionModelV4CallOptions::new(vec![0x00], "audio/wav");
    let err = model.do_transcribe(options).await.unwrap_err();
    // The typed GroqErrorData message is surfaced.
    assert!(format!("{err}").contains("bad audio"));
}
