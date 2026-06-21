//! Core types for the system-reminder subsystem.
//!
//! Every [`AttachmentType`] variant serializes to the exact discriminator
//! string used on the wire (e.g. `"plan_mode"`, `"plan_mode_exit"`,
//! `"plan_mode_reentry"`, `"auto_mode_exit"`). This keeps wire compatibility
//! with transcripts / SDK consumers that round-trip attachment types through
//! JSON.

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use coco_types::SkillDiscoveryPayload;

/// Tier determines when a generator runs. Generators are split into three
/// parallel batches:
///
/// - All-thread attachments → [`ReminderTier::Core`]
/// - Main-thread attachments (gated on `is_main_agent`) → [`ReminderTier::MainAgentOnly`]
/// - User-input attachments (gated on `has_user_input`) → [`ReminderTier::UserPrompt`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderTier {
    /// Runs for every agent, including sub-agents.
    Core,
    /// Main agent only (gated on `is_main_agent`).
    MainAgentOnly,
    /// Only when user input is present this turn (gated on `has_user_input`).
    UserPrompt,
}

/// XML tag used to wrap reminder content.
///
/// `<system-reminder>` is used for almost all attachment cases.
/// Additional tags (e.g. `<new-diagnostics>`) may be introduced for
/// specific reminder types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XmlTag {
    /// `<system-reminder>…</system-reminder>` — the default tag.
    SystemReminder,
    /// Raw content, no wrapping.
    None,
}

impl XmlTag {
    /// The literal tag name, or `None` for raw.
    pub const fn tag_name(self) -> Option<&'static str> {
        match self {
            Self::SystemReminder => Some("system-reminder"),
            Self::None => None,
        }
    }
}

