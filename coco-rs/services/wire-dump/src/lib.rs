//! Per-session raw LLM wire-traffic recorder for debugging.
//!
//! Split into two concerns:
//! - **capture + redact + classify** ([`SessionWireRecorder`]) — buffers
//!   the request/response, redacts secrets, decides success/failure, and
//!   produces a fully-redacted [`WireRecord`].
//! - **output** ([`WireSink`]) — where the record goes. The default
//!   [`FileSink`] writes the per-session `wire/` layout; tests / future
//!   remote sinks inject their own.
//!
//! The recorder is **provider-agnostic**: it knows nothing about the LLM
//! transport or the `WireTap` trait. The consumer feeds it via the
//! inherent [`SessionWireRecorder::on_request`] /
//! [`SessionWireRecorder::on_response_chunk`] /
//! [`SessionWireRecorder::on_response_body`] methods (in coco-rs, a thin
//! `WireTap` adapter in `app/query` does this), and signals completion
//! with [`SessionWireRecorder::finish`]. This keeps the crate free of any
//! `coco-inference` / `vercel-ai` dependency.
//!
//! ## Lifecycle
//! - `on_request` resets the response buffers, so a retried call captures
//!   the **final** attempt, not a concatenation.
//! - `finish(outcome)` is authoritative for the persist decision and
//!   writes on `tokio::task::spawn_blocking` (off the async runtime).
//! - `Drop` is a safety net for paths that never call `finish`
//!   (cancellation, a failed stream *open*), falling back to a byte
//!   heuristic and a synchronous write.

mod sink;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use coco_config::WireDumpLevel;

pub use sink::FileSink;
pub use sink::WireRecord;
pub use sink::WireSink;

/// Session-scoped configuration + shared sequence counter + output sink.
/// Cloned cheaply; one instance lives on `QueryEngineConfig`.
#[derive(Clone, Debug)]
pub struct WireDumpConfig {
    level: WireDumpLevel,
    max_body_bytes: usize,
    redact: bool,
    seq: Arc<AtomicU64>,
    sink: Arc<dyn WireSink>,
    session_dir: Option<PathBuf>,
}

impl WireDumpConfig {
    /// Default config: writes to `<session_dir>/wire/` via [`FileSink`].
    /// Callers gate construction on `!level.is_off()`.
    pub fn new(
        session_dir: PathBuf,
        level: WireDumpLevel,
        max_body_bytes: usize,
        redact: bool,
    ) -> Self {
        Self::with_sink(
            Arc::new(FileSink::new(session_dir.join("wire"))),
            level,
            max_body_bytes,
            redact,
        )
        .with_session_dir(session_dir)
    }

    /// Config with a caller-supplied sink (tests, remote/stdout sinks).
    pub fn with_sink(
        sink: Arc<dyn WireSink>,
        level: WireDumpLevel,
        max_body_bytes: usize,
        redact: bool,
    ) -> Self {
        Self {
            level,
            max_body_bytes,
            redact,
            seq: Arc::new(AtomicU64::new(0)),
            sink,
            session_dir: None,
        }
    }

    fn with_session_dir(mut self, session_dir: PathBuf) -> Self {
        self.session_dir = Some(session_dir);
        self
    }

    /// Build a child config that writes under
    /// `<session_dir>/wire/subagents/agent-<agent_id>/`.
    ///
    /// Returns `None` for custom sinks, where the filesystem layout is
    /// caller-defined.
    pub fn for_subagent(&self, agent_id: &str) -> Option<Self> {
        let session_dir = self.session_dir.as_ref()?;
        Some(Self {
            level: self.level,
            max_body_bytes: self.max_body_bytes,
            redact: self.redact,
            seq: Arc::new(AtomicU64::new(0)),
            sink: Arc::new(FileSink::new(
                session_dir
                    .join("wire")
                    .join("subagents")
                    .join(format!("agent-{agent_id}")),
            )),
            session_dir: Some(session_dir.clone()),
        })
    }

