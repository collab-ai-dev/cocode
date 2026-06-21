//! Generator trait and per-turn context.
//!
//! [`AttachmentGenerator`] is implemented once per reminder type. The
//! [`SystemReminderOrchestrator`](crate::orchestrator::SystemReminderOrchestrator)
//! runs every applicable generator in parallel each turn.
//!
//! [`GeneratorContext`] packages the runtime state a generator needs to
//! decide whether (and what) to emit. It grows **lazily** — Phase A keeps it
//! minimal (scalars a Phase B plan-mode migration requires). Phase C extends
//! the struct when new generators need new inputs.
//!
//! Design notes:
//! - `has_user_input` (bool): true when the user submitted input this turn.
//! - `is_main_agent`: true for the main thread, false for subagents.
//! - `last_human_turn_uuid` is coco-rs specific: the plan-mode reminder
//!   counts *human* turns (non-meta user messages), not LLM iterations, to
//!   handle multi-tool-round turns correctly. See
//!   `app/query/plan_mode_reminder.rs:384` for the precedent.

use std::fmt::Debug;
use std::path::PathBuf;

use async_trait::async_trait;
use coco_context::Phase4Variant;
use coco_context::PlanWorkflow;
use coco_types::SkillDiscoveryPayload;
use coco_types::TaskRecord;
use coco_types::TodoRecord;
use uuid::Uuid;

use crate::error::Result;
use crate::types::AttachmentType;
use crate::types::ReminderTier;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// One generator per reminder type. Impls live under `generators/` (added in
/// Phase B+).
///
/// Lifecycle hooks the orchestrator calls each turn:
///
/// 1. [`is_enabled`](Self::is_enabled) — config gate.
/// 2. [`tier`](Self::tier) — skip if subagent and tier is `MainAgentOnly`, etc.
/// 3. [`generate`](Self::generate) — produce `Some(SystemReminder)` or `None`.
///
/// Cadence is no longer an orchestrator concern: generators that throttle
/// (plan-mode / auto-mode steady-state, todo / task / verify nudges) derive
/// their own gate from history-scan counters pre-computed on
/// [`GeneratorContext`], so returning `Ok(None)` simply skips this turn.
#[async_trait]
pub trait AttachmentGenerator: Send + Sync + Debug {
    /// Stable identifier (snake_case or PascalCase — used for tracing only).
    fn name(&self) -> &str;

    /// The reminder type this generator owns. Must be a 1:1 mapping.
    fn attachment_type(&self) -> AttachmentType;

    /// Tier. Default delegates to [`AttachmentType::tier`]; override only when
    /// a generator wants a different visibility than the type implies.
    fn tier(&self) -> ReminderTier {
        self.attachment_type().tier()
    }

    /// Config gate. Return false to disable this generator for the session.
    fn is_enabled(&self, config: &SystemReminderConfig) -> bool;

    /// Produce the reminder for this turn (or `None`).
    ///
    /// Errors bubble up to the orchestrator, which logs them and continues —
    /// one generator's failure never poisons another's output.
    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>>;
}

