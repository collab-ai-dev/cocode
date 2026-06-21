//! Prompt + tool-definition + factory builders for [`QueryEngine`].
//!
//! Owns the per-turn assembly that feeds the LLM call:
//! - [`QueryEngine::build_prompt`] — system prompt (config or CLAUDE.md
//!   discovery) + staged-collapse application + history normalization.
//! - [`QueryEngine::build_tool_definitions`] — `LanguageModelTool` schemas
//!   resolved per-turn so Agent/Skill tools can inject live runtime state.
//! - [`QueryEngine::tool_context_factory`] — `ToolContextFactory` carrying
//!   the structured hook handle + every shared resource the executor reads.
//! - [`QueryEngine::observe_date_change`] — local-date rollover latch driving
//!   the `date_change` system reminder.
//!
//! Extracted from `engine.rs` so the multi-turn loop stays focused on flow
//! control rather than per-turn data marshalling.

use std::sync::Arc;

use coco_inference::LanguageModelFunctionTool;
use coco_inference::LanguageModelTool;
use coco_llm_types::ToolResultContent as LlmToolResultContent;
use tracing::info;

use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::ToolContent;
use coco_messages::ToolResultMessage;
use coco_types::ToolAppState;

use crate::engine::QueryEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelToolSource {
    BuiltIn,
    Mcp { server_name: String },
    Agent,
    Skill,
}

#[derive(Debug, Clone)]
pub(crate) struct BuiltToolDefinition {
    pub tool: LanguageModelTool,
    pub source: ModelToolSource,
    pub deferred: bool,
}

/// Realize a `Freeform` [`ToolSpec`](coco_tool_runtime::ToolSpec) as the OpenAI
/// Responses custom-grammar wire tool. The provider-specific realization (the
/// `openai.custom` id + `{type:"grammar", …}` shape) lives in
/// `coco_inference::openai_custom_grammar_tool` — this layer only forwards the
/// neutral `(name, description, syntax, definition)`. A `Freeform` tool is
/// OpenAI-Responses-only by construction (e.g. apply_patch).
fn freeform_provider_tool(ff: coco_tool_runtime::FreeformToolSpec) -> LanguageModelTool {
    LanguageModelTool::Provider(coco_inference::openai_custom_grammar_tool(
        ff.name,
        ff.description,
        ff.format.syntax,
        ff.format.definition,
    ))
}

/// Per-turn prompt + the post-budget message snapshot the engine threads
/// into every tool invocation's `ctx.messages`.
///
/// The snapshot is returned here so the engine can hand the same
/// `Arc<Vec<Arc<Message>>>` to `ToolContextFactory::build` for the same
/// turn.
pub(crate) struct BuiltPrompt {
    /// Normalized LLM messages — system prompt + per-turn working copy
    /// after `apply_tool_result_budget_to_prompt`, ready for the API.
    pub prompt: Vec<LlmMessage>,
    /// Same working copy as `Vec<Arc<Message>>`, shared via outer `Arc` so
    /// every tool ctx in this turn observes byte-identical history.
    pub messages_snapshot: Arc<Vec<Arc<Message>>>,
}

impl QueryEngine {
    /// Build the LLM prompt + the post-budget message snapshot from
    /// history. The snapshot is shared (via `Arc`) with every per-turn
    /// `ToolUseContext.messages` so tools observe the same view the
    /// model just received.
    pub(crate) async fn build_prompt(&self, history: &MessageHistory) -> BuiltPrompt {
        let mut prompt = Vec::new();

        // System prompt assembly:
        //
        //   1. If `coco_subagent::is_coordinator_mode(features)` is on, the
        //      leader becomes a coordinator and uses the coordinator-mode
        //      system prompt verbatim. The `simple_mode` toggle
        //      (`EnvKey::CocoSimple`) narrows the worker tool list.
        //   2. Otherwise: explicit config override > built-in default +
        //      CLAUDE.md discovery.
        let system_text = if coco_subagent::is_coordinator_mode(&self.config.features) {
            let simple_mode = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoSimple);
            coco_subagent::coordinator_system_prompt(simple_mode)
        } else if let Some(ref sys) = self.config.system_prompt {
            sys.clone()
        } else {
            let mut text =
                String::from("You are coco, an AI coding assistant. Be concise and helpful.\n\n");
            let cwd = std::env::current_dir().unwrap_or_default();
            for f in &coco_context::discover_memory_files(&cwd) {
                text.push_str(&format!("# {}\n{}\n\n", f.path.display(), f.content));
            }
            text
        };
        prompt.push(LlmMessage::system(&system_text));

        // Pre-build hook: apply staged-collapse commits so each
        // archived range is a single placeholder rather than full turns.
        // No-op when collapse is inactive.
        //
        // Default path (no collapse): `history.to_vec()` returns the
        // engine's `Vec<Arc<Message>>` — N atomic refcount increments,
        // no deep `Message` clones (pointer-only shallow copy).
        let mut messages_for_api: Vec<Arc<Message>> = if self.is_collapse_active() {
            if let Some(ledger) = &self.staged_ledger {
                let commits: Vec<_> = match ledger.try_lock() {
                    Ok(g) => g.commits.clone(),
                    Err(_) => Vec::new(),
                };
                let (collapsed, applied) =
                    coco_compact::apply_collapses_if_needed(history.as_slice(), &commits);
                if applied > 0 {
                    info!(applied, "applied {applied} staged collapses to prompt");
                }
                // Collapse returns owned `Vec<Message>` (the rewrite
                // mutates content in place internally); wrap once.
                collapsed.into_iter().map(Arc::new).collect()
            } else {
                history.to_vec()
            }
        } else {
            history.to_vec()
        };

