# coco-keybindings

Keyboard shortcut resolution. Closed enums for contexts (20: 18
user-rebindable + 2 internal) and actions (~113 variants: 86 schema + 18
internal + 8 documented coco-rs extensions (`app:forceQuit`, `app:help`,
`app:commandPalette`, `app:settings`, `app:sessionBrowser`,
`app:planEditor`, `chat:toggleSystemReminders`, `chat:togglePlanMode` —
folded from the old hardcoded TUI cascade) + Command escape hatch), chord
support (`ctrl+x ctrl+k`,
whitespace-separated), JSON config wrapper, validator with severity,
hot-reloading user-config loader, platform-aware display formatting,
crossterm `KeyEvent` adapter.

## Key Types

| Type | Purpose |
|---|---|
| `KeybindingAction` | Closed enum, ~111 variants (86 schema + 18 internal + 6 coco extensions + Command escape hatch) in `namespace:camelCase`. `Command(String)` escape hatch for user `command:foo`. Custom serde via `try_from = "String", into = "String"`. |
| `KeybindingContext` | Closed enum, 20 variants. `ALL_USER` → 18 user-rebindable; the validator rejects user bindings into `Scroll` / `MessageActions`. |
| `Keybinding` | Parsed binding: `(KeyChord, Option<KeybindingAction>, KeybindingContext)`. `action: None` is a null unbind. |
| `KeybindingBlock` | One block from JSON: `(KeybindingContext, BTreeMap<String, Option<KeybindingAction>>)`. |
| `KeybindingsConfig` | Top-level JSON shape: `{ $schema, $docs, bindings: Vec<KeybindingBlock> }`. `from_json`, `to_json_pretty`, `parse_bindings`. |
| `KeyChord`, `KeyCombo`, `parse_chord`, `parse_combo`, `ParseError` | Chord parser. Whitespace separates combo steps; `" "` is the space key. |
| `ChordResolver`, `ResolveOutcome` | Chord state machine. Outcomes: `NoMatch`, `Fire(action)`, `Pending`, `Unbound` (null-bound), `ChordCancelled`. 1s timeout via `tick(now)`. Esc cancels pending. |
| `ValidationIssue`, `Severity`, `ValidationKind`, `validate`, `format_issue` | Typed warnings for keybinding validation. Five kinds × two severities. |
| `ReservedShortcut`, `NON_REBINDABLE`, `TERMINAL_RESERVED`, `MACOS_RESERVED`, `get_reserved_shortcuts`, `lookup_reserved`, `normalize_key_for_comparison` | Reserved-shortcut detection. |
| `DisplayPlatform`, `keystroke_to_string`, `keystroke_to_display_string`, `chord_to_string`, `chord_to_display_string` | Canonical + platform-aware display rendering for status bar / help. |
| `default_blocks`, `default_config` | Default bindings table. |
| `generate_template` | User-config template derived from defaults, NON_REBINDABLE filtered. |
| **(feature: `crossterm`)** `from_crossterm` | `KeyEvent → KeyCombo` adapter, including the escape+meta quirk fix. |
| **(feature: `loader`)** `load_keybindings`, `KeybindingsLoadResult`, `KeybindingsWatcher`, `default_keybindings_path` | Async loader with hot-reload via `coco-file-watch`. |

## Modules

- `action` — KeybindingAction enum + `FromStr` / `Display` / serde
- `context` — KeybindingContext enum + `ALL` / `ALL_USER` / `description()`
- `parser` — chord tokenization, combo parsing, `ParseError` (thiserror)
- `resolver` — chord state machine, last-wins ordering, prefix-preference, 1s timeout, Esc cancel
- `validator` — config validation: parse errors, duplicates, internal-context-in-user-config, command-outside-chat, voice-on-bare-letter, reserved shortcuts
- `reserved` — non-rebindable + terminal + macOS reserved shortcut tables; canonical-form normalization
- `display` — canonical/platform-aware keystroke + chord rendering
- `defaults` — default bindings table with platform conditionals
- `template` — user-config template generator (filters NON_REBINDABLE)
- `adapter` (feature: `crossterm`) — crossterm `KeyEvent` → `KeyCombo`
- `loader` (feature: `loader`) — async user-config loader + file watcher

## Cargo features

- `crossterm` — enables `adapter` module + `crossterm` dep. Required by TUI consumers.
- `loader` — enables `loader` module + `tokio` / `tracing` / `coco-file-watch` / `dirs` deps.

Default features: none. Library callers without a TUI/runtime stay lean.

## Deliberately Not Implemented

| Item | Why |
|---|---|
| React keybinding hooks (`useKeybinding`, `useShortcutDisplay`, `KeybindingContext`, `KeybindingProviderSetup`) | TEA architecture replaces them with direct dispatch. Equivalent state lives in `app/tui/src/keybinding_resolver.rs` (chord state) + `keybinding_dispatch.rs` (action → TuiCommand). |
| Ant-only customization gate | User customization is always available in coco-rs. No provider-gating here. |
| Feature-gated default-binding blocks (`KAIROS`, `QUICK_SEARCH`, `TERMINAL_PANEL`, `MESSAGE_ACTIONS`, `VOICE_MODE`) | Skipped pending the underlying capability. Re-add behind a Cargo feature when relevant. |