/// Runtime state packaged for every `generate()` call.
///
/// Construct via [`GeneratorContext::builder`]. Adding a new field is backward-
/// compatible — the builder exposes a setter and defaults the scalar.
#[derive(Debug, Clone)]
pub struct GeneratorContext<'a> {
    /// Master config reference (so generators can consult per-reminder
    /// settings without being passed a separate `&SystemReminderConfig`).
    pub config: &'a SystemReminderConfig,

    // ── Turn identity ──
    /// Current turn number (incremented by the engine on each LLM iteration).
    pub turn_number: i32,

    /// True when this is the main thread (`agent_id` is `None`).
    pub is_main_agent: bool,

    /// True when the user submitted input this turn.
    pub has_user_input: bool,

    // ── Permission-mode flags ──
    /// True when the engine is in plan mode this turn.
    pub is_plan_mode: bool,

    /// Whether the `plan_mode` feature is enabled this session (default on).
    /// When `false` the three plan-mode generators short-circuit so an
    /// opted-out user never receives a plan-mode `<system-reminder>`, even
    /// if a resumed transcript still carries `permission_mode == Plan`.
    pub plan_mode_feature_enabled: bool,

    /// True on the first plan-mode turn after a prior exit in this session.
    /// Drives the `plan_mode_reentry` variant.
    ///
    /// Source of truth: `ToolAppState::has_exited_plan_mode`, set by
    /// `ExitPlanModeTool` and cleared after Reentry emits. Engine forwards
    /// to ctx each turn.
    pub is_plan_reentry: bool,

    /// True when the plan workflow is the Interview variant instead of the
    /// 5-phase default (coco-rs `settings.plan_mode.workflow = interview`).
    pub is_plan_interview_phase: bool,

    /// One-shot: set by `ExitPlanModeTool` on success (and by the engine
    /// when it detects an unannounced Plan→non-Plan transition). Drives
    /// the `PlanModeExit` reminder. Engine forwards + clears post-emit.
    pub needs_plan_mode_exit_attachment: bool,

    /// Outcome paired with a tool-driven plan-mode exit. `None` is used for
    /// inferred transitions where the engine cannot know the exit type.
    pub pending_plan_mode_exit_outcome: Option<coco_types::ExitPlanModeOutcome>,

    /// One-shot: set by the engine when leaving Auto mode. Drives the
    /// `AutoModeExit` reminder.
    pub needs_auto_mode_exit_attachment: bool,

    // ── Optional per-turn metadata ──
    /// Path to the plan file for this session, if resolvable.
    pub plan_file_path: Option<PathBuf>,

    /// True when the plan file at `plan_file_path` exists on disk.
    pub plan_exists: bool,

    /// Agent identifier (`None` = main thread; `Some(id)` = subagent).
    pub agent_id: Option<String>,

    /// True when running as a sub-agent. Drives the simplified sub-agent
    /// plan-mode prompt.
    pub is_sub_agent: bool,

    /// Plan-mode 5-phase workflow variant. Ignored for Sparse / Reentry /
    /// sub-agent which render workflow-independent text.
    pub plan_workflow: PlanWorkflow,

    /// Plan-mode Phase-4 variant. Only affects Full 5-phase rendering.
    pub phase4_variant: Phase4Variant,

    /// Number of parallel Explore agents referenced in the 5-phase Full prompt.
    /// Range [1, 10]; clamped when building the attachment.
    pub explore_agent_count: i32,

    /// Number of parallel Plan agents referenced in the 5-phase Full prompt.
    /// Range [1, 10]; clamped when building the attachment.
    pub plan_agent_count: i32,

    /// True when Agent plus the built-in Explore/Plan definitions are visible.
    pub explore_plan_agents_available: bool,

    /// UUID of the most recent non-meta user message in history.
    ///
    /// coco-rs plan-mode specific: the throttle counter advances only when
    /// this UUID differs from the previously-stamped one, so multi-tool-
    /// round iterations within a single human turn count as one turn.
    pub last_human_turn_uuid: Option<Uuid>,

    /// User's raw prompt text this turn (if any). Used by
    /// `at_mentioned_files`, `agent_mentions`, etc. in Phase C.
    pub user_input: Option<String>,

    // ── Phase C (todo / task / critical reminders) ──
    /// Snake / PascalCase names of tools available to the agent this turn.
    /// Generators use this to gate reminders on tool presence (e.g.
    /// todo reminders check for `TodoWrite` / `Brief`).
    pub tools: Vec<String>,

    /// Names that are searchable through ToolSearch but not callable until
    /// loaded. Sorted by canonical tool name.
    pub deferred_tools: Vec<String>,

    /// Assistant turns since the agent last called `TodoWrite`, counting
    /// from the end of history. Engine pre-computes by scanning message
    /// history.
    pub turns_since_last_todo_write: i32,

    /// Assistant turns since the last `todo_reminder` attachment was
    /// emitted. Engine pre-computes the same way.
    pub turns_since_last_todo_reminder: i32,

    /// Assistant turns since the V2 task tools (`TaskCreate`, `TaskUpdate`
    /// etc.) were last invoked.
    pub turns_since_last_task_tool: i32,

    /// Assistant turns since the last `task_reminder` attachment.
    pub turns_since_last_task_reminder: i32,

    /// Snapshot of the agent's V1 TodoWrite list, already scoped to this
    /// agent (engine resolves `app_state.todos_by_agent[agent_or_session_key]`).
    pub todos: Vec<TodoRecord>,

    /// Snapshot of the V2 plan-task list for this session.
    pub plan_tasks: Vec<TaskRecord>,

    /// True when the V2 task-list feature is active.
    pub is_task_v2_enabled: bool,

    // ── Phase C.2 (auto-mode / compaction / date-change) ──
    /// True when the engine is in auto mode, or in plan-mode with the
    /// auto-mode classifier actively driving.
    pub is_auto_mode: bool,

    /// True when auto-compaction is enabled.
    pub is_auto_compact_enabled: bool,

    /// Total context window for the active model, in tokens.
    pub context_window: i64,

    /// Effective context window after reserving for output / thinking.
    /// The compaction reminder compares usage against 25% of this.
    pub effective_context_window: i64,

    /// Tokens currently in context (includes estimation).
    pub used_tokens: i64,

    /// `Some(date)` on the turn the local date changes relative to the
    /// last-emitted date; `None` otherwise. Engine pre-computes by
    /// comparing today's local ISO date to its own "last emitted date" latch.
    pub new_date: Option<String>,

    /// `Some(iso_date)` = today's local ISO date, injected by the engine
    /// every turn so the [`UserContextGenerator`](crate::generators::UserContextGenerator)
    /// can emit a baseline `Today's date is <date>.` reminder. `None` (the
    /// default, e.g. in unit tests) suppresses the reminder.
    pub current_date: Option<String>,
    /// `Some(body)` = the coordinator-mode `workerToolsContext` block
    /// (worker tool pool + connected MCP servers), injected per-turn by
    /// the engine when coordinator mode is active so the leader knows
    /// which tools the workers it spawns will have. Rendered as a sibling
    /// `# workerToolsContext` key inside the same user-context reminder
    /// (TS `prependUserContext` one-message-multi-key shape). `None`
    /// outside coordinator mode.
    pub coordinator_worker_context: Option<String>,

    // ── Phase E (verify-plan reminder) ──
    /// True when the deprecated legacy verify-plan flow has recorded pending
    /// plan verification and a follow-up `VerifyPlanExecution` call is still
    /// outstanding. Set from TS-shaped
    /// `appState.pendingPlanVerification` by requiring pending exists and
    /// neither `verificationStarted` nor `verificationCompleted` is set.
    pub has_pending_plan_verification: bool,

    /// Human turns elapsed since the last `plan_mode_exit` attachment.
    pub turns_since_plan_exit: i32,

    // ── Phase 1 engine-local reminder inputs ──
    /// Total cost in USD accumulated for the current session. Used by
    /// `budget_usd`.
    pub total_cost_usd: f64,

    /// Optional per-session USD budget cap. `Some(n)` activates the
    /// `budget_usd` reminder; `None` suppresses it. Sourced from
    /// `QueryEngineConfig::max_budget_usd`.
    pub max_budget_usd: Option<f64>,

    /// Output tokens produced in the current turn — zero at turn start,
    /// rises as the LLM streams. Used by `output_token_usage`.
    pub output_tokens_turn: i64,

    /// Output tokens for the full session.
    pub output_tokens_session: i64,

    /// Optional per-turn output-token budget. `Some(n>0)` activates the
    /// `output_token_usage` reminder; `None` or `Some(0)` suppresses it.
    pub output_token_budget: Option<i64>,

    /// Configured companion name (e.g. `"Pebble"`). `Some` activates the
    /// `companion_intro` reminder once per session.
    pub companion_name: Option<String>,

    /// Configured companion species (e.g. `"rabbit"`). Must be set together
    /// with `companion_name`; both flow through `companionIntroText`.
    pub companion_species: Option<String>,

    /// True when a `companion_intro` reminder with the current companion
    /// name has already been emitted in this session. Engine precomputes
    /// by scanning history.
    pub has_prior_companion_intro: bool,

    // ── Phase 2 history-diff delta snapshots ──
    /// Pre-computed deferred-tools delta. `Some(info)` with either
    /// `added_lines` or `removed_names` non-empty triggers emission.
    /// Engine scans history for prior `DeferredToolsDelta` attachments
    /// and diffs current `tools` against the accumulated announced set.
    pub deferred_tools_delta: Option<DeferredToolsDeltaInfo>,

    /// Pre-computed agent-listing delta. Engine scans for prior
    /// `AgentListingDelta` attachments in history and diffs current
    /// `agent_definitions` against them.
    pub agent_listing_delta: Option<AgentListingDeltaInfo>,

    /// Pre-computed MCP instructions delta. Engine scans for prior
    /// `McpInstructionsDelta` attachments and diffs current server
    /// instructions (from `services/mcp`) against them.
    pub mcp_instructions_delta: Option<McpInstructionsDeltaInfo>,

    // ── Phase 3 cross-crate snapshots ──
    /// Hook events collected from the async hook registry this turn.
    /// Empty vec suppresses all 5 hook generators.
    pub hook_events: Vec<HookEvent>,

    /// LSP / IDE diagnostic summary for files changed this turn. Empty
    /// vec suppresses the diagnostics reminder.
    pub diagnostics: Vec<DiagnosticFileSummary>,

    /// Active output-style snapshot. `None` suppresses the output-style
    /// reminder.
    pub output_style: Option<OutputStyleSnapshot>,

    /// Queued commands drained this turn. Empty vec suppresses the
    /// queued-command reminder.
    pub queued_commands: Vec<QueuedCommandInfo>,

    /// Background-task status updates to announce this turn. Empty vec
    /// suppresses the task-status reminder.
    pub task_statuses: Vec<TaskStatusSnapshot>,

    /// Skill-listing content (pre-formatted). `None` suppresses the
    /// skill_listing reminder.
    pub skill_listing: Option<String>,

    /// Skills invoked this session. Empty vec suppresses the
    /// invoked_skills reminder.
    pub invoked_skills: Vec<InvokedSkillEntry>,

    /// Teammate mailbox snapshot. `None` suppresses.
    pub teammate_mailbox: Option<TeammateMailboxInfo>,

    /// Team coordination context. `None` suppresses.
    pub team_context: Option<TeamContextSnapshot>,

    /// Agent inbox pending messages. Empty vec suppresses.
    pub agent_pending_messages: Vec<AgentPendingMessage>,

    // ── Phase 4 user-input-tier snapshots (UserPrompt tier) ──
    /// @-mentioned files in the user's prompt this turn.
    pub at_mentioned_files: Vec<crate::generators::user_input::MentionedFileEntry>,
    /// MCP resource references in the prompt.
    pub mcp_resources: Vec<crate::generators::user_input::McpResourceEntry>,
    /// Agent-type mentions in the prompt.
    pub agent_mentions: Vec<crate::generators::user_input::AgentMentionEntry>,

    // ── Main-thread IDE snapshots ──
    /// IDE selection snapshot for this turn.
    pub ide_selection: Option<crate::generators::user_input::IdeSelectionSnapshot>,
    /// IDE opened-file snapshot for this turn.
    pub ide_opened_file: Option<crate::generators::user_input::IdeOpenedFileSnapshot>,

    // ── Memory snapshots ──
    /// Nested memory entries surfaced via @-mention traversal.
    pub nested_memories: Vec<crate::generators::memory::NestedMemoryInfo>,
    /// Relevant memory entries (semantically ranked, async prefetched).
    pub relevant_memories: Vec<crate::generators::memory::RelevantMemoryInfo>,

    // ── Reminder-native silent attachments (Part 1) ──
    /// Paths the session has already loaded into model context. Emitted
    /// by the [`AlreadyReadFileGenerator`](crate::generators::already_read_file::AlreadyReadFileGenerator)
    /// as a silent reminder carrying
    /// [`AlreadyReadFileMeta`](crate::types::AlreadyReadFileMeta) metadata
    /// so the UI can surface "already in context" hints. Non-empty →
    /// generator emits silent attachment.
    pub already_read_file_paths: Vec<PathBuf>,

    /// Image files whose mtime changed since the last observation.
    /// Emitted by [`EditedImageFileGenerator`](crate::generators::edited_image_file::EditedImageFileGenerator)
    /// as a silent reminder with
    /// [`EditedImageFileMeta`](crate::types::EditedImageFileMeta) payload.
    /// Text-diff is impossible for images; the UI may highlight the
    /// change. Non-empty → generator emits silent attachment.
    pub edited_image_file_paths: Vec<PathBuf>,

    // ── Audit-add (May 2026) — model-visible attachment.
    /// Skill-discovery suggestion from `coco-skills` (UserPrompt tier).
    pub skill_discovery: Option<SkillDiscoveryPayload>,

    // ── History-derived plan/auto cadence (filled by the engine turn-scan) ──
    /// Human turns since the last `plan_mode` attachment in history. `None`
    /// means no prior plan-mode attachment this segment (first plan turn →
    /// always emit); `Some(n)` emits only when `n` ≥ the plan-mode cadence.
    /// Replaces the old in-memory throttle's `last_generated_turn` — history
    /// is the source of truth, so cadence survives compaction.
    pub plan_mode_turns_since_attachment: Option<i32>,

    /// Count of `plan_mode` attachments since the last `plan_mode_exit`,
    /// driving the Full-vs-Sparse cycle (the Nth since exit is Full).
    pub plan_mode_attachments_since_exit: i32,

    /// Human turns since the last `auto_mode` attachment. `None` = first
    /// auto-mode turn (always emit). Mirrors the plan-mode pair above.
    pub auto_mode_turns_since_attachment: Option<i32>,

    /// Count of `auto_mode` attachments since the last `auto_mode_exit`
    /// (Full-vs-Sparse cycle).
    pub auto_mode_attachments_since_exit: i32,
}

