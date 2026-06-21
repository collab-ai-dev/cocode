//! In-process driver for the agent loop.
//!
//! Builds a `QueryEngine` mirroring what `coco -p "<prompt>"` constructs
//! (`run_chat` in `app/cli/src/main.rs`) but without a binary process —
//! tests get direct access to the `QueryResult` and the structured
//! `CoreEvent` stream.
//!
//! Hermetic by design: each `LiveSession` owns a tempdir used as `cwd`
//! and `project_dir` so `bypassPermissions` writes can't escape.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use coco_hooks::HookRegistry;
use coco_inference::ModelRuntimeSource;
use coco_query::CommandQueue;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::QueryResult;
use coco_tool_runtime::ToolRegistry;
use coco_types::CoreEvent;
use coco_types::Features;
use coco_types::PermissionMode;
use coco_types::ToolOverrides;

/// Register a curated tool subset for live agent runs.
///
/// `coco_tools::register_all_tools` ships ~42 builtins; several
/// (`AskUserQuestion`, `Grep`, `TodoWrite`, `CronList`,
/// `ListMcpResources`, …) currently emit JSON schemas with
/// `type: null` for empty argument blocks, which DeepSeek's API
/// correctly rejects with HTTP 400. To keep the live tests focused
/// on agent-loop behavior — not on validating every tool's schema —
/// we register only the file/shell tools the scenarios actually
/// need. Upstream schema cleanup is tracked separately.
fn register_suite_tools(registry: &ToolRegistry) {
    registry.register(Arc::new(coco_tools::BashTool));
    registry.register(Arc::new(coco_tools::ReadTool));
    registry.register(Arc::new(coco_tools::WriteTool));
    registry.register(Arc::new(coco_tools::EditTool));
    registry.register(Arc::new(coco_tools::GlobTool));
    // `SkillTool` is statically typed (`SkillInput` has a required
    // `skill` field), so its schema is a well-formed `type: object` —
    // not the empty `type: null` shape DeepSeek rejects. It must be
    // loaded for the `skill_listing` reminder to fire: the reminder is
    // gated behind `reminder_skill_tool_loaded` (engine_turn_reminders.rs).
    registry.register(Arc::new(coco_tools::SkillTool));
}
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::common::runtime::build_client;

/// Knobs the CLI suite tweaks per scenario. Defaults aim at "tiny but
/// realistic" so live tests stay fast and cheap.
#[derive(Clone)]
pub struct SessionConfig {
    /// Triggers compaction earlier when set small. Default 200_000
    /// (matches the production `run_chat` default).
    pub context_window: i64,
    /// Per-call output cap. Default 2_048 keeps each turn cheap.
    pub max_output_tokens: i64,
    /// Hard upper bound on agent loop turns (`None` = unbounded).
    pub max_turns: Option<i32>,
    /// Session-level total token budget (input + output, accumulated
    /// across every API call). `None` lets the engine run unbounded.
    pub total_token_budget: Option<i64>,
    /// Capacity of the `CoreEvent` channel. Bigger window = less
    /// back-pressure on the engine; tests assert post-hoc so 1024 is plenty.
    pub event_buffer: usize,
    /// Optional system-prompt override — `None` lets the engine use
    /// its own composed system prompt.
    pub system_prompt: Option<String>,
    /// Engine-side permission mode. Reminder tests flip this to `Plan`
    /// or `Auto` to trigger `PlanMode` / `AutoMode` reminders.
    pub permission_mode: PermissionMode,
    /// `system_reminder` engine config — controls which reminder
    /// generators are enabled and supplies `critical_instruction`.
    /// Defaults to the prod default (most reminders on).
    pub system_reminder: coco_config::SystemReminderConfig,
    /// Budget cap; when `Some`, fires the `BudgetUsd` reminder every
    /// turn. `None` disables the reminder.
    pub max_budget_usd: Option<f64>,
    /// Pre-built `SessionBootstrap`. Reminders that source from
    /// bootstrap (`OutputStyle`, `SkillListing` listing-only path)
    /// require this. `None` means the engine runs without a bootstrap
    /// — fine for most tests.
    pub session_bootstrap: Option<coco_query::SessionBootstrap>,
    /// Files to drop into the workdir before the engine starts. Each
    /// entry is `(relative_path, content)`. Parent dirs are created.
    /// Used by tests that need pre-populated `CLAUDE.md`,
    /// `.coco/skills/<name>/SKILL.md`, or custom slash commands.
    pub pre_workdir_files: Vec<(PathBuf, String)>,
    /// Hook registry to install on the engine. `None` means no hooks
    /// (the production default for `coco -p`). Tests targeting
    /// PreToolUse / PostToolUse / Stop hook behavior pass `Some`.
    pub hooks: Option<Arc<HookRegistry>>,
    /// Override the `Features` flag set. `None` keeps
    /// `Features::with_defaults()`. Tests that assert on feature-gated
    /// tool registry shape (e.g. WebSearch off → tool absent) supply
    /// a custom set.
    pub features: Option<Features>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            context_window: 200_000,
            max_output_tokens: 2_048,
            max_turns: Some(8),
            total_token_budget: None,
            event_buffer: 1024,
            system_prompt: None,
            permission_mode: PermissionMode::BypassPermissions,
            system_reminder: coco_config::SystemReminderConfig::default(),
            max_budget_usd: None,
            session_bootstrap: None,
            pre_workdir_files: Vec::new(),
            hooks: None,
            features: None,
        }
    }
}

