//! Engine-facing entry point: collapse all Phase-B/C context building into
//! a single call.
//!
//! This is the one API the [`QueryEngine`](app/query) imports for Phase D.3
//! cutover. It takes every per-turn input as named fields on a single struct,
//! builds the [`GeneratorContext`] using the same composable helpers as
//! `context_builder` + `turn_counting`, then runs
//! [`SystemReminderOrchestrator::generate_all`].
//!
//! Why a dedicated struct instead of a builder at this layer: the engine
//! call-site is a single place that knows *every* piece of state at once.
//! A named-struct literal keeps the call readable (fields are self-labelled)
//! and lets IDE "go to definition" jump straight to the field doc.

use std::path::PathBuf;

use coco_context::Phase4Variant;
use coco_context::PlanWorkflow;
use coco_messages::MessageHistory;
use coco_types::AttachmentKind;
use coco_types::PermissionMode;
use coco_types::SkillDiscoveryPayload;
use coco_types::ToolAppState;
use coco_types::ToolName;
use uuid::Uuid;

use crate::context_builder::apply_app_state;
use crate::context_builder::apply_todos_for_key;
use crate::generator::GeneratorContext;
use crate::orchestrator::SystemReminderOrchestrator;
use crate::turn_counting::TASK_MANAGEMENT_TOOLS;
use crate::turn_counting::count_assistant_turns_since_any_tool;
use crate::turn_counting::count_assistant_turns_since_attachment;
use crate::turn_counting::count_assistant_turns_since_tool;
use crate::turn_counting::count_attachments_since_attachment;
use crate::turn_counting::count_human_turns_since_attachment;
use crate::turn_counting::human_turns_since_attachment_opt;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// All per-turn inputs the orchestrator needs. Engine fills this in once
/// per outer loop iteration.
///
/// Field groupings mirror the [`GeneratorContext`] structure so reviewers
/// can see at a glance that every context field has a source.
pub struct TurnReminderInput<'a> {
    // ── Configuration + turn identity ──
    /// Root reminder config (per-generator toggles + timeout + critical text).
    pub config: &'a SystemReminderConfig,
    /// Current human-turn counter. Retained as a log/diagnostic correlation
    /// field; reminder cadence is history-derived, not driven by this number.
    pub turn_number: i32,

    // ── Agent identity ──
    /// `None` on main thread. Used to pick todo-list scope + skip sub-agent
    /// Reentry.
    pub agent_id: Option<String>,

    // ── Per-turn signals ──
    /// `Some(text)` when the user submitted input this turn.
    pub user_input: Option<String>,

    /// UUID of the most recent non-meta user message in history (coco-rs
    /// plan-mode human-turn tracker).
    pub last_human_turn_uuid: Option<Uuid>,

    // ── Plan mode ──
    /// Pre-resolved plan-file path.
    pub plan_file_path: Option<PathBuf>,
    /// Filesystem check result for the plan file.
    pub plan_exists: bool,
    /// Selected plan workflow (from settings.json → engine config).
    pub plan_workflow: PlanWorkflow,
    /// Phase-4 variant (from settings.json).
    pub phase4_variant: Phase4Variant,
    /// Explore / plan sub-agent counts referenced in the 5-phase Full prompt.
    pub explore_agent_count: i32,
    pub plan_agent_count: i32,
    /// Whether the current turn can actually launch built-in Explore/Plan agents.
    pub explore_plan_agents_available: bool,
    /// True during the interview-style plan workflow.
    pub is_plan_interview_phase: bool,
    /// Whether the `plan_mode` feature is enabled this session (default on).
    /// `false` suppresses every plan-mode reminder regardless of mode.
    pub plan_mode_feature_enabled: bool,

    // ── Shared mutable state ──
    /// Typed app-state snapshot — source of truth for permission mode,
    /// one-shot flags, todos, and plan tasks.
    pub app_state: &'a ToolAppState,
    /// Fallback permission mode (from engine config) when `app_state.permission_mode` is `None`.
    pub fallback_permission_mode: PermissionMode,
    /// Session-scoped auto-mode classifier activity. `true` only when
    /// `core/permissions::AutoModeState::is_active()` returns true. Combined
    /// with `mode == Plan` this enables auto-mode reminders to fire in
    /// both Auto-mode and Plan+auto-classifier.
    pub is_auto_classifier_active: bool,

    // ── Tools ──
    /// Wire names of every tool available this turn (builtins + MCP + custom).
    pub tools: Vec<String>,
    /// Wire names of ToolSearch-searchable tools that are not callable yet.
    pub deferred_tools: Vec<String>,
    /// `true` when V2 task tools are active (drives todo vs task reminder
    /// mutual exclusion).
    pub is_task_v2_enabled: bool,

    // ── History + key for todo lookup ──
    /// Full session history up to this turn (used for assistant-turn counting).
    pub history: &'a MessageHistory,
    /// Key into `app_state.todos_by_agent` (resolved from `agentId ?? sessionId`).
    pub todo_key: String,

    // ── Compaction / token budget ──
    /// True when auto-compaction is enabled.
    pub is_auto_compact_enabled: bool,
    /// Total context window of the active model.
    pub context_window: i64,
    /// Effective context window after output reserve.
    pub effective_context_window: i64,
    /// Tokens currently in context.
    pub used_tokens: i64,

    // ── Date change ──
    /// `Some(iso_date)` when the local date has rolled over since the last
    /// date-change emission; `None` otherwise. Engine's per-session latch.
    pub new_date: Option<String>,

    // ── Per-turn user context ──
    /// Today's local ISO date, injected every turn for the
    /// `user_context` reminder.
    /// `Some` on the live engine path; `None` in tests suppresses it.
    pub current_date: Option<String>,

    /// Coordinator (leader) worker-tools context: the worker tool pool +
    /// connected MCP servers, rendered as the `# workerToolsContext` block in
    /// the same per-turn `user_context` reminder. `Some` only on the main
    /// agent when coordinator mode is active; `None` otherwise.
    pub coordinator_worker_context: Option<String>,

    // ── Verify-plan reminder ──
    /// True when TS-shaped pending plan verification exists and has not
    /// started or completed.
    pub has_pending_plan_verification: bool,

    // ── Phase 1 engine-local inputs ──
    /// Session-level USD spend for `budget_usd`. Engine reads from the
    /// session's `CostTracker::total_cost()`.
    pub total_cost_usd: f64,
    /// Configured USD cap for the session. `None` suppresses
    /// `budget_usd`. Sourced from `QueryEngineConfig::max_budget_usd`.
    pub max_budget_usd: Option<f64>,
    /// Output-token counters for `output_token_usage`.
    pub output_tokens_turn: i64,
    pub output_tokens_session: i64,
    /// Per-turn output-token budget for `output_token_usage`.
    /// `Some(n>0)` activates the reminder.
    pub output_token_budget: Option<i64>,
    /// Configured companion name/species for `companion_intro`. Both
    /// must be `Some` for the reminder to fire.
    pub companion_name: Option<String>,
    pub companion_species: Option<String>,
    /// True when a prior `companion_intro` attachment for the current
    /// companion name already exists in history. Engine pre-scans.
    pub has_prior_companion_intro: bool,

    // ── Phase 2 history-diff delta snapshots ──
    /// Pre-computed `deferred_tools_delta`. `None` suppresses emission;
    /// `Some(info)` with non-empty entries fires the reminder.
    pub deferred_tools_delta: Option<crate::generator::DeferredToolsDeltaInfo>,
    /// Pre-computed `agent_listing_delta`.
    pub agent_listing_delta: Option<crate::generator::AgentListingDeltaInfo>,
    /// Pre-computed `mcp_instructions_delta`.
    pub mcp_instructions_delta: Option<crate::generator::McpInstructionsDeltaInfo>,

    // ── Phase 3 cross-crate snapshots ──
    pub hook_events: Vec<crate::generator::HookEvent>,
    pub diagnostics: Vec<crate::generator::DiagnosticFileSummary>,
    pub output_style: Option<crate::generator::OutputStyleSnapshot>,
    pub queued_commands: Vec<crate::generator::QueuedCommandInfo>,
    pub task_statuses: Vec<crate::generator::TaskStatusSnapshot>,
    pub skill_listing: Option<String>,
    pub invoked_skills: Vec<crate::generator::InvokedSkillEntry>,
    pub teammate_mailbox: Option<crate::generator::TeammateMailboxInfo>,
    pub team_context: Option<crate::generator::TeamContextSnapshot>,
    pub agent_pending_messages: Vec<crate::generator::AgentPendingMessage>,

    // ── Phase 4 user-input snapshots ──
    pub at_mentioned_files: Vec<crate::generators::user_input::MentionedFileEntry>,
    pub mcp_resources: Vec<crate::generators::user_input::McpResourceEntry>,
    pub agent_mentions: Vec<crate::generators::user_input::AgentMentionEntry>,

    // ── Main-thread IDE snapshots ──
    pub ide_selection: Option<crate::generators::user_input::IdeSelectionSnapshot>,
    pub ide_opened_file: Option<crate::generators::user_input::IdeOpenedFileSnapshot>,

    // ── Memory snapshots ──
    pub nested_memories: Vec<crate::generators::memory::NestedMemoryInfo>,
    pub relevant_memories: Vec<crate::generators::memory::RelevantMemoryInfo>,

    // ── Reminder-native silent attachments (Part 1) ──
    /// Paths the session has already loaded into model context. Engine
    /// scans `core/context::FileReadState` for unchanged-mtime entries
    /// referenced by this turn's @-mentions / memory prefetches.
    /// Non-empty → `AlreadyReadFileGenerator` emits a silent reminder.
    pub already_read_file_paths: Vec<PathBuf>,
    /// Image files whose mtime changed since last observation.
    /// Engine scans `core/context::FileReadState` for image-extension
    /// entries with mtime drift. Non-empty →
    /// `EditedImageFileGenerator` emits a silent reminder.
    pub edited_image_file_paths: Vec<PathBuf>,

    // ── Audit-add model-visible reminder (May 2026) ──
    /// Pre-formatted skill-discovery suggestions (UserPrompt tier) from
    /// `coco-skills`.
    pub skill_discovery: Option<SkillDiscoveryPayload>,
}

