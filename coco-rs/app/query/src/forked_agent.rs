//! Generic post-turn / side-channel forked-agent helper.
//!
//! All callers funnel through the same `query()` engine; coco-rs
//! routes them through this trait + dispatcher.
//!
//! ## The 9 fork variants (per [`coco_types::ForkLabel`])
//!
//! | Label | Purpose | canUseTool policy |
//! |---|---|---|
//! | `PromptSuggestion` | predict next user prompt | deny-all |
//! | `SideQuestion` | `/btw` answer-once side query | deny-all |
//! | `Compact` | `/compact` summarizer | deny-all |
//! | `ExtractMemories` | post-turn memory write | auto-mem (Read/Glob/Grep + read-only Bash + Edit/Write within memory_dir) |
//! | `SessionMemoryAuto` | auto session-memory rebuild | session-mem (Edit only on exact path) |
//! | `SessionMemoryManual` | `/summary` slash | session-mem |
//! | `AgentSummary` | 30s subagent progress snapshot | deny-all |
//! | `AutoDream` | KAIROS long-term consolidation | auto-mem with broader memory_root |
//! | `Speculation` | pre-execute prompt suggestion | 3-boundary (overlay) |
//!
//! ## Cache-parity contract
//!
//! [`ForkedAgentOptions::for_label`] returns the conservative shape that
//! preserves the parent's prompt cache:
//!
//! - `max_turns: Some(1)` — single round-trip
//! - `transcript_mode: Disabled` — no sidechain noise in the parent's
//!   transcript
//! - `skip_cache_write: true` — fire-and-forget; don't pollute the
//!   shared cache with this branch
//! - `effort: None` — leaves thinking config untouched (setting
//!   `effort: 'low'` on prompt-suggestion forks collapsed cache hit
//!   rate from 92.7% → 61% by changing `budget_tokens` and busting
//!   the cache key)
//!
//! Override these only when cache parity isn't a goal. The per-call
//! `max_output_tokens` lives on `ModelInfo` — to give a fork a
//! different cap, point it at a `ModelInfo` whose
//! `max_output_tokens` / `max_output_tokens_escalate` reflect the
//! intent. There is no per-fork override field by design.

use std::sync::Arc;

use coco_messages::Message;
use coco_tool_runtime::AgentQueryConfig;
use coco_types::CacheSafeParams;
use coco_types::ForkLabel;
use coco_types::TokenUsage;
use tokio_util::sync::CancellationToken;

// Re-export the canUseTool primitives from coco-tool-runtime so fork
// callers can build options without importing the executor crate
// directly.
pub use coco_tool_runtime::CanUseToolCallContext;
pub use coco_tool_runtime::CanUseToolDecision;
pub use coco_tool_runtime::CanUseToolHandle;
pub use coco_tool_runtime::CanUseToolHandleRef;
pub use coco_tool_runtime::DecisionReason;
pub use coco_tool_runtime::NoOpCanUseToolHandle;
pub use coco_tool_runtime::deny_all_handle;

/// Transcript persistence policy for a framework-spawned fork.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForkTranscriptMode {
    /// Do not persist the fork transcript anywhere.
    Disabled,
    /// Persist the fork under the session sidechain/subagent transcript area.
    Sidechain,
}

/// Streaming hook fired for each [`Message`] the fork emits during
/// its agent loop. Used by speculation + auto-dream to update the
/// progress UI / append to a per-task ledger live (instead of waiting
/// for the fork to finish).
pub type OnMessageCallback = Arc<dyn Fn(&Message) + Send + Sync>;

/// Caller-supplied isolation overrides.
///
/// All fields are optional; `None` means "use the engine's default".
#[derive(Default, Clone)]
pub struct ForkedAgentOverrides {
    /// Cancellation token to install on the fork's engine. When
    /// `None`, dispatcher allocates a fresh token. Speculation /
    /// compact thread their own so user `Esc` aborts the fork.
    pub abort: Option<CancellationToken>,
}

impl std::fmt::Debug for ForkedAgentOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkedAgentOverrides")
            .field("abort", &self.abort.is_some())
            .finish()
    }
}

