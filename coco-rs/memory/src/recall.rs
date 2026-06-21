//! Relevant-memory selection (LLM side-query + heuristic fallback).
//!
//! Sends the manifest and the current user query to a fast model;
//! receives a JSON list of up to five filenames the model thinks are
//! most relevant. We track which files have already been surfaced this
//! session so the recall doesn't re-inject the same memory turn after turn.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use coco_types::SideQueryResponse;

use crate::scan::ScannedMemory;
use crate::scan::file_mtime_ms;
use crate::scan::format_memory_manifest;
use crate::scan::memory_age_string;
use crate::scan::memory_freshness_text;

/// Hard cap on relevant memories returned per turn.
pub const MAX_RELEVANT: usize = 5;

/// Per-memory body cap when injecting content into the recall prompt or
/// the user-context attachment.
pub const MAX_BODY_BYTES: usize = 4_096;

/// Cumulative cap on bytes injected per session so a single chatty
/// memory directory can't blow the context budget.
pub const MAX_SESSION_BYTES: i64 = 60 * 1024;

/// One memory selected as relevant to the current query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelevantMemory {
    pub path: String,
    pub content: String,
    pub mtime_ms: i64,
    /// Pre-computed `[age] filename` header for the attachment block.
    pub header: String,
}

/// Cross-turn state for recall: tracks which memories have already
/// been surfaced and the cumulative byte cost.
#[derive(Debug, Default)]
pub struct PrefetchState {
    inner: Mutex<PrefetchInner>,
}

#[derive(Debug, Default)]
struct PrefetchInner {
    already_surfaced: HashSet<String>,
    total_bytes: i64,
}

impl PrefetchState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `expect` semantics on poison: a poisoned `Mutex` means a panic
    /// happened mid-write on `already_surfaced` or `total_bytes`,
    /// leaving the inner state inconsistent. The previous silent-recovery
    /// (`unwrap_or(false)` / `unwrap_or(true)`) masked the bug AND
    /// produced wrong answers downstream (recall surfaces duplicates
    /// or starves the budget). Library-internal invariants should
    /// panic loudly so the bug surfaces in tests rather than
    /// silently rotting recall behavior for the rest of the session.
    fn lock(&self) -> std::sync::MutexGuard<'_, PrefetchInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_surfaced(&self, path: &str) -> bool {
        self.lock().already_surfaced.contains(path)
    }

    pub fn mark_surfaced(&self, path: &str, bytes: i64) {
        let mut s = self.lock();
        s.already_surfaced.insert(path.to_string());
        s.total_bytes += bytes;
    }

    pub fn is_budget_exhausted(&self) -> bool {
        self.lock().total_bytes >= MAX_SESSION_BYTES
    }

    /// Wipe the surfaced set + byte counter — call from
    /// [`crate::MemoryRuntime::reset`] on `/clear` and from the
    /// compact post-step so a fresh transcript can re-surface memory
    /// without inheriting the prior session's dedup + 60 KB cap.
    pub fn reset(&self) {
        let mut s = self.lock();
        s.already_surfaced.clear();
        s.total_bytes = 0;
    }
}

/// System prompt for the relevance-ranker side-query.
///
/// Multi-LLM neutral wording (no "Claude Code" reference); the
/// `with_skip_system_prefix(true)` on the request keeps the agent's
/// preamble out of the ranker so this stays the only system context.
pub const SELECT_MEMORIES_SYSTEM_PROMPT: &str = "\
You are selecting memories that will be useful to an AI coding assistant as it processes a \
user's query. You will be given the user's query and a list of available memory files with \
their filenames and descriptions.\n\
\n\
Return a JSON object: {\"selected_memories\": [\"filename.md\", ...]} listing up to 5 filenames \
for memories that will clearly be useful. Only include memories you are certain will help.\n\
- If unsure, leave it out. Be selective.\n\
- If none clearly help, return an empty list.\n\
- If a list of recently-used tools is provided, do NOT select API/usage references for those \
tools (the assistant is already exercising them). DO still select warnings, gotchas, or known \
issues about those tools — active use is exactly when those matter.";

