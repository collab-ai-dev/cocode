# TUI Streaming-Render Unification Plan

> Collapse the ~7 stacked flicker patches in the streaming render path into one
> coherent model + 2 targeted root-cause fixes. Grounded in: a 3-thread source
> survey (coco-rs / codex-rs reference / paint engine), a live 12-minute session
> log (`~/.coco/logs/coco.log.2026-06-06`, 6922 lines, 13 streamed turns,
> ~2726 frames, width 242), and a 5-agent measure+adversarial-challenge pass.
> Best-Rust-practice, clear architecture, **backward compatibility disregarded**.

> **Supersedes** [`tui-streaming-scrollback-commit-plan.md`](tui-streaming-scrollback-commit-plan.md)
> (2026-05-30, marked NOT VIABLE). That doc proposed the *per-line "codex model"*
> and was adversarially rejected; coco subsequently built a **provisional/finalized
> reconciliation** system instead. This plan fixes *that* system's bugs — it does
> NOT re-propose per-line commit (verified unsafe: `coco-tui-markdown` is not
> line-stable). Design surfaces it references (not redefines):
> [`ui/native-scrollback-architecture.md`](ui/native-scrollback-architecture.md),
> [`ui/terminal-surface-design.md`](ui/terminal-surface-design.md),
> [`ui/agent-console-design.md`](ui/agent-console-design.md).

## Context

## Implementation update — 2026-06-07

WS2a has landed with one deliberate correction to the original wording:
the heavyweight parity machinery is gone, but the finalized path still keeps
the cheap safety guards that matter for correctness after resize/theme/syntax
changes.

- `coco-tui-markdown` now owns `stable_prefix_end(source: &str) -> usize`.
  The TUI no longer carries its own Markdown boundary scanner.
- `StreamRenderController` renders the whole stable source prefix in finalized
  mode and appends only the newly grown rendered-line suffix to native
  scrollback. The mutable tail remains a separate `.streaming()` render.
- Provisional reconciliation now stores only `prefix_source`, rendered
  `line_count`, and `render_key`. Finalization replays if the finalized
  assistant text does not start with `prefix_source`, or if `render_key`
  differs; otherwise it renders finalized history and appends only the residual
  tail after `line_count`.
- SHA prefix digests and rendered-line fingerprint parity were removed from the
  streaming provisional path. `line_fingerprint.rs` remains because header
  fingerprinting still uses it.
- Tests moved the boundary scanner coverage into `coco-tui-markdown` and add a
  progressive invariant: every stable prefix render must be a prefix of the
  finalized full-source render.

The streaming renderer is **not one design — it is a stack of patches** all
chasing one symptom (input-bar jitter while the model streams): `anchor the
streaming viewport`, `reflow on width change only`, `probe synchronized-update`,
`keep viewport clear inside sync window`, a grow-only height watermark, a
turn-end repin replay, and the newest uncommitted `STREAMING_LIVE_TAIL_CAP=8`.

Root cause (one architectural mistake): **coco maintains a parallel
"provisional/streaming" render that diverges from the authoritative scrollback
commit, and reconciles the two via whole-transcript replays at the turn
boundary.** Fix the divergence; delete the reconciliation.

Three user-visible bugs fall out of it. All three are confirmed in the live log.

## The three bugs (measured)