/// Runtime knobs for a forked-agent invocation.
///
/// **No `Default` impl** — every fork must declare its [`ForkLabel`]
/// explicitly so telemetry never sees an unattributed `"fork"` string.
/// Use [`ForkedAgentOptions::for_label`] to start from the cache-safe
/// defaults and customize from there.
#[derive(Clone)]
pub struct ForkedAgentOptions {
    /// Hard cap on turns. `Some(1)` is the standard "one-shot" shape.
    pub max_turns: Option<i32>,
    /// Where the fork transcript should be persisted. Fork engines never
    /// write to the parent's main transcript; sidechain mode records
    /// a separate transcript for compact-like forks.
    pub transcript_mode: ForkTranscriptMode,
    /// `true` ⇒ the fork's API request asks the provider not to
    /// write a fresh prompt-cache entry on the last message.
    pub skip_cache_write: bool,
    /// Optional reasoning-effort override. **Setting this busts cache
    /// parity** for older models that don't have adaptive thinking.
    /// Default `None` preserves the parent's cache key.
    pub effort: Option<coco_types::ReasoningEffort>,
    /// Telemetry-only string for cache-break attribution. Defaults to
    /// `fork_label.as_str()` via [`Self::for_label`].
    pub query_source: String,
    /// Typed fork discriminator. Required — tells telemetry / log
    /// readers which fork variant fired without grepping callsites.
    pub fork_label: ForkLabel,
    /// Per-fork tool-execution gate. `Some` installs the callback at
    /// `ToolUseContext.can_use_tool` so the app/query preparer runs it
    /// before the static permission evaluator consults the tool's
    /// built-in `check_permissions`. The six policies (deny-all /
    /// auto-mem / session-mem / speculation 3-boundary) live in their
    /// respective subsystems; this module just threads the handle
    /// through.
    pub can_use_tool: Option<CanUseToolHandleRef>,
    /// When `true`, hook auto-approve cannot bypass [`Self::can_use_tool`].
    /// Speculation needs this so overlay path-rewrites always run
    /// regardless of hook config.
    pub require_can_use_tool: bool,
    /// Streaming message callback. Fires once per message the fork
    /// emits — live progress for speculation + auto-dream UIs.
    pub on_message: Option<OnMessageCallback>,
    /// Optional state overrides — see [`ForkedAgentOverrides`].
    pub overrides: ForkedAgentOverrides,
}

impl std::fmt::Debug for ForkedAgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkedAgentOptions")
            .field("max_turns", &self.max_turns)
            .field("transcript_mode", &self.transcript_mode)
            .field("skip_cache_write", &self.skip_cache_write)
            .field("effort", &self.effort)
            .field("query_source", &self.query_source)
            .field("fork_label", &self.fork_label)
            .field("can_use_tool_set", &self.can_use_tool.is_some())
            .field("require_can_use_tool", &self.require_can_use_tool)
            .field("on_message_set", &self.on_message.is_some())
            .field("overrides", &self.overrides)
            .finish()
    }
}

impl ForkedAgentOptions {
    /// Build options with the cache-parity-safe defaults for `label`.
    ///
    /// Defaults: `max_turns=Some(1)`, `transcript_mode=Disabled`,
    /// `skip_cache_write=true`, `effort=None`, `can_use_tool=None`,
    /// `require_can_use_tool=false`. `query_source` defaults to
    /// `label.as_str()` so telemetry strings stay aligned with the
    /// typed enum.
    pub fn for_label(label: ForkLabel) -> Self {
        Self {
            max_turns: Some(1),
            transcript_mode: ForkTranscriptMode::Disabled,
            skip_cache_write: true,
            effort: None,
            query_source: label.as_str().to_string(),
            fork_label: label,
            can_use_tool: None,
            require_can_use_tool: false,
            on_message: None,
            overrides: ForkedAgentOverrides::default(),
        }
    }
}

