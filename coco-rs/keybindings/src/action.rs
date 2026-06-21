//! Keybinding actions — closed enum of all `KEYBINDING_ACTIONS`.
//!
//! Two categories:
//! - 86 publicly-validated actions (user-facing schema).
//! - Internal-only actions (`scroll:*`, `selection:*`, `messageActions:*`)
//!   used by the default bindings but absent from the user-facing schema.
//!
//! Wire format is `namespace:camelCase` (e.g. `"app:exit"`). The `Command`
//! variant captures user `command:foo` bindings (validated against
//! regex `^command:[a-zA-Z0-9:\-_]+$`).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

/// All keybinding actions. Wire format is `namespace:camelCase`.
///
/// Custom (de)serialization via `String` ensures we accept and emit the
/// exact wire strings while still getting compile-time match coverage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum KeybindingAction {
    // ── App-level (Global context) ────────────────────────────────────
    AppInterrupt,
    AppExit,
    AppToggleTodos,
    AppToggleTranscript,
    AppToggleBrief,
    AppToggleTeammatePreview,
    AppToggleTeamRoster,
    AppToggleTerminal,
    AppRedraw,
    AppGlobalSearch,
    AppQuickOpen,
    /// coco-rs extension: immediate quit without the double-press confirmation
    /// `app:exit` goes through. Default `ctrl+q`.
    AppForceQuit,
    /// coco-rs extension: open the help overlay. Default `f1`. (`?` on an
    /// empty composer also opens help, hardcoded in the TUI because it must
    /// fall through to typing otherwise.)
    AppHelp,
    /// coco-rs extension: open the command palette. Default `ctrl+p`;
    /// `history:search` (ctrl+r) opens the same surface.
    AppCommandPalette,
    /// coco-rs extension: open the settings overlay. Default `ctrl+,` (the
    /// conventional settings shortcut).
    AppSettings,
    /// coco-rs extension: open the session browser / resume picker. Default
    /// `ctrl+s` (folded from the old TUI cascade).
    AppSessionBrowser,
    /// coco-rs extension: open the plan editor for the current plan. Default
    /// `ctrl+g` (folded from the old TUI cascade).
    AppPlanEditor,

    // ── History navigation ────────────────────────────────────────────
    HistorySearch,
    HistoryPrevious,
    HistoryNext,

    // ── Chat input ───────────────────────────────────────────────────
    ChatCancel,
    ChatKillAgents,
    ChatCycleMode,
    ChatModelPicker,
    ChatFastMode,
    ChatThinkingToggle,
    /// coco-rs extension: cycle the Main role's thinking effort forward
    /// through the current model's
    /// `supported_thinking_levels`, wrapping at the end. Bound to
    /// `ctrl+t` in Chat context (shadowing the global `app:toggleTodos`
    /// while the user is at the input); `app:toggleTodos` remains
    /// reachable from non-Chat contexts.
    ChatCycleThinking,
    ChatSubmit,
    ChatNewline,
    ChatUndo,
    ChatExternalEditor,
    ChatStash,
    ChatImagePaste,
    ChatMessageActions,
    /// coco-rs extension: toggle `<system-reminder>` visibility in the
    /// transcript. Default `ctrl+shift+r`.
    ChatToggleSystemReminders,
    /// coco-rs extension: toggle plan mode. Default `tab` in Chat — dispatch
    /// is state-dependent (an active inline ghost or prompt suggestion accepts
    /// instead of toggling).
    ChatTogglePlanMode,

    // ── Autocomplete menu ────────────────────────────────────────────
    AutocompleteAccept,
    AutocompleteDismiss,
    AutocompletePrevious,
    AutocompleteNext,

    // ── Confirmation dialogs ──────────────────────────────────────────
    ConfirmYes,
    ConfirmNo,
    ConfirmPrevious,
    ConfirmNext,
    ConfirmNextField,
    ConfirmPreviousField,
    ConfirmCycleMode,
    ConfirmToggle,
    ConfirmToggleExplanation,

    // ── Tabs ─────────────────────────────────────────────────────────
    TabsNext,
    TabsPrevious,

    // ── Transcript viewer ────────────────────────────────────────────
    TranscriptExit,

    // ── History search ───────────────────────────────────────────────
    HistorySearchNext,
    HistorySearchAccept,
    HistorySearchCancel,
    HistorySearchExecute,

    // ── Task ─────────────────────────────────────────────────────────
    TaskBackground,

    // ── Theme picker ─────────────────────────────────────────────────
    ThemeToggleSyntaxHighlighting,

    // ── Help menu ────────────────────────────────────────────────────
    HelpDismiss,

    // ── Attachments ──────────────────────────────────────────────────
    AttachmentsNext,
    AttachmentsPrevious,
    AttachmentsRemove,
    AttachmentsExit,

    // ── Footer indicators ────────────────────────────────────────────
    FooterUp,
    FooterDown,
    FooterNext,
    FooterPrevious,
    FooterOpenSelected,
    FooterClearSelection,
    FooterClose,

    // ── Message selector (rewind) ────────────────────────────────────
    MessageSelectorUp,
    MessageSelectorDown,
    MessageSelectorTop,
    MessageSelectorBottom,
    MessageSelectorSelect,

    // ── Diff dialog ──────────────────────────────────────────────────
    DiffDismiss,
    DiffPreviousSource,
    DiffNextSource,
    DiffBack,
    DiffViewDetails,
    DiffPreviousFile,
    DiffNextFile,

    // ── Model picker ─────────────────────────────────────────────────
    ModelPickerDecreaseEffort,
    ModelPickerIncreaseEffort,

    // ── Select component ─────────────────────────────────────────────
    SelectNext,
    SelectPrevious,
    SelectAccept,
    SelectCancel,

    // ── Plugin dialog ────────────────────────────────────────────────
    PluginToggle,
    PluginInstall,

    // ── Permission dialog ────────────────────────────────────────────
    PermissionToggleDebug,

    // ── Settings ─────────────────────────────────────────────────────
    SettingsSearch,
    SettingsRetry,
    SettingsClose,

    // ── Voice (feature-gated `VOICE_MODE`) ───────────────────────────
    VoicePushToTalk,

    // ── Internal: Scroll context ──────────────────────────────────────
    // Not in the user-facing schema; not user-rebindable, but referenced by defaults.
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollTop,
    ScrollBottom,

    // ── Internal: Selection ──────────────────────────────────────────
    SelectionCopy,

    // ── Internal: MessageActions context (feature-gated `MESSAGE_ACTIONS`) ─
    MessageActionsPrev,
    MessageActionsNext,
    MessageActionsTop,
    MessageActionsBottom,
    MessageActionsPrevUser,
    MessageActionsNextUser,
    MessageActionsEscape,
    MessageActionsCtrlC,
    MessageActionsEnter,
    MessageActionsC,
    MessageActionsP,

    // ── Slash command escape hatch ───────────────────────────────────
    /// `command:foo` user binding. Executes the slash command as if typed.
    /// Only valid in the `Chat` context (validated by `validator`).
    /// Inner string excludes the `command:` prefix.
    Command(String),
}

