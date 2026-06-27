//! Core compaction types.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_messages::AssistantContent;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::CompactTrigger;
use serde::Deserialize;
use serde::Serialize;

// ── Auto-compact constants ───────────────────────────────────────────

/// Buffer below effective context window for auto-compact trigger.
pub const AUTOCOMPACT_BUFFER_TOKENS: i64 = 13_000;

/// Token buffer before warning threshold.
pub const WARNING_THRESHOLD_BUFFER_TOKENS: i64 = 20_000;

/// Token buffer before error threshold.
pub const ERROR_THRESHOLD_BUFFER_TOKENS: i64 = 20_000;

/// Reserve for manual compact blocking limit.
pub const MANUAL_COMPACT_BUFFER_TOKENS: i64 = 3_000;

/// Max tokens reserved for the compact summary output.
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: i64 = 20_000;

/// Circuit breaker: stop after this many consecutive auto-compact failures.
pub const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: i32 = 3;

/// Max prompt-too-long retries before giving up.
pub const MAX_PTL_RETRIES: i32 = 3;

/// Max streaming retries for the summary LLM call.
pub const MAX_COMPACT_STREAMING_RETRIES: i32 = 2;

// ── Post-compact attachment budgets ─────────────────────────────────

/// Maximum recently-read files to re-inject after compaction.
pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;

/// Total token budget for post-compact file attachments.
pub const POST_COMPACT_TOKEN_BUDGET: i64 = 50_000;

/// Per-file token cap for post-compact restore.
pub const POST_COMPACT_MAX_TOKENS_PER_FILE: i64 = 5_000;

/// Per-skill token cap for post-compact restore.
pub const POST_COMPACT_MAX_TOKENS_PER_SKILL: i64 = 5_000;

/// Total token budget for all skill re-injections.
pub const POST_COMPACT_SKILLS_TOKEN_BUDGET: i64 = 25_000;

// ── Cleared-content markers ──────────────────────────────────────────

/// Placeholder inserted when old tool result content is cleared.
pub const CLEARED_TOOL_RESULT_MESSAGE: &str = "[Old tool result content cleared]";

/// Synthetic user message prepended on prompt-too-long retry.
pub const PTL_RETRY_MARKER: &str = "[earlier conversation truncated for compaction retry]";

// ── Result types ────────────────────────────────────────────────────

/// Which compaction flow is asking for a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactSummaryKind {
    Full,
    Partial,
}

/// Structured request sent to the caller-provided compact summarizer.
///
/// `messages` contains the conversation slice being summarized as real
/// role-preserving messages. `context_messages` contains the structured
/// conversation to send to the summarizer. They differ for partial
/// "from/newest" compaction, where the earlier context is preserved on
/// the first attempt and only the selected tail is summarized.
///
/// `summary_request` is the instruction prompt appended by the query layer
/// as a final user message or fork prompt.
#[derive(Debug, Clone)]
pub struct CompactSummaryAttempt {
    pub messages: Vec<Arc<Message>>,
    pub context_messages: Vec<Arc<Message>>,
    pub summary_request: String,
    pub prompt_kind: CompactSummaryKind,
    pub pre_compact_tokens: i64,
    pub max_summary_tokens: i64,
}

/// Successful compact-summary response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummaryResponse {
    pub summary: String,
}

/// Outcome for query-level compaction attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactOutcome {
    Applied,
    Skipped,
    Failed,
}

/// Result of a full compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResult {
    /// Compact boundary marker (system message).
    pub boundary_marker: Message,
    /// Raw compact summary returned by the summarizer before wrapping it
    /// in continuation boilerplate. Used by PostCompact hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_summary: Option<String>,
    /// Summary messages (the LLM-generated or session-memory summary).
    pub summary_messages: Vec<Message>,
    /// Post-compact attachments (file restore, plan, skills, tools, MCP).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentMessage>,
    /// Messages preserved (not compacted). `Vec<Arc<Message>>` so
    /// producers that already hold Arc-shared messages (the engine's
    /// `Vec<Arc<Message>>` history snapshot) can Arc-clone instead of
    /// deep-cloning when populating this field, and downstream
    /// consumers (post-compact attachment builders, observers) can
    /// continue Arc-sharing rather than materializing fresh
    /// `Vec<Message>` for every read.
    pub messages_to_keep: Vec<Arc<Message>>,
    /// Hook result messages from pre/post-compact hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hook_results: Vec<Message>,
    /// User-facing status message (shown in TUI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_display_message: Option<String>,
    /// Token count before compaction.
    pub pre_compact_tokens: i64,
    /// Token count after compaction (API call total).
    pub post_compact_tokens: i64,
    /// True post-compact token count (message payload estimate).
    #[serde(default)]
    pub true_post_compact_tokens: i64,
    /// Whether this was a re-compaction.
    #[serde(default)]
    pub is_recompaction: bool,
    /// How compaction was triggered.
    #[serde(default = "default_trigger")]
    pub trigger: CompactTrigger,
}

