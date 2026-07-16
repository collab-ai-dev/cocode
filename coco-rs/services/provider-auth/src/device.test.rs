use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;
use crate::descriptor::OAuthGrant;
use crate::descriptor::UserCodePolicy;
use crate::descriptor::XAI_GROK;

#[tokio::test]
async fn device_login_requests_code_polls_and_builds_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .and(header("x-grok-client-surface", "ui"))
        .and(body_string_contains("grok-cli%3Aaccess"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://accounts.x.ai/device",
            "verification_uri_complete": "https://accounts.x.ai/device?user_code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 1
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .and(body_string_contains("device_code=device-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyLTEiLCJlbWFpbCI6InVAZXhhbXBsZS5jb20ifQ.sig"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let seen_url = Arc::new(Mutex::new(None));
    let sink_state = seen_url.clone();
    let opts = LoginOptions {
        open_browser: false,
        paste: false,
        timeout: Some(Duration::from_secs(5)),
        surface: LoginSurface::Ui,
        on_authorize_url: Some(Arc::new(move |url| {
            *sink_state.lock().expect("url sink lock") = Some(url);
        })),
    };
    let OAuthGrant::DeviceCode(grant) = XAI_GROK.grant else {
        panic!("xAI must use device authorization")
    };
    let credential = login_at(
        &XAI_GROK,
        "grok",
        grant,
        DeviceEndpoints {
            authorize_url: format!("{}/device/code", server.uri()),
            token_url: format!("{}/token", server.uri()),
        },
        &opts,
        Duration::from_secs(5),
        &reqwest::Client::new(),
    )
    .await
    .expect("device login succeeds");

    assert_eq!(credential.flow, coco_types::OAuthFlowId::XaiGrok);
    assert_eq!(credential.access_token, "access-token");
    assert_eq!(credential.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(credential.account_id.as_deref(), Some("user-1"));
    assert_eq!(credential.email.as_deref(), Some("u@example.com"));
    assert_eq!(
        seen_url.lock().expect("url sink lock").as_deref(),
        Some("https://accounts.x.ai/device?user_code=ABCD-EFGH")
    );
}

#[test]
fn display_url_adds_user_code_and_rejects_unsafe_scheme() {
    let device = DeviceCodeResponse {
        device_code: "secret".into(),
        user_code: "ABCD-EFGH".into(),
        verification_uri: "https://accounts.x.ai/device".into(),
        verification_uri_complete: None,
        expires_in: Some(600),
        interval: Some(5),
    };
    assert_eq!(
        display_url(&device).expect("safe URL"),
        "https://accounts.x.ai/device?user_code=ABCD-EFGH"
    );

    let unsafe_device = DeviceCodeResponse {
        verification_uri_complete: Some("javascript:alert(1)".into()),
        ..device
    };
    assert!(display_url(&unsafe_device).is_err());
}

#[test]
fn user_code_accepts_display_safe_characters_only() {
    let policy = UserCodePolicy::AsciiAlphanumericDash;
    assert!(validate_user_code("ABCD-1234", policy).is_ok());
    assert!(validate_user_code("", policy).is_err());
    assert!(validate_user_code("abcd-1234", policy).is_ok());
    assert!(validate_user_code("ABCD\n1234", policy).is_err());
}

#[test]
fn endpoint_errors_never_echo_server_descriptions_or_tokens() {
    let body = r#"{"error":"invalid_grant","error_description":"leaked rt_secret_value"}"#;
    let rendered =
        endpoint_error("device token", reqwest::StatusCode::BAD_REQUEST, body).to_string();
    assert!(rendered.contains("invalid_grant"));
    assert!(!rendered.contains("rt_secret_value"));
    assert!(!rendered.contains("error_description"));
}

#[test]
fn polling_state_machine_handles_pending_slowdown_and_terminal_states() {
    let mut interval = Duration::from_secs(2);
    assert_eq!(
        apply_poll_error("authorization_pending", "Grok", "grok", &mut interval).unwrap(),
        PollAction::Continue
    );
    assert_eq!(interval, Duration::from_secs(2));

    assert_eq!(
        apply_poll_error("slow_down", "Grok", "grok", &mut interval).unwrap(),
        PollAction::Continue
    );
    assert_eq!(interval, Duration::from_secs(7));

    for code in ["access_denied", "expired_token", "invalid_request"] {
        assert!(apply_poll_error(code, "Grok", "grok", &mut interval).is_err());
    }
}