impl KeybindingAction {
    /// Wire-format string. Cheap (`Cow::Borrowed`) for builtin variants;
    /// allocates only for `Command(_)`.
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::AppInterrupt => Cow::Borrowed("app:interrupt"),
            Self::AppExit => Cow::Borrowed("app:exit"),
            Self::AppToggleTodos => Cow::Borrowed("app:toggleTodos"),
            Self::AppToggleTranscript => Cow::Borrowed("app:toggleTranscript"),
            Self::AppToggleBrief => Cow::Borrowed("app:toggleBrief"),
            Self::AppToggleTeammatePreview => Cow::Borrowed("app:toggleTeammatePreview"),
            Self::AppToggleTeamRoster => Cow::Borrowed("app:toggleTeamRoster"),
            Self::AppToggleTerminal => Cow::Borrowed("app:toggleTerminal"),
            Self::AppRedraw => Cow::Borrowed("app:redraw"),
            Self::AppGlobalSearch => Cow::Borrowed("app:globalSearch"),
            Self::AppQuickOpen => Cow::Borrowed("app:quickOpen"),
            Self::AppForceQuit => Cow::Borrowed("app:forceQuit"),
            Self::AppHelp => Cow::Borrowed("app:help"),
            Self::AppCommandPalette => Cow::Borrowed("app:commandPalette"),
            Self::AppSettings => Cow::Borrowed("app:settings"),
            Self::AppSessionBrowser => Cow::Borrowed("app:sessionBrowser"),
            Self::AppPlanEditor => Cow::Borrowed("app:planEditor"),

