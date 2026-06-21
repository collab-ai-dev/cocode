# TUI Rendering Architecture: jcode vs coco-rs

Source-level comparison of how each harness drives its terminal UI: the
display substrate, the frame loop and pacing, the content/scrollback
pipeline, streaming coalescing, capability detection, and the caching
strategy. Every claim below is anchored to a file:line that was read on
both sides; README marketing numbers are treated as unverified until
substantiated in source.

The two projects make an **opposite foundational choice** about where the
transcript lives — and almost every downstream difference (RAM profile,
resize cost, scroll feel, cache strategy) follows from it. That choice is
a documented design target for coco-rs, not an accident, so most
differences are trade-offs rather than deficiencies.

---

## jcode approach

**Substrate — fullscreen alt-screen, simulated scrollback.**
jcode runs a fullscreen alternate-screen ratatui terminal. `ratatui::init()`
(`src/cli/terminal.rs:91`) enters alt-screen + raw mode and returns a
`DefaultTerminal` whose backing `Buffer` is the *whole screen*. There is
**no native-scrollback use** — the scrollback is *simulated inside the
alt-screen buffer*. Mouse capture / focus / keyboard-enhancement are
toggled per a performance policy (`init_tui_runtime`, terminal.rs:108-139).
This is precisely why jcode's README (line 297) says it needed a custom
terminal ("handterm") to recover smooth partial-line scroll: a stock
emulator's native scroll is unused, so anything below full-frame control
is coarse.

**Frame loop & pacing.**
The local/remote loops live in `src/tui/app/run_shell.rs:209-466`, a
`tokio::select!` over crossterm events, a `redraw_interval` tick, an 80 ms
`status_spinner_interval`, `handterm_native_scroll.recv()`, and
`bus_receiver.recv()`. A `needs_redraw: bool` gates the full paint, so idle
frames cost nothing.
- **Adaptive redraw interval** (`src/tui/mod.rs:1128-1206`, constants
  1038-1042): `REDRAW_IDLE` = 250 ms, `REDRAW_DEEP_IDLE` = 5000 ms after
  30 s of no activity, `REDRAW_PASSIVE_LIVENESS` = 1000 ms while
  processing-but-not-streaming, and `fast_interval = 1000/redraw_fps` while
  actively streaming/animating. The default `redraw_fps`/`animation_fps` is
  **60**, clamped 1-120 (`jcode-config-types/src/lib.rs:572-573`).
- **Single-cell spinner fast path** (`run_shell.rs:151-205`,
  `STATUS_SPINNER_FPS` = 12.5): while a turn is processing and the streaming
  body is empty/left-aligned, `draw_status_spinner_only` clones the last
  frame buffer (`current_buffer.clone_from(previous_frame)`), stamps the new
  spinner glyph into the cached status `Rect` via `set_stringn`
  (run_shell.rs:196-204), save/restores the cursor, and flushes through
  `Terminal::flush` so ratatui's diff emits ~1 cell. It **never calls
  `ui::draw`** for a spin tick.
- **Per-tier degradation** (`src/perf.rs`): `detect()` (perf.rs:270-304)
  reads `SSH_CONNECTION`/`SSH_TTY`, WSL (incl. `/proc/version`), terminal
  program, 1-min load ÷ CPU count, and available RAM, and `compute_tier`
  (perf.rs:306-356) returns `Full`/`Reduced`/`Minimal`. **SSH ⇒ Minimal
  immediately** (perf.rs:315-317). `tui_policy_for` (perf.rs:180-243) then
  clamps `redraw_fps` (Reduced ≤ 30, Minimal ≤ 12), forces
  `enable_decorative_animations = false` on WSL/Windows-Terminal/Minimal,
  and disables focus/keyboard-enhancement on WSL+Windows-Terminal. The tier
  is also a user-visible badge and a config override
  (`display.performance` = full/reduced/minimal, perf.rs:19-25, 287-292).

**Content/scrollback pipeline — the heart.**
`src/tui/ui_prepare.rs` builds a `PreparedChatFrame`: a flat
`Vec<Line<'static>>` of *all* wrapped transcript lines plus side-indices
(`wrapped_user_indices`, `wrapped_user_prompt_starts/ends`, `copy_targets`).
`src/tui/ui_viewport.rs::draw_messages:237-407` renders **only the visible
slice** via `materialize_line_slice(scroll, visible_end)` (ui_viewport.rs:319),
with `lower_bound`/`partition_point` binary searches (ui_viewport.rs:310-317)
over a precomputed wrapped-line-count index (`total_wrapped_lines()`,
ui_viewport.rs:258) to locate visible prompts and copy badges. Per-frame
work is therefore **O(viewport height)**, not O(history), even though the
full transcript is resident.

