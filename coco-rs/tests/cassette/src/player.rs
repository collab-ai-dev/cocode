//! `wiremock`-backed cassette replay server.
//!
//! Replay runs over a real loopback HTTP server (not an injected transport),
//! so the provider's genuine `reqwest` + SSE-decoding path executes against the
//! recorded bytes — opencode's high-fidelity "inject the fake at the socket
//! seam, run the real codec" guarantee, adapted to coco-rs's direct-`reqwest`
//! providers (which have no `Fetch` trait seam to swap).

use std::sync::Arc;
use std::sync::Mutex;

use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;

use crate::cassette::Cassette;
use crate::cassette::Interaction;
use crate::cassette::RecordedRequest;

#[derive(Debug)]
struct ReplayState {
    interactions: Vec<Interaction>,
    /// Per-interaction consumed flag (parallel to `interactions`).
    consumed: Vec<bool>,
    /// See [`Cassette::allow_any_order`].
    allow_any_order: bool,
    mismatches: Vec<String>,
    overflow: usize,
}

/// Serves recorded interactions and records any request-shape mismatch
/// (method + path + body) for [`CassettePlayer::verify`].
struct CassetteResponder {
    state: Arc<Mutex<ReplayState>>,
}

/// Canonical JSON body (object key order irrelevant; non-JSON ⇒ `null`).
fn canonical_body(request: &Request) -> serde_json::Value {
    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null)
}

/// Path + query, matching `CassetteBuilder`'s recorded `path` (host stripped).
fn incoming_path(request: &Request) -> String {
    match request.url.query() {
        Some(query) => format!("{}?{}", request.url.path(), query),
        None => request.url.path().to_string(),
    }
}

fn request_matches(recorded: &RecordedRequest, request: &Request) -> bool {
    request
        .method
        .as_str()
        .eq_ignore_ascii_case(&recorded.method)
        && incoming_path(request) == recorded.path
        && canonical_body(request) == recorded.body
}

/// First differing facet (method, then path, then body), or `None` if the
/// request fully matches the recorded shape. The body branch keeps the literal
/// phrase `request body mismatch` that tests assert on.
fn request_mismatch(recorded: &RecordedRequest, request: &Request) -> Option<String> {
    if !request
        .method
        .as_str()
        .eq_ignore_ascii_case(&recorded.method)
    {
        return Some(format!(
            "method mismatch: expected {}, actual {}",
            recorded.method,
            request.method.as_str()
        ));
    }
    let path = incoming_path(request);
    if path != recorded.path {
        return Some(format!(
            "path mismatch: expected {}, actual {path}",
            recorded.path
        ));
    }
    let body = canonical_body(request);
    if body != recorded.body {
        return Some(format!(
            "request body mismatch:\n  expected: {}\n  actual:   {body}",
            recorded.body
        ));
    }
    None
}

impl Respond for CassetteResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.state.lock().expect("replay state mutex poisoned");

        // Choose the interaction to serve. Strict (default): the next unconsumed
        // one, which must then match. Any-order: the first unconsumed one whose
        // method+path+body match this request.
        let chosen = if state.allow_any_order {
            (0..state.interactions.len()).find(|&i| {
                !state.consumed[i] && request_matches(&state.interactions[i].request, request)
            })
        } else {
            state.consumed.iter().position(|consumed| !consumed)
        };

        let Some(idx) = chosen else {
            state.overflow += 1;
            return ResponseTemplate::new(500).set_body_string(
                "cassette: no recorded interaction left to replay for this request",
            );
        };

        // Strict mode validates the full request shape against the chosen
        // interaction (any-order already matched on all three facets above).
        if !state.allow_any_order
            && let Some(detail) = request_mismatch(&state.interactions[idx].request, request)
        {
            state
                .mismatches
                .push(format!("interaction #{idx} {detail}"));
        }

        state.consumed[idx] = true;
        let interaction = state.interactions[idx].clone();
        ResponseTemplate::new(interaction.response.status).set_body_raw(
            interaction.response.body.into_bytes(),
            &interaction.response.content_type,
        )
    }
}

/// A running cassette replay server. Point a provider at [`Self::base_url`],
/// drive it, then call [`Self::verify`].
pub struct CassettePlayer {
    server: MockServer,
    state: Arc<Mutex<ReplayState>>,
}

impl CassettePlayer {
    /// Start a replay server for `cassette`.
    pub async fn start(cassette: Cassette) -> Self {
        let allow_any_order = cassette.allow_any_order;
        let interactions = cassette.interactions;
        let consumed = vec![false; interactions.len()];
        let state = Arc::new(Mutex::new(ReplayState {
            interactions,
            consumed,
            allow_any_order,
            mismatches: Vec::new(),
            overflow: 0,
        }));
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(CassetteResponder {
                state: Arc::clone(&state),
            })
            .mount(&server)
            .await;
        Self { server, state }
    }

    /// Load a cassette from disk and start a replay server for it.
    pub async fn from_path(path: impl AsRef<std::path::Path>) -> Self {
        let cassette = Cassette::load(path).expect("cassette loads");
        Self::start(cassette).await
    }

    /// Base URL of the replay server — pass as the provider's `base_url`.
    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    /// Number of interactions consumed so far.
    pub fn consumed(&self) -> usize {
        self.state
            .lock()
            .expect("replay state mutex poisoned")
            .consumed
            .iter()
            .filter(|consumed| **consumed)
            .count()
    }

    /// Assert the replay was faithful: every recorded interaction consumed in
    /// order, no request-body mismatches, no overflow. Mirrors opencode's
    /// consume-all finalizer + per-request diff.
    pub fn verify(&self) {
        let state = self.state.lock().expect("replay state mutex poisoned");
        assert!(
            state.mismatches.is_empty(),
            "cassette request mismatches:\n{}",
            state.mismatches.join("\n")
        );
        assert_eq!(
            state.overflow, 0,
            "{} request(s) arrived with no recorded interaction left to replay",
            state.overflow
        );
        let consumed = state.consumed.iter().filter(|c| **c).count();
        assert_eq!(
            consumed,
            state.interactions.len(),
            "unused recorded interactions: consumed {} of {}",
            consumed,
            state.interactions.len()
        );
    }
}

#[cfg(test)]
#[path = "player.test.rs"]
mod tests;