/// Build the [`GeneratorContext`] from `input`, run every applicable
/// generator in parallel, and return the reminders to inject.
///
/// Engine is expected to follow this call with [`crate::inject_reminders`]
/// to push the reminders into message history, then clear the one-shot
/// flags on `app_state` (`needs_plan_mode_exit_attachment`, etc.) for the
/// reminders that fired.
///
/// The helper itself is side-effect-free on `app_state`: it only **reads**
/// from the snapshot reference.
pub async fn run_turn_reminders(
    orchestrator: &SystemReminderOrchestrator,
    input: TurnReminderInput<'_>,
) -> Vec<SystemReminder> {
    let TurnReminderInput {
        config,
        turn_number,
        agent_id,
        user_input,
        last_human_turn_uuid,
        plan_file_path,
        plan_exists,
        plan_workflow,
        phase4_variant,
        explore_agent_count,
        plan_agent_count,
        explore_plan_agents_available,
        is_plan_interview_phase,
        plan_mode_feature_enabled,
        app_state,
        fallback_permission_mode,
        is_auto_classifier_active,
        tools,
        deferred_tools,
        is_task_v2_enabled,
        history,
        todo_key,
        is_auto_compact_enabled,
        context_window,
        effective_context_window,
        used_tokens,
        new_date,
        current_date,
        coordinator_worker_context,
        has_pending_plan_verification,
        total_cost_usd,
        max_budget_usd,
        output_tokens_turn,
        output_tokens_session,
        output_token_budget,
        companion_name,
        companion_species,
        has_prior_companion_intro,
        deferred_tools_delta,
        agent_listing_delta,
        mcp_instructions_delta,
        hook_events,
        diagnostics,
        output_style,
        queued_commands,
        task_statuses,
        skill_listing,
        invoked_skills,
        teammate_mailbox,
        team_context,
        agent_pending_messages,
        at_mentioned_files,
        mcp_resources,
        agent_mentions,
        ide_selection,
        ide_opened_file,
        nested_memories,
        relevant_memories,
        already_read_file_paths,
        edited_image_file_paths,
        skill_discovery,
    } = input;

    let is_sub_agent = agent_id.is_some();
    let has_user_input = user_input.is_some();

    // Pre-scan history for turn counters. These are typed over
    // [`ToolName`] — no hardcoded tool-name strings anywhere in the engine
    // integration path.
    let messages = history.as_slice();
    let turns_since_last_todo_write =
        count_assistant_turns_since_tool(messages, ToolName::TodoWrite);
    let turns_since_last_task_tool =
        count_assistant_turns_since_any_tool(messages, TASK_MANAGEMENT_TOOLS);
    // Verify-plan cadence counts human turns since the `plan_mode_exit`
    // attachment.
    let turns_since_plan_exit =
        count_human_turns_since_attachment(messages, coco_types::AttachmentKind::PlanModeExit);

    let turns_since_last_todo_reminder =
        count_assistant_turns_since_attachment(messages, AttachmentKind::TodoReminder);
    let turns_since_last_task_reminder =
        count_assistant_turns_since_attachment(messages, AttachmentKind::TaskReminder);

    // Plan-mode / auto-mode steady-state cadence is now history-derived
    // (replacing the in-memory throttle): human turns since the last
    // attachment (`None` = first turn in this mode → always emit), plus the
    // attachment count since the last exit for the Full-vs-Sparse cycle.
    let plan_mode_turns_since_attachment =
        human_turns_since_attachment_opt(messages, AttachmentKind::PlanMode);
    let plan_mode_attachments_since_exit = count_attachments_since_attachment(
        messages,
        AttachmentKind::PlanMode,
        AttachmentKind::PlanModeExit,
    );
    let auto_mode_turns_since_attachment =
        human_turns_since_attachment_opt(messages, AttachmentKind::AutoMode);
    let auto_mode_attachments_since_exit = count_attachments_since_attachment(
        messages,
        AttachmentKind::AutoMode,
        AttachmentKind::AutoModeExit,
    );

    let builder = GeneratorContext::builder(config)
        .turn_number(turn_number)
        .is_main_agent(!is_sub_agent)
        .has_user_input(has_user_input)
        .plan_mode_feature_enabled(plan_mode_feature_enabled)
        .is_plan_interview_phase(is_plan_interview_phase)
        .plan_file_path(plan_file_path)
        .plan_exists(plan_exists)
        .agent_id(agent_id)
        .is_sub_agent(is_sub_agent)
        .plan_workflow(plan_workflow)
        .phase4_variant(phase4_variant)
        .agent_counts(explore_agent_count, plan_agent_count)
        .explore_plan_agents_available(explore_plan_agents_available)
        .last_human_turn_uuid(last_human_turn_uuid)
        .user_input(user_input)
        .tools(tools)
        .deferred_tools(deferred_tools)
        .is_task_v2_enabled(is_task_v2_enabled)
        .turns_since_last_todo_write(turns_since_last_todo_write)
        .turns_since_last_todo_reminder(turns_since_last_todo_reminder)
        .turns_since_last_task_tool(turns_since_last_task_tool)
        .turns_since_last_task_reminder(turns_since_last_task_reminder)
        .is_auto_compact_enabled(is_auto_compact_enabled)
        .context_window(context_window)
        .effective_context_window(effective_context_window)
        .used_tokens(used_tokens)
        .new_date(new_date)
        .current_date(current_date)
        .coordinator_worker_context(coordinator_worker_context)
        .has_pending_plan_verification(has_pending_plan_verification)
        .turns_since_plan_exit(turns_since_plan_exit)
        .plan_mode_turns_since_attachment(plan_mode_turns_since_attachment)
        .plan_mode_attachments_since_exit(plan_mode_attachments_since_exit)
        .auto_mode_turns_since_attachment(auto_mode_turns_since_attachment)
        .auto_mode_attachments_since_exit(auto_mode_attachments_since_exit)
        .total_cost_usd(total_cost_usd)
        .max_budget_usd(max_budget_usd)
        .output_tokens_turn(output_tokens_turn)
        .output_tokens_session(output_tokens_session)
        .output_token_budget(output_token_budget)
        .companion(companion_name, companion_species)
        .has_prior_companion_intro(has_prior_companion_intro)
        .deferred_tools_delta(deferred_tools_delta)
        .agent_listing_delta(agent_listing_delta)
        .mcp_instructions_delta(mcp_instructions_delta)
        .hook_events(hook_events)
        .diagnostics(diagnostics)
        .output_style(output_style)
        .queued_commands(queued_commands)
        .task_statuses(task_statuses)
        .skill_listing(skill_listing)
        .invoked_skills(invoked_skills)
        .teammate_mailbox(teammate_mailbox)
        .team_context(team_context)
        .agent_pending_messages(agent_pending_messages)
        .at_mentioned_files(at_mentioned_files)
        .mcp_resources(mcp_resources)
        .agent_mentions(agent_mentions)
        .ide_selection(ide_selection)
        .ide_opened_file(ide_opened_file)
        .nested_memories(nested_memories)
        .relevant_memories(relevant_memories)
        .already_read_file_paths(already_read_file_paths)
        .edited_image_file_paths(edited_image_file_paths)
        .skill_discovery(skill_discovery);

    let builder = apply_app_state(
        builder,
        app_state,
        fallback_permission_mode,
        is_auto_classifier_active,
    );
    let builder = apply_todos_for_key(builder, app_state, &todo_key);
    let ctx = builder.build();

    orchestrator.generate_all(ctx).await
}

/// Gap in turns since `at` was last generated, or a large sentinel (2×
/// `turn_number` or `i32::MAX` when the throttle has never fired) that is
/// guaranteed to exceed any per-generator threshold.
#[cfg(test)]
#[path = "turn_runner.test.rs"]
mod tests;
