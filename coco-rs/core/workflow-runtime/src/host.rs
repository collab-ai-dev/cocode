//! The host callback the engine drives for `agent()`/`workflow()` and progress.
//!
//! The trait lives here (engine crate) so `core/workflow-runtime` stays free of
//! `coco-cli`/`coco-tasks` deps; the concrete impl (backed by `AgentHandle` +
//! `TaskHandle`) lives at a layer that has those handles. Mirrors the
//! callback-handle pattern used by `AgentHandle`/`TaskHandle`.

use serde::Deserialize;
use serde::Serialize;

/// Default per-agent stall window (CC `WORKFLOW_STALL_MS_DEFAULT`, 3 min). A
/// single `agent()` spawn that produces no result within this window is aborted
/// and retried by the host-side watchdog. Overridable per call via
/// [`WorkflowAgentOpts::stall_ms`].
pub const WORKFLOW_STALL_MS_DEFAULT: i64 = 180_000;

/// Maximum per-agent stall retries before the call is reported as failed
/// (CC `WORKFLOW_STALL_RETRY`). On exhaustion the `agent()` call rejects → the
/// surrounding `parallel`/`pipeline` slot becomes `null`.
pub const WORKFLOW_STALL_RETRY: i32 = 5;

/// Per-call options for the `agent()` DSL primitive, parsed from the JS opts
/// object. `agent(prompt, opts)` signature.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkflowAgentOpts {
    /// Display label for the progress row (defaults to a prompt-derived label).
    pub label: Option<String>,
    /// Phase title this agent belongs to.
    pub phase: Option<String>,
    /// Per-call model id override.
    pub model: Option<String>,
    /// Per-call reasoning effort (`low`/`medium`/`high`/`xhigh`/`max`/`ultra`).
    pub effort: Option<String>,
    /// Subagent type (e.g. `Explore`); `None` → general-purpose.
    pub agent_type: Option<String>,
    /// Optional execution isolation. `worktree` creates an isolated git
    /// worktree; `remote` is accepted for compatibility but not available.
    pub isolation: Option<coco_types::AgentIsolation>,
    /// When present, the subagent is asked to emit a JSON object matching this
    /// JSON Schema (forced StructuredOutput); the result is parsed.
    pub schema: Option<serde_json::Value>,
    /// Per-call override for the stall watchdog window, in milliseconds. When
    /// the spawn produces no result within this window the host aborts and
    /// retries it (up to [`WORKFLOW_STALL_RETRY`] times). `None` →
    /// [`WORKFLOW_STALL_MS_DEFAULT`]. Mirrors CC's `opts.stallMs`.
    pub stall_ms: Option<i64>,
}

/// Cache key for resume replay. The engine builds one per `agent()` call from
/// the run-global call ordinal, the prompt, and the canonicalized
/// cache-relevant opts; the HOST hashes it
/// (`version:sha256(call_index \0 prompt \0 canonical_opts)`) before consulting
/// the journal. Keeping the hashing host-side lets the host pick the digest +
/// version without pulling a crypto dep into the engine crate.
///
/// **Why the ordinal is in the key.** Without it a script that calls `agent()`
/// with the same prompt more than once — every `loop-until-count` and
/// `loop-until-dry` pattern the tool prose teaches, and any `parallel()` fan-out
/// over one prompt — produces a single journal key, so a resumed run replays one
/// recorded value for every one of those calls. The ordinal makes each call
/// site's key distinct while preserving the longest-unchanged-prefix contract:
/// editing, inserting or deleting a call shifts every later ordinal, so the
/// first changed call misses and the divergence latch re-runs the tail. (CC gets
/// the same property from a chained prefix hash; the ordinal is the same
/// guarantee without carrying chain state across the nesting boundary.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCacheKey {
    /// 0-based lifetime `agent()` ordinal within the run, shared with nested
    /// child workflows.
    pub call_index: i32,
    /// The verbatim `agent()` prompt.
    pub prompt: String,
    /// Deterministic serialization of the cache-relevant opts whitelist
    /// (`schema`, `model`, `effort`, `isolation`, `agentType`) with sorted keys.
    /// Built via [`canonical_agent_opts`] so two calls with the same opts in a
    /// different key order yield the same string (and thus the same hash).
    pub canonical_opts: String,
}

impl AgentCacheKey {
    /// Build a cache key from the resolved call inputs.
    pub fn new(call_index: i32, prompt: String, opts: &WorkflowAgentOpts) -> Self {
        Self {
            call_index,
            prompt,
            canonical_opts: canonical_agent_opts(opts),
        }
    }
}

