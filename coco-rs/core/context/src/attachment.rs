//! Attachment system for per-turn context injection.
//!
//! Handles files, PDFs, memories, hooks, teammates, deferred tools,
//! agent listings, and MCP instructions.
//!
//! Attachments are generated in three parallel batches:
//! - **UserInput**: files/agents mentioned in the prompt
//! - **AllThread**: renewals safe for subagents (queued commands, changed files, etc.)
//! - **MainThreadOnly**: IDE state, diagnostics, token usage (main thread only)
//!
//! Each attachment carries a token estimate so the caller can enforce a
//! per-turn budget without re-serializing.

use std::collections::HashSet;
use std::path::Path;

use coco_types::PermissionBehavior;
use coco_types::ToolName;
use serde::Deserialize;
use serde::Serialize;

use coco_messages::estimate_text_tokens as estimate_tokens;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Max lines to read from a memory file before truncation.
const MAX_MEMORY_LINES: i32 = 200;

/// Max bytes per memory file (200 x 500-char lines = 100KB unbounded;
/// cap keeps aggregate injection bounded: 5 x 4KB = 20KB/turn).
const MAX_MEMORY_BYTES: i64 = 4096;

/// Session-level cap on cumulative relevant-memory bytes (~3 full injections).
const MAX_SESSION_MEMORY_BYTES: i64 = 60 * 1024;

/// Rough chars-per-token for budget calculations (conservative).
const CHARS_PER_TOKEN: f64 = 4.0;

/// Default per-attachment token budget when none is specified.
const DEFAULT_MAX_TOKENS_PER_ATTACHMENT: i64 = 8_000;

/// PDF page threshold: PDFs with more pages become lightweight references.
/// PDFs with more than this many pages stay a reference; smaller ones are
/// inlined as extracted text.
const PDF_INLINE_THRESHOLD: i32 = 10;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Source that produced an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentSource {
    /// User @-mentioned a file in the prompt.
    AtMention,
    /// File changed on disk since last read.
    ChangedFile,
    /// Post-compaction restoration of an @-mentioned file.
    CompactRestore,
    /// Nested CLAUDE.md discovered during directory traversal.
    NestedMemory,
    /// Relevant memory surfaced by the ranker.
    RelevantMemory,
    /// Skill listing injection.
    SkillListing,
    /// Deferred tools delta.
    DeferredTools,
    /// Agent listing delta.
    AgentListing,
    /// MCP instructions delta.
    McpInstructions,
    /// IDE selection or opened file.
    Ide,
    /// Hook output.
    Hook,
    /// Teammate mailbox message.
    Teammate,
    /// System-generated (plan mode, token usage, etc.).
    System,
}

/// Which batch an attachment belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentBatch {
    /// User input attachments (files mentioned in prompt).
    UserInput,
    /// All-thread renewals (queued commands, changed files, etc.).
    AllThread,
    /// Main-thread-only updates (IDE state, diagnostics, etc.).
    MainThreadOnly,
}

/// The primary attachment enum — each variant corresponds to one attachment type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Attachment {
    /// A file read into context (text content).
    File(FileAttachment),
    /// A compact reference to a file (no content, just path).
    CompactFileReference(CompactFileReferenceAttachment),
    /// A PDF too large to inline — lightweight reference.
    PdfReference(PdfReferenceAttachment),
    /// A file already present in context (dedup marker).
    AlreadyReadFile(AlreadyReadFileAttachment),
    /// An image file attachment (stub — base64 data).
    Image(ImageAttachment),
    /// A directory listing.
    Directory(DirectoryAttachment),
    /// An @-mentioned agent.
    AgentMention(AgentMentionAttachment),
    /// Hook execution result.
    Hook(HookAttachment),
    /// A nested CLAUDE.md / memory file.
    NestedMemory(NestedMemoryAttachment),
    /// Relevant memories surfaced by the ranker.
    RelevantMemories(RelevantMemoriesAttachment),
    /// Memory file attachment (legacy / simple).
    Memory(MemoryAttachment),
    /// Teammate mailbox messages.
    TeammateMailbox(TeammateMailboxAttachment),
    /// Skill listing (bundled + MCP commands).
    SkillListing(SkillListingAttachment),
    /// Deferred tools delta announcement.
    DeferredToolsDelta(DeferredToolsDeltaAttachment),
    /// Agent listing delta announcement.
    AgentListingDelta(AgentListingDeltaAttachment),
    /// MCP server instructions delta.
    McpInstructionsDelta(McpInstructionsDeltaAttachment),
    /// Plan mode reminder.
    PlanMode(PlanModeAttachment),
    /// Plan mode exit notification.
    PlanModeExit(PlanModeExitAttachment),
    /// Token usage report.
    TokenUsage(TokenUsageAttachment),
    /// Date change notification.
    DateChange(DateChangeAttachment),
    /// Generic system reminder (extensible).
    SystemReminder {
        attachment_type: String,
        content: String,
    },
}