impl<'a> GeneratorContext<'a> {
    /// Start a builder bound to a config reference.
    pub fn builder(config: &'a SystemReminderConfig) -> GeneratorContextBuilder<'a> {
        GeneratorContextBuilder::new(config)
    }
}

/// Builder for [`GeneratorContext`].
///
/// All per-turn scalars default to "zero / false / None" so tests can
/// construct a minimal context with `builder(&cfg).turn_number(5).build()`.
#[derive(Debug, Clone)]
pub struct GeneratorContextBuilder<'a> {
    config: &'a SystemReminderConfig,
    turn_number: i32,
    is_main_agent: bool,
    has_user_input: bool,
    is_plan_mode: bool,
    plan_mode_feature_enabled: bool,
    is_plan_reentry: bool,
    is_plan_interview_phase: bool,
    needs_plan_mode_exit_attachment: bool,
    pending_plan_mode_exit_outcome: Option<coco_types::ExitPlanModeOutcome>,
    needs_auto_mode_exit_attachment: bool,
    plan_file_path: Option<PathBuf>,
    plan_exists: bool,
    agent_id: Option<String>,
    is_sub_agent: bool,
    plan_workflow: PlanWorkflow,
    phase4_variant: Phase4Variant,
    explore_agent_count: i32,
    plan_agent_count: i32,
    explore_plan_agents_available: bool,
    last_human_turn_uuid: Option<Uuid>,
    user_input: Option<String>,
    tools: Vec<String>,
    deferred_tools: Vec<String>,
    turns_since_last_todo_write: i32,
    turns_since_last_todo_reminder: i32,
    turns_since_last_task_tool: i32,
    turns_since_last_task_reminder: i32,
    todos: Vec<TodoRecord>,
    plan_tasks: Vec<TaskRecord>,
    is_task_v2_enabled: bool,
    is_auto_mode: bool,
    is_auto_compact_enabled: bool,
    context_window: i64,
    effective_context_window: i64,
    used_tokens: i64,
    new_date: Option<String>,
    current_date: Option<String>,
    coordinator_worker_context: Option<String>,
    has_pending_plan_verification: bool,
    turns_since_plan_exit: i32,
    total_cost_usd: f64,
    max_budget_usd: Option<f64>,
    output_tokens_turn: i64,
    output_tokens_session: i64,
    output_token_budget: Option<i64>,
    companion_name: Option<String>,
    companion_species: Option<String>,
    has_prior_companion_intro: bool,
    deferred_tools_delta: Option<DeferredToolsDeltaInfo>,
    agent_listing_delta: Option<AgentListingDeltaInfo>,
    mcp_instructions_delta: Option<McpInstructionsDeltaInfo>,
    hook_events: Vec<HookEvent>,
    diagnostics: Vec<DiagnosticFileSummary>,
    output_style: Option<OutputStyleSnapshot>,
    queued_commands: Vec<QueuedCommandInfo>,
    task_statuses: Vec<TaskStatusSnapshot>,
    skill_listing: Option<String>,
    invoked_skills: Vec<InvokedSkillEntry>,
    teammate_mailbox: Option<TeammateMailboxInfo>,
    team_context: Option<TeamContextSnapshot>,
    agent_pending_messages: Vec<AgentPendingMessage>,
    at_mentioned_files: Vec<crate::generators::user_input::MentionedFileEntry>,
    mcp_resources: Vec<crate::generators::user_input::McpResourceEntry>,
    agent_mentions: Vec<crate::generators::user_input::AgentMentionEntry>,
    ide_selection: Option<crate::generators::user_input::IdeSelectionSnapshot>,
    ide_opened_file: Option<crate::generators::user_input::IdeOpenedFileSnapshot>,
    nested_memories: Vec<crate::generators::memory::NestedMemoryInfo>,
    relevant_memories: Vec<crate::generators::memory::RelevantMemoryInfo>,
    already_read_file_paths: Vec<PathBuf>,
    edited_image_file_paths: Vec<PathBuf>,
    skill_discovery: Option<SkillDiscoveryPayload>,
    plan_mode_turns_since_attachment: Option<i32>,
    plan_mode_attachments_since_exit: i32,
    auto_mode_turns_since_attachment: Option<i32>,
    auto_mode_attachments_since_exit: i32,
}