| Bug | Symptom | Frequency | Sev | Measured cost |
|----|---------|-----------|-----|---------------|
| **A** — input-bar jitter + turn-end replay | viewport height re-evaluated per frame (bounced across 9 distinct heights); at turn end the bar snaps 12→4 and the freed band is re-seated by a **whole-transcript** `replay_all_capped` (`cause="viewport_relax_repin"`) | **every-frame** jitter; **6** replay events (~1/turn-end over 13 turns) | 5 | each replay 12→360 rows, 1.3ms→**26.4ms** as transcript grows |
| **B** — full-screen flash | streamed per-block markdown is concatenated **dropping the inter-block blank line**, so streamed lines never byte-match the finalized whole-text render → `consolidated_final_tail_lines` returns `ReplayRequired` (`cause="provisional_stream_parity_mismatch"`) → whole-transcript replay | **per-turn**, hit **4/13 turns (~31%)** | 4 | replays 47/70/180/360 rows = 4.2 / 9.4 / **61.6** / 34.7 ms |
| **C** — stray spaces + full-width padding | `display_width = UnicodeWidthStr::width(s).max(1)` diverges from ratatui's `symbol.width()`; wide-grapheme continuation cells (`Cell::EMPTY`, `symbol()==" "`, `skip==false`) leak mid-content. `insert_history_rows_direct` also pads **every** committed row to full width (no last-non-blank cutoff) | **every-frame** (byte padding); stray spaces data-dependent | 3 | ~**290 bytes/row** at width 242, content-independent (proves full-width pad); stray-space leak unobserved this session (needs emoji/VS16/CJK) |

**Worst case captured live (turn boundary 23:17:49):** two back-to-back
whole-transcript replays in a ~61ms window — `parity_mismatch` replay (BUG B,
34.7ms / 105 753 bytes / 360 rows) **then** `viewport_relax_repin` replay (BUG A,
26.4ms / 105 755 bytes / 360 rows) = **~61ms / ~211 KB rewritten for one turn-end.**
This is the visible full-screen flash. Note it is the **A+B interaction**, not two
A replays — see the decision below.

## Root cause: one divergence from the codex reference

| | codex-rs/tui (reference) | coco-rs (current) |
|---|---|---|
| commit boundary | per-newline, tail-budget 0 (holds only in-flight tables) | per markdown **block** (blank line / closed fence / ATX heading) — *correct for coco; see decisions* |
| stable render | re-renders **whole accumulated source** each tick; committed = prefix of that one render → finalize == progressive by construction | renders **each block independently** and concatenates → drops the inter-block blank → diverges from finalize → **parity reconciliation** (the bug surface) |
| turn-end height | live region near-empty (committed same-frame), pane shrinks over already-seated content → no re-seat | grow-only watermark holds tall height **one frame past** the commit → shrink frees rows the engine only *clears* → whole-transcript replay to re-seat |

## Decisions (incl. adversarial-review corrections)

1. **Keep block-granularity commit. Reject per-line.** Verified: `coco-tui-markdown`
   renders the whole source via pulldown-cmark and is **not line-stable** (setext
   headings, loose/tight lists, lazy continuation need block context). The prior
   doc's per-line "codex model" stays rejected.
2. **Fix B by rendering the whole committed stable *prefix* in ONE `render_markdown`
   call** (not per-tick whole-*source* like codex, not per-block like today). One
   `Writer` ⇒ the inter-block blank (`tui-markdown/lib.rs:311` `block_gap()` only
   emits it when `!lines.is_empty()`) is present and byte-identical to finalize.
   This makes line parity hold **by construction**, so the SHA-digest and rendered-line
   fingerprint checks are dead code. Keep the source-prefix and render-key replay
   guards so resize/theme/syntax changes do not leave stale native scrollback rows.
3. **Correction — there is no second named turn-end replay in this build.**
   `"stream_finish_pending_replay"` / `stream_finish=true` had **0 log hits**; all
   relax replays log `stream_finish=false`. The observed double-replay is the **A+B
   interaction**. Consequence: **WS2a alone removes the larger half** of the worst-case
   61ms flash (the 34.7ms parity replay); WS2b removes the 26.4ms relax half.
4. **Fix A by committing the live tail in the *same frame* as the shrink** (kill the
   watermark's 1-frame lag, codex's invariant), so the freed rows already hold
   finalized content; re-seat is the **bounded freed band (≤ `STREAMING_LIVE_TAIL_CAP`
   = 8 rows)**, never a whole-transcript replay. Keep the CAP as the deliberate
   fixed-pane content rule.
