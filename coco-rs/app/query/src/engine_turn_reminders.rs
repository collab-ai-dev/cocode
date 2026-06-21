//! Per-turn system-reminder pipeline extracted from `run_session_loop`.
//!
//! The reminder pipeline is the longest single block of work the agent loop
//! performs each turn (5 phases, ~500 lines):
//! 1. Plan-mode side effects that *mutate* `app_state`
//!    (`turn_start_side_effects_only`).
//! 2. Build a [`TurnReminderInput`] from engine state + a fresh
//!    `app_state` snapshot + per-source [`ReminderSources::materialize`]
//!    fan-out.
//! 3. Run the orchestrator with that input → reminders.
//! 4. Post-emit bookkeeping: clear stale exit flags, bump cadence counters,
//!    refresh "last announced" sets for delta-style reminders.
//! 5. Drain the silent attachment inbox + inject reminders into history.
//!
//! Owns one method on [`QueryEngine`], [`QueryEngine::run_turn_reminder_pipeline`],
//! that performs all five phases. Returns the `app_state` snapshot the
//! caller passes to [`QueryEngine::build_tool_definitions`].

use std::collections::HashSet;

use coco_messages::CostTracker;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_system_reminder::AttachmentType as ReminderAttachmentType;
use coco_system_reminder::SystemReminderOrchestrator;
use coco_system_reminder::TurnReminderInput;
use coco_system_reminder::count_human_turns;
use coco_system_reminder::inject_reminders;
use coco_system_reminder::run_turn_reminders;
use coco_types::Feature;
use coco_types::PermissionMode;
use coco_types::SubagentType;
use coco_types::TokenUsage;
use coco_types::ToolAppState;
use coco_types::ToolName;

use crate::engine::QueryEngine;
use crate::engine_helpers::compute_agents_delta;
use crate::engine_helpers::compute_mcp_instructions_delta;
use crate::engine_helpers::compute_tools_delta;
use crate::engine_helpers::latest_user_input_text;
use crate::plan_mode_reminder::PlanModeReminder;

/// Per-turn inputs to [`QueryEngine::run_turn_reminder_pipeline`].
///
/// Bundles the local-to-`run_session_loop` state the pipeline needs to
/// thread through. Three flavors:
/// - `&mut`: state the pipeline mutates (`history`, `plan_reminder`,
///   `last_user_input_uuid`).
/// - `&`: read-only per-turn signals (`orchestrator`, `total_usage`,
///   `cost_tracker`).
/// - by-value: small scalars (`todo_key`, `context_window`,
///   `effective_window`).
pub(crate) struct TurnReminderContext<'a> {
    pub history: &'a mut MessageHistory,
    pub plan_reminder: &'a mut PlanModeReminder,
    pub orchestrator: &'a SystemReminderOrchestrator,
    pub last_user_input_uuid: &'a mut Option<uuid::Uuid>,
    pub total_usage: &'a TokenUsage,
    pub cost_tracker: &'a CostTracker,
    pub todo_key: &'a str,
    pub context_window: i64,
    pub effective_window: i64,
    /// Wire channel for the reminder injection step so each
    /// model-visible reminder appended to `MessageHistory` emits a
    /// `ServerNotification::MessageAppended` and observers stay
    /// coherent (I-1 authority). `None` for paths that don't have a
    /// wire (forked agents, tests).
    pub event_tx: &'a Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
}

#[cfg(test)]
#[path = "engine_turn_reminders.test.rs"]
mod tests;

