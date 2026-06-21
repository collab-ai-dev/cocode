//! Reactive compaction — mid-turn compaction when prompt is too long.
//!
//! Three routines live here:
//! - [`should_reactive_compact`] / [`calculate_drop_target`] — the
//!   threshold-based fallback that lets callers proactively decide to
//!   compact when usage is near the limit (95% of effective window).
//! - [`peel_head_for_ptl_retry`] — drops oldest API-round groups (head of
//!   the slice) until enough tokens are freed. The caller can re-issue the
//!   API call with the survivor list directly, no LLM summarization required.
//!   Peels from the array head (chronologically oldest end). For a true
//!   summarized recovery, use [`crate::compact_conversation`] with the
//!   peeled message slice.
//! - [`api_microcompact`] — light tool-result stripping for cases where
//!   summarization is overkill.
//!
//! The [`ReactiveCompactState`] circuit breaker tracks consecutive
//! failures and disables reactive compaction after repeated failures to
//! avoid wasting API calls (`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`).

use coco_config::AutoCompactConfig;
use coco_messages::Message;

use crate::types::CLEARED_TOOL_RESULT_MESSAGE;
use crate::types::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES;

/// Reactive compact configuration.
#[derive(Debug, Clone)]
pub struct ReactiveCompactConfig {
    /// Context window size in tokens.
    pub context_window: i64,
    /// Max output tokens for the model (used in effective window calculation).
    pub max_output_tokens: i64,
    /// Trigger threshold as percentage of effective window (higher than auto-compact).
    /// Default 95% — reactive triggers closer to the limit than auto-compact (~87%).
    pub trigger_threshold_pct: i32,
    /// Number of recent rounds to preserve.
    pub keep_recent_rounds: usize,
}

impl Default for ReactiveCompactConfig {
    fn default() -> Self {
        Self {
            context_window: 200_000,
            max_output_tokens: 16_384,
            trigger_threshold_pct: 95,
            keep_recent_rounds: 2,
        }
    }
}

/// Mutable state tracking reactive compaction attempts and circuit breaker.
///
/// After `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` (3) consecutive failures,
/// the circuit breaker trips and `should_attempt_reactive_compact` returns
/// `false` until `reset()` is called.
#[derive(Debug, Clone, Default)]
pub struct ReactiveCompactState {
    /// Consecutive failure count.
    failure_count: i32,
    /// Timestamp (ms) of the last compaction attempt.
    last_attempt_ms: i64,
    /// Set when the last reactive attempt only QUEUED a server-side
    /// `context_management` edit (Anthropic) instead of mutating history. If we
    /// re-enter reactive compaction with this set, the cache-preserving retry
    /// did not resolve the overflow, so the caller falls back to client-side
    /// truncation (which frees real local tokens) instead of queue-looping.
    pending_server_side: bool,
}

impl ReactiveCompactState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether reactive compaction should be attempted.
    pub fn should_attempt_reactive_compact(&self) -> bool {
        self.failure_count < MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES
    }

    /// Record a compaction failure. Increments the consecutive failure count.
    pub fn record_failure(&mut self, timestamp_ms: i64) {
        self.failure_count += 1;
        self.last_attempt_ms = timestamp_ms;
    }

    /// Record a compaction success. Resets the failure count to zero.
    pub fn record_success(&mut self, timestamp_ms: i64) {
        self.failure_count = 0;
        self.last_attempt_ms = timestamp_ms;
    }

    /// Mark that the last reactive attempt queued a server-side edit (freed no
    /// local tokens). Read+cleared by [`Self::take_pending_server_side`].
    pub fn set_pending_server_side(&mut self) {
        self.pending_server_side = true;
    }

    /// Read and clear the server-side-pending flag. `true` means the previous
    /// attempt was a server-side queue that has not been confirmed resolved.
    pub fn take_pending_server_side(&mut self) -> bool {
        std::mem::take(&mut self.pending_server_side)
    }

    /// Reset the circuit breaker, re-enabling reactive compaction.
    pub fn reset(&mut self) {
        self.failure_count = 0;
        self.last_attempt_ms = 0;
        self.pending_server_side = false;
    }

    /// Current consecutive failure count.
    pub fn failure_count(&self) -> i32 {
        self.failure_count
    }

    /// Timestamp of the last attempt (0 if none).
    pub fn last_attempt_ms(&self) -> i64 {
        self.last_attempt_ms
    }
}