    /// Begin capturing one LLM call. The consumer feeds the returned
    /// recorder and drives it to completion via `finish`; `Drop` is a
    /// fallback.
    pub fn begin(&self, ctx: WireTurnCtx<'_>) -> Arc<SessionWireRecorder> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        Arc::new(SessionWireRecorder {
            level: self.level,
            max_body_bytes: self.max_body_bytes,
            redact: self.redact,
            seq,
            turn_id: ctx.turn_id.to_string(),
            provider: ctx.provider.to_string(),
            model: ctx.model.to_string(),
            sink: self.sink.clone(),
            state: Mutex::new(RecorderState::default()),
            flushed: AtomicBool::new(false),
        })
    }
}

/// Identifying context for one captured call.
pub struct WireTurnCtx<'a> {
    pub turn_id: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
}

/// Typed completion signal from the consumer, which already knows whether
/// the call succeeded — no byte-level guessing on the `finish` path.
#[derive(Clone, Copy, Debug)]
pub enum WireOutcome {
    Success,
    Failure,
}

impl WireOutcome {
    /// Map a stream's normal/abnormal finish to success/failure.
    pub fn from_is_normal(is_normal: bool) -> Self {
        if is_normal {
            Self::Success
        } else {
            Self::Failure
        }
    }
}

#[derive(Default, Debug)]
struct RecorderState {
    saw_request: bool,
    req_url: String,
    req_headers: BTreeMap<String, String>,
    req_body: Vec<u8>,
    req_truncated: bool,
    resp: Vec<u8>,
    resp_truncated: bool,
    resp_status: Option<u16>,
    /// True once any streaming chunk arrived (distinguishes a streamed
    /// response from a one-shot / error body).
    saw_chunks: bool,
}

/// Resolved 3-state outcome used for the persist decision + the record
/// label. `Unknown` only arises on the `Drop` fallback path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    Error,
    Unknown,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }

    fn is_failure(self) -> bool {
        !matches!(self, Self::Ok)
    }
}

/// Captures one request/response and emits a redacted [`WireRecord`] to
/// the sink on completion. Fed via inherent methods (no trait impl).
#[derive(Debug)]
pub struct SessionWireRecorder {
    level: WireDumpLevel,
    max_body_bytes: usize,
    redact: bool,
    seq: u64,
    turn_id: String,
    provider: String,
    model: String,
    sink: Arc<dyn WireSink>,
    state: Mutex<RecorderState>,
    flushed: AtomicBool,
}

impl SessionWireRecorder {
    fn lock(&self) -> std::sync::MutexGuard<'_, RecorderState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Append `src` to `dst`, capped at `max_body_bytes`; sets `truncated`.
    fn append_capped(dst: &mut Vec<u8>, truncated: &mut bool, cap: usize, src: &[u8]) {
        let remaining = cap.saturating_sub(dst.len());
        if remaining == 0 {
            *truncated = true;
            return;
        }
        let take = src.len().min(remaining);
        dst.extend_from_slice(&src[..take]);
        if take < src.len() {
            *truncated = true;
        }
    }

    /// Record the outgoing request. Resets response state so a retried
    /// call captures the final attempt.
    pub fn on_request(&self, url: &str, headers: &HashMap<String, String>, body: &[u8]) {
        let cap = self.max_body_bytes;
        let mut state = self.lock();
        *state = RecorderState {
            saw_request: true,
            req_url: url.to_string(),
            req_headers: headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..RecorderState::default()
        };
        let mut truncated = false;
        Self::append_capped(&mut state.req_body, &mut truncated, cap, body);
        state.req_truncated = truncated;
    }

    /// Record a streaming response chunk.
    pub fn on_response_chunk(&self, chunk: &[u8]) {
        let cap = self.max_body_bytes;
        let mut state = self.lock();
        state.saw_chunks = true;
        let mut truncated = state.resp_truncated;
        Self::append_capped(&mut state.resp, &mut truncated, cap, chunk);
        state.resp_truncated = truncated;
    }

