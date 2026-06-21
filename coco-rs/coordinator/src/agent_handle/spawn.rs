//! Standalone-subagent spawn dispatch.
//!
//! Owns:
//! - [`SwarmAgentHandle::spawn_subagent`] — applies worktree isolation,
//!   resolves the spawn-time identity
//!   (`coco_subagent::resolve_subagent_selection`), registers user-visible
//!   LocalAgent tasks in TaskManager, builds the `AgentQueryConfig`, and
//!   dispatches sync vs background.
//! - [`spawn_failed`] — tiny shorthand for sync-path early failures.
//!
//! Pure-logic helpers live in `core/subagent`. Handoff classification
//! lives in `super::handoff`; periodic AgentSummary writes through
//! TaskManager progress. Background-spawn resume lives in `super::resume`.

use std::sync::Arc;
use std::time::Instant;

use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;

use super::SwarmAgentHandle;

/// Build an `OrchestrationContext` for subagent-spawn hook firing.
///
/// Standalone so the bg path can build it inside a detached task
/// without holding `&SwarmAgentHandle`. The hook system's full
/// feature surface (project_dir resolution, per-session disable
/// flag, attachment emitter) is approximated here — subagent-spawn
/// hooks don't need the same wire-up the parent's per-turn hooks do.
pub(super) fn hook_ctx_for_cwd(cwd: &str) -> coco_hooks::orchestration::OrchestrationContext {
    hook_ctx_for_subagent(cwd, None, None)
}

/// Variant that stamps `agent_id` / `agent_type` onto the
/// [`coco_hooks::orchestration::OrchestrationContext`] so every fired
/// hook's `BaseHookInput` carries the subagent identity.
pub(super) fn hook_ctx_for_subagent(
    cwd: &str,
    agent_id: Option<&str>,
    agent_type: Option<&str>,
) -> coco_hooks::orchestration::OrchestrationContext {
    coco_hooks::orchestration::OrchestrationContext {
        session_id: String::new(),
        cwd: std::path::PathBuf::from(cwd),
        project_dir: Some(std::path::PathBuf::from(cwd)),
        permission_mode: None,
        transcript_path: None,
        agent_id: agent_id.map(str::to_string),
        agent_type: agent_type.map(str::to_string),
        cancel: tokio_util::sync::CancellationToken::new(),
        disable_all_hooks: false,
        allow_managed_hooks_only: false,
        attachment_emitter: coco_messages::AttachmentEmitter::noop(),
        // Subagent-spawn hooks aren't reminder-bearing in TS — their
        // output flows back into the spawn's initial-user message via
        // `additional_contexts`, not the per-turn reminder pipeline.
        sync_event_sink: None,
        http_url_allowlist: None,
        http_env_var_policy: None,
        async_registry: None,
        async_rewake_sink: None,
        llm_handle: None,
        workspace_trust_accepted: None,
    }
}

/// Free-function helper for `WorktreeCreate`.
/// Coco-rs does git creation directly (`AgentWorktreeManager::create_for`),
/// so the hook is observe-only — non-zero exit codes are logged but
/// don't roll back the worktree.
pub(super) async fn fire_worktree_create_hook(
    registry: Option<Arc<coco_hooks::HookRegistry>>,
    cwd: &str,
    name: &str,
) {
    let Some(registry) = registry else { return };
    let ctx = hook_ctx_for_cwd(cwd);
    if let Err(e) = coco_hooks::orchestration::execute_worktree_create(&registry, &ctx, name).await
    {
        tracing::warn!(error = %e, %name, "WorktreeCreate hook firing failed");
    }
}

/// Free-function helper for `WorktreeRemove`. Fired only when
/// `cleanup_if_unchanged` actually removed the worktree; `Kept`
/// outcomes preserve the user's work so the remove notification is
/// suppressed.
pub(super) async fn fire_worktree_remove_hook(
    registry: Option<Arc<coco_hooks::HookRegistry>>,
    cwd: &str,
    worktree_path: &str,
) {
    let Some(registry) = registry else { return };
    let ctx = hook_ctx_for_cwd(cwd);
    if let Err(e) =
        coco_hooks::orchestration::execute_worktree_remove(&registry, &ctx, worktree_path).await
    {
        tracing::warn!(error = %e, %worktree_path, "WorktreeRemove hook firing failed");
    }
}

/// Free-function variant of `fire_subagent_start_hook` for the bg
/// path. Same fail-open semantics.
pub(super) async fn fire_subagent_start_for_task(
    registry: Option<Arc<coco_hooks::HookRegistry>>,
    cwd: &str,
    agent_id: &str,
    agent_type: &str,
    prompt: &str,
) -> String {
    let Some(registry) = registry else {
        return prompt.to_string();
    };
    let ctx = hook_ctx_for_subagent(cwd, Some(agent_id), Some(agent_type));
    match coco_hooks::orchestration::execute_subagent_start(&registry, &ctx, agent_type, agent_id)
        .await
    {
        Ok(result) if !result.additional_contexts.is_empty() => {
            let blocks = result
                .additional_contexts
                .iter()
                .map(|c| format!("<hook-additional-context>\n{c}\n</hook-additional-context>"))
                .collect::<Vec<_>>()
                .join("\n\n");
            format!("{blocks}\n\n{prompt}")
        }
        Ok(_) => prompt.to_string(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                %agent_id,
                %agent_type,
                "SubagentStart (bg) hook firing failed; proceeding without injected context"
            );
            prompt.to_string()
        }
    }
}

/// Free-function variant of `fire_subagent_stop_hook` for the bg
/// path. Errors logged + swallowed.
pub(super) async fn fire_subagent_stop_for_task(
    registry: Option<Arc<coco_hooks::HookRegistry>>,
    cwd: &str,
    agent_id: &str,
    agent_type: &str,
    transcript_path: Option<&str>,
) {
    let Some(registry) = registry else { return };
    let ctx = hook_ctx_for_subagent(cwd, Some(agent_id), Some(agent_type));
    if let Err(e) = coco_hooks::orchestration::execute_subagent_stop(
        &registry,
        &ctx,
        /*stop_hook_active*/ false,
        agent_type,
        agent_id,
        transcript_path.unwrap_or(""),
        /*last_assistant_message*/ None,
    )
    .await
    {
        tracing::warn!(
            error = %e,
            %agent_id,
            %agent_type,
            "SubagentStop (bg) hook firing failed"
        );
    }
}

impl SwarmAgentHandle {
    /// Build an `OrchestrationContext` keyed off the handle's cwd.
    /// Thin wrapper around [`hook_ctx_for_cwd`] for sync-path callers
    /// that already hold `&self`.
    fn hook_orchestration_context(&self) -> coco_hooks::orchestration::OrchestrationContext {
        hook_ctx_for_cwd(&self.cwd)
    }

    /// Fire SubagentStart hooks and prepend the aggregated
    /// `additional_contexts` (each wrapped in a `<hook-additional-context>`
    /// XML block) to `prompt`. Returns `(effective_prompt, raw_result)`.
    /// Fail-open: any hook error is logged and the original prompt is
    /// returned unchanged.
    pub(super) async fn fire_subagent_start_hook(
        &self,
        agent_id: &str,
        agent_type: &str,
        prompt: &str,
    ) -> (
        String,
        Option<coco_hooks::orchestration::AggregatedHookResult>,
    ) {
        let Some(registry) = self.hook_registry().cloned() else {
            return (prompt.to_string(), None);
        };
        let ctx = self.hook_orchestration_context();
        match coco_hooks::orchestration::execute_subagent_start(
            &registry, &ctx, agent_type, agent_id,
        )
        .await
        {
            Ok(result) => {
                if result.additional_contexts.is_empty() {
                    (prompt.to_string(), Some(result))
                } else {
                    let blocks = result
                        .additional_contexts
                        .iter()
                        .map(|c| {
                            format!("<hook-additional-context>\n{c}\n</hook-additional-context>")
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    (format!("{blocks}\n\n{prompt}"), Some(result))
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    %agent_id,
                    %agent_type,
                    "SubagentStart hook firing failed; proceeding without injected context"
                );
                (prompt.to_string(), None)
            }
        }
    }

    /// Register frontmatter hooks scoped to this spawn's agent_id.
    /// Returns `true` if any hooks were registered (caller should
    /// arrange `clear_frontmatter_hooks(agent_id)` at SubagentStop).
    ///
    /// The `def.hooks` field is `serde_json::Value` because the hooks
    /// shape is an event-keyed map of arrays — same as `Settings.hooks`.
    /// We parse via `coco_hooks::load_hooks_from_config` and stamp
    /// `HookScope::Session` (agent-scoped hooks are session-priority
    /// because they're programmatic, not config).
    pub(super) fn register_frontmatter_hooks(
        &self,
        agent_id: &str,
        definition: Option<&coco_types::AgentDefinition>,
    ) -> bool {
        let Some(def) = definition else {
            return false;
        };
        if def.hooks.is_null() {
            return false;
        }
        let Some(registry) = self.hook_registry() else {
            tracing::debug!(
                agent_type = %def.agent_type,
                "agent declares frontmatter hooks but no HookRegistry is installed; skipping"
            );
            return false;
        };
        match coco_hooks::load_hooks_from_config(&def.hooks, coco_types::HookScope::Session) {
            Ok(hooks) if !hooks.is_empty() => {
                let count = hooks.len();
                // `is_agent: true` — subagent termination fires
                // SubagentStop, not Stop, so frontmatter Stop hooks
                // need rewriting (parity with TS
                // registerFrontmatterHooks.ts:38-45).
                registry.register_for_agent(agent_id.to_string(), hooks, /*is_agent*/ true);
                tracing::debug!(
                    agent_type = %def.agent_type,
                    %agent_id,
                    count,
                    "registered frontmatter hooks for agent scope"
                );
                true
            }
            Ok(_) => false,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    agent_type = %def.agent_type,
                    "failed to parse frontmatter hooks; skipping"
                );
                false
            }
        }
    }

    /// Symmetrical cleanup: drop the per-agent hook bucket.
    ///
    /// **W6.2 full**: kept (rather than deleted as "dead code")
    /// because the bg path's `tokio::spawn` closure uses
    /// `registry.clear_agent_scope` directly via a cloned Arc, and
    /// the sync path now does the same in its detached engine task.
    /// Both bypass this `&self` wrapper. Marking `#[allow(dead_code)]`
    /// keeps the helper available for future callers (e.g. error
    /// paths that abandon a spawn before the engine task runs).
    #[allow(dead_code)]
    pub(super) fn clear_frontmatter_hooks(&self, agent_id: &str) {
        if let Some(registry) = self.hook_registry() {
            registry.clear_agent_scope(agent_id);
        }
    }

