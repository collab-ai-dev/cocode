//! `/keybindings-help` — customize keyboard shortcuts. Mirrors claude-code's keybindings.ts.
//! Reference tables are static snapshots of coco's keybindings crate; a live generator is deferred.

pub const PROMPT: &str = r#"# Keybindings Skill

Create or modify `~/.coco/keybindings.json` to customize keyboard shortcuts.

## CRITICAL: Read Before Write

**Always read `~/.coco/keybindings.json` first** (it may not exist yet). Merge changes with existing bindings — never replace the entire file.

- Use **Edit** tool for modifications to existing files
- Use **Write** tool only if the file does not exist yet

## File Format

```json
{
  "$schema": "https://www.schemastore.org/claude-code-keybindings.json",
  "$docs": "https://code.claude.com/docs/en/keybindings",
  "bindings": [
    {
      "context": "Chat",
      "bindings": {
        "ctrl+e": "chat:externalEditor"
      }
    }
  ]
}
```

Always include the `$schema` and `$docs` fields.

## Keystroke Syntax

**Modifiers** (combine with `+`):
- `ctrl` (alias: `control`)
- `alt` (aliases: `opt`, `option`) — note: `alt` and `meta` are identical in terminals
- `shift`
- `meta` (aliases: `cmd`, `command`)

**Special keys**: `escape`/`esc`, `enter`/`return`, `tab`, `space`, `backspace`, `delete`, `up`, `down`, `left`, `right`

**Chords**: Space-separated keystrokes, e.g. `ctrl+k ctrl+s` (1-second timeout between keystrokes)

**Examples**: `ctrl+shift+p`, `alt+enter`, `ctrl+k ctrl+n`

## Unbinding Default Shortcuts

Set a key to `null` to remove its default binding:

```json
{
  "context": "Chat",
  "bindings": {
    "ctrl+s": null
  }
}
```

## How User Bindings Interact with Defaults

- User bindings are **additive** — they are appended after the default bindings
- To **move** a binding to a different key: unbind the old key (`null`) AND add the new binding
- A context only needs to appear in the user's file if they want to change something in that context

## Common Patterns

### Rebind a key
To change the external editor shortcut from `ctrl+g` to `ctrl+e`:
```json
{
  "context": "Chat",
  "bindings": {
    "ctrl+g": null,
    "ctrl+e": "chat:externalEditor"
  }
}
```

### Add a chord binding
```json
{
  "context": "Global",
  "bindings": {
    "ctrl+k ctrl+t": "app:toggleTodos"
  }
}
```

## Behavioral Rules

1. Only include contexts the user wants to change (minimal overrides)
2. Validate that actions and contexts are from the known lists below
3. Warn the user proactively if they choose a key that conflicts with reserved shortcuts or common tools like tmux (`ctrl+b`) and screen (`ctrl+a`)
4. When adding a new binding for an existing action, the new binding is additive (existing default still works unless explicitly unbound)
5. To fully replace a default binding, unbind the old key AND add the new one

## Validation

After editing `~/.coco/keybindings.json`, re-read the file and confirm:

- It is valid JSON (a top-level object with a `bindings` array).
- Each block's `context` is one of the recognized contexts (see the Available Contexts table below).
- Each action value is either a recognized action string (see Available Actions) or `null` to unbind.
- No chosen key conflicts with a reserved shortcut (see Reserved Shortcuts) — `error` entries will not work; `warning` entries may conflict with the terminal/OS.

**Errors** prevent bindings from working and must be fixed. **Warnings** indicate potential conflicts but the binding may still work.

## Reserved Shortcuts

### Non-rebindable (errors)
- `ctrl+c` — Cannot be rebound — used for interrupt/exit (hardcoded)
- `ctrl+d` — Cannot be rebound — used for exit (hardcoded)
- `ctrl+m` — Cannot be rebound — identical to Enter in terminals (both send CR)