impl SessionConfig {
    /// Smaller window, fewer output tokens — used by the compaction
    /// scenario to force `ContextCompacted` to fire.
    pub fn small_window(window_tokens: i64) -> Self {
        Self {
            context_window: window_tokens,
            max_output_tokens: 1_024,
            max_turns: Some(12),
            ..Self::default()
        }
    }
}

/// Outcome of a single in-process agent run. Exposes the same surfaces a
/// black-box CLI test would inspect: final stdout (`response_text`),
/// turn count, accumulated usage, and the structured event stream.
pub struct SessionOutcome {
    pub result: QueryResult,
    pub events: Vec<CoreEvent>,
    /// Workdir path so tests can read files the agent wrote without
    /// peeking past the `_workdir` `TempDir`. Cloned from
    /// `_workdir.path()` at construction time. Allow-dead-code so
    /// future tests reading agent file output don't trip the lint;
    /// the field is part of the public test surface.
    #[allow(dead_code)]
    pub workdir_path: PathBuf,
    /// Tempdir that backed `cwd`. Owned by the outcome so files the
    /// agent wrote stay readable for follow-up assertions; cleaned up
    /// when the outcome drops. Underscore-prefixed because tests
    /// generally read the agent's reply rather than the filesystem,
    /// but the lifetime tie is load-bearing.
    pub _workdir: TempDir,
}