impl<'a> GeneratorContextBuilder<'a> {
    pub fn new(config: &'a SystemReminderConfig) -> Self {
        Self {
            config,
            turn_number: 0,
            is_main_agent: true,
            has_user_input: false,
            is_plan_mode: false,
            // Default on: tests + callers that don't set it keep plan reminders.
            plan_mode_feature_enabled: true,
            is_plan_reentry: false,
            is_plan_interview_phase: false,
            needs_plan_mode_exit_attachment: false,
            pending_plan_mode_exit_outcome: None,
            needs_auto_mode_exit_attachment: false,
            plan_file_path: None,
            plan_exists: false,
            agent_id: None,
            is_sub_agent: false,
            plan_workflow: PlanWorkflow::default(),
            phase4_variant: Phase4Variant::default(),
            explore_agent_count: DEFAULT_EXPLORE_AGENT_COUNT,
            plan_agent_count: DEFAULT_PLAN_AGENT_COUNT,
            explore_plan_agents_available: false,
            last_human_turn_uuid: None,
            user_input: None,
            tools: Vec::new(),
            deferred_tools: Vec::new(),
            turns_since_last_todo_write: 0,
            turns_since_last_todo_reminder: 0,
            turns_since_last_task_tool: 0,
            turns_since_last_task_reminder: 0,
            todos: Vec::new(),
            plan_tasks: Vec::new(),
            is_task_v2_enabled: false,
            is_auto_mode: false,
            is_auto_compact_enabled: false,
            context_window: 0,
            effective_context_window: 0,
            used_tokens: 0,
            new_date: None,
            current_date: None,
            coordinator_worker_context: None,
            has_pending_plan_verification: false,
            turns_since_plan_exit: 0,
            total_cost_usd: 0.0,
            max_budget_usd: None,
            output_tokens_turn: 0,
            output_tokens_session: 0,
            output_token_budget: None,
            companion_name: None,
            companion_species: None,
            has_prior_companion_intro: false,
            deferred_tools_delta: None,
            agent_listing_delta: None,
            mcp_instructions_delta: None,
            hook_events: Vec::new(),
            diagnostics: Vec::new(),
            output_style: None,
            queued_commands: Vec::new(),
            task_statuses: Vec::new(),
            skill_listing: None,
            invoked_skills: Vec::new(),
            teammate_mailbox: None,
            team_context: None,
            agent_pending_messages: Vec::new(),
            at_mentioned_files: Vec::new(),
            mcp_resources: Vec::new(),
            agent_mentions: Vec::new(),
            ide_selection: None,
            ide_opened_file: None,
            nested_memories: Vec::new(),
            relevant_memories: Vec::new(),
            already_read_file_paths: Vec::new(),
            edited_image_file_paths: Vec::new(),
            skill_discovery: None,
            plan_mode_turns_since_attachment: None,
            plan_mode_attachments_since_exit: 0,
            auto_mode_turns_since_attachment: None,
            auto_mode_attachments_since_exit: 0,
        }
    }

