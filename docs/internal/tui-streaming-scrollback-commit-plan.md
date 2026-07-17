# TUI Streaming Scrollback-Commit Plan (the "codex model")

> **SUPERSEDED (2026-06-07) by
> [`tui-streaming-render-unification-plan.md`](tui-streaming-render-unification-plan.md).**
> That plan keeps coco's block-granularity commit (per-line stays rejected, see
> the verdict below) and instead fixes the provisional/finalized reconciliation
> system coco built after this doc. Read it for the current, ROI-ranked fix set.

> Goal: while an assistant turn is streaming, flush **completed** lines into the
> terminal's native scrollback **incrementally** (once each), so the retained
> ratatui viewport only ever paints the small **in-flight tail** — instead of
> re-rendering the whole growing assistant buffer into the viewport every frame
> until turn end. Best-Rust-practice, clear architecture, backward-compat
> disregarded. Grounded in a 4-agent source survey of coco-rs, codex-rs/tui, and
> jcode (all evidence file:line in the survey).

## ⚠️ ADVERSARIAL-REVIEW VERDICT (2026-05-30): NOT VIABLE AS WRITTEN — DO NOT IMPLEMENT

A 4-lens adversarial review (premise-verification / logic / architecture /
cost-benefit), each re-reading the real source and refute-verified, found **21
confirmed issues incl. 2 P0** that break the core design. The plan's *foundation*
premises are correct (coco already does the codex model for finalized history),
but the **incremental streaming-commit mechanism is unsound for coco as written**.
Blocking issues, grouped:

1. **No anchor UUID for the in-flight stream** (P1 logic-2/prem-3). `StreamingState`
   has no UUID; the assistant UUID is minted fresh at push time (`engine.rs:1365`
   `Uuid::new_v4()`); `HistoryEmissionTracker` dedups by message UUID. There is **no
   shared key** to hang a line-cursor on or to reconcile at finalize. Prereq:
   engine-side pre-allocated assistant UUID threaded onto `StreamingState`
   (already flagged at `protocol.rs:893-897`) — engine work, not just TUI.
2. **Finalize is not one event** (P0 prem-2, P1 logic-5). `ui.streaming` is cleared
   from **8 sites**; the *common* turn (text → tool call) finalizes via
   `ToolUseQueued → flush_streaming_to_messages` (`stream.rs:52`, `projection.rs:16`)
   **before** `MessageAppended`, with no reconciliation → **double-emit every
   streamed line**. Stage 4 keys reconciliation on the wrong event.
3. **Markdown non-locality beyond tables breaks monotonic commit** (P0 logic-1).
   The Stage-0 prefix-stability gate is *empirically false* for coco's renderer:
   reference-link definitions rewrite an **earlier committed line's text** (unbounded
   distance), and loose↔tight list flips insert blank rows **between** committed
   lines. The table-only holdback covers neither, and scrollback commits are
   **irreversible** → wrong, unfixable output.
4. **Unit mismatch: unwrapped vs wrapped** (P1 logic-4). coco's renderer emits
   **unwrapped** logical lines; `insert_history_lines` clips (no wrap) while the live
   tail uses `Paragraph::wrap` → the same line renders **differently** in committed
   scrollback vs the in-flight tail.
5. **Mid-stream replay wipes streamed lines** (P1 prem-4, P2 logic-3). Display-toggle
   / reflow / resize → `clear_owned_scrollback` + re-emit *only finalized cells* →
   wipes in-flight streamed lines (not yet in `cells`); the existing deferral covers
   only resize and even that is incomplete (75ms debounce can fire mid-stream).
6. **Seam infeasible + wrong tier** (P2 arch-3, P1 arch-2, arch-4). Deltas arrive in
   `stream.rs::handle` (`&mut AppState`) which cannot reach a `StreamController` on
   `Tui.surface`; the controller must *derive* from `state.ui.streaming.content` at
   draw time, not be fed (else dual source of truth). And `table_holdback` is **not**
   zero-dep (pulls `table_detect.rs`, 488 LoC) — a second pipe-table grammar in
   tier-1 `app/tui` is wrong; holdback belongs in `coco-tui-markdown` driven off the
   pulldown-cmark `Tag::Table`/`Tag::CodeBlock` events the parser already emits.

**Cost-benefit (the deciding point):** in native mode the per-frame cost is
*already bounded* — `build_live_tail_lines` renders only the streaming cell and is
built-once-and-**moved** (not cloned), with `STREAM_MD_MEMO` covering the re-parse
on unchanged frames (arch-6). So this refactor chases a **UX feature** (completed
lines in native scrollback *during* the stream), not a perf necessity — at the cost
of: engine UUID pre-allocation + a unified `finalize_stream_commit` hook + a
wrapped-line commit substrate + replay protection + a CommonMark-safe (not
table-only) stable-prefix analysis. That is a large, cross-crate, correctness-
critical effort disproportionate to a cosmetic gain.