### Terminal reserved (errors/warnings)
- `ctrl+z` — Unix process suspend (SIGTSTP) (may conflict)
- `ctrl+\` — Terminal quit signal (SIGQUIT) (will not work)

### macOS reserved (errors)
- `cmd+c` — macOS system copy
- `cmd+v` — macOS system paste
- `cmd+x` — macOS system cut
- `cmd+q` — macOS quit application
- `cmd+w` — macOS close window/tab
- `cmd+tab` — macOS app switcher
- `cmd+space` — macOS Spotlight

## Available Contexts

| Context | Description |
| --- | --- |
| `Global` | Active everywhere, regardless of focus |
| `Chat` | When the chat input is focused |
| `Autocomplete` | When autocomplete menu is visible |
| `Confirmation` | When a confirmation/permission dialog is shown |
| `Help` | When the help overlay is open |
| `Transcript` | When viewing the transcript |
| `HistorySearch` | When searching command history (ctrl+r) |
| `Task` | When a task/agent is running in the foreground |
| `ThemePicker` | When the theme picker is open |
| `Settings` | When the settings menu is open |
| `Tabs` | When tab navigation is active |
| `Attachments` | When navigating image attachments in a select dialog |
| `Footer` | When footer indicators are focused |
| `MessageSelector` | When the message selector (rewind) is open |
| `DiffDialog` | When the diff dialog is open |
| `ModelPicker` | When the model picker is open |
| `Select` | When a select/list component is focused |
| `Plugin` | When the plugin dialog is open |

(The internal `Scroll` and `MessageActions` contexts are not user-rebindable and are omitted.)

## Available Actions

The table below lists the actions that ship with a default binding, along with the key(s) and context where they are bound by default. The full enumeration of every action (including feature-gated and internal actions with no default binding) is a deferred live-generator feature.

| Action | Default Key(s) | Context |
| --- | --- | --- |
| `app:interrupt` | `ctrl+c` | Global |
| `app:exit` | `ctrl+d` | Global |
| `app:redraw` | `ctrl+l` | Global |
| `app:toggleTodos` | `ctrl+t` | Global |
| `app:toggleTranscript` | `ctrl+o` | Global |
| `app:toggleTeammatePreview` | `ctrl+shift+o` | Global |
| `app:toggleTeamRoster` | `ctrl+shift+t` | Global |
| `history:search` | `ctrl+r` | Global |
| `app:globalSearch` | `ctrl+shift+f` | Global |
| `app:quickOpen` | `ctrl+shift+p` | Global |
| `app:forceQuit` | `ctrl+q` | Global |
| `app:help` | `f1` | Global |
| `chat:cancel` | `escape` | Chat |
| `chat:killAgents` | `ctrl+x ctrl+k`, `ctrl+f` | Chat |
| `chat:cycleMode` | `shift+tab` | Chat |
| `chat:modelPicker` | `meta+p`, `ctrl+m` | Chat |
| `chat:fastMode` | `meta+o` | Chat |
| `chat:thinkingToggle` | `f2` | Chat |
| `chat:cycleThinking` | `ctrl+y` | Chat |
| `chat:submit` | `enter` | Chat |
| `history:previous` | `up` | Chat |
| `history:next` | `down` | Chat |
| `chat:undo` | `ctrl+_`, `ctrl+shift+-` | Chat |
| `chat:externalEditor` | `ctrl+x ctrl+e`, `ctrl+g` | Chat |
| `chat:stash` | `ctrl+s` | Chat |
| `chat:imagePaste` | `ctrl+v`, `alt+v` | Chat |
| `app:commandPalette` | `ctrl+p` | Chat |
| `app:settings` | `ctrl+,` | Chat |
| `app:sessionBrowser` | `ctrl+s` | Chat |
| `app:planEditor` | `ctrl+g` | Chat |
| `chat:toggleSystemReminders` | `ctrl+shift+r` | Chat |
| `chat:togglePlanMode` | `tab` | Chat |
| `autocomplete:accept` | `tab` | Autocomplete |
| `autocomplete:dismiss` | `escape` | Autocomplete |
| `autocomplete:previous` | `up` | Autocomplete |
| `autocomplete:next` | `down` | Autocomplete |
| `confirm:no` | `escape`, `n` | Settings, Confirmation |
| `select:previous` | `up`, `k`, `ctrl+p` | Settings, Select |
| `select:next` | `down`, `j`, `ctrl+n` | Settings, Select |
| `select:accept` | `space`, `enter` | Settings, Select |
| `settings:close` | `enter` | Settings |
| `settings:search` | `/` | Settings |
| `settings:retry` | `r` | Settings |
| `confirm:yes` | `y` | Confirmation |
| `confirm:toggle` | `enter`, `space` | Confirmation |
| `confirm:previous` | `up` | Confirmation |
| `confirm:next` | `down` | Confirmation |
| `confirm:nextField` | `tab` | Confirmation |
| `confirm:cycleMode` | `shift+tab` | Confirmation |
| `confirm:toggleExplanation` | `ctrl+e` | Confirmation |
| `permission:toggleDebug` | `ctrl+d` | Confirmation |
| `tabs:next` | `tab`, `right` | Tabs |
| `tabs:previous` | `shift+tab`, `left` | Tabs |
| `transcript:exit` | `ctrl+c`, `escape`, `q` | Transcript |
| `historySearch:next` | `ctrl+r` | HistorySearch |
| `historySearch:accept` | `escape`, `tab` | HistorySearch |
| `historySearch:cancel` | `ctrl+c` | HistorySearch |
| `historySearch:execute` | `enter` | HistorySearch |
| `task:background` | `ctrl+b` | Task |
| `theme:toggleSyntaxHighlighting` | `ctrl+t` | ThemePicker |
| `scroll:pageUp` | `pageup` | Scroll |
| `scroll:pageDown` | `pagedown` | Scroll |
| `scroll:top` | `ctrl+home` | Scroll |
| `scroll:bottom` | `ctrl+end` | Scroll |
| `selection:copy` | `ctrl+shift+c`, `cmd+c` | Scroll |
| `help:dismiss` | `escape` | Help |
| `attachments:next` | `right` | Attachments |
| `attachments:previous` | `left` | Attachments |
| `attachments:remove` | `backspace`, `delete` | Attachments |
| `attachments:exit` | `down`, `escape` | Attachments |
| `footer:up` | `up`, `ctrl+p` | Footer |
| `footer:down` | `down`, `ctrl+n` | Footer |
| `footer:next` | `right` | Footer |
| `footer:previous` | `left` | Footer |
| `footer:openSelected` | `enter` | Footer |
| `footer:clearSelection` | `escape` | Footer |
| `messageSelector:up` | `up`, `k`, `ctrl+p` | MessageSelector |
| `messageSelector:down` | `down`, `j`, `ctrl+n` | MessageSelector |
| `messageSelector:top` | `ctrl+up`, `shift+up`, `meta+up`, `shift+k` | MessageSelector |
| `messageSelector:bottom` | `ctrl+down`, `shift+down`, `meta+down`, `shift+j` | MessageSelector |
| `messageSelector:select` | `enter` | MessageSelector |
| `diff:dismiss` | `escape` | DiffDialog |
| `diff:previousSource` | `left` | DiffDialog |
| `diff:nextSource` | `right` | DiffDialog |
| `diff:previousFile` | `up` | DiffDialog |
| `diff:nextFile` | `down` | DiffDialog |
| `diff:viewDetails` | `enter` | DiffDialog |
| `modelPicker:decreaseEffort` | `left` | ModelPicker |
| `modelPicker:increaseEffort` | `right` | ModelPicker |
| `select:cancel` | `escape` | Select |
| `plugin:toggle` | `space` | Plugin |
| `plugin:install` | `i` | Plugin |
"#;