/// The kind of reminder being generated.
///
/// Phase A ships only the set needed to migrate plan-mode reminders from
/// `app/query/plan_mode_reminder.rs`. Phase C extends this with `TodoReminder`,
/// `TaskReminder`, `CompactionReminder`, `AutoMode`, `CriticalSystemReminder`,
/// `VerifyPlanReminder`, `DateChange`, etc.
///
/// Wire format (snake_case) matches `Attachment.type` exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentType {
    // ── Plan-mode reminders (Core tier; run for subagents too) ──
    /// Plan-mode steady-state. Full/Sparse is chosen inside the generator
    /// from the history-derived attachment count since the last exit.
    PlanMode,
    /// Emitted on the turn plan mode exits.
    PlanModeExit,
    /// Emitted on the first turn after re-entering plan mode.
    PlanModeReentry,

    // ── Auto-mode reminders (Core tier) ──
    /// Emitted on the turn auto mode exits.
    AutoModeExit,

    // ── Todo / task reminders (Core tier) ──
    /// Emitted when the agent hasn't used `TodoWrite` for ≥10 turns.
    TodoReminder,
    /// Emitted when V2 task tools haven't been used for ≥10 turns.
    TaskReminder,

    // ── Critical system reminder (Core tier) ──
    /// Per-turn user-supplied instruction, injected verbatim.
    CriticalSystemReminder,

    // ── Auto-mode steady-state (Core tier) ──
    /// Full/Sparse cadence while the engine is in auto mode (or plan-with-auto).
    /// Symmetric to [`PlanMode`](Self::PlanMode).
    AutoMode,

    // ── Compaction + date-change notices (Core tier) ──
    /// One-shot "auto-compact enabled" nudge once the context nears its capacity.
    CompactionReminder,
    /// Fires when the local ISO date rolls past the previously-seen date within
    /// the session (e.g. coding past midnight).
    DateChange,

    /// Per-turn baseline user context. Prepends an `isMeta`
    /// `<system-reminder>` carrying `currentDate` on every turn. Unlike
    /// [`DateChange`](Self::DateChange) (a one-shot rollover notice), this
    /// fires unconditionally so the model always has the date.
    UserContext,

    // ── Verify-plan reminder (MainAgentOnly tier) ──
    /// Deprecated legacy path. Fires every 10 human turns after ExitPlanMode
    /// while TS-shaped `pending_plan_verification` exists and has not started
    /// or completed. Main-thread only — sub-agents don't own the plan. Gated
    /// on the `settings.system_reminder.attachments.verify_plan_reminder`
    /// flag and explicit `VerifyPlanExecution` registration.
    VerifyPlanReminder,

    // ── Phase 1 engine-local reminders (Core tier unless noted) ──
    /// Fires when the user prompt contains the `ultrathink` keyword,
    /// asking the model to apply high reasoning effort on this turn.
    /// Gated on the opt-in `ultrathink_effort` settings flag (default off).
    UltrathinkEffort,

    /// Main-thread-only usage report injected every turn when enabled.
    /// Gated on the opt-in `token_usage` settings flag (default off).
    TokenUsage,

    /// Main-thread-only cost report; fires whenever `max_budget_usd` is set.
    BudgetUsd,

    /// Per-turn output-token report with optional turn budget.
    /// Gated on the opt-in `output_token_usage` settings flag (default off).
    OutputTokenUsage,

    /// One-shot intro emitted once per session when a companion is configured
    /// and hasn't been announced yet. Gated on the opt-in `companion_intro`
    /// settings flag (default off).
    CompanionIntro,

    // ── Phase 2 history-diff delta reminders (Core tier) ──
    /// Announces tool additions / removals via ToolSearch mid-session.
    /// Engine pre-computes the delta against prior announcements in
    /// history; generator emits when the delta is non-empty.
    DeferredToolsDelta,

    /// Announces agent-type additions / removals for the Agent tool.
    /// Engine pre-computes the delta (with `is_initial` on first emit +
    /// optional concurrency note).
    AgentListingDelta,

    /// Announces MCP server instructions added / removed since last
    /// announcement. Engine pre-computes by diffing current server
    /// instructions against prior announcements in history.
    McpInstructionsDelta,

    // ── Phase 3 cross-crate state reminders (Core tier unless noted) ──
    /// Success output from SessionStart / UserPromptSubmit hook execution.
    /// MainAgentOnly.
    HookSuccess,
    /// Hook blocked the turn due to a command error.
    HookBlockingError,
    /// Hook supplied extra context lines.
    HookAdditionalContext,
    /// Hook halted a continuation.
    HookStoppedContinuation,
    /// Post-async-hook response: systemMessage and/or additionalContext as
    /// separate messages.
    AsyncHookResponse,

    /// LSP/IDE diagnostics for files changed this turn. Wrapped in
    /// `<new-diagnostics>` inside `<system-reminder>`.
    Diagnostics,

    /// Active output-style reinforcement; MainAgentOnly.
    OutputStyle,

    /// Mid-turn queued user / system command drained into the turn.
    QueuedCommand,

    /// Background-task status report emitted post-compaction.
    TaskStatus,

    /// Listing of available skills.
    SkillListing,

    /// Content of skills invoked in the current session.
    InvokedSkills,

    /// Unread messages for the current teammate (agentSwarms-gated).
    TeammateMailbox,

    /// One-shot team-coordination context injected on the first turn
    /// (agentSwarms-gated).
    TeamContext,

    /// Pending inbox messages for the agent.
    AgentPendingMessages,

    // ── Memory reminders (Core tier) ──
    /// Per-turn injection of nested CLAUDE.md / memory file contents
    /// triggered by @-mention path traversal. Data sourced from
    /// `core/context::Attachment::NestedMemory`.
    NestedMemory,
    /// Semantically-ranked memory file contents surfaced via async prefetch.
    /// Multi-message reminder (one message per memory entry) wrapped in a
    /// single `<system-reminder>`.
    RelevantMemories,

    // ── Phase 4 user-input-tier reminders (UserPrompt) ──
    /// Announces @-mentioned files; file content itself is loaded via
    /// `core/context::Attachment::File`.
    AtMentionedFiles,
    /// Lists MCP resources referenced in the prompt.
    McpResources,
    /// User-requested agent invocations.
    AgentMentions,

    // ── Main-thread IDE reminders ──
    /// IDE-sourced selection context.
    IdeSelection,
    /// IDE-opened file notification.
    IdeOpenedFile,

    // ── Reminder-native silent attachments ──
    /// Records paths already loaded this session so the @-mention / memory
    /// pipelines don't re-inject the same content. Silent: carries
    /// `ReminderMetadata::AlreadyReadFile` for UI/transcript but injects
    /// zero API tokens.
    ///
    /// Lives in this crate because dedup state is intrinsic to reminder
    /// flow: `AtMentionedFilesGenerator`, `NestedMemoryGenerator`, and
    /// `RelevantMemoriesGenerator` all consult it before emitting file
    /// content.
    AlreadyReadFile,
    /// Records image-file changes that can't be diffed textually. Silent
    /// marker; UI may highlight the changed file.
    EditedImageFile,

    // ── Audit-add (May 2026) — model-visible attachment.
    /// Heuristic skill suggestions for the current prompt. UserPrompt tier.
    SkillDiscovery,
}

