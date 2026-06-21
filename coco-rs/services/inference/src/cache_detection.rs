//! Prompt cache break detection.
//!
//! Two-phase detection:
//!   Phase 1 (pre-call): `record_prompt_state()` snapshots system/tool hashes.
//!   Phase 2 (post-call): `check_response_for_cache_break()` compares cache tokens.
//!
//! ## Multi-provider design
//!
//! Where TS tracks 12 typed Anthropic-specific fields (`betas`,
//! `cachedMCEnabled`, `isUsingOverage`, …), this Rust implementation collapses
//! all provider-specific knobs into a single `extra_body_hash` computed by
//! [`canonical_extra_body_hash`]. Tradeoff:
//! - **Pro**: detector code stays provider-agnostic — adding a new provider
//!   crate (ByteDance, etc.) needs zero detector changes.
//! - **Pro**: any new provider option field is auto-covered (no schema drift
//!   risk).
//! - **Con**: when a break is attributable to provider options, the reason
//!   string says only `"provider options changed"`. Operators reach for the
//!   diff file (gated by `COCO_CACHE_BREAK_DIFF=1`) to learn which sub-field.
//!
//! ## Tracking key
//!
//! - `query_source == "compact"` shares `repl_main_thread`'s key (same
//!   server-side cache).
//! - Other tracked sources (`repl_main_thread`, `sdk`, `agent:*`) prefer the
//!   `agent_id` for per-instance isolation when concurrent subagents would
//!   otherwise collide.
//! - Untracked sources (`speculation`, `session_memory`, `prompt_suggestion`)
//!   silently no-op.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Cache state
// ---------------------------------------------------------------------------

/// Observed cache state for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// First call — no baseline to compare against.
    Cold,
    /// Cache read tokens are stable (within 5% of previous and above min threshold).
    Warm,
    /// Significant drop in cache read tokens detected.
    Broken,
}

// ---------------------------------------------------------------------------
// Prompt state snapshot
// ---------------------------------------------------------------------------

/// Hashed snapshot of prompt state for cache break detection.
/// Stores hashes rather than full content to keep memory low.
#[derive(Debug, Clone)]
struct PromptSnapshot {
    /// Hash of the system prompt (with cache_control stripped).
    system_hash: u64,
    /// Hash of tool schemas (with cache_control stripped).
    tools_hash: u64,
    /// Hash of cache_control metadata on system blocks.
    cache_control_hash: u64,
    /// Ordered tool names.
    tool_names: Vec<String>,
    /// Per-tool schema hashes for pinpointing which tool changed.
    per_tool_hashes: HashMap<String, u64>,
    /// Per-tool schema byte/token estimates.
    per_tool_schema_sizes: HashMap<String, ToolSchemaSize>,
    /// Character count of the system prompt.
    system_char_count: i64,
    /// Model ID.
    model: String,
    /// Whether this model is excluded from cache-break detection.
    cache_break_detection_excluded: bool,
    /// Whether fast mode was active.
    fast_mode: bool,
    /// Sorted beta header list (Anthropic-only; empty for other providers).
    betas: Vec<String>,
    /// Hash of the merged provider_options blob (canonical form).
    extra_body_hash: u64,
    /// Resolved thinking effort string ("low" / "medium" / "" / numeric / …).
    effort_value: String,
    /// Provider's global cache strategy bucket
    /// (`tool_based` / `system_prompt` / `none`). Currently provider-defined;
    /// empty for providers that don't categorize.
    global_cache_strategy: String,
    /// Whether auto-mode was active when the call was issued.
    auto_mode_active: bool,
    /// Anthropic overage state. Latched session-stable when correct; tracked
    /// here so flips trigger a `tengu_prompt_cache_break` regression alarm.
    is_using_overage: bool,
    /// Cache-editing beta header presence. Same regression-tracking rationale
    /// as `auto_mode_active` / `is_using_overage`.
    cached_mc_enabled: bool,
    /// Call count for this source.
    call_count: i64,
    /// Previous cache read tokens (None on first call).
    prev_cache_read_tokens: Option<i64>,
    /// Whether a cache deletion was pending (expected drop).
    cache_deletion_pending: bool,
    /// Last serialized provider_options blob (used for diff fallback when the
    /// hash changes). `None` until `record_prompt_state` has been called once.
    last_extra_body_serialized: Option<String>,
}

