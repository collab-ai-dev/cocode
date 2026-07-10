//! Tool-result write mechanics: the session-scoped [`ToolOutputStore`],
//! artifact naming ([`ArtifactKey`]), per-tool bound declarations
//! ([`ResultSizeBound`]), and binary MCP persistence.
//!
//! Policy (window computation, inline budgets, the Level-2 per-message
//! aggregate budget) lives one module up in
//! [`crate::tool_result_offload`], which builds on this module — the
//! dependency is strictly offload → storage, never back.
//!
//! The `<persisted-output>` markers and their predicates are canonical in
//! [`coco_types::persisted_output`] (re-exported here) so that other crates
//! (e.g. `coco-compact`) share one vocabulary.

use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

pub use coco_types::persisted_output::PERSISTED_OUTPUT_CLOSING_TAG;
pub use coco_types::persisted_output::PERSISTED_OUTPUT_TAG;
pub use coco_types::persisted_output::is_content_already_persisted;

/// Default per-tool persistence threshold, and the value of the trait-default
/// bound declaration. Declarations are AUTHORITATIVE — there is no hidden
/// global clamp: a tool that declares `Bytes(102_000)` gets a 102_000-byte
/// threshold (WebFetch does, to let preapproved docs pages pass verbatim).
pub const DEFAULT_MAX_RESULT_SIZE_BYTES: i64 = 50_000;

/// Default [`crate::Tool::max_result_size_bound`] declaration for tools that
/// do not opt out or override.
pub const DEFAULT_TOOL_MAX_RESULT_SIZE_BOUND: ResultSizeBound =
    ResultSizeBound::Bytes(DEFAULT_MAX_RESULT_SIZE_BYTES);

/// Subdirectory name for tool results within a session.
pub const TOOL_RESULTS_SUBDIR: &str = "tool-results";

/// Per-tool persistence cap declaration.
///
/// The `Bytes` variant always carries a positive UTF-8 byte cap; `Unbounded`
/// makes the tool's opt-out explicit so callers (Level 1 persist + Level 2
/// aggregate budget) match on it instead of comparing to a magic number.
/// Declared values are authoritative — no post-hoc clamping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultSizeBound {
    /// Cap inline result at this many UTF-8 bytes. Must be positive;
    /// callers that need fallible construction use [`Self::try_bytes`].
    Bytes(i64),
    /// Tool opts out of Level 1 persistence — its content is canonical
    /// (e.g. `Read` on a tracked file the model will read again). Inline
    /// regardless of length.
    Unbounded,
}

impl ResultSizeBound {
    /// Const constructor. Panics in `const` evaluation if `n <= 0`.
    pub const fn bytes(n: i64) -> Self {
        assert!(n > 0, "ResultSizeBound::bytes requires a positive cap");
        Self::Bytes(n)
    }

    /// Fallible constructor.
    pub const fn try_bytes(n: i64) -> Option<Self> {
        if n > 0 { Some(Self::Bytes(n)) } else { None }
    }

    pub const fn is_unbounded(self) -> bool {
        matches!(self, Self::Unbounded)
    }

    /// Cap in bytes, or `None` for `Unbounded`.
    pub const fn as_bytes(self) -> Option<i64> {
        match self {
            Self::Bytes(n) => Some(n),
            Self::Unbounded => None,
        }
    }
}

/// Artifact naming policy. The runtime owns NO URL/domain semantics — callers
/// compute names; the store validates and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKey {
    /// Regular tool result: named by `tool_use_id` (globally unique — keeps the
    /// resume semantics of the `create_new` path). `is_json` selects extension.
    ToolUse { id: String, is_json: bool },
    /// Caller-computed file name for shareable/content-addressed artifacts.
    /// Validated by the store: `[A-Za-z0-9._-]+`, `<= 100` bytes, must not
    /// start with `.`. Callers MUST prefix a fixed literal (WebFetch uses
    /// `url-`) so reserved device-name stems (`con.`, `nul.`, `com1.`) can
    /// never be produced. Written via ATOMIC PUBLISH (tmp + rename).
    Named { file_name: String },
}

/// Maximum bytes allowed in a [`ArtifactKey::Named`] file name.
const NAMED_KEY_MAX_BYTES: usize = 100;

impl ArtifactKey {
    /// Extension-terminated file name for this key.
    pub(crate) fn file_name(&self) -> String {
        match self {
            Self::ToolUse { id, is_json } => {
                let ext = if *is_json { "json" } else { "txt" };
                format!("{id}.{ext}")
            }
            Self::Named { file_name } => file_name.clone(),
        }
    }

    /// Named keys are published atomically (tmp + rename) so a concurrent
    /// reader never observes a partial file; `ToolUse` keys use `create_new`.
    pub(crate) fn is_atomic_publish(&self) -> bool {
        matches!(self, Self::Named { .. })
    }

    /// Validate a caller-computed name. `ToolUse` keys are always valid
    /// (`tool_use_id` is a runtime-generated token).
    pub(crate) fn validate(&self) -> Result<(), ArtifactKeyError> {
        let Self::Named { file_name } = self else {
            return Ok(());
        };
        if file_name.is_empty() || file_name.len() > NAMED_KEY_MAX_BYTES {
            return Err(ArtifactKeyError::Length);
        }
        if file_name.starts_with('.') {
            return Err(ArtifactKeyError::LeadingDot);
        }
        if !file_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(ArtifactKeyError::Charset);
        }
        Ok(())
    }
}