impl AttachmentType {
    /// The tier this reminder belongs to (determines injection batch).
    pub const fn tier(self) -> ReminderTier {
        match self {
            Self::PlanMode
            | Self::PlanModeExit
            | Self::PlanModeReentry
            | Self::AutoMode
            | Self::AutoModeExit
            | Self::TodoReminder
            | Self::TaskReminder
            | Self::CriticalSystemReminder
            | Self::CompactionReminder
            | Self::DateChange
            | Self::UserContext
            // Core batch — ultrathink + companion intro + deltas + swarm.
            | Self::UltrathinkEffort
            | Self::CompanionIntro
            | Self::DeferredToolsDelta
            | Self::AgentListingDelta
            | Self::McpInstructionsDelta
            | Self::QueuedCommand
            | Self::SkillListing
            | Self::TeammateMailbox
            | Self::TeamContext
            | Self::AgentPendingMessages
            | Self::NestedMemory
            | Self::RelevantMemories
            => ReminderTier::Core,
            // UserPrompt batch — only fires when the user submitted input this turn.
            Self::AtMentionedFiles
            | Self::McpResources
            | Self::AgentMentions
            // Audit-add: skill discovery is a per-prompt heuristic.
            | Self::SkillDiscovery => ReminderTier::UserPrompt,
            // MainAgentOnly batch.
            Self::VerifyPlanReminder
            | Self::TokenUsage
            | Self::BudgetUsd
            | Self::OutputTokenUsage
            | Self::HookSuccess
            | Self::HookBlockingError
            | Self::HookAdditionalContext
            | Self::HookStoppedContinuation
            | Self::AsyncHookResponse
            | Self::Diagnostics
            | Self::OutputStyle
            | Self::TaskStatus
            | Self::InvokedSkills
            | Self::IdeSelection
            | Self::IdeOpenedFile
            // Silent reminder-native types — cocode-rs places these in
            // MainAgentOnly since the dedup/change bookkeeping is
            // main-thread concern (subagents see parent context).
            | Self::AlreadyReadFile
            | Self::EditedImageFile => ReminderTier::MainAgentOnly,
        }
    }

