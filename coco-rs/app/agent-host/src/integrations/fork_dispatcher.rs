//! Production [`ForkDispatcher`] backed by [`SessionRuntime`].
//!
//! D1 / D2: fork callers (`promptSuggestion`, compaction, and memory paths)
//! need to
//! drive a *fresh* [`coco_query::QueryEngine`] without mutating the
//! parent. This module owns that bridge.
//!
//! Constructs an `AgentQueryConfig` from cache-safe params, runs a
//! one-shot turn against a fresh engine, and returns the response text.
//! The contract is threaded through the
//! [`coco_query::forked_agent::ForkDispatcher`] trait so that
//! `app/query` stays free of CLI / runtime types.
//!
//! ## Cache parity
//!
//! The dispatcher reuses [`coco_query::forked_agent::build_query_config`]
//! to derive a config that matches the parent's prompt-cache key
//! (system prompt bytes, model id, fork-context messages). When
//! callers pass `system_prompt_override`, the override replaces
//! `cache.rendered_system_prompt` *before* the parent history is
//! prepended. Cache-sharing callers such as promptSuggestion must pass
//! their prompt as the fork user message and leave this override unset.
//!
//! ## What this is NOT
//!
//! It is not a generic "run another query" helper — it specifically
//! implements the cache-sharing fork contract. AgentTool spawn goes
//! through [`coco_query::QueryEngineAdapter`] (different contract: no
//! cache slot, full child engine lifecycle).

use std::sync::Arc;

use coco_query::QueryEngineConfig;
use coco_query::forked_agent::{
    ForkDispatcher, ForkTranscriptMode, ForkedAgentOptions, ForkedAgentResult,
};
use coco_tool_runtime::AgentSpawnMetadata;
use coco_types::CacheSafeParams;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use crate::session_runtime::SessionHandle;

/// Backed by `SessionHandle` — captures it once, reuses for every dispatch.
/// Cheap to construct; cheap to call.
pub struct SessionRuntimeForkDispatcher {
    session: SessionHandle,
}

impl SessionRuntimeForkDispatcher {
    pub fn new(session: SessionHandle) -> Self {
        Self { session }
    }
}

