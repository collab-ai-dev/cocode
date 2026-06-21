//! Bootstrap-time assembly of the production [`SwarmAgentHandle`].
//!
//! The handle owns the in-process runner + team manager + worktree
//! manager and is the single seam through which `AgentTool` /
//! `SendMessage` / `TeamCreate` / `TeamDelete` reach the coordinator.
//! Today the only other implementation is `NoOpAgentHandle` (returns
//! "not available in this context" for every call), used only before
//! bootstrap late-binding or in narrow test fixtures.
//!
//! The factory closure inside [`coco_query::QueryEngineAdapter`] is
//! the trickiest piece — it needs to spawn a fresh `QueryEngine` per
//! subagent call, route the call's model selection to the right
//! `ModelRuntimeSource`, and thread it back through `SessionRuntime`'s
//! standard wiring (compaction observers, mailbox, hooks, etc.). This
//! module owns the closure construction so `app/cli/main.rs` doesn't
//! grow a 50-line lambda.
//!
//! Multi-provider role resolution is a Rust-only feature so the wiring
//! lives here.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use coco_coordinator::agent_handle::SwarmAgentHandle;
use coco_coordinator::runner::InProcessAgentRunner;
use coco_coordinator::types::TeamManager;
use coco_coordinator::worktree::AgentWorktreeManager;
use coco_query::agent_adapter::{QueryEngineAdapter, QueryEngineFactory};
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentQueryEngineRef;
use coco_tool_runtime::SkillHandleRef;
use coco_types::LlmModelSelection;
use coco_types::ModelRole;
use tokio::sync::RwLock;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::session_runtime::SessionRuntime;

/// Stale agent-worktree GC threshold — crash-leaked `agent-*` worktrees
/// older than this are swept at session start (30-day retention).
const STALE_WORKTREE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Per-runtime handles required to construct the production
/// `SwarmAgentHandle`. Splitting this out of `SessionRuntime::build`
/// avoids growing the build-options surface for an opt-in feature.
pub struct AgentTeamWiring {
    pub agent_handle: AgentHandleRef,
    /// Skill-execution handle backed by the same subagent
    /// `AgentQueryEngineRef` the swarm handle uses. Installed on the
    /// runtime (`attach_skill_handle`) so `wire_engine` reaches every
    /// per-turn engine, and on the coordinator handle (for frontmatter
    /// `skills:` preload).
    pub skill_handle: SkillHandleRef,
}

fn resolve_agent_runtime_source(
    selection: &LlmModelSelection,
) -> coco_inference::ModelRuntimeSource {
    match selection {
        LlmModelSelection::InheritMain
        | LlmModelSelection::Role {
            role: ModelRole::Main,
        } => coco_inference::ModelRuntimeSource::Role(ModelRole::Main),
        LlmModelSelection::Role { role } => coco_inference::ModelRuntimeSource::Role(*role),
        LlmModelSelection::Explicit { primary }
        | LlmModelSelection::ExplicitWithFallbackRole { primary, .. } => {
            coco_inference::ModelRuntimeSource::Explicit(primary.clone())
        }
    }
}

