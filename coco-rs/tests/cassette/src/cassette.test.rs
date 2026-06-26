use super::*;

#[test]
fn builder_records_streaming_interaction() {
    let mut b = CassetteBuilder::new();
    b.on_request(
        "POST",
        "https://api.anthropic.com/v1/messages",
        br#"{"model":"x","stream":true}"#,
    );
    b.on_response_chunk(b"event: message_start\n");
    b.on_response_chunk(b"data: {}\n\n");
    b.finish_stream(200, "text/event-stream");

    let cassette = b.build();
    assert_eq!(cassette.interactions.len(), 1);
    let i = &cassette.interactions[0];
    assert_eq!(i.request.method, "POST");
    assert_eq!(i.request.path, "/v1/messages");
    assert_eq!(
        i.request.body,
        serde_json::json!({"model":"x","stream":true})
    );
    assert_eq!(i.response.status, 200);
    assert_eq!(i.response.content_type, "text/event-stream");
    assert!(i.response.body.contains("message_start"));
}

#[test]
fn save_round_trips_through_load() {
    let path = std::env::temp_dir().join("coco-cassette-roundtrip.json");
    let _ = std::fs::remove_file(&path);

    let mut b = CassetteBuilder::new();
    b.on_request("POST", "http://x/v1/messages", br#"{"a":1}"#);
    b.on_response_body(200, "application/json", br#"{"ok":true}"#);
    let cassette = b.build();

    cassette.save(&path).expect("save");
    let loaded = Cassette::load(&path).expect("load");
    assert_eq!(loaded, cassette);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn load_missing_is_distinct_error() {
    let err = Cassette::load("/nonexistent/coco/cassette.json").unwrap_err();
    assert!(matches!(err, CassetteError::Missing { .. }), "{err:?}");
}

#[test]
fn save_refuses_when_a_secret_survives_redaction() {
    // Built directly (bypassing the redacting builder) to simulate a redaction
    // miss; `save`'s whole-file re-scan is the backstop that blocks the write.
    let cassette = Cassette::new(vec![Interaction {
        request: RecordedRequest {
            method: "POST".into(),
            path: "/x".into(),
            body: serde_json::Value::Null,
        },
        response: RecordedResponse {
            status: 200,
            content_type: "application/json".into(),
            body: "ghp_0123456789abcdefghijABCDEFGHIJ012345".into(),
        },
    }]);

    let path = std::env::temp_dir().join("coco-cassette-unsafe.json");
    let _ = std::fs::remove_file(&path);
    let err = cassette.save(&path).unwrap_err();
    assert!(
        matches!(err, CassetteError::UnsafeCassette { .. }),
        "{err:?}"
    );
    assert!(!path.exists(), "an unsafe cassette must never be written");
}

#[test]
fn builder_redacts_secrets_on_arrival() {
    let mut b = CassetteBuilder::new();
    b.on_request("POST", "http://x/m", b"{}");
    b.on_response_body(
        200,
        "application/json",
        b"token: ghp_0123456789abcdefghijABCDEFGHIJ012345",
    );
    let cassette = b.build();
    // The redacting builder already scrubbed it, so save() succeeds.
    assert!(
        !cassette.interactions[0]
            .response
            .body
            .contains("ghp_0123456789")
    );
    let path = std::env::temp_dir().join("coco-cassette-redacted-ok.json");
    let _ = std::fs::remove_file(&path);
    cassette
        .save(&path)
        .expect("redacted cassette is safe to write");
    let _ = std::fs::remove_file(&path);
}