/// The cache-relevant `agent()` opts whitelist (CC `canonicalizeAgentOpts`):
/// only these fields participate in the resume cache key, so cosmetic opts
/// (`label`, `phase`, `stall_ms`) never change which cached result is replayed.
/// The key hashes what changes the agent's **output**, not its presentation —
/// relabelling a step or regrouping the progress tree between runs must not
/// re-spawn it.
const CACHE_OPTS_WHITELIST: [&str; 5] = ["schema", "model", "effort", "isolation", "agentType"];

/// Canonicalize the cache-relevant opts into a deterministic string. Serializes
/// the whitelisted fields, recursively sorts every object's keys, and renders
/// the result as compact JSON — so the same logical opts always produce the same
/// string regardless of the JS key order. Mirrors CC's `canonicalizeAgentOpts`.
pub fn canonical_agent_opts(opts: &WorkflowAgentOpts) -> String {
    // Round-trip the whole opts struct through serde_json so each field uses its
    // wire (camelCase) name, then keep only the whitelist and sort recursively.
    let full = serde_json::to_value(opts).unwrap_or(serde_json::Value::Null);
    let mut picked = serde_json::Map::new();
    if let serde_json::Value::Object(map) = full {
        for field in CACHE_OPTS_WHITELIST {
            if let Some(value) = map.get(field) {
                picked.insert(field.to_string(), sort_value(value.clone()));
            }
        }
    }
    // `serde_json::Map` preserves insertion order; we inserted in whitelist
    // order, so the rendering is deterministic. Nested objects were sorted by
    // `sort_value`.
    serde_json::Value::Object(picked).to_string()
}

/// Recursively rewrite every object so its keys are in sorted order, giving a
/// canonical form independent of the original key order.
fn sort_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let sorted: serde_json::Map<String, serde_json::Value> = entries
                .into_iter()
                .map(|(k, v)| (k, sort_value(v)))
                .collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sort_value).collect())
        }
        other => other,
    }
}

/// The result of one `agent()` call: the subagent's final value (a string for a
/// plain run, or the parsed JSON object when `schema` was set).
#[derive(Debug, Clone)]
pub struct WorkflowAgentResult {
    pub value: serde_json::Value,
    pub model: Option<String>,
    pub tokens: Option<i64>,
    pub tool_calls: Option<i32>,
    pub duration_ms: Option<i64>,
}

/// How one `agent()` call ended.
///
/// The third state is the point: a dispatch can be *refused before it runs*,
/// which is neither a result nor a script error. `Err` from
/// [`WorkflowHost::run_agent`] rejects the promise, and a policy refusal must
/// not blow up a script that never did anything wrong — so a refusal resolves
/// to `null`, exactly like a slot that failed inside `parallel()`.
#[derive(Debug, Clone)]
pub enum WorkflowAgentOutcome {
    /// The subagent ran and produced a value.
    Completed(WorkflowAgentResult),
    /// Refused before any spawn — the auto-mode dispatch screen, or a user
    /// skip. `agent()` resolves to `null`.
    Refused {
        /// Human-readable cause, surfaced on the progress row and in the log.
        reason: String,
        /// Refused by policy rather than by a person. Drives the completion
        /// census, which must not read a policy block as an agent failure.
        blocked: bool,
    },
}

/// Signals that a dispatch stopped waiting for a concurrency permit and is
/// about to run.
///
/// A fan-out of 200 `agent()` calls has 200 rows the instant the script fires
/// them, but only as many as the host's concurrency width are actually running;
/// the rest are queued. Downstream that distinction is carried by one thing —
/// whether the row has a start timestamp — and the host is the only place that
/// knows when the wait ended. The engine keeps ownership of the row's shape and
/// hands the host this callback to flip it.
pub type WorkflowAgentStarted<'a> = &'a dyn Fn();

