# Coco TUI Terminal Surface Design

Status: foundational terminal-surface design for the final agent console.

This document defines the native-scrollback terminal surface, core layout
constraints, and non-negotiable rendering invariants. The product-level final
target is `agent-console-design.md`. If the two documents appear to conflict,
`agent-console-design.md` owns product scope and `terminal-surface-design.md`
owns terminal surface constraints.

## Document Map

| Document | Role |
|---|---|
| `agent-console-design.md` | Complete target agent-console architecture on TEA / `AppState`; highest-level product target. |
| `terminal-surface-design.md` | Native-scrollback terminal surface, technical choices, and non-negotiable rendering constraints. |
| `native-scrollback-architecture.md` | Detailed terminal backend and source-backed scrollback design. |
| `codex-rs-tui-comparison.md` | Deep comparison of `codex-rs/tui` visible UI capability versus coco boundaries. |
| `rendering-hardening-and-rollback.md` | Landed cursor/suspend hardening plus rollback record for failed inline attempts. |
| `viewport-seating-regression-fixes.md` | Technical-decision record + fix plan for the owned-top viewport-seating refactor (C1 overflow-shrink regression and hardening). |
| `tui-v2-design.md` | Forward target: three-way comparison (coco / codex-rs / jcode), anchored finalize without fingerprint reconciliation, engine-owned seating, staged Scope B implementation plan. |
| `migration-roadmap.md` | Historical implementation notes; not the final product organization. |
| `../crate-coco-tui.md` | Current implementation baseline for the `coco-tui` crate. |

## Terminal Surface Target

Deliver one long-lived terminal UI implementation:

- native terminal scrollback is the only long-term base surface;
- finalized conversation history behaves like normal terminal output;
- composer, status, live stream, and prompts remain interactive at the bottom;
- large overlays may temporarily use alt-screen, but fullscreen alt-screen is
  not retained as a compatibility base UI;
- terminal rows are always a projection of structured application state;
- every terminal side effect is owned by the TUI surface layer;
- the UI is provider-neutral and uses structured model/provider/role state.

The goal is not to ship a partial native-scrollback MVP. Implementation can be
staged for review, but the native UI is not complete until terminal ownership,
history insertion, source-backed resize replay, overlay placement, cursor
policy, suspend/resume, and terminal-matrix tests all work together.

## Final Surface Model

```text
CoreEvent / terminal input
        -> update
        -> AppState
        -> presentation view models
        -> surface terminal
        -> native scrollback + bottom interactive viewport
```

`AppState` is the source of facts. Presentation view models convert state into
rows, labels, badges, scroll windows, and semantic styles. Widgets render view
models. Render code does not open files, spawn processes, mutate config, call
the network, or send user commands.

The final visible layout is:

1. terminal-native finalized transcript above the viewport;
2. optional live stream / running tool tail in the bottom viewport;
3. active banners;
4. composer/input;
5. status row;
6. inline composer and decision surfaces;
7. alt-screen overlays only for large surfaces that must not overwrite history.

There is no right-side tool execution rail in the final UI. Tool state appears
inline as live activity or finalized transcript rows. Model and thinking-effort
changes are visible in the header/status bar and are not duplicated as success
toasts.

## Technical Choices

| Area | Decision |
|---|---|
| Terminal backend | Build a coco-owned `SurfaceTerminal` adapted from `codex-rs/tui` terminal mechanics. |
| Widget rendering | Keep ratatui for structured widgets, frames, spans, and snapshots. |
| Ratatui features | Follow the canonical policy in `native-scrollback-architecture.md`; this design does not duplicate the feature table. |
| Native scrollback insertion | Insert finalized rows above the retained viewport with explicit terminal control. |
| Resize behavior | Rebuild from `SessionState.messages` / transcript cells, never from terminal contents. |
| Source model | Introduce typed transcript/history cells before terminal emission. |
| Cursor policy | Keep a single cursor claim policy; widgets do not set terminal cursor directly. |
| Suspend/resume | Surface-aware Ctrl+Z leaves terminal modes cleanly and restores from source. |
| Overlays | Large overlays temporarily enter alt-screen; small decision prompts stay inline only when they need scrollback context and can be made attention-safe. |
| Mouse | Do not enable mouse capture; native terminal selection and wheel scroll are preferred. |
| Config | Native scrollback settings live under TUI/display config, not `Feature`. No long-term surface mode switch. |
| Testing | Combine unit tests, ratatui snapshots, VT100 tests, and PTY/manual terminal matrix checks. |

Use `codex-rs/tui` as a terminal-mechanics and visible-UI behavior reference,
not as a module boundary to copy blindly. The deep comparison lives in
`codex-rs-tui-comparison.md`; the target console design lives in
`agent-console-design.md`. The coco implementation owns its own source model,
overlay policy, config shape, and verification gates.

## Core Contracts

### History Contract

- Only finalized history enters native scrollback.
- Streaming deltas, running tool progress, permission prompts, toasts,
  autocomplete, command palette, and status rows stay out of scrollback.
- A message is history-eligible only after it has a stable `ChatMessage.id` and
  no longer depends on active stream/tool state.
- Rewind, truncate, clear, session switch, and stream consolidation reconcile
  against the message source and request replay when append-only emission is no
  longer correct.
- Terminal scrollback is disposable projection; it is never parsed as source.

### Interactive Viewport Contract

- The viewport contains live state and input, not committed transcript history.
- It may show live stream tail, running tool tail, active banners, composer
  popups, decision prompts, toasts, input, and status.
