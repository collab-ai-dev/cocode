//! Warning-first repeated-tool-call guardrails (hermes v0.13 absorption).
//!
//! Detects three loop shapes within one engine run (one user cycle — the
//! handle's lifetime matches the [`crate::Tool`]-executing engine, which
//! hosts rebuild per user prompt):
//!
//! 1. **Exact failure repeat** — the identical `(tool, args)` call failing
//!    again and again.
//! 2. **Same-tool failures** — one tool failing across distinct args.
//! 3. **No-progress repeat** — an idempotent (read-only) call returning a
//!    byte-identical result again and again.
//!
//! Warning-first: at the default `warn_only` level the guard only appends
//! guidance to the failing tool result; `enforce` additionally blocks the
//! repeated call pre-execution with a synthetic error result. Counters are
//! failure-gated (a success clears its exact-repeat slot), so legitimately
//! repeated successful calls (polling, re-reads after edits) are never
//! blocked and only flag when byte-identical results repeat on a read-only
//! tool.
//!
//! Concurrency note: verdicts are computed per prepared call and counts
//! update as results land. A concurrent batch of identical calls may all
//! pass the pre-execution check; the *next* round gets warned/blocked —
//! same semantics hermes gets from its sequential executor.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use coco_config::{LoopGuardrailConfig, LoopGuardrailLevel};
use coco_types::ToolId;
use serde_json::Value;

/// Which guardrail fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailCode {
    ExactFailureRepeat,
    SameToolFailures,
    NoProgressRepeat,
}

impl GuardrailCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExactFailureRepeat => "exact_failure_repeat",
            Self::SameToolFailures => "same_tool_failures",
            Self::NoProgressRepeat => "no_progress_repeat",
        }
    }
}

/// Post-execution guidance to append to the tool result.
#[derive(Debug, Clone)]
pub struct GuardrailWarning {
    pub code: GuardrailCode,
    pub count: i64,
    pub message: String,
}

impl GuardrailWarning {
    /// Render the append-only result suffix. Append-only on the newest
    /// message keeps the prompt cache intact; wording deliberately avoids
    /// a `[SYSTEM:` shape.
    pub fn render_suffix(&self) -> String {
        format!(
            "\n\n[Tool loop warning: {}; count={}; {}]",
            self.code.as_str(),
            self.count,
            self.message
        )
    }
}

/// Pre-execution block decision (only at `enforce` level).
#[derive(Debug, Clone)]
pub struct GuardrailBlock {
    pub code: GuardrailCode,
    pub count: i64,
    pub message: String,
}

impl GuardrailBlock {
    /// Render the synthetic tool-result body. JSON so builtin and MCP
    /// tool consumers read it uniformly.
    pub fn render_synthetic_result(&self) -> String {
        serde_json::json!({
            "error": self.message,
            "guardrail": { "code": self.code.as_str(), "count": self.count },
        })
        .to_string()
    }
}

#[derive(Default)]
struct GuardState {
    /// signature → consecutive failure count (cleared on success).
    exact_failures: HashMap<u64, i64>,
    /// tool name → total failure count this cycle (any args).
    tool_failures: HashMap<String, i64>,
    /// signature → (last result hash, identical-result repeat count).
    no_progress: HashMap<u64, (u64, i64)>,
}

/// Shared per-engine-run guardrail state + resolved thresholds.
///
/// Cheap to clone (Arc-shared); install one per engine and thread it onto
/// every `ToolUseContext`.
#[derive(Clone)]
pub struct LoopGuardrailHandle {
    state: Arc<Mutex<GuardState>>,
    config: LoopGuardrailConfig,
}

impl LoopGuardrailHandle {
    /// Build a handle from resolved config; `None` when the guard is off.
    pub fn from_config(config: &LoopGuardrailConfig) -> Option<Self> {
        if config.level == LoopGuardrailLevel::Off {
            return None;
        }
        Some(Self {
            state: Arc::new(Mutex::new(GuardState::default())),
            config: config.clone(),
        })
    }