    pub fn turn_number(mut self, n: i32) -> Self {
        self.turn_number = n;
        self
    }

    pub fn is_main_agent(mut self, b: bool) -> Self {
        self.is_main_agent = b;
        self
    }

    pub fn has_user_input(mut self, b: bool) -> Self {
        self.has_user_input = b;
        self
    }

    pub fn is_plan_mode(mut self, b: bool) -> Self {
        self.is_plan_mode = b;
        self
    }

    pub fn plan_mode_feature_enabled(mut self, b: bool) -> Self {
        self.plan_mode_feature_enabled = b;
        self
    }

    pub fn is_plan_reentry(mut self, b: bool) -> Self {
        self.is_plan_reentry = b;
        self
    }

    pub fn is_plan_interview_phase(mut self, b: bool) -> Self {
        self.is_plan_interview_phase = b;
        self
    }

    pub fn needs_plan_mode_exit_attachment(mut self, b: bool) -> Self {
        self.needs_plan_mode_exit_attachment = b;
        self
    }

    pub fn pending_plan_mode_exit_outcome(
        mut self,
        outcome: Option<coco_types::ExitPlanModeOutcome>,
    ) -> Self {
        self.pending_plan_mode_exit_outcome = outcome;
        self
    }

    pub fn needs_auto_mode_exit_attachment(mut self, b: bool) -> Self {
        self.needs_auto_mode_exit_attachment = b;
        self
    }

    pub fn plan_file_path(mut self, p: Option<PathBuf>) -> Self {
        self.plan_file_path = p;
        self
    }

    pub fn plan_exists(mut self, b: bool) -> Self {
        self.plan_exists = b;
        self
    }

    pub fn agent_id(mut self, id: Option<String>) -> Self {
        self.agent_id = id;
        self
    }

    pub fn is_sub_agent(mut self, b: bool) -> Self {
        self.is_sub_agent = b;
        self
    }

    pub fn plan_workflow(mut self, w: PlanWorkflow) -> Self {
        self.plan_workflow = w;
        self
    }

    pub fn phase4_variant(mut self, v: Phase4Variant) -> Self {
        self.phase4_variant = v;
        self
    }

    /// Set explore + plan agent counts. Values are clamped at build time.
    pub fn agent_counts(mut self, explore: i32, plan: i32) -> Self {
        self.explore_agent_count = explore;
        self.plan_agent_count = plan;
        self
    }

    pub fn explore_plan_agents_available(mut self, available: bool) -> Self {
        self.explore_plan_agents_available = available;
        self
    }

    pub fn last_human_turn_uuid(mut self, id: Option<Uuid>) -> Self {
        self.last_human_turn_uuid = id;
        self
    }

    pub fn user_input(mut self, text: Option<String>) -> Self {
        self.user_input = text;
        self
    }

    pub fn tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    pub fn deferred_tools(mut self, tools: Vec<String>) -> Self {
        self.deferred_tools = tools;
        self
    }

    pub fn turns_since_last_todo_write(mut self, n: i32) -> Self {
        self.turns_since_last_todo_write = n;
        self
    }

    pub fn turns_since_last_todo_reminder(mut self, n: i32) -> Self {
        self.turns_since_last_todo_reminder = n;
        self
    }