            Self::HistorySearch => Cow::Borrowed("history:search"),
            Self::HistoryPrevious => Cow::Borrowed("history:previous"),
            Self::HistoryNext => Cow::Borrowed("history:next"),

            Self::ChatCancel => Cow::Borrowed("chat:cancel"),
            Self::ChatKillAgents => Cow::Borrowed("chat:killAgents"),
            Self::ChatCycleMode => Cow::Borrowed("chat:cycleMode"),
            Self::ChatModelPicker => Cow::Borrowed("chat:modelPicker"),
            Self::ChatFastMode => Cow::Borrowed("chat:fastMode"),
            Self::ChatThinkingToggle => Cow::Borrowed("chat:thinkingToggle"),
            Self::ChatCycleThinking => Cow::Borrowed("chat:cycleThinking"),
            Self::ChatSubmit => Cow::Borrowed("chat:submit"),
            Self::ChatNewline => Cow::Borrowed("chat:newline"),
            Self::ChatUndo => Cow::Borrowed("chat:undo"),
            Self::ChatExternalEditor => Cow::Borrowed("chat:externalEditor"),
            Self::ChatStash => Cow::Borrowed("chat:stash"),
            Self::ChatImagePaste => Cow::Borrowed("chat:imagePaste"),
            Self::ChatMessageActions => Cow::Borrowed("chat:messageActions"),
            Self::ChatToggleSystemReminders => Cow::Borrowed("chat:toggleSystemReminders"),
            Self::ChatTogglePlanMode => Cow::Borrowed("chat:togglePlanMode"),

            Self::AutocompleteAccept => Cow::Borrowed("autocomplete:accept"),
            Self::AutocompleteDismiss => Cow::Borrowed("autocomplete:dismiss"),
            Self::AutocompletePrevious => Cow::Borrowed("autocomplete:previous"),
            Self::AutocompleteNext => Cow::Borrowed("autocomplete:next"),

            Self::ConfirmYes => Cow::Borrowed("confirm:yes"),
            Self::ConfirmNo => Cow::Borrowed("confirm:no"),
            Self::ConfirmPrevious => Cow::Borrowed("confirm:previous"),
            Self::ConfirmNext => Cow::Borrowed("confirm:next"),
            Self::ConfirmNextField => Cow::Borrowed("confirm:nextField"),
            Self::ConfirmPreviousField => Cow::Borrowed("confirm:previousField"),
            Self::ConfirmCycleMode => Cow::Borrowed("confirm:cycleMode"),
            Self::ConfirmToggle => Cow::Borrowed("confirm:toggle"),
            Self::ConfirmToggleExplanation => Cow::Borrowed("confirm:toggleExplanation"),

            Self::TabsNext => Cow::Borrowed("tabs:next"),
            Self::TabsPrevious => Cow::Borrowed("tabs:previous"),

            Self::TranscriptExit => Cow::Borrowed("transcript:exit"),

            Self::HistorySearchNext => Cow::Borrowed("historySearch:next"),
            Self::HistorySearchAccept => Cow::Borrowed("historySearch:accept"),
            Self::HistorySearchCancel => Cow::Borrowed("historySearch:cancel"),
            Self::HistorySearchExecute => Cow::Borrowed("historySearch:execute"),

            Self::TaskBackground => Cow::Borrowed("task:background"),

            Self::ThemeToggleSyntaxHighlighting => Cow::Borrowed("theme:toggleSyntaxHighlighting"),

            Self::HelpDismiss => Cow::Borrowed("help:dismiss"),

            Self::AttachmentsNext => Cow::Borrowed("attachments:next"),
            Self::AttachmentsPrevious => Cow::Borrowed("attachments:previous"),
            Self::AttachmentsRemove => Cow::Borrowed("attachments:remove"),
            Self::AttachmentsExit => Cow::Borrowed("attachments:exit"),

            Self::FooterUp => Cow::Borrowed("footer:up"),
            Self::FooterDown => Cow::Borrowed("footer:down"),
            Self::FooterNext => Cow::Borrowed("footer:next"),
            Self::FooterPrevious => Cow::Borrowed("footer:previous"),
            Self::FooterOpenSelected => Cow::Borrowed("footer:openSelected"),
            Self::FooterClearSelection => Cow::Borrowed("footer:clearSelection"),
            Self::FooterClose => Cow::Borrowed("footer:close"),