impl QueryEngine {
    /// Run the per-turn system-reminder pipeline.
    ///
    /// 1. `plan_reminder.turn_start_side_effects_only` (mode reconcile +
    ///    mailbox + leader-pending). Mutates `app_state` BEFORE the
    ///    orchestrator reads it below.
    /// 2. Build [`TurnReminderInput`] from engine state + a fresh
    ///    `app_state` snapshot + a parallel [`ReminderSources::materialize`].
    /// 3. Run the orchestrator → reminders.
    /// 4. Post-emit bookkeeping: clear stale exit flags, bump plan-mode
    ///    cadence counters, refresh "last announced" sets for delta
    ///    reminders.
    /// 5. Drain silent attachments, inject reminders into history.
    ///
    /// Returns the `app_state` snapshot the caller passes to
    /// [`QueryEngine::build_tool_definitions`] for the same turn — same
    /// permission mode / pre-plan-mode / stripped-rules view the reminders
    /// just observed.
    pub(crate) async fn run_turn_reminder_pipeline(
        &self,
        ctx: TurnReminderContext<'_>,
    ) -> ToolAppState {
        let TurnReminderContext {
            history,
            plan_reminder,
            orchestrator: reminder_orchestrator,
            last_user_input_uuid,
            total_usage,
            cost_tracker,
            todo_key: reminder_todo_key,
            context_window: reminder_context_window,
            effective_window: reminder_effective_window,
            event_tx,
        } = ctx;
        // Phase 1. Run non-reminder side effects (mode reconciliation +
        // mailbox polling + leader pending-approvals) — these MUTATE
        // app_state (setting `needs_plan_mode_exit_attachment` /
        // `has_exited_plan_mode` when detecting unannounced mode
        // transitions). Must run BEFORE the orchestrator reads
        // app_state below.
        plan_reminder.turn_start_side_effects_only(history).await;

        // Phase 2. Build orchestrator input from engine state + current
        // app_state snapshot.
        //
        // `turn_number` uses **human turns** (non-meta user messages)
        // so plan-mode / auto-mode throttle cadence counts human turns,
        // not LLM iterations. Tool-result rounds within one human turn
        // share the same counter value so reminders don't spam mid-turn.
        // Take the app_state snapshot *before* building the stub so
        // the reminder context observes the same discovered-tool set
        // that `engine_prompt::build_tool_definitions` will use this
        // turn — keeps the `deferred_tools_delta` reminder aligned
        // with what the LLM actually receives as tools.
        let pre_snapshot_discovered: std::sync::Arc<std::collections::HashSet<String>> = match self
            .app_state
            .as_ref()
        {
            Some(state) => std::sync::Arc::new(state.read().await.discovered_tool_names.clone()),
            None => std::sync::Arc::new(std::collections::HashSet::new()),
        };
        // Match `engine_prompt::build_tool_definitions` capability
        // resolution exactly so the registry partitions seen here
        // line up byte-for-byte with the LLM-visible tools set. When
        // either capability is absent, `tool_search_active()` returns
        // false in the registry and `deferred_tools` is empty — the
        // `deferred_tools_delta` reminder collapses to "nothing
        // searchable", which is the correct truth.
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
        // Both partitions share the same filter context so they
        // cover disjoint halves of the registry — `loaded` includes
        // discovered tools, `deferred` excludes them.
        let (reminder_loaded_tools, reminder_deferred_tools): (Vec<String>, Vec<String>) = {
            let stub_ctx = coco_tool_runtime::ToolUseContext::stub_for_filtering(
                self.config.features.clone(),
                self.config.tool_overrides.clone(),
                self.config.tool_filter.clone(),
                self.config.permission_mode,
            )
            .with_discovered_tool_names(pre_snapshot_discovered)
            .with_model_capabilities(supports_tool_reference, supports_client_side_tool_search)
            .with_active_shell_tool(self.config.active_shell_tool);
            let stub_ctx = self.with_current_tool_search_candidates(stub_ctx).await;
            let mut loaded: Vec<String> = self
                .tools
                .loaded_tools(&stub_ctx)
                .iter()
                .map(|t| t.name().to_string())
                .collect();
            loaded.sort();
            let mut deferred: Vec<String> = self
                .tools
                .deferred_tools(&stub_ctx)
                .iter()
                .map(|t| t.name().to_string())
                .collect();
            deferred.sort();
            (loaded, deferred)
        };
        // `reminder_tools` is the model-visible (loaded) tool list —
        // used by `TurnReminderInput::tools` and unchanged consumers below.
        let reminder_tools = reminder_loaded_tools.clone();
        // Current active agent types from the wired catalog (see
        // `current_agent_types` — NOT the dead `session_bootstrap.agents`).
        // Cache-shared forks (`fork_label.is_some()`) are EXCLUDED, exactly like
        // `current_date` below: they reuse the parent's byte-for-byte prompt
        // prefix, so injecting an agent-listing-delta / agent-mention reminder
        // would break cache parity (PR #18143). Main loop + real subagents
        // (`fork_label` None) get the catalog-derived agent reminders.
        let reminder_current_agents = if self.config.fork_label.is_none() {
            self.current_agent_types()
        } else {
            Vec::new()
        };
        let reminder_explore_plan_agents_available = reminder_tools
            .iter()
            .any(|name| name == ToolName::Agent.as_str())
            && reminder_current_agents
                .iter()
                .any(|name| name == SubagentType::Explore.as_str())
            && reminder_current_agents
                .iter()
                .any(|name| name == SubagentType::Plan.as_str());
        let reminder_skill_tool_loaded = reminder_loaded_tools
            .iter()
            .any(|name| name == ToolName::Skill.as_str());
        tracing::debug!(
            loaded_tools = ?reminder_loaded_tools,
            skill_tool_loaded = reminder_skill_tool_loaded,
            "turn reminder loaded tool set resolved"
        );
        let pm_settings = &self.config.plan_mode_settings;
        let workflow_rm = match pm_settings.workflow {
            coco_config::PlanModeWorkflow::FivePhase => coco_context::PlanWorkflow::FivePhase,
            coco_config::PlanModeWorkflow::Interview => coco_context::PlanWorkflow::Interview,
        };
        let phase4_rm = match pm_settings.phase4_variant {
            coco_config::PlanPhase4Variant::Standard => coco_context::Phase4Variant::Standard,
            coco_config::PlanPhase4Variant::Trim => coco_context::Phase4Variant::Trim,
            coco_config::PlanPhase4Variant::Cut => coco_context::Phase4Variant::Cut,
            coco_config::PlanPhase4Variant::Cap => coco_context::Phase4Variant::Cap,
        };
        // Plan file path / existence — same resolver the deprecated
        // emission path uses, so both paths agree on the filesystem state.
        let (reminder_plan_path, reminder_plan_exists) =
            match (self.config_home.as_deref(), &self.config.session_id) {
                (Some(ch), sid) if !sid.is_empty() => {
                    let plans_dir = coco_context::resolve_plans_directory(
                        ch,
                        self.config.project_dir.as_deref(),
                        self.config.plans_directory.as_deref(),
                    );
                    let path = coco_context::get_plan_file_path(
                        sid,
                        &plans_dir,
                        self.config.agent_id.as_deref(),
                    );
                    let exists = path.exists();
                    (Some(path), exists)
                }
                _ => (None, false),
            };

        let reminder_human_turn_number = count_human_turns(history.as_slice());

        // Take an app_state snapshot so the input struct holds an
        // immutable borrow; any post-emit clearing happens after the
        // orchestrator returns.
        let app_state_snapshot = match self.app_state.as_ref() {
            Some(state) => state.read().await.clone(),
            None => ToolAppState::default(),
        };

        // Plan/auto reminder cadence is history-derived inside the
        // generators (turn-scan over the transcript), so there is no
        // throttle state to seed from `app_state` across `run_internal`
        // invocations — the history persists and is the source of truth.

        // `None` means the engine was built without a permissions
        // auto-mode state — auto mode is therefore inactive (false fallback).
        let reminder_auto_classifier_active = self
            .auto_mode_state
            .as_ref()
            .map(|s| s.is_active())
            .unwrap_or(false);
        let reminder_permission_mode = app_state_snapshot
            .permissions
            .mode
            .unwrap_or(self.config.permission_mode);
        let reminder_is_plan_mode = reminder_permission_mode == PermissionMode::Plan;
        let reminder_is_auto_mode = reminder_permission_mode == PermissionMode::Auto
            || (reminder_permission_mode == PermissionMode::Plan
                && reminder_auto_classifier_active);
        // Read the explicit `Feature::TaskV2` gate (default-on) which
        // drives the V1/V2 mutual exclusion at tool level (`is_enabled`
        // on TodoWrite vs TaskCreate/Get/List/Update). The flag is the
        // source of truth; `TASK_MANAGEMENT_TOOLS` is still used by
        // `turn_runner` for counting turns since the last task mutation,
        // but no longer for V2-mode detection.
        let reminder_task_v2_enabled = self.config.features.enabled(Feature::TaskV2);
        // `plan_mode` feature gate (default on). When off, every plan-mode
        // reminder is suppressed — mirrors the hidden EnterPlanMode/ExitPlanMode
        // tools and the Plan rung removed from the TUI mode cycle.
        let reminder_plan_mode_enabled = self.config.features.enabled(Feature::PlanMode);
        // Resolves through `QueryEngineConfig.compact.auto.is_active()`
        // (user toggle AND env kill switches) so the SDK / CLI / TUI
        // can control it per session without re-reading settings from
        // disk.
        let reminder_auto_compact_enabled = self.config.is_auto_compact_active();
        // Diff the current **deferred** tool set against the last
        // announced set on app_state. The loaded set is supplied so a
        // tool that moves deferred → loaded (model discovered via
        // `ToolSearch`) stays silently in the announced pool.
        // Non-empty added or removed triggers the `deferred_tools_delta`
        // reminder.
        let reminder_deferred_tools_delta = compute_tools_delta(
            &reminder_deferred_tools,
            &reminder_loaded_tools,
            &app_state_snapshot.last_announced_tools,
        );
        // Hand the deferred list to post-emit bookkeeping — replaces
        // `announced` with the current deferred set after emission. Moved
        // (last use): the delta above already read it by reference.
        let reminder_deferred_tools_clone = reminder_deferred_tools;
        // Diff the current agent-type set (resolved above from the wired
        // catalog) against the last-announced set on app_state.
        let reminder_agent_listing_delta = compute_agents_delta(
            &reminder_current_agents,
            &app_state_snapshot.last_announced_agents,
        );
        // Date-change latch: current local ISO date vs. the one stored
        // on `ToolAppState.last_emitted_date`. When they differ, emit
        // once + update the latch. Runs at turn start so the reminder
        // sees today's date even for long-running sessions that cross
        // midnight.
        let reminder_new_date = self.observe_date_change().await;
        // Today's local ISO date injected every turn (independent of the
        // rollover latch above) so the model always has the date.
        // Cache-shared forks (`fork_label.is_some()`) are EXCLUDED: they
        // reuse the parent's byte-for-byte prompt prefix and injecting a
        // per-turn message would break cache parity (PR #18143). Main loop
        // + real subagents (`fork_label` None) get it.
        let reminder_current_date = self
            .config
            .fork_label
            .is_none()
            .then(|| chrono::Local::now().format("%Y-%m-%d").to_string());

        // The user's raw prompt text for this turn. Extract from the
        // most-recent non-meta user message's text content; used by
        // both the ultrathink-keyword gate and mention-based reminders.
        //
        // Only the first tool-loop iteration of a human turn gets a
        // non-null input; subsequent tool-result rounds skip it. Track
        // the last user-message UUID already reminder-scanned so the
        // user-input tier fires once per human turn, not once per
        // tool-result iteration.
        let reminder_current_user_uuid = history.iter().rev().find_map(|m| match m.as_ref() {
            Message::User(u) => Some(u.uuid),
            _ => None,
        });
        let reminder_is_new_human_turn = reminder_current_user_uuid != *last_user_input_uuid;
        let reminder_user_input: Option<String> = if reminder_is_new_human_turn {
            *last_user_input_uuid = reminder_current_user_uuid;
            latest_user_input_text(history)
        } else {
            None
        };
        let reminder_mentions: Vec<coco_context::user_input::Mention> = reminder_user_input
            .as_deref()
            .map(|raw| coco_context::user_input::process_user_input(raw).mentions)
            .unwrap_or_default();
        let reminder_at_mentioned_files: Vec<coco_system_reminder::MentionedFileEntry> =
            reminder_mentions
                .iter()
                .filter(|m| {
                    matches!(
                        m.mention_type,
                        coco_context::user_input::MentionType::FilePath
                    )
                })
                .map(|m| coco_system_reminder::MentionedFileEntry {
                    filename: m.text.clone(),
                    display_path: m.text.clone(),
                })
                .collect();
        let reminder_agent_mentions =
            active_agent_mentions(&reminder_mentions, &reminder_current_agents);

        // Fan-out to every per-subsystem source (hooks / LSP / tasks /
        // skills / MCP / swarm / IDE / memory) in parallel, with
        // per-source timeout + error-to-default.
        // Empty `ReminderSources` → all defaults.
        //
        // Resolve relative paths against cwd so they match the absolute
        // keys used by `FileReadState` (populated by `mention_resolver`
        // and the Read tool). Without this, the AlreadyReadFile silent
        // reminder and nested-memory lookups never hit on @-mentions.
        let reminder_cwd = std::env::current_dir().unwrap_or_default();
        let reminder_mentioned_paths: Vec<std::path::PathBuf> = reminder_mentions
            .iter()
            .filter(|m| {
                matches!(
                    m.mention_type,
                    coco_context::user_input::MentionType::FilePath
                )
            })
            .map(|m| {
                let p = std::path::PathBuf::from(&m.text);
                if p.is_absolute() {
                    p
                } else {
                    reminder_cwd.join(p)
                }
            })
            .collect();

        let reminder_source_timeout =
            std::time::Duration::from_millis(if reminder_orchestrator.config().timeout_ms > 0 {
                reminder_orchestrator.config().timeout_ms as u64
            } else {
                coco_system_reminder::DEFAULT_TIMEOUT_MS as u64
            });
        // One-shot flag: every successful compaction (full / SM / reactive)
        // sets it; the next reminder build consumes (swap-to-false) so
        // `task_status` only fires on the immediately-following turn.
        let just_compacted = self
            .pending_just_compacted
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        // Tools the model successfully invoked since the previous human
        // turn. Fed into the `findRelevantMemories` ranker so it can
        // deprioritize reference docs for tools the model is actively
        // exercising. Empty when no human-turn boundary is established
        // yet (start of conversation, or no successful tool runs).
        let reminder_recent_tools = collect_recent_successful_tools(history.as_slice());
        let materialized = self
            .reminder_sources
            .materialize(coco_system_reminder::MaterializeContext {
                config: reminder_orchestrator.config(),
                agent_id: self.config.agent_id.as_deref(),
                user_input: reminder_user_input.as_deref(),
                mentioned_paths: &reminder_mentioned_paths,
                recent_tools: &reminder_recent_tools,
                just_compacted,
                per_source_timeout: reminder_source_timeout,
                skill_overrides: &self.config.skill_overrides,
                skill_tool_loaded: reminder_skill_tool_loaded,
            })
            .await;

        // Coordinator (leader) worker-tools context for the `user_context`
        // reminder's `# workerToolsContext` block. Only the main leader gets
        // it: workers (`agent_id.is_some()`) are excluded because the
        // coordinator env var is process-global and would otherwise leak the
        // leader's worker pool into every worker; cache-shared forks
        // (`fork_label.is_some()`) are excluded for the same prompt-cache
        // parity reason as `current_date`. `coordinator_user_context`
        // self-gates on `is_coordinator_mode`, so this stays `None` outside
        // coordinator mode. `simple_mode` mirrors the system-prompt /
        // worker-pool gate (`EnvKey::CocoSimple`) so the advertised pool
        // matches what workers actually receive. MCP names degrade to the
        // servers currently advertising instructions — the only source on the
        // reminder path that doesn't reach back into the MCP registry.
        let reminder_coordinator_context =
            if self.config.agent_id.is_none() && self.config.fork_label.is_none() {
                let mcp_names: Vec<&str> = materialized
                    .mcp_instructions_current
                    .keys()
                    .map(String::as_str)
                    .collect();
                let simple_mode = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoSimple);
                coco_subagent::coordinator_user_context(
                    &self.config.features,
                    &mcp_names,
                    /*scratchpad_dir*/ None,
                    /*scratchpad_gate_enabled*/ false,
                    simple_mode,
                )
                .remove("workerToolsContext")
            } else {
                None
            };