/// Check if reactive compaction should trigger.
///
/// Uses a HIGHER threshold than auto-compact (95% vs ~87% of effective window).
/// Reactive compact is a fallback when auto-compact missed or wasn't enough.
///
/// `auto_cfg` carries any `CLAUDE_CODE_AUTO_COMPACT_WINDOW` override that
/// also constrains reactive's effective window — they share the same
/// "what the user told us the window is" view.
#[must_use]
pub fn should_reactive_compact(
    estimated_tokens: i64,
    config: &ReactiveCompactConfig,
    auto_cfg: &AutoCompactConfig,
) -> bool {
    if config.context_window <= 0 {
        return false;
    }
    let effective = crate::auto_trigger::effective_context_window(
        config.context_window,
        config.max_output_tokens,
        auto_cfg,
    );
    let threshold = effective * config.trigger_threshold_pct as i64 / 100;
    estimated_tokens >= threshold
}

/// Determine how many tokens to drop to get below the threshold.
/// Target 70% of effective context window after compaction.
#[must_use]
pub fn calculate_drop_target(
    current_tokens: i64,
    config: &ReactiveCompactConfig,
    auto_cfg: &AutoCompactConfig,
) -> i64 {
    let effective = crate::auto_trigger::effective_context_window(
        config.context_window,
        config.max_output_tokens,
        auto_cfg,
    );
    let target = effective * 70 / 100;
    (current_tokens - target).max(0)
}

/// Peel oldest API-round groups (head of the slice) until at least
/// `tokens_to_free` tokens are freed. Returns `Some(remaining)` on success,
/// `None` when only one group (or fewer) survives — caller should escalate
/// to full summarization.
///
/// Unlike [`crate::truncate_head_for_ptl_retry`] (which feeds the summarizer
/// with the dropped portion), this returns the surviving slice **directly**
/// so the caller can resend the API call without going through the LLM
/// summarizer.
#[must_use]
pub fn peel_head_for_ptl_retry(
    messages: &[std::sync::Arc<Message>],
    tokens_to_free: i64,
) -> Option<Vec<std::sync::Arc<Message>>> {
    // `group_messages_by_api_round` is generic over `Borrow<Message>`, so
    // we can group the Arc-vec directly without materializing.
    let groups = crate::grouping::group_messages_by_api_round(messages);
    if groups.len() < 2 {
        return None;
    }
    let mut acc: i64 = 0;
    let mut drop_count = 0;
    for g in &groups {
        // `estimate_tokens` is also generic — feed it the &[&Message] slice
        // directly, no clone.
        acc += coco_messages::estimate_tokens_for_messages(g.as_slice());
        drop_count += 1;
        if acc >= tokens_to_free {
            break;
        }
    }
    let drop_count = drop_count.min(groups.len() - 1);
    if drop_count < 1 {
        return None;
    }
    // Survivors: count the messages BEFORE the drop boundary so we can
    // index back into the Arc-vec and share each retained Arc with the
    // caller. `groups` was derived in order, so we can recover the index
    // by summing prefix group lengths.
    let prefix_len: usize = groups[..drop_count].iter().map(Vec::len).sum();
    Some(messages[prefix_len..].to_vec())
}

/// API-level micro-compaction — trim tool results from oldest messages.
///
/// Lightweight compaction that clears old tool result content while
/// keeping the structure intact. **This breaks the prompt cache** because
/// it mutates messages in place; for the cache-preserving server-side
/// variant build a config via
/// [`crate::api_compact::get_api_context_management`].
pub fn api_microcompact(messages: &mut [Message], tokens_to_free: i64) {
    tracing::debug!(
        tokens_to_free,
        message_count = messages.len(),
        "api_microcompact begin (reactive)"
    );
    let mut freed = 0i64;
    let mut cleared: i32 = 0;
    for msg in messages.iter_mut() {
        if freed >= tokens_to_free {
            break;
        }
        if let Message::ToolResult(tr) = msg {
            let est = coco_messages::estimate_tool_result_message_tokens(tr);
            if est > 50 {
                tr.message = coco_messages::LlmMessage::Tool {
                    content: vec![coco_messages::ToolContent::ToolResult(
                        coco_messages::ToolResultContent {
                            tool_call_id: tr.tool_use_id.clone(),
                            tool_name: String::new(),
                            output: coco_llm_types::ToolResultContent::text(
                                CLEARED_TOOL_RESULT_MESSAGE,
                            ),
                            is_error: false,
                            provider_metadata: None,
                        },
                    )],
                    provider_options: None,
                };
                cleared += 1;
                freed += est;
            }
        }
    }
    tracing::info!(
        cleared,
        freed_tokens = freed,
        target_tokens = tokens_to_free,
        "api_microcompact done (reactive)"
    );
}

#[cfg(test)]
#[path = "reactive.test.rs"]
mod tests;
