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

#[tokio::test]
#[should_panic(expected = "path mismatch")]
async fn verify_panics_on_path_mismatch() {
    // Recorded path is `/messages`; the request hits a different path with a
    // matching body. The strengthened guard catches the wrong-endpoint hit that
    // a body-only matcher would silently allow.
    let player =
        CassettePlayer::start(cassette_with(serde_json::json!({ "model": "x" }), "ok")).await;

    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{}/WRONG", player.base_url()))
        .json(&serde_json::json!({ "model": "x" }))
        .send()
        .await;

    player.verify(); // panics: recorded path != requested path
}

#[tokio::test]
async fn any_order_matches_requests_out_of_recorded_order() {
    // Two interactions with distinct bodies, requested in reverse order. Strict
    // mode would flag both as body mismatches; any-order matches each request to
    // the interaction with the same shape.
    let cassette = Cassette::new(vec![
        Interaction {
            request: RecordedRequest {
                method: "POST".into(),
                path: "/messages".into(),
                body: serde_json::json!({ "n": 1 }),
            },
            response: RecordedResponse {
                status: 200,
                content_type: "application/json".into(),
                body: "first".into(),
            },
        },
        Interaction {
            request: RecordedRequest {
                method: "POST".into(),
                path: "/messages".into(),
                body: serde_json::json!({ "n": 2 }),
            },
            response: RecordedResponse {
                status: 200,
                content_type: "application/json".into(),
                body: "second".into(),
            },
        },
    ])
    .with_any_order();

    let player = CassettePlayer::start(cassette).await;
    let client = reqwest::Client::new();

    // Request #2's body first.
    let r2 = client
        .post(format!("{}/messages", player.base_url()))
        .json(&serde_json::json!({ "n": 2 }))
        .send()
        .await
        .expect("request 2");
    assert_eq!(r2.text().await.expect("body"), "second");

    // Then #1's body.
    let r1 = client
        .post(format!("{}/messages", player.base_url()))
        .json(&serde_json::json!({ "n": 1 }))
        .send()
        .await
        .expect("request 1");
    assert_eq!(r1.text().await.expect("body"), "first");

    player.verify();
    assert_eq!(player.consumed(), 2);
}
