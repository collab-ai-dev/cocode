//! HTTP-level coverage of the token endpoint (`post_token`) and the refresh
//! executor (`refresh_at`) against a wiremock server — the highest-risk wire
//! logic that unit tests over in-memory state can't reach.
//!
//! These drive `refresh_at` (the URL-injectable variant) directly, so they need
//! neither a browser loopback nor the debug-gated `COCO_AUTH_*_TOKEN_URL` env
//! override (which is process-global and would race across tests).

use coco_provider_auth::descriptor::BodyEncoding;
use coco_provider_auth::descriptor::GEMINI_CODE_ASSIST;
use coco_provider_auth::descriptor::OPENAI_CHATGPT;
use coco_provider_auth::error::ProviderAuthError;
use coco_provider_auth::refresh::post_token;
use coco_provider_auth::refresh::refresh_at;
use coco_provider_auth::refresh::revoke;
use coco_provider_auth::token_cell::TokenSnapshot;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn revoke_posts_token_to_revoke_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // Point a copy of the descriptor's revoke_url at the mock (test-only leak).
    let mut d = OPENAI_CHATGPT;
    let url: &'static str = Box::leak(format!("{}/revoke", server.uri()).into_boxed_str());
    d.revoke_url = Some(url);

    revoke(&d, "rt-token", &reqwest::Client::new())
        .await
        .expect("revoke ok");

    let reqs = server.received_requests().await.unwrap();
    let r = reqs
        .iter()
        .find(|r| r.url.path() == "/revoke")
        .expect("revoke endpoint was called");
    let body = String::from_utf8_lossy(&r.body);
    assert!(
        body.contains("token=rt-token"),
        "body must carry the token; got: {body}"
    );
    assert!(
        body.contains("client_id="),
        "body must carry client_id; got: {body}"
    );
}

#[tokio::test]
async fn revoke_is_noop_without_revoke_url() {
    let mut d = OPENAI_CHATGPT;
    d.revoke_url = None;
    // No URL → Ok without any network call.
    revoke(&d, "tok", &reqwest::Client::new())
        .await
        .expect("no-op revoke is Ok");
}

fn prev(refresh_token: &str, login_epoch: u64) -> TokenSnapshot {
    TokenSnapshot {
        access_token: "old-access".into(),
        account_id: Some("acct-1".into()),
        refresh_token: Some(refresh_token.into()),
        subscription_type: Some("pro".into()),
        expires_at_ms: Some(0),
        login_epoch,
    }
}

async fn mock_token_endpoint(response: ResponseTemplate) -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(response)
        .mount(&server)
        .await;
    let url = format!("{}/token", server.uri());
    (server, url)
}

#[tokio::test]
async fn refresh_rotates_keeps_new_token_and_carries_identity() {
    let (_server, url) = mock_token_endpoint(ResponseTemplate::new(200).set_body_json(json!({
        "access_token": "new-access",
        "refresh_token": "new-refresh",
        "expires_in": 3600
    })))
    .await;
    let http = reqwest::Client::new();
    let out = refresh_at(
        &url,
        &OPENAI_CHATGPT,
        "openai-chatgpt",
        &prev("old-refresh", 7),
        &http,
    )
    .await
    .expect("refresh ok");

    assert_eq!(out.access_token, "new-access");
    assert_eq!(out.refresh_token.as_deref(), Some("new-refresh"));
    // Identity fields survive a token rotation.
    assert_eq!(out.login_epoch, 7);
    assert_eq!(out.subscription_type.as_deref(), Some("pro"));
    assert!(out.expires_at_ms.is_some_and(|e| e > 0));
}