// ---------------------------------------------------------------------------
// Detected changes
// ---------------------------------------------------------------------------

/// Changes detected between consecutive prompt states.
#[derive(Debug, Clone, Default)]
pub struct PendingChanges {
    pub system_prompt_changed: bool,
    pub tool_schemas_changed: bool,
    pub model_changed: bool,
    pub fast_mode_changed: bool,
    pub cache_control_changed: bool,
    pub betas_changed: bool,
    pub extra_body_changed: bool,
    pub effort_changed: bool,
    pub global_cache_strategy_changed: bool,
    pub auto_mode_changed: bool,
    pub overage_changed: bool,
    pub cached_mc_changed: bool,
    pub added_tool_count: i64,
    pub removed_tool_count: i64,
    pub system_char_delta: i64,
    pub added_tools: Vec<String>,
    pub removed_tools: Vec<String>,
    pub changed_tool_schemas: Vec<String>,
    pub changed_tool_schema_deltas: Vec<String>,
    pub added_betas: Vec<String>,
    pub removed_betas: Vec<String>,
    pub previous_model: String,
    pub new_model: String,
    pub prev_effort_value: String,
    pub new_effort_value: String,
    pub prev_global_cache_strategy: String,
    pub new_global_cache_strategy: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolSchemaSize {
    pub bytes: i64,
    pub estimated_tokens: i64,
}

impl PendingChanges {
    /// Build a human-readable explanation of what changed.
    pub fn explain(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.model_changed {
            parts.push(format!(
                "model changed ({} -> {})",
                self.previous_model, self.new_model
            ));
        }
        if self.system_prompt_changed {
            let delta = self.system_char_delta;
            let info = if delta == 0 {
                String::new()
            } else if delta > 0 {
                format!(" (+{delta} chars)")
            } else {
                format!(" ({delta} chars)")
            };
            parts.push(format!("system prompt changed{info}"));
        }
        if self.tool_schemas_changed {
            let diff = if self.added_tool_count > 0 || self.removed_tool_count > 0 {
                format!(
                    " (+{}/-{} tools)",
                    self.added_tool_count, self.removed_tool_count
                )
            } else if !self.changed_tool_schema_deltas.is_empty() {
                format!(
                    " (changed schemas: {})",
                    self.changed_tool_schema_deltas.join(", ")
                )
            } else {
                " (tool prompt/schema changed, same tool set)".to_string()
            };
            parts.push(format!("tools changed{diff}"));
        }
        if self.fast_mode_changed {
            parts.push("fast mode toggled".into());
        }
        if self.global_cache_strategy_changed {
            parts.push(format!(
                "global cache strategy changed ({} -> {})",
                if self.prev_global_cache_strategy.is_empty() {
                    "none"
                } else {
                    self.prev_global_cache_strategy.as_str()
                },
                if self.new_global_cache_strategy.is_empty() {
                    "none"
                } else {
                    self.new_global_cache_strategy.as_str()
                },
            ));
        }
        if self.cache_control_changed
            && !self.system_prompt_changed
            && !self.global_cache_strategy_changed
        {
            // Only report as standalone cause when nothing else explains it —
            // otherwise the scope/TTL flip is a consequence, not the root cause.
            parts.push("cache_control changed (scope or TTL)".into());
        }
        if self.betas_changed {
            let added = if self.added_betas.is_empty() {
                String::new()
            } else {
                format!("+{}", self.added_betas.join(","))
            };
            let removed = if self.removed_betas.is_empty() {
                String::new()
            } else {
                format!("-{}", self.removed_betas.join(","))
            };
            let diff = [added, removed]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            parts.push(if diff.is_empty() {
                "betas changed".into()
            } else {
                format!("betas changed ({diff})")
            });
        }
        if self.auto_mode_changed {
            parts.push("auto mode toggled".into());
        }
        if self.overage_changed {
            parts.push("overage state changed (TTL latched, no flip)".into());
        }
        if self.cached_mc_changed {
            parts.push("cached microcompact toggled".into());
        }
        if self.effort_changed {
            parts.push(format!(
                "effort changed ({} -> {})",
                if self.prev_effort_value.is_empty() {
                    "default"
                } else {
                    self.prev_effort_value.as_str()
                },
                if self.new_effort_value.is_empty() {
                    "default"
                } else {
                    self.new_effort_value.as_str()
                },
            ));
        }
        if self.extra_body_changed {
            parts.push("provider options changed".into());
        }
        parts
    }
}

// ---------------------------------------------------------------------------
// Cache break result
// ---------------------------------------------------------------------------

/// Result of checking a response for a cache break.
#[derive(Debug, Clone)]
pub struct CacheBreakResult {
    pub state: CacheState,
    /// Human-readable reason for the break (empty if Warm/Cold).
    pub reason: String,
    /// Changes from phase 1, if any were detected.
    pub changes: Option<PendingChanges>,
    /// Previous cache read tokens.
    pub prev_cache_read_tokens: Option<i64>,
    /// Current cache read tokens.
    pub cache_read_tokens: i64,
    /// Cache creation tokens from the response.
    pub cache_creation_tokens: i64,
}

// ---------------------------------------------------------------------------
// Detector
// ---------------------------------------------------------------------------

/// Minimum absolute token drop to trigger a cache break warning.
const MIN_CACHE_MISS_TOKENS: i64 = 2_000;

/// 5-minute TTL threshold.
const CACHE_TTL_5MIN_MS: i64 = 5 * 60 * 1000;

/// 1-hour TTL threshold.
const CACHE_TTL_1HOUR_MS: i64 = 60 * 60 * 1000;

/// Maximum number of tracked sources to prevent unbounded memory growth.
const MAX_TRACKED_SOURCES: usize = 10;

/// Tracked source prefixes. Only these query sources are monitored for cache breaks.
const TRACKED_SOURCE_PREFIXES: &[&str] = &[
    "repl_main_thread",
    "sdk",
    "agent:custom",
    "agent:default",
    "agent:builtin",
];

fn signed_delta(delta: i64) -> String {
    if delta > 0 {
        format!("+{delta}")
    } else {
        delta.to_string()
    }
}

/// Tracking key for a query source.
///
/// - `compact` shares `repl_main_thread`'s key.
/// - `agent_id` (when supplied for a tracked prefix) takes precedence over
///   `query_source` so concurrent agents of the same type don't collide.
/// - Untracked sources return `None` (silent no-op).
fn tracking_key(query_source: &str, agent_id: Option<&str>) -> Option<String> {
    if query_source == "compact" {
        return Some("repl_main_thread".to_string());
    }
    for prefix in TRACKED_SOURCE_PREFIXES {
        if query_source.starts_with(prefix) {
            return Some(
                agent_id
                    .map(String::from)
                    .unwrap_or_else(|| query_source.to_string()),
            );
        }
    }
    None
}

/// Input for recording prompt state (phase 1).
///
/// Most fields are computed once per call by the inference layer; provider-
/// specific extras get folded into `extra_body_hash` via
/// [`canonical_extra_body_hash`] so this struct stays provider-agnostic.
#[derive(Debug, Clone, Default)]
pub struct PromptStateInput {
    /// Hash of system prompt content (without cache_control).
    pub system_hash: u64,
    /// Hash of tool schemas (without cache_control).
    pub tools_hash: u64,
    /// Hash of cache_control metadata.
    pub cache_control_hash: u64,
    /// Ordered tool names.
    pub tool_names: Vec<String>,
    /// Per-tool schema hashes.
    pub per_tool_hashes: HashMap<String, u64>,
    /// Per-tool schema byte/token estimates for diagnostics.
    pub per_tool_schema_sizes: HashMap<String, ToolSchemaSize>,
    /// Character count of system prompt.
    pub system_char_count: i64,
    /// Model ID.
    pub model: String,
    /// Whether this model is excluded from cache-break detection (its
    /// `cache_read` drops are noise). Resolved from `ModelInfo` by the
    /// caller so this crate never matches against a model id.
    pub cache_break_detection_excluded: bool,
    /// Query source identifier.
    pub query_source: String,
    /// Optional agent id for per-instance subagent isolation.
    pub agent_id: Option<String>,
    /// Whether fast mode is active.
    pub fast_mode: bool,
    /// Sorted beta header list. Anthropic-only; empty for other providers.
    pub betas: Vec<String>,
    /// Hash of the merged provider_options blob.
    pub extra_body_hash: u64,
    /// Optional canonical serialization of provider_options for diff fallback.
    pub extra_body_serialized: Option<String>,
    /// Resolved thinking effort.
    pub effort_value: String,
    /// Global cache strategy bucket.
    pub global_cache_strategy: String,
    /// Whether auto-mode is active.
    pub auto_mode_active: bool,
    /// Anthropic overage state.
    pub is_using_overage: bool,
    /// Cache-editing beta header presence.
    pub cached_mc_enabled: bool,
}

/// Tracks prompt cache state across calls to detect cache breaks.
/// Uses a per-source map with two-phase (pre-call / post-call) API.
pub struct CacheBreakDetector {
    states: HashMap<String, PromptSnapshot>,
    pending_changes: HashMap<String, PendingChanges>,
}

impl CacheBreakDetector {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            pending_changes: HashMap::new(),
        }
    }

    /// Phase 1: Record prompt state before an API call.
    ///
    /// Detects what changed from the previous call and stores pending changes
    /// for phase 2 to use.
    pub fn record_prompt_state(&mut self, input: PromptStateInput) {
        // Record EVERY model, including cache-break-excluded ones (Haiku).
        // Phase 1 must keep the snapshot's model + exclusion flag current so
        // that on a same-source model swap (e.g. fast-mode Sonnet→Haiku) the
        // post-call phase-2 check sees the *current* model's exclusion rather
        // than a stale one. Skipping excluded models here would freeze the
        // snapshot on the previous model and resurrect the very false positive
        // the exclusion exists to suppress. Matches the TS detector, which
        // also records all models and gates exclusively in phase 2.
        let key = match tracking_key(&input.query_source, input.agent_id.as_deref()) {
            Some(k) => k,
            None => return,
        };

        let prev = match self.states.get(&key) {
            Some(p) => p,
            None => {
                // Evict oldest entries if at capacity. Iteration order is
                // arbitrary on HashMap, so "oldest" is approximate — TS uses
                // insertion order via Map; we accept the looseness because
                // the cap is small (10) and false eviction only loses
                // observability, not correctness.
                while self.states.len() >= MAX_TRACKED_SOURCES {
                    if let Some(oldest) = self.states.keys().next().cloned() {
                        self.states.remove(&oldest);
                        self.pending_changes.remove(&oldest);
                    }
                }

                self.states.insert(
                    key,
                    PromptSnapshot {
                        system_hash: input.system_hash,
                        tools_hash: input.tools_hash,
                        cache_control_hash: input.cache_control_hash,
                        tool_names: input.tool_names,
                        per_tool_hashes: input.per_tool_hashes,
                        per_tool_schema_sizes: input.per_tool_schema_sizes,
                        system_char_count: input.system_char_count,
                        model: input.model,
                        cache_break_detection_excluded: input.cache_break_detection_excluded,
                        fast_mode: input.fast_mode,
                        betas: input.betas,
                        extra_body_hash: input.extra_body_hash,
                        effort_value: input.effort_value,
                        global_cache_strategy: input.global_cache_strategy,
                        auto_mode_active: input.auto_mode_active,
                        is_using_overage: input.is_using_overage,
                        cached_mc_enabled: input.cached_mc_enabled,
                        call_count: 1,
                        prev_cache_read_tokens: None,
                        cache_deletion_pending: false,
                        last_extra_body_serialized: input.extra_body_serialized,
                    },
                );
                return;
            }
        };

        let system_prompt_changed = input.system_hash != prev.system_hash;
        let tool_schemas_changed = input.tools_hash != prev.tools_hash;
        let model_changed = input.model != prev.model;
        let fast_mode_changed = input.fast_mode != prev.fast_mode;
        let cache_control_changed = input.cache_control_hash != prev.cache_control_hash;
        let betas_changed = input.betas != prev.betas;
        let extra_body_changed = input.extra_body_hash != prev.extra_body_hash;
        let effort_changed = input.effort_value != prev.effort_value;
        let global_cache_strategy_changed =
            input.global_cache_strategy != prev.global_cache_strategy;
        let auto_mode_changed = input.auto_mode_active != prev.auto_mode_active;
        let overage_changed = input.is_using_overage != prev.is_using_overage;
        let cached_mc_changed = input.cached_mc_enabled != prev.cached_mc_enabled;

        let anything_changed = system_prompt_changed
            || tool_schemas_changed
            || model_changed
            || fast_mode_changed
            || cache_control_changed
            || betas_changed
            || extra_body_changed
            || effort_changed
            || global_cache_strategy_changed
            || auto_mode_changed
            || overage_changed
            || cached_mc_changed;

        if anything_changed {
            let prev_tool_set: HashSet<&str> = prev.tool_names.iter().map(String::as_str).collect();
            let new_tool_set: HashSet<&str> = input.tool_names.iter().map(String::as_str).collect();
            let added_tools: Vec<String> = input
                .tool_names
                .iter()
                .filter(|n| !prev_tool_set.contains(n.as_str()))
                .cloned()
                .collect();
            let removed_tools: Vec<String> = prev
                .tool_names
                .iter()
                .filter(|n| !new_tool_set.contains(n.as_str()))
                .cloned()
                .collect();
            let changed_tool_schemas: Vec<String> = if tool_schemas_changed {
                input
                    .tool_names
                    .iter()
                    .filter(|n| {
                        prev_tool_set.contains(n.as_str())
                            && input.per_tool_hashes.get(n.as_str())
                                != prev.per_tool_hashes.get(n.as_str())
                    })
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let changed_tool_schema_deltas = changed_tool_schemas
                .iter()
                .filter_map(|name| {
                    let prev_size = prev.per_tool_schema_sizes.get(name)?;
                    let new_size = input.per_tool_schema_sizes.get(name)?;
                    Some(format!(
                        "{} ({} bytes, ~{} tokens)",
                        name,
                        signed_delta(new_size.bytes - prev_size.bytes),
                        signed_delta(new_size.estimated_tokens - prev_size.estimated_tokens)
                    ))
                })
                .collect();
            let prev_beta_set: HashSet<&str> = prev.betas.iter().map(String::as_str).collect();
            let new_beta_set: HashSet<&str> = input.betas.iter().map(String::as_str).collect();
            let added_betas: Vec<String> = input
                .betas
                .iter()
                .filter(|b| !prev_beta_set.contains(b.as_str()))
                .cloned()
                .collect();
            let removed_betas: Vec<String> = prev
                .betas
                .iter()
                .filter(|b| !new_beta_set.contains(b.as_str()))
                .cloned()
                .collect();

            self.pending_changes.insert(
                key.clone(),
                PendingChanges {
                    system_prompt_changed,
                    tool_schemas_changed,
                    model_changed,
                    fast_mode_changed,
                    cache_control_changed,
                    betas_changed,
                    extra_body_changed,
                    effort_changed,
                    global_cache_strategy_changed,
                    auto_mode_changed,
                    overage_changed,
                    cached_mc_changed,
                    added_tool_count: added_tools.len() as i64,
                    removed_tool_count: removed_tools.len() as i64,
                    system_char_delta: input.system_char_count - prev.system_char_count,
                    added_tools,
                    removed_tools,
                    changed_tool_schemas,
                    changed_tool_schema_deltas,
                    added_betas,
                    removed_betas,
                    previous_model: prev.model.clone(),
                    new_model: input.model.clone(),
                    prev_effort_value: prev.effort_value.clone(),
                    new_effort_value: input.effort_value.clone(),
                    prev_global_cache_strategy: prev.global_cache_strategy.clone(),
                    new_global_cache_strategy: input.global_cache_strategy.clone(),
                },
            );
        } else {
            self.pending_changes.remove(&key);
        }

        // Update the snapshot in place. The key is guaranteed to be present
        // because the early-return branch above inserted it when absent.
        if let Some(snapshot) = self.states.get_mut(&key) {
            snapshot.system_hash = input.system_hash;
            snapshot.tools_hash = input.tools_hash;
            snapshot.cache_control_hash = input.cache_control_hash;
            snapshot.tool_names = input.tool_names;
            snapshot.per_tool_hashes = input.per_tool_hashes;
            snapshot.per_tool_schema_sizes = input.per_tool_schema_sizes;
            snapshot.system_char_count = input.system_char_count;
            snapshot.model = input.model;
            snapshot.cache_break_detection_excluded = input.cache_break_detection_excluded;
            snapshot.fast_mode = input.fast_mode;
            snapshot.betas = input.betas;
            snapshot.extra_body_hash = input.extra_body_hash;
            snapshot.effort_value = input.effort_value;
            snapshot.global_cache_strategy = input.global_cache_strategy;
            snapshot.auto_mode_active = input.auto_mode_active;
            snapshot.is_using_overage = input.is_using_overage;
            snapshot.cached_mc_enabled = input.cached_mc_enabled;
            snapshot.call_count += 1;
            snapshot.last_extra_body_serialized = input.extra_body_serialized;
        }
    }

    /// Phase 2: Check the API response for a cache break.
    ///
    /// Compares current cache read tokens against the previous value to detect
    /// significant drops (>5% and >2000 tokens).
    pub fn check_response_for_cache_break(
        &mut self,
        query_source: &str,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
        time_since_last_assistant_ms: Option<i64>,
        agent_id: Option<&str>,
    ) -> CacheBreakResult {
        let key = match tracking_key(query_source, agent_id) {
            Some(k) => k,
            None => {
                return CacheBreakResult {
                    state: CacheState::Cold,
                    reason: "untracked source".into(),
                    changes: None,
                    prev_cache_read_tokens: None,
                    cache_read_tokens,
                    cache_creation_tokens,
                };
            }
        };

        let snapshot = match self.states.get_mut(&key) {
            Some(s) => s,
            None => {
                return CacheBreakResult {
                    state: CacheState::Cold,
                    reason: "no prior state".into(),
                    changes: None,
                    prev_cache_read_tokens: None,
                    cache_read_tokens,
                    cache_creation_tokens,
                };
            }
        };

        // Excluded model — skip detection entirely (its server-side
        // caching behavior makes cache_read drops noise, not real breaks).
        if snapshot.cache_break_detection_excluded {
            self.pending_changes.remove(&key);
            return CacheBreakResult {
                state: CacheState::Cold,
                reason: "excluded model".into(),
                changes: None,
                prev_cache_read_tokens: None,
                cache_read_tokens,
                cache_creation_tokens,
            };
        }

        let prev_cache_read = snapshot.prev_cache_read_tokens;
        snapshot.prev_cache_read_tokens = Some(cache_read_tokens);

        // Handle cache deletion (expected drop)
        if snapshot.cache_deletion_pending {
            snapshot.cache_deletion_pending = false;
            self.pending_changes.remove(&key);
            return CacheBreakResult {
                state: CacheState::Warm,
                reason: "cache deletion applied (expected drop)".into(),
                changes: None,
                prev_cache_read_tokens: prev_cache_read,
                cache_read_tokens,
                cache_creation_tokens,
            };
        }

        // First call — no previous value to compare
        let prev = match prev_cache_read {
            Some(p) => p,
            None => {
                self.pending_changes.remove(&key);
                return CacheBreakResult {
                    state: CacheState::Cold,
                    reason: "first call".into(),
                    changes: None,
                    prev_cache_read_tokens: None,
                    cache_read_tokens,
                    cache_creation_tokens,
                };
            }
        };

        // Check for cache break: >5% drop AND absolute drop > threshold
        let token_drop = prev - cache_read_tokens;
        if cache_read_tokens >= (prev as f64 * 0.95) as i64 || token_drop < MIN_CACHE_MISS_TOKENS {
            let changes = self.pending_changes.remove(&key);
            return CacheBreakResult {
                state: CacheState::Warm,
                reason: String::new(),
                changes,
                prev_cache_read_tokens: Some(prev),
                cache_read_tokens,
                cache_creation_tokens,
            };
        }

        // Cache break detected — build explanation
        let changes = self.pending_changes.remove(&key);
        let reason = match &changes {
            Some(c) => {
                let parts = c.explain();
                if parts.is_empty() {
                    build_ttl_reason(time_since_last_assistant_ms)
                } else {
                    parts.join(", ")
                }
            }
            None => build_ttl_reason(time_since_last_assistant_ms),
        };

        CacheBreakResult {
            state: CacheState::Broken,
            reason,
            changes,
            prev_cache_read_tokens: Some(prev),
            cache_read_tokens,
            cache_creation_tokens,
        }
    }

    /// Notify that a cache deletion (e.g. cached microcompact) occurred.
    /// The next response will legitimately have lower cache read tokens.
    pub fn notify_cache_deletion(&mut self, query_source: &str, agent_id: Option<&str>) {
        if let Some(key) = tracking_key(query_source, agent_id)
            && let Some(snapshot) = self.states.get_mut(&key)
        {
            snapshot.cache_deletion_pending = true;
        }
    }

    /// Notify that compaction occurred. Reset the baseline so the expected
    /// drop in cache tokens doesn't trigger a false positive.
    pub fn notify_compaction(&mut self, query_source: &str, agent_id: Option<&str>) {
        if let Some(key) = tracking_key(query_source, agent_id)
            && let Some(snapshot) = self.states.get_mut(&key)
        {
            snapshot.prev_cache_read_tokens = None;
        }
    }

    /// Clean up tracking state for a specific agent.
    pub fn cleanup_agent(&mut self, agent_id: &str) {
        self.states.remove(agent_id);
        self.pending_changes.remove(agent_id);
    }

    /// Reset all tracking state.
    pub fn reset(&mut self) {
        self.states.clear();
        self.pending_changes.clear();
    }

    /// Whether the detector currently has no tracked state. Returns
    /// `true` after [`Self::reset`] or [`Self::new`]. Test-only
    /// observable for assertions that the reset wiring fires (notably
    /// the cross-provider fallback advance covered by finding N2 in
    /// `app/query`).
    pub fn is_empty(&self) -> bool {
        self.states.is_empty() && self.pending_changes.is_empty()
    }

    /// Borrow the previous serialized provider_options for a tracking key,
    /// if any. Used by the diff-file fallback when a break attributes to
    /// `extra_body_changed` but the textual change was stripped before
    /// hashing (Anthropic-specific knobs etc.).
    pub fn previous_extra_body_serialized(
        &self,
        query_source: &str,
        agent_id: Option<&str>,
    ) -> Option<String> {
        let key = tracking_key(query_source, agent_id)?;
        self.states
            .get(&key)
            .and_then(|s| s.last_extra_body_serialized.clone())
    }
}

