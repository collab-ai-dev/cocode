//! User-facing configuration for the `coco-system-reminder` subsystem.
//!
//! Lives in `coco-config` (not `coco-system-reminder`) so it can appear in
//! [`crate::Settings`] alongside every other user-configurable knob. The
//! `coco-system-reminder` crate imports these types via a `coco-config`
//! dependency and uses them verbatim — there is no parallel struct.
//!
//! Every reminder generator looks up its enable flag on
//! [`SystemReminderConfig::attachments`]; the orchestrator consults
//! [`SystemReminderConfig::enabled`] as a master switch and
//! [`SystemReminderConfig::timeout_ms`] for per-generator runtime deadline.

use serde::Deserialize;
use serde::Serialize;

/// Root configuration for the reminder subsystem.
///
/// Serialized form matches the `system_reminder` key under `settings.json`.
/// All fields are `#[serde(default)]`, so partial configs in user settings
/// fill missing fields from [`Default`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemReminderConfig {
    /// Master switch. When false, the orchestrator produces zero reminders
    /// regardless of per-attachment flags.
    pub enabled: bool,

    /// Per-generator timeout in milliseconds. Values `<= 0` fall back to
    /// [`DEFAULT_TIMEOUT_MS`]. Each parallel batch has this budget;
    /// generators that exceed it are skipped.
    pub timeout_ms: i64,

    /// Per-reminder enable flags.
    pub attachments: AttachmentSettings,

    /// User-supplied content injected on every turn (subject to the
    /// `CriticalSystemReminder` generator's own gating). Reads from
    /// `criticalSystemReminder_EXPERIMENTAL` in the tool use context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_instruction: Option<String>,
}

impl Default for SystemReminderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            attachments: AttachmentSettings::default(),
            critical_instruction: None,
        }
    }
}

/// Default per-generator timeout (matches TS `attachments.ts:767`).
pub const DEFAULT_TIMEOUT_MS: i64 = 1000;

/// Per-reminder enable flags. Matches TS `Attachment.type` variants 1:1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachmentSettings {
    /// Plan-mode steady-state reminder (TS `plan_mode`).
    pub plan_mode: bool,
    /// Plan-mode exit banner (TS `plan_mode_exit`).
    pub plan_mode_exit: bool,
    /// Plan-mode re-entry banner (TS `plan_mode_reentry`).
    pub plan_mode_reentry: bool,
    /// Auto-mode exit banner (TS `auto_mode_exit`).
    pub auto_mode_exit: bool,
    /// TodoWrite nudge reminder (TS `todo_reminder`).
    pub todo_reminder: bool,
    /// V2 task-tools nudge reminder (TS `task_reminder`).
    pub task_reminder: bool,
    /// User-supplied per-turn critical instruction (TS `critical_system_reminder`).
    pub critical_system_reminder: bool,
    /// Auto-mode steady-state reminder (TS `auto_mode`).
    pub auto_mode: bool,
    /// Auto-compact enabled nudge (TS `compaction_reminder`).
    pub compaction_reminder: bool,
    /// Date-change notification (TS `date_change`).
    pub date_change: bool,
    /// Per-turn baseline user context (TS `prependUserContext`,
    /// `utils/api.ts:449`). Injects `Today's date is <local ISO>.` every
    /// turn so the model always has the date. On by default.
    pub user_context: bool,
    /// Legacy verify-plan reminder (TS `verify_plan_reminder`). Deprecated
    /// path: emits only when legacy plan verification was explicitly enabled
    /// and `VerifyPlanExecution` is explicitly registered in the active tool
    /// list.
    pub verify_plan_reminder: bool,

    /// Ultrathink reasoning-effort nudge (TS `ultrathink_effort`). **Opt-in**
    /// — TS gates on `feature('ULTRATHINK')` + GrowthBook; external builds
    /// default off. Users flip this in `settings.json` to enable keyword-
    /// driven high-effort routing.
    pub ultrathink_effort: bool,

    /// Token-usage report (TS `token_usage`). **Opt-in** — TS requires
    /// `CLAUDE_CODE_ENABLE_TOKEN_USAGE_ATTACHMENT` env var. When enabled,
    /// injects `used/total; remaining` every main-thread turn. This value is
    /// *volatile* (cumulative tokens), so it is emitted per-turn and appended
    /// at the tail — the freshest copy always rides the end of context
    /// (cache-safe, mirrors TS). Deliberately NOT throttled: a turn-throttle
    /// would leave a stale value buried mid-history during long multi-tool
    /// turns and the model would act on out-of-date numbers.
    pub token_usage: bool,

    /// USD budget report (TS `budget_usd`). Fires whenever
    /// `QueryEngineConfig::max_budget_usd` is set. No additional TS gate;
    /// default on so users who set a budget see it without extra config.
    pub budget_usd: bool,

    /// Output-token report (TS `output_token_usage`). **Opt-in** — TS
    /// gates on `feature('TOKEN_BUDGET')`.
    pub output_token_usage: bool,

    /// Companion intro reminder (TS `companion_intro`). **Opt-in** — TS
    /// gates on `feature('BUDDY')` + configured companion. Off by default.
    pub companion_intro: bool,

    /// Deferred-tool-availability delta (TS `deferred_tools_delta`).
    /// Fires when the current tool set differs from the last announced
    /// set. No extra TS feature gate — on by default.
    pub deferred_tools_delta: bool,

    /// Agent-listing delta (TS `agent_listing_delta`). Announces
    /// available agent types for the Agent tool. On by default.
    pub agent_listing_delta: bool,

    /// MCP server instructions delta (TS `mcp_instructions_delta`).
    /// Fires when MCP server instructions are added / removed. On by
    /// default.
    pub mcp_instructions_delta: bool,

    // ── Phase 3 cross-crate state reminders ──
    /// Hook success output (TS `hook_success`). On by default.
    pub hook_success: bool,
    /// Hook blocking error (TS `hook_blocking_error`). On by default.
    pub hook_blocking_error: bool,
    /// Hook additional context (TS `hook_additional_context`). On by default.
    pub hook_additional_context: bool,
    /// Hook stopped continuation (TS `hook_stopped_continuation`). On by default.
    pub hook_stopped_continuation: bool,
    /// Async hook response (TS `async_hook_response`). On by default.
    pub async_hook_response: bool,
    /// LSP / IDE diagnostics (TS `diagnostics`). On by default.
    pub diagnostics: bool,
    /// Output style reinforcement (TS `output_style`). On by default.
    pub output_style: bool,
    /// Queued command replay (TS `queued_command`). On by default.
    pub queued_command: bool,
    /// Background-task status (TS `task_status`). On by default.
    pub task_status: bool,
    /// Skill listing (TS `skill_listing`). On by default.
    pub skill_listing: bool,
    /// Skills invoked this session (TS `invoked_skills`). On by default.
    pub invoked_skills: bool,
    /// Teammate mailbox (TS `teammate_mailbox`, swarm-gated). On by default.
    pub teammate_mailbox: bool,
    /// Team context (TS `team_context`, swarm-gated). On by default.
    pub team_context: bool,
    /// Agent pending messages (TS `agent_pending_messages`). On by default.
    pub agent_pending_messages: bool,

    // ── Phase 4 user-input-tier reminders ──
    /// At-mentioned file reminder (TS `file` in `userInputAttachments`). On by default.
    pub at_mentioned_files: bool,
    /// MCP resource references (TS `mcp_resource`). On by default.
    pub mcp_resources: bool,
    /// Agent mention reminder (TS `agent_mention`). On by default.
    pub agent_mentions: bool,

    // ── Main-thread IDE reminders ──
    /// IDE selection reminder (TS `selected_lines_in_ide`). On by default.
    pub ide_selection: bool,
    /// IDE opened-file reminder (TS `opened_file_in_ide`). On by default.
    pub ide_opened_file: bool,

    /// Nested memory injection (TS `nested_memory`). Fires per-turn
    /// when @-mention traversal surfaces nested CLAUDE.md files. On by default.
    pub nested_memory: bool,
    /// Relevant memories (TS `relevant_memories`). Async-prefetched,
    /// semantically-ranked memory file contents. On by default.
    pub relevant_memories: bool,

    // ── Reminder-native silent attachments (Part 1) ──
    /// Already-read-file dedup marker (TS `already_read_file`, `normalizeAttachmentForAPI` → `[]`).
    /// Zero API tokens; metadata (file paths) retained for UI / transcript.
    /// Mirrors cocode-rs `AlreadyReadFile`. On by default.
    pub already_read_file: bool,
    /// Edited-image-file marker (TS `edited_image_file`, `normalizeAttachmentForAPI` → `[]`).
    /// Silent — image diffs aren't text; UI surfaces the path. On by default.
    pub edited_image_file: bool,

    // ── Audit-add (May 2026) — model-visible TS attachment.
    /// Heuristic skill discovery reminder (TS `skill_discovery`). Off by default.
    pub skill_discovery: bool,
}