        // Part 1 silent reminder: intersect every path this turn
        // might try to load (@-mentions + nested memory + relevant
        // memory prefetch) with the session file-read cache. Paths
        // whose mtime still matches disk are "already loaded into
        // context" — emit a silent dedup marker so downstream tooling
        // (transcript, telemetry) knows the model has current content
        // for those paths.
        let reminder_already_read_file_paths: Vec<std::path::PathBuf> =
            if let Some(frs) = &self.file_read_state {
                let mut candidates: Vec<std::path::PathBuf> = reminder_mentioned_paths.clone();
                candidates.extend(
                    materialized
                        .nested_memories
                        .iter()
                        .map(|m| std::path::PathBuf::from(&m.path)),
                );
                candidates.extend(
                    materialized
                        .relevant_memories
                        .iter()
                        .map(|m| std::path::PathBuf::from(&m.path)),
                );
                if candidates.is_empty() {
                    Vec::new()
                } else {
                    // Dedup while preserving first-seen order so the
                    // resulting list is deterministic across turns.
                    let mut seen = HashSet::new();
                    candidates.retain(|p| seen.insert(p.clone()));
                    let guard = frs.read().await;
                    guard.unchanged_paths(&candidates).await
                }
            } else {
                Vec::new()
            };

        // Cross-turn typed silent attachments (command_permissions,
        // structured_output, hook events, …) come in via the session
        // `AttachmentEmitter` and are drained from the inbox at the
        // head of `inject_reminders_into_history` below — there is no
        // separate mailbox state to thread into `TurnReminderInput`.

