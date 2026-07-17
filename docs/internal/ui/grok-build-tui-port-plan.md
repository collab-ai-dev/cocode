# Grok-Build TUI Port & Optimization Plan

Status: execution-ready optimization portfolio derived from a verified
comparative analysis of the grok-build TUI (`xai-grok-pager` and sibling
crates) against `coco-tui`. Every work item below survived an adversarial
verification pass that read the coco-rs source to refute the claimed gap;
items that failed verification are listed in [Rejected Candidates](#rejected-candidates)
so they are not re-proposed later.

Authority: this document does not override the architectural invariants in
`terminal-surface-design.md`, `native-scrollback-architecture.md`, or
`agent-console-design.md`. Every item here is designed to land *inside* those
invariants. File/line references reflect the codebase at the time of analysis
and are anchors, not contracts — re-locate by symbol name if lines drift.

Source trees referenced:

- coco: `coco-rs/` (paths below are workspace-relative)
- grok: `agents/grok-build/crates/codegen/` (read-only reference; never
  vendored — all ports are re-implementations adapted to coco conventions)

Effort legend: **S** < 1 day, **M** = 1–3 days, **L** = 1–2 weeks,
**XL** > 2 weeks — including companion `.test.rs` files and insta snapshots
where UI-visible.

---

## 1. Scope & Method

Three multi-agent analysis rounds were run (68 agents): seven parallel
dimension surveys over both codebases, cross-dimension merge into canonical
candidates, a completeness critique that back-filled missed subsystems, and a
per-candidate adversarial verification pass whose default stance was "this gap
is not real" and which read coco source to try to refute each claim. Two
dimensions (render/flicker, scrollback UX) were re-surveyed after transport
failures; their load-bearing coco-side claims were then hand-verified against
source (all confirmed).

Outcome: **57 confirmed work items, 5 rejected candidates.** This document
turns the confirmed items into implementation designs.

---

## 2. Architecture Comparison — The Port Boundary

### 2.1 The two models

**grok-build** is an alt-screen application that owns its scrollback:

- `xai-grok-pager/src/scrollback/`: `IndexMap<EntryId, ScrollbackEntry>`,
  each entry wrapping a `RenderBlock` plus three `RefCell` render caches keyed
  on `(width, generation, theme, is_selected, cwd)`; a `LayoutCache` with
  `dirty_heights` gives incremental height recomputation; two deliberately
  split invalidation counters (`generation` for view changes, `content_generation`
  for content changes).
- `xai-ratatui-inline/`: a forked ratatui terminal that fixes the 0.29 `u16`
  coordinate overflow, adds a per-cell OSC 8 hyperlink layer that participates
  in the frame diff, and discards byte-identical frames entirely.
- App architecture is Redux-style: `Action → sync dispatch(&mut AppView) →
  Vec<Effect> → JoinSet<TaskResult> → Action::TaskComplete` re-entry. The
  reducer is I/O-free and covered by 1,103 plain `#[test]`s.
- Owning every painted row buys: mouse text selection across wrapped lines,
  in-transcript search, sticky prompt headers, a clickable link map, and
  box-drawing table selection — at the cost of ~4k lines of selection code, a
  16k-line scrollback module, and a self-described "nuclear" inline resize
  (2J/3J purge + full history re-print).

**coco** paints into the terminal's native scrollback (`tui-ui`
`engine/history_insert.rs`): finalized rows are inserted once above a retained
bottom viewport and never repainted. The terminal itself provides selection,
copy, and scrollback search. TEA (`AppState` + `TuiEvent` + `update` +
projection) sits above a script-guarded domain-free presentational crate.

### 2.2 Non-negotiable constraints for every item below