fn default_trigger() -> CompactTrigger {
    CompactTrigger::Auto
}

/// Info about a re-compaction scenario.
///
/// Threaded through `CompactRunOptions` so compact results preserve the
/// chain state (H1/H2/H3/H5 disambiguation).
#[derive(Debug, Clone)]
pub struct RecompactionInfo {
    pub is_recompaction: bool,
    pub turns_since_previous: i32,
    pub auto_compact_threshold: i64,
}

/// Result of a micro-compaction (lightweight tool result clearing).
#[derive(Debug, Clone, Default)]
pub struct MicrocompactResult {
    /// Number of tool results cleared.
    pub messages_cleared: i32,
    /// Estimated tokens saved.
    pub tokens_saved_estimate: i64,
    /// Whether triggered by time-based config.
    pub was_time_triggered: bool,
}

/// Token warning state for the TUI/query engine.
#[derive(Debug, Clone)]
pub struct TokenWarningState {
    /// Percentage of context window remaining.
    pub percent_left: i32,
    /// Whether above the warning threshold.
    pub is_above_warning_threshold: bool,
    /// Whether above the error threshold.
    pub is_above_error_threshold: bool,
    /// Whether above the auto-compact threshold.
    pub is_above_auto_compact_threshold: bool,
    /// Whether at the blocking limit (no more input accepted).
    pub is_at_blocking_limit: bool,
}

/// Strategy for API-native context editing.
///
/// Maps to the Anthropic `context_management.edits` payload, with each
/// variant mapping to one wire `type`.
#[derive(Debug, Clone)]
pub enum ContextEditStrategy {
    /// `clear_tool_uses_20250919`: clear tool result content / tool inputs
    /// for older turns, keeping the most-recent N matching the [`ToolUseKeep`].
    ClearToolUses {
        /// Trigger threshold (token count).
        trigger: Option<i64>,
        /// How many recent tool uses to retain. `None` defers to the model
        /// policy default.
        keep_recent: Option<ToolUseKeep>,
        /// Minimum number of input tokens the API must free during this
        /// edit. Set to `triggerThreshold - keepTarget` so the server
        /// clears enough even when its default would clear less. Without
        /// this, the `keep_target` config is informational only.
        clear_at_least: Option<i64>,
        /// Which tool inputs to clear.
        clear_inputs: ClearToolInputs,
        /// Builtin tools excluded from clearing.
        exclude_tools: Vec<coco_types::ToolName>,
        /// Additional non-builtin tool identifiers (MCP / custom) to exclude.
        exclude_tool_strs: Vec<String>,
    },
    /// `clear_thinking_20251015`: drop reasoning blocks from older turns.
    ClearThinking {
        /// Retention policy for thinking blocks.
        keep: ThinkingKeep,
    },
}

/// Retention policy for tool uses under `clear_tool_uses_20250919`.
///
/// Wire shape: `{type: 'tool_uses', value: number}`. Only the numeric
/// variant is modeled — the wire format has no symbolic "all" for this field.
#[derive(Debug, Clone, Copy)]
pub struct ToolUseKeep {
    /// Number of recent tool uses to keep.
    pub value: i32,
}

/// Retention policy for thinking blocks under `clear_thinking_20251015`.
///
/// Wire shape: `{type: 'thinking_turns', value: number} | 'all'`. The
/// `'all'` literal is a distinct variant so callers cannot accidentally
/// smuggle a sentinel value through the numeric path.
#[derive(Debug, Clone, Copy)]
pub enum ThinkingKeep {
    /// Keep all thinking blocks (wire: `'all'`).
    All,
    /// Keep the last N turns (wire: `{type: 'thinking_turns', value: N}`).
    Recent { turns: i32 },
}