// ---------------------------------------------------------------------------
// Attachment variant structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachment {
    pub filename: String,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
    /// Path relative to CWD at creation time, for stable display.
    pub display_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactFileReferenceAttachment {
    pub filename: String,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfReferenceAttachment {
    pub filename: String,
    pub page_count: i32,
    pub file_size: i64,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlreadyReadFileAttachment {
    pub filename: String,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub filename: String,
    pub display_path: String,
    /// Media type (e.g., "image/png").
    pub media_type: String,
    /// Base64-encoded image data (stub — populated by caller).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryAttachment {
    pub path: String,
    pub content: String,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMentionAttachment {
    pub agent_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HookAttachment {
    Success { content: String, hook_name: String },
    BlockingError { error: String, command: String },
    Cancelled { hook_name: String },
    PermissionDecision { decision: PermissionBehavior },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestedMemoryAttachment {
    pub path: String,
    pub content: String,
    pub memory_type: String,
    pub display_path: String,
}

/// A single relevant memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantMemoryEntry {
    pub path: String,
    pub content: String,
    pub mtime_ms: i64,
    /// Pre-computed header string (age + path prefix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Line count when the file was truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantMemoriesAttachment {
    pub memories: Vec<RelevantMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAttachment {
    pub path: String,
    pub content: String,
    pub memory_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMailboxAttachment {
    pub messages: Vec<TeammateMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    pub from: String,
    pub text: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillListingAttachment {
    pub content: String,
    pub skill_count: i32,
    pub is_initial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredToolsDeltaAttachment {
    pub added_names: Vec<String>,
    pub added_lines: Vec<String>,
    pub removed_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentListingDeltaAttachment {
    pub added_types: Vec<String>,
    pub added_lines: Vec<String>,
    pub removed_types: Vec<String>,
    pub is_initial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInstructionsDeltaAttachment {
    pub added_names: Vec<String>,
    pub added_blocks: Vec<String>,
    pub removed_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeAttachment {
    pub reminder_type: ReminderType,
    /// Which workflow the Full variant should use.
    /// Ignored for Sparse / Reentry (they share one text across workflows).
    #[serde(default)]
    pub workflow: PlanWorkflow,
    /// Phase-4 "Final Plan" prompt strictness.
    /// Only relevant for `(Full, FivePhase, main-agent)`.
    #[serde(default)]
    pub phase4_variant: Phase4Variant,
    /// Number of parallel Explore agents referenced by the 5-phase Full.
    /// Default 3. Ignored for non-5phase.
    #[serde(default = "default_explore_count")]
    pub explore_agent_count: i32,
    /// Number of parallel Plan agents referenced by the 5-phase Full.
    /// Default 1. Ignored for non-5phase.
    #[serde(default = "default_plan_count")]
    pub plan_agent_count: i32,
    /// True when the current turn actually exposes the Agent tool and the
    /// built-in Explore/Plan agent definitions. When false, plan-mode
    /// reminders must not instruct the model to launch those agents.
    #[serde(default)]
    pub explore_plan_agents_available: bool,
    #[serde(default)]
    pub is_sub_agent: bool,
    pub plan_file_path: String,
    pub plan_exists: bool,
    /// Model-aware builtin used to create the plan file. `Write` for the
    /// Claude family, `apply_patch` for gpt-5 (which excludes `Write`). The
    /// builder resolves it via [`coco_types::ToolName::write_tool_for`] /
    /// `ToolOverrides::write_tool` so the reminder never names a tool the
    /// model lacks. Defaults to `Write` for back-compat deserialization.
    #[serde(default = "default_write_tool")]
    pub write_tool: ToolName,
    /// Model-aware builtin used to edit the existing plan file (see
    /// [`Self::write_tool`]). Defaults to `Edit`.
    #[serde(default = "default_edit_tool")]
    pub edit_tool: ToolName,
    /// Tool names that are currently searchable but not callable until
    /// loaded with ToolSearch. Sorted by canonical tool name.
    #[serde(default)]
    pub deferred_tools: Vec<String>,
}

fn default_explore_count() -> i32 {
    3
}
fn default_plan_count() -> i32 {
    1
}
fn default_write_tool() -> ToolName {
    ToolName::Write
}
fn default_edit_tool() -> ToolName {
    ToolName::Edit
}

/// Which plan-mode Full-reminder workflow to render.
///
/// Mirrors `coco_config::PlanModeWorkflow` but lives here to avoid
/// `core/context` depending on `coco-config` (would invert the dep
/// layering); the engine converts settings → attachment at build time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanWorkflow {
    #[default]
    FivePhase,
    Interview,
}

/// Phase-4 prompt variant — only affects 5-phase Full.
///
/// Mirrors `coco_config::PlanPhase4Variant`; same layering rationale as
/// `PlanWorkflow`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase4Variant {
    #[default]
    Standard,
    Trim,
    Cut,
    Cap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderType {
    /// First plan-mode turn: full workflow instructions.
    Full,
    /// Subsequent plan-mode turns: short reminder (prompt-cache friendly).
    Sparse,
    /// Returning to plan mode after previously exiting in this session.
    /// Instructs the model to evaluate the existing plan file against
    /// the new request before refining or overwriting it.
    Reentry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeExitAttachment {
    pub plan_file_path: String,
    pub plan_exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<coco_types::ExitPlanModeOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageAttachment {
    pub used: i64,
    pub total: i64,
    pub remaining: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateChangeAttachment {
    pub new_date: String,
}

// ---------------------------------------------------------------------------
// Token estimation for attachments
// ---------------------------------------------------------------------------

impl Attachment {
    /// Estimate the number of tokens this attachment will consume in the
    /// model context. Used for budget enforcement.
    pub fn estimated_tokens(&self) -> i64 {
        match self {
            Attachment::File(f) => {
                // Content tokens + small overhead for XML wrapping
                estimate_tokens(&f.content) + 20
            }
            Attachment::CompactFileReference(_) => 15,
            Attachment::PdfReference(_) => 30,
            Attachment::AlreadyReadFile(_) => 15,
            Attachment::Image(img) => {
                // ~765 tokens per 512x512 tile; estimate from base64 length
                match &img.base64_data {
                    Some(data) => {
                        let tiles = (data.len() / 250_000).max(1);
                        (tiles * 765) as i64
                    }
                    None => 1000,
                }
            }
            Attachment::Directory(d) => estimate_tokens(&d.content) + 15,
            Attachment::AgentMention(_) => 10,
            Attachment::Hook(h) => match h {
                HookAttachment::Success { content, .. } => estimate_tokens(content) + 10,
                HookAttachment::BlockingError { error, .. } => estimate_tokens(error) + 10,
                HookAttachment::Cancelled { .. } => 10,
                HookAttachment::PermissionDecision { .. } => 10,
            },
            Attachment::NestedMemory(nm) => estimate_tokens(&nm.content) + 20,
            Attachment::RelevantMemories(rm) => {
                rm.memories
                    .iter()
                    .map(|m| estimate_tokens(&m.content) + 15)
                    .sum::<i64>()
                    + 10
            }
            Attachment::Memory(m) => estimate_tokens(&m.content) + 10,
            Attachment::TeammateMailbox(tm) => {
                tm.messages
                    .iter()
                    .map(|m| estimate_tokens(&m.text) + 10)
                    .sum::<i64>()
                    + 10
            }
            Attachment::SkillListing(sl) => estimate_tokens(&sl.content) + 10,
            Attachment::DeferredToolsDelta(d) => {
                d.added_lines
                    .iter()
                    .map(|l| estimate_tokens(l))
                    .sum::<i64>()
                    + 20
            }
            Attachment::AgentListingDelta(a) => {
                a.added_lines
                    .iter()
                    .map(|l| estimate_tokens(l))
                    .sum::<i64>()
                    + 20
            }
            Attachment::McpInstructionsDelta(m) => {
                m.added_blocks
                    .iter()
                    .map(|b| estimate_tokens(b))
                    .sum::<i64>()
                    + 20
            }
            Attachment::PlanMode(_) => 100,
            Attachment::PlanModeExit(_) => 40,
            Attachment::TokenUsage(_) => 30,
            Attachment::DateChange(_) => 15,
            Attachment::SystemReminder { content, .. } => estimate_tokens(content) + 10,
        }
    }

    /// The batch this attachment belongs to.
    pub fn batch(&self) -> AttachmentBatch {
        match self {
            Attachment::File(_)
            | Attachment::CompactFileReference(_)
            | Attachment::PdfReference(_)
            | Attachment::AlreadyReadFile(_)
            | Attachment::Image(_)
            | Attachment::Directory(_)
            | Attachment::AgentMention(_) => AttachmentBatch::UserInput,

            Attachment::NestedMemory(_)
            | Attachment::RelevantMemories(_)
            | Attachment::Memory(_)
            | Attachment::SkillListing(_)
            | Attachment::DeferredToolsDelta(_)
            | Attachment::AgentListingDelta(_)
            | Attachment::McpInstructionsDelta(_)
            | Attachment::PlanMode(_)
            | Attachment::PlanModeExit(_)
            | Attachment::DateChange(_)
            | Attachment::SystemReminder { .. } => AttachmentBatch::AllThread,

            Attachment::Hook(_) | Attachment::TeammateMailbox(_) | Attachment::TokenUsage(_) => {
                AttachmentBatch::MainThreadOnly
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File attachment generation
// ---------------------------------------------------------------------------

/// Options for reading a file into an attachment.
#[derive(Debug, Clone, Default)]
pub struct FileReadOptions {
    /// 1-based line offset to start reading from.
    pub offset: Option<i32>,
    /// Number of lines to read.
    pub limit: Option<i32>,
    /// Max tokens the attachment content may occupy.
    pub max_tokens: Option<i64>,
}

/// Read a file from disk and produce a `FileAttachment`.
///
/// Returns `None` if the file cannot be read (permission denied, not found,
/// etc.). Truncates to `max_tokens` if the file is too large.
pub fn generate_file_attachment(
    path: &Path,
    cwd: &Path,
    options: &FileReadOptions,
) -> Option<Attachment> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let display_path = abs_path
        .strip_prefix(cwd)
        .unwrap_or(&abs_path)
        .to_string_lossy()
        .into_owned();

    // PDF: large PDFs stay a reference; small ones (≤ threshold pages) inline
    // their extracted text like a regular file.
    let pdf_text: Option<String> = if is_pdf_path(&abs_path) {
        match extract_pdf_text(&abs_path) {
            Some(text) => {
                let pages = pdf_page_count_from_text(&text);
                if pages > PDF_INLINE_THRESHOLD {
                    return generate_pdf_reference(&abs_path, &display_path, pages);
                }
                Some(text)
            }
            // Extraction failed (encrypted / malformed): keep a reference with a
            // size-heuristic page count rather than read binary bytes as text.
            None => {
                return generate_pdf_reference(
                    &abs_path,
                    &display_path,
                    pdf_page_count_heuristic(&abs_path),
                );
            }
        }
    } else {
        None
    };

    // Image file: read and base64-encode
    if is_image_path(&abs_path) {
        let base64_data = std::fs::read(&abs_path)
            .ok()
            .map(|bytes| encode_base64(&bytes));
        return Some(Attachment::Image(ImageAttachment {
            filename: abs_path.to_string_lossy().into_owned(),
            display_path,
            media_type: guess_media_type(&abs_path),
            base64_data,
        }));
    }

    let content = match pdf_text {
        Some(text) => text,
        None => std::fs::read_to_string(&abs_path).ok()?,
    };
    let max_tokens = options
        .max_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS_PER_ATTACHMENT);

    // Apply offset/limit
    let (sliced, total_lines) = slice_content(&content, options.offset, options.limit);

    // Truncate by token budget
    let estimated = estimate_tokens(&sliced);
    let (final_content, truncated) = if estimated > max_tokens {
        let max_chars = (max_tokens as f64 * CHARS_PER_TOKEN) as usize;
        let truncated_content = truncate_to_char_boundary(&sliced, max_chars);
        (truncated_content, true)
    } else {
        let line_limit = options.limit.unwrap_or(i32::MAX);
        (sliced, total_lines > line_limit as usize)
    };

    Some(Attachment::File(FileAttachment {
        filename: abs_path.to_string_lossy().into_owned(),
        content: final_content,
        truncated,
        display_path,
        offset: options.offset,
        limit: options.limit,
    }))
}

/// Generate a PDF reference attachment for a large PDF (page count already
/// resolved by the caller).
fn generate_pdf_reference(path: &Path, display_path: &str, page_count: i32) -> Option<Attachment> {
    let file_size = std::fs::metadata(path).ok()?.len() as i64;
    Some(Attachment::PdfReference(PdfReferenceAttachment {
        filename: path.to_string_lossy().into_owned(),
        page_count,
        file_size,
        display_path: display_path.to_owned(),
    }))
}

/// Page count from already-extracted PDF text: `pdf-extract` separates pages
/// with form feed (`\u{0C}`), matching `read.rs::read_pdf`.
fn pdf_page_count_from_text(text: &str) -> i32 {
    let pages = if text.contains('\u{0C}') {
        text.split('\u{0C}')
            .filter(|s| !s.trim().is_empty())
            .count()
    } else {
        1
    };
    pages.max(1) as i32
}

/// Size-based page estimate (~100KB/page) used only when text extraction fails.
fn pdf_page_count_heuristic(path: &Path) -> i32 {
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) as f64;
    ((file_size / (100.0 * 1024.0)).ceil() as i32).max(1)
}

/// Extract a PDF's full text via `pdf-extract`. Returns `None` on read/parse
/// failure (encrypted, malformed, …) so callers fall back to a reference.
fn extract_pdf_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    pdf_extract::extract_text_from_mem(&bytes).ok()
}

// ---------------------------------------------------------------------------
// Memory file attachment loading
// ---------------------------------------------------------------------------

/// Load a memory file into a `NestedMemory` attachment.
///
/// Enforces `MAX_MEMORY_LINES` and `MAX_MEMORY_BYTES`. Truncation surfaces
/// partial content with a note rather than dropping the file.
pub fn load_memory_attachment(path: &Path, memory_type: &str, cwd: &Path) -> Option<Attachment> {
    let content = std::fs::read_to_string(path).ok()?;

    let display_path = path
        .strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();

    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();

    let truncated_by_lines = line_count > MAX_MEMORY_LINES as usize;
    let mut result = if truncated_by_lines {
        lines[..MAX_MEMORY_LINES as usize].join("\n")
    } else {
        content.clone()
    };

    let truncated_by_bytes = result.len() > MAX_MEMORY_BYTES as usize;
    if truncated_by_bytes {
        result = truncate_to_char_boundary(&result, MAX_MEMORY_BYTES as usize);
    }

    let truncated = truncated_by_lines || truncated_by_bytes;
    if truncated {
        let reason = if truncated_by_bytes {
            format!("{MAX_MEMORY_BYTES} byte limit")
        } else {
            format!("first {MAX_MEMORY_LINES} lines")
        };
        result.push_str(&format!(
            "\n\n> This memory file was truncated ({reason}). \
             Use the Read tool to view the complete file at: {}",
            path.display()
        ));
    }

    Some(Attachment::NestedMemory(NestedMemoryAttachment {
        path: path.to_string_lossy().into_owned(),
        content: result,
        memory_type: memory_type.to_owned(),
        display_path,
    }))
}

// ---------------------------------------------------------------------------
// Agent listing attachment
// ---------------------------------------------------------------------------

/// Agent definition (minimal subset for attachment generation).
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub agent_type: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

/// Tracks which file paths have already been injected in the current session.
/// Prevents re-injection of the same CLAUDE.md or memory file.
#[derive(Debug, Default)]
pub struct AttachmentDeduplicator {
    /// Paths already loaded (non-evicting — survives LRU cache evictions).
    loaded_paths: HashSet<String>,
    /// Cumulative bytes of relevant memories surfaced this session.
    relevant_memory_bytes: i64,
}

impl AttachmentDeduplicator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if this path has already been injected.
    pub fn is_loaded(&self, path: &str) -> bool {
        self.loaded_paths.contains(path)
    }

    /// Mark a path as loaded.
    pub fn mark_loaded(&mut self, path: &str) {
        self.loaded_paths.insert(path.to_owned());
    }

    /// Returns `true` if the session memory budget is exhausted.
    pub fn is_session_memory_exhausted(&self) -> bool {
        self.relevant_memory_bytes >= MAX_SESSION_MEMORY_BYTES
    }

    /// Account for relevant memory bytes.
    pub fn add_relevant_memory_bytes(&mut self, bytes: i64) {
        self.relevant_memory_bytes += bytes;
    }

    /// Filter a batch of attachments, removing duplicates and marking survivors.
    pub fn dedup_attachments(&mut self, attachments: Vec<Attachment>) -> Vec<Attachment> {
        attachments
            .into_iter()
            .filter(|att| {
                let path = match att {
                    Attachment::File(f) => Some(f.filename.as_str()),
                    Attachment::NestedMemory(nm) => Some(nm.path.as_str()),
                    Attachment::AlreadyReadFile(a) => Some(a.filename.as_str()),
                    _ => None,
                };

                if let Some(p) = path {
                    if self.is_loaded(p) {
                        return false;
                    }
                    self.mark_loaded(p);
                }
                true
            })
            .collect()
    }

    /// Filter relevant memory attachments, deduping and enforcing session budget.
    pub fn dedup_relevant_memories(
        &mut self,
        attachment: RelevantMemoriesAttachment,
    ) -> Option<RelevantMemoriesAttachment> {
        let filtered: Vec<RelevantMemoryEntry> = attachment
            .memories
            .into_iter()
            .filter(|m| {
                if self.is_loaded(&m.path) {
                    return false;
                }
                self.mark_loaded(&m.path);
                self.add_relevant_memory_bytes(m.content.len() as i64);
                true
            })
            .collect();

        if filtered.is_empty() {
            None
        } else {
            Some(RelevantMemoriesAttachment { memories: filtered })
        }
    }

    /// Number of loaded paths.
    pub fn loaded_count(&self) -> usize {
        self.loaded_paths.len()
    }

    /// Cumulative relevant memory bytes surfaced.
    pub fn relevant_memory_bytes(&self) -> i64 {
        self.relevant_memory_bytes
    }

    /// Reset state (e.g., after compaction).
    pub fn reset(&mut self) {
        self.loaded_paths.clear();
        self.relevant_memory_bytes = 0;
    }
}

// ---------------------------------------------------------------------------
// Token budget management
// ---------------------------------------------------------------------------

/// Budget tracker for total attachment tokens in a single turn.
#[derive(Debug)]
pub struct AttachmentBudget {
    max_tokens: i64,
    used_tokens: i64,
}

impl AttachmentBudget {
    /// Create a budget with the given total token limit.
    pub fn new(max_tokens: i64) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
        }
    }

    /// Remaining token budget.
    pub fn remaining(&self) -> i64 {
        self.max_tokens - self.used_tokens
    }

    /// Try to admit an attachment. Returns `true` if it fits within budget.
    pub fn try_admit(&mut self, attachment: &Attachment) -> bool {
        let cost = attachment.estimated_tokens();
        if self.used_tokens + cost <= self.max_tokens {
            self.used_tokens += cost;
            true
        } else {
            false
        }
    }

    /// Filter a list of attachments, keeping only those that fit within budget.
    /// Attachments are admitted in order; once budget is exhausted, remaining
    /// attachments are dropped.
    pub fn filter_within_budget(&mut self, attachments: Vec<Attachment>) -> Vec<Attachment> {
        attachments
            .into_iter()
            .filter(|att| self.try_admit(att))
            .collect()
    }

    /// Total tokens used.
    pub fn used_tokens(&self) -> i64 {
        self.used_tokens
    }

    /// Maximum token budget.
    pub fn max_tokens(&self) -> i64 {
        self.max_tokens
    }
}

// ---------------------------------------------------------------------------
// Parallel batch orchestration
// ---------------------------------------------------------------------------

/// Timeout for each batch generation.
const BATCH_TIMEOUT_MS: u64 = 1000;

/// Collect attachments for a specific batch from a list of pre-generated attachments.
///
/// Runs three parallel batches with 1000ms timeout each. Callers generate
/// all attachments then partition by batch.
///
/// `is_subagent`: if true, MainThreadOnly batch is filtered out.
pub fn collect_batched_attachments(
    all: Vec<Attachment>,
    dedup: &mut AttachmentDeduplicator,
    budget: &mut AttachmentBudget,
    is_subagent: bool,
) -> Vec<Attachment> {
    let filtered: Vec<Attachment> = if is_subagent {
        all.into_iter()
            .filter(|a| a.batch() != AttachmentBatch::MainThreadOnly)
            .collect()
    } else {
        all
    };

    let deduped = dedup.dedup_attachments(filtered);
    budget.filter_within_budget(deduped)
}

/// Run attachment generation with a per-batch timeout.
///
/// Each `batch_fn` produces attachments for one batch. All three run
/// concurrently via `tokio::join!`, each with a `BATCH_TIMEOUT_MS` timeout.
/// Timed-out batches produce zero attachments.
pub async fn generate_all_attachments_async<F1, F2, F3>(
    user_input_fn: F1,
    all_thread_fn: F2,
    main_thread_fn: F3,
    dedup: &mut AttachmentDeduplicator,
    budget: &mut AttachmentBudget,
    is_subagent: bool,
) -> Vec<Attachment>
where
    F1: std::future::Future<Output = Vec<Attachment>>,
    F2: std::future::Future<Output = Vec<Attachment>>,
    F3: std::future::Future<Output = Vec<Attachment>>,
{
    let timeout = std::time::Duration::from_millis(BATCH_TIMEOUT_MS);

    let (user_result, all_result, main_result) = tokio::join!(
        tokio::time::timeout(timeout, user_input_fn),
        tokio::time::timeout(timeout, all_thread_fn),
        tokio::time::timeout(timeout, main_thread_fn),
    );

    let mut all_attachments = Vec::new();

    match user_result {
        Ok(atts) => all_attachments.extend(atts),
        Err(_) => tracing::warn!("UserInput batch timed out after {BATCH_TIMEOUT_MS}ms"),
    }
    match all_result {
        Ok(atts) => all_attachments.extend(atts),
        Err(_) => tracing::warn!("AllThread batch timed out after {BATCH_TIMEOUT_MS}ms"),
    }
    if !is_subagent {
        match main_result {
            Ok(atts) => all_attachments.extend(atts),
            Err(_) => tracing::warn!("MainThreadOnly batch timed out after {BATCH_TIMEOUT_MS}ms"),
        }
    }

    let deduped = dedup.dedup_attachments(all_attachments);
    budget.filter_within_budget(deduped)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Slice content by 1-based line offset and line limit.
fn slice_content(content: &str, offset: Option<i32>, limit: Option<i32>) -> (String, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = offset
        .map(|o| (o.max(1) - 1) as usize)
        .unwrap_or(0)
        .min(total_lines);
    let count = limit
        .map(|l| l.max(0) as usize)
        .unwrap_or(total_lines - start);
    let end = (start + count).min(total_lines);

    let sliced = lines[start..end].join("\n");
    (sliced, total_lines)
}

/// Truncate a string to at most `max_bytes` while respecting UTF-8 char
/// boundaries.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

fn is_image_path(path: &Path) -> bool {
    let image_extensions = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            image_extensions.iter().any(|ie| *ie == lower)
        })
}

/// Simple base64 encoder (standard alphabet, no padding omission).
/// Avoids adding an external `base64` crate dependency for this single use.
fn encode_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn guess_media_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png".to_owned(),
        Some("jpg" | "jpeg") => "image/jpeg".to_owned(),
        Some("gif") => "image/gif".to_owned(),
        Some("webp") => "image/webp".to_owned(),
        Some("bmp") => "image/bmp".to_owned(),
        Some("svg") => "image/svg+xml".to_owned(),
        _ => "application/octet-stream".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "attachment.test.rs"]
mod tests;