/// Build the user-side prompt for the recall ranker.
///
/// Layout:
///
/// ```text
/// Query: <query>
///
/// Available memories:
/// - [<type>] <filename> (<iso-ts>): <description>
/// ...
///
/// Recently used tools: <tool>, <tool>
/// ```
///
/// Uses [`format_memory_manifest`] for the bullet body so the manifest
/// format stays stable — calibration of the ranker depends on this.
/// Already-surfaced files are
/// filtered before formatting (the ranker wastes turns if it returns
/// names the loader will then drop).
pub fn build_selection_prompt(
    query: &str,
    scanned: &[ScannedMemory],
    state: &PrefetchState,
    recent_tools: &[String],
) -> String {
    let visible: Vec<ScannedMemory> = scanned
        .iter()
        .filter(|m| !state.is_surfaced(&m.path.to_string_lossy()))
        .cloned()
        .collect();

    let mut out = String::new();
    out.push_str(&format!("Query: {query}\n\nAvailable memories:\n"));
    if visible.is_empty() {
        out.push_str("(none)");
    } else {
        out.push_str(&format_memory_manifest(&visible));
    }
    if !recent_tools.is_empty() {
        out.push_str(&format!(
            "\n\nRecently used tools: {}",
            recent_tools.join(", ")
        ));
    }
    out
}

/// Outcome of decoding a ranker response into a memory selection.
///
/// The caller in [`crate::MemoryRuntime::recall`] uses this to decide
/// whether the response is **trustworthy** (legal JSON shape — even an
/// empty `selected_memories` array counts) versus **malformed**
/// (truncated, non-JSON, or no extractable selection container).
/// Trustworthy responses are taken at face value — an empty selection
/// is a legitimate "no matches" verdict. Malformed responses trigger
/// the forced-tool fallback so a transient provider format hiccup
/// (broken JSON from the structured-output API, markdown wrapper that
/// doesn't carry an embeddable array) doesn't suppress recall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallSelection {
    /// Response was syntactically well-formed and matched our expected
    /// recall schema. The list may legitimately be empty when the
    /// model decided no memories were relevant.
    Parsed(Vec<String>),
    /// Response could not be decoded against the recall schema —
    /// truncated JSON, illegal characters, or neither a
    /// `selected_memories` object nor an extractable bare array.
    /// Caller should fall back to the multi-LLM forced-tool path.
    Malformed,
}

/// Parse a textual ranker response into a [`RecallSelection`].
///
/// Accepts:
/// 1. `{"selected_memories": [...]}` — the canonical structured-output
///    shape. An empty array is `Parsed(vec![])`, NOT `Malformed`.
/// 2. JSON object missing the `selected_memories` field but otherwise
///    parseable (e.g. `{"reason": "no matches"}`) — treated as
///    `Parsed(vec![])` so a model that legitimately answered "nothing
///    relevant" in free-form JSON does not trigger a fallback retry.
/// 3. Bare `[...]` array somewhere in the response — legacy fallback
///    for markdown-wrapped responses (`\`\`\`json\n[...]\n\`\`\``).
///
/// Anything else — truncated, non-JSON, or no recognisable container —
/// returns [`RecallSelection::Malformed`].
pub fn parse_selection_response(response: &str) -> RecallSelection {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return RecallSelection::Malformed;
    }
    // Two-stage parse, both routed through the workspace's single
    // JSON-repair source of truth (`coco_utils_json_repair`):
    //
    //   1. Strict `serde_json` first (no repair fired); accepts any
    //      legal JSON shape — `{selected_memories: [...]}`, bare
    //      `[...]`, or other valid JSON — and extracts what we can,
    //      including legitimate empty `[]`.
    //   2. On strict-parse failure, hand the trimmed input to
    //      `parse_with_repair`. This recovers from common LLM tics:
    //      markdown code fences, trailing commas, single quotes,
    //      missing brackets, truncated structured-output responses.
    //
    // An empty `selected_memories: []` (or any legal JSON without our
    // schema) returns `Parsed(vec![])` — the runtime treats this as a
    // legitimate "no matches" verdict and does NOT trigger the
    // forced-tool fallback. Only genuinely malformed input
    // (un-parseable even after repair) falls through to `Malformed`.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return extract_from_json_value(v);
    }
    if let Ok((v, outcome)) = coco_utils_json_repair::parse_with_repair(trimmed) {
        if matches!(outcome, coco_utils_json_repair::RepairOutcome::Repaired) {
            // Repair fires when strict JSON parse failed but
            // `llm_json` recovered a parseable value (markdown
            // fence stripped, trailing comma dropped, truncation
            // closed, etc). Log at debug level so dashboards can
            // monitor real-world hit rate without burying happy-
            // path logs; bump to warn if a specific model regresses
            // consistently.
            tracing::debug!(
                target: "coco_memory::recall",
                response_bytes = trimmed.len(),
                "recall ranker response required JSON repair before extraction"
            );
        }
        return extract_from_json_value(v);
    }
    RecallSelection::Malformed
}