/// Assemble the production [`SwarmAgentHandle`] for `runtime`.
///
/// Requires the TaskRuntime-backed registry to already be attached;
/// absence is not a supported production LocalAgent configuration.
///
/// **Late-bind contract**: this fn must run *after*
/// `SessionRuntime::build` has returned the `Arc`, because the
/// `QueryEngineAdapter` factory captures `Arc<SessionRuntime>` to
/// drive per-spawn engine assembly. Calling it from inside `build()`
/// would force a cycle (`Arc::new_cyclic` works, but the resulting
/// closure-vs-Arc dance is far less readable than two-phase init).
pub async fn build_agent_team_wiring(
    runtime: Arc<SessionRuntime>,
    cwd: String,
) -> Result<AgentTeamWiring> {
    // In-process subagents inherit the leader's `ToolPermissionBridge`
    // (installed on `SessionRuntime` and propagated by `wire_engine`):
    // SDK leaders forward over `approval/askForApproval`, TUI leaders
    // drive the permission prompt. No extra channel needed
    // here — the prior in-process mpsc circuit was orphaned and
    // removed in the post-D cleanup pass.
    let runner = Arc::new(InProcessAgentRunner::new(
        cwd.clone(),
        runtime.runtime_config.agent_teams.max_agents,
    ));
    let team_manager = Arc::new(RwLock::new(None::<TeamManager>));

    let task_rt = runtime
        .current_task_runtime()
        .await
        .context("TaskRuntime must be attached before AgentTeam wiring")?;
    let mut handle = SwarmAgentHandle::new(
        runner.clone(),
        team_manager,
        cwd.clone(),
        runtime.runtime_config.clone(),
        task_rt as coco_tool_runtime::AgentTaskRegistryRef,
    );
    let backend_registry = Arc::new(coco_coordinator::pane::BackendRegistry::new());
    backend_registry
        .register_in_process_backend(Arc::new(coco_coordinator::InProcessBackend::new(
            runner.clone(),
        )))
        .await;
    let detection = backend_registry.detect_backend().await;
    match detection.backend_type {
        coco_coordinator::BackendType::Tmux => {
            backend_registry
                .register_pane_backend(Arc::new(coco_coordinator::pane::tmux::TmuxBackend::new(
                    detection.is_native,
                )))
                .await;
        }
        coco_coordinator::BackendType::Iterm2 => {
            backend_registry
                .register_pane_backend(
                    Arc::new(coco_coordinator::pane::iterm2::ITermBackend::new()),
                )
                .await;
        }
        coco_coordinator::BackendType::InProcess => {
            backend_registry.mark_in_process_fallback().await;
        }
        _ => {
            backend_registry.mark_in_process_fallback().await;
        }
    }
    handle.set_backend_registry(backend_registry);

    // Install the agent worktree manager so `AgentTool` spawns with
    // `isolation: "worktree"` get a real git worktree under the main repo's
    // `.coco/worktrees/agent-<slug>` (cwd_override + cleanup-on-success).
    // Without this install, `worktree_manager()` stays `None` and every such
    // spawn fails fast with "no AgentWorktreeManager is configured".
    //
    // Agent worktree creation is ungated — it only requires being inside a
    // git repo. We install whenever discovery succeeds and skip silently
    // otherwise — a later isolation request then surfaces the existing clear
    // error. (`Feature::Worktree` gates the separate interactive
    // EnterWorktree/ExitWorktree tools, not subagent isolation, so it is
    // deliberately not consulted here.)
    match AgentWorktreeManager::discover_from_cwd(Path::new(&cwd)) {
        Ok(manager) => {
            let manager = Arc::new(manager);
            handle.set_worktree_manager(manager.clone());

            // Reap crash-leaked `agent-*` worktrees (parent killed before
            // `cleanup_if_unchanged` ran). Fire-and-forget, mirroring the
            // session-memory / shell-snapshot sweeps in `session_runtime`.
            // `cleanup_stale` runs synchronous git subprocesses, so it goes
            // through `spawn_blocking` to avoid stalling a runtime worker.
            // Bare mode skips the sweep (lives inside bare-mode housekeeping,
            // not the create path).
            if !coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode) {
                tokio::spawn(async move {
                    let removed = tokio::task::spawn_blocking(move || {
                        manager.cleanup_stale(STALE_WORKTREE_TTL)
                    })
                    .await
                    .unwrap_or(0);
                    if removed > 0 {
                        info!("reaped {removed} stale agent worktree(s)");
                    }
                });
            }
        }
        Err(e) => {
            debug!(error = %e, "agent worktree isolation unavailable (cwd not in a git repo)");
        }
    }

    if let Some(task_list) = runtime.current_task_list().await {
        handle.set_task_list(task_list);
    }
    if let Some(router) = runtime.current_team_task_list_router().await {
        handle.set_team_task_list_router(router);
    }

    // Per-agent transcript store for `SwarmAgentHandle::resume_agent`.
    // The same Arc is shared across spawns so concurrent bg agents read/write
    // through one store; absent only in minimal test embeddings.
    if let Some(transcript_store) = runtime.current_agent_transcript_store().await {
        handle.set_transcript_store(transcript_store);
    }

    // Wire the per-spawn engine factory. Captures `Arc<SessionRuntime>`
    // and resolves a fresh role-aware engine on each call. The
    // factory is async (`QueryEngineFactory` returns a boxed future)
    // so it can call `build_engine_from_config` + `client_for_role`
    // directly without blocking the runtime.
    let factory: QueryEngineFactory = {
        let runtime_for_factory = runtime.clone();
        Arc::new(move |mut engine_config, role, cancel| {
            let runtime = runtime_for_factory.clone();
            Box::pin(async move {
                // ── Gap A fix — inherit parent's resolved RuntimeConfig ──
                //
                // Before this fix, `agent_adapter::execute_query` populated
                // 8 policy fields with `*::default()` and the engine then
                // read its own `self.config.*` for those settings — so
                // subagent compact / sandbox / web / shell / memory /
                // plan-mode / system-reminder / tool config silently
                // ignored the user's `~/.coco/settings.json`.
                //
                // Subagents inherit the parent's config tree.
                // Overwrite those fields here from the live RuntimeConfig
                // so subagent engines compact at the user's tuned
                // thresholds, honour the user's sandbox mode, etc.
                engine_config.compact = runtime.runtime_config.compact.clone();
                engine_config.system_reminder = runtime
                    .runtime_config
                    .settings
                    .merged
                    .system_reminder
                    .clone();
                engine_config.tool_config = runtime.runtime_config.tool.clone();
                engine_config.sandbox_config = runtime.runtime_config.sandbox.clone();
                engine_config.memory_config = runtime.runtime_config.memory.clone();
                engine_config.shell_config = runtime.runtime_config.shell.clone();
                engine_config.web_fetch_config = runtime.runtime_config.web_fetch.clone();
                engine_config.web_search_config = runtime.runtime_config.web_search.clone();
                engine_config.plan_mode_settings =
                    runtime.runtime_config.settings.merged.plan_mode.clone();
                if engine_config.wire_dump.is_none()
                    && let Some(parent_wire_dump) =
                        runtime.current_engine_config().await.wire_dump.as_ref()
                    && let Some(agent_id) = engine_config.agent_id.as_deref()
                {
                    engine_config.wire_dump = parent_wire_dump.for_subagent(agent_id);
                }

                let runtime_source = resolve_agent_runtime_source(&role);
                // Build a fresh engine via the runtime's standard
                // path, then select the role/explicit runtime source.
                // `wire_engine` (called inside `build_engine_from_config`)
                // installs all the same observers / mailbox / hooks
                // the top-level engine gets so subagent execution
                // stays observable.
                let engine = runtime
                    .build_engine_from_config(
                        engine_config,
                        cancel.unwrap_or_else(tokio_util::sync::CancellationToken::new),
                        None,
                    )
                    .await;
                // Per-engine `live_command_rules` is fresh for every
                // forked subagent (constructed inside
                // `QueryEngine::new`), so the subagent's skill rules
                // cannot leak to the parent — different Arcs.
                engine.with_model_runtime_source(runtime_source)
            })
        })
    };
    let adapter: AgentQueryEngineRef = Arc::new(QueryEngineAdapter::new(factory));
    handle.set_execution_engine(adapter.clone());

    // ── Skill runtime ──
    //
    // Build the real `SkillHandle` over the same subagent engine adapter.
    // Fork-mode skills route their expanded prompt through `adapter`
    // (one subagent execution path); inline skills expand in-process.
    // Without this install the engine carries `NoOpSkillHandle` and every
    // model `SkillTool` call fails with "no skill runtime installed".
    let skill_handle: SkillHandleRef = Arc::new(
        coco_query::skill_runtime::QuerySkillRuntime::new(runtime.skill_manager())
            .with_agent_engine(adapter.clone())
            .with_session_id(runtime.current_session_id().await)
            // Share the per-turn Bash handle so in-prompt `` !`cmd` ``
            // markers in model-invoked / fork-mode skills run through the
            // same permission-checked route as slash-command handlers.
            .with_bash_handle_cell(runtime.skill_bash_cell()),
    );
    // Coordinator side: lets `preload_frontmatter_skills` resolve agent
    // `skills: [..]` frontmatter via `read_skill_body`.
    handle.set_skill_handle(skill_handle.clone());

    // ── Gap C fix — install teammate execution engine ──
    //
    // The same QueryEngineAdapter that drives subagent spawns also
    // drives in-process teammates via the coordinator's runner-loop,
    // bridged through `TeammateExecutionAdapter`. Without this,
    // `spawn_teammate` registered teammates that never executed.
    //
    // Auto-compact threshold flows from the user's CompactConfig so
    // teammates compact at the same tuning as the leader. Default
    // 100k matches the threshold-formula floor when CompactConfig is
    // unset.
    handle.set_teammate_execution_engine(coco_coordinator::agent_handle::into_execution_engine(
        adapter,
    ));
    let auto_compact_threshold = coco_compact::auto_compact_threshold(
        /*context_window*/ 200_000,
        /*max_output_tokens*/ 16_384,
        &runtime.runtime_config.compact.auto,
    );
    handle.set_teammate_auto_compact_threshold(auto_compact_threshold);

    // Pass the leader's full system prompt as the teammate base. The
    // runner-loop composes this with `TEAMMATE_PROMPT_ADDENDUM` so
    // teammates inherit the same CLAUDE.md + env-context + memory
    // blocks the leader uses.
    if let Some(base_prompt) = runtime.current_engine_config().await.system_prompt.clone() {
        handle.set_teammate_base_system_prompt(base_prompt).await;
    }

    // Wire the hook registry so SubagentStart / SubagentStop hooks fire
    // around subagent execution.
    handle.set_hook_registry(runtime.hook_registry.clone());

    // Wire the MCP handle so per-agent inline `mcpServers: [{name: config}]`
    // entries get registered as dynamic servers at spawn and torn down
    // at SubagentStop. String-ref entries don't need this wire — they
    // reuse the parent's pre-existing connection.
    if let Some(mcp) = runtime.current_mcp_handle().await {
        handle.set_mcp_handle(mcp.clone());
        install_coco_guide_context_builder(&mut handle, runtime.clone(), mcp).await;
    }

    // Auto-sync per-agent project snapshots into local memory dirs at
    // bootstrap. `prompt-update` is treated as an automatic re-sync
    // here — there is no interactive prompt at bootstrap and forcing
    // manual approval would leave newer team baselines silently
    // unconsumed.
    sync_agent_memory_snapshots(&runtime, &cwd).await;

    Ok(AgentTeamWiring {
        agent_handle: Arc::new(handle),
        skill_handle,
    })
}