- It must not redraw finalized messages already emitted to native scrollback.
- It must not shrink long tool output by adding an automatic side rail.

### Overlay Contract

- Help, transcript, diff, settings, model picker, task detail, memory picker,
  global search, and session browser are large overlays and may use alt-screen.
- Permission, question, feedback, and cost warning prompts stay inline only when
  they fit, preserving scrollback context is useful, and the prompt can be made
  visible to the user.
- Every overlay class declares its surface placement.
- Entering and leaving alt-screen saves/restores inline viewport geometry and
  defers pending history rows safely.
- Blocking prompts must not become invisible when the user has scrolled native
  scrollback away from the retained viewport; if visibility is unknown, choose an
  attention-safe local/alt-screen placement plus bell/status banner.
- Visibility is unknown by default because terminals do not expose native
  scrollback viewport position. Treat it as known only after a recent
  app-directed key/focus/open action followed by a successful draw, within a
  short window such as 2 seconds.

### Model And Provider Contract

- Production model picker rows come from `SessionState.model_catalog`.
- Provider availability comes from `SessionState.provider_statuses`.
- Current bindings come from `SessionState.model_by_role`.
- Changes emit `UserCommand::SetModelRole { role, provider, model_id, effort }`.
- `ModelRole::Subagent` is the default LLM binding for subagent execution, not a
  display-only pseudo-role.
- No production UI infers provider from model id prefixes.
- No production UI hardcodes Anthropic model families as control flow.
- Role selection goes through `ModelRole`; add a variant instead of a raw string.

## Removed Complexity

These are deliberate removals, not deferred features:

- fullscreen alt-screen as a long-term base UI;
- user-facing `--alt-screen` / `--native-scrollback` mode switch;
- automatic right-side tool execution list;
- model/thinking success toasts;
- stock ratatui inline viewport without custom terminal ownership;
- streaming deltas written directly into terminal scrollback;
- terminal contents as replay source;
- mouse capture for transcript navigation.

## Failed Decisions To Avoid

| Failed or Rejected Approach | Why it failed |
|---|---|
| Stock ratatui `Viewport::Inline` only | It hides viewport geometry, cursor, diff, and history insertion state needed by this app. |
| Dynamic inline viewport resize | ratatui recomputed geometry from construction-time state and could not keep chrome pinned at the bottom. |
| Fullscreen inline viewport plus `insert_before` | Streaming rows scrolled into terminal history, then finalized rows were inserted again, duplicating turns. |
| Rendering history both in scrollback and viewport | Creates duplicate messages and incoherent scroll position. |
| Emitting streaming deltas into scrollback | Streaming rows are transient and cannot be replayed correctly after width changes. |
| Selective scrollback repair | Terminals do not provide a reliable delete-only-my-rows primitive. |
| Keeping fullscreen alt-screen fallback | Maintains two UI implementations and weakens the native-scrollback architecture. |
| Right-side tool execution rail | Duplicates inline tool state, narrows transcript/tool output, and cannot survive native scrollback replay. |
| Success toasts for model/thinking changes | Header/status already show the state; toast noise competes with diagnostic notifications. |

## Delivery Gates

Native scrollback is not complete until all gates pass:

- custom terminal owns viewport geometry, buffers, cursor style, and diff
  invalidation;
- S1 records the decision between crates.io ratatui, a coco-owned ratatui fork,
  raw crossterm with coco buffer diff, or deferred native scrollback before
  presentation work depends on the terminal substrate;
- history emission is exactly-once for append-only growth;
- truncate/divergence/clear/session switch schedule source-backed replay;
- resize replay rebuilds from transcript cells with an internal row cap,
  message-boundary truncation, and an explicit compact marker when capped;
- overlays restore inline viewport geometry and pending history rows;
- alt-screen entry does not interleave with active source-backed reflow;
- panic/drop cleanup resets scroll region, alt-screen, cursor visibility/style,
  bracketed paste, and focus mode on best-effort paths;
- Ctrl+Z / `fg` works in native mode and overlay mode;
- focus regain always reclaims or hides the cursor correctly;
- finalized history scrolls with terminal wheel/trackpad and supports native
  selection/copy;
- long tool output remains readable without a side rail;
- macOS Terminal.app, iTerm2, tmux, Zellij, Linux terminal, and SSH manual
  matrix pass;
- `cargo test -p coco-tui`, focused snapshots, `git diff --check`, and
  `just quick-check` pass before commit.

## Completion Work Items

Use these merge boundaries. They are verification boundaries, not product
milestones and not a separate phased product roadmap:

1. Final surface substrate: coco-owned terminal, frame, cursor claim, and
   suspend invariants.
2. History source contract: typed transcript/history cells over
   `SessionState.messages`.
3. History insertion and emitter: native row insertion and exactly-once prefix
   tracking.
4. Interactive viewport and overlay placement: remove finalized history from
   the viewport and enforce inline/alt-screen surface policy.
5. Source-backed replay: resize, clear, rewind, session switch, and
   post-stream consolidation repair.
6. Final cleanup: delete legacy fullscreen base renderer and temporary
   validation switches.

## Reference Use

- Read `native-scrollback-architecture.md` before changing terminal backend or
  transcript emission.
- Read `rendering-hardening-and-rollback.md` before touching cursor,
  suspend/resume, inline viewport, or terminal history insertion.
- Read `migration-roadmap.md` before broad overlay, model picker, command
  surface, or presentation-layer refactors.
- Read `../crate-coco-tui.md` before implementation to understand current code
  shape and existing debt.
