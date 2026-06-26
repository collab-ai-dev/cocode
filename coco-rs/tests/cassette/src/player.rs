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

#[derive(Debug)]
struct ReplayState {
    interactions: Vec<Interaction>,
    cursor: usize,
    mismatches: Vec<String>,
    overflow: usize,
}

/// Serves recorded interactions sequentially and records any request-body
/// mismatch for [`CassettePlayer::verify`].
struct CassetteResponder {
    state: Arc<Mutex<ReplayState>>,
}

impl Respond for CassetteResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.state.lock().expect("replay state mutex poisoned");
        let cursor = state.cursor;

        let Some(interaction) = state.interactions.get(cursor).cloned() else {
            state.overflow += 1;
            return ResponseTemplate::new(500).set_body_string(format!(
                "cassette overflow: request arrived with no recorded interaction #{cursor}"
            ));
        };

        // Canonicalized JSON comparison (object key order is irrelevant). A
        // non-JSON body compares as `null` on both sides only if neither parsed.
        let incoming: serde_json::Value =
            serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
        if incoming != interaction.request.body {
            state.mismatches.push(format!(
                "interaction #{cursor} request body mismatch:\n  expected: {}\n  actual:   {}",
                interaction.request.body, incoming
            ));
        }

        state.cursor += 1;
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
        let state = Arc::new(Mutex::new(ReplayState {
            interactions: cassette.interactions,
            cursor: 0,
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
            .cursor
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
        assert_eq!(
            state.cursor,
            state.interactions.len(),
            "unused recorded interactions: consumed {} of {}",
            state.cursor,
            state.interactions.len()
        );
    }
}

#[cfg(test)]
#[path = "player.test.rs"]
mod tests;