    /// Initialise per-agent MCP servers from `def.mcp_servers`. For
    /// each spec:
    /// - `Name(s)` (string-ref): no mutation — we trust the parent's
    ///   pre-existing connection. The model just sees the server's
    ///   tools through the inherited `McpHandle`.
    /// - `Inline(config)`: call `add_dynamic_server(name, config)` on
    ///   the handle. Track the new name under `agent_id` so
    ///   `cleanup_per_agent_mcp` removes only the newly-created ones.
    ///   Cleanup skips string-ref entries (they point at parent-shared
    ///   connections that must not be torn down).
    ///
    /// Any inline-add failure logs at warn and continues — a failed
    /// server logs and the agent runs without that one.
    pub(super) async fn initialize_per_agent_mcp(
        &self,
        agent_id: &str,
        definition: Option<&coco_types::AgentDefinition>,
    ) {
        let Some(def) = definition else {
            return;
        };
        if def.mcp_servers.is_empty() {
            return;
        }
        let Some(handle) = self.mcp_handle() else {
            tracing::debug!(
                agent_type = %def.agent_type,
                "agent declares mcpServers but no McpHandle is installed; skipping per-agent MCP init"
            );
            return;
        };

        let mut newly_created: Vec<String> = Vec::new();
        for spec in &def.mcp_servers {
            match spec {
                coco_types::AgentMcpServerSpec::Name(_) => {
                    // String-ref → relies on parent's connection. No
                    // dynamic mutation; nothing to track for cleanup.
                }
                coco_types::AgentMcpServerSpec::Inline(map) => {
                    let Some((name, config)) = map.iter().next() else {
                        continue;
                    };
                    match handle.add_dynamic_server(name, config.clone()).await {
                        Ok(()) => {
                            tracing::debug!(
                                agent_type = %def.agent_type,
                                %agent_id,
                                server = %name,
                                "registered dynamic agent MCP server"
                            );
                            newly_created.push(name.clone());
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                agent_type = %def.agent_type,
                                %agent_id,
                                server = %name,
                                "failed to register dynamic agent MCP server; agent runs without it"
                            );
                        }
                    }
                }
            }
        }
        if !newly_created.is_empty() {
            self.dynamic_mcp_servers()
                .write()
                .await
                .insert(agent_id.to_string(), newly_created);
        }
    }

    /// Tear down dynamically-added MCP servers registered for this
    /// `agent_id`. Mirror of [`Self::initialize_per_agent_mcp`]. No-op
    /// when nothing was tracked.
    ///
    /// **W6.2 full**: see `clear_frontmatter_hooks` — both detached
    /// spawn paths now do this inline via cloned Arcs. Wrapper kept
    /// for future error-path callers.
    #[allow(dead_code)]
    pub(super) async fn cleanup_per_agent_mcp(&self, agent_id: &str) {
        let entry = self.dynamic_mcp_servers().write().await.remove(agent_id);
        let Some(names) = entry else { return };
        let Some(handle) = self.mcp_handle() else {
            return;
        };
        for name in names {
            if let Err(e) = handle.remove_dynamic_server(&name).await {
                tracing::debug!(
                    error = %e,
                    %agent_id,
                    server = %name,
                    "failed to remove dynamic agent MCP server (continuing cleanup)"
                );
            }
        }
    }

    /// Resolve every skill name in `def.skills` via the installed
    /// `SkillHandle` and prepend the loaded bodies to `prompt` as
    /// `<preloaded-skill name="...">` XML blocks. Missing skills /
    /// missing handle / read errors are logged at debug and silently
    /// dropped — the spawn proceeds with whatever loaded.
    ///
    /// `read_skill_body` enforces the author / runtime gates so
    /// `disable_model_invocation: true` skills and `off`-overridden
    /// skills are filtered out automatically.
    pub(super) async fn preload_frontmatter_skills(
        &self,
        definition: Option<&coco_types::AgentDefinition>,
        prompt: &str,
    ) -> String {
        let Some(def) = definition else {
            return prompt.to_string();
        };
        if def.skills.is_empty() {
            return prompt.to_string();
        }
        let Some(handle) = self.skill_handle() else {
            tracing::debug!(
                agent_type = %def.agent_type,
                count = def.skills.len(),
                "agent declares frontmatter skills but no SkillHandle is installed; skipping preload"
            );
            return prompt.to_string();
        };

        let runtime = self.runtime_config();
        let tiers = &runtime.skill_overrides;
        let mut blocks = Vec::with_capacity(def.skills.len());
        for name in &def.skills {
            match handle.read_skill_body(name, tiers).await {
                Some(body) if !body.trim().is_empty() => blocks.push(format!(
                    "<preloaded-skill name=\"{name}\">\n{body}\n</preloaded-skill>"
                )),
                _ => {
                    tracing::debug!(
                        agent_type = %def.agent_type,
                        skill = %name,
                        "frontmatter skill not found / gated out / empty body; skipping preload"
                    );
                }
            }
        }
        if blocks.is_empty() {
            return prompt.to_string();
        }
        format!("{}\n\n{prompt}", blocks.join("\n\n"))
    }

    /// Fire SubagentStop hooks. Errors are logged and swallowed — a
    /// failed stop hook must not gate the spawn's response. Stop hooks
    /// run for completion / failure / cancel.
    ///
    /// **W6.2 full**: see `clear_frontmatter_hooks` — both detached
    /// spawn paths now invoke `fire_subagent_stop_for_task` directly.
    /// Wrapper kept for early-failure paths that have `&self` in scope.
    #[allow(dead_code)]
    pub(super) async fn fire_subagent_stop_hook(
        &self,
        agent_id: &str,
        agent_type: &str,
        transcript_path: Option<&str>,
    ) -> Option<coco_hooks::orchestration::AggregatedHookResult> {
        let registry = self.hook_registry().cloned()?;
        let ctx = self.hook_orchestration_context();
        match coco_hooks::orchestration::execute_subagent_stop(
            &registry,
            &ctx,
            /*stop_hook_active*/ false,
            agent_type,
            agent_id,
            transcript_path.unwrap_or(""),
            /*last_assistant_message*/ None,
        )
        .await
        {
            Ok(result) => Some(result),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    %agent_id,
                    %agent_type,
                    "SubagentStop hook firing failed"
                );
                None
            }
        }
    }
}

/// Typed outcome carried on the engine-driver → inline-caller oneshot.
///
/// Replaces the previous "drop the channel when handing off to the bg
/// path" mechanism — that approach made `Err(RecvError)` ambiguous
/// (true panic vs orderly bg-handoff). With a typed sum the channel
/// is always sent on; `Err(RecvError)` is now exclusively a panic
/// signal.
///
/// Tokio's oneshot semantics force us to model the "engine completed
/// but caller already moved on" case explicitly.
#[derive(Debug)]
enum EngineOutcome {
    /// Engine ran inline; the awaiter wins the race and returns this
    /// response directly. `complete_silent` has already transitioned
    /// the task to its terminal state without pushing a notification
    /// envelope (the model gets the result via the tool result).
    ///
    /// `AgentSpawnResponse` is ~300 bytes (`Vec<PathBuf>`,
    /// `HashMap<ToolName, i32>`, multiple `Option<String>`); boxing
    /// keeps both variants the same shallow size so the small
    /// `CompletedAfterDetach` arm doesn't pay the worst-case stack
    /// cost. Per `clippy::large_enum_variant`.
    CompletedSync(Box<AgentSpawnResponse>),
    /// External `signal_detach` fired before the engine finished.
    /// The engine continued to completion in the bg, and the
    /// `<task-notification>` envelope has been pushed via
    /// `mark_completed`/`mark_failed`. The awaiter returns
    /// `AsyncLaunched` — the model already received the bg-shaped
    /// reply via the notification envelope.
    CompletedAfterDetach {
        agent_id: String,
        task_id: Option<String>,
        duration_ms: i64,
    },
}

/// Translate the engine-driver oneshot outcome into the awaiter's
/// `AgentSpawnResponse`. Recv-error (the truly degenerate path) maps
/// to a `Failed` response with the panic marker so the model isn't
/// left hanging if the engine task aborts mid-run.
fn resolve_engine_outcome(
    res: Result<EngineOutcome, tokio::sync::oneshot::error::RecvError>,
    agent_id: &str,
    task_id_for_async_response: Option<String>,
    start: Instant,
) -> AgentSpawnResponse {
    match res {
        Ok(EngineOutcome::CompletedSync(response)) => *response,
        Ok(EngineOutcome::CompletedAfterDetach {
            agent_id: detached_agent_id,
            task_id,
            duration_ms,
        }) => AgentSpawnResponse {
            status: AgentSpawnStatus::AsyncLaunched,
            agent_id: Some(task_id.unwrap_or(detached_agent_id)),
            result: None,
            error: None,
            total_tool_use_count: 0,
            total_tokens: 0,
            duration_ms,
            worktree_path: None,
            worktree_branch: None,
            output_file: None,
            prompt: None,
            ..Default::default()
        },
        Err(_) => AgentSpawnResponse {
            status: AgentSpawnStatus::Failed,
            agent_id: Some(task_id_for_async_response.unwrap_or_else(|| agent_id.to_string())),
            result: None,
            error: Some("engine task panicked".into()),
            duration_ms: start.elapsed().as_millis() as i64,
            ..Default::default()
        },
    }
}