            Self::MessageSelectorUp => Cow::Borrowed("messageSelector:up"),
            Self::MessageSelectorDown => Cow::Borrowed("messageSelector:down"),
            Self::MessageSelectorTop => Cow::Borrowed("messageSelector:top"),
            Self::MessageSelectorBottom => Cow::Borrowed("messageSelector:bottom"),
            Self::MessageSelectorSelect => Cow::Borrowed("messageSelector:select"),

            Self::DiffDismiss => Cow::Borrowed("diff:dismiss"),
            Self::DiffPreviousSource => Cow::Borrowed("diff:previousSource"),
            Self::DiffNextSource => Cow::Borrowed("diff:nextSource"),
            Self::DiffBack => Cow::Borrowed("diff:back"),
            Self::DiffViewDetails => Cow::Borrowed("diff:viewDetails"),
            Self::DiffPreviousFile => Cow::Borrowed("diff:previousFile"),
            Self::DiffNextFile => Cow::Borrowed("diff:nextFile"),

            Self::ModelPickerDecreaseEffort => Cow::Borrowed("modelPicker:decreaseEffort"),
            Self::ModelPickerIncreaseEffort => Cow::Borrowed("modelPicker:increaseEffort"),

            Self::SelectNext => Cow::Borrowed("select:next"),
            Self::SelectPrevious => Cow::Borrowed("select:previous"),
            Self::SelectAccept => Cow::Borrowed("select:accept"),
            Self::SelectCancel => Cow::Borrowed("select:cancel"),

            Self::PluginToggle => Cow::Borrowed("plugin:toggle"),
            Self::PluginInstall => Cow::Borrowed("plugin:install"),

            Self::PermissionToggleDebug => Cow::Borrowed("permission:toggleDebug"),

            Self::SettingsSearch => Cow::Borrowed("settings:search"),
            Self::SettingsRetry => Cow::Borrowed("settings:retry"),
            Self::SettingsClose => Cow::Borrowed("settings:close"),

            Self::VoicePushToTalk => Cow::Borrowed("voice:pushToTalk"),

            Self::ScrollPageUp => Cow::Borrowed("scroll:pageUp"),
            Self::ScrollPageDown => Cow::Borrowed("scroll:pageDown"),
            Self::ScrollLineUp => Cow::Borrowed("scroll:lineUp"),
            Self::ScrollLineDown => Cow::Borrowed("scroll:lineDown"),
            Self::ScrollTop => Cow::Borrowed("scroll:top"),
            Self::ScrollBottom => Cow::Borrowed("scroll:bottom"),

            Self::SelectionCopy => Cow::Borrowed("selection:copy"),

            Self::MessageActionsPrev => Cow::Borrowed("messageActions:prev"),
            Self::MessageActionsNext => Cow::Borrowed("messageActions:next"),
            Self::MessageActionsTop => Cow::Borrowed("messageActions:top"),
            Self::MessageActionsBottom => Cow::Borrowed("messageActions:bottom"),
            Self::MessageActionsPrevUser => Cow::Borrowed("messageActions:prevUser"),
            Self::MessageActionsNextUser => Cow::Borrowed("messageActions:nextUser"),
            Self::MessageActionsEscape => Cow::Borrowed("messageActions:escape"),
            Self::MessageActionsCtrlC => Cow::Borrowed("messageActions:ctrlc"),
            Self::MessageActionsEnter => Cow::Borrowed("messageActions:enter"),
            Self::MessageActionsC => Cow::Borrowed("messageActions:c"),
            Self::MessageActionsP => Cow::Borrowed("messageActions:p"),

            Self::Command(name) => Cow::Owned(format!("command:{name}")),
        }
    }

    /// Whether this action is the `command:foo` escape hatch.
    pub fn is_command(&self) -> bool {
        matches!(self, Self::Command(_))
    }
}

impl fmt::Display for KeybindingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// Returned when a string fails to parse as a [`KeybindingAction`].
///
/// Stringly because callers want a user-facing message; surfaced to the
/// validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAction {
    pub raw: String,
    pub reason: UnknownActionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownActionReason {
    /// Not in the closed enum and not a `command:foo` pattern.
    NotARecognizedAction,
    /// Matched the `command:` prefix but the suffix violated the
    /// `^command:[a-zA-Z0-9:\-_]+$` shape.
    InvalidCommandName,
}