/// Build a one-shot [`AgentQueryConfig`] from cached parent params +
/// caller-provided options.
///
/// The returned config's `model` matches the parent's, `system_prompt`
/// matches the parent's pre-rendered bytes, `fork_context_messages`
/// carries the parent's serialized post-turn history, and the
/// canUseTool / fork_label / max_output_tokens fields are threaded
/// through so the engine sees the per-fork policy.
pub fn build_query_config(
    cache: &CacheSafeParams,
    options: &ForkedAgentOptions,
    session_id: &str,
) -> AgentQueryConfig {
    let mut prompt_cache = cache.prompt_cache.clone();
    if let Some(cfg) = prompt_cache.as_mut() {
        cfg.skip_cache_write = options.skip_cache_write;
    }
    AgentQueryConfig {
        system_prompt: cache.rendered_system_prompt.clone(),
        // Real fork identity (parent session + per-fork agent id). The
        // current `ForkDispatcher` builds its own `QueryEngineConfig` and
        // does not read this back, but populating it keeps the config
        // honest and avoids a fabricated test identity in a real run.
        identity: coco_tool_runtime::AgentRunIdentity {
            session_id: session_id.to_string(),
            agent_id: crate::fork_context::auto_agent_id(options.fork_label),
            kind: coco_tool_runtime::AgentRunKind::Fork,
        },
        model_selection: if cache.provider.trim().is_empty() || cache.model_id.trim().is_empty() {
            coco_types::LlmModelSelection::InheritMain
        } else {
            coco_types::LlmModelSelection::Explicit {
                primary: coco_types::ProviderModelSelection {
                    provider: cache.provider.clone(),
                    model_id: cache.model_id.clone(),
                },
            }
        },
        permission_mode: coco_types::PermissionMode::Default,
        permission_prompt_policy: coco_tool_runtime::PermissionPromptPolicy::FailClosed,
        max_turns: options.max_turns,
        prompt_cache,
        // Inherit the parent's history verbatim so the API request's
        // prefix bytes match — this is what enables cache sharing.
        fork_context_messages: cache.fork_context_messages.clone(),
        active_shell_tool: cache.active_shell_tool,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        extra_permission_rules: Vec::new(),
        effort: options.effort,
        // Per-fork policy: thread can_use_tool / fork_label /
        // onto the child engine config so the engine builder reflects
        // them on QueryEngineConfig and ToolUseContext. Empty when not
        // set — preserves pre-canUseTool behavior for callers that
        // haven't migrated.
        can_use_tool: options.can_use_tool.clone(),
        require_can_use_tool: options.require_can_use_tool,
        fork_label: Some(options.fork_label),
        ..Default::default()
    }
}

/// Result of a [`ForkDispatcher::dispatch`] call.
///
/// Carries the full message list. Callers walk `messages` to find
/// the first non-empty assistant text block — model may go
/// "tool→denied→text" across two turns when canUseTool denies.
/// Numeric usage fields are surfaced for telemetry callers; callers
/// that only want the answer can ignore them.
#[derive(Debug, Clone, Default)]
pub struct ForkedAgentResult {
    /// Every assistant + user message produced during the fork (in
    /// emission order). Carries the engine's authoritative
    /// `Arc<Message>` so callers walk the same allocations the
    /// engine wrote, no deep clone at the dispatcher boundary.
    /// Empty when the fork errored before producing any output.
    pub messages: Vec<Arc<Message>>,
    /// Accumulated token usage across the fork's turns.
    pub total_usage: TokenUsage,
}

/// Async trait for dispatching a one-shot forked query.
///
/// Implementations capture whatever they need to build a fresh
/// [`coco_query::QueryEngine`] (typically `Arc<SessionRuntime>` in
/// the CLI) and drive a single turn against it. The parent engine's
/// history is *not* mutated — that's the whole point of forking.
#[async_trait::async_trait]
pub trait ForkDispatcher: Send + Sync {
    /// Run a forked query.
    ///
    /// `cache` is the parent's [`CacheSafeParams`] (typically read
    /// from `QueryEngine::last_cache_safe_params`). `prompt` is the
    /// new user message to append after the cached parent history.
    /// `system_prompt_override` lets non-cache-sharing callers
    /// substitute a different system prompt; cache-sharing callers
    /// should pass `None` so `cache.rendered_system_prompt` is used.
    async fn dispatch(
        &self,
        cache: &CacheSafeParams,
        options: &ForkedAgentOptions,
        prompt: &str,
        system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError>;
}

/// Convenience reference type matching the rest of the engine's
/// trait-object slots.
pub type ForkDispatcherRef = Arc<dyn ForkDispatcher>;

#[cfg(test)]
#[path = "forked_agent.test.rs"]
mod tests;