    /// Record a full (non-streaming) or HTTP-error response body + status.
    pub fn on_response_body(&self, status: u16, _headers: &HashMap<String, String>, body: &[u8]) {
        let cap = self.max_body_bytes;
        let mut state = self.lock();
        state.resp_status = Some(status);
        let mut truncated = state.resp_truncated;
        Self::append_capped(&mut state.resp, &mut truncated, cap, body);
        state.resp_truncated = truncated;
    }

    /// Consumer-driven completion with the authoritative outcome. Persists
    /// on a blocking thread so the async runtime isn't stalled by I/O +
    /// redaction. Idempotent with `Drop`.
    pub fn finish(&self, outcome: WireOutcome) {
        if self.flushed.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some((record, persist)) = self.build_record(Some(outcome)) else {
            return;
        };
        let sink = self.sink.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(move || sink.emit(&record, persist));
        } else {
            sink.emit(&record, persist);
        }
    }

    fn redact_bytes(&self, bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        if self.redact {
            coco_secret_redact::redact_secrets(&text).into_owned()
        } else {
            text.into_owned()
        }
    }

    fn redact_str(&self, s: &str) -> String {
        if self.redact {
            coco_secret_redact::redact_secrets(s).into_owned()
        } else {
            s.to_string()
        }
    }

    /// Build the redacted record + persist decision. `None` ⇒ nothing to
    /// emit (no request captured, or capture disabled).
    fn build_record(&self, explicit: Option<WireOutcome>) -> Option<(WireRecord, bool)> {
        let mut st = self.lock();
        if !st.saw_request {
            return None;
        }
        let outcome = resolve_outcome(&st, explicit);
        let persist_bodies = match self.level {
            WireDumpLevel::Off => return None,
            WireDumpLevel::All => true,
            WireDumpLevel::Error => outcome.is_failure(),
        };
        let transport = if st.saw_chunks {
            "stream"
        } else if st.resp_status.is_some() {
            "http"
        } else {
            "request_only"
        };
        let headers = std::mem::take(&mut st.req_headers)
            .into_iter()
            .map(|(k, v)| (k, self.redact_str(&v)))
            .collect();
        let record = WireRecord {
            seq: self.seq,
            turn_id: self.turn_id.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            transport,
            url: std::mem::take(&mut st.req_url),
            status: st.resp_status,
            outcome: outcome.as_str(),
            headers,
            req_body: self.redact_bytes(&std::mem::take(&mut st.req_body)),
            resp_body: self.redact_bytes(&std::mem::take(&mut st.resp)),
            req_truncated: st.req_truncated,
            resp_truncated: st.resp_truncated,
        };
        Some((record, persist_bodies))
    }
}

impl Drop for SessionWireRecorder {
    fn drop(&mut self) {
        // Safety net for paths that never reach `finish` (cancellation, a
        // failed stream open). Synchronous; guarded so it never
        // double-writes after `finish`.
        if self.flushed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some((record, persist)) = self.build_record(None) {
            self.sink.emit(&record, persist);
        }
    }
}

/// Decide the outcome. The consumer-supplied outcome is authoritative;
/// the byte heuristic is only the `Drop`-path fallback.
fn resolve_outcome(state: &RecorderState, explicit: Option<WireOutcome>) -> Outcome {
    if let Some(outcome) = explicit {
        return match outcome {
            WireOutcome::Success => Outcome::Ok,
            WireOutcome::Failure => Outcome::Error,
        };
    }
    if let Some(status) = state.resp_status
        && !(200..300).contains(&status)
    {
        return Outcome::Error;
    }
    if state.resp.is_empty() {
        return Outcome::Unknown;
    }
    if response_has_error_marker(&state.resp) {
        return Outcome::Error;
    }
    Outcome::Ok
}

/// Provider-agnostic detection of an in-band error event in a response
/// body. Covers the OpenAI Responses `{"type":"error"}` event (the
/// 200-then-error shape) and the Anthropic SSE `event: error` line.
fn response_has_error_marker(resp: &[u8]) -> bool {
    let text = String::from_utf8_lossy(resp);
    text.contains("\"type\":\"error\"")
        || text.contains("\"type\": \"error\"")
        || text.contains("event: error")
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
