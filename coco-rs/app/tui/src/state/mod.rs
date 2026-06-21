//! TUI application state — the Model in TEA.
//!
//! Split into two substates:
//! - [`SessionState`]: agent-synchronized data (model, messages, tools, subagents)
//! - [`UiState`]: local TUI state (input, scroll, surfaces, streaming, theme)

pub mod agents_dialog;
pub mod interaction;
pub mod modal;
pub mod permissions_editor;
pub mod plugin_dialog;
pub mod question_prompt;
pub mod rewind;
pub mod session;
pub mod skills_dialog;
pub mod surface_payloads;
pub mod transcript;
pub mod transcript_view;
pub mod ui;
pub(crate) mod ui_ephemeral;

pub use crate::completion::ActiveSuggestions;
pub use crate::completion::CompletionRequestKey;
pub use crate::completion::DismissedCompletion;
pub use crate::completion::McpResourceCompletion;
pub use crate::completion::SlackChannelCompletion;
pub use crate::completion::SuggestionKind;
pub use crate::display_settings::DisplaySettings;
pub use agents_dialog::AgentsDialogState;
pub use agents_dialog::AgentsDialogTab;
pub use agents_dialog::CreateWizardState;
pub use agents_dialog::CreateWizardStep;
pub use agents_dialog::LibraryRow;
pub use agents_dialog::LibraryToastKind;
pub use agents_dialog::WizardError;
pub use agents_dialog::WizardSource;
pub use agents_dialog::WizardTextField;
pub use agents_dialog::is_valid_desc_char;
pub use agents_dialog::is_valid_name_char;
pub use agents_dialog::resolve_create_target;
pub use agents_dialog::validate_agent_name;
pub use coco_tui_ui::display::SyntaxHighlighting;
pub use interaction::AtPopupState;
pub use interaction::ComposerPopupState;
pub use interaction::ComposerState;
pub use interaction::InteractionPaneState;
pub use interaction::InvalidSlashCommandName;
pub use interaction::PanePromptState;
pub use interaction::SlashCommandName;
pub use interaction::SlashPopupState;
pub use interaction::SymbolPopupState;
pub use modal::ModalQueue;
pub use modal::ModalState;
pub use permissions_editor::AddForm;
pub use permissions_editor::AddStep;
pub use permissions_editor::DeleteConfirm;
pub use permissions_editor::DeleteTarget;
pub use permissions_editor::EditorDestination;
pub use permissions_editor::Focused as PermEditorFocused;
pub use permissions_editor::PermDirRow;
pub use permissions_editor::PermEditorError;
pub use permissions_editor::PermRuleRow;
pub use permissions_editor::PermissionsEditorState;
pub use permissions_editor::PermissionsEditorTab;
pub use plugin_dialog::PluginDialogState;
pub use plugin_dialog::PluginDialogTab;
pub use plugin_dialog::PluginHintResponse;
pub use plugin_dialog::PluginHintState;
pub use question_prompt::OtherInputState;
pub use question_prompt::QuestionFocusTarget;
pub use question_prompt::QuestionFooterAction;
pub use question_prompt::QuestionItem;
pub use question_prompt::QuestionOption;
pub use question_prompt::QuestionPage;
pub use question_prompt::QuestionPromptState;
pub use question_prompt::SubmitAction;
pub use rewind::DiffStatsPreview;
pub use rewind::RestoreType;
pub use rewind::RewindPhase;
pub use rewind::RewindState;
pub use rewind::RewindableMessage;
pub use session::McpServerStatus;
pub use session::ModelBinding;
pub use session::ModelCatalogEntry;
pub use session::ProviderStatus;
pub use session::ProviderUnavailableReason;
pub use session::QueuedCommandDisplay;
pub use session::SavedSession;
pub use session::SessionState;
pub use session::SlashCommandInfo;
pub use session::SubagentInstance;
pub use session::SubagentKind;
pub use session::SubagentStatus;
pub use session::TokenUsage;
pub use session::ToolExecution;
pub use session::ToolStatus;
pub use skills_dialog::SaveDiff as SkillsDialogSaveDiff;
pub use skills_dialog::SkillLock;
pub use skills_dialog::SkillLockSource;
pub use skills_dialog::SkillOverrideState;
pub use skills_dialog::SkillRow;
pub use skills_dialog::SkillsDialogSource;
pub use skills_dialog::SkillsDialogState;
pub use surface_payloads::AddDirectoryState;
pub use surface_payloads::BackgroundTasksState;
pub use surface_payloads::BridgeState;
pub use surface_payloads::BypassPermissionsState;
pub use surface_payloads::CopyPickerCodeBlock;
pub use surface_payloads::CopyPickerSelection;
pub use surface_payloads::CopyPickerState;
pub use surface_payloads::CostWarningPromptState;
pub use surface_payloads::DiffViewState;
pub use surface_payloads::DoctorCheck;
pub use surface_payloads::DoctorState;
pub use surface_payloads::ExplainerFetch;
pub use surface_payloads::ExportFormat;
pub use surface_payloads::ExportState;
pub use surface_payloads::FeedbackState;
pub use surface_payloads::GlobalSearchState;
pub use surface_payloads::IdleReturnState;
pub use surface_payloads::InvalidConfigState;
pub use surface_payloads::McpServerApprovalPromptState;
pub use surface_payloads::McpServerOption;
pub use surface_payloads::McpServerSelectState;
pub use surface_payloads::MemoryDialogEntry;
pub use surface_payloads::MemoryDialogRowKind;
pub use surface_payloads::MemoryDialogScope;
pub use surface_payloads::MemoryDialogState;
pub use surface_payloads::ModelEntry;
pub use surface_payloads::ModelPickerState;
pub use surface_payloads::PermissionDetail;
pub use surface_payloads::PermissionPromptState;
pub use surface_payloads::PlanApprovalPromptState;
pub use surface_payloads::PlanEntryPromptState;
pub use surface_payloads::PrefixInputState;
pub use surface_payloads::QuickOpenState;
pub use surface_payloads::RiskLevel;
pub use surface_payloads::SandboxPermissionPromptState;
pub use surface_payloads::SearchResult;
pub use surface_payloads::SessionBrowserState;
pub use surface_payloads::SessionOption;
pub use surface_payloads::TaskDetailState;
pub use surface_payloads::TeamRosterMember;
pub use surface_payloads::TeamRosterState;
pub use surface_payloads::ThemePickerState;
pub use surface_payloads::TrustState;
pub use surface_payloads::WorktreeExitState;
pub use transcript::TranscriptState;
pub use transcript_view::TranscriptView;
pub use ui::ExitKey;
pub use ui::FocusTarget;
pub use ui::HistoryEntry;
pub use ui::HistorySearch;
pub use ui::InlineGhost;
pub use ui::InputState;
pub use ui::PromptMode;
pub use ui::StreamMode;
pub use ui::StreamingState;
pub use ui::Toast;
pub use ui::ToastSeverity;
pub use ui::UiState;

