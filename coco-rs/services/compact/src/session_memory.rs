//! Session memory compaction: use session memory as a compact summary
//! instead of calling the LLM to re-summarize.
//!
//! When session memory has been extracted (by the memory extraction pipeline),
//! compaction can use it directly as the summary, avoiding a costly LLM call.
//! This module selects which messages to keep, merges similar memories, and
//! produces a `CompactResult`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::SystemMessage;
use coco_types::CompactTrigger;

use crate::compact::annotate_boundary_with_preserved_segment;
use crate::types::CompactError;
use crate::types::CompactResult;
use crate::types::extract_discovered_tool_names;

/// Configuration for session memory compaction thresholds.
#[derive(Debug, Clone)]
pub struct SessionMemoryCompactConfig {
    /// Minimum tokens to preserve after compaction.
    pub min_tokens: i64,
    /// Minimum number of messages with text blocks to keep.
    pub min_text_block_messages: i32,
    /// Maximum tokens to preserve after compaction (hard cap).
    pub max_tokens: i64,
    /// Optional auto-compact threshold guard. When set, the compaction
    /// returns `None` if the resulting context would still be ≥ this
    /// value, forcing the caller to fall back to LLM summarization.
    pub auto_compact_threshold: Option<i64>,
    /// Optional max length (chars) for the inlined session memory
    /// content; longer content is truncated and a pointer to the
    /// memory file is appended (TS `truncateSessionMemoryForCompact`).
    pub max_summary_chars: Option<usize>,
    /// Path to the session memory file, used in the truncation pointer.
    pub session_memory_path: Option<String>,
}

impl Default for SessionMemoryCompactConfig {
    fn default() -> Self {
        Self {
            min_tokens: 10_000,
            min_text_block_messages: 5,
            max_tokens: 40_000,
            auto_compact_threshold: None,
            max_summary_chars: None,
            session_memory_path: None,
        }
    }
}

/// Truncation marker appended when `max_summary_chars` clips session memory.
const SESSION_MEMORY_TRUNCATION_MARKER: &str =
    "\n\n[Session memory truncated for length. Read the file directly for full content.]";

