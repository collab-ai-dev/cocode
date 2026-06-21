//! Output seam for the wire dumper.
//!
//! `SessionWireRecorder` captures + redacts + classifies, producing a
//! fully-redacted [`WireRecord`], then hands it to a [`WireSink`]. The
//! batteries-included [`FileSink`] writes the per-session `wire/` layout;
//! tests (or a future remote / stdout sink) swap in their own `WireSink`.
//!
//! Redaction happens **before** the sink sees the record, so no sink —
//! default or custom — can ever observe a secret.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde_json::Value;
use serde_json::json;

/// One captured request/response, **already redacted** and ready to
/// persist. Pure data — no I/O, no secrets.
#[derive(Clone, Debug)]
pub struct WireRecord {
    pub seq: u64,
    pub turn_id: String,
    pub provider: String,
    pub model: String,
    /// `"stream"` / `"http"` / `"request_only"`.
    pub transport: &'static str,
    pub url: String,
    pub status: Option<u16>,
    /// `"ok"` / `"error"` / `"unknown"`.
    pub outcome: &'static str,
    pub headers: BTreeMap<String, String>,
    pub req_body: String,
    pub resp_body: String,
    pub req_truncated: bool,
    pub resp_truncated: bool,
}

/// Destination for captured records. Called once per completed call.
/// `persist_bodies` is the recorder's level+outcome decision: `false` ⇒
/// record the call in the index only; `true` ⇒ also persist the bodies.
pub trait WireSink: Send + Sync + std::fmt::Debug {
    fn emit(&self, record: &WireRecord, persist_bodies: bool);
}

/// Default sink: writes `<dir>/index.jsonl` (always) plus a
/// `<seq>-<turn>-<provider>.{req.json,resp.txt,meta.json}` triplet when
/// `persist_bodies`. Logs (never panics) on any I/O failure — a debug
/// tool that can't write its dump must say so.
#[derive(Debug)]
pub struct FileSink {
    dir: PathBuf,
}

impl FileSink {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.dir.join(name);
        if let Err(e) = std::fs::write(&path, bytes) {
            tracing::warn!(path = %path.display(), error = %e, "wire-dump: write failed");
        }
    }

    fn append_line(&self, path: &Path, line: &str) {
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(line.as_bytes()) {
                    tracing::warn!(path = %path.display(), error = %e, "wire-dump: index append failed");
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "wire-dump: index open failed");
            }
        }
    }
}

impl WireSink for FileSink {
    fn emit(&self, record: &WireRecord, persist_bodies: bool) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            tracing::warn!(dir = %self.dir.display(), error = %e, "wire-dump: cannot create dir");
            return;
        }
        let base = format!(
            "{seq:04}-{turn}-{provider}",
            seq = record.seq,
            turn = sanitize(&record.turn_id),
            provider = sanitize(&record.provider),
        );
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        if persist_bodies {
            self.write(&format!("{base}.req.json"), record.req_body.as_bytes());
            self.write(&format!("{base}.resp.txt"), record.resp_body.as_bytes());
            let headers: serde_json::Map<String, Value> = record
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            let meta = json!({
                "seq": record.seq,
                "ts_ms": ts_ms,
                "turn_id": record.turn_id,
                "provider": record.provider,
                "model": record.model,
                "transport": record.transport,
                "url": record.url,
                "status": record.status,
                "outcome": record.outcome,
                "headers": headers,
                "req_bytes": record.req_body.len(),
                "req_truncated": record.req_truncated,
                "resp_bytes": record.resp_body.len(),
                "resp_truncated": record.resp_truncated,
            });
            if let Ok(s) = serde_json::to_string_pretty(&meta) {
                self.write(&format!("{base}.meta.json"), s.as_bytes());
            }
        }

        let index = json!({
            "seq": record.seq,
            "ts_ms": ts_ms,
            "turn_id": record.turn_id,
            "provider": record.provider,
            "model": record.model,
            "transport": record.transport,
            "status": record.status,
            "outcome": record.outcome,
            "resp_bytes": record.resp_body.len(),
            "bodies": persist_bodies,
        });
        if let Ok(mut line) = serde_json::to_string(&index) {
            line.push('\n');
            self.append_line(&self.dir.join("index.jsonl"), &line);
        }
    }
}

/// Filesystem-safe slug for a filename component.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