    pub fn turns_since_last_task_tool(mut self, n: i32) -> Self {
        self.turns_since_last_task_tool = n;
        self
    }

    pub fn turns_since_last_task_reminder(mut self, n: i32) -> Self {
        self.turns_since_last_task_reminder = n;
        self
    }

    pub fn todos(mut self, todos: Vec<TodoRecord>) -> Self {
        self.todos = todos;
        self
    }

    pub fn plan_tasks(mut self, tasks: Vec<TaskRecord>) -> Self {
        self.plan_tasks = tasks;
        self
    }

    pub fn is_task_v2_enabled(mut self, b: bool) -> Self {
        self.is_task_v2_enabled = b;
        self
    }

    pub fn is_auto_mode(mut self, b: bool) -> Self {
        self.is_auto_mode = b;
        self
    }

    pub fn is_auto_compact_enabled(mut self, b: bool) -> Self {
        self.is_auto_compact_enabled = b;
        self
    }

    pub fn context_window(mut self, n: i64) -> Self {
        self.context_window = n;
        self
    }

    pub fn effective_context_window(mut self, n: i64) -> Self {
        self.effective_context_window = n;
        self
    }

    pub fn used_tokens(mut self, n: i64) -> Self {
        self.used_tokens = n;
        self
    }

    pub fn new_date(mut self, d: Option<String>) -> Self {
        self.new_date = d;
        self
    }

    pub fn current_date(mut self, d: Option<String>) -> Self {
        self.current_date = d;
        self
    }

    pub fn coordinator_worker_context(mut self, c: Option<String>) -> Self {
        self.coordinator_worker_context = c;
        self
    }

    pub fn has_pending_plan_verification(mut self, b: bool) -> Self {
        self.has_pending_plan_verification = b;
        self
    }

    pub fn turns_since_plan_exit(mut self, n: i32) -> Self {
        self.turns_since_plan_exit = n;
        self
    }

    pub fn total_cost_usd(mut self, n: f64) -> Self {
        self.total_cost_usd = n;
        self
    }

    pub fn max_budget_usd(mut self, n: Option<f64>) -> Self {
        self.max_budget_usd = n;
        self
    }

    pub fn output_tokens_turn(mut self, n: i64) -> Self {
        self.output_tokens_turn = n;
        self
    }

    pub fn output_tokens_session(mut self, n: i64) -> Self {
        self.output_tokens_session = n;
        self
    }

    pub fn output_token_budget(mut self, n: Option<i64>) -> Self {
        self.output_token_budget = n;
        self
    }

    pub fn companion(mut self, name: Option<String>, species: Option<String>) -> Self {
        self.companion_name = name;
        self.companion_species = species;
        self
    }

    pub fn has_prior_companion_intro(mut self, b: bool) -> Self {
        self.has_prior_companion_intro = b;
        self
    }

    pub fn deferred_tools_delta(mut self, info: Option<DeferredToolsDeltaInfo>) -> Self {
        self.deferred_tools_delta = info;
        self
    }

    pub fn agent_listing_delta(mut self, info: Option<AgentListingDeltaInfo>) -> Self {
        self.agent_listing_delta = info;
        self
    }

    pub fn mcp_instructions_delta(mut self, info: Option<McpInstructionsDeltaInfo>) -> Self {
        self.mcp_instructions_delta = info;
        self
    }

    pub fn hook_events(mut self, events: Vec<HookEvent>) -> Self {
        self.hook_events = events;
        self
    }

    pub fn diagnostics(mut self, d: Vec<DiagnosticFileSummary>) -> Self {
        self.diagnostics = d;
        self
    }

    pub fn output_style(mut self, s: Option<OutputStyleSnapshot>) -> Self {
        self.output_style = s;
        self
    }

    pub fn queued_commands(mut self, q: Vec<QueuedCommandInfo>) -> Self {
        self.queued_commands = q;
        self
    }

    pub fn task_statuses(mut self, t: Vec<TaskStatusSnapshot>) -> Self {
        self.task_statuses = t;
        self
    }

    pub fn skill_listing(mut self, s: Option<String>) -> Self {
        self.skill_listing = s;
        self
    }

    pub fn invoked_skills(mut self, s: Vec<InvokedSkillEntry>) -> Self {
        self.invoked_skills = s;
        self
    }

    pub fn teammate_mailbox(mut self, m: Option<TeammateMailboxInfo>) -> Self {
        self.teammate_mailbox = m;
        self
    }

    pub fn team_context(mut self, c: Option<TeamContextSnapshot>) -> Self {
        self.team_context = c;
        self
    }

    pub fn agent_pending_messages(mut self, m: Vec<AgentPendingMessage>) -> Self {
        self.agent_pending_messages = m;
        self
    }

    pub fn at_mentioned_files(
        mut self,
        files: Vec<crate::generators::user_input::MentionedFileEntry>,
    ) -> Self {
        self.at_mentioned_files = files;
        self
    }

    pub fn mcp_resources(
        mut self,
        res: Vec<crate::generators::user_input::McpResourceEntry>,
    ) -> Self {
        self.mcp_resources = res;
        self
    }

    pub fn agent_mentions(
        mut self,
        m: Vec<crate::generators::user_input::AgentMentionEntry>,
    ) -> Self {
        self.agent_mentions = m;
        self
    }

    pub fn ide_selection(
        mut self,
        s: Option<crate::generators::user_input::IdeSelectionSnapshot>,
    ) -> Self {
        self.ide_selection = s;
        self
    }

    pub fn ide_opened_file(
        mut self,
        f: Option<crate::generators::user_input::IdeOpenedFileSnapshot>,
    ) -> Self {
        self.ide_opened_file = f;
        self
    }