/// Perform session memory compaction: replace old messages with the session
/// memory content as a summary, keeping only recent messages.
///
/// `last_summarized_message_id` is the uuid of the last assistant message
/// already covered by `session_memory` — `Some(uuid)` means extraction has
/// run and the anchor is known; `None` means resumed session (memory exists
/// from disk but boundary is unknown). When the uuid doesn't appear in
/// `messages`, returns `Ok(None)` so the caller falls back to LLM-based
/// compaction.
///
/// Returns `None` when:
/// - Session memory is empty / template-only;
/// - The kept-tail anchor is unrecoverable;
/// - The post-compact token count is still ≥ `auto_compact_threshold`
///   (caller should fall back to LLM-based compaction).
pub fn compact_session_memory(
    messages: &[Arc<Message>],
    session_memory: &str,
    last_summarized_message_id: Option<uuid::Uuid>,
    config: &SessionMemoryCompactConfig,
) -> Result<Option<CompactResult>, CompactError> {
    let trimmed = session_memory.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if is_session_memory_template_only(trimmed) {
        return Ok(None);
    }

    // Resolve the last-summarized boundary index:
    //   - Some + found  → start after that index.
    //   - Some + not found → bail (caller falls back to LLM).
    //   - None → resumed session: pretend boundary is just before tail
    //     so `calculate_messages_to_keep_index` initially keeps no
    //     messages and only expands enough to hit minimums.
    let last_summarized_index: Option<usize> = match last_summarized_message_id {
        Some(target) => {
            let found = messages.iter().position(|m| m.uuid() == Some(&target));
            if found.is_none() {
                return Ok(None);
            }
            found
        }
        None => {
            if messages.is_empty() {
                None
            } else {
                Some(messages.len() - 1)
            }
        }
    };

    let start_index = calculate_messages_to_keep_index(messages, last_summarized_index, config);
    let adjusted_index = adjust_index_to_preserve_api_invariants(messages, start_index);

    // Filter out stale compact-boundary messages from the kept tail. Otherwise
    // a re-compact would re-introduce the old boundary and the loader's
    // tail→head walk could prune the new summary. Arc-share kept entries
    // (refcount bump, zero `Message::clone`).
    let messages_to_keep: Vec<Arc<Message>> = messages[adjusted_index..]
        .iter()
        .filter(|arc| {
            !matches!(
                arc.as_ref(),
                Message::System(SystemMessage::CompactBoundary(_))
            )
        })
        .cloned()
        .collect();

    // Truncate session memory text if a cap is configured.
    let (memory_for_summary, was_truncated) = match config.max_summary_chars {
        Some(cap) if trimmed.len() > cap => {
            let cut = cap.saturating_sub(SESSION_MEMORY_TRUNCATION_MARKER.len());
            let mut truncated = trimmed[..cut].to_string();
            truncated.push_str(SESSION_MEMORY_TRUNCATION_MARKER);
            (truncated, true)
        }
        _ => (trimmed.to_string(), false),
    };

    let mut summary = crate::prompt::get_compact_user_summary_message(
        &memory_for_summary,
        /*suppress_follow_up*/ true,
        /*transcript_path*/ None,
        /*recent_messages_preserved*/ true,
    );
    if was_truncated && let Some(path) = &config.session_memory_path {
        summary.push_str(&format!(
            "\n\nSome session memory sections were truncated for length. \
             The full session memory can be viewed at: {path}"
        ));
    }

    let summary_message = Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(&summary),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: true,
        is_virtual: false,
        is_compact_summary: true,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    });
    let summary_uuid = summary_message
        .uuid()
        .copied()
        .unwrap_or_else(uuid::Uuid::nil);

    // Use the non-conservative estimator to stay consistent with
    // `compact_conversation` — both produce the same scale of token
    // counts so callers can compare pre/post values across paths.
    let pre_tokens = coco_messages::estimate_tokens_for_messages(messages);
    let post_tokens = coco_messages::estimate_tokens_for_messages(&messages_to_keep)
        + coco_messages::estimate_text_tokens(&summary);

    // Threshold guard: if compaction wouldn't actually shrink below the
    // autocompact line, skip and let LLM compact handle it.
    if let Some(threshold) = config.auto_compact_threshold
        && post_tokens >= threshold
    {
        return Ok(None);
    }

    let messages_summarized = (messages.len() - messages_to_keep.len()) as i32;
    let mut boundary_struct = coco_messages::SystemCompactBoundaryMessage {
        uuid: uuid::Uuid::new_v4(),
        tokens_before: pre_tokens,
        tokens_after: post_tokens,
        trigger: CompactTrigger::SessionMemory,
        user_context: None,
        messages_summarized: Some(messages_summarized),
        pre_compact_discovered_tools: extract_discovered_tool_names(messages)
            .into_iter()
            .collect(),
        preserved_segment: None,
    };
    // Suffix-preserving: anchor is the summary's uuid.
    annotate_boundary_with_preserved_segment(&mut boundary_struct, summary_uuid, &messages_to_keep);

    Ok(Some(CompactResult {
        boundary_marker: Message::System(SystemMessage::CompactBoundary(boundary_struct)),
        raw_summary: Some(memory_for_summary),
        summary_messages: vec![summary_message],
        attachments: vec![],
        messages_to_keep,
        hook_results: vec![],
        user_display_message: None,
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
        true_post_compact_tokens: post_tokens,
        is_recompaction: false,
        trigger: CompactTrigger::SessionMemory,
    }))
}

/// Inputs to [`should_extract_memory`] — caller-supplied counters
/// describing the current turn's relationship to the last-extracted state.
#[derive(Debug, Clone, Copy)]
pub struct SessionMemoryExtractionInputs {
    /// Total estimated tokens in the conversation right now.
    pub current_tokens: i64,
    /// Tokens at the time of the last successful extraction.
    /// `0` when no extraction has ever run.
    pub tokens_at_last_extract: i64,
    /// Number of tool calls in the most recent turn.
    pub tool_calls_in_last_turn: i32,
}

