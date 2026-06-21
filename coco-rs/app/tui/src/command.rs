//! User commands from TUI to core engine.
//!
//! These are the outbound messages sent from the TUI to the agent loop
//! when the user takes an action that requires core processing.

use std::fmt;

use coco_messages::SystemMessageLevel;
use coco_types::PermissionMode;
use coco_types::PermissionResolutionDetail;
use coco_types::PermissionUpdate;

/// Why the TUI requested process shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    /// User typed `/exit` or `/quit`.
    SlashCommand,
    /// User invoked an immediate-quit command such as Ctrl+Q.
    ImmediateQuit,
    /// User confirmed Ctrl+C double-press exit.
    DoublePressCtrlC,
    /// User confirmed Ctrl+D double-press exit.
    DoublePressCtrlD,
}

impl ShutdownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SlashCommand => "slash_command",
            Self::ImmediateQuit => "immediate_quit",
            Self::DoublePressCtrlC => "double_press_ctrl_c",
            Self::DoublePressCtrlD => "double_press_ctrl_d",
        }
    }
}

impl fmt::Display for ShutdownReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rewind dispatch mode. ADT, not a flag — variants carry the
/// parameters they need and only those parameters, so the type
/// system rejects illegal combinations at compile time (e.g.
/// `RestoreType` cannot leak into the `AutoRestore` path).
///
/// Consolidates the explicit `rewindConversationTo()` flow and the
/// cancel-on-empty-input auto-restore branch.
#[derive(Debug, Clone)]
pub enum RewindMode {
    /// Explicit `/rewind` flow from the picker. May restore files,
    /// summarize, emit the `RewindCompleted` overlay, and run picker
    /// confirmation.
    Explicit {
        restore_type: crate::state::rewind::RestoreType,
        /// 1-based turn number the user picked, for the protocol-level
        /// `rewind/completed` notification. Threaded through so SDK
        /// consumers see it without a second query.
        rewound_turn: i32,
    },
    /// TUI auto-restore on cancel-with-empty-input at a lossless tail
    /// boundary. Synchronous history truncation only — no file
    /// restoration, no modal overlay. The engine emits
    /// `MessageTruncated` so SDK + TUI converge on engine authority.
    AutoRestore,
}

/// Typed payload for [`UserCommand::PushSystemMessage`]. Each variant
/// carries the fields the engine needs to construct the matching
/// [`coco_messages::SystemMessage`] sub-variant before calling
/// `history_push_and_emit`. Lets TUI-originated transcript content
/// (slash output, file-open notices, bash command results, …) flow
/// through the engine instead of being written directly into a
/// TUI-local buffer. See
/// `engine-tui-unified-transcript-plan.md` §3 Commit 2.
#[derive(Debug, Clone)]
pub enum SystemPushKind {
    /// Plain notice → `SystemMessage::Informational { level, title, message }`.
    /// Empty `title` renders without the `"<title>: "` prefix.
    Informational {
        level: SystemMessageLevel,
        title: String,
        message: String,
    },
    /// Bash-mode local command result → `SystemMessage::LocalCommand`.
    LocalCommand { command: String, output: String },
}