/// Install the coco-guide dynamic-context builder onto the swarm
/// handle. Reads runtime context — commands, active agents, MCP
/// clients, resolved settings — and emits the dynamic block when at
/// least one source is non-empty.
///
/// The closure captures `Arc`-shared handles that resolve the snapshot
/// at spawn time so a re-loaded command registry or settings change
/// before the next spawn picks up the latest state. Failure to install
/// (no MCP wired) leaves the static prompt.
async fn install_coco_guide_context_builder(
    handle: &mut coco_coordinator::agent_handle::SwarmAgentHandle,
    runtime: Arc<SessionRuntime>,
    mcp_handle: coco_tool_runtime::McpHandleRef,
) {
    use std::sync::Arc as StdArc;

    let runtime_for_builder = runtime;
    let mcp_for_builder = mcp_handle;
    let builder: coco_coordinator::agent_handle::CocoGuideContextBuilder = StdArc::new(move || {
        // The closure must be sync to fit the trait, but the
        // accessors on SessionRuntime / McpHandle are async.
        // Use `block_in_place` + the current Tokio handle to bridge.
        // Spawn time is rare (one coco-guide spawn per user
        // request), so the block-in-place cost is negligible.
        let runtime_inner = runtime_for_builder.clone();
        let mcp_inner = mcp_for_builder.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let cmd_reg = runtime_inner.current_command_registry().await;
                // Slash commands: split prompt-type into custom (non-plugin)
                // and plugin-sourced.
                let mut custom_commands: Vec<coco_subagent::GuideCommandEntry> = Vec::new();
                let mut plugin_commands: Vec<coco_subagent::GuideCommandEntry> = Vec::new();
                for cmd in cmd_reg.all() {
                    if !matches!(cmd.command_type, coco_types::CommandType::Prompt(_)) {
                        continue;
                    }
                    let entry = coco_subagent::GuideCommandEntry {
                        name: cmd.base.name.clone(),
                        description: cmd.base.description.clone(),
                    };
                    if matches!(
                        cmd.base.loaded_from,
                        Some(coco_types::CommandSource::Plugin { .. })
                    ) {
                        plugin_commands.push(entry);
                    } else {
                        custom_commands.push(entry);
                    }
                }

                // Active non-built-in agents.
                let catalog = runtime_inner.current_agent_catalog().await;
                let custom_agents: Vec<coco_subagent::GuideAgentEntry> = catalog
                    .active()
                    .filter(|def| {
                        def.source.as_str() != coco_types::CommandSource::Builtin.as_str()
                    })
                    .map(|def| coco_subagent::GuideAgentEntry {
                        agent_type: def.agent_type.to_string(),
                        when_to_use: def.when_to_use.clone().unwrap_or_default(),
                    })
                    .collect();

                let mcp_servers = mcp_inner.connected_servers().await;

                // Settings: pretty-print the resolved Settings via serde.
                // Empty string when serialisation fails (rare; serde never
                // panics on the well-typed Settings struct).
                let settings_json =
                    serde_json::to_string_pretty(&runtime_inner.runtime_config.settings.merged)
                        .unwrap_or_default();

                coco_subagent::CocoGuideDynamicContext {
                    custom_commands,
                    plugin_commands,
                    custom_agents,
                    mcp_servers,
                    settings_json,
                }
            })
        })
    });
    handle.set_coco_guide_context_builder(builder);
}