    /// The XML tag for this reminder. Almost all reminders share
    /// [`XmlTag::SystemReminder`].
    pub const fn xml_tag(self) -> XmlTag {
        match self {
            Self::PlanMode
            | Self::PlanModeExit
            | Self::PlanModeReentry
            | Self::AutoMode
            | Self::AutoModeExit
            | Self::TodoReminder
            | Self::TaskReminder
            | Self::CriticalSystemReminder
            | Self::CompactionReminder
            | Self::DateChange
            | Self::UserContext
            | Self::VerifyPlanReminder
            | Self::UltrathinkEffort
            | Self::TokenUsage
            | Self::BudgetUsd
            | Self::OutputTokenUsage
            | Self::CompanionIntro
            | Self::DeferredToolsDelta
            | Self::AgentListingDelta
            | Self::McpInstructionsDelta
            | Self::HookSuccess
            | Self::HookBlockingError
            | Self::HookAdditionalContext
            | Self::HookStoppedContinuation
            | Self::AsyncHookResponse
            | Self::OutputStyle
            | Self::QueuedCommand
            | Self::TaskStatus
            | Self::SkillListing
            | Self::InvokedSkills
            | Self::TeammateMailbox
            | Self::TeamContext
            | Self::AgentPendingMessages
            | Self::AtMentionedFiles
            | Self::McpResources
            | Self::AgentMentions
            | Self::IdeSelection
            | Self::IdeOpenedFile
            | Self::NestedMemory
            | Self::RelevantMemories
            // Silent variants wrap for UI consistency; injection pipeline
            // drops them from the model-visible path regardless.
            | Self::AlreadyReadFile
            | Self::EditedImageFile
            | Self::SkillDiscovery
            => XmlTag::SystemReminder,
            // `diagnostics` wraps its content in `<new-diagnostics>`
            // before the outer `<system-reminder>`. The generator bakes
            // the inner tag into the body text; the outer `<system-reminder>`
            // is applied by the pipeline.
            Self::Diagnostics => XmlTag::SystemReminder,
        }
    }

    /// Stable string identifier. Matches the serde `rename_all` wire form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanMode => "plan_mode",
            Self::PlanModeExit => "plan_mode_exit",
            Self::PlanModeReentry => "plan_mode_reentry",
            Self::AutoMode => "auto_mode",
            Self::AutoModeExit => "auto_mode_exit",
            Self::TodoReminder => "todo_reminder",
            Self::TaskReminder => "task_reminder",
            Self::CriticalSystemReminder => "critical_system_reminder",
            Self::CompactionReminder => "compaction_reminder",
            Self::DateChange => "date_change",
            Self::UserContext => "user_context",
            Self::VerifyPlanReminder => "verify_plan_reminder",
            Self::UltrathinkEffort => "ultrathink_effort",
            Self::TokenUsage => "token_usage",
            Self::BudgetUsd => "budget_usd",
            Self::OutputTokenUsage => "output_token_usage",
            Self::CompanionIntro => "companion_intro",
            Self::DeferredToolsDelta => "deferred_tools_delta",
            Self::AgentListingDelta => "agent_listing_delta",
            Self::McpInstructionsDelta => "mcp_instructions_delta",
            Self::HookSuccess => "hook_success",
            Self::HookBlockingError => "hook_blocking_error",
            Self::HookAdditionalContext => "hook_additional_context",
            Self::HookStoppedContinuation => "hook_stopped_continuation",
            Self::AsyncHookResponse => "async_hook_response",
            Self::Diagnostics => "diagnostics",
            Self::OutputStyle => "output_style",
            Self::QueuedCommand => "queued_command",
            Self::TaskStatus => "task_status",
            Self::SkillListing => "skill_listing",
            Self::InvokedSkills => "invoked_skills",
            Self::TeammateMailbox => "teammate_mailbox",
            Self::TeamContext => "team_context",
            Self::AgentPendingMessages => "agent_pending_messages",
            Self::AtMentionedFiles => "at_mentioned_files",
            Self::McpResources => "mcp_resources",
            Self::AgentMentions => "agent_mentions",
            Self::IdeSelection => "ide_selection",
            Self::IdeOpenedFile => "ide_opened_file",
            Self::NestedMemory => "nested_memory",
            Self::RelevantMemories => "relevant_memories",
            Self::AlreadyReadFile => "already_read_file",
            Self::EditedImageFile => "edited_image_file",
            Self::SkillDiscovery => "skill_discovery",
        }
    }

    /// Every [`AttachmentType`] variant in declaration order. Used by the
    /// completeness test to assert that each reminder has a default
    /// generator registered + a config toggle.
    pub const fn all() -> &'static [AttachmentType] {
        &[
            Self::PlanMode,
            Self::PlanModeExit,
            Self::PlanModeReentry,
            Self::AutoMode,
            Self::AutoModeExit,
            Self::TodoReminder,
            Self::TaskReminder,
            Self::CriticalSystemReminder,
            Self::CompactionReminder,
            Self::DateChange,
            Self::UserContext,
            Self::VerifyPlanReminder,
            Self::UltrathinkEffort,
            Self::TokenUsage,
            Self::BudgetUsd,
            Self::OutputTokenUsage,
            Self::CompanionIntro,
            Self::DeferredToolsDelta,
            Self::AgentListingDelta,
            Self::McpInstructionsDelta,
            Self::HookSuccess,
            Self::HookBlockingError,
            Self::HookAdditionalContext,
            Self::HookStoppedContinuation,
            Self::AsyncHookResponse,
            Self::Diagnostics,
            Self::OutputStyle,
            Self::QueuedCommand,
            Self::TaskStatus,
            Self::SkillListing,
            Self::InvokedSkills,
            Self::TeammateMailbox,
            Self::TeamContext,
            Self::AgentPendingMessages,
            Self::AtMentionedFiles,
            Self::McpResources,
            Self::AgentMentions,
            Self::IdeSelection,
            Self::IdeOpenedFile,
            Self::NestedMemory,
            Self::RelevantMemories,
            Self::AlreadyReadFile,
            Self::EditedImageFile,
            Self::SkillDiscovery,
        ]
    }
}