/// Drive `QueryEngine::run_with_events` end-to-end. Spawns a task to
/// drain events into a `Vec` so the engine never blocks on the channel.
pub async fn run_session(
    provider_name: &str,
    model_id: &str,
    session_cfg: SessionConfig,
    prompt: &str,
) -> Result<SessionOutcome> {
    // Anchor the agent's cwd at `/tmp/coco-tests-cli-<rand>` so any
    // file the agent writes is visibly under /tmp (not under the
    // project directory or macOS's opaque `/var/folders/...`).
    let workdir = crate::common::tmpdir::make("coco-tests-cli-")
        .with_context(|| "create cwd tempdir under /tmp")?;
    let workdir_path = workdir.path().to_path_buf();

    // Pre-populate workdir with caller-supplied fixtures (CLAUDE.md,
    // SKILL.md, slash command markdown, …) before the engine starts so
    // its initial context-assembly + skill discovery picks them up.
    for (rel_path, content) in &session_cfg.pre_workdir_files {
        let target = workdir_path.join(rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dirs for {target:?}"))?;
        }
        std::fs::write(&target, content)
            .with_context(|| format!("write pre-workdir file {target:?}"))?;
    }

    let api_client = build_client(provider_name, model_id)
        .with_context(|| format!("build api client for {provider_name}/{model_id}"))?;

    let tool_registry = ToolRegistry::new();
    register_suite_tools(&tool_registry);
    let tools = Arc::new(tool_registry);

    let cancel = CancellationToken::new();

    let permission_mode = session_cfg.permission_mode;
    let bypass_available = matches!(permission_mode, PermissionMode::BypassPermissions);
    let features = session_cfg
        .features
        .clone()
        .unwrap_or_else(Features::with_defaults);
    let config = QueryEngineConfig {
        model_id: model_id.to_string(),
        permission_mode,
        bypass_permissions_available: bypass_available,
        context_window: session_cfg.context_window,
        max_output_tokens: session_cfg.max_output_tokens,
        max_turns: session_cfg.max_turns,
        total_token_budget: session_cfg.total_token_budget,
        system_prompt: session_cfg.system_prompt,
        is_non_interactive: true,
        project_dir: Some(workdir_path.clone()),
        cwd_override: Some(workdir_path.clone()),
        features: Arc::new(features),
        tool_overrides: Arc::new(ToolOverrides::none()),
        system_reminder: session_cfg.system_reminder,
        max_budget_usd: session_cfg.max_budget_usd,
        ..QueryEngineConfig::default()
    };

    let model_runtimes = api_client.registry();
    // Route the engine's Main turn to the resolved target — the provider
    // whose credentials `require_live!` actually checked. Without this the
    // engine keeps its default `Role(Main)` source, which resolves against
    // the shared runtime's placeholder `model_override` (anthropic) and
    // calls the wrong provider regardless of the test's `(provider, model)`.
    let main_source = ModelRuntimeSource::Explicit(coco_types::ProviderModelSelection {
        provider: provider_name.to_string(),
        model_id: model_id.to_string(),
    });
    let mut engine = QueryEngine::new(
        config,
        model_runtimes,
        tools,
        cancel,
        session_cfg.hooks.clone(),
    )
    .with_model_runtime_source(main_source);
    if let Some(bootstrap) = session_cfg.session_bootstrap.clone() {
        engine = engine.with_session_bootstrap(bootstrap);
    }

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(session_cfg.event_buffer);
    let drainer = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(evt) = event_rx.recv().await {
            events.push(evt);
        }
        events
    });

    let result = engine
        .run_with_events(prompt, event_tx, coco_types::TurnId::generate())
        .await
        .map_err(|e| {
            anyhow::anyhow!("engine.run_with_events on {provider_name}/{model_id}: {e}")
        })?;

    // CLI sessions can fan out to many LLM HTTP calls per prompt
    // (multi-turn agent loop, tool reflection, compaction). Use the
    // cost tracker's authoritative count so the report distinguishes
    // `record_calls` (one per session) from the real `llm_calls`.
    let llm_calls = result.cost_tracker.total_api_calls.max(0) as u64;
    crate::common::usage_report::record_with_llm_calls(
        provider_name,
        model_id,
        "cli.run_session",
        &result.total_usage,
        llm_calls,
    );

    let events = drainer.await.unwrap_or_default();
    Ok(SessionOutcome {
        result,
        events,
        workdir_path,
        _workdir: workdir,
    })
}