        let reminder_input = TurnReminderInput {
            config: reminder_orchestrator.config(),
            turn_number: reminder_human_turn_number,
            agent_id: self.config.agent_id.clone(),
            user_input: reminder_user_input,
            last_human_turn_uuid: history.iter().rev().find_map(|m| match m.as_ref() {
                Message::User(u) => Some(u.uuid),
                _ => None,
            }),
            plan_file_path: reminder_plan_path,
            plan_exists: reminder_plan_exists,
            plan_workflow: workflow_rm,
            phase4_variant: phase4_rm,
            explore_agent_count: pm_settings.explore_agent_count,
            plan_agent_count: pm_settings.plan_agent_count,
            explore_plan_agents_available: reminder_explore_plan_agents_available,
            is_plan_interview_phase: false,
            plan_mode_feature_enabled: reminder_plan_mode_enabled,
            app_state: &app_state_snapshot,
            fallback_permission_mode: self.config.permission_mode,
            is_auto_classifier_active: reminder_auto_classifier_active,
            tools: reminder_tools,
            deferred_tools: reminder_deferred_tools_clone.clone(),
            is_task_v2_enabled: reminder_task_v2_enabled,
            history,
            todo_key: reminder_todo_key.to_string(),
            is_auto_compact_enabled: reminder_auto_compact_enabled,
            context_window: reminder_context_window,
            effective_context_window: reminder_effective_window,
            used_tokens: total_usage.input_tokens.total,
            new_date: reminder_new_date,
            current_date: reminder_current_date,
            coordinator_worker_context: reminder_coordinator_context,
            has_pending_plan_verification: app_state_snapshot
                .pending_plan_verification
                .as_ref()
                .is_some_and(coco_types::PendingPlanVerificationState::needs_reminder),
            // Phase 1 engine-local inputs.
            total_cost_usd: cost_tracker.total_cost_usd(),
            max_budget_usd: self.config.max_budget_usd,
            // Injected at turn start — output token count is zero at this
            // point; cumulative session count comes from usage.
            output_tokens_turn: 0,
            output_tokens_session: total_usage.output_tokens.total,
            // Mirrors claude-code's output_config.task_budget.total — set
            // programmatically via QueryEngineConfig (no CLI flag in TS).
            // `None` keeps the output_token_usage reminder dormant.
            output_token_budget: self.config.output_token_budget,
            // Companion subsystem lives in a future Buddy crate; for now
            // suppress the reminder by leaving these unset.
            companion_name: None,
            companion_species: None,
            has_prior_companion_intro: false,
            deferred_tools_delta: reminder_deferred_tools_delta,
            agent_listing_delta: reminder_agent_listing_delta,
            // McpSource.instructions() returns the current per-server map;
            // engine diffs against `last_announced_mcp_instructions` to
            // produce the delta (same pattern as deferred_tools_delta).
            mcp_instructions_delta: compute_mcp_instructions_delta(
                &materialized.mcp_instructions_current,
                &app_state_snapshot.last_announced_mcp_instructions,
            ),
            // Phase 3: cross-crate state flows via `ReminderSources`.
            // Sources that aren't wired → default output → generator skips.
            hook_events: materialized.hook_events,
            diagnostics: materialized.diagnostics,
            // Reads style name from `SessionBootstrap`
            // (CLI-resolved from `settings.output_style`).
            // This is a simple read, not cross-crate state, so no Source
            // trait is needed.
            output_style: self
                .session_bootstrap
                .as_ref()
                .and_then(|b| b.output_style.as_ref())
                .filter(|s| !s.is_empty())
                .map(|name| coco_system_reminder::OutputStyleSnapshot { name: name.clone() }),
            queued_commands: self
                .command_queue
                .snapshot_for_reminder(self.config.agent_id.as_deref())
                .await,
            task_statuses: materialized.task_statuses,
            // SkillsSource wins when present; else fall back to
            // SessionBootstrap names-only listing.
            skill_listing: if reminder_skill_tool_loaded {
                materialized.skill_listing.or_else(|| {
                    self.session_bootstrap
                        .as_ref()
                        .filter(|b| !b.skills.is_empty())
                        .map(|b| {
                            b.skills
                                .iter()
                                .map(|s| format!("- {s}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                })
            } else {
                None
            },
            invoked_skills: materialized.invoked_skills,
            teammate_mailbox: materialized.teammate_mailbox,
            team_context: materialized.team_context,
            agent_pending_messages: materialized.agent_pending_messages,
            // Phase 4: mention-based reminders are populated from
            // `process_user_input`. MCP resources come from the MCP
            // source; IDE state is a main-thread reminder source.
            at_mentioned_files: reminder_at_mentioned_files,
            mcp_resources: materialized.mcp_resources,
            agent_mentions: reminder_agent_mentions,
            ide_selection: materialized.ide_selection,
            ide_opened_file: materialized.ide_opened_file,
            // Nested memories: engine-driven via the per-batch
            // `drain_nested_memory_triggers` pipeline (Read tool
            // populates `ctx.nested_memory_attachment_triggers` →
            // engine drains end-of-batch → traverses CWD→file →
            // appends here). `MemoryAdapter::nested_memories`
            // intentionally returns empty so the engine path is the
            // single source. We `extend` (rather than replace) in case
            // a future MemorySource impl wants to contribute as well —
            // currently the materialized side is always empty.
            nested_memories: {
                let mut v = self.take_pending_nested_memory().await;
                if !materialized.nested_memories.is_empty() {
                    v.extend(materialized.nested_memories);
                }
                v
            },
            relevant_memories: materialized.relevant_memories,
            // Silent reminder-native attachments (Part 1).
            // `already_read_file_paths`: intersection of this turn's
            // @-mentioned paths with the `FileReadState` cache where
            // mtime still matches disk — computed above via
            // `FileReadState::unchanged_paths`.
            // `edited_image_file_paths`: reserved for a future image-
            // mtime tracker. Text `FileReadState` is text-only; image
            // drift detection would need a parallel cache.
            already_read_file_paths: reminder_already_read_file_paths,
            edited_image_file_paths: Vec::new(),
            skill_discovery: materialized.skill_discovery,
        };
        let reminders = run_turn_reminders(reminder_orchestrator, reminder_input).await;

        // Phase 4. Post-emit bookkeeping on app_state. Writing AFTER the
        // orchestrator read ensures we don't clear a flag whose
        // reminder got throttled (so it can fire next turn).
        //
        // Covers the one-shot flags consumed by the generators that fired
        // (PlanModeExit / AutoModeExit / PlanModeReentry) plus the delta
        // baselines. Plan/auto reminder cadence is no longer mirrored here —
        // it is derived from the transcript history each turn.
        let stale_plan_exit_flag =
            app_state_snapshot.needs_plan_mode_exit_attachment && reminder_is_plan_mode;
        let stale_auto_exit_flag =
            app_state_snapshot.needs_auto_mode_exit_attachment && reminder_is_auto_mode;
        let needs_reminder_bookkeeping =
            !reminders.is_empty() || stale_plan_exit_flag || stale_auto_exit_flag;
        if needs_reminder_bookkeeping && self.app_state.is_some() {
            let fired_types: HashSet<ReminderAttachmentType> =
                reminders.iter().map(|r| r.attachment_type).collect();
            if let Some(state) = self.app_state.as_ref() {
                let mut guard = state.write().await;
                // Clear stale one-shot exit flags when the engine is
                // still in the matching mode instead of preserving them
                // for a later, unrelated turn.
                if stale_plan_exit_flag {
                    guard.needs_plan_mode_exit_attachment = false;
                    guard.pending_plan_mode_exit_outcome = None;
                }
                if stale_auto_exit_flag {
                    guard.needs_auto_mode_exit_attachment = false;
                }
                if fired_types.contains(&ReminderAttachmentType::PlanModeExit) {
                    guard.needs_plan_mode_exit_attachment = false;
                    guard.pending_plan_mode_exit_outcome = None;
                }
                if fired_types.contains(&ReminderAttachmentType::AutoModeExit) {
                    guard.needs_auto_mode_exit_attachment = false;
                }
                if fired_types.contains(&ReminderAttachmentType::PlanModeReentry) {
                    guard.has_exited_plan_mode = false;
                }
                // Replace the announced set with the current **deferred**
                // tool list after successful emission. Subsequent turns
                // then diff against the fresh baseline.
                if fired_types.contains(&ReminderAttachmentType::DeferredToolsDelta) {
                    guard.last_announced_tools =
                        reminder_deferred_tools_clone.iter().cloned().collect();
                }
                // Same pattern for the agent-listing delta.
                if fired_types.contains(&ReminderAttachmentType::AgentListingDelta) {
                    guard.last_announced_agents = reminder_current_agents.iter().cloned().collect();
                }
                // Same pattern for the MCP-instructions delta.
                if fired_types.contains(&ReminderAttachmentType::McpInstructionsDelta) {
                    guard.last_announced_mcp_instructions =
                        materialized.mcp_instructions_current.clone();
                }
            }
        }

        // Phase 5. Inject typed attachment events and reminder messages into history. Model-visible
        // reminders append to `history`; silent reminders
        // (`Coverage::SilentReminder` + `ReminderOutput::Silent*`)
        // come back as `display_only` so they never leak into the
        // API call but stay observable for UI / telemetry.
        // Drain any silent attachments queued by owner crates
        // (hooks / permissions / tools / etc.) since the prior turn.
        // Must happen BEFORE inject_reminders so the reminder pipeline
        // sees any cross-crate-produced attachments in history.
        let drained = self.drain_attachment_inbox(history, event_tx).await;
        if drained > 0 {
            tracing::debug!(
                target: "coco::attachment_inbox",
                drained,
                "drained silent attachments into history"
            );
        }

        // I-1 (Authority): every reminder appended to history must emit
        // a `MessageAppended` so TUI's TranscriptView + SDK NDJSON
        // observers track it. Pull the materialized reminder messages
        // and push each through the event-emitting helper.
        let batch = inject_reminders(reminders);
        if !batch.is_empty() {
            tracing::debug!(
                target: "coco::system_reminder::inject",
                model_visible = batch.model_visible.len(),
                display_only = batch.display_only.len(),
                "injecting reminder batch",
            );
        }
        for msg in batch.model_visible {
            crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
        }
        for msg in &batch.display_only {
            tracing::debug!(
                target: "coco::system_reminder::display_only",
                injected = ?msg,
                "silent reminder routed to display-only sink"
            );
        }

        app_state_snapshot
    }
}

/// Resolve the `@agent-…` mentions in this turn's user input into
/// `agent_mention` reminder entries, mirroring TS `processAgentMentions`:
///
/// - strip the `agent-` prefix via first-occurrence replace (TS
///   `mention.replace('agent-', '')`) so the unquoted form (`@agent-Explore` →
///   `"Explore"`) and the quoted form (`@"Explore (agent)"`, already
///   suffix-stripped by `process_user_input` → `"Explore"`) both normalize to
///   the bare type, and
/// - drop any mention that doesn't resolve to an active agent type
///   (`active_agents` = the wired catalog's `def.name`s, the same set
///   `AgentTool::execute` validates `subagent_type` against), so the reminder
///   never tells the model to invoke an agent the spawn would reject as an
///   unknown `subagent_type`. The runtime guard stays as a backstop.
fn active_agent_mentions(
    mentions: &[coco_context::user_input::Mention],
    active_agents: &[String],
) -> Vec<coco_system_reminder::AgentMentionEntry> {
    mentions
        .iter()
        .filter(|m| matches!(m.mention_type, coco_context::user_input::MentionType::Agent))
        .filter_map(|m| {
            let agent_type = m.text.replacen("agent-", "", 1);
            active_agents
                .iter()
                .any(|a| a == &agent_type)
                .then_some(coco_system_reminder::AgentMentionEntry { agent_type })
        })
        .collect()
}

/// Tool names the assistant successfully invoked since the previous
/// human turn:
///
/// - Slice messages from the most recent user message to the end —
///   that's "this human turn".
/// - In that slice, harvest `tool_call_id → tool_name` from every
///   assistant `ToolCall` block, and `tool_use_id → !is_error` from
///   every `ToolResultMessage`.
/// - Return the unique set of tool names whose result was non-error.
///
/// Empty when no human turn has happened yet (no Message::User in
/// history) or no tool produced a non-error result. Order is
/// unspecified — the recall ranker uses set membership only.
fn collect_recent_successful_tools<M: std::borrow::Borrow<Message>>(messages: &[M]) -> Vec<String> {
    use coco_messages::AssistantContent;
    use coco_messages::LlmMessage;
    use std::collections::HashMap;

    let Some(last_user_idx) = messages
        .iter()
        .rposition(|m| matches!(m.borrow(), Message::User(_)))
    else {
        return Vec::new();
    };

    let mut use_id_to_name: HashMap<String, String> = HashMap::new();
    let mut success_by_use_id: HashMap<String, bool> = HashMap::new();
    for m in &messages[last_user_idx..] {
        match m.borrow() {
            Message::Assistant(a) => {
                if let LlmMessage::Assistant { content, .. } = &a.message {
                    for block in content {
                        if let AssistantContent::ToolCall(call) = block {
                            use_id_to_name
                                .insert(call.tool_call_id.clone(), call.tool_name.clone());
                        }
                    }
                }
            }
            Message::ToolResult(tr) => {
                success_by_use_id.insert(tr.tool_use_id.clone(), !tr.is_error);
            }
            _ => {}
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (use_id, name) in &use_id_to_name {
        if success_by_use_id.get(use_id).copied().unwrap_or(false) && seen.insert(name.clone()) {
            out.push(name.clone());
        }
    }
    out
}