impl std::fmt::Display for AttachmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lift the reminder subset into the full [`coco_types::AttachmentKind`]
/// taxonomy (60 variants).
///
/// `AttachmentType` is the subset of attachment types this crate owns
/// as reminder-producing generators (41 model-visible + 2 silent =
/// 43 variants). The full union lives in `coco-types` so every consumer
/// crate can reference a single authoritative discriminator without
/// pulling system-reminder as a dependency.
///
/// The mapping is total and infallible — every `AttachmentType` has a
/// corresponding `AttachmentKind`. The reverse direction is **not**
/// total (most `AttachmentKind`s don't produce reminders), so there's
/// no `TryFrom` the other way — use [`coco_types::AttachmentKind::coverage`]
/// to ask "does this kind live in my crate?".
impl From<AttachmentType> for coco_types::AttachmentKind {
    fn from(value: AttachmentType) -> Self {
        use coco_types::AttachmentKind as K;
        match value {
            AttachmentType::PlanMode => K::PlanMode,
            AttachmentType::PlanModeExit => K::PlanModeExit,
            AttachmentType::PlanModeReentry => K::PlanModeReentry,
            AttachmentType::AutoMode => K::AutoMode,
            AttachmentType::AutoModeExit => K::AutoModeExit,
            AttachmentType::TodoReminder => K::TodoReminder,
            AttachmentType::TaskReminder => K::TaskReminder,
            AttachmentType::CriticalSystemReminder => K::CriticalSystemReminder,
            AttachmentType::CompactionReminder => K::CompactionReminder,
            AttachmentType::DateChange => K::DateChange,
            AttachmentType::UserContext => K::UserContext,
            AttachmentType::VerifyPlanReminder => K::VerifyPlanReminder,
            AttachmentType::UltrathinkEffort => K::UltrathinkEffort,
            AttachmentType::TokenUsage => K::TokenUsage,
            AttachmentType::BudgetUsd => K::BudgetUsd,
            AttachmentType::OutputTokenUsage => K::OutputTokenUsage,
            AttachmentType::CompanionIntro => K::CompanionIntro,
            AttachmentType::DeferredToolsDelta => K::DeferredToolsDelta,
            AttachmentType::AgentListingDelta => K::AgentListingDelta,
            AttachmentType::McpInstructionsDelta => K::McpInstructionsDelta,
            AttachmentType::HookSuccess => K::HookSuccess,
            AttachmentType::HookBlockingError => K::HookBlockingError,
            AttachmentType::HookAdditionalContext => K::HookAdditionalContext,
            AttachmentType::HookStoppedContinuation => K::HookStoppedContinuation,
            AttachmentType::AsyncHookResponse => K::AsyncHookResponse,
            AttachmentType::Diagnostics => K::Diagnostics,
            AttachmentType::OutputStyle => K::OutputStyle,
            AttachmentType::QueuedCommand => K::QueuedCommand,
            AttachmentType::TaskStatus => K::TaskStatus,
            AttachmentType::SkillListing => K::SkillListing,
            AttachmentType::InvokedSkills => K::InvokedSkills,
            AttachmentType::TeammateMailbox => K::TeammateMailbox,
            AttachmentType::TeamContext => K::TeamContext,
            // `AgentPendingMessages` is a synthetic grouping that maps to
            // `queued_command` with a coordinator-origin flag.
            // Route it to `QueuedCommand` to preserve the wire tag.
            AttachmentType::AgentPendingMessages => K::QueuedCommand,
            AttachmentType::AtMentionedFiles => K::File,
            AttachmentType::McpResources => K::McpResource,
            AttachmentType::AgentMentions => K::AgentMention,
            AttachmentType::IdeSelection => K::SelectedLinesInIde,
            AttachmentType::IdeOpenedFile => K::OpenedFileInIde,
            AttachmentType::NestedMemory => K::NestedMemory,
            AttachmentType::RelevantMemories => K::RelevantMemories,
            AttachmentType::AlreadyReadFile => K::AlreadyReadFile,
            AttachmentType::EditedImageFile => K::EditedImageFile,
            AttachmentType::SkillDiscovery => K::SkillDiscovery,
        }
    }
}

