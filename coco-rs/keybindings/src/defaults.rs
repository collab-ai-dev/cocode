//! Default keybindings.
//!
//! Block-for-block with two exceptions:
//!
//! * Platform-conditional keys (`IMAGE_PASTE_KEY`, `MODE_CYCLE_KEY`) use
//!   `cfg!(target_os = ...)` instead of runtime detection.
//! * Feature-gated blocks (`KAIROS`, `QUICK_SEARCH`, `TERMINAL_PANEL`,
//!   `MESSAGE_ACTIONS`, `VOICE_MODE`) are intentionally skipped — they
//!   depend on Anthropic-internal infrastructure (GrowthBook, etc.) that
//!   coco-rs doesn't ship. Re-add behind a Cargo feature when the
//!   underlying capability lands.

use std::collections::BTreeMap;

use crate::KeybindingAction;
use crate::KeybindingBlock;
use crate::KeybindingContext;
use crate::KeybindingsConfig;

/// Image-paste shortcut. Windows uses `alt+v` because `ctrl+v` is system paste.
#[cfg(target_os = "windows")]
const IMAGE_PASTE_KEY: &str = "alt+v";
#[cfg(not(target_os = "windows"))]
const IMAGE_PASTE_KEY: &str = "ctrl+v";

/// coco-rs extension: the other platform's paste key as a second default
/// binding, mirroring the old hardcoded cascade which accepted both
/// `ctrl+v` and `alt+v` everywhere.
#[cfg(target_os = "windows")]
const IMAGE_PASTE_KEY_ALT: &str = "ctrl+v";
#[cfg(not(target_os = "windows"))]
const IMAGE_PASTE_KEY_ALT: &str = "alt+v";

/// Permission-mode cycle shortcut. Falls back to `meta+m` on Windows without
/// VT mode (we always use `shift+tab` on non-Windows; on Windows we
/// conservatively assume VT mode is available because Node ≥22.17 / Bun
/// ≥1.2.23 enable it).
const MODE_CYCLE_KEY: &str = "shift+tab";

fn make_block<const N: usize>(
    context: KeybindingContext,
    entries: [(&str, KeybindingAction); N],
) -> KeybindingBlock {
    let mut bindings = BTreeMap::new();
    for (chord, action) in entries {
        bindings.insert(chord.to_string(), Some(action));
    }
    KeybindingBlock { context, bindings }
}

/// Return the full default `KeybindingsConfig`. Hot-load merge order:
/// defaults first, user bindings later (last-wins).
pub fn default_config() -> KeybindingsConfig {
    KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: default_blocks(),
    }
}