    pub fn nested_memories(mut self, v: Vec<crate::generators::memory::NestedMemoryInfo>) -> Self {
        self.nested_memories = v;
        self
    }

    pub fn relevant_memories(
        mut self,
        v: Vec<crate::generators::memory::RelevantMemoryInfo>,
    ) -> Self {
        self.relevant_memories = v;
        self
    }

    pub fn already_read_file_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.already_read_file_paths = paths;
        self
    }

    pub fn edited_image_file_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.edited_image_file_paths = paths;
        self
    }

    pub fn skill_discovery(mut self, payload: Option<SkillDiscoveryPayload>) -> Self {
        self.skill_discovery = payload;
        self
    }

    pub fn plan_mode_turns_since_attachment(mut self, n: Option<i32>) -> Self {
        self.plan_mode_turns_since_attachment = n;
        self
    }

    pub fn plan_mode_attachments_since_exit(mut self, n: i32) -> Self {
        self.plan_mode_attachments_since_exit = n;
        self
    }

    pub fn auto_mode_turns_since_attachment(mut self, n: Option<i32>) -> Self {
        self.auto_mode_turns_since_attachment = n;
        self
    }

    pub fn auto_mode_attachments_since_exit(mut self, n: i32) -> Self {
        self.auto_mode_attachments_since_exit = n;
        self
    }

    pub fn build(self) -> GeneratorContext<'a> {
        GeneratorContext {
            config: self.config,
            turn_number: self.turn_number,
            is_main_agent: self.is_main_agent,
            has_user_input: self.has_user_input,
            is_plan_mode: self.is_plan_mode,
            plan_mode_feature_enabled: self.plan_mode_feature_enabled,
            is_plan_reentry: self.is_plan_reentry,
            is_plan_interview_phase: self.is_plan_interview_phase,
            needs_plan_mode_exit_attachment: self.needs_plan_mode_exit_attachment,
            pending_plan_mode_exit_outcome: self.pending_plan_mode_exit_outcome,
            needs_auto_mode_exit_attachment: self.needs_auto_mode_exit_attachment,
            plan_file_path: self.plan_file_path,
            plan_exists: self.plan_exists,
            agent_id: self.agent_id,
            is_sub_agent: self.is_sub_agent,
            plan_workflow: self.plan_workflow,
            phase4_variant: self.phase4_variant,
            explore_agent_count: clamp_agents(self.explore_agent_count),
            plan_agent_count: clamp_agents(self.plan_agent_count),
            explore_plan_agents_available: self.explore_plan_agents_available,
            last_human_turn_uuid: self.last_human_turn_uuid,
            user_input: self.user_input,
            tools: self.tools,
            deferred_tools: self.deferred_tools,
            turns_since_last_todo_write: self.turns_since_last_todo_write,
            turns_since_last_todo_reminder: self.turns_since_last_todo_reminder,
            turns_since_last_task_tool: self.turns_since_last_task_tool,
            turns_since_last_task_reminder: self.turns_since_last_task_reminder,
            todos: self.todos,
            plan_tasks: self.plan_tasks,
            is_task_v2_enabled: self.is_task_v2_enabled,
            is_auto_mode: self.is_auto_mode,
            is_auto_compact_enabled: self.is_auto_compact_enabled,
            context_window: self.context_window,
            effective_context_window: self.effective_context_window,
            used_tokens: self.used_tokens,
            new_date: self.new_date,
            current_date: self.current_date,
            coordinator_worker_context: self.coordinator_worker_context,
            has_pending_plan_verification: self.has_pending_plan_verification,
            turns_since_plan_exit: self.turns_since_plan_exit,
            total_cost_usd: self.total_cost_usd,
            max_budget_usd: self.max_budget_usd,
            output_tokens_turn: self.output_tokens_turn,
            output_tokens_session: self.output_tokens_session,
            output_token_budget: self.output_token_budget,
            companion_name: self.companion_name,
            companion_species: self.companion_species,
            has_prior_companion_intro: self.has_prior_companion_intro,
            deferred_tools_delta: self.deferred_tools_delta,
            agent_listing_delta: self.agent_listing_delta,
            mcp_instructions_delta: self.mcp_instructions_delta,
            hook_events: self.hook_events,
            diagnostics: self.diagnostics,
            output_style: self.output_style,
            queued_commands: self.queued_commands,
            task_statuses: self.task_statuses,
            skill_listing: self.skill_listing,
            invoked_skills: self.invoked_skills,
            teammate_mailbox: self.teammate_mailbox,
            team_context: self.team_context,
            agent_pending_messages: self.agent_pending_messages,
            at_mentioned_files: self.at_mentioned_files,
            mcp_resources: self.mcp_resources,
            agent_mentions: self.agent_mentions,
            ide_selection: self.ide_selection,
            ide_opened_file: self.ide_opened_file,
            nested_memories: self.nested_memories,
            relevant_memories: self.relevant_memories,
            already_read_file_paths: self.already_read_file_paths,
            edited_image_file_paths: self.edited_image_file_paths,
            skill_discovery: self.skill_discovery,
            plan_mode_turns_since_attachment: self.plan_mode_turns_since_attachment,
            plan_mode_attachments_since_exit: self.plan_mode_attachments_since_exit,
            auto_mode_turns_since_attachment: self.auto_mode_turns_since_attachment,
            auto_mode_attachments_since_exit: self.auto_mode_attachments_since_exit,
        }
    }
}

/// Deferred-tools delta snapshot. `added_lines` entries are
/// `"- ToolName: description"` ready for direct newline-join rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeferredToolsDeltaInfo {
    pub added_lines: Vec<String>,
    pub removed_names: Vec<String>,
}

impl DeferredToolsDeltaInfo {
    pub fn is_empty(&self) -> bool {
        self.added_lines.is_empty() && self.removed_names.is_empty()
    }
}

