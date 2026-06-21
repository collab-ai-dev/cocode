//! Bridge: `AgentQueryEngine` (tool-runtime) → `AgentExecutionEngine`
//! (coordinator's runner-loop trait).
//!
//! Rust has both an `AgentQueryEngine` (subagent path, owned by
//! `coco-query`) and an `AgentExecutionEngine` (teammate runner-loop
//! path, owned by `coco-coordinator`). This bridge lets the same engine
//! drive both.
//!
//! Lives inside `coco-coordinator` so SwarmAgentHandle can install the
//! same `Arc<dyn AgentQueryEngine>` it already holds for subagent
//! spawns and have it satisfy the teammate-loop contract.

use std::sync::Arc;

use async_trait::async_trait;

use crate::runner_loop::AgentExecutionEngine;
use crate::runner_loop::AgentQueryConfig as RunnerAgentQueryConfig;
use crate::runner_loop::AgentQueryResult as RunnerAgentQueryResult;

/// Adapter that lets a `coco_tool_runtime::AgentQueryEngine` drive the
/// in-process teammate loop via the coordinator's
/// [`AgentExecutionEngine`] trait.
pub struct TeammateExecutionAdapter {
    inner: coco_tool_runtime::AgentQueryEngineRef,
}

impl TeammateExecutionAdapter {
    pub fn new(inner: coco_tool_runtime::AgentQueryEngineRef) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AgentExecutionEngine for TeammateExecutionAdapter {
    async fn run_query(
        &self,
        prompt: &str,
        config: RunnerAgentQueryConfig,
    ) -> crate::Result<RunnerAgentQueryResult> {
        let tool_runtime_config = coco_tool_runtime::AgentQueryConfig {
            system_prompt: config.system_prompt,
            identity: coco_tool_runtime::AgentRunIdentity::new(
                config.session_id,
                config.agent_id,
                coco_tool_runtime::AgentRunKind::Teammate,
            )
            .map_err(crate::CoordinatorError::generic)?,
            model_selection: config.model_selection,
            permission_mode: config
                .permission_mode
                .unwrap_or(coco_types::PermissionMode::Default),
            permission_prompt_policy: coco_tool_runtime::PermissionPromptPolicy::PromptAllowed,
            max_turns: config.max_turns,
            allowed_tools: config.allowed_tools,
            disallowed_tools: config.disallowed_tools,
            extra_permission_rules: config.extra_permission_rules,
            live_permission_rules: config.live_permission_rules,
            live_permission_mode: config.live_permission_mode,
            tool_overrides: config.tool_overrides,
            features: config.features,
            parent_tool_filter: config.parent_tool_filter,
            active_shell_tool: config.active_shell_tool,
            preserve_tool_use_results: config.preserve_tool_use_results,
            cancel: config.cancel,
            bypass_permissions_available: config.bypass_permissions_available,
            fork_context_messages: config.fork_context_messages,
            is_teammate: true,
            is_in_process_teammate: true,
            effort: config.effort,
            use_exact_tools: config.use_exact_tools,
            mcp_servers: config.mcp_servers,
            ..Default::default()
        };

        let result = self
            .inner
            .execute_query(prompt, tool_runtime_config)
            .await
            .map_err(|e| crate::CoordinatorError::generic(format!("{e}")))?;

        Ok(RunnerAgentQueryResult {
            messages: result.messages,
            token_count: result.input_tokens + result.output_tokens,
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
            turns: result.turns,
            tool_use_count: result.tool_use_count as i32,
            cancelled: result.cancelled,
            response_text: result.response_text,
        })
    }

    /// Full LLM compact for teammate history.
    ///
    /// Pipeline:
    /// 1. Apply micro-compact first to drop resolved tool-result
    ///    content before summarization.
    /// 2. Run `coco_compact::compact_conversation` with our own
    ///    summarize callback that issues a no-tools query through
    ///    the wrapped `AgentQueryEngine` and returns the response
    ///    text as the summary body.
    /// 3. Build post-compact messages (boundary marker + summary
    ///    user message + kept recent rounds) and serialise back to
    ///    `Vec<serde_json::Value>` for the runner-loop.
    ///
    /// On any error (deserialisation / engine failure / compact
    /// failure) returns the input unchanged so the runner-loop's
    /// sliding-window safety valve still bounds growth.
    async fn compact_messages(
        &self,
        messages: Vec<std::sync::Arc<coco_messages::Message>>,
        _total_tokens: i64,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Vec<std::sync::Arc<coco_messages::Message>>> {
        const KEEP_RECENT_FOR_MICRO: usize = 5;
        const KEEP_RECENT_ROUNDS_FOR_FULL: usize = 2;

        // `coco_compact::*` consumes owned `Vec<Message>` (it mutates
        // in place during micro-compact). Unwrap the Arc-vec into
        // owned messages — this is the only deep clone in the path
        // and only fires when the auto-compact threshold trips.
        let mut typed: Vec<coco_messages::Message> =
            messages.iter().map(|arc| (**arc).clone()).collect();

        // Step 1 — micro-compact (no-LLM cleanup of resolved tool
        // results). Cheap and always safe; bounds the prompt fed
        // into the summarizer.
        let _ = coco_compact::micro_compact(&mut typed, KEEP_RECENT_FOR_MICRO);

        // Step 2 — full LLM compact via summarize callback that
        // routes through the wrapped engine. Building a fresh
        // closure here per call keeps the borrow simple — the
        // engine ref is `Arc<dyn>` so cloning is cheap.
        let engine = self.inner.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        let summarize = move |attempt: coco_compact::CompactSummaryAttempt| {
            let engine = engine.clone();
            let session_id = session_id.clone();
            let agent_id = agent_id.clone();
            async move {
                // Summarization sub-run scoped to the teammate's parent
                // session with a derived child agent id. On a bad identity
                // (e.g. empty session) the closure errors and the caller
                // falls back to the micro-compacted history.
                let identity = coco_tool_runtime::AgentRunIdentity::new(
                    session_id,
                    format!("{agent_id}-compact"),
                    coco_tool_runtime::AgentRunKind::Summary,
                )
                .map_err(|e| format!("compact summary identity: {e}"))?;
                let cfg = coco_tool_runtime::AgentQueryConfig {
                    system_prompt: String::new(),
                    identity,
                    model_selection: coco_types::LlmModelSelection::InheritMain,
                    permission_mode: coco_types::PermissionMode::Default,
                    permission_prompt_policy: coco_tool_runtime::PermissionPromptPolicy::FailClosed,
                    max_turns: Some(1),
                    max_output_tokens: Some(attempt.max_summary_tokens),
                    // `CompactSummaryAttempt.context_messages` is already
                    // `Vec<Arc<Message>>` — Arc-share into the fork context
                    // (refcount bump, no `Message::clone`).
                    fork_context_messages: attempt.context_messages.clone(),
                    // Deliberately impossible allow-list: no tools
                    // during the summarization turn. The prompt itself
                    // is separate from structured fork context so we do
                    // not re-flatten history into a legacy string.
                    allowed_tools: vec![String::new()],
                    is_teammate: true,
                    is_in_process_teammate: true,
                    ..Default::default()
                };
                match engine.execute_query(&attempt.summary_request, cfg).await {
                    Ok(result) => Ok(coco_compact::CompactSummaryResponse {
                        summary: result.response_text.unwrap_or_default(),
                    }),
                    Err(e) => Err(e.to_string()),
                }
            }
        };

        let opts = coco_compact::CompactRunOptions {
            keep_recent_rounds: KEEP_RECENT_ROUNDS_FOR_FULL,
            ..Default::default()
        };

        // After in-place micro-compact, re-wrap the owned slice as an
        // Arc-vec for the canonical Arc-shaped `compact_conversation`
        // entry. One `Arc::new` per kept message — same alloc count as
        // the pre-refactor `Arc::new(msg.clone())` step that used to live
        // here, but the downstream pipeline now Arc-shares throughout.
        let typed_arc: Vec<std::sync::Arc<coco_messages::Message>> =
            typed.into_iter().map(std::sync::Arc::new).collect();
        let compact_result =
            match coco_compact::compact_conversation(&typed_arc, &opts, summarize, None).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "teammate full LLM compact failed; falling back to micro-compact only"
                    );
                    // Return the micro-compacted history rather than
                    // the original — the micro pass still trimmed
                    // tool results which buys some bound. Already Arc-shaped.
                    return Ok(typed_arc);
                }
            };

        // Step 3 — assemble post-compact messages. `build_post_compact_messages`
        // already returns `Vec<Arc<Message>>`, so the runner-loop's preferred
        // shape lands directly with no extra wrap.
        Ok(coco_compact::build_post_compact_messages(&compact_result))
    }
}

/// Convenience: wrap an `AgentQueryEngineRef` for the runner-loop side.
pub fn into_execution_engine(
    inner: coco_tool_runtime::AgentQueryEngineRef,
) -> Arc<dyn AgentExecutionEngine> {
    Arc::new(TeammateExecutionAdapter::new(inner))
}
