//! Tool result budget — Level 1 (per-tool persistence) + Level 2
//! (per-message aggregate cap).
//!
//!
//! **Level 1** (per-tool): each tool declares
//! [`crate::Tool::max_result_size_bound`]. When a tool result exceeds
//! the declared cap, [`persist_to_disk`] writes the body to
//! `<session_dir>/tool-results/<id>.{txt,json}` and returns a
//! [`PersistedToolResult`] reference the runtime substitutes for the
//! original content. Tools opt out by returning [`ResultSizeBound::Unbounded`].
//!
//! **Level 2** (per-message): `apply_tool_result_budget` walks tool
//! results in one API-level user-message group and persists the
//! largest fresh results until the group fits
//! [`crate::tool_result_storage::ContentReplacementState`]'s
//! `per_message_chars` budget. Replacement strings are cached by
//! `tool_use_id` so subsequent prompt projections replay byte-
//! identical `<persisted-output>` previews.
//!
//! Both levels are **inert by default** — `apply_tool_result_budget` returns the input
//! unchanged when `state.per_message_chars` is `i64::MAX` (the
//! "feature off" sentinel) and `persist_to_disk` is only called by
//! callers that know their tool opted in.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::RwLock;

/// Default per-tool persistence threshold.
pub const DEFAULT_MAX_RESULT_SIZE_CHARS: i64 = 50_000;

/// Default [`Tool::max_result_size_bound`] declaration for tools that do not
/// opt out or tighten the cap (tool default: `100_000`, then clamped by
/// [`DEFAULT_MAX_RESULT_SIZE_CHARS`]).
pub const DEFAULT_TOOL_MAX_RESULT_SIZE_BOUND: ResultSizeBound = ResultSizeBound::Chars(100_000);

/// Default per-message aggregate cap.
pub const DEFAULT_MAX_PER_MESSAGE_CHARS: i64 = 200_000;

/// Subdirectory name for tool results within a session.
pub const TOOL_RESULTS_SUBDIR: &str = "tool-results";

/// XML tag wrapping the persisted-output reference message.
pub const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
pub const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

/// Replacement marker for Level 2 budget eviction.
pub const TOOL_RESULT_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

/// Per-tool persistence cap declaration.
///
/// Replaces the legacy `i64`-with-`i64::MAX`-sentinel convention. The
/// `Chars` variant always carries a positive byte cap; `Unbounded` makes
/// the tool's opt-out explicit so callers (Level 1 persist + Level 2
/// aggregate budget) match on it instead of comparing to a magic number.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultSizeBound {
    /// Cap inline result at this many UTF-8 bytes. Must be positive;
    /// callers that need fallible construction use [`Self::try_chars`].
    Chars(i64),
    /// Tool opts out of Level 1 persistence — its content is canonical
    /// (e.g. `Read` on a tracked file the model will read again). Inline
    /// regardless of length (`Tool.maxResultSizeChars = Infinity`).
    Unbounded,
}

impl ResultSizeBound {
    /// Const constructor. Panics in `const` evaluation if `n <= 0`.
    pub const fn chars(n: i64) -> Self {
        assert!(n > 0, "ResultSizeBound::chars requires a positive cap");
        Self::Chars(n)
    }

    /// Fallible constructor.
    pub const fn try_chars(n: i64) -> Option<Self> {
        if n > 0 { Some(Self::Chars(n)) } else { None }
    }

    pub const fn is_unbounded(self) -> bool {
        matches!(self, Self::Unbounded)
    }

    /// Cap in chars, or `None` for `Unbounded`.
    pub const fn as_chars(self) -> Option<i64> {
        match self {
            Self::Chars(n) => Some(n),
            Self::Unbounded => None,
        }
    }
}

/// Resolved persistence threshold for one tool.
///
/// - [`ResultSizeBound::Unbounded`] declared → opt-out (returned verbatim).
/// - Otherwise: clamps `declared` against [`DEFAULT_MAX_RESULT_SIZE_CHARS`].
pub fn resolve_persistence_threshold(declared: ResultSizeBound) -> ResultSizeBound {
    match declared {
        ResultSizeBound::Unbounded => ResultSizeBound::Unbounded,
        ResultSizeBound::Chars(n) => ResultSizeBound::Chars(n.min(DEFAULT_MAX_RESULT_SIZE_CHARS)),
    }
}

