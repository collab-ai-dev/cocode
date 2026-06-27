//! Wire-level tests for the Code Assist transport: the request envelope, the
//! Bearer header, the `{response}` unwrap (non-stream + SSE), and the
//! `loadCodeAssist`/`onboardUser` onboarding handshake — all against wiremock.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;
use vercel_ai_google_codeassist::CodeAssistCreds;
use vercel_ai_google_codeassist::GoogleCodeAssistProviderSettings;
use vercel_ai_google_codeassist::create_google_code_assist;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

fn one_shot_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart::new("hi gemini"))],
            provider_options: None,
        }],
        ..Default::default()
    }
}

/// `loadCodeAssist` reply that short-circuits onboarding (account already has a
/// tier + project), so no `onboardUser`/LRO round-trips.
fn already_onboarded(project: &str) -> serde_json::Value {
    json!({ "currentTier": { "id": "free-tier" }, "cloudaicompanionProject": project })
}

fn generate_response(text: &str) -> serde_json::Value {
    json!({
        "response": {
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": text }] },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 3, "candidatesTokenCount": 4 }
        }
    })
}

fn creds(project_id: Option<&str>) -> GoogleCodeAssistProviderSettings {
    let project_id = project_id.map(str::to_string);
    GoogleCodeAssistProviderSettings {
        base_url: None, // set per test to the mock server
        creds: Arc::new(move || {
            Some(CodeAssistCreds {
                access_token: "tok-123".to_string(),
                project_id: project_id.clone(),
            })
        }),
        headers: None,
        name: None,
        client: None,
    }
}

fn first_text(content: &[AssistantContentPart]) -> Option<String> {
    content.iter().find_map(|p| match p {
        AssistantContentPart::Text(t) => Some(t.text.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn do_generate_wraps_envelope_unwraps_response_and_sends_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:loadCodeAssist$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(already_onboarded("proj-1")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:generateContent$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(generate_response("hello from code assist")),
        )
        .mount(&server)
        .await;

    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        ..creds(None)
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    let result = model
        .do_generate(&one_shot_options(), None)
        .await
        .expect("do_generate ok");

    assert_eq!(
        first_text(&result.content).as_deref(),
        Some("hello from code assist")
    );

    // The generateContent request must be the Code Assist envelope with a Bearer.
    let reqs = server.received_requests().await.unwrap();
    let gen_req = reqs
        .iter()
        .find(|r| r.url.path().ends_with(":generateContent"))
        .expect("generateContent was called");
    let body: serde_json::Value = serde_json::from_slice(&gen_req.body).unwrap();
    assert_eq!(body["model"], "gemini-2.5-pro");
    assert_eq!(body["project"], "proj-1");
    assert!(body["user_prompt_id"].is_string());
    assert!(body["request"]["contents"].is_array());
    assert_eq!(
        gen_req
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer tok-123"
    );
}

#[tokio::test]
async fn do_generate_runs_onboarding_when_no_project() {
    let server = MockServer::start().await;
    // No current tier → onboardUser; the LRO completes immediately with a project.
    Mock::given(method("POST"))
        .and(path_regex(r".*:loadCodeAssist$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "allowedTiers": [{ "id": "free-tier", "isDefault": true }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:onboardUser$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "operations/op1",
            "done": true,
            "response": { "cloudaicompanionProject": { "id": "proj-onboarded" } }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:generateContent$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(generate_response("onboarded ok")))
        .mount(&server)
        .await;

    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        ..creds(None)
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    let result = model
        .do_generate(&one_shot_options(), None)
        .await
        .expect("do_generate ok after onboarding");
    assert_eq!(first_text(&result.content).as_deref(), Some("onboarded ok"));

    // The discovered project rides the generateContent envelope.
    let reqs = server.received_requests().await.unwrap();
    let gen_req = reqs
        .iter()
        .find(|r| r.url.path().ends_with(":generateContent"))
        .expect("generateContent was called");
    let body: serde_json::Value = serde_json::from_slice(&gen_req.body).unwrap();
    assert_eq!(body["project"], "proj-onboarded");
}

#[tokio::test]
async fn do_stream_unwraps_code_assist_sse_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:loadCodeAssist$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(already_onboarded("proj-1")))
        .mount(&server)
        .await;
    // SSE chunks are wrapped as {"response": {...}} — the transport must unwrap.
    let sse = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"streamed!\"}]},\"finishReason\":\"STOP\"}]}}\n\n";
    Mock::given(method("POST"))
        .and(path_regex(r".*:streamGenerateContent$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        ..creds(None)
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    let mut result = model
        .do_stream(&one_shot_options(), None)
        .await
        .expect("do_stream opens");

    let mut text = String::new();
    while let Some(part) = result.stream.next().await {
        if let Ok(LanguageModelV4StreamPart::TextDelta { delta, .. }) = part {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "streamed!");
}

#[tokio::test]
async fn do_generate_not_logged_in_errors() {
    let server = MockServer::start().await;
    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        // Supplier returns None → not logged in.
        creds: Arc::new(|| None),
        headers: None,
        name: None,
        client: None,
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    let err = match model.do_generate(&one_shot_options(), None).await {
        Ok(_) => panic!("must error when not logged in"),
        Err(e) => e,
    };
    assert!(
        err.to_string().to_lowercase().contains("login"),
        "error should tell the user to log in; got: {err}"
    );
}

#[tokio::test]
async fn onboarding_validation_required_surfaces_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:loadCodeAssist$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ineligibleTiers": [{
                "reasonCode": "VALIDATION_REQUIRED",
                "reasonMessage": "Please validate your account",
                "validationUrl": "https://validate.example/x"
            }]
        })))
        .mount(&server)
        .await;

    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        ..creds(None)
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    let err = match model.do_generate(&one_shot_options(), None).await {
        Ok(_) => panic!("validation-required must error"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("validate") && msg.contains("https://validate.example/x"),
        "must surface the validation URL; got: {msg}"
    );
}

#[tokio::test]
async fn onboard_free_tier_sends_no_project() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:loadCodeAssist$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "allowedTiers": [{ "id": "free-tier", "isDefault": true }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:onboardUser$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "done": true,
            "response": { "cloudaicompanionProject": { "id": "p" } }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r".*:generateContent$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(generate_response("ok")))
        .mount(&server)
        .await;

    let provider = create_google_code_assist(GoogleCodeAssistProviderSettings {
        base_url: Some(format!("{}/v1internal", server.uri())),
        ..creds(None)
    });
    let model = provider.language_model_instance("gemini-2.5-pro");
    model
        .do_generate(&one_shot_options(), None)
        .await
        .expect("ok");

    let reqs = server.received_requests().await.unwrap();
    let onboard = reqs
        .iter()
        .find(|r| r.url.path().ends_with(":onboardUser"))
        .expect("onboardUser was called");
    let body: serde_json::Value = serde_json::from_slice(&onboard.body).unwrap();
    assert_eq!(body["tierId"], "free-tier");
    // jcode parity: the free tier sends NO project in the onboard body.
    assert!(
        body.get("cloudaicompanionProject").is_none() || body["cloudaicompanionProject"].is_null(),
        "free-tier onboard must omit the project; got: {body}"
    );
}