5. **Validate the engine re-seat before building new API.** The existing
   `insert_history_lines`/`move_viewport_down_for_history` already re-seats freed rows
   *above* the viewport; on a bottom-anchored shrink the freed rows are above the new
   (shorter) viewport — so WS2b may be a **sequencing fix, not a new scroll primitive**.
   Validate first; this can cut WS2b from high→medium blast radius.
6. **Fix C by de-duplicating the skip logic into one `visible_cells()` helper**
   (the triplication across `insert_history_rows_direct` / `buffer_updates` /
   `drawable_cell_indices` is the drift source), dropping `.max(1)` on the skip width
   (provably behavior-neutral vs ratatui-core-0.1.0 `buffer.rs:538`), and adding a
   codex-style `last_nonblank_column` + `ESC[K` cutoff.
7. **Keep block-commit's safety trade-off explicit:** deleting the rendered-line
   parity detector (decision 2) also deletes a **safety net** — a
   `stable_prefix_end` boundary mis-detection could become a silent wrong commit.
   Accepted, mitigated by renderer-prefix tests in `coco-tui-markdown` and by
   keeping source-prefix/render-key replay guards in `app/tui`.

## Workstreams

### WS1 — Engine width correctness (BUG C) · `tui-ui` only · +70 / −22 · risk LOW

**Files:** `coco-rs/tui-ui/src/engine/terminal.rs` (+ `terminal.test.rs`).
All three target loops + `display_width` are **module-private with zero external
callers** (ripgrep-confirmed; cross-crate `display_width`/`buffer_updates` hits are
unrelated namesakes / struct-field reads).
- Extract one `visible_cells(buffer, row)` skip iterator; route
  `insert_history_rows_direct` (~100-113), `buffer_updates` (~677-695),
  `drawable_cell_indices` (~729-743) through it.
- `display_width` (~745): drop `.max(1)` on the **skip** width (keep `.max(1)` only
  on the loop *advance*). Decide upfront whether to also drop it on the
  `affected_width` diff-spread line (~693) for full ratatui parity (cheap, safer).
- `insert_history_rows_direct`: add `last_nonblank_column` cutoff + `ESC[K`.
  **Hazard:** define "blank" as `symbol()==" " AND bg==Reset` so a styled/bg trailing
  run (selection / code-block fill) is NOT erased.
- **Tests:** trailing-blank emits `ESC[K` (not the space run); trailing **styled**
  space preserved; `visible_cells` parity vs the 3 old loops over wide-grapheme +
  combining-mark input. vt100 parser handles `ESC[K`; existing `contains`/`ends_with`
  byte assertions survive.
- **Independent** of WS2; no shared files.

### WS2a — One-render stable prefix + simplify reconciliation (BUG B) · `app/tui` · LANDED 2026-06-07

**Files changed:** `tui-markdown/src/lib.rs`, `streaming/render_controller.rs`,
`surface/history_driver.rs`, `surface/stream.rs`, plus focused tests.
- `render_live_frame` now asks `coco_tui_markdown::stable_prefix_end` for the
  source boundary, renders `source[0..stable_end]` in one finalized
  `render_markdown` call, and emits only the newly grown suffix by line count.
- `surface/stream.rs` removed the SHA digest state and emits provisional appends
  with cumulative `prefix_source`, cumulative rendered `line_count`, and
  `render_key`.
- `surface/history_driver.rs` removed rendered-line fingerprint parity from
  finalization. It still replays on source-prefix mismatch, render-key mismatch,
  or impossible line-count state; otherwise it appends the finalized residual
  tail after `line_count`.
- `line_fingerprint.rs` remains in use for `header_fingerprint`.
- Perf tracing now records stable-prefix render `source_bytes`, `lines`, and
  `elapsed_us`.
