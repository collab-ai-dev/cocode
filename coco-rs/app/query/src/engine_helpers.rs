//! Free helpers used by the agent loop. Extracted from `engine.rs` to keep
//! that file focused on the orchestration impl.
//!
//! Items split into five groups:
//! - Capacity-error detection + model-fallback announcer.
//! - Transcript rendering helpers (extractor input + streaming tool output).
//! - Delta builders for tool / MCP / agent listing reminders.
//! - User-input scraping for the per-turn reminder pipeline.
//! - Streaming tool-call buffer + progress throttle utilities.
//!
//! `engine.rs` selectively re-exports the items its tests reach via
//! `super::name` (e.g. `ProgressThrottle`, `is_capacity_error_message`,
//! `classify_progress_payload`). Other modules (`engine_turn_reminders`,
//! `engine_finalize_turn`) import directly from this module.

use coco_llm_types::ToolResultContent;
use coco_llm_types::UserContentPart;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::AgentStreamEvent;
use crate::CoreEvent;
use crate::ServerNotification;
use crate::emit::emit_protocol;
use crate::emit::emit_stream;
use crate::emit::emit_tui;
use crate::model_runtime::ModelFallbackReason;

/// Per-call buffer used while consuming `StreamEvent`s for a single turn.
/// `input_json` is appended from `ToolCallDelta` chunks and parsed on
/// `ToolCallEnd`. Buffers are keyed by the provider-assigned `tool_call_id`.
pub(crate) struct StreamingToolCallBuffer {
    pub(crate) tool_name: String,
    pub(crate) input_json: String,
    pub(crate) complete: bool,
}

/// Classify an error message as a transient capacity error.
///
/// Thin wrapper over [`coco_inference::InferenceError::classify_stream_message`]
/// — keeps the engine-helpers surface (`bool` return) ergonomic while
/// the keyword sniffing itself lives in the inference layer where the
/// rest of the error classification already does. Multi-provider boundary
/// rule (`CLAUDE.md`) forbids the engine from owning Anthropic-specific
/// keyword matches; this re-export satisfies the rule.
pub(crate) fn is_capacity_error_message(msg: &str) -> bool {
    matches!(
        coco_inference::InferenceError::classify_stream_message(msg),
        Some(
            coco_inference::InferenceError::Overloaded { .. }
                | coco_inference::InferenceError::RateLimited { .. }
        )
    )
}