/// Thresholds used by [`should_extract_memory`]: `minimum_message_tokens_to_init
/// = 10_000`, `minimum_tokens_between_update = 5_000`, `min_tools_for_update = 3`.
/// Kept as a struct so settings.json overrides can tune without touching the algorithm.
#[derive(Debug, Clone, Copy)]
pub struct SessionMemoryExtractionThresholds {
    pub minimum_message_tokens_to_init: i64,
    pub minimum_tokens_between_update: i64,
    pub min_tools_for_update: i32,
}

impl Default for SessionMemoryExtractionThresholds {
    fn default() -> Self {
        Self {
            minimum_message_tokens_to_init: 10_000,
            minimum_tokens_between_update: 5_000,
            min_tools_for_update: 3,
        }
    }
}

/// Pure decision: should the session-memory extractor run this turn?
///
///  - **Init** (no prior extract): require ≥ `minimum_message_tokens_to_init`.
///  - **Update** (prior extract exists): require token-delta ≥
///    `minimum_tokens_between_update` AND
///    (`tool_calls_in_last_turn ≥ min_tools_for_update` OR
///    `tool_calls_in_last_turn == 0`)
///    The second clause is "tool-bursty turns get extracted; idle
///    turns also get extracted to capture the resolution".
///
/// This is a *pure* decision — the actual extraction (forked agent
/// LLM call + file write) is owned upstream by `services/SessionMemory/`.
#[must_use]
pub fn should_extract_memory(
    inputs: SessionMemoryExtractionInputs,
    thresholds: &SessionMemoryExtractionThresholds,
) -> bool {
    let SessionMemoryExtractionInputs {
        current_tokens,
        tokens_at_last_extract,
        tool_calls_in_last_turn,
    } = inputs;

    if tokens_at_last_extract <= 0 {
        // Init path — first extraction.
        return current_tokens >= thresholds.minimum_message_tokens_to_init;
    }
    let delta = current_tokens - tokens_at_last_extract;
    if delta < thresholds.minimum_tokens_between_update {
        return false;
    }
    tool_calls_in_last_turn >= thresholds.min_tools_for_update || tool_calls_in_last_turn == 0
}

/// Select which memories to compact when the memory directory grows too large.
///
/// Picks the oldest/least-recently-referenced entries first, based on
/// `last_used` timestamps. Returns entry names sorted by compaction priority
/// (first = most eligible for removal/merging).
pub fn select_memories_for_compaction(
    entries: &[(String, MemoryMetadata)],
    max_to_keep: usize,
) -> Vec<String> {
    if entries.len() <= max_to_keep {
        return Vec::new();
    }

    let mut scored: Vec<(f64, &str)> = entries
        .iter()
        .map(|(name, meta)| {
            // Score: lower = more eligible for compaction.
            // Staleness (days since last use) dominates, with a small weight
            // for access count so frequently-used old entries survive longer.
            let staleness_days = meta.staleness_days as f64;
            let frequency_bonus = (meta.access_count as f64).ln().max(0.0) * 5.0;
            let score = staleness_days - frequency_bonus;
            (score, name.as_str())
        })
        .collect();

    // Sort descending by score (highest staleness first)
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let to_remove = entries.len() - max_to_keep;
    scored
        .into_iter()
        .take(to_remove)
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Metadata used for compaction scoring.
#[derive(Debug, Clone)]
pub struct MemoryMetadata {
    /// Days since this memory was last accessed/referenced.
    pub staleness_days: i32,
    /// Number of times this memory has been accessed.
    pub access_count: i32,
}

/// Merge similar/overlapping memories into consolidated entries.
///
/// Groups memories by their name prefix (the part before the last `_` or `-`)
/// and merges entries within each group by concatenating their content with
/// deduplication of identical lines.
pub fn merge_similar_memories(memories: &[(String, String)]) -> Vec<(String, String)> {
    // Group by name prefix
    let mut groups: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for (name, content) in memories {
        let prefix = extract_name_prefix(name);
        groups
            .entry(prefix.to_string())
            .or_default()
            .push((name, content));
    }

    let mut merged = Vec::new();
    for (prefix, group) in &groups {
        if group.len() <= 1 {
            for (name, content) in group {
                merged.push(((*name).to_string(), (*content).to_string()));
            }
            continue;
        }

        // Merge: use the prefix as the canonical name, deduplicate lines
        let mut seen_lines = std::collections::HashSet::new();
        let mut merged_content = String::new();
        for (_, content) in group {
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && seen_lines.insert(trimmed.to_string()) {
                    if !merged_content.is_empty() {
                        merged_content.push('\n');
                    }
                    merged_content.push_str(line);
                }
            }
        }
        merged.push((prefix.clone(), merged_content));
    }

    merged.sort_by(|a, b| a.0.cmp(&b.0));
    merged
}