/// Pull `selected_memories` from a parsed JSON value, accepting the
/// three legitimate shapes the recall ranker may emit. Any other
/// legal JSON shape yields `Parsed(vec![])` — a parseable response
/// that just doesn't carry recall content is not a wire-format bug.
fn extract_from_json_value(v: serde_json::Value) -> RecallSelection {
    if let Some(arr) = v.get("selected_memories").and_then(|x| x.as_array()) {
        return RecallSelection::Parsed(
            arr.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect(),
        );
    }
    if let Some(arr) = v.as_array() {
        return RecallSelection::Parsed(
            arr.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect(),
        );
    }
    RecallSelection::Parsed(Vec::new())
}

/// Decode a [`SideQueryResponse`] into a [`RecallSelection`].
///
/// Honors both wire shapes the ranker can emit:
///
/// 1. **`tool_uses[0].input`** — populated by the forced-tool path
///    and by the Anthropic adapter's synthetic-json-tool fallback
///    when its per-model capability table doesn't know about a newer
///    Claude. Tool inputs are already typed JSON, so the only
///    way to "fail" here is for the provider to return tool_uses
///    without a usable `selected_memories` field — which we treat as
///    a legitimate empty verdict (`Parsed(vec![])`).
/// 2. **`text`** — populated by the native structured-output path
///    (OpenAI `response_format.json_schema`, Gemini `responseSchema`,
///    Anthropic `output_format` with the structured-outputs beta).
///    Routed through [`parse_selection_response`] for the
///    parseability gate.
///
/// A response with neither text nor tool_uses is [`RecallSelection::Malformed`].
pub fn extract_recall_selection(resp: &SideQueryResponse) -> RecallSelection {
    if let Some(tu) = resp.tool_uses.first() {
        // Adapter signalled it could not parse the raw `arguments`
        // JSON (strict + repair both failed). Treat as wire-malformed
        // so the runtime's two-attempt strategy triggers the
        // forced-tool fallback (when this came from the structured-
        // output path) or surfaces empty (when this is itself the
        // forced-tool path's response). Without this check we'd
        // silently return `Parsed(vec![])` on a Value::Null tool
        // input and recall would mysteriously stay empty.
        if tu.invalid {
            tracing::debug!(
                target: "coco_memory::recall",
                tool_name = %tu.name,
                "recall side-query tool_use marked invalid by adapter; \
                 treating as malformed for fallback"
            );
            return RecallSelection::Malformed;
        }
        let names = tu
            .input
            .get("selected_memories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return RecallSelection::Parsed(names);
    }
    match resp.text.as_deref() {
        Some(text) => parse_selection_response(text),
        None => RecallSelection::Malformed,
    }
}

/// Load selected memories from disk, applying truncation, freshness
/// headers, and the per-session byte budget. Marks each loaded path as
/// "already surfaced" so it isn't picked again later in the session.
pub fn load_relevant_memories(
    selected_paths: &[String],
    state: &PrefetchState,
) -> Vec<RelevantMemory> {
    let mut out = Vec::new();
    for path_str in selected_paths {
        if out.len() >= MAX_RELEVANT {
            break;
        }
        if state.is_budget_exhausted() {
            break;
        }
        if state.is_surfaced(path_str) {
            continue;
        }
        let path = Path::new(path_str);
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let truncated = if content.len() > MAX_BODY_BYTES {
            let cut = content[..MAX_BODY_BYTES]
                .rfind('\n')
                .unwrap_or(MAX_BODY_BYTES);
            format!(
                "{}\n\nThis memory file was truncated ({MAX_BODY_BYTES} byte limit). Use the \
                 Read tool to view the complete file at: {path_str}",
                &content[..cut]
            )
        } else {
            content
        };
        let mtime_ms = file_mtime_ms(path).unwrap_or(0);
        let age = memory_age_string(mtime_ms);
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        // Freshness caveat is prepended for memories older than one day
        // so the model doesn't blindly trust stale file/line references.
        let freshness = memory_freshness_text(mtime_ms);
        let header = if freshness.is_empty() {
            format!("[{age}] {filename}")
        } else {
            // Blank line between staleness caveat and entry
            // (spacing owned by the caller, not the text).
            format!("{freshness}\n\n[{age}] {filename}")
        };
        let bytes = truncated.len() as i64;
        state.mark_surfaced(path_str, bytes);
        out.push(RelevantMemory {
            path: path_str.clone(),
            content: truncated,
            mtime_ms,
            header,
        });
    }
    out
}

// Note: a previous `select_heuristic` fallback was deliberately
// removed. When the ranker errors or no LLM handle is installed,
// recall stays silent (returns empty) — surfacing arbitrarily-recent
// memories that occupy attention budget and the 60 KB session byte
// cap is worse than surfacing none. The runtime treats "ranker
// unavailable" the same as "ranker returned no matches."

#[cfg(test)]
#[path = "recall.test.rs"]
mod tests;