/// Walk every known agent type's snapshot dir under
/// `<cwd>/.coco/agent-memory-snapshots/` and apply the appropriate
/// `SnapshotAction` for each (User / Project / Local) scope. Errors
/// are logged + swallowed — a snapshot sync failure must not gate
/// session startup.
async fn sync_agent_memory_snapshots(_runtime: &Arc<SessionRuntime>, cwd: &str) {
    let cwd_path = std::path::PathBuf::from(cwd);
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let snapshots_root = cwd_path
        .join(".coco")
        .join(coco_memory::agent_memory_snapshot::SNAPSHOT_BASE);

    let entries = match std::fs::read_dir(&snapshots_root) {
        Ok(e) => e,
        Err(_) => return, // No snapshot tree at all — nothing to do.
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let agent_type = entry.file_name().to_string_lossy().to_string();
        for scope in [
            coco_types::MemoryScope::User,
            coco_types::MemoryScope::Project,
            coco_types::MemoryScope::Local,
        ] {
            let action = coco_memory::agent_memory_snapshot::check_agent_memory_snapshot(
                &agent_type,
                scope,
                &cwd_path,
                &home,
            );
            match action {
                coco_memory::agent_memory_snapshot::SnapshotAction::Initialize {
                    snapshot_timestamp,
                } => {
                    if let Err(e) = coco_memory::agent_memory_snapshot::initialize_from_snapshot(
                        &agent_type,
                        scope,
                        &snapshot_timestamp,
                        &cwd_path,
                        &home,
                    ) {
                        warn!(error = %e, %agent_type, ?scope, "snapshot initialize failed");
                    }
                }
                coco_memory::agent_memory_snapshot::SnapshotAction::PromptUpdate {
                    snapshot_timestamp,
                } => {
                    if let Err(e) = coco_memory::agent_memory_snapshot::replace_from_snapshot(
                        &agent_type,
                        scope,
                        &snapshot_timestamp,
                        &cwd_path,
                        &home,
                    ) {
                        warn!(error = %e, %agent_type, ?scope, "snapshot replace failed");
                    }
                }
                coco_memory::agent_memory_snapshot::SnapshotAction::None => {}
            }
        }
    }
}

/// Convenience for one-shot bootstrap: build the wiring and attach it
/// to the runtime.
pub async fn install_agent_team(runtime: Arc<SessionRuntime>, cwd: String) -> anyhow::Result<()> {
    let wiring = build_agent_team_wiring(runtime.clone(), cwd).await?;
    runtime.attach_agent_handle(wiring.agent_handle).await;
    runtime.attach_skill_handle(wiring.skill_handle).await;
    Ok(())
}
