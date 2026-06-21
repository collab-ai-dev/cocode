use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use coco_config::WireDumpLevel;
use pretty_assertions::assert_eq;

use super::SessionWireRecorder;
use super::WireDumpConfig;
use super::WireOutcome;
use super::WireRecord;
use super::WireSink;
use super::WireTurnCtx;

/// In-memory sink: proves the recorder → sink seam without touching the
/// filesystem, and that the record reaching any sink is already redacted.
#[derive(Debug, Default)]
struct VecSink(Mutex<Vec<(WireRecord, bool)>>);

impl WireSink for VecSink {
    fn emit(&self, record: &WireRecord, persist_bodies: bool) {
        self.0
            .lock()
            .unwrap()
            .push((record.clone(), persist_bodies));
    }
}

fn ctx() -> WireTurnCtx<'static> {
    WireTurnCtx {
        turn_id: "turn-3",
        provider: "openai",
        model: "gpt-5.4",
    }
}

fn cfg(dir: &std::path::Path, level: WireDumpLevel) -> WireDumpConfig {
    WireDumpConfig::new(dir.to_path_buf(), level, 1024, /*redact*/ true)
}

fn read(dir: &std::path::Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join("wire").join(name)).ok()
}

/// Headers used in tests (the `WireTap` trait takes a `HashMap`).
fn auth_headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert(
        "authorization".to_string(),
        "Bearer sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
    );
    h
}

#[test]
fn off_level_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::Off).begin(ctx());
    rec.on_request("https://api/responses", &HashMap::new(), b"{}");
    rec.on_response_chunk(b"data: {\"type\":\"error\"}\n\n");
    rec.finish(WireOutcome::Failure);
    assert!(read(tmp.path(), "index.jsonl").is_none());
}

#[test]
fn finish_failure_persists_bodies_at_error_level() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::Error).begin(ctx());
    rec.on_request(
        "https://api/responses",
        &HashMap::new(),
        br#"{"model":"gpt-5.4"}"#,
    );
    rec.on_response_chunk(b"data: {\"type\":\"error\",\"code\":\"server_error\"}\n\n");
    rec.finish(WireOutcome::Failure);

    let index = read(tmp.path(), "index.jsonl").expect("index written");
    assert!(index.contains("\"outcome\":\"error\""), "index: {index}");
    assert!(index.contains("\"bodies\":true"), "index: {index}");
    assert!(index.contains("\"transport\":\"stream\""), "index: {index}");
    let resp = read(tmp.path(), "0001-turn-3-openai.resp.txt").expect("resp written");
    assert!(resp.contains("server_error"), "resp: {resp}");
    assert!(read(tmp.path(), "0001-turn-3-openai.req.json").is_some());
}

#[test]
fn finish_success_discards_bodies_at_error_level() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::Error).begin(ctx());
    rec.on_request("https://api/responses", &HashMap::new(), b"{}");
    // A successful turn with no chunks (non-streaming) must NOT be
    // misclassified as a failure — the typed outcome decides.
    rec.finish(WireOutcome::Success);

    let index = read(tmp.path(), "index.jsonl").expect("index always written");
    assert!(index.contains("\"outcome\":\"ok\""), "index: {index}");
    assert!(index.contains("\"bodies\":false"), "index: {index}");
    assert!(read(tmp.path(), "0001-turn-3-openai.resp.txt").is_none());
    assert!(read(tmp.path(), "0001-turn-3-openai.req.json").is_none());
}

#[test]
fn all_level_persists_on_success() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::All).begin(ctx());
    rec.on_request("https://api/responses", &HashMap::new(), b"{}");
    rec.on_response_chunk(b"data: {\"type\":\"response.completed\"}\n\n");
    rec.finish(WireOutcome::Success);
    assert!(read(tmp.path(), "0001-turn-3-openai.resp.txt").is_some());
    assert!(read(tmp.path(), "0001-turn-3-openai.req.json").is_some());
}

#[test]
fn http_error_body_is_captured() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::Error).begin(ctx());
    rec.on_request("https://api/responses", &HashMap::new(), b"{}");
    // Transport feeds the error body (P0): a 400 with a structured body.
    rec.on_response_body(
        400,
        &HashMap::new(),
        br#"{"error":{"message":"reasoning item missing required following item"}}"#,
    );
    rec.finish(WireOutcome::Failure);

    let resp = read(tmp.path(), "0001-turn-3-openai.resp.txt").expect("error body written");
    assert!(resp.contains("required following item"), "resp: {resp}");
    let meta = read(tmp.path(), "0001-turn-3-openai.meta.json").expect("meta written");
    assert!(meta.contains("\"status\": 400"), "meta: {meta}");
    assert!(meta.contains("\"transport\": \"http\""), "meta: {meta}");
}

