//! On-disk cassette format, the recording builder, and the secret-scan-on-write
//! guard.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Current cassette schema version.
pub const CASSETTE_VERSION: u32 = 1;

/// A recorded HTTP exchange: one request paired with the response replayed for
/// it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Interaction {
    /// The request as recorded (used for replay-time matching).
    pub request: RecordedRequest,
    /// The response to replay.
    pub response: RecordedResponse,
}

/// The drift-sensitive surface of a recorded request. Host/port are
/// replay-server-specific, so only the path + body are matched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedRequest {
    /// HTTP method (e.g. `"POST"`).
    pub method: String,
    /// URL path (+ query), without scheme/host/port.
    pub path: String,
    /// Parsed JSON request body when the body was JSON; `null` otherwise.
    #[serde(default)]
    pub body: serde_json::Value,
}

/// A recorded response, replayed verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type` to replay (e.g. `"text/event-stream"`).
    pub content_type: String,
    /// Response body verbatim. For streaming responses this is the full SSE
    /// transcript (`"event: ...\ndata: ...\n\n"` repeated).
    pub body: String,
}

/// An ordered set of interactions, replayed sequentially.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cassette {
    /// Schema version; see [`CASSETTE_VERSION`].
    pub version: u32,
    /// Interactions in request order.
    pub interactions: Vec<Interaction>,
}

/// Errors from cassette I/O. Test-support boundary crate ⇒ `thiserror`.
#[derive(Debug, thiserror::Error)]
pub enum CassetteError {
    /// No cassette on disk. The replay-or-fail contract: a test should treat
    /// this as "record me" (run with the recording adapter against a live
    /// provider), never as a silent skip.
    #[error(
        "cassette not found at {path} — record it against a live provider \
         (COCO_TEST_RECORD=1) before replaying"
    )]
    Missing {
        /// Expected cassette path.
        path: PathBuf,
    },
    /// Read failure.
    #[error("failed to read cassette {path}: {source}")]
    Read {
        /// Cassette path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Parse failure.
    #[error("failed to parse cassette {path}: {source}")]
    Parse {
        /// Cassette path.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Write failure.
    #[error("failed to write cassette {path}: {source}")]
    Write {
        /// Cassette path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The serialized cassette still contained detectable credentials. The
    /// write is refused (opencode's `UnsafeCassetteError`) — recorded provider
    /// traffic is the #1 way keys leak into a repo.
    #[error(
        "refusing to write cassette {path}: it contains likely secrets ({labels}); \
         tighten redaction before recording"
    )]
    UnsafeCassette {
        /// Cassette path.
        path: PathBuf,
        /// Comma-joined human labels of the matched secret rules.
        labels: String,
    },
}

impl Cassette {
    /// A cassette at the current version from `interactions`.
    pub fn new(interactions: Vec<Interaction>) -> Self {
        Self {
            version: CASSETTE_VERSION,
            interactions,
        }
    }

    /// Load a cassette from disk. Returns [`CassetteError::Missing`] when the
    /// file does not exist so callers can distinguish "record me" from a real
    /// read error.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CassetteError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(CassetteError::Missing {
                path: path.to_path_buf(),
            });
        }
        let raw = std::fs::read_to_string(path).map_err(|source| CassetteError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&raw).map_err(|source| CassetteError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Serialize, secret-scan, and write. Refuses to write
    /// ([`CassetteError::UnsafeCassette`]) when the serialized cassette
    /// contains detectable credentials — defense in depth on top of the
    /// per-chunk redaction done while recording.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), CassetteError> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self).expect("cassette serializes");

        let hits = coco_secret_redact::scan_secrets(&json);
        if !hits.is_empty() {
            let mut labels: Vec<String> = hits.iter().map(|h| h.label()).collect();
            labels.sort();
            labels.dedup();
            return Err(CassetteError::UnsafeCassette {
                path: path.to_path_buf(),
                labels: labels.join(", "),
            });
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, json).map_err(|source| CassetteError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Accumulates observed request/response bytes into a [`Cassette`].
///
/// Fed exactly the way a `WireTap` is fed — call [`Self::on_request`], then any
/// number of [`Self::on_response_chunk`] (streaming) or one
/// [`Self::on_response_body`] (non-streaming), then [`Self::finish_stream`] /
/// rely on `on_response_body` to close the interaction. Bytes are redacted via
/// `coco-secret-redact` on arrival; [`Cassette::save`] re-scans before writing.
#[derive(Debug, Default)]
pub struct CassetteBuilder {
    interactions: Vec<Interaction>,
    pending_request: Option<RecordedRequest>,
    pending_body: String,
}

impl CassetteBuilder {
    /// New empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new interaction with the outgoing request. Resets any
    /// accumulated streaming body (a retried call captures the final attempt).
    pub fn on_request(&mut self, method: &str, url: &str, body: &[u8]) {
        let text = redact(body);
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        self.pending_request = Some(RecordedRequest {
            method: method.to_string(),
            path: url_path(url),
            body: json,
        });
        self.pending_body.clear();
    }

    /// Append one streamed response chunk.
    pub fn on_response_chunk(&mut self, chunk: &[u8]) {
        self.pending_body.push_str(&redact(chunk));
    }

    /// Close the current (streaming) interaction with its status + content type.
    /// The accumulated chunks become the response body.
    pub fn finish_stream(&mut self, status: u16, content_type: &str) {
        if let Some(request) = self.pending_request.take() {
            let body = std::mem::take(&mut self.pending_body);
            self.interactions.push(Interaction {
                request,
                response: RecordedResponse {
                    status,
                    content_type: content_type.to_string(),
                    body,
                },
            });
        }
    }

    /// Close the current (non-streaming) interaction with a full body.
    pub fn on_response_body(&mut self, status: u16, content_type: &str, body: &[u8]) {
        if let Some(request) = self.pending_request.take() {
            self.pending_body.clear();
            self.interactions.push(Interaction {
                request,
                response: RecordedResponse {
                    status,
                    content_type: content_type.to_string(),
                    body: redact(body),
                },
            });
        }
    }

    /// Consume the builder into a [`Cassette`].
    pub fn build(self) -> Cassette {
        Cassette::new(self.interactions)
    }
}

/// Redact secrets from raw bytes, returning lossy UTF-8.
fn redact(bytes: &[u8]) -> String {
    coco_secret_redact::redact_secrets(&String::from_utf8_lossy(bytes)).into_owned()
}

/// Strip `scheme://host:port` and keep the path (+query). Delimiters are ASCII,
/// so the byte slices land on char boundaries.
fn url_path(url: &str) -> String {
    if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        return match rest.find('/') {
            Some(slash) => rest[slash..].to_string(),
            None => "/".to_string(),
        };
    }
    url.to_string()
}

#[cfg(test)]
#[path = "cassette.test.rs"]
mod tests;