    /// Pre-execution check. Returns a block decision only at `enforce`
    /// level when a hard-stop threshold has been reached for this call.
    pub fn check_block_before_call(&self, tool: &ToolId, args: &Value) -> Option<GuardrailBlock> {
        if self.config.level != LoopGuardrailLevel::Enforce {
            return None;
        }
        let sig = signature(tool, args);
        let tool_name = tool.to_string();
        let state = self.state.lock().ok()?;
        let stop = &self.config.hard_stop_after;

        if let Some(&count) = state.exact_failures.get(&sig)
            && count >= stop.exact_failure
        {
            return Some(GuardrailBlock {
                code: GuardrailCode::ExactFailureRepeat,
                count,
                message: format!(
                    "This exact {tool_name} call already failed {count} times. It was not \
                     executed again. Change the arguments or the approach, or ask the user how \
                     to proceed."
                ),
            });
        }
        if let Some(&(_, count)) = state.no_progress.get(&sig)
            && count >= stop.no_progress
        {
            return Some(GuardrailBlock {
                code: GuardrailCode::NoProgressRepeat,
                count,
                message: format!(
                    "This {tool_name} call already returned the identical result {count} times. \
                     It was not executed again. Use the result you already have or try something \
                     different."
                ),
            });
        }
        if let Some(&count) = state.tool_failures.get(&tool_name)
            && count >= stop.same_tool_failure
        {
            return Some(GuardrailBlock {
                code: GuardrailCode::SameToolFailures,
                count,
                message: format!(
                    "I stopped retrying {tool_name} because it hit the tool-call guardrail \
                     ({count} failed attempts this cycle). Tell me how you'd like to proceed."
                ),
            });
        }
        None
    }

    /// Record a completed call and return a warning to append to its
    /// result when a warn threshold is reached. `idempotent` gates the
    /// no-progress tracker (read-only tools only); `result_text` feeds
    /// the identical-result hash (pass `None` for non-text results).
    pub fn record_after_call(
        &self,
        tool: &ToolId,
        args: &Value,
        failed: bool,
        idempotent: bool,
        result_text: Option<&str>,
    ) -> Option<GuardrailWarning> {
        let sig = signature(tool, args);
        let tool_name = tool.to_string();
        let mut state = self.state.lock().ok()?;
        let warn = &self.config.warn_after;

        if failed {
            let exact = state.exact_failures.entry(sig).or_insert(0);
            *exact += 1;
            let exact_count = *exact;
            let tool_count = {
                let c = state.tool_failures.entry(tool_name.clone()).or_insert(0);
                *c += 1;
                *c
            };
            state.no_progress.remove(&sig);

            if exact_count >= warn.exact_failure {
                return Some(GuardrailWarning {
                    code: GuardrailCode::ExactFailureRepeat,
                    count: exact_count,
                    message: format!(
                        "this exact {tool_name} call has now failed {exact_count} times — do not \
                         repeat it unchanged; change the arguments or the approach"
                    ),
                });
            }
            if tool_count >= warn.same_tool_failure {
                return Some(GuardrailWarning {
                    code: GuardrailCode::SameToolFailures,
                    count: tool_count,
                    message: format!(
                        "{tool_name} has failed {tool_count} times this cycle across different \
                         arguments — reconsider whether this tool fits the task"
                    ),
                });
            }
            return None;
        }

        // Success clears the exact-failure slot for this signature.
        state.exact_failures.remove(&sig);

        if idempotent && let Some(text) = result_text {
            let result_hash = hash_str(text);
            let entry = state.no_progress.entry(sig).or_insert((result_hash, 0));
            if entry.0 == result_hash {
                entry.1 += 1;
            } else {
                *entry = (result_hash, 1);
            }
            let count = entry.1;
            if count >= warn.no_progress {
                return Some(GuardrailWarning {
                    code: GuardrailCode::NoProgressRepeat,
                    count,
                    message: format!(
                        "this {tool_name} call has now returned the identical result {count} \
                         times — the state has not changed; use what you already have"
                    ),
                });
            }
        }
        None
    }
}

/// `(tool, canonical args)` signature. Canonical form sorts object keys
/// recursively so semantically-equal inputs hash identically regardless
/// of key order.
fn signature(tool: &ToolId, args: &Value) -> u64 {
    let mut canonical = String::new();
    write_canonical_json(args, &mut canonical);
    let mut hasher = DefaultHasher::new();
    tool.to_string().hash(&mut hasher);
    canonical.hash(&mut hasher);
    hasher.finish()
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn write_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical_json(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
#[path = "loop_guardrails.test.rs"]
mod tests;