fn spawn_task_event_drain(
    registry: coco_tool_runtime::AgentTaskRegistryRef,
    task_id: String,
    agent_type: String,
    mut event_rx: tokio::sync::mpsc::Receiver<coco_types::CoreEvent>,
) {
    tokio::spawn(async move {
        // ProgressTracker increments on every ToolUseStarted,
        // while TextDelta is appended so TaskOutput can read mid-flight
        // output.
        let mut tracker = coco_types::TaskProgress {
            // Carry the real declared type (Explore / Plan / …) so the TUI
            // can replace the `local_agent` fallback from `TaskStarted`.
            agent_type: Some(agent_type),
            ..Default::default()
        };
        tracing::debug!(task_id = %task_id, "subagent event drain started");
        // `ToolUseQueued` (carries the full input) fires before
        // `ToolUseStarted` (carries no input); stash the input-derived
        // summary keyed by call_id so the activity row can read
        // `Bash(cargo build)` rather than a bare `Bash`.
        let mut pending_summaries: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        while let Some(event) = event_rx.recv().await {
            match event {
                coco_types::CoreEvent::Stream(coco_types::AgentStreamEvent::TextDelta {
                    delta,
                    ..
                }) => {
                    registry.append_output(&task_id, &delta).await;
                }
                coco_types::CoreEvent::Stream(coco_types::AgentStreamEvent::ToolUseQueued {
                    call_id,
                    name,
                    input,
                }) => {
                    let summary = coco_types::tool_summary::tool_input_summary(&name, &input);
                    if !summary.is_empty() {
                        pending_summaries.insert(call_id, summary);
                    }
                }
                coco_types::CoreEvent::Stream(coco_types::AgentStreamEvent::ToolUseStarted {
                    call_id,
                    name,
                    ..
                }) => {
                    tracker.tool_use_count = tracker.tool_use_count.saturating_add(1);
                    tracker.last_tool_name = Some(name.clone());
                    // Maintain a cap-5 ring buffer of recent activities
                    // (MAX_RECENT_ACTIVITIES = 5). Newest at the end;
                    // renderers consume in insertion order.
                    const RECENT_ACTIVITIES_CAP: usize = 5;
                    if tracker.recent_activities.len() >= RECENT_ACTIVITIES_CAP {
                        tracker.recent_activities.remove(0);
                    }
                    tracker.recent_activities.push(coco_types::TaskActivity {
                        tool_name: name,
                        summary: pending_summaries.remove(&call_id),
                    });
                    tracing::debug!(
                        task_id = %task_id,
                        tool = tracker.last_tool_name.as_deref().unwrap_or_default(),
                        tool_use_count = tracker.tool_use_count,
                        "subagent drain: ToolUseStarted → set_progress"
                    );
                    registry.set_progress(&task_id, tracker.clone()).await;
                }
                // Per-round usage + cost roll-up so the activity panel shows
                // live token counts AND spend while the subagent runs. The
                // child engine emits `SessionUsageUpdated` after every model
                // round (`agent_adapter` installs its `CostTracker`); unlike
                // `TurnEnded` the snapshot carries engine-authoritative cost,
                // so this is the only pre-completion cost signal. Counters are
                // monotonic so an out-of-order snapshot can't roll them back.
                coco_types::CoreEvent::Protocol(
                    coco_types::ServerNotification::SessionUsageUpdated(snap),
                ) => {
                    let totals = &snap.totals;
                    let total = totals.input_tokens.saturating_add(totals.output_tokens);
                    let cost_micro = (totals.total_cost_usd * 1_000_000.0) as i64;
                    let mut changed = false;
                    if total > tracker.total_tokens {
                        tracker.total_tokens = total;
                        tracker.input_tokens = totals.input_tokens;
                        tracker.output_tokens = totals.output_tokens;
                        tracker.cache_read_tokens = totals.cache_read_input_tokens;
                        changed = true;
                    }
                    if cost_micro > tracker.cost_micro_usd {
                        tracker.cost_micro_usd = cost_micro;
                        changed = true;
                    }
                    if changed {
                        registry.set_progress(&task_id, tracker.clone()).await;
                    }
                }
                // End-of-cycle fallback: the single `TurnEnded` still carries
                // the final token total (no cost). Kept so token counts land
                // even if a usage snapshot was missed; cost arrives via the
                // completion payload.
                coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(p)) => {
                    if let Some(usage) = &p.usage {
                        let total = usage
                            .input_tokens
                            .total
                            .saturating_add(usage.output_tokens.total);
                        if total > tracker.total_tokens {
                            tracker.total_tokens = total;
                            tracker.input_tokens = usage.input_tokens.total;
                            tracker.output_tokens = usage.output_tokens.total;
                            tracker.cache_read_tokens = usage.input_tokens.cache_read;
                            registry.set_progress(&task_id, tracker.clone()).await;
                        }
                    }
                }
                _ => {}
            }
        }
        tracing::debug!(
            task_id = %task_id,
            total_tools = tracker.tool_use_count,
            "subagent event drain ended (event channel closed)"
        );
    });
}

/// Inputs for the periodic [`spawn_agent_summary_timer`]. Bundled into a
/// struct so the two spawn sites read as one unit (and to stay under the
/// argument-count lint). All fields are pre-cloned by the caller because the
/// detached timer task can't borrow `&self`.
struct AgentSummaryTimer {
    registry: coco_tool_runtime::AgentTaskRegistryRef,
    task_id: String,
    cancel: tokio_util::sync::CancellationToken,
    agent_type: String,
    engine: coco_tool_runtime::AgentQueryEngineRef,
    definition: Option<std::sync::Arc<coco_types::AgentDefinition>>,
    model_selection: coco_types::LlmModelSelection,
    session_id: String,
    /// Reader half of the child engine's per-turn message snapshot.
    live_transcript: coco_tool_runtime::LiveTranscript,
}

fn spawn_agent_summary_timer(timer: AgentSummaryTimer) {
    let AgentSummaryTimer {
        registry,
        task_id,
        cancel,
        agent_type,
        engine,
        definition,
        model_selection,
        session_id,
        live_transcript,
    } = timer;
    const AGENT_SUMMARY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    // Cap the transcript text fed to the summarizer fork so a long sub-agent
    // run can't blow its input budget (matches the prior 4 KB output tail).
    const MAX_TRANSCRIPT_CHARS: usize = 4_000;
    tokio::spawn(async move {
        let mut previous: Option<String> = None;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(AGENT_SUMMARY_INTERVAL) => {}
            }
            if registry.is_terminal(&task_id).await {
                break;
            }
            // Read the engine's live message history. Skip the tick when
            // the transcript is too short to be worth a fork (fewer than
            // 3 messages).
            let messages = live_transcript.snapshot();
            if !coco_subagent::should_summarize(messages.len()) {
                continue;
            }
            // Drop orphaned `tool_use` blocks / whitespace- and thinking-only
            // turns before rendering.
            let cleaned = coco_subagent::filter_transcript(&messages);
            let transcript = coco_subagent::render_transcript_tail(&cleaned, MAX_TRANSCRIPT_CHARS);
            if transcript.trim().is_empty() {
                continue;
            }
            let (sys, user) =
                coco_subagent::build_summary_prompts(&agent_type, previous.as_deref());
            let user_with_buf = format!("{user}\n\n--- recent transcript ---\n{transcript}");
            let identity = match coco_tool_runtime::AgentRunIdentity::new(
                session_id.clone(),
                format!("{task_id}-summary"),
                coco_tool_runtime::AgentRunKind::Summary,
            ) {
                Ok(identity) => identity,
                Err(e) => {
                    tracing::debug!(error = %e, "agent_summary: invalid identity; skipping tick");
                    continue;
                }
            };
            let summary_cfg = coco_tool_runtime::AgentQueryConfig {
                system_prompt: sys,
                identity,
                model_selection: model_selection.clone(),
                permission_mode: coco_types::PermissionMode::Default,
                permission_prompt_policy: coco_tool_runtime::PermissionPromptPolicy::FailClosed,
                max_turns: Some(1),
                allowed_tools: vec![String::new()],
                is_teammate: false,
                definition: definition.clone(),
                can_use_tool: Some(coco_tool_runtime::deny_all_handle(
                    "agent_summary: tools disabled",
                )),
                fork_label: Some(coco_types::ForkLabel::AgentSummary),
                ..Default::default()
            };
            let summary_text = match engine.execute_query(&user_with_buf, summary_cfg).await {
                Ok(resp) => resp.response_text.unwrap_or_default(),
                Err(_) => continue,
            };
            if let Some(clean) = coco_subagent::sanitize_summary(&summary_text) {
                previous = Some(clean.clone());
                registry.set_progress_summary(&task_id, clean).await;
            }
        }
    });
}

/// Sync-path early-failure response. Worktree cleanup is the caller's
/// responsibility — this helper only builds the `AgentSpawnResponse`
/// shell.
pub(super) fn spawn_failed(
    agent_id: String,
    message: String,
    duration_ms: i64,
) -> AgentSpawnResponse {
    AgentSpawnResponse {
        status: AgentSpawnStatus::Failed,
        agent_id: Some(agent_id),
        result: None,
        error: Some(message),
        total_tool_use_count: 0,
        total_tokens: 0,
        duration_ms,
        worktree_path: None,
        worktree_branch: None,
        output_file: None,
        prompt: None,
        ..Default::default()
    }
}

