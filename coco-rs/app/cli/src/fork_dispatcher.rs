//! Production [`ForkDispatcher`] backed by [`SessionRuntime`].
//!
//! D1 / D2: post-turn fork callers (`/btw`, `promptSuggestion`,
//! and future `postTurnSummary` / `extractMemories` paths) need to
//! drive a *fresh* [`coco_query::QueryEngine`] without mutating the
//! parent. This module owns that bridge.
//!
//! Constructs an `AgentQueryConfig` from `lastCacheSafeParams`,
//! runs a one-shot turn against a fresh engine, and returns the
//! response text. The contract is threaded through the
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
//! implements the post-turn cache-sharing contract. AgentTool spawn
//! goes through [`coco_query::QueryEngineAdapter`] (different
//! contract: no cache slot, full child engine lifecycle).

use std::sync::Arc;

use coco_query::QueryEngineConfig;
use coco_query::forked_agent::{
    ForkDispatcher, ForkTranscriptMode, ForkedAgentOptions, ForkedAgentResult,
};
use coco_tool_runtime::AgentSpawnMetadata;
use coco_types::CacheSafeParams;

use crate::session_runtime::SessionRuntime;

/// Backed by `Arc<SessionRuntime>` — captures it once, reuses for
/// every dispatch. Cheap to construct; cheap to call.
pub struct SessionRuntimeForkDispatcher {
    runtime: Arc<SessionRuntime>,
}

impl SessionRuntimeForkDispatcher {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        Self { runtime }
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
        let session_id = self.runtime.current_session_id().await;
        let mut agent_config =
            coco_query::forked_agent::build_query_config(cache, options, &session_id);
        if let Some(system) = system_prompt_override {
            agent_config.system_prompt = system;
        }

        // Resolve the parent runtime config. The fork inherits the
        // parent's tool/sandbox/web_*/feature/role configuration so
        // the child engine sees the same world the parent does.
        let runtime_config = self.runtime.runtime_config.as_ref();
        let parent_engine_config = self.runtime.current_engine_config().await;

        // Forks inherit the parent's settings-driven permission rules via the
        // SHARED `ToolAppState.permissions` base (read-through each batch —
        // `build_fork_engine_from_config` passes `None`, so the engine shares
        // `runtime.app_state`). The config no longer carries the rule maps, so
        // there is nothing to re-resolve here; only the source roots stay on
        // the config for leading-`/` pattern resolution.
        let permission_rule_source_roots =
            crate::permission_rule_loader::permission_rule_source_roots(
                &runtime_config.settings,
                &self.runtime.original_cwd,
            );

        let sidechain_agent_id = (options.transcript_mode == ForkTranscriptMode::Sidechain)
            .then(|| coco_query::fork_context::auto_agent_id(options.fork_label));

        let engine_config = QueryEngineConfig {
            model_id: cache.model_id.clone(),
            permission_mode: coco_types::PermissionMode::Default,
            permission_rule_source_roots,
            context_window: agent_config.context_window.unwrap_or(200_000),
            max_output_tokens: agent_config.max_output_tokens.unwrap_or(16_384),
            // Forks stay bounded (default single round-trip).
            max_turns: Some(agent_config.max_turns.unwrap_or(1)),
            total_token_budget: None,
            prompt_cache: agent_config.prompt_cache.clone(),
            system_prompt: Some(agent_config.system_prompt.clone()),
            streaming_tool_execution: false,
            session_id: session_id.clone(),
            tool_config: runtime_config.tool.clone(),
            sandbox_config: runtime_config.sandbox.clone(),
            sandbox_state: self.runtime.sandbox_state(),
            memory_config: runtime_config.memory.clone(),
            shell_config: runtime_config.shell.clone(),
            active_shell_tool: agent_config.active_shell_tool,
            shell_provider: (agent_config.active_shell_tool
                != coco_types::ActiveShellTool::Disabled)
                .then(|| parent_engine_config.shell_provider.clone())
                .flatten(),
            web_fetch_config: runtime_config.web_fetch.clone(),
            web_search_config: runtime_config.web_search.clone(),
            lsp_config: runtime_config.lsp.clone(),
            compact: runtime_config.compact.clone(),
            features: Arc::new(runtime_config.features.clone()),
            skill_overrides: Arc::new(runtime_config.skill_overrides.clone()),
            tool_overrides: runtime_config.tool_overrides.clone(),
            is_non_interactive: true,
            // Forks are fire-and-forget — no UI to prompt, so a residual `Ask`
            // must fail closed.
            avoid_permission_prompts: true,
            // Fork dispatch is fire-and-forget — model-driven thinking
            // / effort overrides would invalidate the parent cache, so
            // we skip them. `forked_agent::build_query_config` already
            // honors `options.effort` when the caller wants it; we
            // forward that through the engine config below.
            thinking_level: agent_config.effort.map(|effort| coco_types::ThinkingLevel {
                effort,
                budget_tokens: None,
                options: std::collections::HashMap::new(),
            }),
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
            .runtime
            .build_fork_engine_from_config(engine_config, cancel, None)
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
        let result = engine
            .run_with_messages_no_events(messages)
            .await
            .map_err(|e| {
                Box::new(coco_error::PlainError::new(
                    format!("fork engine run_with_messages_no_events: {e}"),
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

        if let Some(agent_id) = sidechain_agent_id.as_deref() {
            self.persist_sidechain_transcript(
                agent_id,
                options.fork_label.as_str(),
                &result.final_messages,
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
    ) {
        if messages.is_empty() {
            return;
        }
        let Some(store) = self.runtime.current_agent_transcript_store().await else {
            return;
        };
        let session_id = self.runtime.current_session_id().await;
        if session_id.is_empty() {
            return;
        }
        let metadata = AgentSpawnMetadata {
            agent_type: fork_label.to_string(),
            worktree_path: None,
            description: Some(agent_id.to_string()),
        };
        if let Err(e) = store
            .write_agent_metadata(&session_id, agent_id, &metadata)
            .await
        {
            tracing::debug!(
                error = %e,
                agent_id,
                "fork sidechain metadata write failed"
            );
        }
        if let Err(e) = store
            .append_agent_messages(&session_id, agent_id, messages)
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
/// `runtime` post-`build()`. Idempotent — calling twice replaces
/// the previous installation.
pub async fn install(runtime: Arc<SessionRuntime>) {
    let dispatcher: coco_query::forked_agent::ForkDispatcherRef =
        Arc::new(SessionRuntimeForkDispatcher::new(runtime.clone()));
    runtime.attach_fork_dispatcher(dispatcher).await;
}

#[cfg(test)]
#[path = "fork_dispatcher.test.rs"]
mod tests;