// ── Internal helpers ────────────────────────────────────────────────

/// Calculate the starting index for messages to keep after compaction.
///
/// Starts from `last_summarized_index + 1` and expands backwards until we
/// meet both minimum thresholds, or hit the max cap, or hit the floor at
/// the previous CompactBoundary.
///
/// Floor rationale: the preserved-segment chain has a disk discontinuity
/// at the previous boundary (att[0]→summary shortcut from dedup-skip);
/// expanding past it would let the loader's tail→head walk bypass inner
/// preserved messages and prune them.
fn calculate_messages_to_keep_index(
    messages: &[Arc<Message>],
    last_summarized_index: Option<usize>,
    config: &SessionMemoryCompactConfig,
) -> usize {
    if messages.is_empty() {
        return 0;
    }

    // Start from the message after the anchor; if no anchor, start at len
    // (no kept messages initially) so the expansion loop fills exactly to
    // the configured minimums.
    let mut start_index = match last_summarized_index {
        Some(idx) => (idx + 1).min(messages.len()),
        None => messages.len(),
    };

    // Floor at the last CompactBoundary so we never expand past a prior
    // compaction's archive line.
    let floor = messages
        .iter()
        .rposition(|m| {
            matches!(
                m.as_ref(),
                Message::System(SystemMessage::CompactBoundary(_))
            )
        })
        .map(|i| i + 1)
        .unwrap_or(0);

    // Initial accounting from start_index..len.
    let mut total_tokens: i64 = 0;
    let mut text_block_count: i32 = 0;
    for msg in &messages[start_index..] {
        total_tokens =
            total_tokens.saturating_add(coco_messages::estimate_message_tokens(msg.as_ref()));
        if has_text_blocks(msg.as_ref()) {
            text_block_count += 1;
        }
    }

    // Already over either bound — no expansion needed.
    if total_tokens >= config.max_tokens
        || (total_tokens >= config.min_tokens && text_block_count >= config.min_text_block_messages)
    {
        return start_index;
    }

    // Expand backwards until we meet both minimums, hit the max cap, or
    // hit the boundary floor.
    while start_index > floor {
        let i = start_index - 1;
        let msg_tokens = coco_messages::estimate_message_tokens(messages[i].as_ref());
        total_tokens = total_tokens.saturating_add(msg_tokens);
        if has_text_blocks(messages[i].as_ref()) {
            text_block_count += 1;
        }
        start_index = i;

        if total_tokens >= config.max_tokens {
            break;
        }
        if total_tokens >= config.min_tokens && text_block_count >= config.min_text_block_messages {
            break;
        }
    }

    start_index
}

/// Detect whether the on-disk session memory file contains only the
/// extractor's empty template (no actual content yet).
///
/// Heuristic: the template uses second-level headings (`## ...`) and a
/// known trailer; a file with no body lines under any heading is considered
/// empty. Also recognizes canonical placeholder strings ("No memories yet",
/// "(empty)").
pub(crate) fn is_session_memory_template_only(content: &str) -> bool {
    let normalized = content.trim();
    if normalized.is_empty() {
        return true;
    }
    // Reject obvious empty markers regardless of structure.
    let lower = normalized.to_lowercase();
    if lower.contains("no memories yet") || lower == "(empty)" {
        return true;
    }
    // A template-only file has only headings + blank lines and possibly
    // bullet placeholders like "- _none yet_".
    let mut has_content_line = false;
    for line in normalized.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('#') {
            continue;
        }
        let t_lower = t.to_lowercase();
        if t_lower.contains("none yet") || t_lower.contains("no entries") {
            continue;
        }
        has_content_line = true;
        break;
    }
    !has_content_line
}

