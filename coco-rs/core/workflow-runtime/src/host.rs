//! The host callback the engine drives for `agent()`/`workflow()` and progress.
//!
//! The trait lives here (engine crate) so `core/workflow-runtime` stays free of
//! `coco-cli`/`coco-tasks` deps; the concrete impl (backed by `AgentHandle` +
//! `TaskHandle`) lives at a layer that has those handles. Mirrors the
//! callback-handle pattern used by `AgentHandle`/`TaskHandle`.

use serde::Deserialize;
use serde::Serialize;

/// Per-call options for the `agent()` DSL primitive, parsed from the JS opts
/// object. Mirrors the TS `agent(prompt, opts)` signature.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkflowAgentOpts {
    /// Display label for the progress row (defaults to a prompt-derived label).
    pub label: Option<String>,
    /// Phase title this agent belongs to.
    pub phase: Option<String>,
    /// Per-call model id override.
    pub model: Option<String>,
    /// Per-call reasoning effort (`low`/`medium`/`high`/`xhigh`/`max`).
    pub effort: Option<String>,
    /// Subagent type (e.g. `Explore`); `None` → general-purpose.
    pub agent_type: Option<String>,
    /// Optional execution isolation. `worktree` creates an isolated git
    /// worktree; `remote` is accepted for compatibility but not available.
    pub isolation: Option<coco_types::AgentIsolation>,
    /// When present, the subagent is asked to emit a JSON object matching this
    /// JSON Schema (forced StructuredOutput); the result is parsed.
    pub schema: Option<serde_json::Value>,
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

/// Callback surface the engine drives. The implementor bridges to the real
/// subagent system and the task progress channel.
#[async_trait::async_trait]
pub trait WorkflowHost: Send + Sync + 'static {
    /// `agent()` → spawn one subagent and await its result. Returns `Err` with a
    /// human message on failure; the DSL maps that to a rejected promise (so the
    /// surrounding `parallel`/`pipeline` records `null` for that item).
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String>;

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
}
