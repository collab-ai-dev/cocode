# coco-keybindings

Keyboard shortcut resolution. Closed enums for contexts and actions
(schema actions + internal actions + documented coco-rs extensions +
`Command` escape hatch — the `KeybindingAction` enum in `action.rs` is
the authoritative list), chord support (`ctrl+x ctrl+k`,
whitespace-separated), JSON config wrapper, validator with severity,
hot-reloading user-config loader, platform-aware display formatting,
crossterm `KeyEvent` adapter.

## Key Types

| Type | Purpose |
|---|---|
| `KeybindingAction` | Closed enum in `namespace:camelCase` — the enum is the variant list; don't trust counts elsewhere. Includes documented coco-rs extensions folded from the old hardcoded TUI cascade (e.g. `app:forceQuit`, `app:commandPalette`, `chat:togglePlanMode`) and a `Command(String)` escape hatch for user `command:foo`. Custom serde via `try_from = "String", into = "String"`. |
| `KeybindingContext` | Closed enum; `ALL_USER` is the user-rebindable subset. The validator rejects user bindings into the internal contexts (`Scroll` / `MessageActions`). |
| `Keybinding` | Parsed binding: `(KeyChord, Option<KeybindingAction>, KeybindingContext)`. `action: None` is a null unbind. |
| `KeybindingBlock` / `KeybindingsConfig` | JSON shapes: `{ $schema, $docs, bindings: Vec<KeybindingBlock> }`. `from_json`, `to_json_pretty`, `parse_bindings`. |
| `KeyChord`, `KeyCombo`, `parse_chord`, `parse_combo`, `ParseError` | Chord parser. Whitespace separates combo steps; `" "` is the space key. |
| `ChordResolver`, `ResolveOutcome` | Chord state machine. Outcomes: `NoMatch`, `Fire(action)`, `Pending`, `Unbound` (null-bound), `ChordCancelled`. Timeout via `tick(now)`; Esc cancels pending. |
| `ValidationIssue`, `Severity`, `ValidationKind`, `validate`, `format_issue` | Typed warnings for keybinding validation. |
| `ReservedShortcut`, `NON_REBINDABLE`, `TERMINAL_RESERVED`, `MACOS_RESERVED`, `get_reserved_shortcuts`, `lookup_reserved` | Reserved-shortcut detection + canonical-form normalization. |
| `DisplayPlatform`, `keystroke_to_string`, `chord_to_display_string`, … | Canonical + platform-aware display rendering for status bar / help. |
| `default_blocks`, `default_config`, `generate_template` | Default bindings table + user-config template (NON_REBINDABLE filtered). |
| **(feature: `crossterm`)** `from_crossterm` | `KeyEvent → KeyCombo` adapter, incl. the escape+meta quirk fix. |
| **(feature: `loader`)** `load_keybindings`, `KeybindingsLoadResult`, `KeybindingsWatcher`, `default_keybindings_path` | Async loader with hot-reload via `coco-file-watch`. |

## Modules

- `action` / `context` — closed enums + serde / `FromStr` / `Display` / descriptions
- `parser` — chord tokenization, combo parsing, `ParseError` (thiserror)
- `resolver` — chord state machine, last-wins ordering, prefix-preference, timeout, Esc cancel
- `validator` — parse errors, duplicates, internal-context-in-user-config, command-outside-chat, voice-on-bare-letter, reserved shortcuts
- `reserved` — non-rebindable + terminal + macOS reserved-shortcut tables
- `display` — canonical/platform-aware keystroke + chord rendering
- `defaults` / `template` — default bindings (platform conditionals) + user-config template generator
- `adapter` (feature: `crossterm`) — crossterm `KeyEvent` → `KeyCombo`
- `loader` (feature: `loader`) — async user-config loader + file watcher

## Cargo features

- `crossterm` — enables `adapter` + `crossterm` dep. Required by TUI consumers.
- `loader` — enables `loader` + `tokio` / `tracing` / `coco-file-watch` / `dirs` deps.

Default features: none — library callers without a TUI/runtime stay lean.

## Deliberately Not Implemented

| Item | Why |
|---|---|
| React keybinding hooks (`useKeybinding`, `useShortcutDisplay`, …) | TEA architecture replaces them with direct dispatch — see the TUI seam below. |
| Ant-only customization gate | User customization is always available in coco-rs. No provider-gating here. |
| Feature-gated default blocks `KAIROS`, `QUICK_SEARCH`, `TERMINAL_PANEL`, `MESSAGE_ACTIONS` | Depend on Anthropic-internal infrastructure coco-rs doesn't ship. Action variants exist in the enum (so user configs parse) but no defaults are emitted; re-add behind a Cargo feature when the capability lands. `VOICE_MODE` is **not** deferred: defaults emit `("f3", voice:pushToTalk)` — coco-rs ships `coco-voice`, and the binding is inert until `Feature::Voice` is enabled. |

## TUI seam (app/tui)

The TUI consumes this crate via three `app/tui/src/` modules:

- `keybinding_resolver.rs` — `KeybindingHandle` (cheap-clone `Arc<RwLock<..>>`)
  wrapping `ChordResolver` + warnings + display platform; lives in
  `AppState.ui.kb_handle` (no process-wide global).
- `keybinding_dispatch.rs` — `dispatch_action(&action, &state) -> Option<TuiCommand>`;
  exhaustive match, no wildcard arm. Actions whose surface coco-rs hasn't
  built return `None` so the key falls through. `Command(name)` →
  `ExecuteSlashCommand(name)`.
- `keybinding_setup.rs` — `install_keybindings()` returns handle + watcher +
  warnings channel (surfaced as toasts).

`keybinding_bridge::map_key` runs the resolver first; a resolver-consumed
keystroke (fire, pending chord, null unbind, chord cancel) never reaches the
hardcoded fallback. Only non-bindable input stays hardcoded: per-surface
navigation maps, readline editing (`keymap/`), `?`-on-empty-composer help,
PageUp/PageDown scrolling, F6 focus cycling. The help overlay and chat
truncation hints render chords via `kb_handle.display_for(...)` so user
re-bindings reflect immediately.

## Conventions

- **Wire format** for actions and contexts: `app:exit`, `Global`, etc.
  Round-trip through serde is lossless.
- **Chord syntax**: combos joined by `+`, chord steps separated by
  whitespace. `" "` (a single space) is the space-key binding.
- **Canonical key names**: `escape`, `enter`, `delete`, `backspace`,
  `pageup`, `pagedown`, `space`. Aliases (`esc`, `return`, `del`, `bs`,
  `pgup`, `pgdn`) normalize at parse time.
- **Last-wins** within a context: later registration of the same chord wins.
- **Context priority**: callers pass an ordered context stack to
  `ChordResolver::feed`; the most-specific context's bindings are
  searched first.
- **Chord timeout**: `resolver::CHORD_TIMEOUT` (`Duration`, 1 s) between
  combos in a multi-combo chord. Drive via `ChordResolver::tick(now)`.