/// Record a per-provider rate-limit observation onto
/// `ToolAppState.rate_limits`. Called from the engine's capacity-error
/// branch so subsequent post-turn forks (prompt-suggestion in particular)
/// can suppress when their target provider is throttled.
///
/// Matches the read site at `engine_finalize_turn::build_suggestion_context`,
/// which is keyed on `cache.provider` (set when the parent turn ran). The
/// `provider` parameter here MUST match the `provider` recorded on the
/// post-turn cache slot — both come from the active runtime snapshot, so
/// this is true by construction. Asserted in `prompt_suggestion.test.rs`'s
/// selectivity matrix.
///
/// Idempotent: a second 429 from the same provider replaces the entry
/// (last-write-wins). Selectivity is the read-side filter.
///
/// `retry_after_ms` translates to wall-clock `reset_at_ms` immediately
/// so the read site doesn't need monotonic clocks to evaluate freshness.
pub(crate) async fn record_rate_limit_observation(
    app_state: &Arc<RwLock<coco_types::ToolAppState>>,
    provider: &str,
    api: coco_types::ProviderApi,
    retry_after_ms: Option<i64>,
) {
    if provider.is_empty() {
        // Defensive: empty provider means we can't key the entry. Skip
        // rather than write an entry no one will read selectively.
        return;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let entry = coco_types::RateLimitEntry {
        api,
        // 429 / Overloaded both surface as Rejected — callers wanting
        // finer granularity can extend later (header-parsed warnings).
        status: coco_types::RateLimitStatus::Rejected,
        reset_at_ms: retry_after_ms.map(|ms| now_ms + ms),
        // `retry_after_ms` is in ms; the wire field is seconds. Convert
        // for telemetry parity with raw `Retry-After` header values.
        retry_after_seconds: retry_after_ms.map(|ms| (ms / 1000).max(0)),
        last_observed_ms: now_ms,
    };
    let mut snap = app_state.write().await;
    snap.rate_limits.insert(provider.to_string(), entry);
}

/// Clear a provider's rejected rate-limit observation after a successful
/// request. Entries with no reset header otherwise suppress post-turn
/// promptSuggestion indefinitely.
pub(crate) async fn clear_rate_limit_observation(
    app_state: &Arc<RwLock<coco_types::ToolAppState>>,
    provider: &str,
) {
    if provider.is_empty() {
        return;
    }
    let mut snap = app_state.write().await;
    snap.rate_limits.remove(provider);
}

/// Announce a model fallback / recovery transition as an inline
/// stream notice. Writes a system-tagged line into the transcript so
/// SDK consumers + the TUI see it alongside the agent's response.
///
/// Templates are direction-aware:
/// - `CapacityDegrade` → "Switched to {new} due to high demand for {original}."
/// - `ProbeRecovery`   → "Recovered to primary {new} after probe."
///
/// `original` may be empty if the previous slot never identified
/// itself — the message degrades gracefully on each branch.
pub(crate) async fn emit_model_fallback_notice(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    original: &str,
    new_model: &str,
    session_id: &str,
    reason: ModelFallbackReason,
) {
    let notice = match reason {
        ModelFallbackReason::CapacityDegrade { .. } => {
            if original.is_empty() {
                format!("[system] Switched to fallback model {new_model} due to high demand.\n")
            } else {
                format!("[system] Switched to {new_model} due to high demand for {original}.\n")
            }
        }
        ModelFallbackReason::ProbeRecovery => {
            format!("[system] Recovered to primary {new_model} after probe.\n")
        }
    };
    let _ = emit_stream(
        event_tx,
        AgentStreamEvent::TextDelta {
            turn_id: session_id.to_string(),
            delta: notice,
        },
    )
    .await;
}

/// Extract the first `ToolResult` text payload from a run of
/// `ordered_messages` so the streaming path can populate
/// `ToolUseCompleted.output` with the same string the SDK expects.
/// Mirrors the non-streaming runner's `render_completed_output`
/// helper in `tool_call_runner.rs`.
pub(crate) fn extract_streaming_result_text(ordered: &[Message]) -> String {
    for msg in ordered {
        if let Message::ToolResult(tr) = msg
            && let LlmMessage::Tool { content, .. } = &tr.message
        {
            for part in content {
                if let coco_messages::ToolContent::ToolResult(r) = part {
                    match &r.output {
                        ToolResultContent::Text { value, .. } => {
                            return value.clone();
                        }
                        ToolResultContent::ErrorText { value, .. } => {
                            return value.clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    String::new()
}

/// Compute the `deferred_tools_delta`.
///
/// **Three inputs**:
///
///   - `current_deferred` — names the model can find via `ToolSearch`
///     this turn but cannot yet invoke directly.
///   - `current_loaded` — names the model already has full schema for
///     (eager tools + discovered deferred tools).
///   - `last_announced` — names announced in the most recent
///     `deferred_tools_delta` reminder, persisted on `ToolAppState`.
///
/// Diff rules:
///
///   - `added = current_deferred - last_announced` — newly searchable
///     names the model has not been told about yet (MCP server just
///     connected, or first turn after session start).
///   - `removed = last_announced - (current_deferred ∪ current_loaded)`
///     — names the model knew about that have left the registry
///     entirely (MCP server disconnect). A name that moves from
///     deferred → loaded (model discovered it via `ToolSearch`) is
///     **silently** kept in the announced pool — its schema now
///     appears directly in the request's tool list, no further
///     reminder is required.
///
/// Returns `None` when both diffs are empty.
pub(crate) fn compute_tools_delta(
    current_deferred: &[String],
    current_loaded: &[String],
    last_announced: &HashSet<String>,
) -> Option<coco_system_reminder::DeferredToolsDeltaInfo> {
    let registry_pool: HashSet<&str> = current_deferred
        .iter()
        .map(String::as_str)
        .chain(current_loaded.iter().map(String::as_str))
        .collect();

    let mut added_lines: Vec<String> = current_deferred
        .iter()
        .filter(|t| !last_announced.contains(t.as_str()))
        .map(|t| format!("- {t}"))
        .collect();
    let mut removed_names: Vec<String> = last_announced
        .iter()
        .filter(|t| !registry_pool.contains(t.as_str()))
        .cloned()
        .collect();

    if added_lines.is_empty() && removed_names.is_empty() {
        return None;
    }
    // Stable ordering so consecutive emissions with the same delta
    // produce byte-identical reminders (simpler to diff in tests + logs).
    added_lines.sort();
    removed_names.sort();
    Some(coco_system_reminder::DeferredToolsDeltaInfo {
        added_lines,
        removed_names,
    })
}

/// Whether the most recent assistant message's total context exceeds
/// `threshold` tokens.
///
/// Accepts the threshold as a parameter so users can tune it for their
/// configured plan-role model (set via
/// `PlanModeSettings.plan_model_fallback_threshold_tokens`).
///
/// The "total context" is the sum of normalized `input_tokens` (all input
/// buckets, including cache read/write) and `output_tokens`. Returns `false`
/// when the history has no assistant message yet (cold start).
pub(crate) fn most_recent_assistant_exceeds<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    threshold: i64,
) -> bool {
    for msg in messages.iter().rev() {
        if let Message::Assistant(a) = msg.borrow()
            && let Some(usage) = &a.usage
        {
            let total = usage
                .input_tokens
                .total
                .saturating_add(usage.output_tokens.total);
            return total > threshold;
        }
    }
    false
}

/// Extract the raw user-input text from the most-recent non-meta user
/// message in history. Returns `None` when there's no plain-text user
/// message (e.g. the session opened with a compacted summary).
pub(crate) fn latest_user_input_text(history: &MessageHistory) -> Option<String> {
    for msg in history.iter().rev() {
        let Message::User(u) = msg.as_ref() else {
            continue;
        };
        if let LlmMessage::User { content, .. } = &u.message {
            for part in content {
                if let UserContentPart::Text(tp) = part {
                    return Some(tp.text.clone());
                }
            }
        }
    }
    None
}

/// Compute the `mcp_instructions_delta` between the current server-instruction
/// set and the last-announced set on `ToolAppState`.
///
/// The announced map is persisted on
/// `app_state.last_announced_mcp_instructions` so the diff is
/// O(|current ∪ announced|).
pub(crate) fn compute_mcp_instructions_delta(
    current: &HashMap<String, String>,
    last_announced: &HashMap<String, String>,
) -> Option<coco_system_reminder::McpInstructionsDeltaInfo> {
    let mut added_blocks: Vec<String> = current
        .iter()
        .filter(|(name, text)| {
            last_announced
                .get(name.as_str())
                .is_none_or(|prev| prev != *text)
        })
        .map(|(name, text)| format!("## {name}\n\n{text}"))
        .collect();
    let mut removed_names: Vec<String> = last_announced
        .keys()
        .filter(|name| !current.contains_key(name.as_str()))
        .cloned()
        .collect();

    if added_blocks.is_empty() && removed_names.is_empty() {
        return None;
    }
    added_blocks.sort();
    removed_names.sort();
    Some(coco_system_reminder::McpInstructionsDeltaInfo {
        added_blocks,
        removed_names,
    })
}

/// Compute the `agent_listing_delta` between the current agent types and
/// the last-announced set on `ToolAppState`. `is_initial` is true when no
/// agents have been announced yet (first emission of the session); that
/// flips the "Available agent types" header (vs "New agent types are now
/// available").
///
/// `show_concurrency_note` is unconditionally `true` here. The concurrency
/// hint is informational and always relevant — the renderer
/// (`agent_listing_delta.rs`) emits it on every delta, not just the initial
/// one, so the model is reminded to parallelize when new agents become
/// available.
pub(crate) fn compute_agents_delta(
    current_agents: &[String],
    last_announced: &HashSet<String>,
) -> Option<coco_system_reminder::AgentListingDeltaInfo> {
    let current_set: HashSet<&String> = current_agents.iter().collect();

    let mut added_lines: Vec<String> = current_agents
        .iter()
        .filter(|t| !last_announced.contains(t.as_str()))
        .map(|t| format!("- {t}"))
        .collect();
    let mut removed_types: Vec<String> = last_announced
        .iter()
        .filter(|t| !current_set.contains(*t))
        .cloned()
        .collect();

    if added_lines.is_empty() && removed_types.is_empty() {
        return None;
    }
    added_lines.sort();
    removed_types.sort();
    let is_initial = last_announced.is_empty();
    Some(coco_system_reminder::AgentListingDeltaInfo {
        added_lines,
        removed_types,
        is_initial,
        show_concurrency_note: true,
    })
}

/// LRU + time-window throttle for protocol-level tool-progress events.
///
/// One throttle per `parent_tool_use_id`, ≤1 emission / 30 s,
/// LRU-bound to 100 keys.
pub(crate) struct ProgressThrottle {
    last_sent: lru::LruCache<String, std::time::Instant>,
    throttle: std::time::Duration,
}

impl ProgressThrottle {
    /// Default: 30 s window, 100-key LRU.
    pub(crate) fn new() -> Self {
        let cap = std::num::NonZeroUsize::new(100).unwrap_or(std::num::NonZeroUsize::MIN);
        Self::with_params(std::time::Duration::from_secs(30), cap)
    }

    /// Test-only constructor that takes an explicit window + LRU
    /// size. The tests use a 1 ms window so they don't need to sleep.
    pub(crate) fn with_params(
        throttle: std::time::Duration,
        max_tracking: std::num::NonZeroUsize,
    ) -> Self {
        Self {
            last_sent: lru::LruCache::new(max_tracking),
            throttle,
        }
    }

    /// Returns `true` if a protocol event for `key` should be
    /// emitted now and stamps the send time. Returns `false` (skip)
    /// when a prior emission fell inside the throttle window.
    pub(crate) fn allow(&mut self, key: &str, now: std::time::Instant) -> bool {
        // `peek` (vs `get`) does not bump recency — within-window
        // skips must not refresh the LRU position.
        if let Some(prev) = self.last_sent.peek(key)
            && now.duration_since(*prev) < self.throttle
        {
            return false;
        }
        self.last_sent.put(key.to_string(), now);
        true
    }
}

/// Extract `(tool_name, elapsed_seconds, task_id)` from a
/// `ToolProgress.data` payload IF it matches a bash/powershell shape.
/// Returns `None` for unrelated payload types (e.g. agent/skill
/// progress) — those follow different propagation rules and are not
/// currently surfaced as `ServerNotification::ToolProgress`.
pub(crate) fn classify_progress_payload(
    data: &serde_json::Value,
) -> Option<(&'static str, f64, Option<String>)> {
    let obj = data.as_object()?;
    let ptype = obj.get("type").and_then(serde_json::Value::as_str)?;
    let tool_name = match ptype {
        "bash_progress" => "Bash",
        "powershell_progress" => "PowerShell",
        _ => return None,
    };
    let elapsed = obj
        .get("elapsedTimeSeconds")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let task_id = obj
        .get("taskId")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    Some((tool_name, elapsed, task_id))
}

/// Fan out a single `ToolProgress` event to both TUI and protocol
/// layers, applying the throttle to the protocol layer. Extracted
/// from the session drain loop so it can be unit-tested without
/// standing up a full engine.
pub(crate) async fn drain_one_progress(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    progress: coco_tool_runtime::ToolProgress,
    throttle: &mut ProgressThrottle,
) {
    // Fan-out #1: raw TUI event, always emitted.
    let tool_use_id = progress.tool_use_id.clone();
    let _ = emit_tui(
        event_tx,
        coco_types::TuiOnlyEvent::ToolProgress {
            tool_use_id: tool_use_id.clone(),
            data: progress.data.clone(),
        },
    )
    .await;

    // Fan-out #2: protocol ToolProgress. Only bash/powershell
    // progress qualifies.
    let Some((tool_name, elapsed, task_id)) = classify_progress_payload(&progress.data) else {
        return;
    };

    // Throttle key: TS uses `parentToolUseID` because `toolUseID`
    // rotates per progress event in its world. Rust's tool_use_id
    // is stable, so it's a safe fallback when parent is absent.
    let key = progress
        .parent_tool_use_id
        .clone()
        .unwrap_or_else(|| tool_use_id.clone());
    if !throttle.allow(&key, std::time::Instant::now()) {
        return;
    }

    let _ = emit_protocol(
        event_tx,
        ServerNotification::ToolProgress(coco_types::ToolProgressParams {
            tool_use_id,
            tool_name: tool_name.to_string(),
            parent_tool_use_id: progress.parent_tool_use_id,
            elapsed_time_seconds: elapsed,
            task_id,
        }),
    )
    .await;
}

#[cfg(test)]
#[path = "engine_helpers.test.rs"]
mod tests;