impl SwarmAgentHandle {
    pub(super) async fn spawn_subagent(
        &self,
        request: &AgentSpawnRequest,
    ) -> Result<AgentSpawnResponse, String> {
        let start = Instant::now();
        let agent_type = request
            .subagent_type
            .as_deref()
            .unwrap_or("general-purpose");

        let agent_id = coco_types::generate_task_id(coco_types::TaskType::BgAgent);
        let task_registry = self.task_registry().clone();
        let is_dream = matches!(request.fork_label, Some(coco_types::ForkLabel::AutoDream));
        let is_skip_registration = matches!(
            request.fork_label,
            Some(coco_types::ForkLabel::ExtractMemories)
                | Some(coco_types::ForkLabel::SessionMemoryAuto)
                | Some(coco_types::ForkLabel::SessionMemoryManual)
                | Some(coco_types::ForkLabel::PromptSuggestion)
                | Some(coco_types::ForkLabel::SideQuestion)
                | Some(coco_types::ForkLabel::Compact)
                | Some(coco_types::ForkLabel::AgentSummary)
                | Some(coco_types::ForkLabel::Speculation)
        );
        tracing::info!(
            agent_id = %agent_id,
            agent_type = %agent_type,
            run_in_background = request.run_in_background,
            isolation = ?request.isolation,
            spawn_mode = ?request.spawn_mode,
            "subagent spawn dispatch"
        );

        // Worktree isolation: any creation error returns a model-visible
        // failure — never silently fall back to sync-without-isolation.
        let worktree_session = if request.isolation == Some(coco_types::AgentIsolation::Worktree) {
            match self.worktree_manager() {
                Some(m) => {
                    let slug = format!(
                        "agent-{}",
                        agent_id
                            .strip_prefix("agent-")
                            .unwrap_or(&agent_id)
                            .chars()
                            .take(8)
                            .collect::<String>()
                    );
                    match m.create_for(&slug) {
                        Ok(s) => {
                            // Fire WorktreeCreate hook so user hooks can
                            // react to per-agent worktree creation.
                            // Coco-rs always uses git internally — the
                            // hook is observe-only.
                            fire_worktree_create_hook(
                                self.hook_registry().cloned(),
                                &self.cwd,
                                &slug,
                            )
                            .await;
                            Some(s)
                        }
                        Err(e) => {
                            return Ok(spawn_failed(
                                agent_id,
                                format!("Worktree creation failed: {e}"),
                                start.elapsed().as_millis() as i64,
                            ));
                        }
                    }
                }
                None => {
                    return Ok(spawn_failed(
                        agent_id,
                        "Isolation 'worktree' requested but no AgentWorktreeManager is \
                         configured. Use SwarmAgentHandle::set_worktree_manager."
                            .into(),
                        start.elapsed().as_millis() as i64,
                    ));
                }
            }
        } else {
            None
        };

        let Some(engine) = self.execution_engine() else {
            if let (Some(m), Some(session)) = (self.worktree_manager(), worktree_session.clone()) {
                let removed_path = session.path.display().to_string();
                let _ = m.cleanup_if_unchanged(session);
                fire_worktree_remove_hook(self.hook_registry().cloned(), &self.cwd, &removed_path)
                    .await;
            }
            return Ok(spawn_failed(
                agent_id,
                "No AgentQueryEngine configured on SwarmAgentHandle. Use \
                 SwarmAgentHandle::set_execution_engine at session bootstrap."
                    .into(),
                start.elapsed().as_millis() as i64,
            ));
        };

        let cwd_override = worktree_session
            .as_ref()
            .map(|s| s.path.clone())
            .or_else(|| request.cwd.clone());

        // Spawn-time identity resolution (T3 + T7). Single source of
        // truth for model routing: `AgentDefinition`. No per-request
        // override slots — model + role flow exclusively from the
        // definition (whether loaded from .md/built-in catalog or
        // synthesized in-process by code-driven forks like the
        // memory crate's extract/dream/session services).
        //
        //   model:  definition.model > role-resolved (via ModelSpec)
        //   role:   definition.model_role > subagent_type → role
        //         > ModelRole::Subagent
        //
        // The model-facing AgentTool schema does NOT expose `model`
        // or `model_role` — multi-LLM design rules out LLM-driven
        // model selection (LLM has no awareness of operator's
        // provider/model_id mappings).
        //
        // The definition flows through `AgentSpawnRequest.definition`,
        // populated either by AgentTool from `ctx.agent_catalog` (LLM
        // path) or by internal callers constructing a synthetic def
        // at spawn time (memory crate forks).
        //
        // When `definition` is `None` (test contexts), the resolver
        // degrades cleanly to subagent_type→role mapping.
        let agent_type_id: Option<coco_types::AgentTypeId> = request
            .subagent_type
            .as_deref()
            .map(|t| t.parse().expect("AgentTypeId::from_str is Infallible"));
        let selection = coco_subagent::resolve_subagent_selection(
            request.definition.as_deref(),
            agent_type_id.as_ref(),
        );

        // Resolve the prior-history + system-prompt pair from the
        // requested spawn mode:
        //
        // - Fresh     → no history; system prompt seeded from
        //               `definition.system_prompt`. Built-ins populate
        //               this via [`coco_subagent::builtin_prompts`];
        //               markdown agents via the body of their `.md`
        //               file. Without this, the child would fall
        //               through to the engine's generic default
        //               instead of receiving its role instructions.
        // - Fork      → parent's pre-rendered system-prompt bytes
        //               verbatim (cache parity), parent history threaded
        //               through with real `tool_result` bodies intact.
        // - Resume    → seed from `definition.system_prompt` like
        //               Fresh; prior history kept verbatim (NO
        //               placeholder rewrite — the child needs real
        //               tool outputs to continue).
        //
        // `preserve_tool_use_results` flips on for Fork and Resume so
        // downstream compaction doesn't strip the inherited results.
        // Resolve cwd + model once — both feed into env_info / CLAUDE.md
        // discovery / per-agent memory.
        let cwd_for_prompt = cwd_override
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(&self.cwd));
        // Model resolution by spawn mode:
        //
        // - **Fork**: pin to the snapshot's `api_model_name` (carried
        //   non-optionally inside `SpawnMode::Fork`). The whole point
        //   of fork is prompt-cache parity; reading live
        //   `RuntimeConfig` here would silently bust the cache after
        //   a hot-reload between the parent's last turn and this
        //   spawn. Used for BOTH env-block rendering AND the actual
        //   `AgentQueryConfig.model_selection` below — they must agree.
        //
        // - **Resume**: rebuild fresh from current runtime. Resume
        //   restarts a previously-backgrounded agent in a (possibly
        //   different) process; pinning to a snapshot captured *now*
        //   at engine bootstrap would conflate "current parent" with
        //   "original spawn" and is meaningless.
        //
        // - **Fresh**: `def.model` > role-resolved primary.
        // Fork pins the model to the parent's captured identity for
        // prompt-cache parity. We carry BOTH the `api_model_name` (for the
        // `<env>` block + `AgentQueryConfig.model_selection`) AND the provider, so
        // the selection below can be an `Explicit { provider, model_id }`
        // that resolves the SAME provider config the parent used. A bare
        // model name would parse as neither `Explicit` nor a role and fall
        // back to live `Role { Main }` resolution — which a mid-session
        // role remap or hot-reload could repoint to a different provider,
        // the exact cache bust this snapshot exists to prevent. (base_url /
        // wire_api still resolve from that provider's config by name;
        // pinning the provider closes the role-remap gap.)
        let fork_pin = match &request.spawn_mode {
            coco_tool_runtime::SpawnMode::Fork {
                parent_snapshot, ..
            } => Some((
                parent_snapshot.provider.clone(),
                parent_snapshot.api_model_name.clone(),
            )),
            _ => None,
        };
        let fork_model_selection =
            fork_pin.map(
                |(provider, model_id)| coco_types::LlmModelSelection::Explicit {
                    primary: coco_types::ProviderModelSelection { provider, model_id },
                },
            );
        // Single source of truth for the child's model: the same typed
        // selection feeds BOTH the `<env>` block display AND the engine
        // factory's routing, so the displayed model can never disagree
        // with the one actually resolved.
        //
        // - **Fork**: pin the parent's exact (provider, model) for
        //   prompt-cache parity (see `fork_pin` above).
        // - **Plan mode (non-fork)**: promote to `ModelRole::Plan` so a
        //   custom agent called with `mode: plan` reasons on the Plan
        //   model regardless of its declared role (keeping any explicit
        //   definition model as the primary).
        // - **Fresh/Resume**: the spawn-resolved selection
        //   (`definition.model` > role-resolved).
        let effective_model_selection = if let Some(sel) = fork_model_selection {
            sel
        } else if request.mode == Some(coco_types::PermissionMode::Plan)
            && !matches!(
                request.spawn_mode,
                coco_tool_runtime::SpawnMode::Fork { .. }
            )
        {
            coco_types::LlmModelSelection::from_model_and_role(
                selection.model.as_deref(),
                Some(coco_types::ModelRole::Plan),
            )
        } else {
            selection.model_selection.clone()
        };
        // Bare, catalog-canonical model id for the env block, resolved
        // from that same selection (Role/InheritMain → role-resolved id
        // with Main fallback; Explicit → its bare `model_id`).
        let model_for_env = self.model_id_for_env(&effective_model_selection);
        // `dirs::home_dir()` can return `None` on minimal containers
        // (no `$HOME`, no passwd entry). The legacy code fell back to
        // `/tmp`, which silently routed memory lookups to the wrong
        // User-scope per-agent memory follows `COCO_CONFIG_HOME` (via
        // `global_config::config_home()`), NOT the system home dir.
        // Multi-tenant / containerised setups where `~/.coco` is
        // unwritable still get a usable agent-memory dir. Project /
        // Local scopes are per-repo and ignore `config_home`.
        let config_home = coco_config::global_config::config_home();

        // Per-agent memory block. Fork inherits parent's rendered prompt
        // verbatim so memory injection is skipped there.
        let inject_memory = !matches!(
            request.spawn_mode,
            coco_tool_runtime::SpawnMode::Fork { .. }
        );
        let memory_block = match (
            inject_memory,
            request.definition.as_deref().and_then(|d| d.memory_scope),
        ) {
            (true, Some(scope)) => Some(coco_memory::agent_memory::load_agent_memory_prompt(
                agent_type,
                scope,
                &cwd_for_prompt,
                &config_home,
            )),
            _ => None,
        };

        // Build the Fresh/Resume system prompt via the shared
        // `coco_context::build_system_prompt` assembler — same code path
        // the leader uses. This restores:
        //   - <env>...</env> block (Working directory, git repo Y/N,
        //     Platform, Shell, OS Version, model line, knowledge cutoff)
        //   - 4 AGENT_NOTES bullets (absolute paths, no emojis, …)
        //   - CLAUDE.md discovery (gated by `def.omit_claude_md`)
        //   - Memory block (appended at the correct cache-broken
        //     position)
        //
        // For `coco-guide` specifically: the agent's static identity is
        // augmented with a per-spawn dynamic context block listing the
        // user's custom skills / agents / MCP servers / plugin commands /
        // settings.json snapshot. The builder closure is installed by
        // the CLI bootstrap; absent installation the spawn falls back to
        // the static base only.
        let coco_guide_context_builder = self.coco_guide_context_builder().cloned();
        let build_fresh_prompt = || -> String {
            let def = request.definition.as_deref();
            let static_identity = def
                .and_then(|d| d.system_prompt.as_deref())
                .filter(|s| !s.is_empty())
                .unwrap_or(coco_context::prompt::DEFAULT_AGENT_IDENTITY);
            // Append the coco-guide dynamic block when this spawn is
            // for the `coco-guide` agent AND a context builder is
            // installed. Owned `String` so the assembler below can
            // borrow with `&str` like the static path. Empty result
            // (every section omitted) is treated identically to
            // "no builder installed".
            let identity_owned: Option<String> =
                if agent_type == coco_types::SubagentType::CocoGuide.as_str() {
                    coco_guide_context_builder.as_ref().and_then(|build| {
                        let ctx = build();
                        coco_subagent::coco_guide_dynamic_block(&ctx)
                            .map(|block| format!("{static_identity}{block}"))
                    })
                } else {
                    None
                };
            let identity: &str = identity_owned.as_deref().unwrap_or(static_identity);
            let claude_md_files: Vec<coco_context::MemoryFile> =
                if def.map(|d| d.omit_claude_md).unwrap_or(false) {
                    Vec::new()
                } else {
                    coco_context::discover_memory_files(&cwd_for_prompt)
                };
            // Suppress git status under COCO_REMOTE or a disabled
            // `include_git_instructions` setting.
            let git_env = coco_config::EnvSnapshot::from_current_process();
            let include_git_status = !git_env.is_truthy(coco_config::EnvKey::CocoRemote)
                && coco_config::gitsettings::should_include_git_instructions(
                    &self.runtime_config().settings.merged,
                    &git_env,
                );
            let env_info = coco_context::get_environment_info(
                &cwd_for_prompt,
                &model_for_env,
                include_git_status,
            );
            coco_context::build_system_prompt(
                identity,
                &claude_md_files,
                &env_info,
                // `skill_listing` here is the system-prompt slot — never
                // populated by the main agent either (skill listing flows
                // via `coco_system_reminder::generators::skill_listing`
                // per-turn, not as a baked-in section). Subagent matches
                // main agent: pass None. Separate gap covered below
                // wires the skill listing through the subagent's
                // GeneratorContext so the per-turn reminder fires.
                /*skill_listing=*/
                None,
                memory_block.as_deref(),
                // AGENT_NOTES via `notes_after_env` — the subagent path
                // bundles `notes` with the env block. By passing them
                // through this slot they render BEFORE memory, not after.
                // Main agent path passes `None` because it uses richer
                // per-section rules instead of these 4 condensed bullets.
                Some(coco_context::prompt::AGENT_NOTES),
                /*output_style=*/ None,
                /*additional_working_directories=*/ &[],
            )
            .full_text()
        };