use coco_types::PermissionMode;

/// Complete TUI application state.
#[derive(Debug)]
pub struct AppState {
    /// Agent-synchronized state.
    pub session: SessionState,
    /// UI-only local state.
    pub ui: UiState,
    /// Application lifecycle.
    pub running: RunningState,
    /// Wall-clock source. Production uses [`coco_tui_ui::clock::SystemClock`];
    /// tests substitute [`coco_tui_ui::clock::MockClock`] to pin time for
    /// the todo-panel completion lift / hide windows and the
    /// subagent-progress stamp paths. `Arc<dyn Clock>` (Send + Sync)
    /// so it survives `tokio::spawn` across worker threads.
    pub clock: std::sync::Arc<dyn coco_tui_ui::clock::Clock>,
}

/// Application lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunningState {
    #[default]
    Running,
    Done,
}

/// The frame-paced animation the surface needs next, derived purely from
/// [`AppState`]. `App::redraw` maps each variant to a single self-armed
/// frame; [`UiAnimation::Idle`] arms nothing, letting the frame scheduler
/// sleep until a real event arrives.
///
/// This is the *one* place that answers "should the TUI keep painting?".
/// Collapsing it into a single derivation — instead of flag checks
/// scattered across the run loop — is what closes the gap where a turn
/// blocked on a user prompt re-painted identical spinner frames forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiAnimation {
    /// Streaming text reveal: one row per painted frame, so the reveal
    /// pacing *is* the frame cadence. Re-armed even behind a prompt — the
    /// stream keeps producing visible rows.
    StreamReveal,
    /// Running-turn / compaction spinner. Its glyph is a pure function of
    /// the paused-aware status clock, so it only advances while the turn
    /// is genuinely progressing.
    SpinnerOnly,
    /// Nothing animates; the scheduler may sleep until the next event.
    Idle,
}

impl AppState {
    /// Create a new default state with a [`SystemClock`].
    pub fn new() -> Self {
        Self::with_clock(coco_tui_ui::clock::SystemClock::arc())
    }

    /// Create a new state with an explicit clock — used by tests to
    /// pin time deterministically.
    pub fn with_clock(clock: std::sync::Arc<dyn coco_tui_ui::clock::Clock>) -> Self {
        Self {
            session: SessionState::default(),
            ui: UiState::new(),
            running: RunningState::Running,
            clock,
        }
    }