/// Role of an injected message, used by multi-message reminders.
///
/// Most reminders produce a single user message. A few (e.g. `already_read_file`
/// when we port it) need paired assistant/user messages to carry
/// `tool_use` + `tool_result` blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// Content block inside a multi-message reminder.
///
/// Matches Anthropic content-block shapes (`text` / `image` /
/// `tool_use` / `tool_result`) so multi-message reminders can produce
/// exact-equivalent API payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text block.
    Text { text: String },
    /// Image block — carries mid-turn image pastes routed through the
    /// queued-command path.
    Image {
        /// IANA media type (e.g. `image/png`, `image/jpeg`).
        media_type: String,
        /// Base64-encoded image payload.
        data_base64: String,
    },
    /// Synthetic tool-use block.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Tool-result block paired with a prior `tool_use`.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// A single message within a [`ReminderOutput::Messages`] sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    /// Hidden from UI transcripts.
    #[serde(default)]
    pub is_meta: bool,
}

impl ReminderMessage {
    /// Construct a user message with a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            is_meta: true,
        }
    }

    /// Construct a user message with a single text block followed by image
    /// blocks. Queued commands with image pastes emit `[text, ...imageBlocks]`
    /// so both reach the API. `images` is a list of `(media_type, base64_data)`
    /// pairs; an empty vec is equivalent to [`Self::user_text`].
    pub fn user_text_with_images(text: impl Into<String>, images: Vec<(String, String)>) -> Self {
        let mut blocks = vec![ContentBlock::Text { text: text.into() }];
        for (media_type, data_base64) in images {
            blocks.push(ContentBlock::Image {
                media_type,
                data_base64,
            });
        }
        Self {
            role: MessageRole::User,
            blocks,
            is_meta: true,
        }
    }

    /// Construct an assistant message with the given blocks.
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            is_meta: true,
        }
    }
}