/// Adjust the keep index to preserve tool_use/tool_result pairs **and**
/// thinking blocks that share an `AssistantMessage.uuid` with kept rounds.
///
/// Step 1 walks backwards including assistant messages that own tool_use
/// blocks referenced by tool_results in the kept range. Step 2 walks
/// backwards including assistant messages whose `uuid` matches a kept
/// assistant — those messages may contain thinking blocks that the API
/// requires for tool-call validity on thinking-enabled models.
pub fn adjust_index_to_preserve_api_invariants(
    messages: &[Arc<Message>],
    start_index: usize,
) -> usize {
    if start_index == 0 || start_index >= messages.len() {
        return start_index;
    }

    let mut adjusted = start_index;

    // Step 1: tool_result → owning tool_use assistant messages.
    let mut needed_tool_use_ids: HashSet<String> = messages[adjusted..]
        .iter()
        .filter_map(|arc| match arc.as_ref() {
            Message::ToolResult(tr) => Some(tr.tool_use_id.clone()),
            _ => None,
        })
        .collect();
    // Drop ids whose tool_use is already in the kept range.
    for id in collect_tool_use_ids(&messages[adjusted..]) {
        needed_tool_use_ids.remove(&id);
    }

    let mut i = adjusted;
    while i > 0 && !needed_tool_use_ids.is_empty() {
        i -= 1;
        let Message::Assistant(asst) = messages[i].as_ref() else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &asst.message else {
            continue;
        };
        let mut matched = false;
        for part in content {
            if let AssistantContent::ToolCall(tc) = part
                && needed_tool_use_ids.remove(&tc.tool_call_id)
            {
                matched = true;
            }
        }
        if matched {
            adjusted = i;
        }
    }

    // Step 2: include assistant messages sharing UUID with kept assistants
    // (thinking-block reconstitution).
    let kept_uuids: HashSet<uuid::Uuid> = messages[adjusted..]
        .iter()
        .filter_map(|arc| match arc.as_ref() {
            Message::Assistant(a) => Some(a.uuid),
            _ => None,
        })
        .collect();

    let mut i = adjusted;
    while i > 0 {
        i -= 1;
        let Message::Assistant(asst) = messages[i].as_ref() else {
            continue;
        };
        if kept_uuids.contains(&asst.uuid) {
            adjusted = i;
        }
    }

    adjusted
}

fn collect_tool_use_ids(messages: &[Arc<Message>]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for arc in messages {
        let Message::Assistant(asst) = arc.as_ref() else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &asst.message else {
            continue;
        };
        for part in content {
            if let AssistantContent::ToolCall(tc) = part {
                ids.insert(tc.tool_call_id.clone());
            }
        }
    }
    ids
}

/// Check if a message contains meaningful text content.
pub fn has_text_blocks(message: &Message) -> bool {
    match message {
        Message::User(u) => match &u.message {
            coco_messages::LlmMessage::User { content, .. } => content
                .iter()
                .any(|c| matches!(c, coco_messages::UserContent::Text(_))),
            _ => false,
        },
        Message::Assistant(a) => match &a.message {
            coco_messages::LlmMessage::Assistant { content, .. } => content
                .iter()
                .any(|c| matches!(c, coco_messages::AssistantContent::Text(_))),
            _ => false,
        },
        _ => false,
    }
}

/// Extract the prefix from a memory name (before the last `_` or `-`).
fn extract_name_prefix(name: &str) -> &str {
    let base = name.strip_suffix(".md").unwrap_or(name);
    if let Some(pos) = base.rfind(['_', '-']) {
        &base[..pos]
    } else {
        base
    }
}

#[cfg(test)]
#[path = "session_memory.test.rs"]
mod tests;