/// Commands sent from TUI to the core agent loop.
#[derive(Debug, Clone)]
pub enum UserCommand {
    /// Submit a bash-mode entry (input started with `!`). The TUI has
    /// already stripped the leading `!`; the engine bridge in
    /// `tui_runner` runs the command via `coco_shell::ShellExecutor`
    /// and pushes a `SystemMessage::LocalCommand` (input + output) onto
    /// the engine transcript via `history_push_and_emit`. Bypasses the
    /// model loop entirely.
    SubmitBash {
        /// User-message UUID minted at submit time so the BashInput
        /// and BashOutput messages can share a parent id for rewind.
        user_message_id: String,
        /// Shell command (already prefix-stripped).
        command: String,
    },
    /// Open a memory file chosen from the `/memory` picker. The TUI
    /// only owns selection state; the CLI bridge owns filesystem and
    /// process effects so terminal/editor behavior stays outside
    /// reducers and renderers.
    OpenMemoryFile {
        /// Memory file target selected by the picker.
        path: std::path::PathBuf,
    },
    /// Open the current prompt draft in an external editor. The TUI
    /// sends the current text; the CLI bridge owns temp-file and
    /// process effects, then emits a TUI event with the edited text.
    OpenPromptEditor {
        /// Prompt content to seed into the editor buffer.
        initial_content: String,
    },
    /// Open this session's plan file in an external editor. The CLI
    /// bridge resolves the concrete plan-file path from the current
    /// session id and runtime config before launching the editor.
    OpenPlanEditor,
    /// The TUI has left raw mode and any active state alt-screen, so
    /// the CLI runner may now start the editor process for `request_id`.
    ExternalEditorTerminalReady {
        /// Opaque id from `TuiOnlyEvent::ExternalEditorPrepare`.
        request_id: String,
    },
    /// The TUI failed to prepare terminal modes, so the CLI runner
    /// should drop the pending editor request and surface this failure.
    ExternalEditorTerminalPrepareFailed {
        /// Opaque id from `TuiOnlyEvent::ExternalEditorPrepare`.
        request_id: String,
        /// User-visible failure summary.
        error: String,
    },
    /// Submit user input text with resolved paste data.
    SubmitInput {
        /// User-message UUID minted at submit time. The agent driver
        /// builds the `Message::User` carrying this id and emits it via
        /// `history_push_and_emit`; `FileHistoryState` keys the per-turn
        /// snapshot on the same id. Single source of truth so rewind
        /// picker selections, file-history snapshots, and the JSONL
        /// transcript line up.
        ///
        user_message_id: String,
        /// Resolved text content (paste pills expanded, image pills removed).
        content: String,
        /// Original input text (with pills intact) for display in chat history.
        display_text: Option<String>,
        /// Image data from pasted images (clipboard or drag-drop).
        images: Vec<coco_tui_ui::paste::ImageData>,
    },
    /// Persist a submitted prompt to the cross-session composer history
    /// (`<config_home>/history.jsonl`). Emitted by the composer alongside
    /// the in-memory `add_to_history` so up-arrow recall survives restarts;
    /// the driver writes it off-thread via `coco_session::PromptHistory`.
    /// `display` carries any mode prefix (`!` / `/`) so recall returns to
    /// the same mode.
    PersistPromptHistory { display: String },
    /// Interrupt current operation (Ctrl+C).
    Interrupt(coco_types::TurnAbortReason),
    /// Interrupt a teammate's active turn without killing the teammate.
    InterruptAgentCurrentWork { agent_id: String },
    /// Cancel a running subagent / background task. Fires the task's
    /// cancellation token via `TaskManager::kill_running`; the engine's
    /// existing `TaskCompleted` event then folds the row out of the UI.
    ///
    /// Wired from the `/agents` dialog's Running tab when the user
    /// presses `X` on the highlighted row.
    CancelSubagent {
        /// `TaskStateBase.id` of the running subagent invocation.
        task_id: String,
    },
    /// Open an agent markdown file in `$EDITOR` / `$VISUAL`. The CLI
    /// bridge suspends the TUI, spawns the editor, and emits
    /// `TuiOnlyEvent::AgentFileSaved` on exit so the dialog refreshes
    /// against the live catalog.
    ///
    /// Wired from the `/agents` Library tab when the user presses
    /// Enter on an agent row (edit) or on the `Create new agent` row
    /// (create — `path` points at a freshly-created template).
    OpenAgentEditor {
        /// Absolute markdown path. The bridge calls `run_editor_on_file`
        /// directly; no special creation logic — callers stage any
        /// new-file template content into `path` before dispatch.
        path: std::path::PathBuf,
    },
    /// Delete an agent markdown file and trigger a catalog reload so
    /// the dialog re-renders without the deleted row.
    ///
    /// Wired from the `/agents` Library tab `d` keystroke after the
    /// user confirms the delete overlay.
    DeleteAgentFile {
        /// Absolute markdown path. Bridge `fs::remove_file`s and then
        /// `reload_agent_catalog()`s.
        path: std::path::PathBuf,
    },
    /// Finalize the `/agents` Library inline create wizard. The CLI
    /// bridge:
    ///   1. resolves the target directory (`~/.coco/agents` or
    ///      `<cwd>/.coco/agents`),
    ///   2. writes a markdown template with the wizard
    ///      inputs in the frontmatter,
    ///   3. dispatches the existing `$EDITOR` flow on the new file so
    ///      the user can fine-tune body / tools / model / color,
    ///   4. on editor exit, reloads the agent catalog and refreshes
    ///      the dialog payload.
    ///
    /// Finalizes the new-agent creation wizard.
    CreateAgent {
        /// Canonical agent identifier (validated by
        /// `coco_tui::state::validate_agent_name`).
        name: String,
        /// `whenToUse` description body. Required.
        description: String,
        /// Selected source — narrows TS `BaseAgentSource` to the two
        /// scopes coco-rs writes to directly (user / project).
        source: coco_types::AgentSource,
    },
    /// Set permission mode. Replaces the legacy `SetPlanMode { bool }`
    /// — plan-mode activation is just `SetPermissionMode { mode: Plan }`.
    SetPermissionMode { mode: PermissionMode },
    /// Apply one permission update from the `/permissions` rule editor —
    /// add / remove a rule or working directory at a chosen destination.
    /// The CLI applies it to the live engine config, persists it to the
    /// destination settings file (User / Project / Local), then re-emits
    /// `OpenPermissionsEditor` so the open overlay refreshes in place.
    /// Like the "Always Allow" persist path, but the editor lets the
    /// user pick any of the three writable scopes.
    ApplyPermissionUpdate { update: PermissionUpdate },
    /// Leader → teammate: set a teammate's permission mode from the teams
    /// roster picker (gap 8). Routed to `AgentHandle::set_teammate_mode`,
    /// which persists it + notifies the live teammate via `ModeSetRequest`.
    SetTeammateMode { name: String, mode: PermissionMode },
    /// Leader → teammates: set MANY teammates' modes at once (roster
    /// "cycle all", Shift+Left/Right). One atomic `team.json` write +
    /// per-teammate `ModeSetRequest`. Routed to `AgentHandle::set_teammate_modes`.
    SetTeammateModes {
        updates: Vec<(String, PermissionMode)>,
    },
    /// Set the Main role's thinking effort.
    ///
    /// Emitted by [`crate::events::TuiCommand::CycleThinkingLevel`]
    /// (Ctrl+T). `level` is the wire-form string from
    /// `ReasoningEffort::to_string` (e.g. `"high"`, `"xhigh"`). The
    /// engine resolves it in-memory via `SessionRuntime::apply_role_effort`
    /// — no file write.
    SetThinkingLevel { level: String },
    /// Set the model bound to `role` plus its thinking effort. Emitted
    /// by the role-pill model picker on Enter; the engine applies the
    /// selection in-memory via `SessionRuntime::apply_role_override`
    /// — no file write. Non-Main roles take effect on the next turn
    /// that drives that role; Main effort takes effect immediately,
    /// Main model_id changes require a session restart (v1 limitation).
    SetModelRole {
        role: coco_types::ModelRole,
        provider: String,
        model_id: String,
        /// Chosen effort. `None` when the model has no thinking capability.
        effort: Option<coco_types::ReasoningEffort>,
    },
    /// Respond to a permission prompt.
    ApprovalResponse {
        request_id: String,
        approved: bool,
        always_allow: bool,
        /// User feedback explaining their decision (why they approved/denied).
        feedback: Option<String>,
        /// Modified tool input (user edited the command/path before approving).
        updated_input: Option<serde_json::Value>,
        /// Trusted tool-specific approval metadata.
        resolution_detail: Option<PermissionResolutionDetail>,
        /// Permission rules to persist from this decision (suggestions the user accepted).
        permission_updates: Vec<PermissionUpdate>,
        /// Optional content blocks (image attachments etc.) the user
        /// pasted alongside the answer. Today no TUI gesture emits this;
        /// SDK clients ship via `ApprovalResolveParams.content_blocks`.
        content_blocks: Option<Vec<serde_json::Value>>,
    },
    /// Execute a skill by name.
    ExecuteSkill { name: String, args: Option<String> },
    /// Persist a `skill_overrides` patch to
    /// `<cwd>/.coco/settings.local.json` and republish
    /// `RuntimeConfig`. Emitted by the `/skills` dialog's Enter
    /// handler when the diff actually changes disk state.
    ///
    /// **No display metadata here.** TUI stashes `total_edits` on
    /// `UiState.pending_skills_save_edits` before dispatch and
    /// reads it back when [`coco_types::TuiOnlyEvent::SkillOverridesSaved`]
    /// arrives. The CLI bridge only reports Ok / typed Err.
    ///
    /// The CLI bridge in `tui_runner` owns the
    /// [`coco_config::SettingsWriter`] handle — keeping the writer
    /// out of the TUI lib avoids a coco-tui → coco-config write
    /// dep that other dialogs don't need yet.
    WriteSkillOverrides {
        /// Settings JSON patch ready for
        /// [`coco_config::SettingsWriter::write_local`] (the
        /// `Value::Null` entries delete keys).
        patch: serde_json::Value,
    },
    /// Execute a registered slash command without echoing the raw slash
    /// invocation into chat history.
    ExecuteSlashCommand {
        name: crate::state::SlashCommandName,
        args: String,
    },
    /// Queue a command for mid-turn injection.
    ///
    /// Sent by [`crate::update::QueueInput`] when the user presses
    /// Enter while the agent is streaming. The CLI bridge in
    /// `tui_runner` forwards this to
    /// `runtime.command_queue().enqueue(...)` so the engine sees the
    /// prompt at the next drain point. `images` carries any pasted
    /// images at submit time so mid-turn screenshot pastes survive
    /// queueing — same shape as [`Self::SubmitInput`].
    QueueCommand {
        prompt: String,
        images: Vec<coco_tui_ui::paste::ImageData>,
    },
    /// Remove a queued command from the engine queue and return it to
    /// the composer for editing.
    EditQueuedCommand { id: String },
    /// Remove all user-editable queued commands from the engine queue
    /// and combine them with the current composer draft.
    EditQueuedCommands {
        current_input: String,
        current_cursor: usize,
    },
    /// Background all foreground tasks. Sent by the live single-press
    /// Ctrl+B path (`TuiCommand::BackgroundAllTasks` in update.rs).
    BackgroundAllTasks,
    /// Kill all running agents.
    KillAllAgents,
    /// Toggle fast mode.
    ToggleFastMode,
    /// Trigger manual compaction. Optional `custom_instructions` carry
    /// any text after `/compact` so the LLM summarizer prompt can honor
    /// the user's focus directive.
    Compact { custom_instructions: Option<String> },
    /// Rewind to an earlier user message.
    ///
    /// `mode` is an ADT, not a flag — the `AutoRestore` variant
    /// structurally cannot carry a `RestoreType`, so the
    /// "auto-restore never touches files" invariant is enforced by
    /// the type system, not by separate command variants.
    ///
    /// See `engine-tui-unified-transcript-plan.md` §4.2 / §7.4.
    Rewind {
        message_id: String,
        mode: RewindMode,
    },
    /// Lazily fetch the LLM risk explanation for a permission prompt (the user
    /// toggled the Ctrl+E explainer panel). The runner runs the explainer via
    /// the session SideQuery handle and replies with
    /// `TuiOnlyEvent::PermissionExplanationReady`.
    RequestPermissionExplanation {
        request_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
    },
    /// Request selected-message restore diff stats.
    RequestDiffStats { message_id: String },
    /// Request file-history availability for every real rewind row on
    /// picker open. Keeps per-candidate async load semantics without
    /// dropping rows on the bounded command channel.
    RequestDiffStatsBatch { message_ids: Vec<String> },
    /// Team lead responding to a teammate's plan-approval request.
    /// The engine routes this to the teammate's mailbox as a
    /// `plan_approval_response` envelope.
    PlanApprovalResponse {
        request_id: String,
        /// Teammate agent name to address the response envelope to —
        /// carried in from `PlanApprovalPromptState.from` so we don't have
        /// to re-scan mailbox state to correlate the request_id.
        teammate_agent: String,
        approved: bool,
        /// Optional feedback the leader attached (e.g. "good, but please
        /// add tests"). `None` when the user just approved/denied
        /// without typing anything.
        feedback: Option<String>,
    },
    /// Shutdown the application.
    Shutdown { reason: ShutdownReason },
    /// Fire an `idle_prompt` Notification hook. The TUI emits this
    /// once per turn-completion epoch when the user has been idle past
    /// the configured threshold. The runtime translates this into a
    /// `coco_hooks::orchestration::execute_notification` call so
    /// registered `Notification` hooks can react.
    FireIdleNotification { message: String },
    /// Push a TUI-originated system message into engine `MessageHistory`.
    /// The engine handler constructs the matching
    /// `coco_messages::SystemMessage::*` from `kind` and calls
    /// `history_push_and_emit`, so the round-trip surfaces via the
    /// normal `MessageAppended` → `TranscriptView` → render path.
    PushSystemMessage { kind: SystemPushKind },
    /// Push pre-built slash-command transcript messages (echo + result)
    /// into engine `MessageHistory`. Unlike [`Self::PushSystemMessage`],
    /// these are `Message::User` envelopes with command tags; they are
    /// `is_visible_in_transcript_only` (rendered `❯`/`⎿` but not sent
    /// to the model — slash commands are user↔tool interactions).
    /// Built via `coco_messages::build_slash_command_messages` so the TUI
    /// owns the localized text while the engine stays the transcript authority.
    PushSlashResult {
        messages: Vec<coco_messages::Message>,
    },
}