impl Default for CacheBreakDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a TTL-based reason when no client-side changes explain the break.
fn build_ttl_reason(time_since_last_assistant_ms: Option<i64>) -> String {
    match time_since_last_assistant_ms {
        Some(t) if t > CACHE_TTL_1HOUR_MS => "possible 1h TTL expiry (prompt unchanged)".into(),
        Some(t) if t > CACHE_TTL_5MIN_MS => "possible 5min TTL expiry (prompt unchanged)".into(),
        Some(_) => "likely server-side (prompt unchanged, <5min gap)".into(),
        None => "unknown cause".into(),
    }
}

// ---------------------------------------------------------------------------
// Canonical hashing helpers
// ---------------------------------------------------------------------------

/// Compute a stable hash of a `serde_json::Value` after canonicalizing it
/// (recursive sort of object keys). The hash is stable across processes for
/// the same logical content even when the source representation uses an
/// unordered map (e.g. `HashMap<String, Value>` inside a struct that was
/// serialized via `serde_json::to_value`).
///
/// Returns 0 for `Null` so callers can use 0 as "absent" without false hits.
#[must_use]
pub fn canonical_extra_body_hash(value: &serde_json::Value) -> u64 {
    if value.is_null() {
        return 0;
    }
    let canonical = canonicalize_value(value);
    djb2_hash(
        serde_json::to_string(&canonical)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// Serialize a `Value` in canonical form (object keys sorted recursively).
/// Used for diff-file output so before/after blobs compare meaningfully.
#[must_use]
pub fn canonical_extra_body_serialize(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let canonical = canonicalize_value(value);
    serde_json::to_string_pretty(&canonical).unwrap_or_default()
}

fn canonicalize_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize_value(v));
            }
            // Convert BTreeMap → serde_json::Map (insertion order = alphabetical
            // because BTreeMap iterates sorted). serde_json's default
            // serializer preserves Map iteration order.
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_value).collect())
        }
        other => other.clone(),
    }
}

/// Compute a djb2 hash over bytes. Used for system/tools/extra_body hashes
/// across the detector. Deterministic, fast, no crypto dependencies.
#[must_use]
pub fn djb2_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 5381;
    for b in bytes {
        hash = hash.wrapping_mul(33).wrapping_add(*b as u64);
    }
    hash
}

#[cfg(test)]
#[path = "cache_detection.test.rs"]
mod tests;
