//! Wire-level tests for the xAI multimodal surfaces: image generation
//! (b64 + URL-download responses, edit routing), speech (binary audio
//! round-trip), batch transcription (multipart → verbose JSON), and video
//! (create → poll lifecycle, edit routing, timeout / failure paths).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use base64::Engine as _;
use serde_json::json;
use vercel_ai_provider::ImageData;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageModelV4File;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::SpeechModelV4CallOptions;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;
use vercel_ai_provider::TranscriptionSegmentV4;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_xai::XaiProvider;
use vercel_ai_xai::XaiProviderSettings;
use vercel_ai_xai::create_xai;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn provider_for(server: &MockServer) -> XaiProvider {
    create_xai(XaiProviderSettings::api_key(
        Some(server.uri()),
        Some("test-key".into()),
    ))
}

fn xai_options(value: serde_json::Value) -> Option<ProviderOptions> {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    Some(po)
}

// ─── Image ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn image_do_generate_returns_base64_images_and_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "b64_json": "Zm9v", "revised_prompt": "a fluffy cat" },
                { "b64_json": "YmFy" }
            ],
            "usage": { "cost_in_usd_ticks": 42 }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).image("grok-imagine-image");
    let result = model
        .do_generate(ImageModelV4CallOptions::new("a cat").with_n(2))
        .await
        .expect("do_generate");

    assert_eq!(result.images.len(), 2);
    assert_eq!(result.images[0].as_base64(), Some("Zm9v"));
    assert_eq!(result.images[1].as_base64(), Some("YmFy"));
    assert!(result.warnings.is_empty());

    let meta = result.provider_metadata.expect("metadata");
    assert_eq!(
        meta.0.get("xai"),
        Some(&json!({
            "images": [ { "revisedPrompt": "a fluffy cat" }, {} ],
            "costInUsdTicks": 42,
        }))
    );

    // Request body: forced b64_json + auth header.
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    assert_eq!(body["model"], "grok-imagine-image");
    assert_eq!(body["prompt"], "a cat");
    assert_eq!(body["n"], 2);
    assert_eq!(body["response_format"], "b64_json");
    assert_eq!(
        requests[0]
            .headers
            .get("Authorization")
            .map(|v| v.to_str().unwrap()),
        Some("Bearer test-key")
    );
}

#[tokio::test]
async fn image_do_generate_downloads_url_images() {
    let server = MockServer::start().await;
    let image_bytes = b"png-bytes".to_vec();
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [ { "url": format!("{}/generated/img.png", server.uri()) } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/generated/img.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(image_bytes.clone()))
        .mount(&server)
        .await;

    let model = provider_for(&server).image("grok-imagine-image");
    let result = model
        .do_generate(ImageModelV4CallOptions::new("a cat"))
        .await
        .expect("do_generate");

    let expected = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].data, ImageData::Base64(expected));
}

#[tokio::test]
async fn image_files_route_to_edits_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [ { "b64_json": "Zm9v" } ]
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).image("grok-imagine-image");
    let options =
        ImageModelV4CallOptions::new("make it blue").with_files(vec![ImageModelV4File::Url {
            url: "https://example.com/in.png".into(),
            provider_options: None,
        }]);
    let result = model.do_generate(options).await.expect("do_generate");
    assert_eq!(result.images[0].as_base64(), Some("Zm9v"));

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    assert_eq!(
        body["image"],
        json!({ "url": "https://example.com/in.png", "type": "image_url" })
    );
}

#[tokio::test]
async fn image_api_error_surfaces_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "prompt too long", "type": "invalid_request_error" }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).image("grok-imagine-image");
    let err = model
        .do_generate(ImageModelV4CallOptions::new("a cat"))
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("prompt too long"), "{err}");
}

// ─── Speech ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn speech_do_generate_round_trips_audio_bytes() {
    let server = MockServer::start().await;
    let audio = vec![0x49u8, 0x44, 0x33, 0x04, 0x00];
    Mock::given(method("POST"))
        .and(path("/tts"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(audio.clone())
                .insert_header("content-type", "audio/mpeg"),
        )
        .mount(&server)
        .await;

    let model = provider_for(&server).speech("");
    let options = SpeechModelV4CallOptions {
        text: "Hello from xAI".into(),
        speed: Some(1.5),
        provider_options: xai_options(json!({ "sampleRate": 44100 })),
        ..Default::default()
    };
    let result = model.do_generate_speech(options).await.expect("speech");

    assert_eq!(result.audio, audio);
    assert_eq!(result.content_type, "audio/mpeg");
    assert!(result.warnings.is_empty());
    assert_eq!(result.response.model_id.as_deref(), Some(""));

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    assert_eq!(body["text"], "Hello from xAI");
    assert_eq!(body["voice_id"], "eve");
    assert_eq!(body["language"], "auto");
    assert_eq!(body["speed"], 1.5);
    assert_eq!(
        body["output_format"],
        json!({ "codec": "mp3", "sample_rate": 44100 })
    );
}

#[tokio::test]
async fn speech_api_error_surfaces_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tts"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": { "message": "rate limited", "type": "rate_limit_error" }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).speech("");
    let err = model
        .do_generate_speech(SpeechModelV4CallOptions::new("hi"))
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("rate limited"), "{err}");
}

// ─── Transcription ───────────────────────────────────────────────────────