/// Agent-listing delta snapshot. `is_initial` flips the added-header text;
/// the optional concurrency note is appended when `show_concurrency_note`
/// is true.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentListingDeltaInfo {
    pub added_lines: Vec<String>,
    pub removed_types: Vec<String>,
    pub is_initial: bool,
    pub show_concurrency_note: bool,
}

impl AgentListingDeltaInfo {
    pub fn is_empty(&self) -> bool {
        self.added_lines.is_empty() && self.removed_types.is_empty()
    }
}

/// MCP instructions delta snapshot. `added_blocks` entries are full
/// pre-formatted server blocks ready for `\n\n`-join rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpInstructionsDeltaInfo {
    pub added_blocks: Vec<String>,
    pub removed_names: Vec<String>,
}

impl McpInstructionsDeltaInfo {
    pub fn is_empty(&self) -> bool {
        self.added_blocks.is_empty() && self.removed_names.is_empty()
    }
}

// ── Phase 3 cross-crate snapshot types ──

/// One hook event to be surfaced as a reminder this turn. Engine
/// pre-computes a `Vec<HookEvent>` by draining the async hook registry;
/// the 5 hook generators filter the vec for their variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// `hook_success`. Only SessionStart / UserPromptSubmit with
    /// non-empty content produce text.
    Success {
        hook_name: String,
        hook_event: HookEventKind,
        content: String,
    },
    /// `hook_blocking_error`.
    BlockingError {
        hook_name: String,
        command: String,
        error: String,
    },
    /// `hook_additional_context` — content is a list of lines
    /// joined by `\n` in the output text.
    AdditionalContext {
        hook_name: String,
        content: Vec<String>,
    },
    /// `hook_stopped_continuation`.
    StoppedContinuation { hook_name: String, message: String },
    /// `async_hook_response` — produces up to two messages (system
    /// + additionalContext) inside one `<system-reminder>`.
    AsyncResponse {
        system_message: Option<String>,
        additional_context: Option<String>,
    },
}

/// Hook-event kind gate for `hook_success` — only emits the
/// reminder for SessionStart and UserPromptSubmit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEventKind {
    SessionStart,
    UserPromptSubmit,
    Other,
}

/// Single diagnostic file entry. `formatted` is the pre-rendered per-file
/// diagnostic block produced by `DiagnosticTrackingService.formatDiagnosticsSummary`.
/// Engine owns the formatting since LSP/IDE providers differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticFileSummary {
    pub path: String,
    pub formatted: String,
}

/// Output-style reinforcement snapshot. Contains the resolved display
/// name for the active output style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputStyleSnapshot {
    pub name: String,
}

/// Queued-command replay snapshot. Drained queue items are re-injected
/// as `queued_command` attachments wrapped via `wrapCommandText` — each
/// origin variant gets its own framing sentence so the model knows how
/// urgently / trustingly to treat the interruption. When the queued item
/// carried images (mid-turn paste of a screenshot, …), they are appended
/// after the wrapped text via a `ReminderMessage` with text + image
/// content blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueuedCommandInfo {
    pub content: String,
    /// Origin tag carried over from the queue. `None` is treated as
    /// `Human` by [`crate::wrap_command_text`].
    pub origin: Option<crate::queue_origin::QueueOrigin>,
    /// Image attachments paired with the queued text. Empty for text-only
    /// queued commands, which is the common case.
    pub images: Vec<QueuedCommandImage>,
}

/// One image paired with a queued command. Wire-shape mirrors
/// `coco_context::ImageAttachment` (media_type + base64) but lives here
/// so the system-reminder crate stays self-contained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueuedCommandImage {
    /// IANA media type (e.g. `image/png`).
    pub media_type: String,
    /// Base64-encoded image payload.
    pub data_base64: String,
}

/// Background-task status snapshot. Rendered differently per status:
/// `killed` emits a brief "stopped by user"
/// note; `running` warns against duplicate spawns with optional delta
/// summary + output-file pointer; `completed` / `failed` surface the
/// full delta with `(type: ...) (status: ...) (description: ...)`
/// metadata so the agent can route a follow-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatusSnapshot {
    pub task_id: String,
    pub description: String,
    pub status: TaskRunStatus,
    /// Task type used in `(type: ...)` metadata for completed / failed renderings.
    pub task_type: String,
    pub delta_summary: Option<String>,
    pub output_file_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRunStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

/// Skill record for `invoked_skills`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvokedSkillEntry {
    pub name: String,
    pub path: String,
    pub content: String,
}

/// Teammate mailbox snapshot — a bundle of unread messages for the
/// agent. The engine / swarm layer pre-formats the entries and passes the
/// final string to the generator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeammateMailboxInfo {
    pub formatted: String,
}

/// Team context for the first-turn team-coordination injection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamContextSnapshot {
    pub agent_id: String,
    pub agent_name: String,
    pub team_name: String,
    pub team_config_path: String,
    pub task_list_path: String,
}

/// Pending agent-inbox message (from other teammates).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPendingMessage {
    pub from: String,
    pub text: String,
}

/// Default explore-agent count referenced in the 5-phase Full prompt.
pub const DEFAULT_EXPLORE_AGENT_COUNT: i32 = 3;

/// Default plan-agent count.
pub const DEFAULT_PLAN_AGENT_COUNT: i32 = 1;

/// Inclusive min/max clamps on agent counts (see `AgentCount::new`).
pub const MIN_AGENTS: i32 = 1;
pub const MAX_AGENTS: i32 = 10;

fn clamp_agents(n: i32) -> i32 {
    n.clamp(MIN_AGENTS, MAX_AGENTS)
}

#[cfg(test)]
#[path = "generator.test.rs"]
mod tests;