1. **Native scrollback stays.** No item may introduce an app-owned main
   scrollback. Features that inherently require owning painted rows either
   land in the Ctrl+O transcript reader (coco's only owned-buffer surface) or
   are redesigned (see per-item notes).
2. **`tui-ui` purity seam.** `tui-ui` gets only domain-free additions
   (plain-data types, ratatui/crossterm/std deps). Anything touching
   `AppState`, i18n, `coco_config`, or engine domain types lands in `app/tui`.
   The seam guard (`scripts/check-tui-ui-seam.sh`) is the arbiter.
3. **TEA discipline.** State mutations flow through `update`; new UI surfaces
   are `ModalState`/overlay variants; new side effects use existing channels
   (until Workstream G1 lands an effect seam).
4. **Single scrollback-commit owner** (invariant from
   `terminal-surface-design.md`): any feature that emits history rows goes
   through the existing commit path; no second emitter.
5. **No `unsafe`** without a dedicated wrapper crate and explicit sign-off
   (relevant to A5, F5).
6. Modules stay < 800 LoC; companion `.test.rs`; insta snapshots for anything
   visible.

### 2.3 Where coco already wins — do not port

Verified head-to-head; these are listed so nobody "ports" a regression:

| Area | coco | grok |
|---|---|---|
| Sync update | Probed (`DECRQM ?2026$p` + DA1 fence, `app/tui/src/sync_update_probe.rs:35-108`, cached in `tui-ui/src/engine/compatibility.rs`) | Emits BSU/ESU blind |
| Frame composition | One BSU/ESU window brackets clear + history insert + viewport draw; ESU guaranteed on inner-draw error (`app/tui/src/terminal.rs:391-432`) | Multiple independent flushes on insert paths |
| Resize | Source-backed history replay, width-keyed only, 75 ms debounce (`tui-ui/src/engine/history_reflow.rs`) | Inline mode: 2J/3J purge + full re-print |
| ratatui | 0.30, `usize` coordinates | Forked 0.29 to fix `u16` wrap |
| Frame scheduling | `FrameRequester` actor, 120 fps clamp, self-armed spinner cadence | Hand-rolled timers across a 4,100-line event loop |
| Paint telemetry | Per-stage timings (plan/bsu/history/viewport/present) with sampling gates | Minimal |
| Modals | Priority queue with preempt/restore (`app/tui/src/state/ui.rs:344-396`) | Single `Option<ActiveModal>` slot |
| Events | Exhaustive 3-layer CoreEvent folding (compile-time forcing) | Imperative pushes from many sites |
| Transcript authority | Cells are a pure derivation of `MessageHistory` (I-2), exactly-once commit owner | Parallel `EntryId` bookkeeping spliced across timelines |
| Copy/export | `/copy` with lookback + fence picker + OSC 52 temp-file fallback; `/export` md/json/text incl. tool calls | Markdown-only export, tool calls collapsed to one line |
| i18n, module size, seam enforcement | Yes | No |

### 2.4 ratatui 0.30.1 / 0.30.2 upgrade note

coco is pinned to ratatui 0.30.0 (ratatui-core 0.1.0). Upstream 0.30.1
(2026-06) and 0.30.2 (2026-06) change the calculus for two items and add one
migration chore; nothing in this plan is obsoleted, because coco's paint
engine (`SurfaceTerminal`) does its own cell diff and does not use stock
`ratatui::Terminal`.

- **`CellDiffOption` (#1605, #2480; correctness follow-up #2587 in 0.30.2).**
  `Cell::skip: bool` became an enum with `ForceWidth` (squeeze an escape
  sequence into a cell's symbol and force its diff width to the visible
  width) and `AlwaysUpdate` (image-protocol cells). This is the extension
  point grok forked ratatui to get, now upstream, with a reference
  implementation (`tui-link`). Impact: D3 Phase 2 should use this pattern
  instead of a bespoke link layer (see D3); C8's viewport-side objection is
  weakened (revisit trigger), though the history-reflow objection stands.
- **Migration chore:** `buffer_updates()` and `drawable_cell_indices()`
  (`tui-ui/src/engine/terminal.rs`) read the now-deprecated `cell.skip`
  field. On upgrade, switch to `CellDiffOption` and teach the custom diff to
  honor `ForceWidth`/`AlwaysUpdate` — this is D3's enabling step.
- **Free wins on upgrade:** halfwidth dakuten/handakuten width fix (#2499)
  and the `CellWidth` trait (#2400) flow in via ratatui-core width
  computation (expect a few CJK insta snapshot updates).
- **Absorbable technique, minor magnitude (deep-verified):** the *idea*
  behind #2416 (`Terminal::flush` per-frame Vec allocation removal) applies
  to coco's custom diff — `buffer_updates()`
  (`tui-ui/src/engine/terminal.rs:795-820`) clones every changed cell into
  a fresh `Vec`, while the full downstream chain needs only references:
  `SurfaceBackend: Backend` inherits ratatui's
  `draw(I: Iterator<Item = (u16, u16, &Cell)>)` (references, not owned —
  the clone is not signature-forced), and `CrosstermBackend::draw` performs
  no internal cell copy (it serializes `cell.symbol()`'s bytes into the
  writer, which is the output itself). However the waste is small:
  `Cell.symbol` is `Option<CompactString>` (24-byte inline — a grapheme
  symbol essentially never heap-allocates on clone), and `Vec::new()`
  doesn't allocate on empty (idle) frames — so the cost is a few growth
  allocations plus ~40 B/cell memcpy on busy frames only. See work item
  **B8** (ROI low; do alongside the `CellDiffOption` migration).
- **Absorb as regression tests:** coco's custom diff was audited against the
  three upstream `Buffer::diff` bug classes and is *currently correct* —
  #2308 (style-only changes in wide-char trailing cells must not emit:
  coco never emits trailing cells, `to_skip` gate), #2587 (cells "uncovered"
  when a wide char is replaced by a narrow one must re-emit: coco's
  `invalidated` propagation uses `max(prev_width, next_width)`, which
  re-emits them — more conservatively than upstream's style-filtered fix),
  #2487 (saturating advance for forced widths: applies once
  `CellDiffOption` support lands). Port upstream's regression tests for all
  three classes into `terminal.test.rs` so the B8/D3 diff changes cannot
  regress them (folded into B8).
- **Verified not applicable:** resize cursor-query avoidance (#2485) — coco
  has zero cursor-position queries anywhere (`autoresize` uses
  `backend.size()`, an ioctl, and the engine parks the cursor with absolute
  moves only); inline-viewport resize clear (#2355) — coco already resets
  the previous buffer + full-repaints on any observed size change
  (`note_observed_screen_size` → `invalidate_viewport`) inside the single
  BSU window, with seat deferred-shrink handling the shrink path; the stock
  inline viewport's missing-clear bug is a path coco never had. A1's no-op
  frame elision remains unaddressed upstream and stays in this plan.
- MSRV bumped to 1.88 (coco toolchain is 1.93.1 — fine); crossterm stays
  0.29-compatible. Upgrade is a low-risk patch bump; do it before starting
  D3.

### 2.5 Rejected candidates

Adversarially verified as already-covered or non-problems. Do not re-propose
without new evidence.

| ID | Candidate | Rejection |
|---|---|---|
| c22 | `@`-completion directory drill-down | Already exists and is better designed: `app/tui/src/completion/mod.rs:361-388` (`DirAccept::Finalize/Drill`, fuzzy-rescan avoidance, `keep_popup`) |
| c35 | Turn-end reconciliation grace timer | Grok needs it because its turn lifecycle has two racing delivery paths; coco has one path — the loss mode is designed out at the transport layer |
| c38 | Display-refresh-aware draw cadence | coco's paint clock is demand-driven (spinner self-schedules at 50 ms); the free-running-frame problem grok solves does not exist here |
| c41 | Remote announcements banner | Weak product need; toast infrastructure already exists |
| c44 | Queue send-now / interject | coco already merges queued prompts into the in-flight turn (`app/tui/src/update.rs:364-380` "Steer (queue) whenever a turn is in flight") |

---

## 3. Workstream A — Flicker & Terminal-Output Hygiene

The user-facing goal: zero visible jitter at idle, during streaming, during
resize drags, and across focus changes; and a terminal that is never left
broken, even on hard faults.

### A1. Cursor-escape deduplication + zero-byte idle frames  — ROI high, M

**Problem (verified).** `SurfaceTerminal::apply_cursor_claim`
(`tui-ui/src/engine/terminal.rs:780-793`) unconditionally emits
`set_cursor_style` + `show_cursor` + `set_cursor_position` on every frame, and
the frame path unconditionally emits BSU + ESU + flush
(`app/tui/src/terminal.rs:403,416`) — including on every spinner/animation
frame (`surface/controller.rs:439-450`). Terminals restart the cursor-blink
timer on `Show`/`MoveTo`: at spinner cadence the composer cursor appears
permanently solid (a visible "the app is repainting at me" tell), and idle
frames still write bytes, waking the emulator per frame.

**Grok reference.** `xai-grok-pager-render/src/render/draw.rs:1-37` documents
the defect class; `CursorState::action/apply` de-dupes Show/Hide/MoveTo
(draw.rs:247-318); `flush()` returns `has_changes` and `draw_frame` discards
the entire buffered frame — including the queued BSU — when no cell changed
and the cursor is stable. Pinned by test `idle_frame_emits_zero_bytes`.

**Decision.** Port the *state-gating idea*, not grok's buffered-writer
mechanism (that belongs to A6). Two increments:

1. **Cursor dedup (standalone).** Add `last_cursor: Option<CursorClaim>`
   (style + position + visibility) beside `last_parked_cursor` on
   `SurfaceTerminal`. `apply_cursor_claim` compares and emits only deltas:
   style change → `set_cursor_style`; visibility change → show/hide; position
   change → `set_cursor_position`. History-insert re-park and
   `invalidate_viewport()` must clear `last_cursor` (position trust is lost
   whenever the engine writes raw VT outside the diff).
2. **Frame-level no-op elision.** In the `app/tui` frame path, compute
   "viewport diff is empty AND no history rows pending AND cursor claim
   unchanged" *before* emitting BSU. If true, skip BSU/draw/ESU/flush
   entirely. This requires `buffer_updates()` (or a cheap dirty flag
   maintained by the engine) to be consultable pre-BSU; the engine already
   owns both buffers, so expose `fn has_pending_changes(&self) -> bool`.

**Why not grok's design as-is.** Grok buffers the whole frame and throws it
away post-hoc; coco can decide *before* writing because the diff source is
already in memory. Cheaper and no writer-thread dependency.

**Landing.**
- `tui-ui/src/engine/terminal.rs`: `last_cursor` field, gated
  `apply_cursor_claim`, `has_pending_changes()`, invalidation resets.
- `app/tui/src/terminal.rs`: no-op elision before the BSU stage (keep the
  perf-stage timers; add a `frame_skipped` counter to the existing
  `tui::perf` events so the win is observable).

**Tests.** Engine tests with the recording backend: (a) two identical draws →
second emits zero cursor escapes and zero buffer writes; (b) blink
preservation — N spinner frames with unchanged viewport emit no
`Show`/`MoveTo`; (c) `invalidate_viewport` → next frame re-emits full cursor
claim; (d) history-insert between frames → cursor re-asserted. Extend
`bench_metrics` idle-frame benchmark.

**Risk.** Low. The one subtle case is external cursor movement (suspend/
resume, `$EDITOR` handoff): `prepare_external_process` / resume paths must
clear `last_cursor` — audit all `invalidate_viewport()` call sites and the
job-control path.

### A2. Resize quiet-period (draw suppression during drags) — ROI medium, S

**Problem (verified).** `TuiEvent::Resize` (`app/tui/src/app.rs:1081-1089`)
just records the size and returns `needs_redraw = true`; the FrameRequester
then paints at up to the 120 fps clamp at every transient width of a drag.
Only the *history replay* is debounced (75 ms, `history_reflow.rs:12`); the
per-frame live-tail rebuild re-wraps at each intermediate width, missing its
width-keyed cache every frame — visible re-wrap churn plus wasted CPU.

**Grok reference.** `RESIZE_DEBOUNCE = 16ms`; each resize event resets the
timer; a continuous drag triggers exactly one relayout; refocus force-repaint
deliberately overrides the debounce (`event_loop.rs:1430-1435, 1867-1878`).

**Decision.** Arm a deadline instead of scheduling a frame on resize. On
`TuiEvent::Resize`, record `pending_resize: Option<(Size, Instant)>` and call
`FrameRequester::schedule_frame_in(RESIZE_QUIET_PERIOD)` (16 ms; the requester
already coalesces). Apply `state.ui.terminal_size` only when the quiet period
elapses without a newer resize; force-repaint paths (SIGCONT resume, A3 focus
heal) bypass the debounce, mirroring grok. Do not change the 75 ms history
replay debounce — the two timers layer correctly (viewport settles first,
replay follows).

**Landing.** `app/tui/src/app.rs` resize arm + a small helper in
`app/tui/src/frame_layout.rs` or a new `resize_debounce.rs` (< 100 LoC) with
companion tests using the existing app test harness.

**Tests.** Burst of N resize events within the window → exactly one applied
size + one frame; resize then immediate focus-heal → immediate repaint.

### A3. FocusGained forced repaint under out-of-band repainters — ROI medium, S

**Problem (verified).** `TuiEvent::FocusChanged` (`app/tui/src/app.rs:1091-1108`)
only tracks focus and requests a normal diff redraw — the comment says the
redraw exists to re-assert the *cursor* pin. The cell diff's previous buffer
believes stranded cells are intact, so viewport content overwritten
out-of-band by tmux/Zellij/`vim :terminal` persists until an unrelated
invalidation. `SurfaceTerminal::invalidate_viewport()` already exists
(`tui-ui/src/engine/terminal.rs:456-459`) — it is simply never called on
focus.

**Grok reference.** `TerminalContext::repaints_pane_out_of_band()` (any
multiplexer or embedded-editor terminal) gates a FocusGained heal:
`terminal.clear()` + full redraw, explicitly overriding the resize debounce
(`event_loop.rs:2684-2691`).

**Decision.** On `FocusChanged { focused: true }`, if the session runs under
an out-of-band repainter, call `invalidate_viewport()` before the redraw.
Detection: start with env checks (`TMUX`, `STY`, `ZELLIJ` — Zellij detection
already exists in `compatibility.rs:27-31`; add tmux/screen), and fold into
the G3 capability model when it lands. Note the honest limit: coco can heal
only its owned viewport region; rows already in native scrollback belong to
the terminal (this is correct behavior, not a gap).

**Landing.** `app/tui/src/app.rs` focus arm + detection helper (shared home
with G3). Tests: recording backend asserts full repaint (previous buffer
reset) on focus-gain when gated, plain diff redraw otherwise.

### A4. Canonical DEC private-mode ledger + RESTORE_SEQ — ROI medium, S

**Problem (verified).** Terminal teardown is composed independently at four
sites — `leave_tui_modes` (`app/tui/src/terminal.rs:135-146`),
`restore_terminal` (`:180-184`), `Tui::drop` (`:754-796`, with a comment-only
explanation of why unpaired `?1049l` is omitted), and
`keyboard_modes.rs:215-230` (hand-rolled `\x1b[<u`). Ordering constraints
(`?2026l` must come first so a multiplexer never relays a stuck
deferred-present; kitty pop must precede any `?1049l` because kitty keeps a
per-screen key-mode stack) live only in comments. There is no
raw-bytes restore constant — which A5 (signal handler) requires, since
crossterm calls are not async-signal-safe.

**Grok reference.** `xai-crash-handler/src/terminal.rs`: a documented mode
table, a hard `RESTORE_SEQ` byte constant, and ordering-invariant tests.

**Decision.** Add a small module — `tui-ui/src/engine/restore_seq.rs` — that
is the single source of truth:

```rust
/// Every DEC private mode / stateful protocol coco can leave enabled, with
/// the teardown ordering invariants encoded as tests, not comments.
pub const RESTORE_SEQ: &[u8] = b"\x1b[?2026l\x1b[<u\x1b[?2004l\x1b[?1004l\x1b[?1007l\x1b[?25h";
```

(Exact contents to be finalized against coco's actual mode set: `?2026`,
kitty push/pop, `?2004` bracketed paste, `?1004` focus, `?1007` alternate
scroll, `?25` cursor; `?1049` stays *conditional* and outside the constant —
coco's main surface never enters alt-screen, and the Drop path's conditional
structure is deliberate.) The existing teardown sites keep their composition
but gain assertions (in `terminal.test.rs` with the recording backend) that
their emissions match the ledger's applicable subset and ordering. The
constant is exported for A5.

**Why tui-ui.** Raw byte constants and ordering tests are dependency-free and
domain-free; `app/tui` imports the constant for the panic path.

### A5. Two-tier async-signal-safe fault handler with terminal restore — ROI high, M

**Problem (verified).** coco's only crash cleanup is the unwind panic hook
(`app/tui/src/terminal.rs:190-213`) — heap-allocating, crossterm-based, never
reached on SIGSEGV/SIGBUS. Workspace-wide grep for
`sigaction`/`sigaltstack`/`signal-hook`: zero hits. A hard fault in a C
dependency (jemalloc, ring, whisper) leaves the user in raw mode with kitty
key reporting on and possibly inside an un-terminated `?2026h` window. Because
**every coco frame paints inside a synchronized-update window**, a fault
mid-frame freezes the visible terminal — it reads as a total lockup, and the
user must run `reset` blind. This failure mode is strictly worse for coco than
for a typical TUI.

**Grok reference.** `xai-crash-handler`: two-tier design — a termios-only
restorer installed pre-TUI, upgraded to termios + raw `RESTORE_SEQ` writes
when TUI modes are entered; handlers are async-signal-safe (`write(2)` of
pre-computed bytes only).

**Decision.** Port, adapted:

- New crate `utils/crash-handler` wrapping the unavoidable `libc` FFI
  (`sigaction`, `sigaltstack`, `tcsetattr`), following the documented
  process-hardening exemption for wrapping unsafe deps in a dedicated crate.
  **Requires explicit sign-off per the no-unsafe rule before implementation.**
- Two tiers: `install_terminal_restore_only()` at the top of `app/cli` main
  (termios snapshot + restore); `arm_tui_restore(seq: &'static [u8])` /
  `disarm_tui_restore()` toggled from `enter_tui_modes`/`leave_tui_modes`,
  writing A4's `RESTORE_SEQ` (which is why `?2026l` leads the constant).
- Handler body: restore termios, `write(2)` the restore bytes to the tty fd,
  re-raise with default disposition. No allocation, no locks, no formatting.
- Scope split: this item is *terminal restore only*. Crash-report persistence
  is F5 and can start with the zero-unsafe panic half independently.

**Tests.** Unit tests on the safe wrapper API (arm/disarm idempotence,
sequence selection); a PTY e2e (behind A7's harness) that SIGSEGVs a child
fixture and asserts the pty is left in cooked mode with `?2026l` observed.

### A6. Background terminal-writer thread with drain barrier — ROI medium, L

**Problem (verified).** `CrosstermBackend<Stdout>` is written synchronously on
the tokio event loop (`app/tui/src/terminal.rs:49`); coco's own perf comment
(`terminal.rs:399-401`) acknowledges that a slow flush means "the kernel pipe
is full and the emulator hasn't drained prior frames" — i.e. a busy tmux pane,
SSH link, or wedged emulator stalls input handling, CoreEvent processing, and
timers mid-frame.

**Grok reference.** `render/draw.rs:115-246`: frames are byte-buffered and
shipped to a dedicated `term-writer` OS thread (64 KiB `BufWriter`);
`WriterSync` counts queued vs written frames; `wait_drained()` provides a
bounded barrier used before `$EDITOR`/`$PAGER` children take the tty so a late
frame cannot land on the child's alt screen or tear mid-escape.
`GROK_TEST_FRAME_WRITE_DELAY_MS` injects latency for tests.

**Decision.** Port the shape into `tui-ui` as a generic, std-only
`FrameWriter`:

- `FrameWriter` implements `io::Write` by buffering into a frame `Vec<u8>`;
  `present()` sends the buffer over a bounded channel (capacity 2 — one
  in-flight + one queued; a full channel means the previous frame is still
  writing, and the new frame *replaces* the queued one, which is exactly the
  frame-drop semantic we want under backpressure) to a named OS thread that
  writes + flushes.
- `DrainBarrier { queued: AtomicU64, written: AtomicU64, wait_drained(timeout) }`.
- Env-injectable write delay for tests (`COCO_TUI_TEST_WRITE_DELAY_MS`, routed
  through `coco_config::EnvKey` and passed in as a value — tui-ui does not
  read env itself).

**Integration cost is the real work** (why this is L, and sequenced after A1):

- `Tui::drop` teardown ordering: `wait_drained` before the final prompt park
  and before A4's restore emission.
- Job control (SIGTSTP) and external-process handoff: drain before yielding
  the tty (`prepare_external_process`, `job_control.rs`).
- Panic hook and A5: both must write directly to the fd, bypassing the thread
  (the thread may be the panicking one).
- Perf stages: `present_flush` currently measures real terminal latency; keep
  it meaningful by timing on the writer thread and reporting via the existing
  counter channel.

**Sequencing note.** A1's no-op elision removes most idle-frame writes, which
shrinks this item's payoff to the genuinely-slow-terminal case. Ship A1 first;
treat A6 as the SSH/tmux resilience item, not a flicker item.

### A7. PTY end-to-end render-integrity harness — ROI medium, L

**Problem (verified).** coco's native-scrollback escape emission (DECSTBM
scroll-region inserts, absolute-addressed row writes, BSU windows) is
well-tested at the recording-backend level but has exactly three real-PTY
tests (`app/tui/tests/suite/`: `resume_restores_scrollback.rs`,
`clear_emits_reset.rs`, `auto_restore_truncates.rs`). Scroll/resize/timing
races across a live vte have thin coverage — precisely the class of bug that
recording backends cannot catch (they never exercise emulator state).

**Grok reference.** A dedicated harness crate (`xai-grok-pager-pty-harness`:
env hygiene stripping `HOST_TERMINAL_ENV_VARS`, screen capture, scripted
drivers, `scroll_matrix/`, timing) plus 100+ `pty_e2e` scenarios including
`ansi_scrollback_content_integrity.rs`, with injected writer latency to
reproduce backpressure races.

**Decision.** Do not port the harness wholesale. Grow coco's existing seeds:

1. Extract shared helpers from the three existing suite tests into
   `utils/test-harness` or a `tests/suite/pty_support.rs`: spawn-under-pty
   (via `utils/pty`), deterministic env hygiene (strip `TERM_PROGRAM`, `TMUX`,
   kitty vars — also the mitigation for the macOS keychain-prompt flake noted
   in project memory), a vte-backed screen model for assertions.
2. Add a **scroll/resize matrix**: streaming content × {narrow→wide,
   wide→narrow, height-only} × {during stream, after commit} asserting
   scrollback content integrity (no duplicated/lost/torn rows).
3. Once A6 lands, add injected write-delay scenarios.

Keep the suite small and deterministic (10–20 scenarios, not 100); each new
flicker/engine item (A1–A3) contributes one scenario as its acceptance gate.

---

## 4. Workstream B — CPU & Memory

### B1. Closed-fence highlight memo (multi-fence streaming tails) — ROI high, S

**Problem (verified).** `tui-markdown/src/highlight.rs:426-480` keeps a
*single global* `StreamingFenceSlot` with rebuild-on-non-prefix. When the
mutable tail contains more than one fence — the common LLM pattern of a closed
fence inside a still-open list plus the growing open fence — each render pass
alternates slot ownership and both fences are fully re-tokenized every frame
(grok measured 50–100 ms per re-run on large blocks; the exact O(block²)
pathology the slot exists to prevent). Streaming deliberately bypasses the
committed LRU (`highlight.rs:317-327`), so no other cache catches this.

**Decision.** Add a `ClosedFenceMemo` beside the slot, entirely inside
`highlight.rs`:

- `HashMap<u64, Arc<[StyledLine]>>` keyed by `hash(code, lang, theme_hash)`,
  byte-budget eviction (grok's `CLOSED_MEMO_CAP_BYTES` idea; start at 1 MiB).
- In the `rebuild_cause == non_prefix_content` branch: consult the memo first;
  on miss, tokenize once, insert into the memo **and** rebuild the slot. A
  closed fence then costs one tokenize on its first frame and hits the memo
  thereafter; the open fence reclaims the slot once and extends O(delta).
- Grok's `body_reaches_eof` routing flag is unnecessary here — memo-first
  covers it.

No parser or `app/tui` changes; tier-2 leaf; companion tests in
`highlight.test.rs` (multi-fence tail alternation stops re-tokenizing;
memo eviction; theme-change invalidation via `theme_hash`).

### B2. Append-only streaming markdown (stop re-rendering the stable prefix) — ROI medium, L

**Problem (verified).** `app/tui/src/transcript/stream.rs:148-153`: every
time `stable_prefix_end` advances, the controller re-renders the **entire**
stable prefix from byte 0 and replaces `stable_lines` wholesale (the memo is
deliberately bypassed on this path — `transcript/render/assistant.rs:179-185`:
"the StreamRenderController is the cache on this path"). For a response with
K top-level blocks this is K full-prefix renders — O(N²/block-size) total
parse + `Line`/`Span` allocation, with jank largest late in the stream.
`tui-ui/benches/render.rs:22-25` already names this as deferred work.

**Grok reference.** `xai-grok-markdown/src/checkpoint.rs` + `streaming.rs`:
parser-event-grounded checkpoints freeze rendered lines at top-level block
boundaries; only the tail is ever re-rendered; wrapping is incremental
(`markdown_content.rs:30-44` — "turns streaming from O(N²) total wrapping to
~O(N)").

**Decision — the cheap variant, not grok's machinery.** coco already has the
hard half: a tested conservative stable-boundary finder and a stable/tail
split. The missing piece is *append at the boundary* instead of re-render
from zero:

- When `stable_prefix_end` advances from `prev` to `new`, render
  `source[prev..new]` as an independent markdown document and **append** the
  resulting lines to `stable_lines`, instead of re-rendering `source[..new]`.
- Certify the seam behaviors this assumes, as unit tests over the boundary
  finder: boundaries only at top-level block starts (never inside lists,
  tables, fences, block quotes), so rendering the slice independently equals
  rendering it in context. Where a construct violates this (e.g. setext
  headings, lazy continuation), the boundary finder must already refuse to
  advance — encode those cases as tests, and keep a fallback full re-render
  path behind a mismatch debug-assert during rollout.
- Wrapping stays as-is initially (wrap-on-projection); incremental wrap is a
  follow-up only if profiles still show it.

**Landing.** `app/tui/src/transcript/stream.rs` (+ possibly a small
`tui-markdown` helper). Composes cleanly with native scrollback:
`stable_lines` already feed history insertion append-only. Acceptance: the
`markdown_streaming` bench in `tui-ui/benches/render.rs` goes from quadratic
to ~linear; add a long-stream case (500-block document).

### B3. Transcript-overlay height-cache retention — ROI medium, S

**Problem (verified).** `app/tui/src/widgets/transcript_modal.rs:75-87`:
`begin_frame` clears the **entire** `heights` map whenever
`content_generation` changes, and the generation hash includes the last cell's
content length and every tool-execution status — so every streaming delta
flushes the cache. With the overlay open pinned to Tail during a stream,
`total_height()` re-measures every cell every change: O(history) full cell
renders per delta (the single-digit-FPS pattern grok's layout cache exists to
avoid). Cells are immutable pure derivations (invariant I-2), so a keyed
height can never go stale on *append*.

**Decision.** Retention policy change, not grok's virtual-y patching (coco's
lazy prefix-sum already handles positional recompute):

- On generation change: keep `heights`, reset only the `prefix` vector.
- Enumerate the true staleness vectors and invalidate precisely: tool-call
  cells whose `ToolExecution` status changed (fold status into
  `TranscriptHeightCacheKey` — cheapest correct fix), cells with newly
  attached reasoning/citations (already content-keyed), width change (already
  a full reset).

**Landing.** `transcript_modal.rs` + `transcript_modal.test.rs`: streaming
append with overlay open re-measures only the new/changed cells (assert via a
measure-count probe); B7 adds the paired bench.

### B4. Cliff-attributed jemalloc purges — ROI medium, S

**Problem (verified).** Purge fires only on `MemoryPhase::TurnEnded`
(`app/tui/src/app.rs:660-671`); `perf.rs:82-107` already enumerates
`HistoryReplaced`/`ContextCleared`/`MessageTruncated`/`SessionReset` but those
only log a sample. After a large session resume, `/clear`, or rewind, freed
replay pages stay resident until the *next* turn ends (macOS jemalloc has no
background thread — documented in `jemalloc_purge.rs:3-9`). The purge log
hardcodes `"turn_ended"`, so per-site effectiveness is invisible.

**Decision.** ~30 lines in `app/tui`: generalize
`spawn_turn_ended_purge(...)` to `spawn_purge(reason: &'static str, ...)`;
call it from `note_lifecycle_memory_phase` for all enumerated cliff phases;
thread `reason` into the purge log as a structured field
(`phase.as_str()` works directly). Skip grok's release-hook IoC seam and
post-draw coalescer — coco's `spawn_blocking` purge already serializes.

### B5. Always-on memory-trace artifact — ROI medium, M

**Problem (verified).** All memory diagnostics are opt-in at launch:
`MemoryPerfTracker` (`perf.rs:146-263`) emits `tracing::debug!` that the
default filter drops, gated further on `tui.performance.memory_enabled`; heap
dumps need a special jemalloc build. A field memory bug in a normal run leaves
zero evidence.

**Grok reference.** `memory_trace.rs`: rotating JSONL artifact — 30 s samples,
purge events with before/after gauge + duration, doubling threshold buckets
with halving hysteresis that trigger a full `malloc_stats_print` dump.

**Decision.** New `app/tui/src/memory_trace.rs` (< 800 LoC): port the pure
`Thresholds` hysteresis state machine with its unit tests; JSONL sink under
`<config_home>/logs/memtrace/` (same convention as `jemalloc_purge.rs:121`);
events: periodic sample, threshold-crossing with `malloc_stats_print` text,
purge before/after (wired from B4's `reason`). Call `coco_utils_jemalloc`
directly — no provider-injection seam. Cheap enough to be always-on (one
sample/30 s); gate only the stats-dump size.

### B6. Bounded CoreEvent drain — ROI medium, S

**Problem (verified).** The CoreEvent arm (`app/tui/src/app.rs:522-537`)
drains `while let Ok(next) = notification_rx.try_recv()` with no batch bound
on a capacity-256 channel; during a hot stream the loop can process the whole
backlog before re-polling terminal input — worst-case keystroke latency is a
full 256-handler burst.

**Decision.** Minimal port: `const CORE_EVENT_DRAIN_BATCH_MAX: usize = 32;`
bound the loop, plus a comment stating the starvation invariant. The unbiased
`select!` then fairly re-polls input between batches. **Do not** port grok's
full apparatus (input pump task, `biased` select, `input_rx.is_empty()` gate)
— coco's FrameRequester already neutralizes the paint-storm half; the
machinery would be architecture for a problem coco doesn't have.

### B7. Paired interaction benches — ROI low, S (do with B3)

Add a criterion bench `app/tui/benches/transcript_overlay.rs` (gated on the
existing `testing` feature like `native_replay.rs`): synthetic ~3k-cell
transcript, drive `TranscriptStateWidget::render` + scroll stepping, paired
variants — `layout_index.reset()` each iteration vs preserved index. Purpose:
makes B3 measurable and guards it. Pattern, not code, from grok's
`benches/bench.md` skip-vs-rebuild pairs.

### B8. Upstream diff regression tests + viewport-diff copy elision — ROI low, S

**Status after deep verification.** The clone in `buffer_updates()`
(`tui-ui/src/engine/terminal.rs:795-820`) is genuinely unnecessary — the
whole downstream chain takes references (`SurfaceBackend: Backend` inherits
ratatui's `draw(Iterator<Item = (u16, u16, &Cell)>)`; `CrosstermBackend::draw`
does no internal cell copy, only byte serialization of `cell.symbol()` into
the writer). But the magnitude is small: `Cell.symbol` is
`Option<CompactString>` (24-byte inline storage), so cloning a grapheme cell
is a ~40 B memcpy with **no heap allocation**, and `Vec::new()` allocates
nothing on idle frames. Real cost: a few Vec growth allocations + N×~40 B
memcpy on busy frames. This is a tidy-up, not a win — hence ROI low.

**Decision.**
- **The valuable half — port upstream's diff regression tests** (see §2.4)
  into `terminal.test.rs`: style-only trailing-cell changes emit nothing
  (#2308 class); wide→narrow replacement re-emits the uncovered cell
  (#2587 class — coco's `invalidated = max(prev_width, next_width)`
  propagation already handles this; pin it); forced-width advance saturates
  (#2487 class — activates with the `CellDiffOption` migration). These pin
  the audited-correct behaviors before the D3-enabling diff changes land.
- **The tidy-up half — do only while already touching the diff** for the
  `CellDiffOption` migration (that migration must rewrite this loop's
  `next.skip` check anyway). Concrete shape (~30 lines, one file + its
  companion test, byte-identical output):
  1. Add `update_index_scratch: Vec<usize>` to `SurfaceTerminal` (precedent:
     `history_row_scratch: String`). `collect_update_indices(&mut self)`
     destructures `self` (`let Self { buffers, current,
     update_index_scratch, invalidated, .. } = self;`) — the in-body
     `self.current_buffer()` method calls must become direct field indexing
     (`&buffers[*current]`), or the scratch's mutable borrow conflicts; this
     is the only borrow-check trap. `clear()` + push `index` instead of
     `(x, y, next.clone())`.
  2. Call site: `let buffer = &self.buffers[self.current];` +
     `self.backend.draw(indices.iter().map(|&i| { let (x, y) =
     buffer.pos_of(i); (x, y, &buffer.content[i]) }))` — three disjoint
     field borrows, compiles without gymnastics.
  3. Adapt the one direct-consumer test
     (`terminal.test.rs` `surface_terminal_skips_hidden_cells_after_wide_chars`)
     to resolve cells through the buffer; all other tests assert on the
     recording backend's bytes or `stats.buffer_updates` counts and are
     unaffected.

**Observable.** `ViewportDrawStats.diff_elapsed` (already recorded per
frame); the regression tests are the deliverable.

---

## 5. Workstream C — Composer & Input

### C1. Undo/redo engine with mutation-kind batching — ROI high, M

**Problem (verified).** `tui-ui/src/widgets/textarea.rs:129-211` has only
`undo_stack: Vec<UndoSnapshot>` (cap 64) — **no redo stack exists in the
workspace** (`vim/wiring.rs:36-37`: "we have no redo stack and this would just
oscillate"; Ctrl+Shift+Z is a dead chord). Undo granularity is one snapshot
per mutating `TuiCommand` (committed externally at `update.rs:84-91` and
`vim/wiring.rs:27-40`) — no word-boundary batching, no grouping primitive.
Groups are also the substrate C3 and C8 need (chip insert = one undo step).

**Grok reference.** `xai-ratatui-textarea`: undo entries carry a
`MutationKind` (insert-char, delete-back, delete-word, paste, …); consecutive
same-kind single-char mutations coalesce until a word boundary/kind change;
`begin_undo_group()/end_undo_group()` brackets compound edits.

**Decision.** Implement natively inside coco's `TextArea` (pure widget state,
seam-safe, zero new deps):

- `undo_stack`/`redo_stack: Vec<UndoState>`; any new mutation clears redo.
- Internal `pre_mutate(kind: MutationKind)` called by every mutating verb:
  pushes a checkpoint unless it coalesces with the previous entry (same kind,
  adjacent position, no word boundary crossed).
- `undo_group(|ta| ...)` scope for compound operations.
- Wiring: add `TuiCommand::RedoInput` + keybinding; **delete** the external
  snapshot path at `update.rs:84-91` once verbs self-checkpoint; vim keeps its
  external `commit_undo` API but it must also clear redo (reconcile in
  `vim/wiring.rs`).

**Tests.** Port grok's batching rule table as cases: type word → one undo
step; type two words → two; backspace run → one; paste → own step; group →
atomic; undo→type→redo dead. Vim-path parity tests.

### C2. Word-boundary soft-wrap with viewport scrolling — ROI high, M

**Problem (verified, three-part).** (1) `wrap_logical_line`
(`textarea.rs:835-905`) wraps by grapheme count — mid-word breaks. (2) The
composer render (`app/tui/src/widgets/input.rs:268-334`) ignores
`TextArea::wrapped_lines` entirely — it splits on `\n` and renders a
`Paragraph` without `.wrap()`, so a long single-line prompt is horizontally
clipped **invisible**. (3) `scroll_offset` (`input.rs:339-347`) is
vertical-only over hard lines.

**Decision.**
- `tui-ui`: replace `wrap_logical_line`'s body with word-aware wrapping via
  `textwrap` (already a workspace dep and blessed by CLAUDE.md), preserving
  the `Vec<Range<usize>>` contract, the `WrapCache`, `cursor_pos`, and
  `desired_height` APIs.
- `app/tui`: rewire `InputWidget` to render from
  `textarea.wrapped_lines(width)` with a scroll offset shared with cursor
  placement (replace the `split('\n')` path); thread real width into
  `surface/viewport.rs::input_height_for_state` (the `_width` parameter is
  already signature-adjacent).

**Tests.** tui-ui: wrap ranges at word boundaries, CJK/emoji width cases
(reuse width-aware truncation fixtures); app/tui: insta snapshots — long
paste visible and wrapped, cursor follows across visual rows, composer grows
to `MAX_INPUT_HEIGHT` then scrolls.

### C3. Atomic `TextElement` chips (paste pills, file refs) — ROI high, L

**Problem (verified — self-documented).** `textarea.rs:14` explicitly lists
"TextElement / placeholder ranges" as a deliberate omission from the codex-rs
port. Pills are literal strings: `app.rs:1052-1053` inserts a `[Pasted #N]`
string; resolution is `str::replace` (`tui-ui/src/paste.rs:108-148`). An
edited pill label **silently drops the paste payload** (real data loss);
backspace erases pills bracket-by-bracket; nothing supports styled chip
display or expand-in-place.

**Decision.** Port grok's element model shape, re-implemented against coco's
verb-based textarea:

- `TextElement { range: Range<usize>, kind: ElementKind, display: Line<'static> }`
  stored sorted on `TextArea`; `ElementKind` is a small closed enum
  (`Paste`, `Image`, `FileRef`) with **no domain payload** — the payload key
  stays in `app/tui`'s `PasteManager`, keyed by element id. This keeps tui-ui
  domain-free.
- Element-aware atomic boundaries: cursor motion treats an element as one
  grapheme; backspace/delete removes the whole element; any edit that would
  split an element instead selects it (or is rejected) — corruption becomes
  unrepresentable.
- APIs: `insert_element`, `replace_element_with_text` (expand-in-place),
  element enumeration for the resolver; `PasteManager` re-keys entries by
  element id instead of label matching.
- Likely a companion module `widgets/textarea_elements.rs` to respect the
  800 LoC target.
- C1's undo groups make insert/expand atomically undoable.

**Sequencing.** After C1 (groups) and C2 (wrap must count element display
width, not source width — implement `wrapped_lines` element-awareness in the
same pass).

### C4. Keyboard selection model (anchor/head) — ROI medium, M

**Problem (verified).** `TextArea` has no selection state or range ops; no
select-all/selection-replace; `VimState` has exactly Insert/Normal — Visual is
unimplementable without a buffer selection primitive.

**Decision.** Add `selection_anchor: Option<usize>` (byte offset; cursor is
the head) + range ops (`selection_range`, `selected_text`,
`delete_selection`, `select_all`, `clear_selection`; insert-replaces-selection
semantics) to `TextArea`; render as span styling over wrapped lines
(theme-driven selection style). Keyboard acquisition only (Shift+arrows,
Cmd/Ctrl+A, later vim Visual); **skip all mouse-drag code**. Unblocks vim
Visual as a follow-up.

### C5. Ctrl+R fuzzy history search overlay — ROI high, M

**Problem (verified).** `update/edit.rs:341-360` is a case-insensitive
substring linear scan on the UI thread; stepping previews one entry at a time;
`HistorySearch` state (`state/ui.rs:633-645`) holds only query + index. No
result list, no ranking, no highlights, no browse mode.
`app/session/src/history.rs:244 get_timestamped_history` was built for this
picker and has **zero consumers**.

**Decision.** Adapt grok's UX (visible ranked list, per-char highlight
indices, stick-to-bottom selection with re-anchor-on-typing, browse mode on
plain Up), but match synchronously with nucleo in the TEA update path — coco's
autocomplete already proves this is fast enough; skip grok's daemon thread.

- State: extend `HistorySearch` with `results: Vec<HistoryMatch>` (entry id,
  highlight indices), selection, `browse: bool`.
- Matching: new module beside `autocomplete/`, reusing the nucleo `Matcher`
  pattern from `autocomplete/skill_search.rs`, fed by
  `get_timestamped_history`.
- Render: overlay list above the composer via
  `coco_tui_ui::widgets::select_list`; highlight spans computed in `app/tui`
  (C6 provides the `SuggestionItem` highlight plumbing — share it).
- Keys: extend the existing `TuiCommand::HistorySearch*` family.

### C6. Fuzzy-match highlight indices in completion popups — ROI medium, S

**Problem (verified — half-built feature).**
`utils/file-search/src/index.rs:37-38` computes and documents
`FileSuggestion.match_indices: Vec<i32>` "for highlighting" — and every
consumer drops them. `SuggestionItem` (`widgets/suggestion_popup.rs:35-47`)
has no highlight field; `build_row` renders one monolithic span.

**Decision.** Finish it: add `highlight_indices: Vec<u32>` (char positions) to
`SuggestionItem`; `build_row` splits the label into alternating spans (bold/
primary for matched chars, including when unselected); thread indices at the
three producer sites (file search already has them; slash ranker knows prefix
ranges; skill search exposes nucleo indices). Care point: truncation must
remap indices (truncate first, then filter/shift indices). Snapshot refresh.

### C7. Cmd+V clipboard-attachment probe — ROI medium, M

**Problem (verified).** The bracketed-paste arm (`app.rs:~1010-1064`) never
probes the OS clipboard for raster data; image attach exists only behind
in-app Ctrl+V (`update/clipboard.rs::paste_from_clipboard`). A terminal-level
Cmd+V after screenshotting pastes nothing (image-only clipboard → empty
bracketed paste on some terminals) or caption text only.

**Decision.** Port the pure decision layer into `tui-ui/src/paste.rs` (probe
gating, payload-vs-clipboard normalization/matching, `AttachmentProbeRoute`),
add a clipboard *text-read* function to `tui-ui/src/clipboard.rs` (currently
write-only + image-read), and spawn the async probe from the Paste arm using
the existing background-task → `TuiEvent` pattern. Honest scope: the reliable
wins are short-text+image clipboards and empty-paste terminals; many terminals
emit nothing on image-only Cmd+V (probe never fires). Ship after C3 so the
result lands as an image chip.

### C8. Inline image chips — thin slice only — ROI low (thin slice), M

**Verified but descoped.** Full terminal-graphics preview (kitty/iTerm2/sixel)
fights the cell-diff engine hardest (post-flush raw escapes coordinated with
history reflow; grok rides its forked inline backend + ~5k LoC of protocol
machinery) — **do not build**. Revisit trigger: ratatui 0.30.1's
`CellDiffOption::AlwaysUpdate` (§2.4) removes the viewport-side objection
(it is the mechanism the ratatui-image ecosystem builds on); the
history-reflow objection still stands, so any revisit is viewport-only
(composer chip preview), never committed rows. The thin slice worth shipping with C3: decode
dimensions on attach (via `coco-utils-image`), reject undersized/oversized
images with a toast *before* they cost a failed API turn, cap attachment
count, and render image chips as styled `TextElement`s with size labels.

### C9. Permission prompt: MCP server-scope allow + deny-with-reason — ROI high, M

**Problem (verified — UI-only gap, engine ready).** (1) The permissions
engine already matches server-level rules (`core/permissions/src/rule_compiler.rs:226-243`
handles `mcp__server` and `mcp__server__*`), but the TUI only ever constructs
exact-tool rules (`permission_options.rs:346-369`) — no "always allow this
whole MCP server" choice, so many-tool servers cause prompt fatigue. (2)
Denying cannot carry a reason to the model, wasting a blind retry turn; the
`UserCommand` plumbing for feedback delivery exists (ExitPlanMode's feedback
pattern is the template).

**Decision.** Implement natively in the TEA prompt (do not port grok's 3k-line
`permission_view`): add a scope field (Tool/Server, derived from splitting
`mcp__server__tool`) to `PermissionPromptState` + a toggle key; emit a
`mcp__<server>` pattern rule on Server scope (zero engine changes). Add a
deny-reason inline input as one more interceptor cloned from the ExitPlanMode
feedback flow. Snapshots for both.

---

## 6. Workstream D — Rendering Quality (markdown / diff / tables)

### D1. Syntax-highlighted diff bodies with background tint — ROI high, M

**Problem (verified).** `tui-ui/src/widgets/diff_display.rs:165-268` renders
hunk content as monochrome-per-side foreground spans; no syntect anywhere in
tui-ui; themes have only `diff_added`/`diff_removed` fg tokens (no bg); hunk
gaps show `@@ -a +b @@` with no skipped-line counts. This is the single most
visible rendering delta between the two TUIs for edit-heavy sessions.

**Decision.** Keep the seam intact: `app/tui` composes, `tui-ui` paints.

- `tui-ui`: extend the diff line render path to accept optional pre-styled
  content spans per line (the widget then layers add/remove **background**
  tint + word-emphasis over token colors); add `diff_added_bg`/
  `diff_removed_bg` theme tokens across all built-in themes (pick tints that
  survive both polarities; ANSI themes fall back to the current fg-only
  rendering). Add unchanged-line-count separators between hunks
  (`⋯ 12 unchanged lines`).
- `app/tui`: at the existing composition site
  (`transcript/render/tool_result.rs:217` + `:876`), highlight hunk content
  per file extension via the already-public
  `coco_tui_markdown::highlight_code_lines`, and hand styled spans to the
  widget. tui-ui never depends on tui-markdown/syntect.
- **Drop** grok's background full-file upgrade worker and its benches for the
  first pass — coco's diffs are immutable cells; per-hunk highlight at derive
  time is sufficient.

**Tests.** Snapshot: mixed-language diff, add/remove tint + token colors,
separators; ANSI-theme fallback snapshot; theme-token audit test.

### D2. Table cell wrapping + proportional column widths + styled cells — ROI high, M

**Problem (verified).** `tui-markdown/src/lib.rs`: cells are flattened to
plain `String` at parse time (`:916-918`, `:963-966` — bold/italic/code/link
styling discarded); `finish_table` caps every column uniformly at
`(budget/cols).clamp(3,40)` (`:1114-1115`); `pad_cell` truncates overflow with
an ellipsis (`:1281-1303`) — **silent data loss** on any cell over 40 columns;
one visual line per row always.

**Decision.** Re-implement grok's algorithm natively in the Writer (no grok
types): cells become `Vec<Span<'static>>`; column sizing = min-word floors +
proportional distribution of remaining budget by "want" (natural width);
cell wrapping via `textwrap` producing multiple visual lines per logical row;
width-aware (CJK=2) throughout via existing truncation utilities. **Skip**
everything tied to grok's owned scrollback (`table_geometry` selection/export,
`TableHyperlink` machinery).

**Tests.** Snapshots: skewed tables at 60/100/140 cols, CJK cells, inline code
+ links inside cells, degenerate (1-col, 12-col) shapes.

### D3. OSC 8 hyperlinks (bare-URL detection + flicker-safe link layer) — ROI medium, L

**Problem (verified).** Zero OSC 8 emission workspace-wide;
`tui-markdown` `finish_link` appends a visible ` (url)` with an explicit
comment that the paint engine has no OSC 8 plumbing (escape sequences in span
content would corrupt width-aware wrapping — correct). Links are never
clickable; URLs cost columns.

**Why this fits native scrollback unusually well (from the re-survey):** most
terminals persist OSC 8 in their own scrollback, so links stay clickable after
rows scroll away — grok's `VisibleLinkMap`/mouse machinery exists only because
its rows die when they leave its buffer. **Skip all of it.**

**Decision — two phases:**

- **Phase 1 (the value):** link metadata as a *sidecar*, never in span text.
  `tui-markdown` records `LinkSpan { source_range, url }` per line; bare-URL/
  file-path detection via `linkify` over prose text. Because
  `render_history_rows` word-wraps (`history_insert.rs:165-173`) and
  `history_reflow` re-wraps on resize, link column ranges are computed
  **post-wrap** against final rows. Emission: `history_insert` serialization
  and the viewport cell-flush wrap maximal same-link runs in one OSC 8
  open/close (control-char-sanitized, guaranteed close). Capability-gated
  (G3; conservative default allowlist: iTerm2, WezTerm, kitty, Ghostty,
  VTE ≥ 0.50, tmux ≥ 3.4 passthrough).
- **Phase 2 (optional):** clickable links in the *live viewport*. As of
  ratatui 0.30.1, do **not** build grok's bespoke link layer
  (`set_frame_links`/`flush_with_links`) — use upstream
  `CellDiffOption::ForceWidth` instead: embed the OSC 8 open sequence in the
  link run's first cell with forced visible width (reference: the `tui-link`
  widget; correctness fix for this machinery landed in 0.30.2 #2587).
  Prerequisite: the 0.30.2 upgrade + teaching coco's custom
  `buffer_updates()` to honor `CellDiffOption` (§2.4). Build only if Phase
  1's viewport-repaint-clears-links proves visible in practice.

`LinkSpan` is plain geometry — seam-safe in tui-ui.

### D4. LaTeX → Unicode rendering — ROI medium, L

**Problem (verified).** Parser options omit `ENABLE_MATH`
(`tui-markdown/src/lib.rs:104-108`); `Event::InlineMath/DisplayMath` render
literally (`:705-706`). Any model output with math shows raw TeX noise.

**Decision.** Port grok's `latex/` Unicode converter (~1.7k LoC pure,
panic-free, well-tested — the cleanest architectural fit in the portfolio)
into `tui-markdown/src/latex/`; enable `ENABLE_MATH`; replace the literal
fallthrough with the converter (raw-source fallback kept). **Collapse** grok's
1.3k-LoC streaming delimiter normalizer into a simple normalize-before-parse
pass — coco re-parses from full accumulated source, so a length-changing
mid-stream rewrite would break the `starts_with` prefix invariant in
`transcript/stream.rs`; normalizing the full source before parse sidesteps
that entirely. Ship in two stages: delimiter normalization + inline math
first, display-math blocks second.

### D5. Mermaid `sequenceDiagram` rendering — ROI medium, M

**Problem (verified).** `tui-mermaid/src/lib.rs:58-62` matches only
`DiagramData::Graph`; sequence diagrams — among the most common LLM outputs —
render as raw fences (asserted by `lib.test.rs:49-50`).

**Decision (verification cut effort from L to M).** The pinned
`mermaid-rs-renderer` upstream **already exports**
`DiagramData::Sequence(SequenceData)` with lifelines, message geometry,
frames, notes, activations. coco only needs a projection:
`render_sequence(&Layout, &SequenceData, styles, cols)` mapping float geometry
onto the existing `CellGrid`/quantize/aspect machinery, exactly parallel to
`render_graph`. Use grok's ~500 LoC sequence module (`xai-grok-markdown/src/
mermaid.rs:3070+`) only as a visual/glyph reference (arrowheads, lifeline
dashes, activation bars). `catch_unwind` containment at `lib.rs:54` already
covers the new arm. Snapshot tests + density guard tuning.

### D6. Line-citation fence info resolution (`start:end:path`) — ROI low, S

`find_syntax` (`tui-markdown/src/highlight.rs:138-167`) has no branch for the
`37:65:src/foo.rs` citation form some models (notably Grok — now a supported
subscription in coco) emit as fence info; such blocks render unhighlighted.
Fix: ~25-line `parse_line_citation_fence_info` (`splitn(3,':')`,
digit-validated range, non-empty path) + resolve grammar via
`Path::extension()` before the alias table; thread through `tier_allows`.
Skip surfacing the range in the header for now.

---

## 7. Workstream E — Transcript Reader (Ctrl+O) Features

Everything in this workstream deliberately lands in the overlay reader —
coco's one owned-buffer surface — because the native-scrollback main surface
cannot host it (the terminal owns those rows). This is the architectural
translation of grok's owned-scrollback UX.

### E1. In-transcript full-text search — ROI high, L

**Problem (verified).** No transcript search exists: Ctrl+Shift+F is ripgrep
over workspace files, Ctrl+R is composer history (C5), the reader has only
scroll/select/expand. Users cannot find text in a long conversation.

**Grok reference.** `scrollback/search.rs`: per-entry **rendered plain text**
corpus keyed by `content_generation` (so "is really important" matches across
`**really**` styling); `Arc<[IndexedEntry]>` handed to a daemon thread;
keystrokes enqueue only; `drain_to_latest` coalesces bursts; stale-query
results dropped; n/N wrap; reveal scrolls the match into view.

**Decision.** Port the design onto reader primitives that already exist:

- Corpus: derived per-cell plain text (from `RenderedCell` lines, styling
  stripped), keyed by cell uuid + content hash; built lazily on first search,
  updated incrementally on cell append/change (I-2 makes staleness detection
  trivial).
- Matching: start synchronous (substring + smart-case) in the update path;
  the corpus scan for even multi-thousand-cell transcripts is cheap relative
  to a keystroke. Add the daemon-thread offload only if profiling demands it
  (grok's daemon exists because its corpus includes full rendered history of
  unbounded sessions).
- UX: search field in the reader footer; match count `k/N`; n/N navigation;
  reveal via the existing `TranscriptScrollPosition::Anchor { cell_id,
  offset_rows }`; per-line match highlight spans computed at render.
- State: `TranscriptSearch { query, matches: Vec<(CellId, LineIdx, Range)>,
  cursor }` on the reader's overlay state (I-3: view state stays in the
  overlay struct).

### E2. Sticky turn-prompt headers — ROI medium, M

**Problem (verified).** The reader gives no indication which user turn the
viewport is inside while scrolling a long wall of output.

**Grok reference.** `sticky.rs`: pure 1D math —
`PromptDescriptor { y_virtual, full_height, min_height, sticky }` →
`compute_sticky_layout(scroll, viewport, prompts)` returning pinned + pushed
headers with gradual collapse and push-off transition; ~900 lines of
exhaustive scroll-sweep tests; a `scroll_for_content` invariant keeping the
bottom line stable during collapse.

**Decision.** Port the algorithm (it is rendering-free plain-integer math —
an unusually direct port) as a domain-free module; grok's test corpus ports
with it. Home: `tui-ui` (plain usize/u16 in/out) with the reader supplying
user cells as descriptors and heights from its existing per-cell measurement
(B3's retained cache makes this cheap). Render pinned headers as an overlay
band at the top of the reader viewport.

### E3. Per-cell copy affordances — ROI medium, S

**Problem (verified).** `/copy` reaches only recent assistant prose + fences;
the reader has cell selection (`selected_cell_id`) and tool cells carry
`call_id`/`tool_name`/`input`, but **no copy action exists on a selected
cell** (reader keys: only select/toggle/scroll).

**Decision.** Near-drop-in: two reader keybindings — `y` copy cell text,
`Y` copy cell meta — where meta is kind-dependent ("copy cmd" for shell tool
cells, "copy path" for file tools, "copy url" for web tools), extracted from
the already-present tool accessors in `transcript/derive.rs:423-472`; route
through the existing clipboard stack (OSC 52 + temp-file fallback) + toast.
UTF-8-safe truncation via `coco_utils_string` for any preview text.

### E4. Expand-after-commit re-print ring — ROI medium, M

**Problem (verified).** `ToggleToolCollapse` (`update.rs:1113`) only mutates
live-viewport rendering. Once rows are committed to native scrollback they are
frozen — a tool output committed collapsed dead-ends at `… +217 lines`; the
only recourse is opening the reader and finding the cell.

**Grok reference — purpose-built for coco's architecture.** Grok's own
native-scrollback ("minimal") mode invented this: every entry committed in a
folded display mode is recorded in a bounded `commit_expand_ring` (VecDeque,
cap 256); since committed terminal text cannot be mutated, **expansion is a
re-print** — Ctrl+E pops the most recent still-live id and re-prints the full
render below; a `pending_expand` queue requeues at the front on failed writes.

**Decision.** Port the mechanism keyed on coco identity: ring of
`(message_uuid, call_id)` for cells committed collapsed; Ctrl+E (when the
reader is closed) re-emits the cell's full render as a new history block
through the **existing single commit owner** (`surface/stream.rs` commit path
— never a second emitter), with a small "re-printed from turn N" header line.
Skip ids invalidated by rewind/clear (the ring stores uuids; the derivation
layer validates liveness).

### E5. Nested subagent conversation views (read-only) — ROI medium, L

**Problem (verified).** `surface/agent_view.rs:1-11` documents the overlay as
summary-only ("the per-message transcript is deliberately not loaded");
`SubagentInstance` carries only `TaskActivity` summaries; no CoreEvent
forwards child message streams.

**Decision.** Redesign-inspired-by, not grok's `Box<AgentView>` nesting
(child views owning scrollback + prompt would violate the single-commit-owner
and derived-cell invariants):

1. Engine: opt-in CoreEvent (or extended ServerNotification) forwarding child
   messages keyed by subagent id — emitted only while a viewer is attached
   (`with_event_sink` convention).
2. `app/tui` state: per-subagent message buffer in `SessionState`, derived
   into `TranscriptCells` by the same pure derivation (preserving I-2).
3. UI: upgrade the existing agent-view overlay to render a scrollable child
   transcript via the reader's widget stack. Read-only; interaction (answer a
   child's permission prompt) belongs to G8's protocol work.

---

## 8. Workstream F — Session, Queue & Startup UX

### F1. Session picker upgrades — ROI high, L (phase 1 is M)

**Problem (verified).** `SessionBrowserState`/`SessionOption` is a flat
`{id, label, message_count, created_at}` list
(`state/surface_payloads.rs:349-364`); filter is label-substring; no
cwd/repo context, no preview, no content search, created-at ordering only.
The engine-side `SessionSummary` **already persists** cwd/updated_at/title —
they are dropped at the TUI boundary.

**Decision — two phases.**
- **Phase 1 (M):** plumb cwd/updated_at/title into `SessionOption` (the
  `OpenSessionBrowser` path in `app/cli/src/tui/session_switching.rs`); repo
  grouping with the current cwd's group pinned first; non-selectable group
  header rows via the `Vec<Option<item>>` index-mapping pattern (or a generic
  "select list with header rows" extension in tui-ui); relative times via the
  shared `format_age`; in-place row expansion showing metadata + first/last
  message preview.
- **Phase 2 (L driver):** transcript-content search lane — new `app/session`
  scan API over persisted transcripts, streamed results, dedup against title
  matches, selection anchoring while results stream.

### F2. Queue management (pane + in-place edit + snapshot broadcast) — ROI medium, M→L

Three verified, layered items:

- **F2a — interactive queue pane (M).** Today: a read-only 6-row dimmed strip
  (`widgets/queue_status_widget.rs`), previews engine-truncated to 80 chars,
  and the only mutation is recall-ALL. The dangling seam:
  `UserCommand::EditQueuedCommand{id}` is **fully handled end-to-end**
  (driver → agent-host → engine `remove_by_id` → `QueuedCommandEditReady`)
  with zero UI senders. Build: focusable queue pane keyed by the stable
  `QueuedCommandDisplay.id` — Enter/e recall-one (pure key wiring to the
  shipped backend), `x` delete-one (one new `UserCommand::RemoveQueuedCommand`
  + driver arm reusing `remove_by_id` + `CommandDequeued`), `v` view full
  text (needs F2c or a fetch).
- **F2b — in-place edit state machine (M–L, after F2a).** Add
  `CommandQueue::update_by_id` (preserving id/priority/origin/position),
  composer mode `EditingQueued { id, original }` with draft stash/restore, and
  a drain-hold flag checked by `dequeue_next_prompt_batch` while the front
  item is being edited (wake on edit exit). Skip grok's server-shared queue
  semantics.
- **F2c — full-queue snapshot broadcast (M).** Replace the three incremental
  wire events + count-truncation reconciliation (self-described fragile,
  `protocol.rs:779-821`) with a `ServerNotification::QueueChanged { entries }`
  snapshot (id, full text, kind/origin, editable, position) emitted off the
  existing `mark_changed`/`subscribe_changes` watch revision. Late-attaching
  surfaces (SDK, resumed TUI, hub UI) become correct for free. **Skip**
  version preconditions/optimistic echo — no remote queue mutation exists yet.

### F3. Project picker (non-project-dir first-prompt interception) — ROI medium, M

**Problem (verified).** Launching from `$HOME`/Downloads silently makes that
dir the project (CLAUDE.md discovery, session slug, permissions all key off
cwd); no nudge, no recent-projects affordance anywhere.

**Decision.** ~100-LoC pure classifier (git-ancestor ⇒ project; home/downloads
/tmp ⇒ not) in a small utils crate or `utils/git` adjunct; recent dirs from
`app/session`'s cross-project catalog (dedupe by cwd, newest 5); interception
at first prompt submit (not startup — zero cost on the happy path, slash
commands exempt) via the existing question/select modal; dir switch routes
through the session-restart path (the one tricky piece — reuse the existing
directory-change machinery rather than in-place cwd mutation); opt-out
persisted via `write_user_setting`.

### F4. Welcome screen (pre-session surface) — ROI medium, L

**Problem (verified).** First run = blank composer; a missing credential
surfaces as a failed first turn; auth is reachable only via `/login`; resume
plumbing exists but is never offered; a `TrustState` modal exists unwired.

**Decision.** Build as a coco modal-pane surface (`modal_pane/welcome.rs` +
payload + renderer), **not** grok's owned full-screen buffer: logo band,
unauthenticated detection → login menu (reuse `login_picker`), inline recent
sessions (reuse the resume listing path), folder-trust gate (wire the dormant
`TrustState`), dismissed into normal flow on first submit. Drop grok's mouse
hit-testing, hero centering, and business arms — that descopes this from XL
to L.

### F5. Crash reporting (panic-first) — ROI medium, L

**Problem (verified).** Nothing is persisted on any crash; no startup crash
detection; `process-hardening` would suppress core dumps if wired.

**Decision.** Split by unsafe-budget:
- **Now (zero unsafe):** persist a symbolicated report (backtrace, version,
  OS, session id — secrets redacted via `utils/secret-redact`) from the
  existing panic hook into `<config_home>/crashes/` (bounded history, 5);
  `check_previous_crash()` in `app/cli` main surfaces a one-line notice +
  report path at next startup.
- **Later (with A5's crate, after sign-off):** signal-path blob capture
  (async-signal-safe raw dump, symbolicated on next startup) — grok's
  design, but only if field evidence shows non-panic crashes matter (coco's
  profile is panic-dominated safe Rust).

### F6. Terminal title + OSC 9;4 progress + notification config — ROI medium, M

**Problem (verified).** Zero title writes and zero progress sequences
workspace-wide; exactly two hardcoded `notify()` sites, one of which
(`app.rs:767-771` attention-request) fires with **no focus gating or dedup**
(confirmed bug); no user notification config.

**Decision.** Three thin pieces: pure OSC builders (title OSC 0/2, progress
OSC 9;4 with per-brand support gates) in `tui-ui` next to
`widgets/notification.rs`; a `TitleManager` state machine (idle/busy/
attention, focus-conditioned, deduped) driven from turn lifecycle events in
`app/tui`; `NotificationsConfig` in `coco-config` (per-event: always/unfocused/
never). Fix the ungated attention bell in the same change. Multi-session tab
identification is the payoff.

### F7. Contextual ephemeral tips — ROI medium, M

**Problem (verified).** No discoverability layer (vim mode, Ctrl+O, chords,
external editor are undiscoverable); the existing Ctrl+R hint is implemented
on the wrong primitive (a stacking, occlusion-blind toast).

**Decision.** Port the ~150-line primitive (grok's file is mostly docs):
single-slot `EphemeralTipState` with dedup-key TTL refresh, per-session
seen-cap, occlusion-paused TTL, clear-on-submit. Home: `tui-ui/src/widgets/`
(domain-free, matches the notification precedent); seen-counts on `UiState`;
tick wiring on the existing animation tick. Migrate the Ctrl+R hint onto it;
add at most two detectors initially. Anti-nag semantics are the point — do
not ship more than one visible tip slot.

---

## 9. Workstream G — Architecture & Extensibility

### G1. Effects-as-data update seam — ROI medium, L (strategic)

**Problem (verified).** `handle_command` is
`async fn(&mut AppState, cmd, &mpsc::Sender<UserCommand>) -> bool` — side
effects are inline sends threaded through 20+ files; tests are runtime-bound
(79 `#[tokio::test]`s in `update.test.rs` alone, with try_recv bookkeeping
shims); there is no single point to log/inspect/deny effects. Grok's reducer
returns `Vec<Effect>` and has 1,103 plain `#[test]`s.

**Decision — generalize coco's own precedent, port nothing.**
`update/exit.rs::ExitEffect` already models the pattern. Introduce
`UiEffect` (`SendUserCommand(UserCommand)`, `Quit`, `Toast`, `SpawnTask`, …)
in `update/effect.rs`; convert `handle_command` submodules **one at a time**
from `async fn(..., command_tx)` to `fn(...) -> Vec<UiEffect>`; `App::run`
owns the single executor draining effects into `command_tx`. The long tail is
`bottom_pane`/`modal_pane` handlers that currently hold `command_tx` — convert
opportunistically; both styles coexist during migration. Payoff: deterministic
runtime-free update tests with `assert_eq!(effects, vec![...])`, and the
audit chokepoint. Do this as the substrate refactor before (or with) the next
large update-layer feature; C1/C9/F2 do not need to wait for it.

### G2. Settings registry + searchable settings modal — ROI high, XL (strategic)

**Problem (verified — the largest UX hole found).** coco's entire settings
surface is 264 LoC (4 fixed tabs; the only editable values are two display
toggles + output style + permission mode), against a config system spanning
features, compaction, MCP, memory, providers, tui.performance — none editable
in-TUI, none searchable, no restart-required signaling. No settings metadata
exists anywhere.

**Decision — take two ideas, rebuild on coco primitives, do not adapt grok's
12.6k-LoC modal.**
1. **`SettingsRegistry`** (new `app/tui/src/settings_registry/` — needs
   `coco_config` + i18n, so app/tui, not tui-ui): pure metadata per setting —
   key path, `SettingKind` (bool/enum/number/string/theme/keybinding-ref),
   label/description i18n keys, search keywords, write target
   (user/project settings.json), `restart_required: bool`, feature-gate.
2. **Modal** as `modal_pane/settings_v2/` split per the 800-LoC rule
   (registry/browse/picker/editor/render): searchable flat list (reuse the
   `filter_focused` pattern from plugin_dialog), category browse, inline
   editors per kind, **preview-mutates-live / persist-once-at-commit**
   discipline (grok's load-bearing interaction idea), `write_user_setting`
   for persistence (already exists), restart-required toast.
Phase: registry + read-only browse/search first (M) — it is immediately
useful as documentation — then editors per kind.

### G3. Terminal brand/capability model (slim) — ROI medium, M

**Problem (verified, overstated by the original candidate).** Detection is
fragmented across five files (keyboard_modes, tui-ui color, notification,
clipboard_copy, compatibility), each re-sniffing env inconsistently; and
Shift+Enter/Opt+Enter newline is broken on Apple Terminal (no kitty support) —
though coco already ships three working fallbacks (Alt+Enter, Ctrl+J,
backslash+Enter continuation), so this is polish.

**Decision — slim adaptation, not grok's full `TerminalContext`** (XTVERSION
probe, ModifierFate matrix, KeyboardNormalizer solve problems coco's
kitty-flags approach already handles):
- A small `TerminalBrand`/`Capabilities` struct in `app/tui`, built once at
  startup from an injected env map (testable without env mutation), consumed
  by: A3's out-of-band-repaint gate, D3's OSC 8 route, F6's progress/title
  gates, notification backend selection. Existing detection sites migrate to
  it opportunistically.
- A ~60-LoC `utils/macos-modifiers` crate wrapping
  `CGEventSourceFlagsState` (the one unavoidable `unsafe extern`; isolated per
  the wrap-unsafe-deps rule; `cfg(target_os = "macos")` with no-op fallback)
  so modified-Enter can be rescued on Apple Terminal.

### G4. Color degradation tiers (Basic/None + NO_COLOR) — ROI medium, M

**Problem (verified).** `ColorCapability` has exactly TrueColor/Ansi256
(`tui-ui/src/color.rs:16-21`, `#[non_exhaustive]` — explicitly designed for
extension); `detect_from_env` bottoms out at Ansi256, so `TERM=ansi` boxes
receive `38;5;N` they cannot render; `NO_COLOR` appears nowhere in the TUI;
the hand-tuned `DarkAnsi`/`LightAnsi` themes exist but are manual picker
choices only.

**Decision.** Extend in place: add `Basic` and `None` variants; NO_COLOR check
+ 16-color/dumb-TERM branch in `detect_from_env`; RGB/Indexed→ANSI16
quantization in `adapt_color` (`Theme::downsample` is already
capability-parameterized); one shell-side wiring change auto-substituting
DarkAnsi/LightAnsi at Basic and a monochrome pass at None.

### G5. Live OS dark/light theme follow — ROI medium, M

**Problem (verified).** The OSC 11 probe runs exactly once per process
(`system_theme_probe.rs:19-28`, OnceLock-guarded); toggling OS dark mode
mid-session leaves a stale theme; `auto` maps only to built-in Dark/Light
(`theme/config.rs:797-812`) — daltonized/custom themes cannot participate.

**Decision.** Poller (the `dark-light` crate) feeding the **existing**
theme-reload channel (`reload_rx` → `apply_theme_reload`, `app.rs:560` — the
chokepoint already exists for theme.json hot-reload); `auto` becomes a
configurable pair `{ auto_dark: ThemeName, auto_light: ThemeName }` resolved
through the existing registry. Watcher respects runtime setting changes.

### G6. Terminal-native (polarity-safe) built-in theme — ROI medium, S

All six built-in themes are polarity-committed; the OSC 11 auto-probe guesses
wrong exactly on mismatched/SSH/degraded profiles. Add
`ThemeName::Terminal`: body/chrome = `Color::Reset`, sparse ANSI-16 accents,
DIM-based secondary text (avoid the hardcoded DarkGray that washes out on
tuned dark profiles) — legible by construction on every profile, zero
detection. Picker exposure is free via `ThemeRegistry::choices()`.

---

## 10. Strategic Items (design-review first)

### G7. Fleet dashboard — ROI medium, XL

coco's multi-agent UI today: read-only agent overlay, an inline switcher
(view/stop only), a team roster that can only cycle permission modes, and a
resume picker. Grok's dashboard (24k LoC) proves the UX: grouped/pinned agent
rows, a peek panel that can **reply and answer permissions**, attach popup,
dispatch-from-overview.

**Not a TUI-only change:** coco must first add protocol read models (peek
another session's recent messages + pending permission/question state) to
`app/server` — grok gets this free from in-process `app.agents`. Port the
concepts (row identity, peek-and-reply, answer-from-overview); build the
grouped-row/two-pane widgets as domain-free tui-ui additions; land after E5's
event forwarding exists. Phase: read-only fleet list → peek → reply/answer →
dispatch.

### G8. Session hunk-tracking service ("review what the agent touched") — ROI medium, XL

Verified narrower than proposed: coco already owns the two hardest inputs —
`core/context/src/file_history.rs` (per-turn content-addressed snapshots,
`track_edit` write-path interception) and `changed_files.rs` (external-change
detection). What's missing is the session-level hunk model with agent-vs-user
attribution, per-hunk accept/reject, and a review pane (`/diff` is a raw
6000-char `git diff` dump). Design a coco-native service crate on those
primitives + `utils/git` + `utils/file-watch`, with an opt-in event sink; do
**not** transplant grok's 13k-LoC actor and event taxonomy. Requires its own
design doc before implementation.

---

## 11. Deferred / Opportunistic (Tier 3)

| ID | Item | Note |
|---|---|---|
| c33 | OSC 12 cursor-color theming | S; clean fit (emit at `apply_theme_runtime`, reset via A4's ledger + suspend/resume); cosmetic — filler task |
| c34 | Overlay paired benches | folded into B7 |
| c36 | Searchable shortcuts cheatsheet | M; build only type-to-filter over `keymap::KEYMAP` if help discoverability becomes a real complaint |
| c37 | Glyph-capability module | Skip unless legacy Windows ConHost becomes a target; on modern terminals the fallback funnel is an identity function |
| c52 | Input flight recorder | M; clean design (alloc-free ring + privacy-scrubbed dump) but developer tooling — build when an unreproducible-input-bug backlog exists |
| c12 | JoinSet task seam | Skip-for-now; coco's nine bespoke channels are persistent watchers the pattern wouldn't replace; revisit when G1 lands and a first one-shot consumer appears |
| c15 | Test fixtures | Half-day: hoist `drained_channel`/`next_user_command` helpers + 3–4 scenario builders when tests are next touched; `AppState::new()` already solves grok's constructor pathology |
| c32 | Citation fences | see D6 |
| c30 | Image preview | thin slice only, see C8 |

---

## 12. Phasing & Sequencing

Dependencies that order the work: A4 → A5; A1 → A6; C1 → C3 → C7/C8;
B3 → B7; F2a → F2b; G3 feeds A3/D3/F6; E5 → G7; G1 is a rolling substrate
refactor. The ratatui 0.30.2 upgrade (§2.4 — `cell.skip` →
`CellDiffOption` migration in the custom diff) is a standalone S chore that
must precede D3 and any C8 revisit.

**Phase 1 — quick wins (all S/M, no architectural risk):**
B1 (fence memo) → A1 (cursor dedup/idle frames) → B8 (diff regression
tests; the copy elision rides along with the ratatui 0.30.2
`CellDiffOption` migration, §2.4) → A2 (resize quiet period) → A3 (focus
heal) → B6 (bounded drain) → B3 (+B7) (overlay cache) → B4 (purge sites) →
C6 (fuzzy highlights) → C1 (undo/redo) → C2 (soft-wrap) → C9 (permission
prompt).

**Phase 2 — rendering quality + terminal robustness:**
D1 (diff highlight) → D2 (tables) → A4 (mode ledger) → A5 (fault handler;
unsafe sign-off first) → C5 (Ctrl+R) → G4 (color tiers) → G6 (terminal theme)
→ E3 (per-cell copy) → D5 (mermaid sequence).

**Phase 3 — structural features:**
C3 (text elements) → B2 (append-only streaming) → E1 (transcript search) →
F1 (session picker) → D3 (OSC 8 phase 1) → G5 (live theme) → B5 (memory
trace) → E4 (expand ring) → F2a/F2c (queue pane + snapshot) → E2 (sticky
headers) → D4 (LaTeX) → F6 (title/progress) → F7 (tips) → F3 (project
picker).

**Phase 4 — strategic (design review before code):**
G2 (settings registry/modal) → G1 (effects-as-data, rolling) → A6 (writer
thread) → A7 (PTY harness, grows throughout) → E5 (subagent views) → F4
(welcome) → F5 (crash reports) → G7 (fleet dashboard) → G8 (hunk tracking).

---

## 13. Verification & Acceptance

- Every engine-visible item (A1–A3, D3) adds a recording-backend test *and*
  one PTY scenario to A7's suite as its acceptance gate.
- Every perf item states its observable: B1/B2 via `tui-ui/benches/render.rs`;
  B3 via B7's paired bench; A1 via the `frame_skipped` perf counter and the
  idle-frame byte count; B4/B5 via the memtrace artifact itself.
- Every UI-visible item ships insta snapshots (`cargo insta pending-snapshots
  -p coco-tui`).
- Items touching teardown (A4, A5, A6, c33) must pass the full
  suspend/resume + `$EDITOR` handoff + panic-path matrix in
  `terminal.test.rs`.
- The `just quick-check` / `just pre-commit` discipline and the tui-ui seam
  guard apply to every change; no item may add `tui-ui` deps beyond
  ratatui/crossterm/unicode/std/textwrap without updating the seam guard
  deliberately.