#[async_trait::async_trait]
impl ForkDispatcher for SessionRuntimeForkDispatcher {
    async fn dispatch(
        &self,
        cache: &CacheSafeParams,
        options: &ForkedAgentOptions,
        prompt: &str,
        system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError> {
        // Derive the AgentQueryConfig shape from the cache slot. This
        // keeps the byte-faithful contract documented on `forked_agent`
        // (skip_cache_write, transcript_mode, max_turns: Some(1) by default).
        let session_id = self.session.current_typed_session_id().await;
        let mut agent_config =
            coco_query::forked_agent::build_query_config(cache, options, &session_id);
        if let Some(system) = system_prompt_override {
            agent_config.system_prompt = system;
        }

        // Resolve the parent runtime config. The fork inherits the
        // parent's tool/sandbox/web_*/feature/role configuration so
        // the child engine sees the same world the parent does.
        let runtime_config = self.session.runtime_config().as_ref();
        let parent_engine_config = self.session.current_engine_config().await;

        // Forks inherit the parent's settings-driven permission rules via the
        // SHARED `ToolAppState.permissions` base (read-through each batch —
        // `build_fork_engine_from_config` passes `None`, so the engine shares
        // `runtime.app_state()`). The config no longer carries the rule maps, so
        // there is nothing to re-resolve here; only the source roots stay on
        // the config for leading-`/` pattern resolution.
        let permission_rule_source_roots =
            crate::permission_rule_loader::permission_rule_source_roots(
                &runtime_config.settings,
                self.session.original_cwd(),
            );

        let sidechain_agent_id = (options.transcript_mode == ForkTranscriptMode::Sidechain)
            .then(|| coco_query::fork_context::auto_agent_id(options.fork_label));

        let engine_config = QueryEngineConfig {
            model_id: cache.model_id.clone(),
            permission_mode: coco_types::PermissionMode::Default,
            permission_rule_source_roots,
            // Forks stay bounded (default single round-trip).
            max_turns: Some(agent_config.max_turns.unwrap_or(1)),
            total_token_budget: None,
            prompt_cache: agent_config.prompt_cache.clone(),
            system_prompt: Some(agent_config.system_prompt.clone()),
            streaming_tool_execution: false,
            tool_config: runtime_config.tool.clone(),
            sandbox_config: runtime_config.sandbox.clone(),
            sandbox_state: self.session.sandbox_state(),
            memory_config: runtime_config.memory.clone(),
            shell_config: runtime_config.shell.clone(),
            active_shell_tool: agent_config.active_shell_tool,
            shell_provider: (agent_config.active_shell_tool
                != coco_types::ActiveShellTool::Disabled)
                .then(|| parent_engine_config.shell_provider.clone())
                .flatten(),
            // Inherit the parent's session-scoped output rewriter so forked
            // side-queries compress Bash output too (mirrors `shell_provider`).
            output_rewriter: (agent_config.active_shell_tool
                != coco_types::ActiveShellTool::Disabled)
                .then(|| parent_engine_config.output_rewriter.clone())
                .flatten(),
            web_fetch_config: runtime_config.web_fetch.clone(),
            web_search_config: runtime_config.web_search.clone(),
            lsp_config: runtime_config.lsp.clone(),
            compact: runtime_config.compact.clone(),
            features: Arc::new(runtime_config.features.clone()),
            skill_overrides: Arc::new(runtime_config.skill_overrides.clone()),
            tool_overrides: runtime_config.tool_overrides.clone(),
            mcp_tool_exposure: parent_engine_config.mcp_tool_exposure,
            mcp_server_tool_exposure: parent_engine_config.mcp_server_tool_exposure.clone(),
            is_non_interactive: true,
            log_assistant_responses: parent_engine_config.log_assistant_responses,
            // Forks are fire-and-forget — no UI to prompt, so a residual `Ask`
            // must fail closed.
            avoid_permission_prompts: true,
            // Parent-parity thinking: `build_query_config` already
            // resolved `options.effort.or(cache.effort)` — the parent's
            // effective wire effort unless the caller deliberately
            // overrode it. Thread it as the engine's explicit level so
            // the fork's thinking params match the parent's byte-for-
            // byte (thinking config keys Anthropic's messages-level
            // cache; divergence re-reads the parent history uncached —
            // PR #18143 class). Effort only, no budget — same model ⇒
            // same `supported_thinking_levels` ladder ⇒ same wire budget.
            thinking_level: agent_config.effort.map(|effort| coco_types::ThinkingLevel {
                effort,
                budget_tokens: None,
                options: std::collections::HashMap::new(),
            }),
            fallback_min_context_window: options.fallback_min_context_window,
            // Per-fork plumbing — thread the canUseTool callback,
            // fork_label, and query_source override onto the child
            // engine config so the tool-call preparer's canUseTool gate
            // (`resolve_can_use_tool_decision`) enforces uniformly and
            // log lines self-identify which fork they belong to.
            can_use_tool: options.can_use_tool.clone(),
            query_source_override: Some(options.query_source.clone()),
            fork_label: Some(options.fork_label),
            // Sub-context isolation primitives applied at the
            // per-call ToolUseContext build site (tool_context.rs
            // reads `fork_isolation` and applies auto agent_id,
            // fresh denial tracking, query_chain_id / query_depth
            // bump, allowed_write_roots fence, and require_can_use_tool).
            fork_isolation: Some(Arc::new({
                let mut iso =
                    coco_query::fork_context::ForkContextOverrides::for_label(options.fork_label);
                iso.query_source = options.query_source.clone();
                iso.agent_id = sidechain_agent_id.clone();
                iso.can_use_tool = options.can_use_tool.clone();
                iso.require_can_use_tool = options.require_can_use_tool;
                iso
            })),
            ..Default::default()
        };

        // Build a fresh engine via the runtime's standard wiring.
        // `wire_engine` installs every per-session subsystem — the fork
        // gets the same hooks / observers / mailbox / agent handle the
        // parent has, which keeps event emission / permission gating
        // consistent across the parent and child.
        //
        // Cancellation: forks are short-lived; honor the caller's
        // override (speculation / compact share parent's abort token
        // so user `Esc` aborts the fork) — fall back to a fresh
        // independent token when the caller didn't supply one.
        let cancel = options.overrides.abort.clone().unwrap_or_default();
        let engine = self
            .session
            .build_fork_engine_from_config(engine_config, session_id.clone(), cancel, None)
            .await;

        let parent_msg_count = agent_config.fork_context_messages.len();
        tracing::debug!(
            fork_label = %options.fork_label,
            query_source = %options.query_source,
            parent_message_count = parent_msg_count,
            "fork dispatch start"
        );

        // Drive the engine. `fork_context_messages` carries the
        // parent's history verbatim (shared via `Arc<Message>`),
        // mirroring the cache-share path. Empty fork-context messages
        // → run with the prompt only (rare; promptSuggestion etc.
        // always pass parent history).
        let mut messages: Vec<std::sync::Arc<coco_messages::Message>> =
            agent_config.fork_context_messages.clone();
        messages.push(std::sync::Arc::new(coco_messages::create_user_message(
            prompt,
        )));
        // Run the child engine as its own executor task — never await it
        // inline. Inline await stacks the fork's entire poll chain (agent
        // loop → turn pipeline → system-reminder orchestrator) on top of
        // the caller's, which overflows the worker stack in debug builds
        // and is additive when forks nest (reactive compaction inside a
        // fork re-enters here; query_depth cap is 16). Spawning restarts
        // poll depth from the executor.
        //
        // `AbortOnDropHandle` preserves inline-await drop semantics:
        // dropping the dispatch future aborts the child instead of
        // detaching it to keep burning tokens. A child panic surfaces as
        // `JoinError` and maps to a fork error so callers degrade (compact
        // falls back to its direct no-tools call) rather than unwinding
        // the session task. The current span is forwarded explicitly —
        // spans do not cross `tokio::spawn`.
        let engine_task = AbortOnDropHandle::new(tokio::spawn(
            async move { engine.run_with_messages_no_events(messages).await }
                .instrument(tracing::Span::current()),
        ));
        let result = engine_task
            .await
            .map_err(|e| format!("fork engine task join: {e}"))
            .and_then(|run| {
                run.map_err(|e| format!("fork engine run_with_messages_no_events: {e}"))
            })
            .map_err(|msg| {
                Box::new(coco_error::PlainError::new(
                    msg,
                    coco_error::StatusCode::Internal,
                )) as coco_error::BoxedError
            })?;

        // Multi-message capture. Strip the parent-history prefix +
        // the user prompt the fork prepended so the caller only sees
        // the fork's own emissions. Slicing an Arc-vec is a vec of
        // pointer bumps — no deep clone of message bodies.
        let fork_messages: Vec<std::sync::Arc<coco_messages::Message>> = result
            .final_messages
            .iter()
            .skip(parent_msg_count + 1) // +1 for the user prompt the fork prepended
            .cloned()
            .collect();

        if let Some(agent_id) = sidechain_agent_id.as_ref().map(coco_types::AgentId::as_str) {
            self.persist_sidechain_transcript(
                agent_id,
                options.fork_label.as_str(),
                &result.final_messages,
                parent_engine_config.mcp_tool_exposure,
            )
            .await;
        }

        tracing::debug!(
            fork_label = %options.fork_label,
            query_source = %options.query_source,
            parent_message_count = parent_msg_count,
            stop_reason = ?result.stop_reason,
            "fork dispatch complete"
        );

        Ok(ForkedAgentResult {
            messages: fork_messages,
            total_usage: result.total_usage,
        })
    }
}

impl SessionRuntimeForkDispatcher {
    async fn persist_sidechain_transcript(
        &self,
        agent_id: &str,
        fork_label: &str,
        messages: &[std::sync::Arc<coco_messages::Message>],
        mcp_tool_exposure: coco_types::McpToolExposure,
    ) {
        if messages.is_empty() {
            return;
        }
        let Some(store) = self.session.current_agent_transcript_store().await else {
            return;
        };
        let session_id = self.session.current_typed_session_id().await;
        let metadata = AgentSpawnMetadata {
            agent_type: fork_label.to_string(),
            worktree_path: None,
            description: Some(agent_id.to_string()),
            killed_by: None,
            mode: None,
            isolation: None,
            mcp_tool_exposure,
        };
        if let Err(e) = store
            .write_agent_metadata(session_id.as_str(), agent_id, &metadata)
            .await
        {
            tracing::debug!(
                error = %e,
                agent_id,
                "fork sidechain metadata write failed"
            );
        }
        if let Err(e) = store
            .append_agent_messages(session_id.as_str(), agent_id, messages)
            .await
        {
            tracing::debug!(
                error = %e,
                agent_id,
                "fork sidechain transcript write failed"
            );
        }
    }
}

/// Convenience: install a [`SessionRuntimeForkDispatcher`] onto
/// `session` post-`build()`. Idempotent — calling twice replaces
/// the previous installation.
pub async fn install(session: SessionHandle) {
    let dispatcher: coco_query::forked_agent::ForkDispatcherRef =
        Arc::new(SessionRuntimeForkDispatcher::new(session.clone()));
    session.attach_fork_dispatcher(dispatcher).await;
}

#[cfg(test)]
#[path = "fork_dispatcher.test.rs"]
mod tests;