        // Budget rewrites: CoW — only the K messages with selected
        // tool-result replacements pay a deep `Message` clone +
        // `Arc::new`; the N-K unchanged entries keep the parent Arc.
        self.apply_tool_result_budget_to_prompt(&mut messages_for_api)
            .await;

        // Apply the model-facing steering wrapper to queued-steering user
        // messages just before normalize (mirrors TS, which wraps queued
        // commands only at API-serialization time). The stored history /
        // `messages_snapshot` stays raw; only this API-bound view is wrapped,
        // so the transcript keeps showing the user's plain text. CoW: only
        // steering messages are rebuilt.
        let api_messages = crate::helpers::wrap_steering_messages_for_api(&messages_for_api);

        // Normalize takes `&[M] where M: Borrow<Message>`; `Arc<Message>`
        // borrows directly so we hand the Arc-slice through without an
        // extra materialization at this seam.
        let normalized = coco_messages::normalize_messages_for_api(&api_messages);
        prompt.extend(normalized);

        BuiltPrompt {
            prompt,
            messages_snapshot: Arc::new(messages_for_api),
        }
    }

    /// Apply the per-message tool-result aggregate budget over the
    /// turn's working copy. CoW: only the K messages with selected
    /// replacements pay a deep `Message` clone + fresh `Arc::new`. The
    /// remaining N-K entries keep the parent Arc that
    /// `MessageHistory` still references — verbose bodies stay
    /// available for transcript / resume reads. Unchanged messages
    /// pass through by reference.
    async fn apply_tool_result_budget_to_prompt(&self, messages: &mut Vec<Arc<Message>>) {
        let budget = &self.config.compact.tool_result_budget;
        if !budget.enabled {
            return;
        }

        let Some(session_dir) = self.tool_result_session_dir_for_prompt() else {
            return;
        };

        {
            let mut state = self.tool_result_replacement_state.write().await;
            state.per_message_chars = budget.per_message_chars;
        }

        for group in collect_api_user_tool_result_groups::<Arc<Message>>(messages.as_slice()) {
            let mut candidates = Vec::new();
            for idx in &group {
                let Message::ToolResult(tr) = messages[*idx].as_ref() else {
                    continue;
                };
                let Some(projected) = project_tool_result_content(tr) else {
                    continue;
                };
                let persistence_opted_out = self
                    .tools
                    .get(&tr.tool_id)
                    .is_some_and(|tool| tool.max_result_size_bound().is_unbounded());
                candidates.push(
                    coco_tool_runtime::tool_result_storage::ToolResultCandidate {
                        tool_use_id: tr.tool_use_id.clone(),
                        content_chars: projected.content.len() as i64,
                        content: projected.content,
                        tool_name: Some(tr.tool_id.to_string()),
                        persistence_opted_out,
                        is_json: projected.is_json,
                    },
                );
            }
            if candidates.is_empty() {
                continue;
            }

            let outcome = coco_tool_runtime::tool_result_storage::apply_tool_result_budget(
                &candidates,
                &self.tool_result_replacement_state,
                &session_dir,
            )
            .await;

            let mut persist_records_failed = false;
            if budget.persist_records
                && !outcome.newly_replaced.is_empty()
                && let (Some(store), Some(session_id)) =
                    (&self.transcript_store, &self.transcript_session_id)
            {
                let records: Vec<coco_session::ContentReplacementRecord> = outcome
                    .newly_replaced
                    .iter()
                    .map(|r| {
                        coco_session::ContentReplacementRecord::tool_result(
                            r.tool_use_id.clone(),
                            r.replacement.clone(),
                        )
                    })
                    .collect();
                if let Err(e) = store.insert_content_replacement(
                    session_id,
                    self.config.agent_id.as_deref(),
                    &records,
                ) {
                    tracing::warn!(
                        error = %e,
                        "failed to persist tool-result content replacement records"
                    );
                    persist_records_failed = true;
                }
            }
            if persist_records_failed {
                let mut state = self.tool_result_replacement_state.write().await;
                for replacement in &outcome.newly_replaced {
                    state.replacements.remove(&replacement.tool_use_id);
                }
            }

            let replacements = {
                let state = self.tool_result_replacement_state.read().await;
                candidates
                    .iter()
                    .filter_map(|c| {
                        state
                            .replacements
                            .get(&c.tool_use_id)
                            .map(|replacement| (c.tool_use_id.clone(), replacement.clone()))
                    })
                    .collect::<std::collections::HashMap<_, _>>()
            };
            if replacements.is_empty() {
                continue;
            }
            for idx in &group {
                // Skip non-ToolResult entries and ToolResults without a
                // selected replacement — both pass through unchanged
                // (parent Arc preserved, no allocation).
                let Message::ToolResult(orig) = messages[*idx].as_ref() else {
                    continue;
                };
                let Some(replacement) = replacements.get(&orig.tool_use_id) else {
                    continue;
                };
                // CoW with no big-body memcpy: construct a fresh
                // `ToolResultMessage` from `orig`'s small metadata
                // (uuid / tool_use_id / tool_id / tool_name /
                // provider_metadata) + the short replacement body.
                // The original's verbose `output.value` (which can be
                // megabytes for grep/find-style tools) is never
                // memcopied — the rewrite spreads `{...block, content:
                // replacement}` without copying the discarded content
                // string.
                let new_msg = match rewrite_tool_result_to_placeholder(orig, replacement) {
                    Some(rebuilt) => Message::ToolResult(rebuilt),
                    None => {
                        // Defensive fallback for unexpected message
                        // shape (no matching block / non-Tool inner
                        // message). Clone-then-mutate keeps semantics
                        // identical to pre-optimization.
                        let mut cloned = (*messages[*idx]).clone();
                        if let Message::ToolResult(ref mut tr) = cloned {
                            replace_tool_result_content(tr, replacement);
                        }
                        cloned
                    }
                };
                messages[*idx] = Arc::new(new_msg);
            }
        }
    }

    fn tool_result_session_dir_for_prompt(&self) -> Option<std::path::PathBuf> {
        if let (Some(store), Some(session_id)) =
            (&self.transcript_store, &self.transcript_session_id)
        {
            return store.session_artifact_dir(session_id);
        }
        None
    }

    /// Build tool definitions for the LLM (function tool schemas).
    ///
    /// Each `Tool::prompt(&PromptOptions)` call returns the description the
    /// model sees that turn. Agent/Skill tools use this hook to inject live
    /// runtime state (current agent / skill listings) into their description.
    /// For tools that don't override `prompt`, the trait default delegates to
    /// `description()`, preserving the legacy behavior.
    pub(crate) async fn build_tool_definitions(
        &self,
        app_state: &ToolAppState,
    ) -> Vec<LanguageModelTool> {
        self.build_tool_definitions_detailed(app_state)
            .await
            .into_iter()
            .map(|d| d.tool)
            .collect()
    }

    pub(crate) async fn build_tool_definitions_detailed(
        &self,
        app_state: &ToolAppState,
    ) -> Vec<BuiltToolDefinition> {
        // Carry the `ToolSearch` discovery set into the filter pipeline
        // so deferred tools the model has unlocked get their schema in
        // this turn's request.
        let discovered = std::sync::Arc::new(app_state.discovered_tool_names.clone());

        // Resolve both `ToolSearch`-related capabilities from the
        // active client's `ModelInfo`. Three-state outcome:
        //   - server (Anthropic Sonnet 4.5+/Opus 4+ via beta
        //     `tool-search-tool-2025-10-19`): tools array carries every
        //     enabled tool with `deferLoading: true` on deferred ones.
        //     Server expands `tool_reference` content blocks into
        //     `<functions>` markup inline — `tools` shape is constant
        //     across turns, prompt-cache prefix stays warm.
        //   - client-side only (capable models without server beta —
        //     GPT-5, Gemini, DeepSeek, Haiku, …): tools array contains
        //     only the loaded set; deferred tools enter on the next
        //     turn after `ToolSearch` writes
        //     `discovered_tool_names`. One cache break per discovery.
        //   - neither (unknown / custom model that didn't declare
        //     either capability): the registry filter auto-disables
        //     deferral via `tool_search_active`, so every enabled
        //     tool's full schema lands on turn 1 (safe default).
        let snapshot = self.runtime_snapshot();
        let supports_tool_reference = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.model_info.as_ref())
            .is_some_and(|info| {
                info.has_capability(coco_types::Capability::ServerSideToolReference)
            });
        let supports_client_side_tool_search = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.model_info.as_ref())
            .is_some_and(|info| info.has_capability(coco_types::Capability::ClientSideToolSearch));

        let stub_ctx = coco_tool_runtime::ToolUseContext::stub_for_filtering(
            self.config.features.clone(),
            self.config.tool_overrides.clone(),
            self.config.tool_filter.clone(),
            self.config.permission_mode,
        )
        .with_discovered_tool_names(discovered.clone())
        .with_model_capabilities(supports_tool_reference, supports_client_side_tool_search)
        .with_active_shell_tool(self.config.active_shell_tool);
        let stub_ctx = self.with_current_tool_search_candidates(stub_ctx).await;

        // The tool list sent to the model. When the server-side path
        // is live (capability declared AND `Feature::ToolSearch` on),
        // `enabled` includes deferred tools too; `deferred_marker`
        // captures which names need the `deferLoading` provider-option
        // patch below. Otherwise (client-side path OR feature off OR
        // capability missing), `loaded_tools` handles the partition
        // — its short-circuit on `tool_search_active` covers the
        // capability-missing case automatically.
        let use_server_side_path = supports_tool_reference && stub_ctx.tool_search_active();
        let (mut model_tools, deferred_marker): (Vec<_>, std::collections::HashSet<String>) =
            if use_server_side_path {
                let enabled = self.tools.enabled(&stub_ctx);
                let deferred: std::collections::HashSet<String> = self
                    .tools
                    .deferred_tools(&stub_ctx)
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect();
                (enabled, deferred)
            } else {
                (
                    self.tools.loaded_tools(&stub_ctx),
                    std::collections::HashSet::new(),
                )
            };
        // Deterministic tool order for prompt-cache stability. The registry is a
        // `HashMap`, so `.values()` order is unstable across process restarts and
        // reshuffles wholesale when an MCP server registers mid-session (the
        // insert rehashes the table and reorders ALL keys). Sort so the prefix
        // stays byte-stable, AND **partition built-ins before MCP/dynamic tools**:
        // built-ins form a stable prefix while MCP tools (which connect/disconnect
        // mid-session) are appended at the end, so MCP churn never shifts a
        // built-in's index. Mirrors codex-rs (built-ins first, MCP appended) and
        // jcode (sorted "for prompt cache hits"). `false < true` ⇒ non-MCP first;
        // ties broken by wire name. NB: keyed on `is_mcp()` (not a name prefix) —
        // MCP `name()` is the bare tool name, so a case/name coincidence cannot be
        // relied on to keep MCP out of the built-in prefix.
        model_tools.sort_by(|a, b| (a.is_mcp(), a.name()).cmp(&(b.is_mcp(), b.name())));
        let tool_names: Vec<String> = model_tools.iter().map(|t| t.name().to_string()).collect();

        let skill_names: Vec<String> = {
            let mut names = self
                .session_bootstrap
                .as_ref()
                .map(|b| b.skills.clone())
                .unwrap_or_default();
            names.sort();
            names
        };

        let permission_mode = app_state
            .permissions
            .mode
            .unwrap_or(self.config.permission_mode);
        let permission_context = coco_types::ToolPermissionContext {
            mode: permission_mode,
            additional_dirs: app_state.permissions.additional_dirs.clone(),
            allow_rules: std::collections::HashMap::new(),
            deny_rules: std::collections::HashMap::new(),
            ask_rules: std::collections::HashMap::new(),
            bypass_available: self.config.bypass_permissions_available,
            pre_plan_mode: app_state.permissions.pre_plan_mode,
            stripped_dangerous_rules: app_state.permissions.stripped_dangerous_rules.clone(),
            session_plan_file: None,
            permission_rule_source_roots: self.config.permission_rule_source_roots.clone(),
        };
        // Thread the agent catalog + connected MCP servers through to
        // PromptOptions so AgentTool::prompt can render the dynamic
        // per-agent listing with MCP-availability filter.
        let agent_catalog = self.agent_catalog.clone();
        // Read the MCP-ready set off the engine's installed handle
        // (`with_mcp_handle` at session bootstrap). When unset
        // (tests / minimal embeddings), `None` flows to the renderer, which
        // then HIDES every agent that REQUIRES MCP servers (none are
        // available) while still letting non-MCP agents through. That matches
        // the call-time guard, which rejects the same set via the NoOp
        // handle's empty `list_tools()` — no advertise-then-reject gap.
        let ready_mcp_servers = self.mcp_servers_ready_snapshot().await;
        let coordinator_mode = coco_subagent::is_coordinator_mode(self.config.features.as_ref());
        let fork_enabled = coco_subagent::is_fork_subagent_active(
            &self.config.features,
            self.config.is_non_interactive,
        );
        // `isPlanModeInterviewPhaseEnabled()` is settings-only
        // (no Growthbook, no env var). See `core/context/CLAUDE.md`.
        let is_plan_interview_phase = matches!(
            self.config.plan_mode_settings.workflow,
            coco_config::PlanModeWorkflow::Interview
        );

        // These flags shape the model-visible AgentTool description.
        // Resolved from env / config / runtime state and forwarded to
        // `AgentToolPromptRenderer` to swap section bodies accordingly.
        let background_tasks_disabled =
            coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBackgroundTasksDisable);
        let agent_list_via_attachment =
            coco_config::env::is_env_truthy(coco_config::EnvKey::CocoAgentListInMessages);
        // 3p builds (the default) keep `ant_build` off — coco-rs ships
        // only worktree isolation. The flag is wired here so an internal
        // build can flip it via config without re-rendering callers.
        let ant_build = false;
        // `has_embedded_search_tools` is host-build dependent. Coco-rs
        // ships the dedicated `Glob`/`Grep` tools, so the flag stays
        // off — the AgentTool description points at them rather than
        // `find` / `grep` via Bash.
        let has_embedded_search_tools = false;
        let agent_teams_available = self
            .config
            .features
            .enabled(coco_types::Feature::AgentTeams);
        // Subscription tier and teammate flags do not yet have a
        // resolved source in coco-rs. Defaulting to `false` keeps the
        // inline concurrency hint and full subagent prompt — the most
        // permissive 3p shape.
        let is_pro_subscription = false;
        let is_in_process_teammate = false;
        let is_teammate = false;

        let prompt_options = coco_tool_runtime::PromptOptions {
            is_non_interactive: self.config.is_non_interactive,
            tool_names,
            allowed_agent_types: None,
            skill_names,
            permission_context: Some(permission_context),
            agent_catalog,
            ready_mcp_servers,
            coordinator_mode,
            fork_enabled,
            is_plan_interview_phase,
            has_embedded_search_tools,
            agent_teams_available,
            is_in_process_teammate,
            is_teammate,
            agent_list_via_attachment,
            is_pro_subscription,
            background_tasks_disabled,
            ant_build,
        };

        // Session context for `Tool::tool_spec`. Lets AgentTool drop
        // `run_in_background` from its model-facing schema when the runtime
        // would silently veto it.
        let schema_ctx = coco_tool_runtime::SchemaContext {
            background_tasks_disabled,
            fork_mode_active: fork_enabled,
            features: Some(self.config.features.clone()),
            // Active model's apply_patch shape (None → Freeform). ApplyPatchTool
            // reads this to choose its `tool_spec` (Freeform grammar vs JSON).
            apply_patch_tool_type: snapshot
                .as_ref()
                .and_then(|s| s.model_info.as_ref())
                .and_then(|i| i.apply_patch_tool_type),
        };

        // Cache breakpoint at the built-in/MCP boundary. When MCP/dynamic tools
        // follow the stable built-in block, flag the LAST built-in tool with a
        // `cacheBoundary` hint so the built-in prefix is cached as its own
        // segment — a mid-session MCP connect/disconnect that appends/removes the
        // tail then doesn't invalidate it. Without a breakpoint here the only
        // cache segment is the whole [tools+system+history] prefix (auto-marker on
        // the last user message), so any tool-set change forces a full re-cache of
        // the built-ins too (tools sit first in the request).
        //
        // The engine emits only the provider-agnostic *hint* (it alone knows the
        // `is_mcp` partition); the Anthropic adapter owns the cache POLICY —
        // resolving the marker's TTL to match the auto-marker, gating on caching
        // being active, and counting it against the 4-breakpoint budget. Other
        // providers ignore the `anthropic` namespace; OpenAI-family auto
        // prefix-caching already benefits from the built-ins-first ordering.
        let builtin_count = model_tools.iter().filter(|t| !t.is_mcp()).count();
        let cache_boundary_idx = builtin_mcp_boundary_idx(builtin_count, model_tools.len());

        let mut out = Vec::with_capacity(model_tools.len());
        for (idx, tool) in model_tools.into_iter().enumerate() {
            // `tool_spec(ctx)` is the single source of truth for the tool's
            // model-facing wire shape (description + parameters/grammar). The
            // default builds a JSON `Function` from `prompt()` + the runtime
            // schema; AgentTool/Bash/ExitPlanMode override to omit runtime-only
            // hook-injected fields (`mcp_servers`, `_simulatedSedEdit`,
            // `plan`), and ApplyPatch overrides to a `Freeform` grammar tool
            // (codex `ToolSpec::Freeform`).
            let spec = tool.tool_spec(&schema_ctx, &prompt_options).await;
            // Build the `anthropic` provider-options block. `deferLoading: true`
            // hides a not-yet-discovered tool's schema from the model (ToolSearch);
            // `cacheBoundary: true` flags the built-in/MCP boundary tool for the
            // adapter's prefix-cache breakpoint. Other providers ignore the
            // `anthropic` namespace.
            let tool_name = tool.name();
            let mut anthropic: std::collections::HashMap<String, serde_json::Value> =
                std::collections::HashMap::new();
            if deferred_marker.contains(tool_name) {
                anthropic.insert("deferLoading".to_string(), serde_json::Value::Bool(true));
            }
            if Some(idx) == cache_boundary_idx {
                anthropic.insert("cacheBoundary".to_string(), serde_json::Value::Bool(true));
            }
            let deferred = deferred_marker.contains(tool_name);
            let source = if let Some(info) = tool.mcp_info() {
                ModelToolSource::Mcp {
                    server_name: info.server_name.clone(),
                }
            } else if tool_name == coco_types::ToolName::Agent.as_str() {
                ModelToolSource::Agent
            } else if tool_name == coco_types::ToolName::Skill.as_str() {
                ModelToolSource::Skill
            } else {
                ModelToolSource::BuiltIn
            };
            let provider_options = if anthropic.is_empty() {
                None
            } else {
                let mut po_map = std::collections::HashMap::new();
                po_map.insert("anthropic".to_string(), anthropic);
                Some(coco_llm_types::ProviderOptions(po_map))
            };
            // Convert the neutral `ToolSpec` to the provider wire form. A
            // `Freeform` tool becomes an OpenAI provider-defined custom tool
            // (`id: "openai.custom"`) carrying the lark grammar; `prepare_tools`
            // serializes it to `{type:"custom", name, format}`. The anthropic
            // cache/defer hints only apply to `Function` tools — the `Provider`
            // variant has no provider-options slot (and Freeform tools never
            // defer: `should_defer()` is false, so they are never in
            // `deferred_marker`).
            let wire = match spec {
                coco_tool_runtime::ToolSpec::Function(f) => {
                    LanguageModelTool::Function(LanguageModelFunctionTool {
                        name: f.name,
                        description: Some(f.description),
                        input_schema: coco_tool_runtime::canonicalize_model_tool_schema(
                            &f.parameters,
                        ),
                        input_examples: None,
                        strict: f.strict.then_some(true),
                        provider_options,
                    })
                }
                coco_tool_runtime::ToolSpec::Freeform(ff) => freeform_provider_tool(ff),
            };
            out.push(BuiltToolDefinition {
                tool: wire,
                source,
                deferred,
            });
        }
        out
    }

    /// Build a factory that knows how to construct [`ToolUseContext`]
    /// snapshots from the engine's current config + shared handles.
    ///
    /// Each turn calls `factory.build(...)` to get a fresh context; the
    /// factory re-reads live `ToolAppState` per call so permission-mode
    /// mutations from a prior batch (e.g. `EnterPlanMode`) propagate
    /// without a config reload.
    ///
    /// The field mapping itself — including the five previously-hardcoded
    /// fields (`thinking_level`, `is_non_interactive`, `max_budget_usd`,
    /// `custom_system_prompt`, `append_system_prompt`) — is verified in
    /// `tool_context.test.rs`.
    /// Snapshot the MCP servers that are actually usable — i.e. servers that
    /// contributed tools (connected AND authenticated) — for the model-facing
    /// agent listing. Returns `None` only when no MCP handle was installed.
    ///
    /// `None` is the no-handle sentinel, NOT "skip the filter": the AgentTool
    /// prompt renderer (`core/subagent prompt.rs`) HIDES every agent that
    /// requires MCP servers when `ready_mcp_servers` is `None` (agents with no
    /// requirement still pass). That mirrors TS, where an empty
    /// `mcpServersWithTools` makes `hasRequiredMcpServers` false for any
    /// MCP-requiring agent. The call-time guard stays aligned: with no handle
    /// `ctx.mcp` is the NoOp handle whose `list_tools()` is empty, so `execute`
    /// rejects exactly the agents the listing hid — no advertise-then-reject
    /// gap. (The "absent → show all" behavior lives only in the non-model-facing
    /// `context_analysis::agent_estimates` estimator.)
    ///
    /// Derived from `list_tools()` (distinct `server_name`s), NOT
    /// `connected_servers()`. This mirrors TS, which builds `mcpServersWithTools`
    /// from `mcp__`-prefixed tool names for both the prose listing and the
    /// call-time guard, and matches `AgentTool`'s own `mcp_servers_with_tools`
    /// call-time check. Using `connected_servers()` here would advertise an
    /// agent whose required server is connected-but-unauthenticated (zero tools)
    /// and then let `execute` reject the spawn — the advertise-then-reject gap.
    pub(crate) async fn mcp_servers_ready_snapshot(&self) -> Option<Vec<String>> {
        let handle = self.mcp_handle.as_ref()?;
        let mut servers: Vec<String> = Vec::new();
        for tool in handle.list_tools().await {
            if !servers.iter().any(|s| s == &tool.server_name) {
                servers.push(tool.server_name);
            }
        }
        Some(servers)
    }

    /// Current active agent type names from the wired catalog — the single
    /// source of truth for every catalog-derived reminder (agent mentions,
    /// explore/plan hint, agent-listing delta) in BOTH the turn-reminder and
    /// post-compaction paths. Keyed on `def.name` (= TS `agentType`), matching
    /// the model-visible "Available agent types" listing and the set
    /// `AgentTool::execute` validates `subagent_type` against. Empty when no
    /// catalog is wired (tests / minimal embeddings).
    ///
    /// Deliberately NOT `session_bootstrap.agents`: that field is `None` in
    /// every production path (TUI/SDK/headless), so reading it silently emptied
    /// these reminders. Funnel both reminder paths through here so they cannot
    /// drift back onto the dead field independently.
    pub(crate) fn current_agent_types(&self) -> Vec<String> {
        self.agent_catalog
            .as_ref()
            .map(|catalog| catalog.active().map(|def| def.name.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) async fn with_current_tool_search_candidates(
        &self,
        mut ctx: coco_tool_runtime::ToolUseContext,
    ) -> coco_tool_runtime::ToolUseContext {
        if !ctx.tool_search_supported() {
            return ctx;
        }
        ctx.tool_search_has_candidates = true;
        let has_deferred = !self.tools.deferred_tools(&ctx).is_empty();
        let has_pending_mcp = match self.mcp_handle.as_ref() {
            Some(handle) => !handle.pending_server_names().await.is_empty(),
            None => false,
        };
        ctx.tool_search_has_candidates = has_deferred || has_pending_mcp;
        ctx
    }

    pub(crate) fn tool_context_factory(
        &self,
        hook_tx: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
    ) -> crate::tool_context::ToolContextFactory {
        let snapshot = self.runtime_snapshot();
        // Build the structured hook handle here — every tool call built
        // through this factory gets the same `QueryHookHandle`, so
        // PreToolUse/PostToolUse/PostToolUseFailure fire consistently
        // regardless of the call site. When the session has no hook
        // registry (tests / single-turn helpers) we pass `None` and the
        // `ToolUseContext` receives no handle — executor treats that as
        // a no-op, matching legacy behavior.
        let hook_handle = self.hooks.as_ref().map(|registry| {
            let handle: coco_tool_runtime::HookHandleRef =
                std::sync::Arc::new(crate::hook_adapter::QueryHookHandle::new(
                    registry.clone(),
                    self.orchestration_ctx(),
                    hook_tx.cloned(),
                ));
            handle
        });
        crate::tool_context::ToolContextFactory {
            config: self.config.clone(),
            tools: self.tools.clone(),
            turn_abort: self.turn_abort.clone(),
            mailbox: self.mailbox.clone(),
            pending_messages: self.pending_messages.clone(),
            task_list: self.task_list.clone(),
            team_task_list_router: self.team_task_list_router.clone(),
            todo_list: self.todo_list.clone(),
            task_handle: self.task_handle.clone(),
            permission_bridge: self.permission_bridge.clone(),
            app_state: self.app_state.clone(),
            file_read_state: self.file_read_state.clone(),
            file_history: self.file_history.clone(),
            config_home: self.config_home.clone(),
            tool_result_session_dir: self.tool_result_session_dir_for_prompt(),
            transcript_path: self
                .transcript_store
                .as_ref()
                .and_then(|store| store.transcript_path(&self.config.session_id)),
            hook_handle,
            // Real `AgentHandle` when the CLI / SDK / TUI installed
            // one via `with_agent_handle`; otherwise fall back to
            // `NoOpAgentHandle`.
            agent_handle: self.agent_handle.clone(),
            // `SkillHandle` same pattern — real handle when
            // installed, `NoOpSkillHandle` otherwise.
            skill_handle: self.skill_handle.clone(),
            // `LspHandle` same pattern — real handle when installed via
            // `with_lsp_handle` at session bootstrap; otherwise
            // `NoOpLspHandle` so `is_connected() = false` and `LspTool`
            // is filtered out of the model's tool list.
            lsp_handle: self.lsp_handle.clone(),
            // `McpHandle` is installed via `with_mcp_handle` from
            // `SessionRuntime.wire_engine` (which reads the late-bound
            // slot populated by `install_session_late_binds`).
            // Without this, MCP tools (`McpAuthTool`, list/read
            // resources, dynamic `McpTool`) fall back to
            // `NoOpMcpHandle` and surface "not configured" errors.
            mcp_handle: self.mcp_handle.clone(),
            schedule_store: self.schedule_store.clone(),
            // Session-scoped schema validator. Clone is cheap —
            // inner state is `Arc<RwLock<HashMap>>` shared across
            // per-turn ctx rebuilds so the compile cache persists.
            // Agent definition catalog snapshot (T7). When the
            // bootstrap installed one via `with_agent_catalog`,
            // AgentTool reads `subagent_type → AgentDefinition` from
            // here at the spawn boundary.
            agent_catalog: self.agent_catalog.clone(),
            // Parent runtime snapshot captured from the runtime registry
            // and threaded onto every ToolUseContext for AgentTool to pin
            // Fork-mode prompt-cache parity.
            parent_runtime_snapshot: snapshot
                .map(|snapshot| std::sync::Arc::new(snapshot.runtime_snapshot)),
            // Per-engine live Command-source rule store. Same Arc as
            // `QueryEngine.live_command_rules` and the
            // `EngineLiveRulesHandle` installed on the executor —
            // factory.build merges it into `allow_rules[Command]` per
            // batch. See `engine_live_rules` for the lifecycle.
            live_command_rules: self.live_command_rules.clone(),
        }
    }

    /// Build a representative base [`ToolUseContext`](coco_tool_runtime::ToolUseContext)
    /// for this session. app/cli uses it to back the in-prompt-shell
    /// `BashToolHandle` (the handle clones it per command and folds in the
    /// skill's `allowed-tools` before the permission check + Bash execution).
    /// Carries the same resolved tool config, sandbox state, permission
    /// context, handles, and cwd cell a per-turn tool call would receive.
    pub async fn build_base_tool_context(&self) -> coco_tool_runtime::ToolUseContext {
        self.tool_context_factory(None)
            .build(crate::tool_context::ToolContextOverrides::default())
            .await
    }

    /// Detect local-date rollover for the `date_change` system reminder.
    ///
    /// Reads `ToolAppState::last_emitted_date`, compares it to today's
    /// local ISO date, and:
    ///
    /// - seeds the latch on first observation, returning `None`
    ///   (no reminder — TS `getDateChangeAttachments` matches: the first
    ///   turn of a session never emits because there's no prior date);
    /// - returns `Some(today)` and updates the latch on a mismatch
    ///   (engine passes it to `TurnReminderInput.new_date` and the
    ///   `DateChangeGenerator` emits once);
    /// - returns `None` when the latch already matches today.
    ///
    /// No-op (returns `None`) when `self.app_state` is `None`.
    pub(crate) async fn observe_date_change(&self) -> Option<String> {
        let state = self.app_state.as_ref()?;
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut guard = state.write().await;
        match guard.last_emitted_date.as_deref() {
            Some(prev) if prev == today => None,
            Some(_) => {
                guard.last_emitted_date = Some(today.clone());
                Some(today)
            }
            None => {
                // First observation: seed without emitting.
                guard.last_emitted_date = Some(today);
                None
            }
        }
    }
}

/// Index of the cache-boundary tool — the LAST built-in — when a non-empty
/// built-in prefix is followed by MCP/dynamic tools; `None` otherwise (all
/// built-in, all MCP, or empty). Pure and underflow-safe: the guard must
/// short-circuit BEFORE `builtin_count - 1` is evaluated, since a `0` count
/// would underflow `usize`. See `build_tool_definitions`.
fn builtin_mcp_boundary_idx(builtin_count: usize, total: usize) -> Option<usize> {
    (builtin_count > 0 && builtin_count < total).then(|| builtin_count - 1)
}

#[derive(Debug)]
struct ProjectedToolResultContent {
    content: String,
    is_json: bool,
}

fn collect_api_user_tool_result_groups<M: std::borrow::Borrow<Message>>(
    messages: &[M],
) -> Vec<Vec<usize>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut seen_assistant_ids = std::collections::HashSet::new();

    for (idx, msg) in messages.iter().enumerate() {
        match msg.borrow() {
            Message::Assistant(asst) => {
                let assistant_id = asst
                    .request_id
                    .clone()
                    .unwrap_or_else(|| asst.uuid.to_string());
                if seen_assistant_ids.insert(assistant_id) && !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
            }
            Message::ToolResult(_) => current.push(idx),
            Message::User(_)
            | Message::System(_)
            | Message::Attachment(_)
            | Message::Progress(_)
            | Message::Tombstone(_) => {}
        }
    }

    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn project_tool_result_content(tr: &ToolResultMessage) -> Option<ProjectedToolResultContent> {
    let LlmMessage::Tool { content, .. } = &tr.message else {
        return None;
    };
    let part = content.iter().find_map(|part| match part {
        ToolContent::ToolResult(result) if result.tool_call_id == tr.tool_use_id => Some(result),
        _ => None,
    })?;

    match &part.output {
        LlmToolResultContent::Text { value, .. }
        | LlmToolResultContent::ErrorText { value, .. } => Some(ProjectedToolResultContent {
            content: value.clone(),
            is_json: false,
        }),
        LlmToolResultContent::Json { value, .. }
        | LlmToolResultContent::ErrorJson { value, .. } => {
            let content = serde_json::to_string(value).ok()?;
            Some(ProjectedToolResultContent {
                content,
                is_json: true,
            })
        }
        LlmToolResultContent::ExecutionDenied { reason, .. } => Some(ProjectedToolResultContent {
            content: reason.clone().unwrap_or_default(),
            is_json: false,
        }),
        LlmToolResultContent::Content { value, .. } => {
            let mut texts = Vec::new();
            for part in value {
                match part {
                    coco_llm_types::ToolResultContentPart::Text { text, .. } => {
                        texts.push(text.clone());
                    }
                    _ => return None,
                }
            }
            Some(ProjectedToolResultContent {
                content: texts.join("\n\n"),
                is_json: false,
            })
        }
    }
}

fn replace_tool_result_content(tr: &mut ToolResultMessage, replacement: &str) -> bool {
    let LlmMessage::Tool { content, .. } = &mut tr.message else {
        return false;
    };
    for part in content.iter_mut() {
        let ToolContent::ToolResult(result) = part else {
            continue;
        };
        if result.tool_call_id != tr.tool_use_id {
            continue;
        }
        result.output = if result.is_error {
            LlmToolResultContent::error_text(replacement)
        } else {
            LlmToolResultContent::text(replacement)
        };
        return true;
    }
    false
}

/// Build a fresh `ToolResultMessage` from `orig`'s metadata + a new
/// short replacement body — without cloning the discarded
/// `output.value` of the original. Skips the
/// `(*messages[idx]).clone()` → `mutate` → drop-big-string anti-pattern
/// that the legacy CoW path used (memcpy a 100 KB tool result then
/// immediately overwrite it with a ~200-byte `<persisted-output>`
/// preview).
///
/// Preserves metadata (`tool_use_id` / `tool_name` / `cache_control`
/// /etc.):
///
/// - `uuid` / `tool_use_id` / `tool_id` / `is_error` on the outer
///   `ToolResultMessage` are copied / cheaply cloned.
/// - For the inner content block whose `tool_call_id` matches
///   `orig.tool_use_id`, build a fresh `ToolResultPart` keeping
///   `tool_name` + `provider_metadata` and replacing only `output`.
/// - Other content blocks (rare — coco-rs's
///   `create_tool_result_message` produces single-block messages) are
///   cloned verbatim.
///
/// Returns `None` when the original's `LlmMessage` isn't `Tool` or no
/// matching block exists — caller falls back to the legacy
/// clone-then-mutate path so we never silently lose a rewrite.
fn rewrite_tool_result_to_placeholder(
    orig: &ToolResultMessage,
    replacement: &str,
) -> Option<ToolResultMessage> {
    let LlmMessage::Tool {
        content: orig_content,
        provider_options,
    } = &orig.message
    else {
        return None;
    };
    if !orig_content.iter().any(|p| {
        matches!(
            p,
            ToolContent::ToolResult(r) if r.tool_call_id == orig.tool_use_id
        )
    }) {
        return None;
    }
    let new_content: Vec<ToolContent> = orig_content
        .iter()
        .map(|part| match part {
            ToolContent::ToolResult(result) if result.tool_call_id == orig.tool_use_id => {
                let new_output = if result.is_error {
                    LlmToolResultContent::error_text(replacement)
                } else {
                    LlmToolResultContent::text(replacement)
                };
                ToolContent::ToolResult(coco_llm_types::ToolResultPart {
                    tool_call_id: result.tool_call_id.clone(),
                    tool_name: result.tool_name.clone(),
                    output: new_output,
                    is_error: result.is_error,
                    provider_metadata: result.provider_metadata.clone(),
                })
            }
            other => other.clone(),
        })
        .collect();
    Some(ToolResultMessage {
        uuid: orig.uuid,
        source_assistant_uuid: orig.source_assistant_uuid,
        display_data: orig.display_data.clone(),
        message: LlmMessage::Tool {
            content: new_content,
            provider_options: provider_options.clone(),
        },
        tool_use_id: orig.tool_use_id.clone(),
        tool_id: orig.tool_id.clone(),
        is_error: orig.is_error,
    })
}

#[cfg(test)]
#[path = "engine_prompt.test.rs"]
mod tests;
