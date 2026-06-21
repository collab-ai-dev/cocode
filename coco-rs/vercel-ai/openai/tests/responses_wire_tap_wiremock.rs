//! End-to-end wire-tap coverage for the Responses transport.
//!
//! Drives `do_stream` against a wiremock with a `WireTap` attached and
//! asserts the tap actually receives the request, the streamed response
//! chunks, and — on an HTTP error — the error *body*. This covers the
//! provider → provider-utils → tap path that the recorder unit tests
//! bypass (they call the tap methods directly), and specifically locks
//! the HTTP-error-body capture that a delegate-only wrapper would miss.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use futures::StreamExt;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::create_openai;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::WireTap;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[derive(Default, Debug)]
struct Captured {
    request_bodies: Vec<Vec<u8>>,
    chunks: Vec<Vec<u8>>,
    error_bodies: Vec<(u16, Vec<u8>)>,
}

#[derive(Debug)]
struct RecordingTap(Mutex<Captured>);

impl WireTap for RecordingTap {
    fn on_request(&self, _url: &str, _headers: &HashMap<String, String>, body: &[u8]) {
        self.0.lock().unwrap().request_bodies.push(body.to_vec());
    }
    fn on_response_chunk(&self, chunk: &[u8]) {
        self.0.lock().unwrap().chunks.push(chunk.to_vec());
    }
    fn on_response_body(&self, status: u16, _headers: &HashMap<String, String>, body: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .error_bodies
            .push((status, body.to_vec()));
    }
}

fn options_with_tap(tap: Arc<RecordingTap>) -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart::new("hi"))],
            provider_options: None,
        }],
        wire_tap: Some(tap),
        ..Default::default()
    }
}

#[tokio::test]
async fn streaming_success_tees_request_and_chunks() {
    let server = MockServer::start().await;
    let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n\
               data: {\"type\":\"response.completed\"}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_openai(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    });
    let tap = Arc::new(RecordingTap(Mutex::new(Captured::default())));
    let options = options_with_tap(tap.clone());

    let mut result = provider
        .responses("gpt-test")
        .do_stream(&options, None)
        .await
        .expect("stream opens");
    while result.stream.next().await.is_some() {}

    let cap = tap.0.lock().unwrap();
    assert_eq!(cap.request_bodies.len(), 1, "request fed exactly once");
    assert!(!cap.chunks.is_empty(), "response chunks teed to the tap");
    let joined = String::from_utf8_lossy(&cap.chunks.concat()).into_owned();
    assert!(
        joined.contains("response.completed"),
        "teed bytes: {joined}"
    );
    assert!(cap.error_bodies.is_empty(), "no error body on success");
}

#[tokio::test]
async fn http_error_feeds_error_body_to_tap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"error":{"message":"bad reasoning item"}}"#),
        )
        .mount(&server)
        .await;

    let provider = create_openai(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    });
    let tap = Arc::new(RecordingTap(Mutex::new(Captured::default())));
    let options = options_with_tap(tap.clone());

    let _err = provider
        .responses("gpt-test")
        .do_stream(&options, None)
        .await
        .expect_err("400 must error");

    let cap = tap.0.lock().unwrap();
    assert_eq!(cap.request_bodies.len(), 1, "request still fed on error");
    assert_eq!(cap.error_bodies.len(), 1, "error body captured");
    assert_eq!(cap.error_bodies[0].0, 400, "status captured");
    let body = String::from_utf8_lossy(&cap.error_bodies[0].1);
    assert!(body.contains("bad reasoning item"), "error body: {body}");
}