/// Per-session tool-result directory.
pub fn tool_results_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(TOOL_RESULTS_SUBDIR)
}

/// Path where a persisted tool result lives.
pub fn tool_result_path(session_dir: &Path, id: &str, is_json: bool) -> PathBuf {
    let ext = if is_json { "json" } else { "txt" };
    tool_results_dir(session_dir).join(format!("{id}.{ext}"))
}

/// Outcome of persisting a tool result to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedToolResult {
    pub filepath: PathBuf,
    pub original_size: i64,
    pub is_json: bool,
    pub preview: String,
    pub has_more: bool,
}

/// Outcome of persisting a binary MCP output to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedMcpBinaryOutput {
    pub filepath: PathBuf,
    pub original_size: i64,
    pub mime_type: String,
}

/// Preview size in bytes for the reference message.
pub const PREVIEW_SIZE_BYTES: usize = 2000;

/// Return a UTF-8-safe preview no longer than `max_bytes`.
///
/// When possible, cut at the last newline before the byte cap so the
/// model sees whole lines in the preview. `has_more` reports whether
/// the original content exceeded the cap.
pub fn generate_preview(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_string(), false);
    }

    let mut cut = max_bytes.min(content.len());
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    if cut == 0 {
        return (String::new(), true);
    }

    let bytes = content.as_bytes();
    if let Some(newline_idx) = bytes[..cut].iter().rposition(|&b| b == b'\n')
        && newline_idx > 0
    {
        cut = newline_idx + 1;
    }

    (content[..cut].to_string(), true)
}

/// Persist a tool result to disk and return a structured reference
/// the caller substitutes for the inline content. Caller decides
/// whether to invoke based on `content.len() > resolve_persistence_threshold(...)`.
///
/// `content` is the raw tool result body. `is_json` selects extension
/// (`.json` vs `.txt`) and is informational only — content is written verbatim.
pub async fn persist_to_disk(
    session_dir: &Path,
    id: &str,
    content: &str,
    is_json: bool,
) -> std::io::Result<PersistedToolResult> {
    let dir = tool_results_dir(session_dir);
    tokio::fs::create_dir_all(&dir).await?;
    let filepath = tool_result_path(session_dir, id, is_json);
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&filepath)
        .await
    {
        Ok(mut file) => {
            use tokio::io::AsyncWriteExt;
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }

    let stored = tokio::fs::read_to_string(&filepath).await?;
    let (preview, has_more) = generate_preview(&stored, PREVIEW_SIZE_BYTES);
    Ok(PersistedToolResult {
        filepath,
        original_size: stored.len() as i64,
        is_json,
        preview,
        has_more,
    })
}

pub fn mcp_binary_output_path(session_dir: &Path, id: &str, mime_type: Option<&str>) -> PathBuf {
    tool_results_dir(session_dir).join(format!("{}.{}", id, extension_for_mime_type(mime_type)))
}