#[tokio::test]
async fn refresh_persists_flow_keeps_old_token_when_omitted() {
    // GEMINI_CODE_ASSIST is `RefreshTokenRotation::Persists`: the server may
    // omit a new refresh token and the old one must be retained.
    let (_server, url) = mock_token_endpoint(ResponseTemplate::new(200).set_body_json(json!({
        "access_token": "new-access",
        "expires_in": 3600
    })))
    .await;
    let http = reqwest::Client::new();
    let out = refresh_at(
        &url,
        &GEMINI_CODE_ASSIST,
        "gemini-code-assist",
        &prev("keep-me", 3),
        &http,
    )
    .await
    .expect("refresh ok");

    assert_eq!(out.refresh_token.as_deref(), Some("keep-me"));
}

#[tokio::test]
async fn refresh_rotates_without_new_token_retains_old() {
    // The Rotates+omitted defensive branch: keep the old token (and warn).
    let (_server, url) = mock_token_endpoint(ResponseTemplate::new(200).set_body_json(json!({
        "access_token": "new-access",
        "expires_in": 3600
    })))
    .await;
    let http = reqwest::Client::new();
    let out = refresh_at(
        &url,
        &OPENAI_CHATGPT,
        "openai-chatgpt",
        &prev("still-here", 1),
        &http,
    )
    .await
    .expect("refresh ok");

    assert_eq!(out.refresh_token.as_deref(), Some("still-here"));
}

#[tokio::test]
async fn refresh_401_maps_to_session_expired() {
    let (_server, url) = mock_token_endpoint(
        ResponseTemplate::new(401).set_body_string(r#"{"error":"invalid_grant"}"#),
    )
    .await;
    let http = reqwest::Client::new();
    let err = refresh_at(
        &url,
        &OPENAI_CHATGPT,
        "openai-chatgpt",
        &prev("dead", 1),
        &http,
    )
    .await
    .expect_err("401 must error");

    assert!(
        matches!(err, ProviderAuthError::SessionExpired { .. }),
        "401 on refresh ⇒ SessionExpired; got: {err:?}"
    );
}

#[tokio::test]
async fn refresh_without_refresh_token_is_session_expired() {
    let http = reqwest::Client::new();
    let mut p = prev("unused", 1);
    p.refresh_token = None;
    let err = refresh_at(
        "http://127.0.0.1:1/never-called",
        &OPENAI_CHATGPT,
        "openai-chatgpt",
        &p,
        &http,
    )
    .await
    .expect_err("no refresh token");

    assert!(matches!(err, ProviderAuthError::SessionExpired { .. }));
}

#[tokio::test]
async fn post_token_non_2xx_redacts_and_caps_body() {
    // An error body carrying an OpenAI-style secret, longer than the 512 cap.
    let leak = format!("error sk-{} tail{}", "A".repeat(40), "z".repeat(1000));
    let (_server, url) =
        mock_token_endpoint(ResponseTemplate::new(400).set_body_string(leak)).await;
    let http = reqwest::Client::new();
    let err = post_token(
        &http,
        &url,
        BodyEncoding::Form,
        &[("grant_type", "x".into())],
    )
    .await
    .expect_err("400 must error");

    match err {
        ProviderAuthError::TokenEndpoint {
            status, message, ..
        } => {
            assert_eq!(status, 400);
            assert!(
                message.len() <= 512,
                "body must be capped; len={}",
                message.len()
            );
            assert!(
                !message.contains("sk-AAAA"),
                "the secret must be redacted; got: {message}"
            );
        }
        other => panic!("expected TokenEndpoint, got: {other:?}"),
    }
}

#[tokio::test]
async fn post_token_decodes_success_response() {
    let (_server, url) = mock_token_endpoint(
        ResponseTemplate::new(200).set_body_json(json!({"access_token": "abc", "expires_in": 120})),
    )
    .await;
    let http = reqwest::Client::new();
    let tr = post_token(&http, &url, BodyEncoding::Json, &[("k", "v".into())])
        .await
        .expect("decode ok");

    assert_eq!(tr.access_token, "abc");
    assert_eq!(tr.expires_in, Some(120));
}