/// The payload a generator produced for this turn.
///
/// **Model-visible outputs** (injected into the API request):
/// - `Text` → single user message (the 95% case).
/// - `Messages` → multiple paired messages (used for reminders that need
///   `tool_use` + `tool_result` structure).
/// - `ModelAttachment` → JSON payload serialized then wrapped; for
///   structured data the model should treat as an attachment rather than prose.
///
/// **Silent output** (zero tokens to API; still routed to UI via
/// [`crate::inject::NormalizedMessages::display_only`]):
/// - `SilentAttachment` — structured payload for UI / telemetry.
///
/// Attachment types that produce no API tokens (e.g. `already_read_file`,
/// `edited_image_file`) use `SilentAttachment` so the data reaches
/// UI/transcript layers without reaching the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReminderOutput {
    Text(String),
    Messages(Vec<ReminderMessage>),
    SkillDiscovery(SkillDiscoveryPayload),
    ModelAttachment {
        payload: Value,
    },
    /// Silent structured attachment. Parallel to [`Self::ModelAttachment`].
    /// Used when the payload is structured data (e.g. a list of deduped
    /// file paths) that the UI consumes but the model never sees.
    SilentAttachment {
        payload: Value,
    },
}

impl ReminderOutput {
    /// True when this output produces zero API content.
    ///
    /// Two cases: (1) explicit `SilentAttachment` variant always returns
    /// true; (2) empty model-visible variants return true so generators
    /// can signal "nothing to say this turn" via empty strings / empty
    /// vecs / null payloads.
    pub fn is_silent(&self) -> bool {
        match self {
            Self::SilentAttachment { .. } => true,
            Self::Text(s) => s.is_empty(),
            Self::Messages(m) => m.is_empty(),
            Self::SkillDiscovery(payload) => payload.skills.is_empty(),
            Self::ModelAttachment { payload } => payload.is_null(),
        }
    }

    /// If the output is a single text blob, return a borrow of the content.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Structured, per-attachment-type UI/telemetry payload attached to
/// silent reminders.
///
/// Only silent reminders carry metadata; model-visible reminders inline
/// their data in the rendered content. Adding a new silent type means:
/// (1) add a variant here carrying the typed payload, (2) add an
/// [`AttachmentType`] variant, (3) add a generator that returns a
/// `SystemReminder` with `output = Silent*` + `metadata = Some(...)`.
///
/// Wire format uses `#[serde(tag = "type")]` so JSON-round-tripped
/// transcripts preserve the variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReminderMetadata {
    /// Payload for [`AttachmentType::AlreadyReadFile`] — list of session-
    /// deduped paths the UI may surface as "already in context".
    AlreadyReadFile(AlreadyReadFileMeta),
    /// Payload for [`AttachmentType::EditedImageFile`] — paths of image
    /// files whose mtime changed since last seen.
    EditedImageFile(EditedImageFileMeta),
}

/// Paths deduped across the current session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AlreadyReadFileMeta {
    pub paths: Vec<PathBuf>,
}

/// Image files modified since last observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EditedImageFileMeta {
    pub paths: Vec<PathBuf>,
}

/// The unified generator return type.
///
/// One generator produces at most one [`SystemReminder`] per turn (or `None`
/// when not applicable). The orchestrator collects them into a `Vec`, throttle
/// state is updated, and the inject step converts each into `coco_messages::Message`
/// entries — silent reminders are filtered from the model path and routed to
/// UI/transcript via [`crate::inject::NormalizedMessages::display_only`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemReminder {
    pub attachment_type: AttachmentType,
    pub output: ReminderOutput,
    /// Default true — reminders are always meta unless a generator explicitly
    /// surfaces user-visible content.
    #[serde(default = "default_is_meta")]
    pub is_meta: bool,
    /// Explicit silent flag. Kept distinct from [`ReminderOutput::is_silent`]
    /// so a generator can mark a text-carrying reminder as silent (e.g. for
    /// staged rollout / observability) without switching output variant.
    /// Consumers should check both: `reminder.is_silent || reminder.output.is_silent()`.
    #[serde(default)]
    pub is_silent: bool,
    /// UI / telemetry payload for silent reminders. `None` for model-visible
    /// reminders; `Some(...)` when the silent reminder carries structured
    /// data the UI should surface (e.g. deduped file paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ReminderMetadata>,
}