/// Persist binary MCP output to disk and return a model-visible reference.
///
/// Stores binary MCP payloads under the same per-session `tool-results`
/// directory as text tool results, deriving the file extension from MIME type.
pub async fn persist_mcp_binary_to_disk(
    session_dir: &Path,
    id: &str,
    bytes: &[u8],
    mime_type: Option<&str>,
) -> std::io::Result<PersistedMcpBinaryOutput> {
    let dir = tool_results_dir(session_dir);
    tokio::fs::create_dir_all(&dir).await?;
    let filepath = mcp_binary_output_path(session_dir, id, mime_type);
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

/// Render the `<persisted-output>` reference message body.
pub fn render_persisted_reference(persisted: &PersistedToolResult) -> String {
    let mut buf = String::with_capacity(persisted.preview.len() + 256);
    buf.push_str(PERSISTED_OUTPUT_TAG);
    buf.push('\n');
    buf.push_str(&format!(
        "Output too large ({}). Full output saved to: {}\n\n",
        format_byte_size(persisted.original_size as usize),
        persisted.filepath.display()
    ));
    buf.push_str(&format!(
        "Preview (first {}):\n",
        format_byte_size(PREVIEW_SIZE_BYTES)
    ));
    buf.push_str(&persisted.preview);
    buf.push_str(if persisted.has_more { "\n...\n" } else { "\n" });
    buf.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    buf
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

pub fn is_content_already_persisted(content: &str) -> bool {
    content.trim_start().starts_with(PERSISTED_OUTPUT_TAG)
}

pub fn empty_tool_result_message(tool_name: &str) -> String {
    format!("({tool_name} completed with no output)")
}

/// Per-session content-replacement state for Level 2 budget. Tracks:
///
/// - `replacements`: tool_use_id → replacement string (the exact
///   `<persisted-output>` preview body). Keyed for prompt-cache
///   stability — same id always projects to the same replacement
///   across re-renders.
/// - `seen_ids`: tool_use_ids the budget has already considered. Once
///   seen, a result is "frozen" — never re-replaced even if it
///   shrinks under the cap.
/// - `per_message_chars`: budget cap. `i64::MAX` ⇒ feature off
///   (`apply_tool_result_budget` returns input unchanged).
#[derive(Debug, Default, Clone)]
pub struct ContentReplacementState {
    pub replacements: HashMap<String, String>,
    pub seen_ids: std::collections::HashSet<String>,
    pub per_message_chars: i64,
}

impl ContentReplacementState {
    pub fn new(per_message_chars: i64) -> Self {
        Self {
            per_message_chars,
            ..Default::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.per_message_chars != i64::MAX
    }
}

/// Shared handle for engine wiring.
pub type ContentReplacementStateRef = Arc<RwLock<ContentReplacementState>>;

/// One tool-result candidate for budget evaluation. Caller projects
/// from their message representation (typically `tool_result` blocks
/// inside a user message). The runtime consumes a flat list because
/// the engine's message types live in `coco-messages` (a higher
/// layer) — passing typed refs here would require depending on it.
#[derive(Debug, Clone)]
pub struct ToolResultCandidate {
    pub tool_use_id: String,
    pub content: String,
    pub content_chars: i64,
    /// Tool name when known — drives Level 1 per-tool opt-out
    /// (`is_persistence_opted_out`). `None` ⇒ apply Level 2 only.
    pub tool_name: Option<String>,
    /// Whether this candidate's tool opted out of persistence
    /// (declared [`ResultSizeBound::Unbounded`] for `max_result_size_bound`).
    /// When `true`, the budget pipeline skips it (canonical-content tools
    /// like `Read` on a tracked file use this).
    pub persistence_opted_out: bool,
    /// Whether the persisted file should use `.json` rather than
    /// `.txt`.
    pub is_json: bool,
}

/// A single tool-result replacement record.
///
/// Returned by [`apply_tool_result_budget`] as `BudgetOutcome.newly_replaced`
/// and persisted alongside the message log. Resume seeds the
/// replacement map directly off the loaded records (see
/// `seed_tool_result_replacement_state` in `coco-cli`); no shared
/// rebuild function — each consumer (TUI, SDK, headless) owns the
/// seeding because the surrounding state plumbing differs slightly.
///
/// Serializable for transcript persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentReplacement {
    pub tool_use_id: String,
    pub replacement: String,
}

/// Type alias for transcript-persisted records. Identical layout to
/// [`ContentReplacement`] (same payload — the budget pass emits, the
/// transcript persists, the resume path reads back). We keep both names
/// so the call site
/// reads naturally (`outcome.newly_replaced` vs `records: &[...Record]`).
pub type ContentReplacementRecord = ContentReplacement;

/// Outcome of running [`apply_tool_result_budget`].
#[derive(Debug, Clone, Default)]
pub struct BudgetOutcome {
    /// Tool-use IDs that got newly replaced this pass (caller
    /// substitutes each replacement in the API prompt projection).
    pub newly_replaced: Vec<ContentReplacement>,
    /// Total chars freed from the in-message aggregate.
    pub freed_chars: i64,
}

/// Decide which fresh tool-result candidates to persist to fit the
/// per-message char budget:
///
/// 1. Re-apply cached replacements from `state.replacements`.
/// 2. Compute aggregate size using cached replacement length for
///    already-replaced IDs and inline length for everything else.
/// 3. If aggregate exceeds the cap, pick largest fresh candidates
///    (`!seen_ids`, not already replaced, not opted out), persist each,
///    and store the exact `<persisted-output>` replacement string.
/// 4. Mark every candidate as seen. Persist failures are frozen
///    without replacement so later turns do not make different
///    replacement decisions for the same ID.
///
/// Caller applies `state.replacements` to the actual message content
/// (lookup by `tool_use_id`).
pub async fn apply_tool_result_budget(
    candidates: &[ToolResultCandidate],
    state: &ContentReplacementStateRef,
    session_dir: &Path,
) -> BudgetOutcome {
    let snapshot = {
        let state = state.read().await;
        if !state.is_active() {
            return BudgetOutcome::default();
        }
        (
            state.per_message_chars,
            state.seen_ids.clone(),
            state.replacements.clone(),
        )
    };
    let (per_message_chars, seen_ids, replacements) = snapshot;

    // Partition by prior decision (per-message budget walk). Already-replaced
    // IDs (`mustReapply`) are
    // re-applied by the caller from `state.replacements` and contribute 0 to
    // the trigger total. `frozen` (seen, never replaced) counts at full size;
    // `fresh` (never seen) is the new message's content, budgeted this pass.
    let mut fresh: Vec<&ToolResultCandidate> = Vec::new();
    let mut frozen_chars: i64 = 0;
    for c in candidates {
        if replacements.contains_key(&c.tool_use_id) {
            // mustReapply — excluded from the trigger total.
        } else if seen_ids.contains(&c.tool_use_id) {
            frozen_chars += c.content_chars;
        } else {
            fresh.push(c);
        }
    }

    // A re-processed message has no fresh candidates; freeze and return so the
    // caller just re-applies cached replacements.
    if fresh.is_empty() {
        let mut state = state.write().await;
        for c in candidates {
            state.seen_ids.insert(c.tool_use_id.clone());
        }
        return BudgetOutcome::default();
    }

    // Opted-out tools (Read, `Unbounded`) and content that is already a
    // persisted reference never persist and never count toward the trigger.
    let mut eligible: Vec<&ToolResultCandidate> = fresh
        .iter()
        .copied()
        .filter(|c| !c.persistence_opted_out && !is_content_already_persisted(&c.content))
        .collect();
    let fresh_chars: i64 = eligible.iter().map(|c| c.content_chars).sum();

    let mut still_over = frozen_chars + fresh_chars;
    if still_over <= per_message_chars {
        let mut state = state.write().await;
        for c in candidates {
            state.seen_ids.insert(c.tool_use_id.clone());
        }
        return BudgetOutcome::default();
    }

    eligible.sort_by(|a, b| b.content_chars.cmp(&a.content_chars));

    let mut outcome = BudgetOutcome::default();
    for cand in eligible {
        if still_over <= per_message_chars {
            break;
        }
        match persist_to_disk(session_dir, &cand.tool_use_id, &cand.content, cand.is_json).await {
            Ok(persisted) => {
                let replacement = render_persisted_reference(&persisted);
                still_over -= cand.content_chars - replacement.len() as i64;
                outcome.freed_chars += cand.content_chars - replacement.len() as i64;
                outcome.newly_replaced.push(ContentReplacement {
                    tool_use_id: cand.tool_use_id.clone(),
                    replacement,
                });
            }
            Err(_) => {
                // Best effort, but frozen: do not keep retrying and
                // risk changing the prompt prefix on later turns.
            }
        }
    }
    let mut state = state.write().await;
    for replacement in &outcome.newly_replaced {
        state.replacements.insert(
            replacement.tool_use_id.clone(),
            replacement.replacement.clone(),
        );
    }
    for c in candidates {
        state.seen_ids.insert(c.tool_use_id.clone());
    }
    outcome
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

fn extension_for_mime_type(mime_type: Option<&str>) -> &'static str {
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

fn trim_trailing_zero_decimal(n: f64) -> String {
    let s = format!("{n:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
#[path = "tool_result_storage.test.rs"]
mod tests;
