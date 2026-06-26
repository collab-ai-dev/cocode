use super::*;
use crate::cassette::Cassette;
use crate::cassette::Interaction;
use crate::cassette::RecordedRequest;
use crate::cassette::RecordedResponse;

fn cassette_with(req_body: serde_json::Value, resp: &str) -> Cassette {
    Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".into(),
            path: "/messages".into(),
            body: req_body,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "text/event-stream".into(),
            body: resp.into(),
        },
    }])
}

#[tokio::test]
async fn player_replays_recorded_response_and_verifies() {
    let player = CassettePlayer::start(cassette_with(
        serde_json::json!({ "model": "x" }),
        "event: done\ndata: {}\n\n",
    ))
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/messages", player.base_url()))
        // Key order intentionally differs from nothing here, but the matcher is
        // canonicalized JSON so order never matters.
        .json(&serde_json::json!({ "model": "x" }))
        .send()
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.expect("body");
    assert!(text.contains("event: done"));

    player.verify();
    assert_eq!(player.consumed(), 1);
}

#[tokio::test]
#[should_panic(expected = "request body mismatch")]
async fn verify_panics_on_request_body_mismatch() {
    let player = CassettePlayer::start(cassette_with(
        serde_json::json!({ "model": "expected" }),
        "ok",
    ))
    .await;

    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{}/messages", player.base_url()))
        .json(&serde_json::json!({ "model": "WRONG" }))
        .send()
        .await;

    player.verify(); // panics: recorded request body != sent body
}

#[tokio::test]
#[should_panic(expected = "unused recorded interactions")]
async fn verify_panics_when_interactions_unconsumed() {
    let player = CassettePlayer::start(cassette_with(serde_json::json!({ "a": 1 }), "x")).await;
    // No request issued → the single interaction stays unconsumed.
    player.verify();
}
