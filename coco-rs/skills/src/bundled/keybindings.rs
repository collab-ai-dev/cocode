//! `/keybindings-help` — customize keyboard shortcuts.

use std::collections::BTreeMap;

pub fn prompt() -> String {
    let keybindings_path = format!(
        "~/{}/keybindings.json",
        coco_utils_common::COCO_CONFIG_DIR_NAME
    );
    TEMPLATE
        .replace("__KEYBINDINGS_PATH__", &keybindings_path)
        .replace("__AVAILABLE_CONTEXTS__", &available_contexts_table())
        .replace("__DEFAULT_BINDINGS__", &default_bindings_table())
}

fn available_contexts_table() -> String {
    coco_keybindings::KeybindingContext::ALL_USER
        .iter()
        .map(|context| format!("| `{context}` | {} |", context.description()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn default_bindings_table() -> String {
    let mut rows = Vec::new();
    for block in coco_keybindings::defaults::default_blocks()
        .into_iter()
        .filter(|block| block.context.is_user_rebindable())
    {
        let mut chords_by_action: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (chord, action) in block.bindings {
            let Some(action) = action else {
                continue;
            };
            chords_by_action
                .entry(action.to_string())
                .or_default()
                .push(format!("`{chord}`"));
        }
        rows.extend(chords_by_action.into_iter().map(|(action, chords)| {
            format!("| `{action}` | {} | {} |", chords.join(", "), block.context)
        }));
    }
    rows.join("\n")
}

const TEMPLATE: &str = r#"# Keybindings Skill

Create or modify `__KEYBINDINGS_PATH__` to customize keyboard shortcuts.

## CRITICAL: Read Before Write

**Always read `__KEYBINDINGS_PATH__` first** (it may not exist yet). Merge changes with existing bindings — never replace the entire file.

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

After editing `__KEYBINDINGS_PATH__`, re-read the file and confirm:

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
__AVAILABLE_CONTEXTS__

(The internal `Scroll` and `MessageActions` contexts are not user-rebindable and are omitted.)

## Available Actions

The table below lists the actions that ship with a default binding, along with the key(s) and context where they are bound by default. The full enumeration of every action (including feature-gated and internal actions with no default binding) is a deferred live-generator feature.

| Action | Default Key(s) | Context |
| --- | --- | --- |
__DEFAULT_BINDINGS__
"#;

#[cfg(test)]
#[path = "keybindings.test.rs"]
mod tests;