    /// Whether the app should exit.
    pub fn should_exit(&self) -> bool {
        self.running == RunningState::Done
    }

    /// Signal the app to exit.
    pub fn quit(&mut self) {
        self.running = RunningState::Done;
    }

    /// Whether an interaction surface is currently active.
    pub fn has_active_surface(&self) -> bool {
        self.ui.has_active_surface()
    }

    /// Whether the agent is actively streaming.
    pub fn is_streaming(&self) -> bool {
        self.ui.streaming.is_some()
    }

    /// Whether a spinner should be shown.
    pub fn should_show_spinner(&self) -> bool {
        self.is_streaming() || self.session.is_busy()
    }

    /// What the surface needs to animate next, if anything. Pure read of
    /// the model — it never schedules; `App::redraw` translates the result
    /// into a self-armed frame.
    ///
    /// The spinner only needs a fresh frame while it is both *visible* and
    /// *advancing*. The glyph is a pure function of the paused-aware
    /// `elapsed_ms` (`status_indicator::spinner_frame`), so it is static —
    /// and re-arming pure waste — when either:
    /// - a full-screen modal covers the interactive viewport
    ///   (`render_interactive_viewport` early-returns to the modal), or
    /// - the turn is paused on a blocking tool-permission prompt (frozen
    ///   `elapsed_ms`).
    ///
    /// A bottom-pane prompt that does *not* pause (PlanExit, Question,
    /// McpServerApproval, …) leaves the indicator visible and the clock
    /// running, so it must keep animating — the elapsed clock stays live
    /// through those dialogs. Streaming is always armed: it produces a
    /// new visible row per painted frame.
    pub(crate) fn ui_animation(&self) -> UiAnimation {
        if self.is_streaming() {
            return UiAnimation::StreamReveal;
        }
        // Subagents and background tasks can outlive the foreground turn
        // (swarm teammates, `run_in_background`, the inter-turn gap). Their
        // panel rows carry live elapsed clocks, so the spinner cadence must
        // stay armed while they run — gating purely on `turn_active` let the
        // frame scheduler sleep, freezing those clocks until the next event.
        let background_activity =
            self.session.has_running_subagent() || self.session.has_running_background_task();
        let spinner_animating =
            (self.ui.ephemeral.turn_active() || self.session.is_compacting || background_activity)
                && self.ui.modal.is_none()
                && !self.ui.ephemeral.turn_paused();
        if spinner_animating {
            UiAnimation::SpinnerOnly
        } else {
            UiAnimation::Idle
        }
    }

    /// Whether Ctrl+C has work to interrupt (streaming, busy session,
    /// queued commands). Used by the exit double-press to distinguish
    /// "cancel a task" from "arm the exit prompt".
    pub fn has_interruptible_work(&self) -> bool {
        self.is_streaming() || self.session.is_busy() || !self.session.queued_commands.is_empty()
    }

    /// Whether Esc-driven rewind is currently appropriate: the input
    /// must be empty, the session must have user-visible history, and
    /// no state can be occluding the cursor.
    ///
    /// "Session has history" reads from the engine-authoritative
    /// `transcript` view.
    pub fn rewind_available_from_input(&self) -> bool {
        self.ui.input.is_empty()
            && !self.session.transcript.is_empty()
            && !self.ui.has_active_surface()
    }

    /// Cycle permission mode (Shift+Tab).
    ///
    /// Delegates to [`PermissionMode::next_in_cycle`]. Bypass/auto gate
    /// flags are forwarded from the session.
    pub fn cycle_permission_mode(&mut self) {
        self.session.permission_mode = self.session.permission_mode.next_in_cycle(
            self.session.plan_mode_available,
            self.session.bypass_permissions_available,
            self.session.auto_mode_available,
        );
    }

    /// Toggle plan mode on/off (Tab).
    ///
    /// Quick shortcut distinct from the full cycle: flips between
    /// `Plan` and `Default`, preserving nothing. Callers that need to
    /// return to an earlier elevated mode should use the full cycle.
    ///
    /// When the `plan_mode` feature is disabled, entering plan mode is a
    /// no-op (Tab does nothing); exiting an already-active Plan mode is
    /// still allowed so a resumed session can escape.
    pub fn toggle_plan_mode(&mut self) {
        self.session.permission_mode = if self.session.permission_mode == PermissionMode::Plan {
            PermissionMode::Default
        } else if self.session.plan_mode_available {
            PermissionMode::Plan
        } else {
            self.session.permission_mode
        };
    }

    /// Whether the current session is in plan mode. Derived from
    /// [`SessionState::permission_mode`] — there is no separate bool.
    pub fn is_plan_mode(&self) -> bool {
        self.session.permission_mode == PermissionMode::Plan
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