**Three-level render cache, all keyed on width + version + display modes:**
1. **Full-prep frame cache** (`ui_prepare.rs:459-515`,
   `FullPrepCacheKey { width, height, diff_mode, messages_version,
   streaming_text_hash, … }`): a redraw that changes nothing returns the
   cached `Arc<PreparedChatFrame>` with zero rebuild.
2. **Body cache + incremental reuse** (`ui_prepare.rs:669-728`): on a miss
   it calls `take_best_incremental_base` + `prepare_body_incremental`
   (730+) to re-wrap **only the messages appended since the cached base**,
   cloning the prior `wrapped_lines` and extending. Streaming text gets its
   own section cache.
3. **Per-message line LRU** (`crates/jcode-tui-messages/src/cache.rs:51-127`):
   a global 2048-entry LRU (`OnceLock<Mutex<MessageCacheState>>`,
   `MESSAGE_CACHE_LIMIT = 2048`) mapping
   `MessageCacheKey { width, diff_mode, message_hash, content_len,
   diagram_mode, centered, mermaid_epoch, mermaid_aspect_bucket }` →
   `Arc<Vec<Line>>`. `get_cached_message_lines` (cache.rs:91-127)
   short-circuits the wrap on a hit. Markdown/diff wrapping for an unchanged
   message at an unchanged width is **never recomputed**, including across
   resize back-and-forth. (jcode also memoizes the assistant
   raw-line/logical-line selection map separately, `ui_prepare.rs:11-115`
   `AssistantAuxData` + its own 2048-entry LRU, so text-selection metadata
   doesn't re-parse markdown.)

**Streaming coalescing.**
`crates/jcode-tui-core/src/stream_buffer.rs` flushes streamed text at
semantic boundaries (newline / ```` ``` ````), capped to a 96-char "smooth
frame" with a 150 ms timeout, so a large SSE burst reveals over a few frames
instead of one giant reflow.

**Frame metrics / flicker detection (observability only).**
`src/tui/ui_frame_metrics.rs` records per-draw
`DrawCallAttribution { total_ms, render_ms, backend_flush_ms,
changed_cells, total_cells, … }` (computed in `draw_full` by diffing the
previous buffer, run_shell.rs:126-147), tracks slow-frame and **flicker**
histories, and exposes cache hit/miss/incremental-reuse counters. This does
not change what is painted.

**Color & capability.**
`crates/jcode-tui-style/src/color.rs:15-59` `detect_color_capability` reads
`COLORTERM` (`truecolor`/`24bit` → TrueColor), a `TERM_PROGRAM` allowlist,
terminal env markers, and `TERM` (`*256color*` → Color256;
kitty/ghostty/alacritty → TrueColor), defaulting to Color256. When truecolor
is absent, `rgb()` (color.rs:74-80) returns
`Color::Indexed(rgb_to_xterm256(r,g,b))`, with nearest-cube
(`nearest_cube_index`, color.rs:123-128) vs grayscale-ramp
(`nearest_gray_index`) selection by weighted distance (color.rs:85-106). This
is **runtime RGB→xterm-256 downsampling**, cached in a `OnceLock`.

---

## coco-rs approach

**Substrate — main buffer, host-native scrollback (deliberately different).**
coco-rs stays in the **main terminal buffer** and uses the **host
terminal's native scrollback**. `setup_terminal` (`app/tui/src/terminal.rs:138-153`)
enables raw mode + bracketed paste + focus reporting but **does not enter
alt-screen** for the normal surface and **does not capture the mouse**,
preserving native drag-select and Ctrl-C (`crate-coco-tui.md:258-270`).
Alt-screen is entered only for large review/navigation modals
(terminal.rs:299-313). The substrate is a bespoke `SurfaceTerminal<B>`
(`surface/terminal.rs:82-557`): coco owns viewport geometry, a **double
buffer + its own cell diff** (`buffer_updates`, terminal.rs:485-510, with
wide-grapheme `skip` accounting), scroll-region history insertion, and
BSU/ESU synchronized-update framing
(`BeginSynchronizedUpdate`/`EndSynchronizedUpdate`, terminal.rs:98-105,
429-435). This choice is the documented target in
`docs/coco-rs/ui/native-scrollback-architecture.md`.

**Frame loop & pacing.**
`App::run` (`app/tui/src/app.rs:257-375`) is a `tokio::select!` over
crossterm events, the `CoreEvent` channel, async file/symbol search,
theme/keybinding/display hot-reload channels, a 250 ms `tick_interval`
(gated by `needs_tick`, app.rs:286), and a **`draw_rx` broadcast** from the
frame scheduler.
- **Coalescing 120 FPS scheduler** (`frame_requester.rs` +
  `frame_rate_limiter.rs`, ported from codex-rs): handlers set
  `needs_redraw` and call `frame_requester.schedule_frame()`; an actor task
  coalesces requests, clamps to `MIN_FRAME_INTERVAL ≈ 8.33 ms` (120 FPS,
  frame_rate_limiter.rs:12) via `clamp_deadline`, and broadcasts one `()` to
  trigger exactly one `redraw()`. The scheduler sleeps ~1 year when nothing
  is pending (frame_requester.rs:101-104), so **idle frames cost nothing**.
  This is a **cap, not a clock** — there is no free-running render loop.
- **CoreEvent coalescing** (app.rs:298-303): on a stream event it drains all
  ready `notification_rx` events via `try_recv` before redrawing, so 100+
  TextDeltas/sec collapse into one paint.
- **Self-perpetuating spinner**: while a turn or stream is active, `redraw()`
  re-arms via `schedule_frame_in(SPINNER_TICK_INTERVAL)` (app.rs:414-420,
  `SPINNER_TICK_INTERVAL = 50 ms`, constants.rs:40) — a ~20 fps spinner
  cadence with **no unconditional wall-clock timer**.

**Native-scrollback content pipeline.**
`surface/controller.rs::draw_at_inner:107-202` is the orchestrator:
- The finalized transcript is engine-authoritative
  `&[RenderedCell]` (`state.session.transcript.cells()`, controller.rs:130),
  a **pure derivation** of `coco_messages::MessageHistory` (invariants
  I-1/I-2/I-3, `app/tui/CLAUDE.md`). `transcript_view.rs:11-13` states
  outright that per-cell layout caching is **not** part of the view and is
  left to the renderer at draw time.
- **Append-only emission** (`surface/history_driver.rs`):
  `HistoryEmissionTracker` tracks exactly-once-by-UUID; the steady-state
  path renders **only the new cells** (`render_finalized_history_lines(&cells[start..])`,
  history_lines.rs:117 region) and writes them above the viewport via
  `SurfaceTerminal::insert_history_lines` (scroll-region writes,
  terminal.rs:336-404). Steady-state per-append work is **O(new cells)**.
- **Replay** (`replay_all_capped`, history_driver.rs:160-177) fires only on
  three conditions, checked at controller.rs:134-141:
  (1) `history_display_changed` (a `HistoryDisplayState` toggle —
  show_thinking / syntax-highlighting / system-reminders),
  (2) `needs_reflow_replay` (`replay_due`, a **75 ms-debounced** width/viewport
  change, `history_reflow.rs:8`), or
  (3) `needs_stream_finish_replay`. It re-renders all cells capped at
  `DEFAULT_MAX_REFLOW_ROWS = 9000` (history_lines.rs:24, 65-97).
- **Interactive viewport** (`surface/viewport.rs:43-72`): only the small
  bottom region (live tail + activity panel + composer + status bar) is
  drawn each frame through `SurfaceTerminal::draw_viewport`, which runs
  coco's own double-buffer diff and applies the cursor claim
  (`cursor::compute_cursor`, controller.rs:189). Crucially, in native mode
  `build_live_tail_lines` feeds `committed_cells = &[]` (viewport.rs:350-355,
  gated by `finalized_history_in_viewport()`, modal.rs:74-76) — **the full
  transcript is not re-wrapped per frame**; only the streaming tail and tool
  executions are.

**Streaming pacing.**
`streaming/chunking.rs::AdaptiveChunking` is a two-gear policy — Smooth
(1 line/tick) and CatchUp (4 lines/tick) — with **hysteresis** (250 ms
mode-hold) and a severe-backlog escape hatch (≥ 64 queued or ≥ 300 ms age).
Conceptually like jcode's stream buffer, but with explicit mode hysteresis
that prevents smooth/catch-up flapping.

**Capability / color.**
Terminal *notification* delivery is detected from
`$TERM_PROGRAM`/`$LC_TERMINAL`/`$TERM` with tmux/screen DCS passthrough
(`widgets/notification.rs:31+`). Native scrollback is gated only for Zellij
(`surface/compatibility.rs:7,28,34,41`, `ZellijNativeScrollbackDisabled`).
Themes emit `Color::Rgb` (`theme.rs`); an "ANSI-only" palette is a
**manually selectable theme** (theme.rs:375-378, 426) that skips RGB tints —
there is **no automatic runtime RGB→256 downsampling**.

Design intent is documented in
`docs/coco-rs/ui/native-scrollback-architecture.md`,
`engine-tui-unified-transcript-plan.md`, and
`engine-tui-phase3d-renderer-migration-plan.md` (all migration commits
landed; the production `Tui` runs on `SurfaceTerminal` +
`NativeSurfaceController`).

---

## Head-to-head comparison

### 1. Scrollback philosophy — opposite trade-offs, not better/worse
| | jcode | coco-rs |
|---|---|---|
| Surface | Fullscreen alt-screen (`ratatui::init`, terminal.rs:91) | Main buffer (`setup_terminal`, terminal.rs:138-153) |
| Old history storage | Whole wrapped transcript resident in RAM (`PreparedChatFrame`) | Written to host scrollback, **0 retained TUI RAM** (`insert_history_lines`, terminal.rs:336) |
| Scroll/select/copy | Custom; needs handterm for smooth sub-line scroll (README:297) | Terminal-native (mouse wheel, drag-select, search) for free |
| Resize | Cheap (re-wrap only changed-width tail, via caches) | Replay (re-wrap, 75 ms-debounced) |

Neither is a deficiency; they are deliberate opposite trade-offs. coco-rs's
choice is its documented target.

### 2. Per-message render caching across resize/toggle — jcode genuinely ahead
jcode caches wrapped lines at three levels (per-message LRU
`cache.rs:51-127`, incremental body `ui_prepare.rs:669-728`, full-frame
`ui_prepare.rs:459-515`), so a width change re-wraps only messages whose
`(content_hash, width, …)` key is new and toggling back to a prior width is a
pure cache hit. coco-rs has **no per-cell line cache**: `transcript_view.rs:11-13`
defers it to the renderer, and `render_finalized_history_lines`
(history_lines.rs:47-63) runs `ChatWidget::build_lines_owned()` fresh.
**Important nuance (verified):** because coco uses native scrollback, this
does **not** cost per frame — the live tail renders `committed_cells = &[]`
(viewport.rs:350-355). The cost lands only on the three replay triggers
(controller.rs:134-141): debounced resize, display-toggle, and stream-finish.
On those paths, coco re-wraps the entire (≤ 9000-row) transcript from
scratch; jcode pays only for the tail that actually changed width. So this is
a **resize/toggle-replay** optimization, not a steady-state one.

### 3. Adaptive performance-tier scaling — jcode genuinely ahead, but smaller gap than it looks
jcode's `perf.rs` measures load÷CPU, RAM, SSH, WSL, terminal program
(perf.rs:306-356) and *automatically* clamps redraw/animation FPS and
disables decorative animation on weak/remote/multiplexed hosts
(`tui_policy_for`, perf.rs:180-243; SSH ⇒ Minimal, 12 fps). coco-rs has **no
equivalent** — its only environment branch is the Zellij scrollback gate
(compatibility.rs). **But the gap is narrower than the raw claim implies:**
jcode *free-runs* a `redraw_interval` clock (README's "over a thousand fps"
headroom), which is exactly *why* it needs a tier clamp. coco-rs is
fully event-driven — 120 FPS is only a *cap* (frame_rate_limiter.rs:12,
`clamp_deadline`), CoreEvents are coalesced (app.rs:298-303), idle sleeps
(`needs_tick`, app.rs:286), and the in-turn spinner self-arms at only ~20 FPS
(app.rs:414-420). coco already avoids most of the waste the tier solves; the
residual benefit is (a) lowering the in-turn redraw/spinner cadence over a
slow SSH link, and (b) gating any *future* decorative animation. coco-rs has
**no decorative animations to disable today**.

### 4. Single-cell spinner fast path — jcode marginally ahead
jcode repaints only the spinner cell during processing
(`draw_status_spinner_only`, run_shell.rs:151-205) and never calls `ui::draw`
for the spin tick. coco-rs's 50 ms spinner re-runs the whole `redraw()` →
`render_interactive_viewport` (viewport.rs:43) rebuilds turn-activity,
live-tail, popup, and footer view models each tick. Net **wire bytes are
similar** (coco's `buffer_updates` cell-diff, terminal.rs:485-510, catches
the no-ops), but coco spends more *CPU* per tick rebuilding view models. With
a small bottom viewport this is cheap; it would matter only with a heavy
activity panel.

### 5. Color downsampling — jcode marginally ahead on 256-color terminals
jcode downsamples RGB→xterm-256 at the color layer when `COLORTERM` lacks
truecolor (color.rs:74-106); coco emits `Color::Rgb` (theme.rs) and relies on
the emulator to clamp (with an opt-in ANSI theme). On a 256-color-only
terminal jcode maps themed colors to the nearest cube entry deterministically;
coco's depend on the emulator's own (often poorer) clamp. Minor, real.

**Resource summary.** jcode trades RAM (full wrapped transcript + 2048-line
LRU + prepared-frame caches) and code complexity for CPU savings on
resize/scroll and adaptive throttling. coco-rs trades a heavier resize-replay
for **zero retained-history RAM** and native-terminal integration. Both are
event-gated and cap full draws; **neither busy-loops at "1000 fps."**

---

## Where coco-rs already matches or wins

**1. The "1000 fps" headline does not hold as an operating rate.** jcode's
default redraw cap is **60 fps** (`jcode-config-types/src/lib.rs:572-573`),
the loop is event-gated with 250 ms-5 s idle intervals (mod.rs:1038-1206),
and the spinner animates at 12.5 fps. coco-rs's 120-FPS coalescing
rate-limiter (frame_rate_limiter.rs:12) is, if anything, a *higher* hard
ceiling, and both designs paint only on dirty state. coco-rs is **not "245×
slower"** at steady-state rendering — the README number is a process-startup
PTY benchmark against the TypeScript Claude Code, not a render-throughput
comparison, and is not relevant to coco-rs.

**2. Native scrollback is a genuine architectural win for coco-rs's goals.**
coco-rs writes finalized history into the host scrollback
(`insert_history_lines`, terminal.rs:336) and never holds it in a RAM line
buffer, so very long sessions cost ~0 extra TUI RAM for transcript and the
user's terminal-native scroll/select/search/copy all work without a custom
terminal. jcode must keep the whole wrapped transcript resident and
explicitly built a new terminal ("handterm", README:297) to recover smooth
scroll — coco-rs gets terminal-native scroll for free.

**3. coco-rs ships its own cell-diff and synchronized-update framing —
equal and tear-free.** `SurfaceTerminal::buffer_updates`
(terminal.rs:485-510) is a hand-written double-buffer diff with proper
wide-grapheme `skip` accounting, wrapped in
`BeginSynchronizedUpdate`/`EndSynchronizedUpdate` (terminal.rs:98-105,
429-435). jcode relies on stock ratatui's diff and adds BSU/ESU only
implicitly; coco-rs's explicit synchronized-update bracket directly addresses
the "stale content / background bleed on rapid redraw" issues that jcode's
own `terminal-capabilities.md:67-78` warns about.

**4. Stronger transcript-correctness model.** coco-rs's transcript is a
*pure derivation* of the engine's single-source `MessageHistory` with
exactly-once-by-UUID emission and three pinned invariants (I-1/I-2/I-3,
`app/tui/CLAUDE.md`; phase3d D2 fixed per-turn re-emit so SDK observers get
one event per UUID). jcode's `App` mutates a local `display_messages` list
with a `display_messages_version` counter and reconciles via caches —
functional, but no comparable single-authority invariant. coco-rs's model is
cleaner for multi-consumer (TUI + SDK NDJSON) correctness.

**5. coco-rs's adaptive streaming chunker is at least as sophisticated.**
`AdaptiveChunking` (chunking.rs) has two gears with **hysteresis + mode-hold
+ severe-backlog escape**, vs jcode's fixed 96-char/150 ms smooth frame
(stream_buffer.rs). coco-rs's hysteresis specifically prevents the
smooth/catch-up flapping a fixed threshold can cause.

**6. Frame-requester idle cost is parity-or-better.** coco-rs's scheduler
sleeps for an effectively infinite duration when nothing is pending
(frame_requester.rs:101-104) and coalesces a whole `select!` iteration's
`schedule_frame()` calls into one paint. jcode's loop wakes on every
`redraw_interval` tick (250 ms when idle, until deep-idle at 30 s). coco-rs's
pure event-driven paint path is arguably tighter.

**Net:** coco-rs's core rendering architecture is sound and, for its stated
goals (native scrollback, multi-provider, SDK correctness), already ahead of
jcode in correctness and retained-memory. The recommendations below are
narrow CPU optimizations and an optional remote-throttle.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions with an adversarial verdict of **confirmed** or **nuanced**
are kept. For nuanced items the verified correction is folded in. All
respect coco-rs's documented non-goals.

### R1 — Memoize per-cell wrapped lines to make resize/toggle replay incremental (nuanced → medium)
**Why.** jcode memoizes per-message wrapped lines in a 2048-entry LRU
(`crates/jcode-tui-messages/src/cache.rs:51-127`, keyed by
`{width, diff_mode, message_hash, content_len, …}`, `get_cached_message_lines`
short-circuits on hit) plus an incremental body assembly
(`ui_prepare.rs:669-728`). coco-rs has **no render-line cache**:
`transcript_view.rs:11-13` defers per-cell layout caching to the renderer,
and `render_finalized_history_lines` (history_lines.rs:47-63) runs
`ChatWidget::build_lines_owned()` fresh. **Verified scope correction:**
coco-rs uses native scrollback, so this is *not* a per-frame cost — in native
mode the live tail renders `committed_cells = &[]` (viewport.rs:350-355). The
full re-wrap fires only on the three replay triggers at controller.rs:134-141.
So re-scope this as a **resize/reflow + display-toggle** optimization, not a
per-frame one. It pays off in exactly: (1) the 75 ms-debounced resize replay
(`replay_all_capped`, history_driver.rs:160-177), (2) `HistoryDisplayState`
changes (show_thinking / syntax-highlighting / system-reminders toggles,
controller.rs:124-141, which currently triggers a full re-wrap of every
cell), and (3) the overflow-trim walk (R2).

**Change.** Add a per-`RenderedCell` line cache at the renderer layer (a field
on `NativeSurfaceController` or `SurfaceHistoryDriver`, or a small LRU in
`surface/history_lines.rs`), bounded ~2-4k. Key it by the engine-authoritative
`RenderedCell.message_uuid` (transcript_view.rs) **+ width + the
`HistoryDisplayState` fingerprint already computed at controller.rs:124-137**.
Look up per cell in `render_finalized_history_lines` /
`render_replay_history_lines`; call `ChatWidget` only on misses. Keep the
append path unchanged. **Do not cache the streaming tail** (it mutates every
tick).

**Correctness.** Finalized cells are append-only and content-stable (I-2), so
a `(uuid, width, fingerprint)` key is sound. The key must include *every*
input that affects wrapped output (width, syntax-highlighting toggle,
thinking/reminder visibility, reasoning-metadata revision) or stale lines
render — reusing the existing `HistoryDisplayState` fingerprint is exactly the
right key set. **Layer.** `app/tui` (renderer). **Impact** medium ·
**Effort** medium · **Risk** medium (cache-key completeness; bounded LRU).
Respects non-goals (pure TUI-side optimization; no engine/provider change).

### R2 — Avoid O(messages × cells) re-render in the replay overflow-trim walk (confirmed → medium)
**Why.** jcode never does an O(n) re-render to find the overflow boundary:
`ui_viewport.rs:258` uses `total_wrapped_lines()`, `:310-317` uses
`lower_bound`/`partition_point` over a precomputed line-count index, and
`:319` `materialize_line_slice(scroll, visible_end)` slices the cached vector
— O(log n). coco-rs's `render_replay_history_lines` (history_lines.rs:65-97)
first renders **all** cells (line 70), then the loop at lines 81-91 calls
`render_finalized_history_lines(&cells[start..])` **once per
engine-message start** until `lines.len() <= max_rows` — each call re-wraps
that whole suffix. For a transcript far over `DEFAULT_MAX_REFLOW_ROWS = 9000`
(history_lines.rs:24) this is **O(messages × cells)**, reached on every
reflow/display-change replay (controller.rs:161/173). Debounced 75 ms and only
on huge histories, but genuinely quadratic.

**Change.** In `surface/history_lines.rs`, render each cell's lines **once**
(ideally via the R1 cache), accumulate per-cell line counts into a prefix-sum,
then `partition_point`/binary-search the suffix start whose cumulative rows fit
`max_rows`; materialize the final lines a single time. Drops the path from
O(messages × cells) to O(cells) wrap + O(log messages) search — mirrors
ui_viewport.rs:310-319.

**Correctness pin.** The truncation marker counts **engine messages, not
cells** (`engine_message_starts` walks UUID boundaries, history_lines.rs:102-112;
`replay_truncation_marker` reports omitted *messages*, history_lines.rs:114-120).
Binary-search the cell prefix-sum but report the marker count from
`message_starts` so the "... N older messages retained" line stays
message-accurate. **Layer.** `app/tui`. **Impact** medium · **Effort** low ·
**Risk** low (observable only on > 9000-row histories — add a test with that
much content). Respects non-goals. Most valuable when stacked on R1.

### R3 — Adaptive frame-rate / spinner throttle for SSH and multiplexers (nuanced → low-medium)
**Why.** jcode auto-detects SSH/WSL/load/RAM/terminal (perf.rs:270-356) and
clamps redraw/animation FPS (Reduced ≤ 30, Minimal ≤ 12; SSH ⇒ Minimal),
applied to the live cadence at mod.rs:1128-1133. coco-rs fixes
`MIN_FRAME_INTERVAL` at 120 FPS (frame_rate_limiter.rs:12) and
`SPINNER_TICK_INTERVAL` at 50 ms (constants.rs:40); the only env branch is
Zellij (compatibility.rs). **Verified down-scope:** coco-rs is **not** a fixed
clock — it is event-driven, 120 FPS is only a cap, CoreEvents coalesce
(app.rs:298-303), and the in-turn spinner is already ~20 FPS (app.rs:414-420).
So frame it as "cap the *in-turn* redraw/spinner cadence + gate decorative
animation," not "replace a fixed clock" (coco has none).

**Change.** Add a coarse 2-3 tier `TuiPerfPolicy` in `coco-config`
`DisplaySettings` (hot-reloaded via the existing `display_settings_rx` at
app.rs:324). Detect SSH (`SSH_CONNECTION`/`SSH_TTY`) and tmux/screen
(`$TMUX`/`$STY`), with an explicit `COCO_*`-prefixed override. Feed it into
(a) the spinner self-arm interval (app.rs:419) and (b)
`FrameRateLimiter::MIN_FRAME_INTERVAL` (e.g. 30 FPS over SSH). Default stays
120 FPS local.

**Constraints.** Must respect the `COCO_*` env-var rule and route through
`RuntimeConfig`/`DisplaySettings` — **no ad-hoc `std::env::var` inside the
render loop** (project rule). Detection heuristics can misfire; keep tiers
conservative and pure-config so they hot-reload. Note coco-rs has **no
decorative animations to disable today**, so that half of jcode's policy is
moot. **Layer.** `coco-config` + `app/tui`. **Impact** low-medium ·
**Effort** medium · **Risk** medium (false SSH/tmux positives — mitigate with
override). Respects non-goals.

### R4 — Automatic RGB→xterm-256 downsampling for non-truecolor terminals (confirmed → low)
**Why.** jcode downsamples RGB→xterm-256 when `COLORTERM` lacks truecolor
(`color.rs:15-59` detect, `74-106` `rgb_to_xterm256` with nearest-cube/gray).
coco-rs emits `Color::Rgb` directly and only avoids RGB in opt-in `*_ansi`
themes (theme.rs:375-378, 426); no runtime downsample exists. A non-truecolor
terminal that hasn't manually selected an ANSI theme gets raw `Color::Rgb` and
relies on the emulator to clamp (often poorly).

**Change.** Add a color-capability detector (`COLORTERM` + `TERM`, mirroring
jcode color.rs:19-59) in `app/tui` or a utils crate; when truecolor is absent,
map `Color::Rgb → Color::Indexed` via a nearest-cube/gray function (jcode's
`rgb_to_xterm256` is a clean reference to reimplement). Apply it **once at the
`UiStyles` facade** (`presentation/styles`) so every renderer benefits without
per-widget edits. Default to emitting RGB when detection is ambiguous (current
behavior — capable terminals unaffected); provide a `COCO_*` override.

**Scope.** Helps only users on 256-color-only emulators who haven't already
picked an ANSI theme; for them it is a clear quality win. **Layer.** `app/tui`
(`presentation/styles`). **Impact** low · **Effort** low · **Risk** low.
Respects non-goals.

### R5 — Single-cell spinner fast path (nuanced → low, DEFER unless measured)
**Why.** jcode stamps only the spinner cell during processing
(`draw_status_spinner_only`, run_shell.rs:151-205) and never calls the full
frame builder. coco-rs's 50 ms spinner self-arm (app.rs:414-420) drives a full
`redraw()` → `render_interactive_viewport` (viewport.rs:43) that
unconditionally rebuilds turn-activity, live-tail, popup, and footer view
models. The per-tick view-model **CPU** is spent regardless.

**Verified value is genuinely low:** (1) coco's spinner is ~20 FPS not jcode's
higher cadence; (2) the history side is already a no-op on these ticks (native
mode feeds `committed_cells = &[]`, viewport.rs:350-355; append emission is
`Noop` when no new cells, controller.rs:131-141); (3) `buffer_updates`
(terminal.rs:485-510) already caps wire bytes via cell-diff; (4) the
interactive viewport is small. So this saves CPU on view-model assembly only,
not wire bytes, and matters only when the activity panel is heavy.

**Change (if pursued).** Mirror jcode: cache the status-indicator `Rect`
(viewport.rs:185-199 already has a known position) and, when a
"nothing-but-spinner-changed" guard holds (no new stream content, unchanged
activity rows, no toast/queue change), re-stamp only that row into the
`SurfaceTerminal` current buffer and run the existing `buffer_updates` diff,
skipping the full view-model build. **Layer.** `app/tui`. **Impact** low ·
**Effort** medium · **Risk** medium (guard correctness — a wrong "nothing
changed" drops a real update). **Recommendation: DEFER** until profiling shows
`render_interactive_viewport`'s rebuild is hot during streaming turns.
Respects non-goals.

### R6 — Virtualize the compatibility-fallback (Viewport-mode) live tail (from verifier finding; medium)
**Why (verified).** coco's native-scrollback model offloads scroll to the
terminal, but its **compatibility-fallback path** still renders *all*
committed cells with no slice/virtualization: when
`finalized_history_in_viewport()` is true (Viewport mode, e.g. Zellij —
compatibility.rs / modal.rs:74-76), `build_live_tail_lines` sets
`committed_cells = state.session.transcript.cells()` (viewport.rs:350-355) and
calls `build_lines_owned()` over the **whole transcript every frame**. On a
long session in a fallback terminal this is a per-frame perf cliff — exactly
what jcode's `materialize_line_slice` + `partition_point` index avoids
(ui_viewport.rs:258, 310-319).

**Change.** In Viewport/fallback mode, build a precomputed per-cell
line-count index (reusing the R1 cache) and materialize only the visible
slice via a binary search over cumulative counts (mirror ui_viewport.rs:319),
instead of wrapping every cell each frame. **Layer.** `app/tui`
(`surface/viewport.rs`). **Impact** medium (only in fallback terminals like
Zellij; native path unaffected) · **Effort** medium · **Risk** medium (must
preserve scroll-offset and streaming-tail composition). Respects non-goals.
Naturally stacks on R1's per-cell cache.

### R7 — Phase-align the spinner tick instead of flat re-arm (from verifier finding; low)
**Why (verified).** jcode resets its spinner interval to the animation clock
and skips missed ticks (`reset_status_spinner_interval` +
`status_spinner_delay_until_next_frame`, `MissedTickBehavior::Skip`,
run_shell.rs:14-54), so the glyph advances on a true 80 ms grid without drift
or burst. coco-rs re-arms a **flat** `schedule_frame_in(SPINNER_TICK_INTERVAL)`
(app.rs:414-420) with no phase alignment — after a heavy paint or a coalesced
event the next spinner frame can land late and the animation can visibly
hiccup.

**Change.** Track the spinner's intended phase and schedule the next tick to
the next grid boundary (skip missed ticks) rather than "now + 50 ms". Small,
local. **Layer.** `app/tui` (app.rs spinner self-arm). **Impact** low (visual
smoothness only) · **Effort** low · **Risk** low. Respects non-goals.

---

## Rejected after adversarial review

No M02 suggestion was outright **refuted** — all five analyst suggestions
verified on both the jcode and coco-rs sides. Two, however, were materially
**re-scoped** by the adversarial pass and must be read with that correction
(folded into the recommendations above), not at the analyst's original
framing:

- **R1 (per-cell line cache) is NOT a per-frame win.** The analyst framed it
  as "coco re-wraps the entire transcript from scratch [implying every
  frame]." Verified false for the production native path: the live tail feeds
  `committed_cells = &[]` (viewport.rs:350-355), so the full re-wrap happens
  only on the three debounced/triggered replay paths (controller.rs:134-141),
  not per frame. Kept at **medium** impact as a resize/toggle optimization,
  not the originally implied high-frequency one.

- **R3 (adaptive throttle) does NOT replace a fixed 120 FPS clock.** The
  analyst implied coco "keeps its 120-FPS-capped cadence regardless." Verified
  that 120 FPS is only a *cap* on an event-driven loop (clamp_deadline,
  frame_rate_limiter.rs:12; coalesced events, app.rs:298-303; idle sleep,
  app.rs:286), and the in-turn spinner is already ~20 FPS (app.rs:414-420).
  coco already avoids most of the waste jcode's tier solves, so the value is
  the narrower "cap in-turn cadence over SSH"; down-scoped to **low-medium**.
  Also note: jcode's tier disables decorative animation, but **coco-rs has no
  decorative animations**, so half of that policy is moot here.

- **R5 (single-cell spinner fast path)** verified on both sides but its
  payoff is genuinely **low** (native history side is already a no-op on
  spinner ticks; cell-diff already caps wire bytes). Recommendation is
  **DEFER unless profiling shows the view-model rebuild is hot** — kept in the
  list as a low-priority CPU micro-opt, not a rendering win.

Marketing claims checked and found **not to hold as stated** (covered in
"Where coco-rs already matches or wins"): jcode's "1000+ fps" (real default
cap is 60 fps, `jcode-config-types/src/lib.rs:572-573`; event-gated loop,
mod.rs:1038-1206) and "245× faster than Claude Code" (a process-startup PTY
benchmark vs the TypeScript product, not a render-throughput comparison vs
coco-rs).
