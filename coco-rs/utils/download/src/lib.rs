//! Streaming file downloader: fetch a URL to disk with progress reporting,
//! SHA-256 verification, and atomic replace.
//!
//! Tier-2 leaf utility (`thiserror`, no `coco-error`). The repo had no reusable
//! download helper before this — every prior HTTP fetch buffered the whole body
//! in memory with no checksum and no atomic install. This crate closes those
//! gaps for large-artifact downloads (e.g. Whisper weights):
//!
//! - **Streamed to disk**, never buffered whole — safe for multi-GB files.
//! - **Atomic install**: bytes land in a sibling `*.part` file, `fsync`ed and
//!   renamed onto `dest` only after the checksum passes. An interrupted or
//!   corrupt download never leaves a half-written file at the real path (the
//!   failure mode that silently poisons a naive downloader's cache).
//! - **Verified**: optional pinned SHA-256 (rejects a tampered/truncated file)
//!   and an optional `Content-Length` pre-check.
//! - **Cancellable**: a `CancellationToken` aborts mid-stream and cleans up; a
//!   per-read timeout also breaks a stalled transfer whose caller never fires it.
//! - **Redirect-following**: HuggingFace `resolve/` → CDN redirects work out of
//!   the box. With `restrict_to_public`, redirects are bounded and each hop's
//!   host is re-checked against the SSRF guard.

use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;

/// Emit a progress update at most every this many bytes (progress is lossy —
/// updates use `try_send` and are dropped if the consumer is behind).
const PROGRESS_STRIDE: u64 = 512 * 1024;

/// Connection (not total-transfer) timeout. A total timeout would abort a
/// legitimately slow large download; cancellation is the escape hatch instead.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-read timeout: if no bytes arrive for this long, the transfer errors out
/// instead of hanging forever. A legitimate transfer streams continuously, so
/// this only trips on a genuine stall — the safety net for an unattended
/// download whose caller never fires the cancel token.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Redirect hops allowed when `restrict_to_public` is set.
const MAX_REDIRECTS: usize = 5;

/// What to fetch and how to verify it.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// Source URL. Redirects are followed.
    pub url: String,
    /// Final on-disk destination. Written atomically via a sibling `*.part`.
    pub dest: PathBuf,
    /// Lowercase-hex SHA-256 the finished file must match. `None` skips
    /// verification (e.g. a user-supplied URL with no pinned digest).
    pub expected_sha256: Option<String>,
    /// Expected size in bytes. Cross-checked against `Content-Length` before the
    /// transfer AND enforced as a hard cap during streaming (so a mirror that
    /// omits/lies about `Content-Length` still can't stream past it). `None`
    /// skips both.
    pub expected_size: Option<u64>,
    /// `User-Agent` header to send.
    pub user_agent: String,
    /// SSRF guard: when `true`, reject non-http(s) schemes and any URL — initial
    /// or redirect hop — whose host is a private/loopback/link-local IP literal,
    /// and cap redirects. Set for downloads whose URL can come from untrusted
    /// (e.g. project) config. (Residual: a hostname resolving to a private IP is
    /// not caught — that needs a connect-time resolver.)
    pub restrict_to_public: bool,
}

/// Progress updates emitted during a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadProgress {
    /// Transfer began; `total` is the `Content-Length` when the server sent one.
    Started { total: Option<u64> },
    /// Cumulative bytes written so far (throttled to ~`PROGRESS_STRIDE`).
    Progress { received: u64, total: Option<u64> },
    /// All bytes written; verifying the checksum.
    Verifying,
    /// Finished and installed at the destination; `total` bytes written.
    Done { total: u64 },
}

/// Failure modes across request → stream → verify → install.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// The download was cancelled via the `CancellationToken`.
    #[error("download cancelled")]
    Cancelled,

    /// The HTTP request itself failed (connect, TLS, read).
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The server returned a non-success status.
    #[error("server returned status {status} for {url}")]
    Status { url: String, status: u16 },

    /// The URL (or a redirect hop) targets a blocked host under the SSRF guard.
    #[error("blocked host `{host}` (private/loopback/link-local or non-http scheme)")]
    BlockedHost { host: String },

    /// The transfer streamed more bytes than the pinned `expected_size`.
    #[error("size overflow: exceeded expected {expected} bytes")]
    SizeOverflow { expected: u64 },

    /// `Content-Length` disagreed with the expected size before transfer.
    #[error("size mismatch: expected {expected} bytes, server reported {actual}")]
    SizeMismatch { expected: u64, actual: u64 },

    /// The finished file's SHA-256 did not match the pinned digest.
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    /// A filesystem operation failed.
    #[error("filesystem error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Download `req.url` to `req.dest`, streaming to a sibling `*.part` file and