/// Return the default blocks as a `Vec<KeybindingBlock>`.
///
/// Block order is stable so user expectations (e.g. iteration in `/help`)
/// are consistent.
pub fn default_blocks() -> Vec<KeybindingBlock> {
    vec![
        // ── Global ────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Global,
            [
                // ctrl+c / ctrl+d use special double-press handling at
                // the dispatch layer; they are listed here so the
                // resolver can render them in /help, but the validator
                // refuses user attempts to rebind them (P5 reserved).
                ("ctrl+c", KeybindingAction::AppInterrupt),
                ("ctrl+d", KeybindingAction::AppExit),
                ("ctrl+l", KeybindingAction::AppRedraw),
                ("ctrl+t", KeybindingAction::AppToggleTodos),
                ("ctrl+o", KeybindingAction::AppToggleTranscript),
                ("ctrl+shift+o", KeybindingAction::AppToggleTeammatePreview),
                // coco-rs-only affordance. Sibling of `ctrl+t`; only active
                // when the session has teammates (gated in dispatch).
                ("ctrl+shift+t", KeybindingAction::AppToggleTeamRoster),
                ("ctrl+r", KeybindingAction::HistorySearch),
                // coco-rs doesn't gate on QUICK_SEARCH (the surfaces are
                // part of the base TUI), so these ship unconditionally.
                ("ctrl+shift+f", KeybindingAction::AppGlobalSearch),
                ("ctrl+shift+p", KeybindingAction::AppQuickOpen),
                // coco-rs extensions — folded from the old hardcoded TUI
                // cascade so they are user-rebindable. `app:forceQuit`
                // skips the `app:exit` double-press confirmation;
                // `app:help` is the F1 entry (the `?` shortcut on an
                // empty composer stays hardcoded in the TUI because it
                // must fall through to typing otherwise).
                ("ctrl+q", KeybindingAction::AppForceQuit),
                ("f1", KeybindingAction::AppHelp),
            ],
        ),
        // ── Chat ──────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Chat,
            [
                ("escape", KeybindingAction::ChatCancel),
                // ctrl+x prefix avoids shadowing readline editing keys.
                ("ctrl+x ctrl+k", KeybindingAction::ChatKillAgents),
                (MODE_CYCLE_KEY, KeybindingAction::ChatCycleMode),
                ("meta+p", KeybindingAction::ChatModelPicker),
                ("meta+o", KeybindingAction::ChatFastMode),
                // Use a function key instead of Option/Alt on macOS:
                // many terminals send Option+T as the printable `†`
                // unless users opt into "Option as Meta". F2 avoids
                // readline editing keys and does not insert text.
                ("f2", KeybindingAction::ChatThinkingToggle),
                // coco-rs extension: Ctrl+Y in the Chat context cycles
                // the Main role's thinking effort
                // forward through the active model's
                // `supported_thinking_levels`. Ctrl+T is reserved for the
                // global `app:toggleTodos` view cycle (Chat → Tasks →
                // Subagents), which now wins from every context — the
                // input-edit `Ctrl+Y → yank` fallback only applies in
                // non-Chat contexts via the legacy cascade.
                ("ctrl+y", KeybindingAction::ChatCycleThinking),
                ("enter", KeybindingAction::ChatSubmit),
                ("up", KeybindingAction::HistoryPrevious),
                ("down", KeybindingAction::HistoryNext),
                // Undo: dual binding for legacy + kitty-keyboard protocol terminals.
                ("ctrl+_", KeybindingAction::ChatUndo),
                ("ctrl+shift+-", KeybindingAction::ChatUndo),
                ("ctrl+x ctrl+e", KeybindingAction::ChatExternalEditor),
                ("ctrl+g", KeybindingAction::ChatExternalEditor),
                ("ctrl+s", KeybindingAction::ChatStash),
                (IMAGE_PASTE_KEY, KeybindingAction::ChatImagePaste),
                // coco-rs extensions — folded from the old hardcoded TUI
                // cascade so they are user-rebindable.
                (IMAGE_PASTE_KEY_ALT, KeybindingAction::ChatImagePaste),
                ("ctrl+f", KeybindingAction::ChatKillAgents),
                ("ctrl+m", KeybindingAction::ChatModelPicker),
                ("ctrl+p", KeybindingAction::AppCommandPalette),
                ("ctrl+,", KeybindingAction::AppSettings),
                ("ctrl+s", KeybindingAction::AppSessionBrowser),
                ("ctrl+g", KeybindingAction::AppPlanEditor),
                ("ctrl+shift+r", KeybindingAction::ChatToggleSystemReminders),
                // `tab` toggles plan mode; dispatch is state-dependent (an
                // active inline ghost / prompt suggestion accepts instead).
                ("tab", KeybindingAction::ChatTogglePlanMode),
            ],
        ),
        // ── Autocomplete ──────────────────────────────────────────────
        make_block(
            KeybindingContext::Autocomplete,
            [
                ("tab", KeybindingAction::AutocompleteAccept),
                ("escape", KeybindingAction::AutocompleteDismiss),
                ("up", KeybindingAction::AutocompletePrevious),
                ("down", KeybindingAction::AutocompleteNext),
            ],
        ),
        // ── Settings ──────────────────────────────────────────────────
        make_block(
            KeybindingContext::Settings,
            [
                ("escape", KeybindingAction::ConfirmNo),
                ("up", KeybindingAction::SelectPrevious),
                ("down", KeybindingAction::SelectNext),
                ("k", KeybindingAction::SelectPrevious),
                ("j", KeybindingAction::SelectNext),
                ("ctrl+p", KeybindingAction::SelectPrevious),
                ("ctrl+n", KeybindingAction::SelectNext),
                ("space", KeybindingAction::SelectAccept),
                ("enter", KeybindingAction::SettingsClose),
                ("/", KeybindingAction::SettingsSearch),
                ("r", KeybindingAction::SettingsRetry),
            ],
        ),
        // ── Confirmation ──────────────────────────────────────────────
        make_block(
            KeybindingContext::Confirmation,
            [
                ("y", KeybindingAction::ConfirmYes),
                ("n", KeybindingAction::ConfirmNo),
                ("enter", KeybindingAction::ConfirmToggle),
                ("escape", KeybindingAction::ConfirmNo),
                ("up", KeybindingAction::ConfirmPrevious),
                ("down", KeybindingAction::ConfirmNext),
                ("tab", KeybindingAction::ConfirmNextField),
                ("space", KeybindingAction::ConfirmToggle),
                ("shift+tab", KeybindingAction::ConfirmCycleMode),
                ("ctrl+e", KeybindingAction::ConfirmToggleExplanation),
                ("ctrl+d", KeybindingAction::PermissionToggleDebug),
            ],
        ),
        // ── Tabs ──────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Tabs,
            [
                ("tab", KeybindingAction::TabsNext),
                ("shift+tab", KeybindingAction::TabsPrevious),
                ("right", KeybindingAction::TabsNext),
                ("left", KeybindingAction::TabsPrevious),
            ],
        ),
        // ── Transcript ────────────────────────────────────────────────
        make_block(
            KeybindingContext::Transcript,
            [
                ("ctrl+c", KeybindingAction::TranscriptExit),
                ("escape", KeybindingAction::TranscriptExit),
                ("q", KeybindingAction::TranscriptExit),
            ],
        ),
        // ── HistorySearch ─────────────────────────────────────────────
        make_block(
            KeybindingContext::HistorySearch,
            [
                ("ctrl+r", KeybindingAction::HistorySearchNext),
                ("escape", KeybindingAction::HistorySearchAccept),
                ("tab", KeybindingAction::HistorySearchAccept),
                ("ctrl+c", KeybindingAction::HistorySearchCancel),
                ("enter", KeybindingAction::HistorySearchExecute),
            ],
        ),
        // ── Task ──────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Task,
            [("ctrl+b", KeybindingAction::TaskBackground)],
        ),
        // ── ThemePicker ───────────────────────────────────────────────
        make_block(
            KeybindingContext::ThemePicker,
            [("ctrl+t", KeybindingAction::ThemeToggleSyntaxHighlighting)],
        ),
        // ── Scroll (internal) ─────────────────────────────────────────
        make_block(
            KeybindingContext::Scroll,
            [
                ("pageup", KeybindingAction::ScrollPageUp),
                ("pagedown", KeybindingAction::ScrollPageDown),
                // wheelup/wheeldown have no crossterm equivalent at the
                // KeyEvent layer; included here for completeness.
                ("ctrl+home", KeybindingAction::ScrollTop),
                ("ctrl+end", KeybindingAction::ScrollBottom),
                ("ctrl+shift+c", KeybindingAction::SelectionCopy),
                ("cmd+c", KeybindingAction::SelectionCopy),
            ],
        ),
        // ── Help ──────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Help,
            [("escape", KeybindingAction::HelpDismiss)],
        ),
        // ── Attachments ───────────────────────────────────────────────
        make_block(
            KeybindingContext::Attachments,
            [
                ("right", KeybindingAction::AttachmentsNext),
                ("left", KeybindingAction::AttachmentsPrevious),
                ("backspace", KeybindingAction::AttachmentsRemove),
                ("delete", KeybindingAction::AttachmentsRemove),
                ("down", KeybindingAction::AttachmentsExit),
                ("escape", KeybindingAction::AttachmentsExit),
            ],
        ),
        // ── Footer ────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Footer,
            [
                ("up", KeybindingAction::FooterUp),
                ("ctrl+p", KeybindingAction::FooterUp),
                ("down", KeybindingAction::FooterDown),
                ("ctrl+n", KeybindingAction::FooterDown),
                ("right", KeybindingAction::FooterNext),
                ("left", KeybindingAction::FooterPrevious),
                ("enter", KeybindingAction::FooterOpenSelected),
                ("escape", KeybindingAction::FooterClearSelection),
            ],
        ),
        // ── MessageSelector ───────────────────────────────────────────
        make_block(
            KeybindingContext::MessageSelector,
            [
                ("up", KeybindingAction::MessageSelectorUp),
                ("down", KeybindingAction::MessageSelectorDown),
                ("k", KeybindingAction::MessageSelectorUp),
                ("j", KeybindingAction::MessageSelectorDown),
                ("ctrl+p", KeybindingAction::MessageSelectorUp),
                ("ctrl+n", KeybindingAction::MessageSelectorDown),
                ("ctrl+up", KeybindingAction::MessageSelectorTop),
                ("shift+up", KeybindingAction::MessageSelectorTop),
                ("meta+up", KeybindingAction::MessageSelectorTop),
                ("shift+k", KeybindingAction::MessageSelectorTop),
                ("ctrl+down", KeybindingAction::MessageSelectorBottom),
                ("shift+down", KeybindingAction::MessageSelectorBottom),
                ("meta+down", KeybindingAction::MessageSelectorBottom),
                ("shift+j", KeybindingAction::MessageSelectorBottom),
                ("enter", KeybindingAction::MessageSelectorSelect),
            ],
        ),
        // ── DiffDialog ────────────────────────────────────────────────
        make_block(
            KeybindingContext::DiffDialog,
            [
                ("escape", KeybindingAction::DiffDismiss),
                ("left", KeybindingAction::DiffPreviousSource),
                ("right", KeybindingAction::DiffNextSource),
                ("up", KeybindingAction::DiffPreviousFile),
                ("down", KeybindingAction::DiffNextFile),
                ("enter", KeybindingAction::DiffViewDetails),
            ],
        ),
        // ── ModelPicker ───────────────────────────────────────────────
        make_block(
            KeybindingContext::ModelPicker,
            [
                ("left", KeybindingAction::ModelPickerDecreaseEffort),
                ("right", KeybindingAction::ModelPickerIncreaseEffort),
            ],
        ),
        // ── Select ────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Select,
            [
                ("up", KeybindingAction::SelectPrevious),
                ("down", KeybindingAction::SelectNext),
                ("j", KeybindingAction::SelectNext),
                ("k", KeybindingAction::SelectPrevious),
                ("ctrl+n", KeybindingAction::SelectNext),
                ("ctrl+p", KeybindingAction::SelectPrevious),
                ("enter", KeybindingAction::SelectAccept),
                ("escape", KeybindingAction::SelectCancel),
            ],
        ),
        // ── Plugin ────────────────────────────────────────────────────
        make_block(
            KeybindingContext::Plugin,
            [
                ("space", KeybindingAction::PluginToggle),
                ("i", KeybindingAction::PluginInstall),
            ],
        ),
    ]
}

#[cfg(test)]
#[path = "defaults.test.rs"]
mod tests;