- Tests cover the blank-line multi-append regression, cumulative line-count
  finalization, source-prefix/render-key guards, and the markdown-layer
  progressive-prefix invariant.

### WS2b — Same-frame commit + bounded re-seat (BUG A) · `app/tui` + engine seam · +210 / −320 (~110 net deleted) · risk HIGH

**Files:** `app/tui/.../terminal.rs`, `surface/controller.rs`, `surface/history_driver.rs`,
`tui-ui/src/engine/terminal.rs` (+ 3 test files).
- Commit the live tail to scrollback **in the same frame** as the viewport shrink
  (remove the watermark's 1-frame lag); re-seat only the bounded freed band
  (≤ CAP rows) — **validate decision 5** (existing `move_viewport_down_for_history`
  may serve it) before adding a new `SurfaceTerminal` scroll-reseat method.
- **Delete** `streaming_height_high_water` + `apply_streaming_height_floor` +
  `streaming_height_floor`, `hold_bottom_edge_on_relax`, `needs_repin_on_relax`,
  `request_repin_replay`/`pending_repin`/the `"viewport_relax_repin"` branch, and
  `stream_finish_replay_needed` (sole caller removed).
- **Keep** `NATIVE_VIEWPORT_MIN/MAX_HEIGHT`, the CAP + drain, and the **prompt-scoped
  watermark** (`prompt_height_high_water` / `interactive_viewport_max_height` — the
  AskUserQuestion grow-only path is orthogonal; preserve prompt-close re-pinning
  without the deleted helpers).
- **Stays inside the single DEC-2026 window** owned by `Tui::draw_with_frame_index`;
  must not nest `?2026h`.
- **Tests:** delete ~8 watermark/hold/repin tests; rewrite the two no-gap-invariant
  tests so a 12→4 shrink re-seats ≤CAP committed rows with **no `replay_all_capped`**
  (outcome `Appended`/`Noop`, never `Replayed`, no blank gap); build committed history
  **independently of the live tail** (avoid the BUG-B self-fulfilling trap).
- **Shares `controller.rs`** with WS2a → sequence after WS2a (merge conflict in the
  provisional-append match + the repin/replay-cause branches).

**Current implementation note:** WS2b is now landed in code. The normal
turn-end path no longer carries `streaming_height_high_water`,
`request_repin_replay`/`pending_repin`, `"viewport_relax_repin"`, or
`stream_finish_replay_needed`; full replay remains only for display/reflow
changes and provisional finalization guards. Native history finalization uses
the named `native_history_projection` / `native_history_append_compatible`
policy so source-order transcript/chat rendering stays separate from the
append-compatibility text-before-leading-thinking path. The history/stream
boundary now passes a single `CommittedStablePrefix { source, line_count,
render_key }`.

**Follow-up implementation note:** the retained viewport now has a two-stage
main-screen geometry contract. Before the conversation fills the screen, it
flows directly after native history (`history_bottom_y`). Once natural growth
reaches the terminal bottom, the surface latches into bottom-pinned mode:
subsequent live tail, activity, prompts, and tool progress consume rows upward
from the terminal bottom. The patch-state height model
(`streaming_viewport_height_floor`, prompt high-water, grow-only floor, and
one-frame bottom-edge hold) is gone, and zero-residual stream finalization no
longer re-seats the viewport to history. History append/replay still owns the
scrollback content above the viewport.

## ROI

| Workstream | Fixes | Cost (net LoC / risk) | Value | **ROI** |
|---|---|---|---|---|
| **WS2a** | B (31% of turns; up to 61.6ms flash) + deletes ~280 LoC dead machinery; also removes the **larger half** of the worst-case 61ms turn-end flash | −280 / MEDIUM | very-high | **very-high** |
| **WS1** | C (every committed row's ~290 byte/row pad; stray emoji/CJK spaces) | +48 / LOW, isolated, **compounds** A+B flush cost | high but weakest standalone visibility (symptoms inferred, not observed this session) | **high** |
| **WS2b** | A (every-frame jitter — sev 5 — + the 26.4ms relax half of the flash) | −110 / HIGH, gated behind WS2a, touches enforced engine seam | highest per-frame value | **medium** (high value behind highest risk + strict dependency) |

**Land order: WS2a → WS1 (parallelizable, no shared files) → WS2b.**
Dependency direction is mandatory: without WS2a, WS2b's same-frame commit falls
back to the very `replay_all_capped` it deletes.

**Net effect if all three land:** the every-frame input-bar jitter is gone; the
per-turn full-screen flash (both halves) is gone; committed rows stop being padded
to full width (cutting ~290→~content bytes/row, which also shrinks any remaining
append flush time); stray wide-grapheme spaces in native scrollback are gone; and
~390 net lines of reconciliation/height-patch machinery are deleted.

## Problems NOT fixed (honest residual)

1. **The stable boundary scanner is still conservative, not parser-derived.**
   It now lives in `coco-tui-markdown` and is covered by renderer-prefix tests,
   but it remains a source scanner beside pulldown-cmark. Over-conservative
   boundaries keep text in the live tail; over-committing would still be a bug.
2. **Non-DEC-2026 terminals** still single-frame-flash on the shrink (reduced from a
   ~360-row full-screen flash to a ≤8-row band, not eliminated).
3. **CJK/wide-char in surfaces other than native scrollback** (Ctrl+O transcript reader
   overlay, `diff_display`, `truncate.rs`) — WS1 fixes only the three engine scrollback
   loops; other paths keep their own width logic.
4. **Two viewport-sizing strategies remain** — streaming uses same-frame commit+reseat;
   interactive prompts keep the grow-only watermark. Watch for a **prompt-close**
   regression (the deleted `request_repin_replay` partly served that path).
5. **The bounded re-seat itself can flicker** if commit-vs-shrink sequencing is off by
   one frame (a new, ≤8-row-bounded flicker class) — eliminated only by correct
   sequencing + the new TestBackend assertion.
6. **Turn-start grow (4→12) still shifts content** (by design; scroll-region-handled).
   "jitter every frame" is fixed; the initial grow is not claimed to be motionless.

## Hidden costs (don't under-budget)

- **Insta churn ≈ 0** — no `.snap` files under `app/tui/src/{surface,streaming}` or
  `tui-ui`; the 47 snapshots live in `presentation/`/`widgets/` and don't exercise the
  streaming controller. The "snapshots may shift" note is precautionary.
- **WS2b engine API may be over-billed** — route through existing
  `move_viewport_down_for_history` if validation (decision 5) confirms.
- **WS2a O(n²) re-render is latent** — the cache mitigation is a contingency, not built.
- **Test rewrites are skilled labor** — the regression tests must build committed
  history *independently of the live tail* to avoid re-creating the self-fulfilling trap.
- **`controller.rs` merge** — sequence WS2a then WS2b (shared file).

## Verification (end-to-end)

1. Per workstream: `just fmt` + `just quick-check` each iteration;
   `just test-crate coco-tui-ui` (WS1) / `just test-crate coco-tui` (WS2);
   `cargo insta pending-snapshots -p coco-tui` → accept any (expected near-zero) drift.
   Final gate: `just pre-commit` once per commit.
2. Manual (perf logging on: `tui.performance.enabled=true` + filter `tui=debug`):
   stream a long markdown reply (≥2 blank-separated blocks + a code fence + an
   emoji/CJK line). Confirm in `~/.coco/logs/coco.log.*`:
   - WS1: bytes/row tracks content (no fixed ~290 at width 242); no stray spaces.
   - WS2a: **zero** `cause="provisional_stream_parity_mismatch"`.
   - WS2b: **zero** `cause="viewport_relax_repin"`; input-bar `Rect` bottom constant
     across the turn; turn-end frame is `Appended`/`Noop`, never `Replayed`.
