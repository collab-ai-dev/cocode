//! Screening seam for subagent dispatches that never reach the tool pipeline.
//!
//! In auto mode every model-issued tool call is classified before it runs, so
//! an `Agent(prompt, subagent_type)` call is screened before it spawns. A
//! workflow script's `agent()` is a *script* call into a host function: it is
//! not a tool call, so it never reaches that pipeline. Without this seam a
//! workflow — approved once, then free to compute prompts at runtime from its
//! args, from files, or from a previous agent's output — dispatches subagents
//! that the equivalent `Agent` call could not.
//!
//! Implementors MUST decide by rebuilding the equivalent `Agent` tool call and
//! running it through the same auto-mode decision the real call would hit.
//! Sharing that decision is the point: it is what structurally stops the two
//! dispatch paths from drifting apart again.
//!
//! This screen is a second layer, not the only one — a spawned subagent
//! inherits auto mode, so its own tool calls stay classified either way.

use std::sync::Arc;

use coco_messages::Message;
use coco_types::ToolPermissionContext;

/// A subagent dispatch awaiting screening.
pub struct SubagentDispatch<'a> {
    /// The prompt the subagent would receive.
    pub prompt: &'a str,
    /// Requested agent type, when the caller named one.
    pub subagent_type: Option<&'a str>,
    /// Requested structured-output schema. It reaches the classifier as prompt
    /// text, so implementors bound it before including it.
    pub output_schema: Option<&'a serde_json::Value>,
    /// The dispatching turn's permission context. The screen reads its mode
    /// and rules, exactly as the Agent tool's own permission check does.
    pub permission_context: &'a ToolPermissionContext,
    /// The dispatching agent's transcript, so the classifier judges the
    /// request in the context that produced it.
    pub messages: &'a [Arc<Message>],
    /// Effective working directory, for the classifier's path-safety context.
    pub cwd: Option<&'a str>,
}

/// The screen's verdict.
pub enum SubagentDispatchVerdict {
    /// Dispatch may proceed.
    Allow,
    /// Dispatch is refused. The caller drops the slot — it does NOT raise a
    /// script error.
    Block { reason: String },
}

/// Decides whether a non-tool-call subagent dispatch may proceed.
///
/// The mode gate lives in the implementor, not the caller: every policy
/// decision stays on one side of the seam, so a caller never has to reason
/// about permission modes and a second caller inherits the behaviour.
#[async_trait::async_trait]
pub trait SubagentDispatchScreen: Send + Sync {
    async fn screen(&self, dispatch: SubagentDispatch<'_>) -> SubagentDispatchVerdict;
}

/// Shared handle type for `ToolUseContext`.
pub type SubagentDispatchScreenHandle = Arc<dyn SubagentDispatchScreen>;

/// Allows every dispatch — the default when no classifier is wired (tests,
/// embedders without a model runtime).
///
/// Allowing is the right default because the screen is defence in depth over
/// per-call classification, which still runs inside the spawned subagent. A
/// fail-closed default would instead break every context that never opted into
/// auto mode.
pub struct NoOpSubagentDispatchScreen;

#[async_trait::async_trait]
impl SubagentDispatchScreen for NoOpSubagentDispatchScreen {
    async fn screen(&self, _dispatch: SubagentDispatch<'_>) -> SubagentDispatchVerdict {
        SubagentDispatchVerdict::Allow
    }
}