/// Variant that exposes the engine's `CommandQueue` so callers can
/// enqueue mid-turn while `engine.run_with_events` is in flight. The
/// queue is `Clone` and internally `Arc`-shared, so the same handle on
/// both ends drains the same backing storage.
///
/// Usage pattern: spawn a task that waits a short delay, then calls
/// `queue.enqueue(...)`. The engine drains the queue between turns, so
/// a long-running first turn (e.g. one that calls Bash with a small
/// sleep) gives the parallel task time to inject before the loop drains.
pub async fn run_session_with_steering(
    provider_name: &str,
    model_id: &str,
    session_cfg: SessionConfig,
    prompt: &str,
    steer: impl FnOnce(CommandQueue) -> tokio::task::JoinHandle<()> + Send + 'static,
) -> Result<SessionOutcome> {
    let workdir = crate::common::tmpdir::make("coco-tests-cli-")
        .with_context(|| "create cwd tempdir under /tmp")?;
    let workdir_path = workdir.path().to_path_buf();

    for (rel_path, content) in &session_cfg.pre_workdir_files {
        let target = workdir_path.join(rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dirs for {target:?}"))?;
        }
        std::fs::write(&target, content)
            .with_context(|| format!("write pre-workdir file {target:?}"))?;
    }

    let api_client = build_client(provider_name, model_id)
        .with_context(|| format!("build api client for {provider_name}/{model_id}"))?;

    let tool_registry = ToolRegistry::new();
    register_suite_tools(&tool_registry);
    let tools = Arc::new(tool_registry);

    let cancel = CancellationToken::new();
    let permission_mode = session_cfg.permission_mode;
    let bypass_available = matches!(permission_mode, PermissionMode::BypassPermissions);
    let features = session_cfg
        .features
        .clone()
        .unwrap_or_else(Features::with_defaults);
    let config = QueryEngineConfig {
        model_id: model_id.to_string(),
        permission_mode,
        bypass_permissions_available: bypass_available,
        context_window: session_cfg.context_window,
        max_output_tokens: session_cfg.max_output_tokens,
        max_turns: session_cfg.max_turns,
        total_token_budget: session_cfg.total_token_budget,
        system_prompt: session_cfg.system_prompt,
        is_non_interactive: true,
        project_dir: Some(workdir_path.clone()),
        cwd_override: Some(workdir_path.clone()),
        features: Arc::new(features),
        tool_overrides: Arc::new(ToolOverrides::none()),
        system_reminder: session_cfg.system_reminder,
        max_budget_usd: session_cfg.max_budget_usd,
        ..QueryEngineConfig::default()
    };

    let model_runtimes = api_client.registry();
    // Route the engine's Main turn to the resolved target — the provider
    // whose credentials `require_live!` actually checked. Without this the
    // engine keeps its default `Role(Main)` source, which resolves against
    // the shared runtime's placeholder `model_override` (anthropic) and
    // calls the wrong provider regardless of the test's `(provider, model)`.
    let main_source = ModelRuntimeSource::Explicit(coco_types::ProviderModelSelection {
        provider: provider_name.to_string(),
        model_id: model_id.to_string(),
    });
    let mut engine = QueryEngine::new(
        config,
        model_runtimes,
        tools,
        cancel,
        session_cfg.hooks.clone(),
    )
    .with_model_runtime_source(main_source);
    if let Some(bootstrap) = session_cfg.session_bootstrap.clone() {
        engine = engine.with_session_bootstrap(bootstrap);
    }

    let queue: CommandQueue = engine.command_queue().clone();
    let steer_task = steer(queue);

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(session_cfg.event_buffer);
    let drainer = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(evt) = event_rx.recv().await {
            events.push(evt);
        }
        events
    });

    let result = engine
        .run_with_events(prompt, event_tx, coco_types::TurnId::generate())
        .await
        .map_err(|e| {
            anyhow::anyhow!("engine.run_with_events on {provider_name}/{model_id}: {e}")
        })?;
    let _ = steer_task.await;

    let llm_calls = result.cost_tracker.total_api_calls.max(0) as u64;
    crate::common::usage_report::record_with_llm_calls(
        provider_name,
        model_id,
        "cli.run_session_with_steering",
        &result.total_usage,
        llm_calls,
    );

    let events = drainer.await.unwrap_or_default();
    Ok(SessionOutcome {
        result,
        events,
        workdir_path,
        _workdir: workdir,
    })
}