### Revised recommendation
- **Do NOT pursue the streaming incremental-commit refactor.** It is unsound as
  written and the corrected version is a major cross-crate undertaking for a UX
  nicety; the per-frame cost it targets is already bounded.
- **One standalone fix is independently worth doing** (logic-4): make
  `render_history_lines` (`tui-ui/src/engine/history_insert.rs`) **wrap logical lines
  to viewport width** (port codex's PreWrap). This fixes a *pre-existing latent clip
  in the finalized-history path* and is unrelated to streaming — small, safe, real.
- The F19 `Rc<[Line]>` memo is **not worth it either** — arch-6 confirms the native
  live tail is already built-once-and-moved, so the clone it removes mostly only
  matters in the Zellij fallback.
- **If** "completed lines in scrollback during the stream" is wanted *as a feature*,
  write a NEW plan starting with the prerequisites (engine pre-allocated UUID +
  unified finalize hook + wrapped commit + replay protection), and replace the
  table-only holdback with **block-boundary-only** commit (flush a line only when it
  precedes a blank-delimited, fully-closed block that is not a list item, not
  pipe-prose, and has no unresolved `[ref]`).

Everything below is the **original (now-refuted) plan**, retained for the record.

---

## TL;DR

coco-rs **already implements the codex model for finalized history**: committed
cells are rendered once and pushed to native scrollback via
`SurfaceTerminal::insert_history_lines`, deduped exactly-once by
`HistoryEmissionTracker` (UUID-prefix), replayed only on reflow / display-toggle
/ resume. **The only gap is streaming**: the in-flight assistant message is held
whole in `ui.streaming.content` and re-rendered into the viewport every frame;
nothing reaches scrollback until `MessageAppended` at turn end.

This plan closes that gap by porting codex's incremental streaming-commit
(newline-gated collector + table holdback + stable/tail split) and feeding the
**existing** commit substrate line-by-line during the turn. It is **native-mode
only**; the Zellij compatibility-fallback path is explicitly unchanged.

**Honest scope note:** this does *not* eliminate the per-delta markdown re-parse
(markdown is non-local — codex re-renders the whole source on every delta too).
The wins are: (1) completed lines live in real terminal scrollback *during* the
stream (scroll-back works mid-turn), (2) the per-frame viewport paint/clone is
bounded to the unstable tail instead of the whole buffer, (3) the per-frame
whole-`Vec<Line>` clone (the F19 issue) disappears for streaming.

---

## What already exists (the substrate — reuse, do not rebuild)

| Mechanism | Location | Role |
|---|---|---|
| `SurfaceTerminal::insert_history_lines(lines)` | `tui-ui/src/engine/terminal.rs:335-403` | The one scroll-region commit primitive (BSU/ESU framed): pushes `Vec<Line>` into native scrollback above the viewport, returns rows inserted. |
| `HistoryEmissionTracker` (`plan`/`mark_emitted_through`) | `app/tui/src/surface/history_emitter.rs:59-139` | Exactly-once append plan keyed on engine-message UUID prefix: `Noop` / `Append{start}` / `ReplayRequired`. |
| `SurfaceHistoryDriver::emit_append_only` / `replay_all_capped` | `app/tui/src/surface/history_driver.rs:67-177` | Renders only the new cell suffix → `insert_history_lines`; full replay (clear + re-emit, binary-search capped at 9000 rows) on reflow/display/resume. |
| `finalized_history_in_viewport()` / `native_history_enabled()` | `app/tui/src/surface/modal.rs:69-82` | The native-vs-fallback branch + the "alt-screen modal defers emission" gate. |
| Active streaming cell | `app/tui/src/presentation/transcript.rs:226-254` | Synthetic `ActiveTranscriptCell::Streaming` appended after committed cells; UI-only, never in `MessageHistory` (invariant I-3). Clean seam to shrink. |

In native mode `build_live_tail_lines` already passes `committed_cells = &[]`
(`viewport.rs:405-410`), so the live tail today is *only* the streaming cell —
the architectural seam we need is already there.

## The two residual per-frame costs

- **GAP A (native, the target):** the active streaming cell re-renders the
  **whole** `ui.streaming.content` into the viewport every frame; completed
  lines never reach scrollback until `MessageAppended` clears `ui.streaming`
  (`protocol.rs:921-924`). The `STREAM_MD_MEMO` only short-circuits the re-parse
  on *unchanged* (think-pause) frames; on every token-delta the whole buffer is
  re-parsed and re-painted.
- **GAP B (Zellij fallback):** `finalized_history_in_viewport()` is true →
  `build_live_tail_lines` re-renders the **entire transcript** in the viewport
  every frame. Inherent — there is no scrollback to commit to. **Out of scope;
  left unchanged** (Rc/`STREAM_MD_MEMO` are the only levers there).

## How codex does it (the pattern to port)

1. **Newline-gated source collector** (`markdown_stream.rs`): `push_delta` appends
   to an append-only buffer; `commit_complete_source()` returns
   `buffer[committed..rfind('\n')+1]` and advances the cursor — **the trailing
   partial line is never committed** (it could still become a heading / fence /
   table delimiter).
2. **Structural holdback** (`streaming/table_holdback.rs`): a byte-offset state
   machine that, in addition to the partial line, holds back a *speculative
   table header* (`| A | B |` with no delimiter yet) and a *confirmed table*
   (header + `| --- |`) as mutable tail, because a new row reshapes every column
   width. Skips lines inside code fences.
3. **Full re-render + index-diff** (`streaming/controller.rs`): on each committed
   delta it re-renders **all** of `raw_source` to `Vec<Line>` (markdown is
   non-local), then splits at `target_stable_len = rendered.len() − tail_budget`:
   `[..stable]` is enqueued for commit, `[stable..]` is the mutable tail cell.
   `emitted ≤ enqueued ≤ rendered.len()` — **monotonic**: a line, once emitted to
   scrollback, can never change.
4. **Commit** through `insert_history_lines`; tail painted by ratatui each frame.
5. **Finalize**: re-render once from full source, emit the remainder, then
   replace the streamed run with one source-backed cell for resize reflow.
   *(coco needs no equivalent of this last step — its replay already re-renders
   finalized cells from `Arc<Message>` at the current width.)*

---

## Design decisions (locked)

- **Native-mode only.** Branch on `!finalized_history_in_viewport()`. Scrollback
  *writes* additionally gate on `native_history_enabled()` (skip while an
  alt-screen modal defers emission). Zellij fallback: **change nothing**.
- **Unit of commit = a newline-terminated source line, minus structural
  holdback.** Never commit the trailing partial line or a speculative/confirmed
  table region. Port the holdback scanner faithfully — it is what makes the
  monotonic-commit invariant safe.
- **Re-parse stays whole-buffer + index-diff.** Do not attempt to render only the
  appended tail; markdown is non-local. The win is *not* parse elimination.
- **Reuse the existing commit substrate.** No new scrollback primitive. Extend
  the emitter with a *line-granular* cursor for the single in-flight assistant
  message UUID.
- **Finalize reconciliation is the #1 correctness rule.** At
  `MessageAppended(Assistant)` the authoritative finalized cell must append only
  the **residual** (held-back table + final partial line, rendered once), never
  re-emit lines already streamed. Preserve I-1: the streamed scrollback lines are
  a presentation optimization *reconciled against*, not duplicating, the
  authoritative cell.
- **Pacing: correctness first.** Start with immediate flush-all-stable-lines;
  layer codex's `chunking.rs`/`commit_tick.rs` animation later only if cadence
  regresses.
- **Resize mid-stream:** keep the existing `resize_requested_during_stream →
  stream_finish_replay` deferral; already-streamed lines stay at the old width
  until finalize replays the consolidated message (codex makes the same trade).

---

## Staged implementation

### Stage 0 — GATE: prove coco-tui-markdown prefix-stability *(do first, blocks all)*
The index-diff approach assumes rendering `source[..k]` then more source yields a
**superset whose earlier lines are unchanged** (outside the holdback region). If a
late blank line flips list looseness or a delimiter row retroactively rewrites an
earlier line *outside* what the holdback covers, emitted lines would need to
change — which is impossible once they are in scrollback.
- **Action:** add a `streamed == full-render` property test against
  `coco_tui_markdown::render_markdown(...).streaming()` over a corpus (loose/tight
  lists, tables, nested fences, alerts). Port codex's controller assertions.
- **Exit criterion:** either the renderer is prefix-stable, or the holdback set is
  expanded to cover every observed non-local rewrite. **If this fails, stop** —
  the whole approach rests on it.

### Stage 1 — `StreamController` (pure logic, fully unit-tested, NOT wired)
- New module `app/tui/src/streaming/` : `controller.rs` + `table_holdback.rs`
  (port codex's `table_holdback.rs` ~verbatim — zero coco deps) + companion
  `*.test.rs`.
- `StreamController`: `push_delta(&str)`, `commit_complete_source()` (newline
  gate), full re-render via `render_markdown(...).streaming()`, holdback-driven
  `stable_lines()` / `current_tail_lines()`, `finalize() -> remaining`.
- Unit tests: streamed-output == single full-buffer render, for the Stage-0
  corpus + table-row-by-row.
- **Zero behavior change** (not yet wired) — lands + verifies in isolation.

### Stage 2 — line-granular emission cursor
- Extend `HistoryEmissionTracker` (or add a sibling `StreamLineEmitter`) to track
  `emitted_stream_lines: usize` for the in-flight assistant UUID, plus a method
  to flush `stable[emitted..]` via `terminal.insert_history_lines` and advance.
- Gate writes on `native_history_enabled()`.
- Unit tests in `history_emitter.test.rs` / `history_driver.test.rs`.

### Stage 3 — wire into draw + handler (closes GAP A)
- Own a `StreamController` on `NativeSurfaceController` (frame-stable, **not** a
  thread_local). Feed deltas from the streaming handler (`&mut` seam — the
  renderer is `&AppState`, so the commit-offset advance happens handler/draw-side,
  not in the renderer).
- In `draw_at_inner` (native + not deferred): flush newly-stable lines via the
  Stage-2 emitter **before** the viewport draw.
- Shrink `build_live_tail_lines` (native branch) to render **only**
  `controller.current_tail_lines()` instead of the whole `ui.streaming.content`.
- Remove the character-reveal pacing (`display_cursor`/`advance_display`/
  `visible_content`) — superseded by commit pacing (confirm no other consumer).

### Stage 4 — finalize reconciliation (the correctness crux)
- At `MessageAppended(Assistant)` / `TurnEnded`: carry the streamed UUID's
  already-emitted line count into `HistoryEmissionTracker` so the normal
  `emit_append_only` for the finalized cell appends **only the residual** (held-
  back table re-rendered once + any final partial line). Assert **no
  double-emission** and **no I-1 violation**.
- Keep `replay_all_capped` + the binary-search cap for the legitimate replay
  triggers (reflow, display-toggle, resume) — unchanged.

### Stage 5 — cleanup
- Delete `STREAM_MD_MEMO` if the native tail now renders only the partial tail
  (keep it for the fallback path if still used there).
- Remove dead reveal-pacing code; update `app/tui/CLAUDE.md` Transcript section to
  document streaming-commit + the "streamed lines reconciled against, not
  duplicating, the authoritative cell" note (pre-empts an I-1 review flag).

### Stage 6 — OPTIONAL pacing polish
- Port `chunking.rs` (`AdaptiveChunkingPolicy`: batch when queue depth ≥ 8 or
  oldest line age ≥ 120ms) + `commit_tick.rs` for smooth line-by-line cadence,
  only if immediate-flush cadence looks janky.

---

## Risks (ranked)

1. **Markdown non-locality / prefix instability** — Stage-0 gate. Highest risk;
   if it fails the index-diff/monotonic-commit model is unsound. Mitigation:
   conservative holdback; the Stage-0 property test is the go/no-go.
2. **Finalize double-emission / I-1** — Stage 4. Mitigation: line-count
   reconciliation + explicit no-double-emit tests; document the presentation-vs-
   authority distinction.
3. **Monotonic commit** — once a line is in scrollback it cannot be retracted;
   the holdback must be conservative enough that emitted lines never change.
4. **Resize mid-stream width mismatch** — accept codex's trade (old width until
   finalize replay).
5. **Cadence regression** — immediate flush may look choppy; Stage 6 mitigates.

## Explicitly NOT doing
- The per-delta whole-buffer **re-parse is not eliminated** (inherent to non-local
  markdown; codex doesn't either).
- The **Zellij fallback** path is unchanged (no scrollback ⇒ codex model can't
  apply; Rc/`STREAM_MD_MEMO` remain its only levers).
- codex's `ConsolidateAgentMessage` step — unnecessary; coco's replay already
  re-renders finalized cells from `Arc<Message>` source at the current width.

## Verification
- Stage 0/1: unit/property tests (`streamed == full render`) in
  `controller.test.rs` / `table_holdback.test.rs`.
- Stage 2/4: extend `history_emitter.test.rs` / `history_driver.test.rs`
  (append-vs-replay + line-granular stream commit + no-double-emit on finalize).
- insta snapshots for the streaming-commit boundary (CLAUDE.md UI-change rule).
- `cargo nextest run -p coco-tui` (process-isolated), `just quick-check`, then
  `just pre-commit` once as the final gate.

## Minimum-viable vs full
- **MVP (Stages 0–4):** correctness-complete codex model with immediate flush.
- **Full (add 5–6):** dead-code removal + smooth animated pacing.
- **Gate:** Stage 0 must pass before any wiring. If the team wants the cheap 80%
  without the streaming machinery, the fallback is the F19 `Rc<[Line]>` memo
  (removes the per-frame clone only) — but that is *not* the codex model and is
  documented as the lesser alternative.