#[tokio::test]
async fn transcription_do_generate_parses_verbose_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/stt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "Hello world",
            "language": "en",
            "duration": 2.5,
            "words": [
                { "text": "Hello", "start": 0.0, "end": 1.0 },
                { "text": "world", "start": 1.0, "end": 2.5 }
            ]
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).transcription("");
    let options = TranscriptionModelV4CallOptions::new(vec![1u8, 2, 3], "audio/wav")
        .with_provider_options(
            xai_options(json!({ "language": "en", "keyterm": ["Grok", "xAI"] }))
                .expect("provider options"),
        );
    let result = model.do_transcribe(options).await.expect("transcribe");

    assert_eq!(result.text, "Hello world");
    assert_eq!(result.language.as_deref(), Some("en"));
    assert_eq!(result.duration_in_seconds, Some(2.5));
    assert_eq!(
        result.segments,
        Some(vec![
            TranscriptionSegmentV4::new("Hello", 0.0, 1.0),
            TranscriptionSegmentV4::new("world", 1.0, 2.5),
        ])
    );

    // Multipart request: scalar fields first, the audio `file` field last.
    let requests = server.received_requests().await.expect("requests");
    let content_type = requests[0]
        .headers
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("multipart/form-data"),
        "{content_type}"
    );
    let raw_body = String::from_utf8_lossy(&requests[0].body).to_string();
    let language_pos = raw_body.find("name=\"language\"").expect("language field");
    let keyterm_pos = raw_body.find("name=\"keyterm\"").expect("keyterm field");
    let file_pos = raw_body.find("name=\"file\"").expect("file field");
    assert!(language_pos < file_pos, "file must be the final field");
    assert!(keyterm_pos < file_pos, "file must be the final field");
    assert!(raw_body.contains("filename=\"audio.wav\""));
    assert!(raw_body.contains("Grok"));
    assert!(raw_body.contains("xAI"));
}

#[tokio::test]
async fn transcription_treats_empty_language_and_missing_words_as_absent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/stt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "Silence",
            "language": ""
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).transcription("");
    let result = model
        .do_transcribe(TranscriptionModelV4CallOptions::new(
            vec![0u8],
            "audio/mpeg",
        ))
        .await
        .expect("transcribe");

    assert_eq!(result.text, "Silence");
    assert_eq!(result.language, None);
    assert_eq!(result.duration_in_seconds, None);
    assert_eq!(result.segments, Some(vec![]));
}

// ─── Video ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn video_do_generate_polls_until_done() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req-1"
        })))
        .mount(&server)
        .await;
    // First poll: still pending. Mounted first with a one-response budget so
    // the follow-up poll falls through to the `done` mock below.
    Mock::given(method("GET"))
        .and(path("/videos/req-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "pending"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "done",
            "video": { "url": "https://vidgen.x.ai/out.mp4", "duration": 5.0 }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let options = VideoModelV4CallOptions {
        prompt: "a cat playing piano".into(),
        provider_options: xai_options(json!({ "pollIntervalMs": 10 })),
        ..Default::default()
    };
    let result = model.do_generate_video(options).await.expect("video");

    assert_eq!(result.videos.len(), 1);
    assert_eq!(
        result.videos[0].as_url(),
        Some("https://vidgen.x.ai/out.mp4")
    );
    assert_eq!(result.videos[0].content_type.as_deref(), Some("video/mp4"));

    // create + 2 polls
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 3);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    assert_eq!(body["model"], "grok-imagine-video");
    assert_eq!(body["prompt"], "a cat playing piano");
    // Poll controls never reach the wire body.
    assert!(body.get("pollIntervalMs").is_none());
}

#[tokio::test]
async fn video_edit_mode_posts_to_edits_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req-edit"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-edit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "done",
            "video": { "url": "https://vidgen.x.ai/edited.mp4" }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let options = VideoModelV4CallOptions {
        prompt: "make it rain".into(),
        provider_options: xai_options(json!({
            "mode": "edit-video",
            "videoUrl": "https://example.com/src.mp4",
            "pollIntervalMs": 10,
        })),
        ..Default::default()
    };
    let result = model.do_generate_video(options).await.expect("video");
    assert_eq!(
        result.videos[0].as_url(),
        Some("https://vidgen.x.ai/edited.mp4")
    );

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    assert_eq!(
        body["video"],
        json!({ "url": "https://example.com/src.mp4" })
    );
}

#[tokio::test]
async fn video_polling_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req-slow"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-slow"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "pending"
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let options = VideoModelV4CallOptions {
        prompt: "slow".into(),
        provider_options: xai_options(json!({
            "pollIntervalMs": 10,
            "pollTimeoutMs": 35,
        })),
        ..Default::default()
    };
    let err = model
        .do_generate_video(options)
        .await
        .expect_err("must time out");
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn video_failed_status_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req-bad"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-bad"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "failed"
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let options = VideoModelV4CallOptions {
        prompt: "bad".into(),
        provider_options: xai_options(json!({ "pollIntervalMs": 10 })),
        ..Default::default()
    };
    let err = model.do_generate_video(options).await.expect_err("failed");
    assert!(err.to_string().contains("Video generation failed"), "{err}");
}

#[tokio::test]
async fn video_moderation_block_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "req-mod"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-mod"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "done",
            "video": { "url": "https://vidgen.x.ai/out.mp4", "respect_moderation": false }
        })))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let options = VideoModelV4CallOptions {
        prompt: "blocked".into(),
        provider_options: xai_options(json!({ "pollIntervalMs": 10 })),
        ..Default::default()
    };
    let err = model.do_generate_video(options).await.expect_err("blocked");
    assert!(err.to_string().contains("content policy"), "{err}");
}

#[tokio::test]
async fn video_missing_request_id_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let model = provider_for(&server).video("grok-imagine-video");
    let err = model
        .do_generate_video(VideoModelV4CallOptions::new("no id"))
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("request_id"), "{err}");
}