/// Which tool inputs to clear during context editing.
#[derive(Debug, Clone)]
pub enum ClearToolInputs {
    /// Clear inputs for all eligible tools (wire: `clear_tool_inputs: true`).
    All,
    /// Clear inputs only for the listed builtin tools.
    SpecificTools(Vec<coco_types::ToolName>),
    /// Don't clear inputs (wire: `clear_tool_inputs: false` / omitted).
    None,
}

/// State store tracking whether the autocompact warning should be suppressed.
///
/// Set to true after a successful compaction so the TUI doesn't redundantly
/// warn the user about the (now-stale) pre-compact token count. Cleared at
/// the start of each new compaction attempt or when the next API response
/// gives an accurate token count.
#[derive(Debug, Default)]
pub struct CompactWarningState {
    suppressed: AtomicBool,
}

impl CompactWarningState {
    pub const fn new() -> Self {
        Self {
            suppressed: AtomicBool::new(false),
        }
    }

    /// Suppress the warning. Call after successful compaction.
    pub fn suppress(&self) {
        self.suppressed.store(true, Ordering::Release);
    }

    /// Clear suppression. Call at the start of a new compact attempt.
    pub fn clear(&self) {
        self.suppressed.store(false, Ordering::Release);
    }

    /// Whether the warning is currently suppressed.
    pub fn is_suppressed(&self) -> bool {
        self.suppressed.load(Ordering::Acquire)
    }
}

/// Collect names of tools that were "discovered" via ToolSearch in the
/// conversation (i.e. deferred-load tools that were materialized).
///
/// Returned set is sorted (BTreeSet) so the boundary marker stores them
/// deterministically.
pub fn extract_discovered_tool_names<M: std::borrow::Borrow<Message>>(
    messages: &[M],
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for m in messages {
        let msg: &Message = m.borrow();
        let Message::Assistant(asst) = msg else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &asst.message else {
            continue;
        };
        for part in content {
            // ToolSearch produces tool_use blocks named "ToolSearch" whose
            // input lists discovered tools. Walk their `tools` field.
            if let AssistantContent::ToolCall(tc) = part
                && tc.tool_name == "ToolSearch"
                && let Some(arr) = tc.input.get("tools").and_then(|v| v.as_array())
            {
                for item in arr {
                    if let Some(name) = item.as_str() {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

/// Compaction errors.
#[coco_error::stack_trace_debug]
#[derive(snafu::Snafu)]
#[snafu(visibility(pub), module)]
pub enum CompactError {
    /// LLM summarization call failed.
    #[snafu(display("LLM call failed: {message}"))]
    LlmCallFailed {
        message: String,
        #[snafu(implicit)]
        location: coco_error::Location,
    },
    /// Token budget exceeded.
    #[snafu(display("token budget exceeded: {actual} > {limit}"))]
    TokenBudgetExceeded {
        actual: i64,
        limit: i64,
        #[snafu(implicit)]
        location: coco_error::Location,
    },
    /// Cancelled by user.
    #[snafu(display("compaction cancelled"))]
    Cancelled {
        #[snafu(implicit)]
        location: coco_error::Location,
    },
    /// Stream retry exhausted.
    #[snafu(display("stream retry exhausted after {attempts} attempts"))]
    StreamRetryExhausted {
        attempts: i32,
        #[snafu(implicit)]
        location: coco_error::Location,
    },
    /// Prompt too long for summarization.
    #[snafu(display("prompt too long: {message}"))]
    PromptTooLong {
        message: String,
        #[snafu(implicit)]
        location: coco_error::Location,
    },
}

impl coco_error::ErrorExt for CompactError {
    fn status_code(&self) -> coco_error::StatusCode {
        use coco_error::StatusCode;
        match self {
            Self::LlmCallFailed { .. } => StatusCode::ProviderError,
            Self::TokenBudgetExceeded { .. } | Self::PromptTooLong { .. } => {
                StatusCode::ContextWindowExceeded
            }
            Self::Cancelled { .. } => StatusCode::Cancelled,
            Self::StreamRetryExhausted { .. } => StatusCode::StreamError,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub use compact_error::*;