impl Default for AttachmentSettings {
    fn default() -> Self {
        Self {
            plan_mode: true,
            plan_mode_exit: true,
            plan_mode_reentry: true,
            auto_mode_exit: true,
            todo_reminder: true,
            task_reminder: true,
            critical_system_reminder: true,
            auto_mode: true,
            compaction_reminder: true,
            date_change: true,
            user_context: true,
            verify_plan_reminder: true,
            // Phase 1 — feature-gated in TS, default off in external builds.
            ultrathink_effort: false,
            token_usage: false,
            output_token_usage: false,
            companion_intro: false,
            // TS fires unconditionally when max_budget_usd is set — default on.
            budget_usd: true,
            // Phase 2 — delta reminders fire unconditionally in TS; on by default.
            deferred_tools_delta: true,
            agent_listing_delta: true,
            mcp_instructions_delta: true,
            // Phase 3 — on by default; generators short-circuit when ctx state is empty.
            hook_success: true,
            hook_blocking_error: true,
            hook_additional_context: true,
            hook_stopped_continuation: true,
            async_hook_response: true,
            diagnostics: true,
            output_style: true,
            queued_command: true,
            task_status: true,
            skill_listing: true,
            invoked_skills: true,
            teammate_mailbox: true,
            team_context: true,
            agent_pending_messages: true,
            // Phase 4 user-input tier — on by default; these three fire only
            // when user submitted input this turn (UserPrompt tier).
            at_mentioned_files: true,
            mcp_resources: true,
            agent_mentions: true,
            // Main-thread IDE reminders — on by default; source returns
            // empty when no IDE state is available.
            ide_selection: true,
            ide_opened_file: true,
            nested_memory: true,
            relevant_memories: true,
            // Part 1 — silent reminder-native attachments. Zero tokens to
            // API; UI/transcript only. Safe to leave on by default.
            already_read_file: true,
            edited_image_file: true,
            skill_discovery: false,
        }
    }
}

#[cfg(test)]
#[path = "system_reminder.test.rs"]
mod tests;