impl fmt::Display for UnknownAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            UnknownActionReason::NotARecognizedAction => {
                write!(f, "unknown keybinding action `{}`", self.raw)
            }
            UnknownActionReason::InvalidCommandName => write!(
                f,
                "invalid command binding `{}`: name may only contain \
                 alphanumerics, `:`, `-`, `_`",
                self.raw
            ),
        }
    }
}

impl std::error::Error for UnknownAction {}

impl FromStr for KeybindingAction {
    type Err = UnknownAction;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Slash command escape hatch — keep before the closed-set match so
        // the regex check runs even if a future schema revision adds a literal
        // `command:foo` to KEYBINDING_ACTIONS.
        if let Some(rest) = s.strip_prefix("command:") {
            if rest.is_empty()
                || !rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
            {
                return Err(UnknownAction {
                    raw: s.to_string(),
                    reason: UnknownActionReason::InvalidCommandName,
                });
            }
            return Ok(Self::Command(rest.to_string()));
        }

        let action = match s {
            "app:interrupt" => Self::AppInterrupt,
            "app:exit" => Self::AppExit,
            "app:toggleTodos" => Self::AppToggleTodos,
            "app:toggleTranscript" => Self::AppToggleTranscript,
            "app:toggleBrief" => Self::AppToggleBrief,
            "app:toggleTeammatePreview" => Self::AppToggleTeammatePreview,
            "app:toggleTeamRoster" => Self::AppToggleTeamRoster,
            "app:toggleTerminal" => Self::AppToggleTerminal,
            "app:redraw" => Self::AppRedraw,
            "app:globalSearch" => Self::AppGlobalSearch,
            "app:quickOpen" => Self::AppQuickOpen,
            "app:forceQuit" => Self::AppForceQuit,
            "app:help" => Self::AppHelp,
            "app:commandPalette" => Self::AppCommandPalette,
            "app:settings" => Self::AppSettings,
            "app:sessionBrowser" => Self::AppSessionBrowser,
            "app:planEditor" => Self::AppPlanEditor,

            "history:search" => Self::HistorySearch,
            "history:previous" => Self::HistoryPrevious,
            "history:next" => Self::HistoryNext,

            "chat:cancel" => Self::ChatCancel,
            "chat:killAgents" => Self::ChatKillAgents,
            "chat:cycleMode" => Self::ChatCycleMode,
            "chat:modelPicker" => Self::ChatModelPicker,
            "chat:fastMode" => Self::ChatFastMode,
            "chat:thinkingToggle" => Self::ChatThinkingToggle,
            "chat:cycleThinking" => Self::ChatCycleThinking,
            "chat:submit" => Self::ChatSubmit,
            "chat:newline" => Self::ChatNewline,
            "chat:undo" => Self::ChatUndo,
            "chat:externalEditor" => Self::ChatExternalEditor,
            "chat:stash" => Self::ChatStash,
            "chat:imagePaste" => Self::ChatImagePaste,
            "chat:messageActions" => Self::ChatMessageActions,
            "chat:toggleSystemReminders" => Self::ChatToggleSystemReminders,
            "chat:togglePlanMode" => Self::ChatTogglePlanMode,

            "autocomplete:accept" => Self::AutocompleteAccept,
            "autocomplete:dismiss" => Self::AutocompleteDismiss,
            "autocomplete:previous" => Self::AutocompletePrevious,
            "autocomplete:next" => Self::AutocompleteNext,

            "confirm:yes" => Self::ConfirmYes,
            "confirm:no" => Self::ConfirmNo,
            "confirm:previous" => Self::ConfirmPrevious,
            "confirm:next" => Self::ConfirmNext,
            "confirm:nextField" => Self::ConfirmNextField,
            "confirm:previousField" => Self::ConfirmPreviousField,
            "confirm:cycleMode" => Self::ConfirmCycleMode,
            "confirm:toggle" => Self::ConfirmToggle,
            "confirm:toggleExplanation" => Self::ConfirmToggleExplanation,

            "tabs:next" => Self::TabsNext,
            "tabs:previous" => Self::TabsPrevious,

            "transcript:exit" => Self::TranscriptExit,

            "historySearch:next" => Self::HistorySearchNext,
            "historySearch:accept" => Self::HistorySearchAccept,
            "historySearch:cancel" => Self::HistorySearchCancel,
            "historySearch:execute" => Self::HistorySearchExecute,

            "task:background" => Self::TaskBackground,

            "theme:toggleSyntaxHighlighting" => Self::ThemeToggleSyntaxHighlighting,

            "help:dismiss" => Self::HelpDismiss,

            "attachments:next" => Self::AttachmentsNext,
            "attachments:previous" => Self::AttachmentsPrevious,
            "attachments:remove" => Self::AttachmentsRemove,
            "attachments:exit" => Self::AttachmentsExit,

            "footer:up" => Self::FooterUp,
            "footer:down" => Self::FooterDown,
            "footer:next" => Self::FooterNext,
            "footer:previous" => Self::FooterPrevious,
            "footer:openSelected" => Self::FooterOpenSelected,
            "footer:clearSelection" => Self::FooterClearSelection,
            "footer:close" => Self::FooterClose,

            "messageSelector:up" => Self::MessageSelectorUp,
            "messageSelector:down" => Self::MessageSelectorDown,
            "messageSelector:top" => Self::MessageSelectorTop,
            "messageSelector:bottom" => Self::MessageSelectorBottom,
            "messageSelector:select" => Self::MessageSelectorSelect,

            "diff:dismiss" => Self::DiffDismiss,
            "diff:previousSource" => Self::DiffPreviousSource,
            "diff:nextSource" => Self::DiffNextSource,
            "diff:back" => Self::DiffBack,
            "diff:viewDetails" => Self::DiffViewDetails,
            "diff:previousFile" => Self::DiffPreviousFile,
            "diff:nextFile" => Self::DiffNextFile,

            "modelPicker:decreaseEffort" => Self::ModelPickerDecreaseEffort,
            "modelPicker:increaseEffort" => Self::ModelPickerIncreaseEffort,

            "select:next" => Self::SelectNext,
            "select:previous" => Self::SelectPrevious,
            "select:accept" => Self::SelectAccept,
            "select:cancel" => Self::SelectCancel,

            "plugin:toggle" => Self::PluginToggle,
            "plugin:install" => Self::PluginInstall,

            "permission:toggleDebug" => Self::PermissionToggleDebug,

            "settings:search" => Self::SettingsSearch,
            "settings:retry" => Self::SettingsRetry,
            "settings:close" => Self::SettingsClose,

            "voice:pushToTalk" => Self::VoicePushToTalk,

            "scroll:pageUp" => Self::ScrollPageUp,
            "scroll:pageDown" => Self::ScrollPageDown,
            "scroll:lineUp" => Self::ScrollLineUp,
            "scroll:lineDown" => Self::ScrollLineDown,
            "scroll:top" => Self::ScrollTop,
            "scroll:bottom" => Self::ScrollBottom,

            "selection:copy" => Self::SelectionCopy,

            "messageActions:prev" => Self::MessageActionsPrev,
            "messageActions:next" => Self::MessageActionsNext,
            "messageActions:top" => Self::MessageActionsTop,
            "messageActions:bottom" => Self::MessageActionsBottom,
            "messageActions:prevUser" => Self::MessageActionsPrevUser,
            "messageActions:nextUser" => Self::MessageActionsNextUser,
            "messageActions:escape" => Self::MessageActionsEscape,
            "messageActions:ctrlc" => Self::MessageActionsCtrlC,
            "messageActions:enter" => Self::MessageActionsEnter,
            "messageActions:c" => Self::MessageActionsC,
            "messageActions:p" => Self::MessageActionsP,

            other => {
                return Err(UnknownAction {
                    raw: other.to_string(),
                    reason: UnknownActionReason::NotARecognizedAction,
                });
            }
        };
        Ok(action)
    }
}

// Bridges for `#[serde(try_from = "String", into = "String")]`. Keep these
// non-public so they don't pollute the API surface.

impl TryFrom<String> for KeybindingAction {
    type Error = UnknownAction;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<KeybindingAction> for String {
    fn from(action: KeybindingAction) -> Self {
        action.to_string()
    }
}

#[cfg(test)]
#[path = "action.test.rs"]
mod tests;