        // Resolve (system_prompt, prior_messages, preserve_tool_use_results)
        // by spawn mode. `is_fork` controls whether the child's first
        // user turn gets wrapped in `<fork-boilerplate>` XML so the
        // recursion guard ([`coco_subagent::is_in_fork_child`]) can
        // detect fork-of-fork and the worker receives its rules. TS
        // parity: `forkSubagent.ts::buildChildMessage`.
        let (system_prompt, fork_context_messages, preserve_tool_use_results, is_fork) =
            match &request.spawn_mode {
                coco_tool_runtime::SpawnMode::Fork {
                    rendered_system_prompt,
                    parent_messages,
                    parent_snapshot: _,
                } => {
                    // Fork MUST use the parent's pre-rendered prompt verbatim
                    // AND its conversation history with real tool results
                    // intact, so the child's request prefix is byte-identical
                    // to the parent's (prompt-cache hit) and the child sees
                    // the output the parent gathered. `preserve_tool_use_results
                    // = true` keeps the results through compaction. The
                    // parent's pre-response snapshot has only complete
                    // tool_use/result pairs, so no rewrite/filter is needed.
                    (
                        rendered_system_prompt.clone(),
                        parent_messages.clone(),
                        true,
                        true,
                    )
                }
                coco_tool_runtime::SpawnMode::Resume { parent_messages } => {
                    (build_fresh_prompt(), parent_messages.clone(), true, false)
                }
                coco_tool_runtime::SpawnMode::Fresh => (
                    build_fresh_prompt(),
                    request.fork_context_messages.clone(),
                    false,
                    false,
                ),
                // `SpawnMode` is `#[non_exhaustive]` (cross-crate), so the
                // compiler forces a wildcard. Future variants MUST be
                // wired explicitly at this seam — failing fast beats
                // a Fresh fallback that silently degrades cache parity
                // / recursion guarantees the new variant might rely on.
                other => {
                    return Ok(spawn_failed(
                        agent_id,
                        format!(
                            "Unhandled SpawnMode variant {other:?}; the coordinator must be \
                             updated to thread the new mode through `spawn_subagent`."
                        ),
                        start.elapsed().as_millis() as i64,
                    ));
                }
            };
        // Live transcript: when this spawn drives a periodic AgentSummary
        // timer, hand the child engine a shared, per-turn snapshot of its
        // message history so the timer summarizes the real transcript
        // (TS `agentSummary.ts` `getAgentTranscript`) rather than the raw
        // output buffer. `None` when summaries are off → the engine skips the
        // snapshot push entirely (zero cost on the non-summarized hot path).
        // The same handle flows into `spawn_background` via `query_config`.
        let live_transcript = request
            .enable_summarization
            .then(coco_tool_runtime::LiveTranscript::new);
        let mut query_config = coco_tool_runtime::AgentQueryConfig {
            system_prompt,
            identity: match coco_tool_runtime::AgentRunIdentity::new(
                request.session_id.clone(),
                agent_id.clone(),
                coco_tool_runtime::AgentRunKind::Subagent,
            ) {
                Ok(identity) => identity,
                Err(e) => {
                    return Ok(spawn_failed(
                        agent_id,
                        e,
                        start.elapsed().as_millis() as i64,
                    ));
                }
            },
            // Routing uses the SAME selection that drove `model_for_env`
            // above (fork pin > plan-mode promotion > spawn-resolved).
            model_selection: effective_model_selection,
            permission_mode: request.mode.unwrap_or(coco_types::PermissionMode::Default),
            permission_prompt_policy: if request.run_in_background || request.fork_label.is_some() {
                coco_tool_runtime::PermissionPromptPolicy::FailClosed
            } else {
                coco_tool_runtime::PermissionPromptPolicy::PromptAllowed
            },
            // Read-scope inheritance: forward the parent's read working dirs so
            // an isolated-worktree subagent can read the parent project without
            // a prompt (TS subagent cwd + additionalWorkingDirectories parity).
            inherited_read_dirs: request.inherited_read_dirs.clone(),
            // `max_turns` precedence: constraints (memory forks tighten
            // via `AgentSpawnConstraints.max_turns`) > definition. Top-
            // level `request.max_turns` was a dead slot and is gone.
            max_turns: request
                .constraints
                .as_ref()
                .and_then(|c| c.max_turns)
                .or_else(|| request.definition.as_ref().and_then(|d| d.max_turns)),
            context_window: None,
            prompt_cache: None,
            max_output_tokens: None,
            // Coordinator-mode tool pool: when the leader is in coordinator
            // mode, AgentTool spawns are workers and must see only the
            // worker tool pool. Outside coordinator mode the child's own
            // `AgentDefinition.allowed_tools` (frontmatter `tools:`) is
            // threaded through as the child engine's `ToolFilter`
            // allow-list — resolved against the real registry in
            // `agent_adapter` and narrowed by the parent filter.
            // `Wildcard` (`tools: undefined` / `['*']`) stays empty =
            // permissive; an `Explicit` list restricts. Threading it here
            // is load-bearing: leaving it empty silently dropped the
            // allow-list so a restricted custom agent ran with the
            // parent's full tool surface.
            allowed_tools: if request
                .features
                .as_deref()
                .is_some_and(coco_subagent::is_coordinator_mode)
            {
                let simple_mode = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoSimple);
                coco_subagent::worker_tool_pool(simple_mode)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            } else {
                match request.definition.as_ref().map(|d| &d.allowed_tools) {
                    // Normalise `Bash(*)` → `Bash`: a `ToolFilter` matches
                    // by `ToolId`, so a parenthesised entry would parse to
                    // `Custom("Bash(*)")` and never match the real tool.
                    Some(coco_types::ToolAllowList::Explicit(list)) => {
                        coco_subagent::parse_tool_allow_list(list)
                            .into_iter()
                            .map(str::to_string)
                            .collect()
                    }
                    _ => Vec::new(),
                }
            },
            // `disallowed_tools` = the agent's own deny-list (frontmatter)
            // PLUS the universal subagent block (applied before the
            // allow-list): Agent / AskUserQuestion / TaskOutput / TaskStop /
            // Enter+ExitPlanMode are denied for every spawned subagent
            // regardless of its `tools:` — otherwise a wildcard (default) or
            // self-listing subagent could spawn nested agents, prompt the
            // user, etc. `ExitPlanMode` is re-admitted in plan mode. coco
            // enforces per-id via `ToolFilter::allows`, so these names drop
            // from the child's tool list.
            disallowed_tools: {
                let mut denied = request
                    .definition
                    .as_ref()
                    .map(|d| d.disallowed_tools.clone())
                    .unwrap_or_default();
                let plan_mode = request.mode == Some(coco_types::PermissionMode::Plan);
                for name in coco_subagent::subagent_disallowed_tools(plan_mode) {
                    if !denied.iter().any(|d| d == name) {
                        denied.push(name.to_string());
                    }
                }
                // Async clamp (TS `filterToolsForAgent` parity): a
                // background subagent is restricted to the async-safe tool
                // set — every non-async-safe built-in is denied (MCP tools
                // pass through). Coordinator-mode spawns already narrow via
                // `worker_tool_pool` on the allow-list side; forks inherit
                // the parent's exact tool pool. So this only applies to a
                // plain background AgentTool spawn.
                let is_coordinator = request
                    .features
                    .as_deref()
                    .is_some_and(coco_subagent::is_coordinator_mode);
                let is_async = request.run_in_background
                    || request
                        .definition
                        .as_ref()
                        .map(|d| d.background)
                        .unwrap_or(false);
                let is_fork = matches!(
                    request.spawn_mode,
                    coco_tool_runtime::SpawnMode::Fork { .. }
                );
                if is_async && !is_coordinator && !is_fork {
                    for name in coco_subagent::async_subagent_disallowed_tools(plan_mode) {
                        if !denied.iter().any(|d| d == name) {
                            denied.push(name.to_string());
                        }
                    }
                }
                denied
            },
            // Coordinator / AgentTool spawns don't carry skill-style
            // auto-allow rules — those flow only through
            // `SkillRuntime` Fork path. Leave empty.
            extra_permission_rules: Vec::new(),
            live_permission_rules: None,
            live_permission_mode: None,
            tool_overrides: request.tool_overrides.clone(),
            features: request.features.clone(),
            skill_overrides: request.skill_overrides.clone(),
            parent_tool_filter: request.parent_tool_filter.clone(),
            active_shell_tool: request.active_shell_tool,
            preserve_tool_use_results,
            is_teammate: false,
            is_in_process_teammate: false,
            plan_mode_required: false,
            // Subagents/teammates never inherit the leader's bypass capability
            // (A7b) — always gated. See the teammate site in `mod.rs`.
            bypass_permissions_available: false,
            cwd_override,
            fork_context_messages,
            allowed_write_roots: request
                .constraints
                .as_ref()
                .map(|c| c.allowed_write_roots.clone())
                .unwrap_or_default(),
            // `AgentDefinition.effort` is the single source of truth
            // for static effort overrides. Read it here (was: blank
            // pass-through of the never-set `request.effort`). The
            // resolved string passes through to RunnerConfig and is
            // looked up against the active model's
            // `supported_thinking_levels` by
            // `session_runtime::thinking_level_for_effort_from`.
            effort: request.definition.as_ref().and_then(|d| d.effort),
            // The four fields below all read from `AgentDefinition` —
            // the previously-dead `request.<field>` pass-through slots
            // are gone.
            use_exact_tools: request
                .definition
                .as_ref()
                .map(|d| d.use_exact_tools)
                .unwrap_or(false),
            mcp_servers: request
                .definition
                .as_ref()
                .map(|d| {
                    d.mcp_servers
                        .iter()
                        .filter_map(|spec| spec.name().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            initial_prompt: request
                .definition
                .as_ref()
                .and_then(|d| d.initial_prompt.clone()),
            definition: request.definition.clone(),
            // In-process AgentTool spawns inherit the leader's
            // `ToolPermissionBridge` via `wire_engine`. Setting an override
            // here would mask whatever the leader installed (SDK
            // askForApproval / TUI overlay / legacy auto-allow).
            // Cross-process pane teammates use `MailboxPermissionBridge`
            // (file IPC) instead.
            permission_bridge: None,
            // The bg path below populates a per-task event channel so live
            // text deltas reach the task's output buffer.
            event_tx: None,
            wire_dump: None,
            // Per-fork canUseTool callback inherits from the request. The
            // AgentTool spawn path doesn't set one by default; memory /
            // dream / session services thread their per-policy handle
            // via `request.can_use_tool` after PR 4.
            can_use_tool: request.can_use_tool.clone(),
            require_can_use_tool: request.require_can_use_tool,
            fork_label: request.fork_label,
            cancel: None,
            live_transcript: live_transcript.clone(),
        };

        if request.run_in_background {
            return self
                .spawn_background(
                    request,
                    agent_id,
                    agent_type,
                    query_config,
                    worktree_session,
                    start,
                    engine,
                    is_fork,
                )
                .await;
        }

        // ── Frontmatter hooks registration ──
        //
        // Register `def.hooks` under the spawn's agent_id so they're
        // visible to every event firing during this spawn. Cleared at
        // SubagentStop below. No-op when def.hooks is null / hook
        // registry unwired.
        let registered_frontmatter_hooks =
            self.register_frontmatter_hooks(&agent_id, request.definition.as_deref());

        // ── Per-agent MCP servers ──
        //
        // String-ref entries piggyback on the parent's pre-existing
        // connections; inline `{name: config}` entries get registered
        // as dynamic servers and torn down at SubagentStop.
        self.initialize_per_agent_mcp(&agent_id, request.definition.as_deref())
            .await;

        // ── Frontmatter skills preload ──
        //
        // When `def.skills` is non-empty, resolve each skill's body
        // via the installed `SkillHandle` and prepend the loaded
        // contents to the prompt — wrapped in `<preloaded-skill>`
        // blocks so the model can distinguish them from the actual
        // task prompt.
        let prompt_with_skills = self
            .preload_frontmatter_skills(request.definition.as_deref(), &request.prompt)
            .await;

        // ── SubagentStart hook firing ──
        //
        // Fire user-defined SubagentStart hooks and inject their
        // `additional_contexts` into the child's prompt as a leading
        // system-reminder block.
        let (decorated_prompt, _start_result) = self
            .fire_subagent_start_hook(&agent_id, agent_type, &prompt_with_skills)
            .await;

        // Fork mode: wrap the decorated directive in the
        // `<fork-boilerplate>...</fork-boilerplate>` envelope so:
        //   - the worker receives its rules (no-converse, scope-bound,
        //     report-format).
        //   - the conversation contains the boilerplate tag so a future
        //     `is_in_fork_child(parent_messages)` scan blocks recursive
        //     forking.
        let effective_prompt = if is_fork {
            coco_subagent::build_fork_child_message(&decorated_prompt)
        } else {
            decorated_prompt
        };

        // W4 (B1 fix): register the sync agent in TaskRuntime when
        // a registry is wired. Sync agents populate `appState.tasks`
        // so the UI panel + TaskList tool see them as Running.
        // Without this, sync agents were invisible — only background
        // agents were tracked.
        //
        // W6 (Dream registration): when the spawn carries the
        // `AutoDream` fork label, register as `TaskType::Dream`
        // instead of `LocalAgent` so the TUI panel + `TaskList` tool
        // can differentiate auto-memory consolidation from
        // user-spawned subagents. Other framework-spawned forks (extract /
        // session-memory / prompt-suggestion / side-question /
        // compact / agent-summary / speculation) are
        // service-internal and don't surface as user-visible
        // tasks — not registered as user-visible tasks.
        //
        // The cancel token from `register_*_task` lets `kill_task`
        // propagate into the engine via the `tokio::select!` race
        // below — same mechanism the bg path uses.
        let sync_task = if is_skip_registration {
            None
        } else {
            let task_cancel = tokio_util::sync::CancellationToken::new();
            let description = request
                .description
                .clone()
                .unwrap_or_else(|| agent_type.to_string());
            let tid = if is_dream {
                task_registry
                    .register_dream_task(&description, task_cancel.clone())
                    .await
            } else {
                // Foreground spawns (run_in_background=false) optionally arm the
                // auto-detach timer. Background spawns ignore the
                // field — they detach immediately by definition.
                let registration = match request
                    .auto_background_ms
                    .filter(|_| !request.run_in_background)
                {
                    Some(ms) => {
                        coco_tool_runtime::AgentRegistration::ForegroundWithAutoDetach { ms }
                    }
                    None => coco_tool_runtime::AgentRegistration::Foreground,
                };
                task_registry
                    .register_agent_task_with_id(
                        agent_id.clone(),
                        &description,
                        request.tool_use_id.as_deref(),
                        request.invoking_agent_id.as_deref(),
                        task_cancel.clone(),
                        registration,
                    )
                    .await
            };
            Some((tid, task_cancel))
        };

        // W6.2 (full): the entire sync execution path — engine call,
        // cleanup chain, response build — now lives inside a detached
        // `tokio::spawn` body. The inline caller races a oneshot
        // receiver against the detach signal and external cancel:
        //
        // - **Oneshot delivery**: engine task finishes, sends the
        //   built `AgentSpawnResponse`. Inline caller returns it.
        //   `complete_silent` runs (state only, no notification —
        //   response goes inline).
        // - **Detach signal**: external `signal_detach(tid)` (TUI
        //   Ctrl+B). Inline caller sets the detached flag and
        //   returns `AsyncLaunched` immediately. Engine task keeps
        //   running, eventually calls `mark_completed`/`mark_failed`
        //   on its own (pushes `<task-notification>` envelope).
        //   This is the "detach but keep running" behavior.
        // - **External cancel** (`kill_task(tid)`): engine task's
        //   inner select observes the cancel and exits with an Err
        //   QueryResult. Cleanup still runs in the engine task; the
        //   final response is `AgentSpawnResponse::Failed`. Inline
        //   caller receives it via oneshot.
        //
        // Pre-clone every Arc the engine task needs (so the closure
        // doesn't borrow `&self`). Matches the pattern used by
        // `spawn_background` below.
        let hook_registry_for_engine = self.hook_registry().cloned();
        let mcp_handle_for_engine = self.mcp_handle().cloned();
        let dynamic_mcp_servers_for_engine = self.dynamic_mcp_servers().clone();
        let worktree_manager_for_engine = self.worktree_manager().cloned();
        let side_query_for_engine = self.side_query().cloned();
        let cwd_for_engine = self.cwd.clone();
        let task_registry_for_engine = task_registry.clone();
        let agent_id_for_engine = agent_id.clone();
        let agent_type_for_engine = agent_type.to_string();
        // Permission mode gates the post-spawn handoff classifier (auto
        // only). Pre-cloned so the detached engine task can read it
        // without borrowing `request` across the `await`.
        let mode_for_engine = request.mode;
        let task_id_for_engine = sync_task.as_ref().map(|(id, _)| id.clone());
        let task_cancel_for_engine = sync_task.as_ref().map(|(_, c)| c.clone());
        let worktree_session_for_engine = worktree_session.clone();
        let registered_frontmatter_hooks_for_engine = registered_frontmatter_hooks;

        // Detach handle for the inline caller's race. Internal memory
        // forks skip TaskManager registration, so they have no detach arm.
        let detach_handle_for_inline: Option<std::sync::Arc<tokio::sync::Notify>> =
            match sync_task.as_ref() {
                Some((tid, _)) => task_registry.detach_handle(tid).await,
                None => None,
            };

        if let Some((tid, task_cancel)) = sync_task.as_ref() {
            let (event_tx, event_rx) = tokio::sync::mpsc::channel::<coco_types::CoreEvent>(64);
            query_config.event_tx = Some(event_tx);
            spawn_task_event_drain(
                task_registry.clone(),
                tid.clone(),
                agent_type_for_engine.clone(),
                event_rx,
            );
            // `live_transcript` is `Some` iff `request.enable_summarization`,
            // so this gates the timer on the same condition while handing it
            // the reader half of the engine's snapshot sink.
            if let Some(live) = live_transcript.clone() {
                spawn_agent_summary_timer(AgentSummaryTimer {
                    registry: task_registry.clone(),
                    task_id: tid.clone(),
                    cancel: task_cancel.clone(),
                    agent_type: agent_type.to_string(),
                    engine: engine.clone(),
                    definition: request.definition.clone(),
                    model_selection: query_config.model_selection.clone(),
                    session_id: query_config.identity.session_id.clone(),
                    live_transcript: live,
                });
            }
        }

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<EngineOutcome>();
        let detached_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let detached_flag_for_engine = detached_flag.clone();
        let sync_start = start;
        let engine_for_task = engine.clone();

        // Parent-turn interrupt propagation (TS shared-`AbortController`
        // parity). A foreground subagent that has **not** detached is
        // cancelled when the parent turn is aborted (e.g. user ESC);
        // otherwise the detached engine task below would run to completion
        // while the parent already recorded the `Agent(...)` tool call as
        // interrupted — the desync that left the subagent panel stuck.
        // Exemptions: background spawns (`run_in_background`) and agents that
        // detach mid-run (`detached_flag`) are meant to outlive the turn.
        // `spawn_complete` releases the linker on normal completion so it
        // never outlives the engine task.
        let spawn_complete = tokio_util::sync::CancellationToken::new();
        if !request.run_in_background
            && let Some(parent_abort) = request.parent_turn_abort.clone()
            && let Some((_, task_cancel)) = sync_task.as_ref()
        {
            let task_cancel = task_cancel.clone();
            let detached_flag_for_link = detached_flag.clone();
            let spawn_complete_for_link = spawn_complete.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = parent_abort.cancelled() => {
                        if !detached_flag_for_link.load(std::sync::atomic::Ordering::SeqCst) {
                            task_cancel.cancel();
                        }
                    }
                    () = spawn_complete_for_link.cancelled() => {}
                }
            });
        }
        let spawn_complete_for_engine = spawn_complete.clone();

        tokio::spawn(async move {
            // ── Engine query (race against task cancel) ──────────
            let query_result = if let Some(c) = task_cancel_for_engine.as_ref() {
                tokio::select! {
                    biased;
                    () = c.cancelled() => {
                        Err(Box::new(coco_error::PlainError::new(
                            "task cancelled by leader",
                            coco_error::StatusCode::Cancelled,
                        )) as coco_error::BoxedError)
                    }
                    r = engine_for_task.execute_query(&effective_prompt, query_config) => r,
                }
            } else {
                engine_for_task
                    .execute_query(&effective_prompt, query_config)
                    .await
            };
            let duration_ms = sync_start.elapsed().as_millis() as i64;

            // ── Cleanup chain (no `&self` — uses cloned Arcs) ─────
            fire_subagent_stop_for_task(
                hook_registry_for_engine.clone(),
                &cwd_for_engine,
                &agent_id_for_engine,
                &agent_type_for_engine,
                /*transcript*/ None,
            )
            .await;

            if registered_frontmatter_hooks_for_engine
                && let Some(reg) = hook_registry_for_engine.as_ref()
            {
                reg.clear_agent_scope(&agent_id_for_engine);
            }

            let dynamic_names = dynamic_mcp_servers_for_engine
                .write()
                .await
                .remove(&agent_id_for_engine);
            if let (Some(names), Some(handle)) = (dynamic_names, mcp_handle_for_engine.as_ref()) {
                for name in names {
                    if let Err(e) = handle.remove_dynamic_server(&name).await {
                        tracing::debug!(
                            error = %e,
                            agent_id = %agent_id_for_engine,
                            server = %name,
                            "sync cleanup: failed to remove dynamic agent MCP server"
                        );
                    }
                }
            }

            let (worktree_path, worktree_branch) = match (
                worktree_manager_for_engine.as_ref(),
                worktree_session_for_engine,
            ) {
                (Some(m), Some(session)) => {
                    let session_path = session.path.display().to_string();
                    match m.cleanup_if_unchanged(session) {
                        crate::worktree::WorktreeCleanupOutcome::Removed => {
                            fire_worktree_remove_hook(
                                hook_registry_for_engine.clone(),
                                &cwd_for_engine,
                                &session_path,
                            )
                            .await;
                            (None, None)
                        }
                        crate::worktree::WorktreeCleanupOutcome::Kept { path, branch, .. } => {
                            (Some(path), Some(branch))
                        }
                    }
                }
                _ => (None, None),
            };

            // ── Build AgentSpawnResponse ──────────────────────────
            // Capture the real USD cost before the match consumes
            // `query_result`; the bg/detach completion path below only
            // has `response` in scope, not `qr`.
            let cost_usd = query_result
                .as_ref()
                .ok()
                .map(|qr| qr.cost_usd)
                .unwrap_or(0.0);
            // Full token breakdown (incl cache) for the completion payload.
            let token_usage = query_result
                .as_ref()
                .ok()
                .map(|qr| qr.usage)
                .unwrap_or_default();
            let response = match query_result {
                Ok(qr) => {
                    tracing::info!(
                        agent_id = %agent_id_for_engine,
                        agent_type = %agent_type_for_engine,
                        tool_use_count = qr.tool_use_count,
                        tokens_in = qr.input_tokens,
                        tokens_out = qr.output_tokens,
                        duration_ms,
                        "subagent spawn ok"
                    );
                    let response_text = super::handoff::classify_handoff_inline(
                        &agent_type_for_engine,
                        &qr,
                        side_query_for_engine.as_ref(),
                        mode_for_engine,
                    )
                    .await;
                    AgentSpawnResponse {
                        status: AgentSpawnStatus::Completed,
                        agent_id: Some(agent_id_for_engine.clone()),
                        result: response_text,
                        error: None,
                        total_tool_use_count: qr.tool_use_count,
                        total_tokens: qr.input_tokens + qr.output_tokens,
                        input_tokens: qr.input_tokens,
                        output_tokens: qr.output_tokens,
                        tool_use_counts: count_tool_uses_in_messages(&qr.messages),
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                        paths_written: Vec::new(),
                        duration_ms,
                        worktree_path: worktree_path.clone(),
                        worktree_branch: worktree_branch.clone(),
                        output_file: None,
                        prompt: None,
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %agent_id_for_engine,
                        agent_type = %agent_type_for_engine,
                        duration_ms,
                        error = %e,
                        "subagent spawn failed"
                    );
                    AgentSpawnResponse {
                        status: AgentSpawnStatus::Failed,
                        agent_id: Some(agent_id_for_engine.clone()),
                        result: None,
                        error: Some(e.to_string()),
                        total_tool_use_count: 0,
                        total_tokens: 0,
                        duration_ms,
                        worktree_path: worktree_path.clone(),
                        worktree_branch: worktree_branch.clone(),
                        output_file: None,
                        prompt: None,
                        ..Default::default()
                    }
                }
            };

            // ── Route based on detached flag ─────────────────────
            //
            // Two paths, always typed: build an `EngineOutcome` and
            // send via the oneshot. The channel is **never dropped
            // without sending**, so the inline awaiter's `Err(RecvError)`
            // path now only fires on a true panic — not on a
            // bg-handoff-after-engine-completion race. Both completion
            // and detach paths remain observable via a typed outcome on
            // a single channel.
            let was_detached = detached_flag_for_engine.load(std::sync::atomic::Ordering::SeqCst);
            let outcome = if was_detached {
                // Inline awaiter has likely already returned AsyncLaunched
                // via `detach.notified()`. Push the `<task-notification>`
                // envelope so the model rediscovers the result through the
                // bg path (same envelope `register_background_agent_task`
                // spawns produce).
                if let Some(tid) = task_id_for_engine.as_deref() {
                    match response.status {
                        AgentSpawnStatus::Completed => {
                            let payload = coco_tool_runtime::AgentCompletionPayload {
                                result: response.result.clone(),
                                usage: Some(coco_tool_runtime::AgentUsage {
                                    usage: token_usage,
                                    tool_uses: response.total_tool_use_count as i32,
                                    duration_ms,
                                    cost_usd,
                                }),
                                worktree: response.worktree_path.clone().map(|path| {
                                    coco_tool_runtime::AgentWorktree {
                                        path: path.display().to_string(),
                                        branch: response.worktree_branch.clone(),
                                    }
                                }),
                            };
                            task_registry_for_engine.mark_completed(tid, payload).await;
                        }
                        AgentSpawnStatus::Failed => {
                            task_registry_for_engine
                                .mark_failed(
                                    tid,
                                    response.error.as_deref().unwrap_or("agent failed"),
                                )
                                .await;
                        }
                        _ => {}
                    }
                }
                EngineOutcome::CompletedAfterDetach {
                    agent_id: response
                        .agent_id
                        .clone()
                        .unwrap_or(agent_id_for_engine.clone()),
                    task_id: task_id_for_engine.clone(),
                    duration_ms,
                }
            } else {
                // Inline caller is still alive. Silent terminal — the
                // response carries the outcome inline; no notification
                // envelope (would be redundant with the tool result the
                // model sees).
                if let Some(tid) = task_id_for_engine.as_deref() {
                    task_registry_for_engine
                        .complete_silent(
                            tid,
                            matches!(response.status, AgentSpawnStatus::Completed),
                        )
                        .await;
                }
                EngineOutcome::CompletedSync(Box::new(response))
            };
            let _ = resp_tx.send(outcome);
            // Release the parent-abort linker (if any) — the engine task is
            // done, so there is nothing left to cancel.
            spawn_complete_for_engine.cancel();
        });

        // ── Inline caller: race resp_rx vs detach ────────────────
        //
        // Three observable inputs map to three branches:
        //
        // | Wire                                | Path                                    |
        // |-------------------------------------|-----------------------------------------|
        // | `Ok(EngineOutcome::CompletedSync)`  | engine ran inline → return its response |
        // | `Ok(EngineOutcome::CompletedAfterDetach)` | engine raced detach → AsyncLaunched (bg path pushed notification) |
        // | `Err(RecvError)`                    | true panic in engine task               |
        // | `detach.notified()` wins            | external Ctrl+B → AsyncLaunched         |
        //
        // The `Err` arm now only fires on a real panic — both completion
        // and detach paths stay observable.
        let task_id_for_async_response = sync_task.as_ref().map(|(id, _)| id.clone());

        if let Some(detach) = detach_handle_for_inline {
            tokio::select! {
                biased;
                res = resp_rx => Ok(resolve_engine_outcome(
                    res, &agent_id, task_id_for_async_response, start,
                )),
                () = detach.notified() => {
                    detached_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    tracing::info!(
                        target: "coco::agent_handle::sync",
                        agent_id = %agent_id,
                        "sync agent detached via signal_detach; engine continues in bg"
                    );
                    Ok(AgentSpawnResponse {
                        status: AgentSpawnStatus::AsyncLaunched,
                        agent_id: Some(task_id_for_async_response.unwrap_or(agent_id)),
                        result: None,
                        error: None,
                        total_tool_use_count: 0,
                        total_tokens: 0,
                        duration_ms: start.elapsed().as_millis() as i64,
                        worktree_path: None,
                        worktree_branch: None,
                        output_file: None,
                        prompt: None,
                        ..Default::default()
                    })
                }
            }
        } else {
            // No detach handle (internal memory forks). Just await the
            // engine task's response.
            Ok(resolve_engine_outcome(
                resp_rx.await,
                &agent_id,
                task_id_for_async_response,
                start,
            ))
        }
    }

    /// Background dispatch: spawn the engine in a detached tokio task,
    /// register with `AgentTaskRegistry`, persist transcript
    /// metadata for resume, drain text deltas into the task's output
    /// buffer, and run the periodic AgentSummary timer.
    ///
    /// Returns `AsyncLaunched` immediately. Worktree isolation is rejected
    /// at this seam so the cleanup contract isn't ambiguous.
    #[allow(clippy::too_many_arguments)]
    async fn spawn_background(
        &self,
        request: &AgentSpawnRequest,
        agent_id: String,
        agent_type: &str,
        query_config: coco_tool_runtime::AgentQueryConfig,
        worktree_session: Option<crate::worktree::AgentWorktreeSession>,
        start: Instant,
        engine: coco_tool_runtime::AgentQueryEngineRef,
        is_fork: bool,
    ) -> Result<AgentSpawnResponse, String> {
        let prompt = request.prompt.clone();
        let agent_id_for_task = agent_id.clone();

        let cancel = tokio_util::sync::CancellationToken::new();
        let task_registry = self.task_registry().clone();
        let description = request
            .description
            .clone()
            .unwrap_or_else(|| agent_type.to_string());
        // D3 / D4 (PR-1 W1): forward both the originating
        // `Agent(...)` tool_use_id and the *invoker* agent_id
        // (the agent that called AgentTool, not the new
        // subagent's id) so completion notifications route
        // correctly.
        //
        // PR 1 / W1: bg path uses `AgentRegistration::Background` so
        // the task entry's `is_backgrounded` flag initializes to
        // `true` from creation.
        let task_id = task_registry
            .register_agent_task_with_id(
                agent_id.clone(),
                &description,
                request.tool_use_id.as_deref(),
                request.invoking_agent_id.as_deref(),
                cancel.clone(),
                coco_tool_runtime::AgentRegistration::Background,
            )
            .await;

        // Per-agent metadata sidecar. Persisted at registration so
        // resume can route the rehydrated spawn to the right `agent_type`
        // and (if worktree-isolated) restore cwd_override.
        let session_id = request.session_id.clone();
        if let Some(store) = self.transcript_store() {
            let store_for_meta = store.clone();
            let session_for_meta = session_id.clone();
            let task_for_meta = task_id.clone();
            let meta = coco_tool_runtime::AgentSpawnMetadata {
                agent_type: agent_type.to_string(),
                worktree_path: worktree_session
                    .as_ref()
                    .map(|s| s.path.display().to_string()),
                description: request.description.clone(),
            };
            if let Err(e) = store_for_meta
                .write_agent_metadata(&session_for_meta, &task_for_meta, &meta)
                .await
            {
                tracing::debug!(error = %e, "agent metadata write failed");
            }
        }

        // Drain `Stream::TextDelta` events from the engine into the task's
        // output buffer so `TaskOutput` returns mid-flight text.
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<coco_types::CoreEvent>(64);
        let mut query_config = query_config;
        query_config.event_tx = Some(event_tx);
        spawn_task_event_drain(
            task_registry.clone(),
            task_id.clone(),
            agent_type.to_string(),
            event_rx,
        );

        // Periodic AgentSummary timer: only run when the spawn requested it
        // via `enable_summarization`. Default-off keeps a saturated coordinator
        // (16 spawns × 30 s = 32 LLM calls/min) off the user's hot path
        // unless they explicitly opted in. The reader half of the engine's
        // snapshot sink rides on `query_config` (`Some` iff
        // `enable_summarization`).
        match query_config.live_transcript.clone() {
            Some(live) => spawn_agent_summary_timer(AgentSummaryTimer {
                registry: task_registry.clone(),
                task_id: task_id.clone(),
                cancel: cancel.clone(),
                agent_type: agent_type.to_string(),
                engine: engine.clone(),
                definition: request.definition.clone(),
                model_selection: query_config.model_selection.clone(),
                session_id: query_config.identity.session_id.clone(),
                live_transcript: live,
            }),
            None => tracing::debug!(
                %agent_id,
                "periodic AgentSummary disabled (request.enable_summarization = false)"
            ),
        }

        let registry_for_task = task_registry.clone();
        let task_id_for_task = task_id.clone();
        let cancel_for_task = cancel.clone();
        let transcript_store_for_task = self.transcript_store().cloned();
        let session_id_for_task = session_id.clone();
        let hook_registry_for_task = self.hook_registry().cloned();
        let cwd_for_task = self.cwd.clone();
        let agent_type_for_task = agent_type.to_string();
        // Bg-path handoff classifier: runs for background spawns too (auto
        // mode only). Clone the side-query handle + permission mode so the
        // detached task can gate and run it after completion.
        let side_query_for_task = self.side_query().cloned();
        let mode_for_task = request.mode;
        // Preload frontmatter skills synchronously here — bg task can't
        // borrow `&self`, so resolve bodies upfront and prepend
        // before handing the prompt to the detached task.
        let prompt_after_skills = self
            .preload_frontmatter_skills(request.definition.as_deref(), &prompt)
            .await;
        let prompt = prompt_after_skills;
        // Register frontmatter hooks BEFORE the detached task starts
        // so they're visible during execute_query. Tracked here so the
        // task can clear them at SubagentStop time without re-borrowing
        // `&self`.
        let registered_frontmatter_hooks =
            self.register_frontmatter_hooks(&agent_id, request.definition.as_deref());
        // Initialise per-agent MCP servers synchronously here. Cleanup
        // happens after the spawn completes — we capture the dynamic
        // server map + handle into the task so it can run cleanup
        // without re-borrowing `&self`.
        self.initialize_per_agent_mcp(&agent_id, request.definition.as_deref())
            .await;
        let mcp_handle_for_task = self.mcp_handle().cloned();
        let dynamic_mcp_servers_for_task = self.dynamic_mcp_servers().clone();
        let worktree_manager_for_task = self.worktree_manager().cloned();
        let worktree_session_for_task = worktree_session.clone();
        let bg_start = std::time::Instant::now();
        tokio::spawn(async move {
            // Fire SubagentStart hooks before kicking off execution and
            // prepend any returned context blocks to the prompt. Bg path
            // mirrors the sync path's behaviour.
            let decorated_prompt = fire_subagent_start_for_task(
                hook_registry_for_task.clone(),
                &cwd_for_task,
                &agent_id_for_task,
                &agent_type_for_task,
                &prompt,
            )
            .await;

            // Fork mode: wrap the decorated directive in the
            // `<fork-boilerplate>...</fork-boilerplate>` envelope. See
            // sync-path comment above for the rationale (recursion
            // guard + worker rules).
            let effective_prompt = if is_fork {
                coco_subagent::build_fork_child_message(&decorated_prompt)
            } else {
                decorated_prompt
            };

            // Race the engine query against the cancellation token so
            // `kill_task` propagates. The engine itself honours its config
            // `cancel`; the extra select here covers a cancel mid-engine.
            let outcome = tokio::select! {
                _ = cancel_for_task.cancelled() => {
                    Err(Box::new(coco_error::PlainError::new(
                        "task cancelled by leader",
                        coco_error::StatusCode::Cancelled,
                    )) as coco_error::BoxedError)
                }
                r = engine.execute_query(&effective_prompt, query_config) => r,
            };

            // Fire SubagentStop AFTER execution completes (success,
            // failure, or cancel). Transcript path is populated when
            // the per-agent JSONL store is wired and a session-id is
            // available.
            let transcript_path = transcript_store_for_task
                .as_ref()
                .filter(|_| !session_id_for_task.is_empty())
                .map(|_| format!("agent-{task_id_for_task}.jsonl"));
            fire_subagent_stop_for_task(
                hook_registry_for_task.clone(),
                &cwd_for_task,
                &agent_id_for_task,
                &agent_type_for_task,
                transcript_path.as_deref(),
            )
            .await;

            // Cleanup per-agent hook bucket. Mirrors sync path's
            // `clear_frontmatter_hooks(&agent_id)`.
            if registered_frontmatter_hooks && let Some(reg) = hook_registry_for_task.as_ref() {
                reg.clear_agent_scope(&agent_id_for_task);
            }

            // Tear down dynamically-added MCP servers. Pull names from
            // the shared map (same write lock the sync path uses) and
            // call `remove_dynamic_server` per entry.
            let dynamic_names = dynamic_mcp_servers_for_task
                .write()
                .await
                .remove(&agent_id_for_task);
            if let (Some(names), Some(handle)) = (dynamic_names, mcp_handle_for_task.as_ref()) {
                for name in names {
                    if let Err(e) = handle.remove_dynamic_server(&name).await {
                        tracing::debug!(
                            error = %e,
                            agent_id = %agent_id_for_task,
                            server = %name,
                            "bg cleanup: failed to remove dynamic agent MCP server"
                        );
                    }
                }
            }
            // Clone the session so the borrow at notification-build
            // time (below) still has access. `cleanup_if_unchanged`
            // takes ownership of one copy; the original survives for
            // the worktree info on the `<task-notification>` envelope.
            if let (Some(manager), Some(session)) = (
                worktree_manager_for_task.as_ref(),
                worktree_session_for_task.clone(),
            ) {
                let session_path = session.path.display().to_string();
                if matches!(
                    manager.cleanup_if_unchanged(session),
                    crate::worktree::WorktreeCleanupOutcome::Removed
                ) {
                    fire_worktree_remove_hook(
                        hook_registry_for_task.clone(),
                        &cwd_for_task,
                        &session_path,
                    )
                    .await;
                }
            }
            // Persist the full message history to the per-agent JSONL
            // transcript on success. `agent/resume` reads this back via
            // `AgentTranscriptStore::load_agent_messages` and threads the
            // entries into the resumed spawn's `fork_context_messages`.
            if let (Some(store), Ok(qr)) = (transcript_store_for_task.as_ref(), outcome.as_ref())
                && !session_id_for_task.is_empty()
                && !qr.messages.is_empty()
                && let Err(e) = store
                    .append_agent_messages(&session_id_for_task, &task_id_for_task, &qr.messages)
                    .await
            {
                tracing::debug!(error = %e, "agent transcript write failed");
            }
            match outcome {
                Ok(qr) => {
                    // The completion notification carries the final assistant
                    // text, usage stats, and worktree info so the model
                    // sees a rich `<result>` / `<usage>` / `<worktree>`
                    // envelope on the next turn.
                    let duration_ms = bg_start.elapsed().as_millis() as i64;
                    // Run the handoff classifier on the background result too
                    // (auto mode only). It returns the (possibly
                    // safety-prefixed) response text; fall back to the last
                    // assistant message when the classifier passes through
                    // with no `response_text`.
                    let result = super::handoff::classify_handoff_inline(
                        &agent_type_for_task,
                        &qr,
                        side_query_for_task.as_ref(),
                        mode_for_task,
                    )
                    .await
                    .or_else(|| last_assistant_text(&qr.messages));
                    let usage = Some(coco_tool_runtime::AgentUsage {
                        usage: qr.usage,
                        tool_uses: qr.tool_use_count as i32,
                        duration_ms,
                        cost_usd: qr.cost_usd,
                    });
                    let worktree = worktree_session_for_task.as_ref().map(|s| {
                        coco_tool_runtime::AgentWorktree {
                            path: s.path.display().to_string(),
                            branch: Some(s.branch.clone()),
                        }
                    });
                    registry_for_task
                        .mark_completed(
                            &task_id_for_task,
                            coco_tool_runtime::AgentCompletionPayload {
                                result,
                                usage,
                                worktree,
                            },
                        )
                        .await;
                }
                Err(e) => {
                    registry_for_task
                        .mark_failed(&task_id_for_task, &e.to_string())
                        .await;
                }
            }
        });

        let output_file = task_registry.output_file_path(&task_id).await;

        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::AsyncLaunched,
            // Return the registry's `task_id` so the model can address the
            // spawn via TaskGet / TaskOutput / TaskStop.
            agent_id: Some(task_id),
            result: None,
            error: None,
            total_tool_use_count: 0,
            total_tokens: 0,
            duration_ms: start.elapsed().as_millis() as i64,
            worktree_path: None,
            worktree_branch: None,
            output_file,
            prompt: None,
            ..Default::default()
        })
    }
}