/// Callback surface the engine drives. The implementor bridges to the real
/// subagent system and the task progress channel.
///
/// `?Send`: the engine is `!Send` and drives the host on its dedicated
/// current-thread runtime / `LocalSet`, so host futures are awaited on a single
/// thread. This is what lets [`WorkflowHost::run_nested_workflow`] re-enter the
/// `!Send` [`WorkflowEngine`](crate::WorkflowEngine) inline. The trait OBJECT is
/// still `Send + Sync` so `Arc<dyn WorkflowHost>` can be constructed and shared.
#[async_trait::async_trait(?Send)]
pub trait WorkflowHost: Send + Sync + 'static {
    /// `tool(name, input)` → execute one provably read-only tool call through
    /// the application's canonical tool pipeline. The default keeps embedders
    /// fail-closed until they explicitly wire the capability.
    async fn run_tool(
        &self,
        _tool_name: String,
        _input: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err("programmatic tool calls are not available".into())
    }

    /// `agent()` → spawn one subagent and await its result.
    ///
    /// `Err` is a *failure* (the subagent errored, stalled out, or could not be
    /// built) and rejects the promise, so the surrounding `parallel`/`pipeline`
    /// records `null` for that slot. A dispatch the host *refuses* — policy
    /// screen, user skip — is `Ok(Refused)` instead, which resolves to `null`
    /// without raising into the script.
    ///
    /// The implementor MUST call `started` once the dispatch has taken its
    /// concurrency permit and is about to run — see [`WorkflowAgentStarted`].
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
        started: WorkflowAgentStarted<'_>,
    ) -> Result<WorkflowAgentOutcome, String>;

    /// Emit one progress delta (phase / log / agent state). Synchronous and
    /// non-blocking (fire into a channel) so `log()`/`phase()` stay sync JS
    /// functions matching the TS DSL.
    fn push_progress(&self, event: coco_types::WorkflowProgressEvent);

    /// Total token budget available to this workflow, when known.
    fn budget_total_tokens(&self) -> Option<i64> {
        None
    }

    /// Tokens consumed by workflow child agents so far.
    fn budget_spent_tokens(&self) -> i64 {
        0
    }

    /// Record tokens consumed by a completed child agent.
    fn record_agent_tokens(&self, _tokens: i64) {}

    /// Whether the token budget is exhausted: a positive `total` is set and the
    /// spent total has reached it. The engine consults this *before* each
    /// `agent()` call and throws when true, so the next call rejects (degrading
    /// its `parallel`/`pipeline` slot to `null`). In-flight agents finish and
    /// their results are preserved.
    fn budget_exhausted(&self) -> bool {
        false
    }

    /// Resume cache lookup: `Some(value)` on a journal hit (the prior run
    /// completed this exact `agent()` call), `None` on a miss. The engine
    /// consults this *only while the replay cursor has not diverged* (see
    /// [`WorkflowRunState::replay_allowed`](crate::WorkflowRunState::replay_allowed));
    /// a hit short-circuits the spawn and emits a `cached: true` progress event.
    /// Default no-op (no journal ⇒ always miss).
    ///
    /// **Synchronous by contract.** The journal is hydrated once at launch, so
    /// a lookup is a map probe. Keeping it off the await path means replay
    /// decisions — including the divergence latch — happen in `agent()` call
    /// order even when the calls were fired concurrently by `parallel()`.
    fn cached_agent_result(&self, _key: &AgentCacheKey) -> Option<serde_json::Value> {
        None
    }

    /// Record a completed `agent()` result into the resume journal so a future
    /// `resumeFromRunId` replays it instead of re-spawning. Called after a
    /// successful spawn that was NOT served from cache. Default no-op.
    async fn record_agent_result(&self, _key: &AgentCacheKey, _value: &serde_json::Value) {}

    /// `workflow(nameOrRef, args)` → run a saved or `{scriptPath}` child workflow
    /// inline, sharing this run's governance: the same concurrency semaphore,
    /// token budget, journal and abort signal (from the SAME host Arc) plus the
    /// same agent ordinal, phase table and replay cursor (from the SAME
    /// [`WorkflowRunState`](crate::WorkflowRunState)). The implementor MUST
    /// forward both — re-entering with fresh state would let a child restart the
    /// lifetime agent cap and collide with the parent's progress rows. `depth`
    /// is the child engine's depth (the parent installs `workflow()` at depth 0,
    /// so the child is invoked at depth 1); the child engine installs a throwing
    /// `workflow()`, enforcing the one-level nesting limit. Returns the child's
    /// resolved value, or `Err` (unknown name, unreadable scriptPath, child syntax
    /// error, child runtime failure) which the JS `workflow()` rejects with so the
    /// parent script can `catch` it. Default: nesting unsupported.
    async fn run_nested_workflow(
        &self,
        _name_or_ref: String,
        _args: serde_json::Value,
        _depth: i32,
    ) -> Result<serde_json::Value, String> {
        Err("nested workflows not supported".into())
    }
}

#[cfg(test)]
#[path = "host.test.rs"]
mod tests;
