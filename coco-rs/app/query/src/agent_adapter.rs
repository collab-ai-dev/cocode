//! Agent query adapter — bridges QueryEngine to AgentQueryEngine trait.
//!
//! Adapts the existing QueryEngine to provide subagent query execution
//! via the AgentQueryEngine trait.
//!
//! **Dependency flow**:
//! ```text
//! coco-tool-runtime (defines AgentQueryEngine trait)
//! ↓
//! coco-query (this adapter implements it via QueryEngine)
//! ↓
//! coco-state (SwarmAgentHandle / InProcessTeammateRunner consumes it)
//! ```

use std::sync::Arc;

use coco_tool_runtime::AgentQueryConfig;
use coco_tool_runtime::AgentQueryEngine;
use coco_tool_runtime::AgentQueryResult;
use coco_tool_runtime::PermissionPromptPolicy;
use coco_types::Features;
use coco_types::LlmModelSelection;
use coco_types::ThinkingLevel;
use coco_types::ToolFilter;
use coco_types::ToolOverrides;
use tokio_util::sync::CancellationToken;

use crate::engine::QueryEngine;
use crate::engine::QueryEngineConfig;

/// Factory function type for creating QueryEngine instances.
/// Each agent query gets a fresh engine with its own config plus a
/// typed model selection that the factory uses to select the right
/// runtime source. `InheritMain` defaults to the parent session's model
/// unless the agent definition specifies a model.
/// The factory is async because production implementations (see
/// `app/cli/src/agent_handle_factory.rs`) need to call into the
/// session runtime's role-client resolver and engine builder, both
/// of which are async. The adapter calls `(factory)(cfg, role).await`
/// from inside `execute_query`, which itself runs in an async context
/// — see `coco_query::agent_adapter::QueryEngineAdapter::execute_query`.
pub type QueryEngineFactory = Arc<
    dyn Fn(
            QueryEngineConfig,
            LlmModelSelection,
            Option<CancellationToken>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = QueryEngine> + Send>>
        + Send
        + Sync,
>;

/// Adapter that wraps QueryEngine to implement AgentQueryEngine.
/// Each subagent gets its own `QueryEngineAdapter` with a dedicated
/// QueryEngine instance configured for the agent's model, tools, and budget.
pub struct QueryEngineAdapter {
    /// Factory function to create QueryEngine instances per query.
    engine_factory: QueryEngineFactory,
}

impl QueryEngineAdapter {
    pub fn new(engine_factory: QueryEngineFactory) -> Self {
        Self { engine_factory }
    }
}

#[async_trait::async_trait]
impl AgentQueryEngine for QueryEngineAdapter {
    async fn execute_query(
        &self,
        prompt: &str,
        config: AgentQueryConfig,
    ) -> Result<AgentQueryResult, coco_error::BoxedError> {
        let identity = config.identity.clone();
        let permission_mode = config.permission_mode;
        let model_selection = config.model_selection.clone();
        let engine_model_id = model_selection.display_model_id().unwrap_or_default();
        // Per-engine ISOLATED additions (fork-mode skill `allowed-tools`,
        // Command-source). Seeded into the engine's `live_command_rules`; they
        // never touch the shared app_state base (TS no-op setAppState parity).
        let initial_command_rules = config.extra_permission_rules.clone();
        // `createSubagentContext` + `agentGetAppState` derivation: the
        // subagent read-through-inherits the parent's shared base (deny/ask/
        // allow/mode), and this layers the per-engine deltas on top.
        let permission_derivation = crate::config::PermissionDerivation {
            // `allowed_tools` is the registry VISIBILITY filter (→ `tool_filter`
            // below), NOT a permission-allow source. TS only does allowedTools
            // replace-on-restrict for the SEPARATE SDK `allowedTools` permission
            // param (); the ordinary AgentTool `tools:`
            // frontmatter only narrows visibility. Deriving allow rules from it
            // both over-permits (auto-allows the listed tools) and under-permits
            // (drops the parent's non-CliArg allow sources). So leave this `None`
            // here — the subagent inherits the parent's full allow read-through,
            // and the tool filter handles restriction. (Reserved for a future
            // dedicated SDK-allowedTools field on `AgentQueryConfig`.)
            allowed_tools_replace: None,
            // Agent-definition `permissionMode` override (already resolved against
            // parent precedence by `resolve_subagent_mode` at the AgentTool
            // layer). The factory applies it unless the parent's live mode is
            // Bypass/AcceptEdits/Auto. Without this the
            // resolved mode is lost (base.mode = parent's live mode wins).
            mode_override: Some(permission_mode),
            // Parent cwd bridge + inherited read dirs (worktree-isolated child
            // reads the parent project). Layered on the inherited base dirs.
            extra_additional_dirs: inherited_read_dirs_to_additional_dirs(
                &config.inherited_read_dirs,
            ),
        };

        let engine_config = QueryEngineConfig {
            // A subagent uses its own configured turn cap, or runs
            // unbounded (None) when unset — same as the main loop. The shared
            // token-budget / continuation cap / interrupt still bound it.
            max_turns: config.max_turns,
            total_token_budget: None,
            prompt_cache: config.prompt_cache.clone(),
            system_prompt: Some(config.system_prompt),
            append_system_prompt: None,
            model_id: engine_model_id,
            permission_mode,
            permission_mode_availability: config.permission_mode_availability,
            use_auto_mode_during_plan: config.use_auto_mode_during_plan,
            // This child engine's OWN run depth (= parent + 1, stamped at
            // the AgentTool boundary). For plain spawns the tool-context
            // builder reads this directly; for forks it is overridden by
            // `fork_isolation.child_query_depth()` so the fork counts toward
            // the same nesting cap.
            query_depth: config.child_query_depth,
            max_budget_usd: None,
            // Subagents inherit no output-token budget (reminder stays dormant);
            // the caller sets it explicitly when desired, same as the main loop.
            output_token_budget: None,
            requires_structured_output: false,
            max_structured_output_retries: crate::config::max_structured_output_retries(),
            streaming_tool_execution: true,
            is_non_interactive: true,
            // Hardcoded for ALL subagents: a residual `Ask` fails closed
            // (deny) since coco has no parent-terminal prompt routing for
            // child engines. `bubble`-mode subagents bubble the prompt to
            // the parent terminal; `avoid_permission_prompts` is only set
            // for async subagents. The type defines `PermissionMode::Bubble`
            // but does not yet route subagent prompts upward, so
            // unconditional fail-closed is the correct (fail-safe) choice.
            // Make this conditional on `permission_mode != Bubble` only
            // once that routing lands — doing so earlier would turn a clean
            // deny into a dangling Ask.
            avoid_permission_prompts: matches!(
                config.permission_prompt_policy,
                PermissionPromptPolicy::FailClosed
            ),
            // Subagents inherit the parent's debug/verbose surface only
            // when the parent piped that into `AgentQueryConfig`; today
            // we don't propagate, so default to `false`.
            debug: false,
            verbose: false,
            // Subagent reasoning-effort override. The resolver in
            // `core/subagent/src/spawn_resolution.rs` carries the effort
            // string forward; here we parse it into a `ThinkingLevel` so
            // the engine threads it into `QueryParams.thinking_level` →
            // `PerCallOverrides`. An unrecognized string degrades to `None`
            // (the model's `default_thinking_level` from `ModelInfo` then
            // applies) rather than failing the spawn.
            // `config.effort` is the typed `ReasoningEffort` discriminator
            // selecting one entry from the resolved model's
            // `supported_thinking_levels`. The build path lives at
            // `session_runtime::thinking_level_for_effort_from` and is
            // model-aware (different `budget_tokens` per model). At this
            // engine-config layer the budget hasn't been resolved yet —
            // we just thread the categorical level (no budget, default
            // options) and let the downstream apply model-relative
            // overrides where they exist.
            // `None` means the spawn states no per-call effort (layer 1),
            // so it falls through to the resolved role runtime's per-slot
            // effort (layer 2 — e.g. `models.explore.effort`), then the
            // model default (layer 3), then the provider default. A
            // subagent that wants thinking off declares `effort: off` in
            // its definition; this layer never synthesizes an `Off`. An
            // explicit per-spawn `effort` (definition frontmatter or the
            // AgentTool input) still wins as the layer-1 override.
            thinking_level: config.effort.map(|effort| ThinkingLevel {
                effort,
                budget_tokens: None,
                options: std::collections::HashMap::new(),
            }),
            fast_mode: false,
            fallback_min_context_window: None,
            session_id: identity.session_id.clone(),
            project_dir: None,
            // Subagent base rules come from the shared parent `app_state`
            // (read-through, the factory reads it each batch — TS
            // `createSubagentContext` parity). Per-engine isolated additions flow
            // through `initial_command_rules`, and the read-through deltas
            // (allowedTools replace + extra dirs) through `permission_derivation`.
            live_permission_rules: config.live_permission_rules.clone(),
            live_permission_mode: config.live_permission_mode.clone(),
            permission_rule_source_roots: Default::default(),
            initial_command_rules,
            permission_derivation: Some(permission_derivation),
            // Propagate the subagent's cwd_override (set by worktree
            // isolation or explicit `cwd:` input) so the child
            // engine's ToolContextFactory installs it onto every
            // ToolUseContext. Absolute-path tools ignore it; Glob /
            // Grep / Bash operate inside the override.
            cwd_override: config.cwd_override.clone(),
            plans_directory: None,
            agent_id: Some(identity.agent_id.clone()),
            is_teammate: config.is_teammate,
            is_in_process_teammate: config.is_in_process_teammate,
            plan_mode_required: config.plan_mode_required,
            plan_mode_settings: coco_config::PlanModeSettings::default(),
            disable_all_hooks: false,
            allow_managed_hooks_only: false,
            enable_token_budget_continuation: false,
            compact: coco_config::CompactConfig::default(),
            wire_dump: config.wire_dump.clone(),
            system_reminder: coco_config::SystemReminderConfig::default(),
            tool_config: coco_config::ToolConfig::default(),
            sandbox_config: coco_config::SandboxSettings::default(),
            // Subagent spawn path does not yet propagate parent sandbox
            // state — `AgentQueryConfig` carries no slot for it. Children
            // run unsandboxed via this entry point; revisit when
            // teammate/swarm flows need parity with the CLI bootstrap.
            sandbox_state: None,
            memory_config: coco_config::MemoryConfig::default(),
            shell_config: coco_config::ShellConfig::default(),
            active_shell_tool: config.active_shell_tool,
            // Subagent flows don't carry the parent's shell provider
            // (snapshot/session-env/`/env`/shell-prefix). Worktree-isolated
            // subagents set `cwd_override` so the bash tool's spawn already
            // points at the right directory; running without snapshot is
            // an acceptable tradeoff for an isolated transient session.
            shell_provider: None,
            // No session-level CWD persistence for subagents — their cwd
            // is fenced via `cwd_override` and they don't share state
            // with the parent session.
            original_cwd: None,
            session_cwd: None,
            web_fetch_config: coco_config::WebFetchConfig::default(),
            web_search_config: coco_config::WebSearchConfig::default(),
            lsp_config: coco_config::LspConfig::default(),
            // Layer 1 — inherit parent's resolved features. Defaulting
            // to `with_defaults()` would silently re-enable gates the
            // user disabled at the top level (Sandbox, WebSearch, ...).
            // The Option fallback only kicks in when the caller really
            // doesn't have a parent context (no test path takes this
            // branch in production).
            features: config
                .features
                .clone()
                .unwrap_or_else(|| Arc::new(Features::with_defaults())),
            // Layer 2 — inherit parent's resolved tool overrides (filled
            // in by the parent before handing off `AgentQueryConfig`).
            // Falling back to `none()` would WIDEN the set beyond what
            // the active model actually accepts; we'd expose tools the
            // model can't call. The factory may replace this with
            // role-resolved overrides when it builds the child engine.
            tool_overrides: config
                .tool_overrides
                .clone()
                .unwrap_or_else(|| Arc::new(ToolOverrides::none())),
            // Subagents inherit the parent's resolved skill_overrides
            // tiers so they apply the same listing + Skill tool gates
            // for the duration of their fork. Subagents only narrow
            // — they never widen the parent's permission shape.
            skill_overrides: config
                .skill_overrides
                .clone()
                .unwrap_or_else(|| Arc::new(coco_config::SkillOverrideTiers::default())),
            // Layer 4 — derive the subagent's allow/deny from its
            // AgentDefinition, then narrow against the parent's filter
            // so a child's `allowed_tools` cannot widen what the parent
            // restricted. Empty allow + deny ⇒ filter is permissive on
            // the child side, but `narrow_with(parent)` keeps every
            // parent-side restriction.
            tool_filter: {
                let child = ToolFilter::new(config.allowed_tools, config.disallowed_tools);
                match &config.parent_tool_filter {
                    Some(parent) => child.narrow_with(parent),
                    None => child,
                }
            },
            // Sandboxed write fence — propagated as-is. Empty = no fence.
            allowed_write_roots: config.allowed_write_roots.clone(),
            // Subagents inherit the SDK opt-in: stay false by default
            // so background subagent runs don't flood the parent's
            // SDK stream with hook events.
            include_hook_events: false,
            // Per-fork canUseTool plumbing — inherits from
            // AgentQueryConfig so fork-spawned subagents (memory /
            // dream / session services) honour their per-policy
            // callbacks. Other (AgentTool) spawns leave it `None`.
            can_use_tool: config.can_use_tool.clone(),
            query_source_override: None,
            fork_label: config.fork_label,
            // Sub-context isolation for fork-flavored subagent spawns.
            // When `fork_label` is set (memory services: extract /
            // dream / session_memory; agent_summary timer), build a
            // `ForkContextOverrides` so the per-call ToolUseContext
            // builder applies auto agent_id, fresh DenialTracker,
            // query_chain_id / query_depth bump, and write fence.
            // User-invoked AgentTool spawns leave `fork_label = None`
            // and skip isolation (they inherit the parent context).
            fork_isolation: config.fork_label.map(|label| {
                let mut iso = crate::fork_context::ForkContextOverrides::for_label(label);
                iso.can_use_tool = config.can_use_tool.clone();
                iso.require_can_use_tool = config.require_can_use_tool;
                if !config.allowed_write_roots.is_empty() {
                    iso.allowed_write_roots = config.allowed_write_roots.clone();
                }
                // `config.child_query_depth` is `parent + 1`; store the
                // parent depth here and let `child_query_depth()` apply the
                // fork's own +1 at context-build time.
                iso.parent_query_depth = config.child_query_depth.saturating_sub(1);
                std::sync::Arc::new(iso)
            }),
        };

        // Model resolution: the adapter threads the subagent's typed
        // selection through to the factory so concrete provider/model
        // selections use explicit runtimes and role selections install
        // the role-specific runtime.
        tracing::debug!(
            session_id = %identity.session_id,
            agent_id = %identity.agent_id,
            kind = ?identity.kind,
            "agent_adapter: executing child query"
        );

        let mut engine =
            (self.engine_factory)(engine_config, model_selection, config.cancel.clone()).await;
        // D3: install the per-spawn permission bridge if one was
        // threaded through. AgentTool spawns set this so worker tool
        // deny paths forward to the leader instead of failing closed.
        // `None` keeps the factory-default bridge (typically the
        // parent's, installed by `wire_engine`).
        if let Some(bridge) = config.permission_bridge.clone() {
            engine = engine.with_permission_bridge(bridge);
        }

        // Live transcript: when the coordinator attached a summary timer to
        // this spawn, install the shared snapshot sink so each turn-finalize
        // publishes the child's message history to the timer. `None` keeps
        // the engine snapshot-free (main loop, non-summarized spawns).
        if let Some(live) = config.live_transcript.clone() {
            engine = engine.with_live_transcript(live);
        }

        // Per-round usage + cost: install a fresh `CostTracker` so the child
        // emits `SessionUsageUpdated` after every model round. The engine's
        // only token report otherwise rides the single end-of-cycle
        // `TurnEnded`, and `TurnEnded` carries no cost at all — so without
        // this the coordinator's spawn drain can't surface live spend (or even
        // live tokens) on the subagent's activity row. The tracker is private
        // to this child engine; its snapshot reaches only the spawn drain (the
        // child's `event_tx`), never the parent session's usage.
        engine = engine.with_session_usage_tracker(Arc::new(tokio::sync::Mutex::new(
            coco_messages::CostTracker::new(),
        )));

        // Structured-output forcing (workflow `agent(prompt, {schema})`):
        // when the spawn carries an output schema, the child must emit its
        // final answer via the synthetic `StructuredOutput` tool rather than
        // free-form text. Mirror the headless `--json-schema` path
        // (`coco_cli::headless::inject_structured_output_tool_if_requested`):
        // register the compiled tool into a PER-SPAWN tool registry and enable
        // `requires_structured_output` on the child engine. Per-spawn
        // isolation is mandatory — registering on the shared session
        // registries would leak the tool to the parent + siblings.
        if let Some(schema) = config.output_schema.clone()
            && let Err(error) = install_structured_output(&mut engine, schema.as_ref())
        {
            return Err(Box::new(coco_error::PlainError::new(
                format!("structured-output setup failed: {error}"),
                coco_error::StatusCode::Internal,
            )) as coco_error::BoxedError);
        }

        // Fork mode: if the parent surfaced context messages, use
        // `run_with_messages` so the child's first turn sees the
        // parent's history prepended.
        // Caller-supplied `event_tx` lets bg AgentTool spawns
        // observe live `Stream::TextDelta` events (TaskOutput live
        // streaming). When `None`, fall back to a discarded channel
        // so the engine still has somewhere to write.
        let event_tx = config.event_tx.clone().unwrap_or_else(|| {
            let (tx, _rx) = tokio::sync::mpsc::channel::<crate::CoreEvent>(16);
            tx
        });

        let result = if !config.fork_context_messages.is_empty() {
            // Parent history is already shared via `Arc<Message>`; move
            // the owned Arc-slice out of `config` (last use) and append
            // the new user prompt after the inherited history.
            let mut messages: Vec<std::sync::Arc<coco_messages::Message>> =
                config.fork_context_messages;
            // Resume: seed the tool-result budget state from this agent's
            // persisted records + freeze the replayed tool_use_ids so the
            // resumed run's prompt prefix stays byte-identical (prompt-cache
            // stable). No-op for forks / fresh spawns (gated inside on a wired
            // store + agent_id).
            engine.seed_resumed_replacement_state(&messages).await;
            messages.push(std::sync::Arc::new(coco_messages::create_user_message(
                prompt,
            )));
            engine
                .run_with_messages(messages, event_tx, coco_types::TurnId::generate())
                .await
                .map_err(|e| {
                    Box::new(coco_error::PlainError::new(
                        e.to_string(),
                        coco_error::StatusCode::Internal,
                    )) as coco_error::BoxedError
                })?
        } else {
            // The single-prompt path uses `run_with_events` so the
            // caller's `event_tx` (or our discarded fallback) drives
            // the same emission stream as the fork path.
            engine
                .run_with_events(prompt, event_tx, coco_types::TurnId::generate())
                .await
                .map_err(|e| {
                    Box::new(coco_error::PlainError::new(
                        e.to_string(),
                        coco_error::StatusCode::Internal,
                    )) as coco_error::BoxedError
                })?
        };

        // Count ToolResult messages as a proxy for tool_use_count —
        // every committed tool_use produces exactly one tool_result,
        // so this tracks the actual tool_use count.
        let tool_use_count = result
            .final_messages
            .iter()
            .filter(|m| matches!(m.as_ref(), coco_messages::Message::ToolResult(_)))
            .count() as i64;
        // Return the engine's authoritative `Arc<Message>` history
        // directly — callers (SwarmAgentHandle, teammate runner)
        // forward the same Arcs through transcript / audit pipelines
        // without paying a serialize / deserialize round-trip.
        Ok(AgentQueryResult {
            response_text: Some(result.response_text),
            messages: result.final_messages,
            turns: result.turns,
            input_tokens: result.total_usage.input_tokens.total,
            output_tokens: result.total_usage.output_tokens.total,
            tool_use_count,
            usage: result.total_usage,
            cost_usd: result.cost_tracker.total_cost_usd(),
            input_cost_usd: result.cost_tracker.input_cost_usd(),
            output_cost_usd: result.cost_tracker.output_cost_usd(),
            cancelled: result.cancelled,
            // Captured `StructuredOutput` tool-call input (schema-validated)
            // when `output_schema` forced the contract; `None` otherwise.
            structured_output: result.structured_output,
        })
    }
}

/// Install the structured-output contract on a child engine: a per-spawn tool
/// registry carrying every parent tool plus a freshly-compiled
/// `StructuredOutputTool`, and the inline `requires_structured_output` nudge.
/// This keeps the contract scoped to one spawn without relying on a Stop hook.
fn install_structured_output(
    engine: &mut QueryEngine,
    schema: &serde_json::Value,
) -> Result<(), String> {
    // Per-spawn tool registry: clone the parent's tool handles, then add the
    // compiled StructuredOutput tool. `register_structured_output_tool` fails
    // when the schema is invalid — propagate that as the setup error.
    let tools = Arc::new(coco_tool_runtime::ToolRegistry::new());
    for tool in engine.tools.all() {
        tools.register(tool);
    }
    coco_tools::register_structured_output_tool(&tools, schema.clone())?;

    engine.tools = tools;
    engine.config.requires_structured_output = true;
    Ok(())
}

/// Convert a subagent's inherited read-scope dirs (the parent cwd +
/// `additional_dirs`) into the `session_additional_dirs` map the engine folds
/// into `ToolPermissionContext.additional_dirs`. This is what lets an
/// isolated-worktree subagent READ the parent project without a prompt — TS
/// `createSubagentContext` cwd + `additionalWorkingDirectories` parity.
fn inherited_read_dirs_to_additional_dirs(
    dirs: &[String],
) -> std::collections::HashMap<String, coco_types::AdditionalWorkingDir> {
    dirs.iter()
        .map(|path| {
            (
                path.clone(),
                coco_types::AdditionalWorkingDir {
                    path: path.clone(),
                    source: coco_types::WorkingDirectorySource::Session,
                },
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "agent_adapter.test.rs"]
mod tests;