fn default_is_meta() -> bool {
    true
}

impl SystemReminder {
    /// Build a text reminder. The content is wrapped at injection time per
    /// [`AttachmentType::xml_tag`].
    pub fn new(attachment_type: AttachmentType, content: impl Into<String>) -> Self {
        Self {
            attachment_type,
            output: ReminderOutput::Text(content.into()),
            is_meta: true,
            is_silent: false,
            metadata: None,
        }
    }

    pub fn skill_discovery(payload: SkillDiscoveryPayload) -> Self {
        Self {
            attachment_type: AttachmentType::SkillDiscovery,
            output: ReminderOutput::SkillDiscovery(payload),
            is_meta: true,
            is_silent: false,
            metadata: None,
        }
    }

    /// Build a silent text reminder. Same shape as [`Self::new`] but with
    /// `is_silent = true`, so the inject pipeline routes the body to
    /// `NormalizedMessages::display_only` (UI / transcript) and never
    /// emits an API-visible `AttachmentMessage::api(...)`.
    ///
    /// Use this for reminder-native kinds whose
    /// [`AttachmentKind::is_api_visible`] is `false` (the API sees nothing)
    /// but whose body is still useful to UI consumers.
    pub fn silent_text(attachment_type: AttachmentType, content: impl Into<String>) -> Self {
        Self {
            attachment_type,
            output: ReminderOutput::Text(content.into()),
            is_meta: true,
            is_silent: true,
            metadata: None,
        }
    }

    /// Build a reminder from a prebuilt multi-message sequence.
    pub fn messages(attachment_type: AttachmentType, messages: Vec<ReminderMessage>) -> Self {
        Self {
            attachment_type,
            output: ReminderOutput::Messages(messages),
            is_meta: true,
            is_silent: false,
            metadata: None,
        }
    }

    /// Build a silent attachment reminder carrying structured metadata.
    ///
    /// Use for attachment types whose `normalizeAttachmentForAPI` returns `[]`
    /// but whose payload the UI still consumes (e.g. `already_read_file`,
    /// `edited_image_file`). The `payload` is the JSON projection of
    /// `metadata` — keeping both gives consumers a typed path (`metadata`)
    /// and a wire-format path (`payload`).
    pub fn silent_attachment(attachment_type: AttachmentType, metadata: ReminderMetadata) -> Self {
        let payload = serde_json::to_value(&metadata).unwrap_or(Value::Null);
        Self {
            attachment_type,
            output: ReminderOutput::SilentAttachment { payload },
            is_meta: true,
            is_silent: true,
            metadata: Some(metadata),
        }
    }

    /// Mark this reminder as silent (log-only, no injection). Does not
    /// change the [`ReminderOutput`] variant — use [`Self::silent_attachment`]
    /// to set both atomically.
    pub fn silent(mut self) -> Self {
        self.is_silent = true;
        self
    }

    /// Attach structured metadata.
    pub fn with_metadata(mut self, metadata: ReminderMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// True when this reminder should not reach the model (silent flag
    /// OR silent output variant OR empty content).
    pub fn is_effectively_silent(&self) -> bool {
        self.is_silent || self.output.is_silent()
    }

    /// Unwrap the text content if this is a (silent or model-visible) text reminder.
    pub fn content(&self) -> Option<&str> {
        self.output.as_text()
    }

    /// The XML tag this reminder wraps its content in.
    pub fn xml_tag(&self) -> XmlTag {
        self.attachment_type.xml_tag()
    }
}

#[cfg(test)]
#[path = "types.test.rs"]
mod tests;
