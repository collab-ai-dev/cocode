//! `KeybindingAction` → `TuiCommand` dispatch.
//!
//! The resolver in `coco-keybindings` produces a typed
//! [`KeybindingAction`] from a key event + active context stack. This
//! module owns the TUI-side mapping from those actions to
//! [`TuiCommand`]s, including state-dependent dispatch (`Enter` while
//! streaming queues input, etc.) — the part that can't live in the
//! pure-logic resolver.
//!
//! Returns `None` when the action has no TUI-side handler. The caller
//! treats that as "swallow without effect" so unmapped actions don't
//! fall through to the legacy hardcoded cascade.

use coco_keybindings::KeybindingAction;

use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::SlashCommandName;

/// Map a resolved [`KeybindingAction`] to the TUI-side command.
///
/// `None` means no handler is wired — either the action represents a
/// feature not yet built, or it is layered above this dispatch point
/// (e.g. `command:foo` slash commands flow through the slash-command
/// runner, not this map).
///
/// The legacy cascade in `keybinding_bridge::map_key` only runs when
/// the resolver returned `NoMatch`; if we return `None` here the
/// keystroke is swallowed deliberately so a user-customized binding
/// doesn't accidentally fire a TUI-cascade fallback.
pub fn dispatch_action(action: &KeybindingAction, state: &AppState) -> Option<TuiCommand> {
    use KeybindingAction::*;
    Some(match action {
        // ── App-level (Global) ──────────────────────────────────────
        // Ctrl+C and Ctrl+D both go through `update::exit`'s
        // double-press machine — they do NOT immediately quit. See
        // `defaults.rs` and `reserved.rs` for the user-rebind block.
        AppInterrupt => TuiCommand::Interrupt,
        AppExit => TuiCommand::RequestExit,
        AppRedraw => TuiCommand::ClearScreen,
        // `app:toggleTodos` (Ctrl+Y) — cycle the right-rail expanded
        // view between None / Tasks / (Teammates if running).
        // `update::handle_command` does the cycle math.
        AppToggleTodos => TuiCommand::ToggleExpandedTasksView,
        // `app:toggleTranscript` (Ctrl+O) — open the verbose, scrollable
        // transcript state. Pressing it again from inside the state closes
        // it (handled in the state branch below).
        AppToggleTranscript => TuiCommand::ToggleTranscript,
        // `app:toggleTeammatePreview` (Ctrl+Shift+O) — toggle teammate
        // spinner-line message previews on/off.
        AppToggleTeammatePreview => TuiCommand::ToggleTeammateMessagePreview,
        // `app:toggleTeamRoster` (Ctrl+Shift+T) — open the teammate roster /
        // mode picker. Gated on the session having a teammate so the binding
        // is an inert no-op in non-team sessions (returns `None`, swallowing
        // the key without a cascade fallthrough) rather than shadowing globally.
        AppToggleTeamRoster => {
            return state
                .session
                .subagents
                .iter()
                .any(|s| s.kind == crate::state::SubagentKind::Teammate)
                .then_some(TuiCommand::OpenTeamRoster);
        }
        AppGlobalSearch => TuiCommand::ShowGlobalSearch,
        AppQuickOpen => TuiCommand::ShowQuickOpen,
        // `app:forceQuit` (ctrl+q) deliberately bypasses the `app:exit`
        // double-press confirmation — it is the power-user immediate quit.
        AppForceQuit => TuiCommand::Quit,
        AppHelp => TuiCommand::ShowHelp,
        AppCommandPalette => TuiCommand::ShowCommandPalette,
        AppSettings => TuiCommand::ShowSettings,
        AppSessionBrowser => TuiCommand::ShowSessionBrowser,
        AppPlanEditor => TuiCommand::OpenPlanEditor,
        // `app:toggleBrief` / `app:toggleTerminal` are feature-gated
        // capabilities not yet shipped. If a user explicitly binds one,
        // silently no-op — the keybinding is accepted but has no effect.
        AppToggleBrief | AppToggleTerminal => return None,

        // ── History navigation ──────────────────────────────────────
        HistorySearch => TuiCommand::HistorySearchStart,
        HistoryPrevious => TuiCommand::CursorUp,
        HistoryNext => TuiCommand::CursorDown,

        // ── Chat input ──────────────────────────────────────────────
        ChatCancel => {
            // Esc → Cancel always; the *second* Esc within
            // `DOUBLE_PRESS_TIMEOUT` (and only with no state + empty
            // input + history) opens the rewind picker. The poll is
            // here because dispatch reads + mutates the tracker
            // atomically — putting the arm in `app.rs` ahead of
            // dispatch would create the same "set then compare to
            // self" bug the old `last_esc_time` path had.
            //
            // The tracker is mutated through &AppState — see
            // `keybinding_resolver` for why `kb_handle` is `Arc<RwLock>`.
            // We don't have interior mutability for the tracker, so
            // double-press for Esc lives in `update::handle_command`'s
            // `TuiCommand::Cancel` arm via `state.ui.esc_tracker.poll`.
            TuiCommand::Cancel
        }
        ChatKillAgents => TuiCommand::KillAllAgents,
        ChatCycleMode => TuiCommand::CyclePermissionMode,
        ChatModelPicker => TuiCommand::CycleModel,
        ChatFastMode => TuiCommand::ToggleFastMode,
        ChatThinkingToggle => TuiCommand::ToggleThinking,
        // Local extension: cycle Main role thinking effort through
        // the active model's `supported_thinking_levels`. See
        // `update.rs::CycleThinkingLevel`.
        ChatCycleThinking => TuiCommand::CycleThinkingLevel,
        ChatSubmit => {
            // SubmitInput owns the streaming decision: it queues by default
            // and emits submit_interrupt only when every running tool is
            // cancel-interruptible.
            TuiCommand::SubmitInput
        }
        ChatNewline => TuiCommand::InsertNewline,
        ChatExternalEditor => TuiCommand::OpenExternalEditor,
        // `chat:stash` saves the current input draft for later.
        // Single-slot swap: pressing the binding stashes the current
        // text and restores the prior stash if any — same key triggers
        // both directions. Update handler in `update.rs` does the swap.
        ChatStash => TuiCommand::StashInputDraft,
        ChatImagePaste => TuiCommand::PasteFromClipboard,
        // `chat:undo` — restore the previous composer snapshot from the
        // TextArea undo stack (captured per mutating edit in `update.rs`).
        ChatUndo => TuiCommand::UndoInput,
        // `chat:messageActions` — message-actions cursor not yet shipped;
        // silently no-op.
        ChatMessageActions => return None,
        // ctrl+shift+r: toggle <system-reminder> visibility in the transcript.
        ChatToggleSystemReminders => TuiCommand::ToggleSystemReminders,
        // Tab is state-dependent — an active inline ghost or visible prompt
        // suggestion accepts it instead of toggling plan mode.
        ChatTogglePlanMode => {
            if state.ui.input.active_inline_ghost().is_some() {
                TuiCommand::AutocompleteAccept
            } else if crate::keybinding_bridge::prompt_suggestion_visible(state) {
                TuiCommand::AcceptPromptSuggestion
            } else {
                TuiCommand::TogglePlanMode
            }
        }

        // ── Autocomplete ────────────────────────────────────────────
        AutocompleteAccept => TuiCommand::AutocompleteAccept,
        AutocompleteDismiss => TuiCommand::Cancel,
        AutocompletePrevious => TuiCommand::SurfacePrev,
        AutocompleteNext => TuiCommand::SurfaceNext,

        // ── Confirmation ────────────────────────────────────────────
        ConfirmYes => TuiCommand::Approve,
        ConfirmNo => TuiCommand::Deny,
        ConfirmPrevious => TuiCommand::SurfacePrev,
        ConfirmNext => TuiCommand::SurfaceNext,
        ConfirmNextField => TuiCommand::SurfaceNext,
        ConfirmPreviousField => TuiCommand::SurfacePrev,
        ConfirmCycleMode => TuiCommand::CyclePermissionMode,
        ConfirmToggle => TuiCommand::SurfaceConfirm,
        ConfirmToggleExplanation => TuiCommand::TogglePermissionExplanation,
        // `PermissionToggleDebug`: no equivalent debug surface.
        PermissionToggleDebug => return None,

        // ── Tabs ────────────────────────────────────────────────────
        TabsNext => TuiCommand::SettingsNextTab,
        TabsPrevious => TuiCommand::SettingsPrevTab,

        // ── Transcript ──────────────────────────────────────────────
        TranscriptExit => TuiCommand::Cancel,

        // ── Help ────────────────────────────────────────────────────
        HelpDismiss => TuiCommand::Cancel,

        // ── HistorySearch ───────────────────────────────────────────
        HistorySearchNext => TuiCommand::SurfaceNext,
        HistorySearchAccept | HistorySearchExecute => TuiCommand::SurfaceConfirm,
        HistorySearchCancel => TuiCommand::Cancel,

        // ── Task ────────────────────────────────────────────────────
        TaskBackground => TuiCommand::BackgroundAllTasks,

        // ── ThemePicker ─────────────────────────────────────────────
        ThemeToggleSyntaxHighlighting => TuiCommand::ToggleSyntaxHighlighting,

        // ── Attachments ─────────────────────────────────────────────
        AttachmentsNext => TuiCommand::SurfaceNext,
        AttachmentsPrevious => TuiCommand::SurfacePrev,
        AttachmentsExit => TuiCommand::Cancel,
        // No remove-attachment surface yet; user-bound keys silently
        // no-op until the attachments panel lands.
        AttachmentsRemove => return None,

        // ── Footer ──────────────────────────────────────────────────
        FooterUp => TuiCommand::SurfacePrev,
        FooterDown => TuiCommand::SurfaceNext,
        FooterNext => TuiCommand::SurfaceNext,
        FooterPrevious => TuiCommand::SurfacePrev,
        FooterOpenSelected => TuiCommand::SurfaceConfirm,
        FooterClearSelection | FooterClose => TuiCommand::Cancel,

        // ── MessageSelector ─────────────────────────────────────────
        MessageSelectorUp => TuiCommand::SurfacePrev,
        MessageSelectorDown => TuiCommand::SurfaceNext,
        MessageSelectorTop => TuiCommand::SurfaceJumpStart,
        MessageSelectorBottom => TuiCommand::SurfaceJumpEnd,
        MessageSelectorSelect => TuiCommand::SurfaceConfirm,

        // ── Diff ────────────────────────────────────────────────────
        DiffDismiss => TuiCommand::Cancel,
        DiffPreviousFile => TuiCommand::SurfacePrev,
        DiffNextFile => TuiCommand::SurfaceNext,
        DiffPreviousSource => TuiCommand::SurfacePrev,
        DiffNextSource => TuiCommand::SurfaceNext,
        DiffBack => TuiCommand::Cancel,
        DiffViewDetails => TuiCommand::SurfaceConfirm,

        // ── ModelPicker ─────────────────────────────────────────────
        // Left/Right cycle the *effort axis* — separate from Up/Down
        // (`SelectPrevious` / `SelectNext`) which move between models.
        ModelPickerDecreaseEffort => TuiCommand::ModelPickerCycleEffort(-1),
        ModelPickerIncreaseEffort => TuiCommand::ModelPickerCycleEffort(1),

        // ── Select ──────────────────────────────────────────────────
        SelectNext => TuiCommand::SurfaceNext,
        SelectPrevious => TuiCommand::SurfacePrev,
        SelectAccept => TuiCommand::SurfaceConfirm,
        SelectCancel => TuiCommand::Cancel,

        // ── Plugin ──────────────────────────────────────────────────
        // Plugin context actions — no Plugin state yet; silently no-op
        // until the state lands.
        PluginToggle | PluginInstall => return None,

        // ── Settings ────────────────────────────────────────────────
        SettingsClose => TuiCommand::SurfaceConfirm,
        // SettingsSearch / SettingsRetry are inside-state state-machine
        // actions (not application-level TuiCommands). The Settings state
        // reads them directly from the resolver when it owns key dispatch
        // — they intentionally route to None here.
        SettingsSearch | SettingsRetry => return None,

        // ── Voice ───────────────────────────────────────────────────
        // Push-to-talk toggle. The `VoiceSession` lives on `App`, so the
        // command is intercepted in `App::handle_event` rather than routed
        // through `update::handle_command`. Gated at runtime by
        // `Feature::Voice` (inert when voice is disabled).
        VoicePushToTalk => TuiCommand::VoiceToggle,

        // ── Scroll (internal) ───────────────────────────────────────
        ScrollPageUp if transcript_active(state) => TuiCommand::TranscriptPage(-1),
        ScrollPageUp => TuiCommand::PageUp,
        ScrollPageDown if transcript_active(state) => TuiCommand::TranscriptPage(1),
        ScrollPageDown => TuiCommand::PageDown,
        ScrollLineUp if transcript_active(state) => TuiCommand::TranscriptScrollLines(-1),
        ScrollLineUp => TuiCommand::ScrollUp,
        ScrollLineDown if transcript_active(state) => TuiCommand::TranscriptScrollLines(1),
        ScrollLineDown => TuiCommand::ScrollDown,
        ScrollTop if transcript_active(state) => TuiCommand::TranscriptJumpStart,
        ScrollTop => TuiCommand::SurfaceJumpStart,
        ScrollBottom if transcript_active(state) => TuiCommand::TranscriptJumpEnd,
        ScrollBottom => TuiCommand::SurfaceJumpEnd,

        // ── Selection ───────────────────────────────────────────────
        SelectionCopy => TuiCommand::CopyLastMessage,

        // ── Slash command escape hatch ──────────────────────────────
        // `command:foo` user binding from `keybindings.json` →
        // synthesize a `/foo` submit. The agent driver's existing
        // slash-command runner handles the dispatch.
        Command(name) => SlashCommandName::new(name.clone())
            .map(TuiCommand::ExecuteSlashCommand)
            .unwrap_or(TuiCommand::Noop),

        // ── MessageActions:* (11 variants) ───────────────────────────
        // Internal context; the validator rejects user bindings into it.
        // Message-actions state is not yet implemented so no defaults
        // emit these. Match arm exists purely to keep the match exhaustive
        // without a wildcard.
        MessageActionsPrev
        | MessageActionsNext
        | MessageActionsTop
        | MessageActionsBottom
        | MessageActionsPrevUser
        | MessageActionsNextUser
        | MessageActionsEscape
        | MessageActionsCtrlC
        | MessageActionsEnter
        | MessageActionsC
        | MessageActionsP => return None,
    })
}

fn transcript_active(state: &AppState) -> bool {
    matches!(
        state.ui.modal,
        Some(crate::state::ModalState::Transcript(_))
    )
}

#[cfg(test)]
#[path = "keybinding_dispatch.test.rs"]
mod tests;