/// Why a [`ArtifactKey::Named`] file name was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKeyError {
    Length,
    LeadingDot,
    Charset,
}

impl std::fmt::Display for ArtifactKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::Length => "artifact name must be 1..=100 bytes",
            Self::LeadingDot => "artifact name must not start with '.'",
            Self::Charset => "artifact name must match [A-Za-z0-9._-]",
        };
        f.write_str(msg)
    }
}

/// Per-session tool-result directory.
pub fn tool_results_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(TOOL_RESULTS_SUBDIR)
}

/// Outcome of persisting a binary MCP output to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedMcpBinaryOutput {
    pub filepath: PathBuf,
    pub original_size: i64,
    pub mime_type: String,
}

pub fn render_mcp_binary_reference(persisted: &PersistedMcpBinaryOutput) -> String {
    let mut buf = String::with_capacity(256);
    buf.push_str(PERSISTED_OUTPUT_TAG);
    buf.push('\n');
    buf.push_str(&format!(
        "MCP output is binary ({}; {}). Full output saved to: {}\n",
        format_byte_size(persisted.original_size as usize),
        persisted.mime_type,
        persisted.filepath.display()
    ));
    buf.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    buf
}

pub fn empty_tool_result_message(tool_name: &str) -> String {
    format!("({tool_name} completed with no output)")
}

/// Session-scoped facade for model-facing tool output persistence.
///
/// Owns the write mechanics so the storage policy stays at the tool-runtime
/// boundary. Higher layers pass this type instead of raw session paths.
#[derive(Debug, Clone)]
pub struct ToolOutputStore {
    session_dir: PathBuf,
}

impl ToolOutputStore {
    pub fn new(session_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_dir: session_dir.into(),
        }
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Write an artifact to disk and return its path.
    ///
    /// - [`ArtifactKey::ToolUse`] → `create_new`; an existing file (same
    ///   globally-unique id ⟹ same bytes) is kept.
    /// - [`ArtifactKey::Named`] → **atomic publish**: write `.tmp-<uuid>` then
    ///   rename over the target, so a concurrent reader never sees a partial
    ///   file and last-writer-wins is safe for content-addressed names.
    ///
    /// The footer is always computed from the in-memory string that was
    /// written — never by re-reading disk.
    pub async fn write_artifact(
        &self,
        key: &ArtifactKey,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        key.validate()
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, e.to_string()))?;
        let dir = tool_results_dir(&self.session_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let filepath = dir.join(key.file_name());

        use tokio::io::AsyncWriteExt;
        if key.is_atomic_publish() {
            let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
            {
                let mut file = tokio::fs::File::create(&tmp).await?;
                file.write_all(content.as_bytes()).await?;
                file.flush().await?;
            }
            if let Err(e) = tokio::fs::rename(&tmp, &filepath).await {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(e);
            }
        } else {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&filepath)
                .await
            {
                Ok(mut file) => {
                    file.write_all(content.as_bytes()).await?;
                    file.flush().await?;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Ok(filepath)
    }

    /// Persist a binary payload (MCP output, WebFetch binary body) under this
    /// session's tool-output store, deriving the extension from the MIME type.
    /// Idempotent per id: an existing file wins.
    pub async fn persist_binary(
        &self,
        id: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
    ) -> std::io::Result<PersistedMcpBinaryOutput> {
        let dir = tool_results_dir(&self.session_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let filepath = dir.join(format!("{}.{}", id, extension_for_mime_type(mime_type)));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filepath)
            .await
        {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(bytes).await?;
                file.flush().await?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }

        let metadata = tokio::fs::metadata(&filepath).await?;
        Ok(PersistedMcpBinaryOutput {
            filepath,
            original_size: metadata.len() as i64,
            mime_type: mime_type.unwrap_or("application/octet-stream").to_string(),
        })
    }
}

/// Map a MIME type to a file extension. Falls back to `bin`.
pub fn extension_for_mime_type(mime_type: Option<&str>) -> &'static str {
    let Some(mime_type) = mime_type else {
        return "bin";
    };
    let mime = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "application/json" => "json",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" => "gz",
        "application/octet-stream" => "bin",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "image/gif" => "gif",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "text/csv" => "csv",
        "text/html" => "html",
        "text/markdown" => "md",
        "text/plain" => "txt",
        "video/mp4" => "mp4",
        "video/mpeg" => "mpeg",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        _ => "bin",
    }
}

fn format_byte_size(bytes: usize) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{bytes} bytes");
    }
    if kb < 1024.0 {
        return format!("{}KB", trim_trailing_zero_decimal(kb));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{}MB", trim_trailing_zero_decimal(mb));
    }
    let gb = mb / 1024.0;
    format!("{}GB", trim_trailing_zero_decimal(gb))
}

fn trim_trailing_zero_decimal(n: f64) -> String {
    let s = format!("{n:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
#[path = "tool_result_storage.test.rs"]
mod tests;