/// atomically renaming it into place only after optional size + checksum
/// verification pass. `progress` receives lossy updates; `cancel` aborts
/// mid-stream (leaving no partial file at `dest`).
pub async fn download_file(
    req: DownloadRequest,
    progress: Option<mpsc::Sender<DownloadProgress>>,
    cancel: CancellationToken,
) -> Result<(), DownloadError> {
    let temp = temp_path(&req.dest);
    let result = stream_to_temp(&req, &temp, progress.as_ref(), &cancel).await;
    if result.is_err() {
        // Best-effort cleanup: never leave a partial `*.part` behind.
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}

async fn stream_to_temp(
    req: &DownloadRequest,
    temp: &Path,
    progress: Option<&mpsc::Sender<DownloadProgress>>,
    cancel: &CancellationToken,
) -> Result<(), DownloadError> {
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    if req.restrict_to_public {
        // Pre-flight guard for the initial URL; redirect hops are checked by the
        // policy on the client below.
        let parsed = reqwest::Url::parse(&req.url).map_err(|_| DownloadError::BlockedHost {
            host: req.url.clone(),
        })?;
        if let Some(host) = blocked_host(&parsed) {
            return Err(DownloadError::BlockedHost { host });
        }
    }

    let mut builder = reqwest::Client::builder()
        .user_agent(&req.user_agent)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT);
    if req.restrict_to_public {
        builder = builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.stop()
            } else if blocked_host(attempt.url()).is_some() {
                attempt.error(BlockedRedirect)
            } else {
                attempt.follow()
            }
        }));
    }
    let client = builder.build()?;
    let response = client.get(&req.url).send().await?;
    if !response.status().is_success() {
        return Err(DownloadError::Status {
            url: req.url.clone(),
            status: response.status().as_u16(),
        });
    }

    let total = response.content_length();
    if let (Some(expected), Some(actual)) = (req.expected_size, total)
        && expected != actual
    {
        return Err(DownloadError::SizeMismatch { expected, actual });
    }
    emit(progress, DownloadProgress::Started { total });

    if let Some(parent) = temp.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| DownloadError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|source| DownloadError::Io {
            path: temp.to_path_buf(),
            source,
        })?;

    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut last_emitted: u64 = 0;
    let mut stream = response.bytes_stream();

    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            next = stream.next() => match next {
                Some(chunk) => chunk?,
                None => break,
            },
        };
        received += chunk.len() as u64;
        // Hard cap: a mirror that omits or lies about Content-Length still can't
        // stream past the pinned size (disk-fill / oversized-payload guard).
        if let Some(max) = req.expected_size
            && received > max
        {
            return Err(DownloadError::SizeOverflow { expected: max });
        }
        hasher.update(chunk.as_ref());
        file.write_all(chunk.as_ref())
            .await
            .map_err(|source| DownloadError::Io {
                path: temp.to_path_buf(),
                source,
            })?;
        if received - last_emitted >= PROGRESS_STRIDE {
            last_emitted = received;
            emit(progress, DownloadProgress::Progress { received, total });
        }
    }

    file.flush().await.map_err(|source| DownloadError::Io {
        path: temp.to_path_buf(),
        source,
    })?;
    file.sync_all().await.map_err(|source| DownloadError::Io {
        path: temp.to_path_buf(),
        source,
    })?;
    drop(file);

    emit(progress, DownloadProgress::Verifying);
    if let Some(expected) = &req.expected_sha256 {
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(DownloadError::ChecksumMismatch {
                expected: expected.clone(),
                actual,
            });
        }
    }

    tokio::fs::rename(temp, &req.dest)
        .await
        .map_err(|source| DownloadError::Io {
            path: req.dest.clone(),
            source,
        })?;
    emit(progress, DownloadProgress::Done { total: received });
    Ok(())
}

/// `dest` + `.part` — a sibling that shares `dest`'s directory (so the final
/// `rename` is same-filesystem and atomic), distinct from any real extension.
fn temp_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Lossy progress emit: never blocks the transfer, drops the update if the
/// receiver is full or gone.
fn emit(progress: Option<&mpsc::Sender<DownloadProgress>>, event: DownloadProgress) {
    if let Some(tx) = progress {
        let _ = tx.try_send(event);
    }
}

/// Marker error returned from the redirect policy when a hop targets a blocked
/// host. Surfaces as a `reqwest::Error` → `DownloadError::Http`.
#[derive(Debug)]
struct BlockedRedirect;

impl std::fmt::Display for BlockedRedirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "redirect to a blocked (private/loopback/link-local) host"
        )
    }
}

impl std::error::Error for BlockedRedirect {}

/// If `url`'s scheme isn't http(s) or its host is a blocked IP literal, return
/// the offending host/scheme; else `None`. A DNS hostname (non-literal) is not
/// resolved here, so it passes — the documented residual.
fn blocked_host(url: &reqwest::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Some(format!("{}:", url.scheme()));
    }
    let host = url.host_str()?;
    let trimmed = host.trim_start_matches('[').trim_end_matches(']');
    match trimmed.parse::<IpAddr>() {
        Ok(ip) if ip_is_blocked(ip) => Some(host.to_string()),
        _ => None,
    }
}

/// Loopback / private / link-local / unspecified / broadcast ranges (the SSRF
/// targets — cloud metadata `169.254.169.254`, `10./172.16./192.168.`, `127.`).
fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|m| ip_is_blocked(IpAddr::V4(m)))
        }
    }
}