## TUI integration

The TUI consumes `coco-keybindings` via three modules in `app/tui/src/`:

- `keybinding_resolver.rs` — defines the `KeybindingHandle` (cheap-clone
  `Arc<RwLock<HandleInner>>`) wrapping `ChordResolver` + warnings + display
  platform. Lives in `AppState.ui.kb_handle`; tests get their own per
  `AppState::new()`. No process-wide global — `cargo test --lib` runs
  without `serial_test` guards.
- `keybinding_dispatch.rs` — `dispatch_action(&action, &state) -> Option<TuiCommand>`.
  Match is exhaustive over all 105 schema variants (no wildcard arm).
  Real implementations for the user-facing chat / overlay / app actions
  (including `app:toggleTodos` → cycle expanded view, `app:toggleTranscript`
  → transcript overlay, `app:toggleTeammatePreview` → toggle preview).
  `Command(name)` → `ExecuteSlashCommand(name)` is the user-binding
  escape hatch. Feature-gated actions whose surface coco-rs hasn't built
  (Plugin*, Voice*, MessageActions:*, etc.) silently return None —
  unimplemented features have no registered handler so the key falls through.
- `keybinding_setup.rs` — `install_keybindings()` returns a
  `KeybindingSetup { watcher, warnings_rx, initial, handle }`. Caller
  installs `handle` into `app.state.ui.kb_handle`, plumbs `warnings_rx`
  into the App's tokio::select! loop for toast surfacing, and holds
  `watcher` alive for the TUI's lifetime.

`keybinding_bridge::map_key(state, key)` runs the resolver via
`state.ui.kb_handle.resolve_key(...)` first; if the resolver fires an
action with a TUI handler it wins. If the resolver consumed the
keystroke (chord cancelled, null unbind, pending chord), the keystroke
is swallowed — preventing the hardcoded fallback from doing something a
user-customized binding wouldn't do. The former "TUI-only shortcut"
cascade has been folded into defaults as documented coco-rs extension
actions (tui-v2 Stage 4); what remains hardcoded is only what cannot be
a binding: per-surface navigation maps (picker/confirmation/question/…
Up/Down/Enter/filter chars), readline input editing (documented in
`keymap/`), `?`-on-empty-composer help (must fall through to typing),
PageUp/PageDown scrolling, and F6 focus cycling.

The help overlay (`render_overlays/help.rs`) renders shortcuts dynamically
via `state.ui.kb_handle.display_for(action, ctx)` so user re-bindings
reflect immediately. i18n keys carry only the description text.

The chat widget (`widgets/chat/render_system.rs`) takes
`Option<&'a KeybindingHandle>` via builder so the truncation hint
(`…(<chord> to see full summary)`) reflects the user's actual binding.

## Conventions

- **Wire format** for actions and contexts: `app:exit`, `Global`, etc.
  Round-trip through serde is lossless.
- **Chord syntax**: combos joined by `+`, chord steps separated by
  whitespace. `" "` (a single space) is the space-key binding.
- **Canonical key names**: `escape`, `enter`, `delete`, `backspace`,
  `pageup`, `pagedown`, `space`. Aliases (`esc`, `return`, `del`, `bs`,
  `pgup`, `pgdn`) normalize to the canonical form at parse time.
- **Last-wins** within a context: registering the same chord twice with
  different actions, the later registration wins.
- **Context priority**: callers pass an ordered context stack to
  `ChordResolver::feed`; the most-specific context's bindings are
  searched first.
- **Chord timeout**: 1 second between combos in a multi-combo chord
  (`CHORD_TIMEOUT_MS = 1000`). Drive via `ChordResolver::tick(now)`.

## Tests

Per-module `*.test.rs` companion files. `cargo test -p coco-keybindings
--all-features --lib` runs all 96.

## Known Follow-ups

(none — all G3 / A2 / N4 follow-ups landed; see the ResolverResult test
suite + `keybinding_dispatch.test.rs` for coverage)

## Deferred feature-gated bindings

These blocks are deferred until a Cargo feature wires the underlying capability:

- `KAIROS` / `KAIROS_BRIEF` → `app:toggleBrief` (Anthropic-internal feature)
- `QUICK_SEARCH` → `app:globalSearch`, `app:quickOpen`
- `TERMINAL_PANEL` → `app:toggleTerminal`
- `MESSAGE_ACTIONS` → `chat:messageActions` + 11 actions in MessageActions context
- `VOICE_MODE` → `voice:pushToTalk` (needs SoX + microphone probes)

Action variants for these are present in the enum (so user configs can
parse) but no defaults are emitted.