/// Walk the child agent's message log and tally `tool_name` from every
/// assistant tool-call block. Memory telemetry uses the
/// `Write + Edit + NotebookEdit` count to populate
/// `MemoryEvent::ExtractionCompleted::files_written` without re-running
/// the LLM.
fn count_tool_uses_in_messages(
    messages: &[std::sync::Arc<coco_messages::Message>],
) -> std::collections::HashMap<String, i64> {
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for arc in messages {
        let coco_messages::Message::Assistant(a) = arc.as_ref() else {
            continue;
        };
        let coco_messages::LlmMessage::Assistant { content, .. } = &a.message else {
            continue;
        };
        for part in content {
            if let coco_messages::AssistantContent::ToolCall(tc) = part {
                *counts.entry(tc.tool_name.clone()).or_insert(0) += 1;
            }
        }
    }
    counts
}

/// Concatenate every assistant text part in the most recent
/// assistant message. Used for the `<result>` section of the
/// background-agent completion notification. Returns `None` when the
/// log has no assistant message or the last one is text-empty
/// (tool-only turn).
fn last_assistant_text(messages: &[std::sync::Arc<coco_messages::Message>]) -> Option<String> {
    for arc in messages.iter().rev() {
        let coco_messages::Message::Assistant(a) = arc.as_ref() else {
            continue;
        };
        let coco_messages::LlmMessage::Assistant { content, .. } = &a.message else {
            continue;
        };
        let mut chunks: Vec<String> = Vec::new();
        for part in content {
            if let coco_messages::AssistantContent::Text(t) = part
                && !t.text.is_empty()
            {
                chunks.push(t.text.clone());
            }
        }
        return if chunks.is_empty() {
            None
        } else {
            Some(chunks.join("\n"))
        };
    }
    None
}