#[test]
fn secrets_redacted_in_body_and_headers() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::All).begin(ctx());
    rec.on_request(
        "https://api/responses",
        &auth_headers(),
        b"token: sk-ant-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    rec.on_response_chunk(b"data: {\"type\":\"response.completed\"}\n\n");
    rec.finish(WireOutcome::Success);

    let req = read(tmp.path(), "0001-turn-3-openai.req.json").expect("req written");
    assert!(!req.contains("sk-ant-bbbb"), "body secret leaked: {req}");
    let meta = read(tmp.path(), "0001-turn-3-openai.meta.json").expect("meta written");
    assert!(
        !meta.contains("sk-ant-aaaa"),
        "header secret leaked: {meta}"
    );
    assert!(
        meta.contains("authorization"),
        "headers should be captured: {meta}"
    );
}

#[test]
fn retry_captures_final_attempt_only() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = cfg(tmp.path(), WireDumpLevel::All).begin(ctx());
    rec.on_request("https://api/responses", &HashMap::new(), b"attempt-1");
    rec.on_response_chunk(b"PARTIAL-ATTEMPT-1");
    // Retry: a fresh request resets the capture.
    rec.on_request("https://api/responses", &HashMap::new(), b"attempt-2");
    rec.on_response_chunk(b"FULL-ATTEMPT-2");
    rec.finish(WireOutcome::Success);

    let resp = read(tmp.path(), "0001-turn-3-openai.resp.txt").expect("resp written");
    assert!(!resp.contains("ATTEMPT-1"), "stale attempt leaked: {resp}");
    assert!(
        resp.contains("FULL-ATTEMPT-2"),
        "final attempt missing: {resp}"
    );
    let req = read(tmp.path(), "0001-turn-3-openai.req.json").expect("req written");
    assert!(
        req.contains("attempt-2") && !req.contains("attempt-1"),
        "req: {req}"
    );
}

#[test]
fn drop_without_finish_falls_back_to_heuristic() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let rec: std::sync::Arc<SessionWireRecorder> =
            cfg(tmp.path(), WireDumpLevel::Error).begin(ctx());
        rec.on_request("https://api/responses", &HashMap::new(), b"{}");
        rec.on_response_body(500, &HashMap::new(), b"upstream exploded");
        // No finish() — e.g. a failed stream open. Drop must still flush.
    }
    let index = read(tmp.path(), "index.jsonl").expect("drop flushed");
    assert!(index.contains("\"outcome\":\"error\""), "index: {index}");
    assert!(read(tmp.path(), "0001-turn-3-openai.resp.txt").is_some());
}

#[test]
fn finish_is_idempotent_with_drop() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let rec = cfg(tmp.path(), WireDumpLevel::All).begin(ctx());
        rec.on_request("https://api/responses", &HashMap::new(), b"{}");
        rec.on_response_chunk(b"ok");
        rec.finish(WireOutcome::Success);
        // Drop here must not write a second index line.
    }
    let index = read(tmp.path(), "index.jsonl").expect("index");
    assert_eq!(index.lines().count(), 1, "exactly one index line: {index}");
}

#[test]
fn sequence_counter_increments_per_call() {
    let tmp = tempfile::tempdir().unwrap();
    let config = cfg(tmp.path(), WireDumpLevel::All);
    for _ in 0..2 {
        let rec = config.begin(ctx());
        rec.on_request("https://api/responses", &HashMap::new(), b"{}");
        rec.on_response_chunk(b"ok");
        rec.finish(WireOutcome::Success);
    }
    assert!(read(tmp.path(), "0001-turn-3-openai.req.json").is_some());
    assert!(read(tmp.path(), "0002-turn-3-openai.req.json").is_some());
    assert_eq!(read(tmp.path(), "index.jsonl").unwrap().lines().count(), 2);
}

#[test]
fn custom_sink_receives_an_already_redacted_record() {
    let sink = Arc::new(VecSink::default());
    let config =
        WireDumpConfig::with_sink(sink.clone(), WireDumpLevel::All, 1024, /*redact*/ true);
    let rec = config.begin(ctx());
    rec.on_request(
        "https://api/responses",
        &auth_headers(),
        b"token: sk-ant-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    rec.on_response_chunk(b"data: {\"type\":\"response.completed\"}\n\n");
    rec.finish(WireOutcome::Success);

    let captured = sink.0.lock().unwrap();
    assert_eq!(captured.len(), 1, "sink got exactly one record");
    let (record, persist) = &captured[0];
    assert!(*persist, "level=all persists bodies");
    assert_eq!(record.outcome, "ok");
    assert_eq!(record.transport, "stream");
    // Redaction happens BEFORE the sink — no sink can observe a secret.
    assert!(
        !record.req_body.contains("sk-ant-bbbb"),
        "body: {}",
        record.req_body
    );
    assert!(
        record.headers.values().all(|v| !v.contains("sk-ant-a")),
        "header secret reached the sink: {:?}",
        record.headers
    );
}
