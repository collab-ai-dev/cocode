# coco-keybindings — Crate Plan

TS source: `keybindings/` (12 files, 2.6K LOC)

## Dependencies

```
coco-keybindings depends on:
  - coco-types, serde, serde_json

coco-keybindings does NOT depend on:
  - coco-tools, coco-query, coco-inference, any app/ crate
```

## Data Definitions

```rust
pub struct Keybinding {
    pub key: String,          // e.g. "ctrl+s", "ctrl+shift+p", "ctrl+x ctrl+k" (chord)
    pub action: KeybindingAction,
    pub context: KeybindingContext,
}

/// 18 keybinding contexts (from schema.ts).
/// Determines when a binding is active.
pub enum KeybindingContext {
    Global, Chat, Autocomplete, Confirmation, Help,
    Transcript, HistorySearch, Task, ThemePicker,
    Settings, Tabs, Attachments, Footer, MessageSelector,
    DiffDialog, ModelPicker, Select, Plugin,
}

/// 73+ keybinding actions (from schema.ts).
/// IMPORTANT: Action names use exact `namespace:camelCase` format from TS.
/// These are string-based in TS; Rust uses an enum but must serialize to these exact names.
///
/// app:* (10): app:exit, app:help, app:toggleVerbose, app:toggleTheme,
///   app:cyclePermissionMode, app:showCommandPalette, app:showModelPicker,
///   app:showSessionBrowser, app:showTasks, app:toggleThinking,
///   (feature-gated: app:toggleBrief, app:globalSearch, app:quickOpen, app:toggleTerminal)
/// chat:* (15): chat:submit, chat:newline, chat:clear, chat:compact,
///   chat:interrupt, chat:paste, chat:imagePaste, chat:up, chat:down,
///   chat:historyPrev, chat:historyNext, chat:undo, chat:redo,
///   chat:selectAll, chat:backgroundAll (feature-gated: chat:messageActions)
/// history:* (3): historySearch:next, historySearch:prev, historySearch:submit, historySearch:cancel
/// autocomplete:* (4): autocomplete:accept, autocomplete:next, autocomplete:prev, autocomplete:dismiss
/// confirm:* (9): confirm:yes, confirm:no, confirm:always, confirm:yesAndRemember,
///   confirm:explain, confirm:openDiff, confirm:retry, confirm:editInput, confirm:viewOutput
/// tabs:* (2): tabs:next, tabs:prev
/// transcript:* (2): transcript:toggleSearch, transcript:copy
/// task:* (1): task:stop
/// theme:* (1): theme:confirm
/// help:* (1): help:dismiss
/// attachments:* (4): attachments:up, attachments:down, attachments:select, attachments:dismiss
/// footer:* (6): footer:up, footer:down, footer:select, footer:dismiss, footer:toggleExpand, footer:tab
/// messageSelector:* (5): messageSelector:up, messageSelector:down, messageSelector:select, messageSelector:dismiss, messageSelector:confirm
/// diff:* (6): diff:accept, diff:reject, diff:nextFile, diff:prevFile, diff:nextHunk, diff:prevHunk
/// modelPicker:* (2): modelPicker:select, modelPicker:dismiss
/// select:* (4): select:up, select:down, select:select, select:dismiss
/// plugin:* (2): plugin:install, plugin:uninstall
/// permission:* (1): permission:feedback
/// settings:* (3): settings:up, settings:down, settings:select
/// voice:* (1): voice:pushToTalk (feature-gated: VOICE_MODE)
pub type KeybindingAction = String; // Exact "namespace:action" string; validated at load time
```

## Core Logic

```rust
/// Loading order: platform defaults → user overrides (~/.claude/keybindings.json).
/// User bindings merged over defaults (override, not replace).
pub fn load_keybindings() -> Vec<Keybinding>;
pub fn save_keybindings(bindings: &[Keybinding]);

/// Chord support: "ctrl+x ctrl+k" notation.
/// Parser splits on space, each segment is a key combo.
/// Ambiguity detection: warns if chord prefix conflicts with single-key binding.
pub fn parse_shortcut(shortcut: &str) -> Vec<KeyCombo>;

/// Platform-specific defaults (from defaultBindings.ts 340 LOC):
/// IMAGE_PASTE_KEY: alt+v (Windows) | ctrl+v (other)
/// MODE_CYCLE_KEY: shift+tab (VT mode) | meta+m (Windows without VT)
pub fn get_platform_defaults() -> Vec<Keybinding>;

/// Reserved shortcuts: ctrl+c (interrupt) and ctrl+d (exit).
/// Double-press semantics: first press = soft action, second within 500ms = hard action.
pub fn is_reserved_shortcut(key: &KeyCombo) -> bool;

/// Key event matching: compare incoming key event against registered bindings
/// for the current active context.
pub fn match_keybinding(
    event: &KeyEvent,
    context: KeybindingContext,
    bindings: &[Keybinding],
) -> Option<KeybindingAction>;
```
