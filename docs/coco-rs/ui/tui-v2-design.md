# TUI v2 — Three-Way Comparison and Clean-Slate Architecture

Status: implementation record (2026-06-11). Stages 0-4 are implemented on
the current `feat/tui` line; Scope C remains a deferred optimization.

This document does two things:

1. A fair three-way architecture comparison of the current `coco-rs` TUI
   (`app/tui` + `tui-ui` + `tui-markdown`), `codex-rs/tui` (reference tree in
   this repo), and the jcode TUI (`/lyz/codespace/3rd/jcode`, 15 `jcode-tui-*`
   crates).
2. A `tui-v2` architecture that **disregards backward compatibility and
   migration history**: it keeps the assets the iteration history validated,
   and deletes the machinery the iteration history condemned.

Companion documents: `single-render-path-refactor.md` (Policy A/B history),
`native-scrollback-architecture.md` (terminal mechanics),
`viewport-seating-regression-fixes.md` (geometry contract),
`codex-rs-tui-comparison.md` (earlier codex-only comparison),
`../engine-tui-unified-transcript-plan.md` (transcript authority).

## 1. Method and Scope

All three implementations were surveyed at source level (module maps, LoC,
render pipelines, streaming models, test suites). Numbers below are measured,
not estimated, but scope differs per project — codex ships onboarding/auth/
realtime-audio/multi-thread routing; jcode ships remote/auth/session tooling;
coco ships themes/i18n/plugin-skill-agent dialogs. LoC comparisons are
indicative, not a quality metric.

| Metric | coco-rs TUI | codex-rs/tui | jcode TUI |
|---|---|---|---|
| Production LoC | ~53k (44k `app/tui` + 8k `tui-ui` + 1.1k `tui-markdown`) | ~168k | ~162.7k total across 15 crates (incl. tests; core `ui*.rs` ≈ 44k) |
| Test LoC | ~26k (90 companion files, 50+ insta snapshots, 3 benches) | ~37.5k (vt100 backend, ~100 snapshots) | thin relative to size (~2.4k deterministic app tests) |
| Largest production file | 1,735 (`state/surface_payloads.rs`) | 11,188 (`bottom_pane/chat_composer.rs`) | 3,240 (`inline_interactive.rs`) |
| Crates | 4 (`tui`, `tui-ui`, `tui-markdown`, `tui-mermaid`) | 1 | 15 |

## 2. The Three Architectures at a Glance

| Dimension | coco-rs | codex-rs/tui | jcode |
|---|---|---|---|
| Terminal surface | Native scrollback + retained bottom viewport (`SurfaceTerminal`, BSU/ESU, cell diff) | Native scrollback + inline viewport (the original design coco ported) | Alt-screen + app-owned scroll buffer; **no native scrollback** |
| Transcript authority | Engine `MessageHistory` is canonical; TUI is a pure derived view (I-1/I-2/I-3) | UI owns history (`Vec<Box<dyn HistoryCell>>` built from protocol events inside `ChatWidget`) | UI is the authority (`display_messages` + lazy-loaded compacted history) |
| Streaming | Source-backed stable-prefix watermark **+ fingerprint reconciliation + replay fallback** (Policy B) | Source-backed stable/tail + newline gating + animation queue + table holdback; **no reconciliation layer needed** | Full re-render from source per delta, gated at semantic markdown checkpoints, heavy caching; **no commit problem at all** |
| State model | TEA: `AppState` + pure reducers + typed effects | Mega-controller: `ChatWidget`/`App` own protocol+UI state; `AppEvent` bus (200+ variants) | Single wide `App` struct (hundreds of fields), in-place mutation, actor-ish select loop |
| Presentation layering | Enforced pure crate seam (`tui-ui`: "view-models in, ratatui out", CI-checked script) | Single crate; `Renderable` trait but domain types reach rendering | 15 crates split by feature, some near-pure (`-style`, `-render`) but no enforced seam |
| Theming / i18n | 6–9 themes + custom JSON hot reload + daltonized palettes + truecolor→256 downsample; `rust-i18n` (en, zh-CN) | None (adaptive light/dark detection only); English-only | Fixed RGB palette; English-only |
| Mouse / input | No mouse capture (native selection preserved); kitty keyboard; vim mode; 3-layer keybinding bridge | No mouse capture; kitty keyboard; large keymap + keymap editor | Full mouse (drag-select copy mode, wheel, click-to-cursor); kitty keyboard |
| Media | Image attach (paste→base64), mermaid via `tui-mermaid` | Image attach | Inline image rendering (sixel/kitty), custom mermaid renderer, pinned side panes |
| Frame scheduling | `FrameRequester` coalescing at 120 FPS, idle frames free | Same pattern (it is the origin of the pattern) | Event-driven redraw + ~1 Hz animation tick; full-frame clear per draw (macOS workaround) |
| Perf engineering | O(delta) fingerprints, borrowed projections, replay/tail caches, syntect prewarm, staged perf instrumentation | Animation queue cadence, frame coalescing; resize is O(document) | Best startup/memory footprint of the three (14 ms first frame, ~28 MB); content-hash render caches, per-block syntect LRU, jemalloc |

## 3. Dimension Detail — What Each Got Right and Wrong

### 3.1 Terminal surface

- **codex-rs** invented the model coco uses: cursor-probed inline viewport,
  ESC-sequence history insertion, URL-aware wrapping, Zellij raw mode. It is
  the proven product shape — finalized history scrolls with the wheel,
  native copy works, the transcript survives exit.
- **coco-rs** ported the mechanics behind a domain-free crate with VT100
  byte-level tests and a panic-safe restore path. Same capability, better
  isolation. Cost already paid: the inline-viewport experiments, the seating
  regressions, the replay-duplication limitation (terminal scrollback above
  the screen is immutable — true for any implementation, including codex).
- **jcode** opted out. Alt-screen plus an app-owned scroll buffer with lazy
  history loading gives infinite in-app scrollback and avoids the entire
  insert/replay/reflow problem class — but finalized output is trapped in the
  app: no native wheel scroll into shell history, no transcript left after
  exit, broken expectations under tmux. For an agent console this is the
  weaker product posture; it is, however, the reason jcode's renderer can be
  radically simpler.

**Verdict:** native scrollback is the right product target and coco has it
working. v2 keeps it. jcode's simplicity is not transferable without giving
up the product property; codex's mechanics are already absorbed.

### 3.2 Streaming — the decisive comparison

All three converge on the same insight at the markdown layer: only commit
content past a conservative *semantic* boundary (coco `stable_prefix_end`,
codex newline-gating + table holdback, jcode checkpoint state machine).
jcode even tried checkpoint splicing and **abandoned it as unsafe** (markdown
list/separator continuity) — independent confirmation that conservative
boundaries are correct.

The difference is what happens above the markdown layer:

- **jcode**: no commit boundary exists at all. The viewport is app-owned, so
  every frame may re-render everything from source; caching (content-hash +
  width keys, per-code-block syntect LRU) makes it affordable. Simplest
  correct model — available only because of §3.1's trade.
- **codex-rs**: one projection. `StreamCore` renders the accumulated source;
  the stable region feeds an animation queue into scrollback; the tail stays
  live. At finalize, the *same source* is consolidated into the history cell.
  There is no second renderer to agree with, hence **no reconciliation
  machinery**.
- **coco-rs**: two composition layers. The stream path renders accumulated
  source through the committed markdown renderer; the finalize path projects
  `Message → cells → ordering rules → history lines`. Their agreement is
  enforced *at runtime* by per-line `u64` fingerprints plus replay-on-mismatch.
  The entire churn ledger of `single-render-path-refactor.md` — `#160`
  provisional ledger, `skip(count)` repair, Policy A deletion, Policy B
  fingerprints, the §6.1 leading-thinking presentation/verify alignment —
  is a sequence of patches on this one seam. Each new cell shape (thinking
  reorder today; streamed tool summaries or citations tomorrow) re-opens the
  proof obligation.

**Verdict:** coco's watermark, O(delta) fingerprinting, and conservative
boundary are all good engineering — but they are engineering applied to a
problem codex's shape does not have. v2's core change (§6.2) removes the
second composition layer instead of verifying it.

### 3.3 Transcript authority

This is coco's strongest architectural asset and neither comparator has it:

- coco: engine `MessageHistory` is the single source of truth; the TUI cell
  view is a pure derivation (`message_to_cells`), reconciled through four
  typed lifecycle events. Resume, rewind, compaction, SDK NDJSON consumers,
  and the future Hub all read the same stream and cannot desync. The
  side-cache pattern (reasoning metadata keyed by UUID, pruned with its
  anchor) keeps derivation pure.
- codex: `ChatWidget` builds and owns history cells from protocol events.
  Works for one front-end; a second consumer would re-derive its own truth.
- jcode: the UI's message list *is* the record.

**Verdict:** keep wholesale in v2. This is also exactly why coco cannot copy
codex's streaming literally — v2 must unify the projections *under* engine
authority (§6.2), not abandon authority to get one projection.

### 3.4 State management and module discipline

- coco: TEA reducers, typed `UserCommand`/`TuiEvent`, exhaustive
  `ServerNotification` handling, largest file 1.7k LoC, every module with a
  companion test file. Most testable of the three.
- codex: pragmatic but monolithic — an 11k-LoC composer, a `ChatWidget` that
  mixes protocol, product, config, and rendering concerns. The earlier
  comparison doc already rejected this object graph; nothing new changes
  that judgment.
- jcode: a single very wide `App` struct mutated in place. Fast to write,
  hostile to invariant isolation — the kind of shape coco's C1-class bugs
  show the cost of, except without the frame-level harness coco built.

### 3.5 Geometry (viewport seating)

Only coco and codex have this problem (jcode's viewport is trivial).
codex hides it inside `custom_terminal.rs` + `tui.rs` with modest state.
coco split it across two crates (`app/tui::terminal.rs` decides, `tui-ui`
engine executes and *clamps* `history_bottom_y`), and the C1 regression came
precisely from a pure predicate consuming a clamped cross-crate proxy. The
fix produced explicit invariants (I-V1..I-V4) and a frame harness — good
outputs, but the split-brain remains structural. v2 moves the whole decision
into the engine (§6.3).

### 3.6 Features, testing, performance

- Feature surface: coco is the only one with themes + hot reload + color-blind
  palettes + i18n + multi-provider-neutral pickers. codex has the deepest
  interaction layer (bottom-pane view stack, approvals, elicitation forms,
  thread routing). jcode has the richest terminal-media layer (inline images,
  side panes, mouse) and the best startup/memory numbers.
- Testing: coco and codex are comparable (snapshots + vt100 + behavior pins);
  coco adds seam guards and benches; jcode is comparatively thin.
- Performance culture: jcode budgets outcomes (first frame, RSS); coco
  instruments stages and optimizes allocations; codex tunes cadence. v2
  should adopt jcode's *budget-as-gate* posture on top of coco's
  instrumentation (§6.6).

## 4. Root Cause — Why coco Churned Where Others Didn't

The iteration ledger (all landed in this repo's history):

| # | Iteration | Outcome |
|---|---|---|
| 1 | Fullscreen alt-screen base (TS/Ink port) | Deleted |
| 2 | Stock ratatui `Viewport::Inline` + `insert_before` (Phase C) | Rolled back — duplicated turns |
| 3 | Cursor-pin + suspend/resume hardening (Phase A/B) | Kept as invariants |
| 4 | Native scrollback via `SurfaceTerminal` port | Landed — current base |
| 5 | `#160` provisional ledger + finalize `skip(count)` reconcile | Condemned and deleted (Policy A) |
| 6 | Policy A: viewport-only streaming | Baseline |
| 7 | Policy B: mid-stream stable-row commits + fingerprints (`#167`) | Current |
| 8 | Owned-top seating refactor → C1 HIGH regression → overflow-aware extent contract | Fixed; I-V1..I-V4 |
| 9 | Post-B hardening: §6.1 thinking alignment, O(delta) fingerprints, borrowed projections, syntect prewarm | Current |

Diagnosis, in order of weight:

1. **Two render authorities over the hardest surface.** coco chose the
   hardest terminal model (native scrollback — rows become immutable once
   emitted) *and* the strictest state model (engine authority + derived
   cells). The combination created an obligation neither comparator has:
   prove the stream projection and the cell projection emit identical leading
   rows. Items 5, 6, 7, 9 are all this one seam.
2. **Cross-crate geometry with clamped proxies.** Item 8. The engine clamps
   `history_bottom_y`; the app-side pure predicate consumed it as if it were
   history extent.
3. **Incremental migration.** Items 1–4 were a moving target where each step
   had to coexist with the previous surface. The docs now explicitly
   disclaim compatibility, but the cost was already paid.

Causes 1 and 2 are structural and fixable by design; cause 3 is historical
and disappears by definition in a clean-slate v2.

## 5. Assets v2 Keeps (validated by the same history)

1. **Native scrollback surface** — `SurfaceTerminal`, BSU/ESU framing,
   cell-diff, VT100 test backend, panic-safe restore, suspend/resume, cursor
   claim arbitration. Re-litigating this would repeat items 1–4.
2. **Engine transcript authority + pure derivation** (I-1/I-2/I-3, lifecycle
   events, side-cache pattern, UUID-stable emission tracking).
3. **The enforced `tui-ui` seam** — unique among the three; the reason the
   paint engine, themes, widgets are reusable and independently testable.
4. **`stable_prefix_end`** conservative markdown boundary (independently
   validated by jcode's splice failure and codex's holdback).
5. **TEA + typed effects + module-size discipline.**
6. **Theming/i18n/keybinding bridge** product surface.
7. **Test pyramid**: companion tests, insta snapshots, VT100 byte tests,
   frame-level geometry harness, benches, seam guard scripts.
8. **Frame coalescing** (`FrameRequester`) and the perf instrumentation
   stages.
9. **Accepted limitation as documented physics**: replay cannot clear rows
   already scrolled out; replay stays a last resort for render-key changes.

## 6. tui-v2 Architecture

### 6.1 Goals / non-goals

Goals: remove the two structural churn factories (§4 causes 1–2) without
giving up engine authority or native scrollback; absorb codex's interaction
depth and jcode's perf-budget culture; keep everything in §5.

Non-goals: alt-screen base surface or app-owned scrollback (jcode model);
UI-owned history (codex model); mouse capture; right-side rails; any
backward-compatibility toggle; provisional/dual render paths in any form.

### 6.2 Core decision 1 — drop the fingerprint reconciliation; anchor on source, fall back to replay

> **Revision note (source-verified 2026-06-10).** An earlier draft of this
> section claimed finalize could be "a flush of the same rendered vector —
> prefix equality trivially true because it is the same memory," guarded by
> "one O(n) whole-message `source_text(m) == raw_source` compare." **Recon
> against the live code refuted both claims on the default path.** §6.2.1
> records the mechanism; this section states the corrected, safe design.

Scrollback immutability is terminal physics — no design removes it. What v2
removes is one specific piece of machinery: the **per-rendered-line
fingerprint re-verification** that coco runs at finalize to re-prove that
the rows already emitted mid-stream match the final render. That check is
redundant with the markdown prefix-stability property: rendering a markdown
*source prefix* through the committed renderer yields a *row-prefix* of
rendering the full source, past `stable_prefix_end` boundaries. If that
property holds, then a **source-level** anchor check is sufficient and the
fingerprints are belt-and-suspenders. (Implementation note, post-review: the
pre-existing advance test only pinned prefix-vs-larger-prefix at stable
boundaries; the relation the finalize actually uses — stable-prefix render vs
the committed render of the **full** text, unstable tail included — is pinned
by `transcript/stream.test.rs::test_stable_lines_are_row_prefix_of_full_committed_render`,
added with Stage 1 and covering fence / loose-list / setext / blockquote /
table / late-reference-link / partial-line traps at two widths.)

The finalize keeps the existing per-segment cell anchoring
(`history_driver.rs:537-594`) and swaps the verification:

```text
stream deltas ──accumulate──▶ raw_source (per streamed run)
                                   │ render_committed_assistant_markdown(…, RenderKey)  (the ONLY renderer)
                                   ▼
                           rendered_lines (cached)
         rows[emitted..stable] ──▶ native scrollback   (mid-stream watermark advance)
         rows[emitted..]       ──▶ live viewport tail

finalize = MessageAppended(canonical m):
    locate the anchor AssistantText cell (skip same-message leading thinking)
    anchor.text.starts_with(emitted_source_prefix)  AND  RenderKey matches ?
      ├─ yes → flush anchor's remaining rows, then render m's other cells
      │        (leading thinking, tool calls, following text) in canonical order
      └─ no  → warn! + reset watermark + replay from m   (same trigger surface as today)

transcript cells: derived from m as today (I-1 / I-2 unchanged)
```

What is **deleted**: `RenderedLineFingerprint` in the *stream* path,
`EmittedStreamPrefix.line_fingerprints`, `PendingStreamPrefix.line_fingerprints`,
the `fingerprint_lines(...) == pending.line_fingerprints` re-verification
(`history_driver.rs:630-650`), and the per-line fingerprint accumulation in
the stream driver (`surface/stream.rs:138-149`). What is **kept**: the
`source_prefix` / `starts_with` anchor check (`:582`), the render-key gate
(`:601`), the leading-thinking reorder (the *presentation* half of §6.1),
the watermark (integer pair + `stable_prefix_end`), replay as the mismatch
fallback, **and the finalize markdown render of the full canonical text
(`:610-618`)** — that render is not verification overhead, it *produces the
suffix rows* (`:652`); by finalize time the stream projection may already be
cleared (the overlay drops at `MessageAppended`), so the suffix must
re-render from canonical source. "Flush" in this design therefore means
"append the suffix of the anchored canonical render", not "reuse the
in-memory streamed vector". The `RenderedLineFingerprint` **type** and
`fingerprint_lines` survive for the independent session-header dedup path
(`history_driver.rs:45,143,439`) — only the stream-path usage is removed.

So v2's headline is narrower and honest: **not "zero reconciliation," but
"no rasterized reconciliation — the cheaper source-anchor check already in
the code, with the fingerprint belt-and-suspenders removed and its soundness
resting on the already-tested markdown prefix-stability property."** It still
collapses the three-module driver triangle into one `transcript/` owner and
deletes the O(delta) fingerprint accumulation; it does **not** claim to
eliminate replay (see §6.2.1).

#### 6.2.1 Why the whole-message flush fails: within-message multi-text turns

> **Second correction (cross-validated 2026-06-10).** The first recon pass
> claimed streaming-tool-exec mode never emits `ToolUseQueued` and that the
> accumulator therefore concatenates across tool boundaries in that mode.
> **Both halves were wrong** — the emission is indirect
> (`engine_stream_consume.rs:395` → `tool_call_preparer.rs:132` →
> `tool_runner.rs:53` emits `ToolUseQueued` mid-stream at each
> `ToolCallEnd`), and the mode asymmetry is the *inverse* of what recon
> reported. The corrected mechanics below are all first-hand source reads.

The divergent shape is `text → tool_use → text` **within one assistant
message** (one stream, one `Finish`). It is a *minority* shape, not the
dominant one: the dominant agent loop (text → tool ends the message → tool
results → next round streams a **new** message) resets cleanly, because the
`MessageAppended` handler clears the streaming accumulator between rounds
(`protocol.rs:922-934`) and each round maps 1:1 to one `AssistantText` cell.

For the within-message shape, the canonical side is one `Message::Assistant`
with snapshot `[Text("before"), ToolCall, Text("after")]`, fanned by
`derive.rs:105-127` into **two separate** `AssistantText` cells (pinned:
`services/inference/src/stream.test.rs:412-478`). The TUI stream side
diverges from that in **both** tool-exec modes, by two different mechanisms
(`enable_streaming_tools` defaults `true`, `common/config/src/sections.rs:399`):

- **Non-streaming (batch-at-end) mode:** tool prep runs only after `Finish`
  (the `streaming_handle` guard, `engine_stream_consume.rs:337`), so no
  `ToolUseQueued` arrives mid-stream; the single `StreamingState`
  accumulates `"beforeafter"` as one markdown run. The emitted stable prefix
  is a prefix of the *merged* doc; once it extends past `"before"`, the
  finalize anchor `"before".starts_with(prefix)` fails → **replay**.
- **Streaming mode:** `ToolUseQueued` *is* emitted mid-stream (the indirect
  chain above), the TUI flushes at the boundary
  (`server_notification_handler/stream.rs:52`), and `"after"` re-streams as
  a fresh segment. But the second segment's mid-stream commit
  **unconditionally overwrites the single `pending_stream_prefix` slot**
  (`history_driver.rs:308`); finalize then anchors the *first* text cell
  `"before"` against the `"after"` prefix → mismatch → **replay**.

Either way: **this shape replays today**, fingerprints or not. v2 Scope B
preserves that behavior bit-for-bit. And either way a whole-message
`source_text(m) == raw_source` compare is the wrong granularity — the unit
of equality is one streamed segment ↔ one `AssistantContent::Text` part.

**The rejected "fix."** Recon proposed adding `flush_streaming_to_messages`
to the `ToolUseStarted` arm. Rejected as a **no-op**, not as dangerous: in
streaming mode `ToolUseQueued` already arrives immediately before
`ToolUseStarted` and already flushes; in non-streaming mode `ToolUseQueued`
arrives only after `Finish`, when `MessageAppended` has already reset the
accumulator. (An earlier draft of this section called the fix a visible
regression — wrong: the overlay clear at the tool boundary is today's
existing streaming-mode behavior.)

**Two scopes, decided per §6.2 verdict:**

- **Scope B (Stage 1, recommended):** delete the fingerprint machinery only;
  keep the source-anchor check + replay fallback. Within-message multi-text
  turns keep replaying exactly as today. No engine change.
- **Scope C (deferred optimization):** *eliminate* the within-message
  replay. The verified fix locus is the single `pending_stream_prefix` slot
  (`history_driver.rs:308` overwrite): it needs per-segment prefixes, or
  suppression of post-boundary segment emission until finalize. Deferred —
  and the cross-validation *reduced* its value: it buys back replays only on
  a minority shape, not on every tool turn.

The engine source-contract pin (§8) stays as specified: it pins the
*per-`AssistantContent::Text`-part* equality at the inference layer (part
identity is erased at the `StreamEvent::TextDelta` level), and documents the
`emit_stream` error-swallow (`engine_stream_consume.rs:260`) as the one real
divergence. Scope, stated precisely (review correction): the underlying
`send(..).await` fails only once the TUI receiver is dropped — it cannot lose
a delta transiently while the TUI lives. Divergence **within** the committed
prefix is caught by the finalize source-anchor → replay; divergence **past**
the committed prefix would be unguarded (a late list sibling can retroactively
flip earlier rows via the loose/tight rule while `starts_with` still passes),
but reaching it requires a mid-turn delta loss this channel cannot produce.

### 6.3 Core decision 2 — geometry is one pure function, engine-owned

The C1 lesson, made structural: the seat/pin decision moves entirely into
the `tui-ui` engine.

```rust
// tui-ui::engine — pure, no clamped proxies can leak in because
// the unclamped extent never leaves the engine.
pub struct SeatInputs {
    pub screen: Size,
    pub desired_viewport_height: u16,
    pub overlay: OverlayPlacement,
    // engine-internal, overflow-aware:
    // finalized_history_extent_rows (NOT clamped to viewport top)
}
pub fn seat_viewport(&self, inputs: SeatInputs) -> SeatDecision;
```

The app supplies *intent* (desired height, overlay placement); the engine
owns extent, pin, reveal, and clamping in one place. I-V1..I-V4 from
`viewport-seating-regression-fixes.md` become the module's initial contract,
and the backend-generic frame harness exists from the first commit (the C1
class lived exactly in the untested cross-crate gap).

> **Amendment (2026-06-11) — codex-aligned shrink; reveal machinery
> deleted.** The C1 remedy (extent-forced bottom re-pin + tail-cache
> reveal) caused the permission-prompt duplication: after a prompt's
> destructive grow scrolled history into native scrollback, the freed rows
> on shrink could only be "backed" by repainting cached tail rows that
> were still visible just above the gap — duplicating them on screen
> (geometrically unavoidable: the newest cached rows are always the rows
> directly above the viewport whenever any history is visible). The seat
> now follows codex (`codex-rs/tui/src/tui.rs::draw`,
> `insert_history.rs`): shrink keeps the top anchored; appends walk the
> viewport back down (`move_viewport_down_for_history`). Deleted:
> `history_backs_row` / `finalized_history_extent_rows`,
> `fill_history_gap_rows`, `HistoryTailCache` + `fill_history_tail_gap` /
> `history_tail_reveal_rows` plumbing, and the reveal arm of the shrink
> arbitration. I-V1 strengthens to universal flush
> (`SurfaceTerminal::seats_flush`, no pinned-gap exemption).
>
> **Second iteration (same day):** a fully codex-style float bounced the
> bottom-aligned composer — log 47647 showed unbacked 1-row shrinks
> (status-line/tail churn, height 5↔4) lifting `viewport_bottom` 54→53
> for runs of ~26 frames. Codex tolerates the float because its composer
> sits at the pane TOP; coco's is bottom-aligned. So a shrink while
> seated on the screen bottom commits only its append-backed rows
> (`SeatInputs.guaranteed_append_rows`) and DEFERS the rest
> (`SeatDecision.deferred_shrink_rows`): the bottom edge stays glued, the
> surplus height renders as the viewport's top filler
> (`Constraint::Min(0)`), and later appends collapse it. Deferral never
> repaints history, so the duplication class stays structurally
> impossible. The Stage-2 record below predates this amendment.

### 6.4 Crate and module layout

Crates unchanged (`tui-ui`, `tui-markdown`, `tui-mermaid` survive as-is;
the seam scripts continue to guard them). Inside `app/tui`:

```text
app/tui/src/
  transcript/        # NEW single owner of the §6.2 pipeline
    cells.rs         #   project_cells (canonical Message → cells, ordering rules)
    render.rs        #   render_cell — the only renderer
    stream.rs        #   raw_source + rendered_lines + watermark + RenderKey invalidation
    emission.rs      #   exactly-once tracking, finalize flush + source check, replay decision
  surface/           # thin: frame plan, overlay placement, replay execution
  bottom_pane/       # NEW codex-style composer + local view stack
  presentation/      # pickers / pagers / prompts view models (as today)
  state/  update/  server_notification_handler/   # as today
```

Re-homed / collapsed (not all deleted — see §6.2 revision): the render core
of `streaming/render_controller.rs` and the watermark + live-tail logic of
`surface/stream.rs` move into `transcript/stream.rs`; the finalize logic of
`surface/history_driver.rs` moves into `transcript/emission.rs` minus the
deleted fingerprint-verify half; `surface/line_fingerprint.rs` stays (its
session-header consumers survive — only the stream-path usage is removed).
Also: the legacy keybinding cascade layer (folded into `coco-keybindings`
defaults) and deprecated `model.rs` are deleted as Stage-4 cleanup.

`bottom_pane/` absorbs the interaction-pane prompt sprawl
(`update/interaction.rs`, 1.7k LoC) into the codex-validated shape: a
retained composer plus a stack of local surfaces with key routing
`focused local surface → keybinding context → composer → global`, under
coco's existing overlay priority/attention-safety rules.

### 6.5 Streaming v2 behavior spec — and the watermark verdict

**Mid-stream watermark emission: evaluated, kept.** The question was whether
v2 still needs codex's watermark output (stable rows entering scrollback
mid-stream) once finalize becomes a flush, or whether finalize-only emission
(Policy A posture) would be simpler still.

- Product: a long in-flight answer exceeds the bounded viewport (live tail
  cap ≈ 8 rows). With finalize-only emission the user can read nothing but
  the tail until the turn ends — strictly worse mid-stream than both codex
  (streams stable rows into scrollback) and jcode (app-owned buffer is fully
  scrollable in-flight). The original Policy B motivation stands.
- Cost: under flush-finalize the watermark machinery shrinks to one integer
  pair + `stable_prefix_end` + render-key invalidation. The expensive parts
  of today's Policy B (fingerprints, finalize verify, §6.1 verify alignment)
  were costs of the dual projection, not of the watermark itself.
- Geometry is unaffected by the choice: finalize-only emission would still
  insert rows while bottom-pinned, so dropping mid-stream emission buys no
  viewport-seating simplification.
- No mode toggle: one emission policy, always on (a "commit only when the
  stable region exceeds the viewport" hybrid adds a second behavior boundary
  for no structural gain).

Rules carried over unchanged:

- codex's animation-queue pacing (drain a few rows per tick for a typing
  cadence) remains an optional cosmetic on top of the same watermark.
- Tool calls remain transcript-commit-bounded (unresolved `ToolUse` blocks
  the prefix; orphan results don't; duplicate call ids need one result each
  — `committable_prefix_len` + `find_forward_unconsumed_tool_result`,
  `history_driver.rs:673-716`, kept verbatim with their tests).
- **Multi-toolcall rendering is part of the kept presentation contract**:
  the shared projection (`presentation/transcript.rs:201-244`, used by both
  the transcript reader and `native_history_presentation` →
  `history_lines.rs:285`) pairs each `ToolUse` with its forward result into
  one `ToolCall` presentation cell, emits a `ToolBatch { start, end, count }`
  header for 2+ adjacent tool uses — rendered as
  "‖ N in parallel · Grep, Read ×3" with names sorted and repeats collapsed
  (`tool_batch_name_summary`, landed pre-Stage-1 so the user can see which
  tools are running before any result pairs) — and renders orphan results
  standalone. It re-homes verbatim into `transcript/cells.rs` (U1).
  Note the anchored finalize is the *good* case for parallel batches: one
  text segment anchors, and the suffix-compose renders
  `[thinking][batch header][ToolUse+result pairs]` through the same
  projection once `committable_prefix_len` unblocks — the pending prefix
  must survive the whole batch-execution window, which is existing,
  test-pinned behavior (`completed_parallel_tool_batch.snap` is in the
  zero-churn acceptance set). The §6.2.1 problem shape (a *second text
  part* after tools) is orthogonal to batch size.
- Tables / open fences / partial lines / setext / reference links stay in the
  mutable tail (`stable_prefix_end`).
- Height-only changes never replay; render-key changes always reset the
  watermark and replay.

### 6.6 Adopted from the comparators

From codex-rs:
- bottom-pane local view stack (§6.4);
- paced row-commit cadence (§6.5, optional);
- the `display / transcript / raw` three-view contract on cells (raw =
  copy-friendly source-backed lines), completing what coco has partially.

From jcode:
- per-code-block syntax-highlight LRU keyed by `(content_hash, lang)` inside
  `tui-markdown` (today coco prewarms grammars but re-highlights per render);
- scroll-anchor reconciliation for the transcript pager when older history
  loads lazily (`HistoryScrollAnchor` pattern);
- perf **budgets as CI gates** on the existing criterion benches: first
  frame ms, stream-advance µs, replay ms, baseline RSS — jcode proves these
  numbers are achievable in this exact stack;
- their abandoned checkpoint-splice experiment is recorded evidence: do not
  loosen `stable_prefix_end` toward splicing.

Explicitly not adopted: jcode's alt-screen/own-buffer surface, full-frame
clear per draw, mouse capture, fixed theming; codex's mega-controller state
ownership, UI-owned history, hardcoded styling.

### 6.7 Invariants (v2 statement)

1. Engine `MessageHistory` is canonical; the TUI projection is pure (I-1/I-2/I-3).
2. One `project_cells`, one `render_cell`; no streaming-only renderer exists.
3. Finalize anchors on a per-`AssistantText`-cell source check
   (`text.starts_with(emitted_source_prefix)` + render-key match); on match it
   flushes the anchor's remaining rendered rows, on mismatch it replays.
   **Rasterized** rendered-row (fingerprint) reconciliation does not exist;
   its soundness rests on the already-tested markdown prefix-stability
   property, not on memory identity (§6.2, §6.2.1).
4. `project_cells` orders a message's leading thinking cells after its first
   text cell, suffix-independently, so replay order matches emission order.
5. Rows enter native scrollback exactly once; the watermark advances only on
   committed inserts.
6. Replay is reserved for render-key changes and source-contract violations;
   it is never a steady-state path.
7. The seat/pin decision is computed inside the engine from unclamped,
   overflow-aware extents (I-V2 generalized).
8. Widgets perform no terminal side effects; one cursor claim wins per frame.
9. `tui-ui` stays domain-free; seam scripts remain blocking checks.
10. **The "rows already in native scrollback" fact is single-owned.** One
    `ScrollbackStreamCommit` (owned by `SurfaceStreamDriver`) is read by BOTH
    the live-tail increment and the anchored finalize — there is no second copy
    on the history driver. It advances only on a committed insert and clears
    only when the rows leave scrollback (`invalidate_commit` on replay/reset)
    or the finalize consumes them (`consume_commit`); a transient `streaming ==
    None` frame never clears it. This makes §6.7-5 ("rows enter scrollback
    exactly once") hold by construction: a stale-high commit surviving a replay
    (leading-row LOSS) and a stale-low commit re-emitting present rows (leading-
    row DUPLICATION) are both unrepresentable. Supersedes the earlier split
    state where the watermark lived on `SurfaceStreamDriver` and a duplicate
    `PendingStreamPrefix` lived on `SurfaceHistoryDriver`, kept in sync by
    matching clear conditions (the reactive `(g)` watermark-identity patch was a
    symptom of that split).

## 7. Risks

| Risk | Mitigation |
|---|---|
| Dropping the row fingerprints could let a mis-rendered prefix reach scrollback | The fingerprints re-verified the markdown prefix-stability property: a source-prefix render is a row-prefix of the full render. The soundness anchor is `transcript/stream.test.rs::test_stable_lines_are_row_prefix_of_full_committed_render` (prefix vs **full**-document render, the exact relation the finalize uses), with `test_stable_lines_remain_prefix_stable_across_advances` as the secondary advance pin; the source `starts_with` + render-key gate stay; replay remains the fallback. A renderer prefix-stability violation in a covered construct fails CI; the pins are example-based, so an uncovered construct remains a (small) residual risk — extend the trap source when `stable_prefix_end` learns new constructs. |
| Within-message multi-text turns (`text→tool→text` in one assistant message) still replay | This is **parity with today**, not a regression (§6.2.1): the current fingerprinted code already replays this shape in both tool-exec modes (non-streaming: merged-accumulator anchor mismatch; streaming: `pending_stream_prefix` single-slot overwrite at `history_driver.rs:308`). It is a minority shape — the dominant cross-round loop resets cleanly via `MessageAppended`. Eliminating it is Scope C, explicitly deferred. The engine source-contract pin is per-`AssistantContent::Text`-part, documenting (not papering over) the `emit_stream` error-swallow as the one real divergence. |
| The text-first ordering rule constrains future cell designs | Only the *order* of a message's cells must be suffix-independent, not row content. It is the *presentation* half of §6.1 (kept); the *verify* half is deleted. Document the rule per new `TranscriptCellKind`. |
| The text-first ordering rule constrains future cell designs | Far weaker than the previous draft's prefix-monotonicity: only the *order* of a message's cells must be suffix-independent, not row content. Document it per new `TranscriptCellKind`. |
| Rewrite cost vs. incremental value | v2 is mostly *deletion plus consolidation*: the engine, markdown, themes, state, handlers, tests, and product surfaces carry over. The new code is `transcript/` (replacing three modules) and `bottom_pane/`. |
| Engine-owned seating could bloat `tui-ui` with app policy | Only geometry moves; overlay *policy* (which surface wants alt-screen) stays in the shell. The engine takes typed intent, returns a decision. |

## 8. Verification Plan

- Carry over the full §10 test list of `single-render-path-refactor.md`
  (stream append, tail exclusion, exactly-once finalize, holdback classes,
  tool pairing, pinned-geometry, height-only no-replay).
- Keep as the soundness anchors (do NOT delete):
  `test_stable_lines_are_row_prefix_of_full_committed_render` (primary —
  prefix vs full-document render, the relation the finalize uses),
  `test_stable_lines_remain_prefix_stable_across_advances` (secondary —
  append-only advances), and the render-key replay tests — they are what
  makes dropping the fingerprints safe.
- Add: an engine-layer source-contract pin — for each
  `AssistantContent::Text` part, `part.text` equals the byte concat of that
  part's `TextDelta` run (empty dropped symmetrically), covering single-run,
  `text→tool→text`, and interrupted turns; it documents the `emit_stream`
  error-swallow as the known asymmetry. A forced-mismatch test (anchor source
  diverges → replay, no partial flush). An order-consistency pin (replay cell
  order == emission order for thinking+text+tool shapes).
- Rewrite (mechanism change, behavior preserved): the finalize-verify tests
  in `history_driver.test.rs` assert flush-on-anchor-match / replay-on-mismatch
  without the fingerprint assertion. Delete only
  `stream_append_fingerprints_accumulate_incrementally_to_full_prefix`.
- **Acceptance gate:** the ~17 native-surface `.snap` files stay byte-identical
  (Scope B is parity); any churn means observable composition changed and must
  be corrected, not accepted.
- Geometry: the I-V1..I-V4 frame-harness suite runs against the engine-owned
  `seat_viewport` from day one; C1/A4 regression cases ported verbatim.
- VT100 byte tests and the manual terminal matrix (Terminal.app, iTerm2,
  tmux, Zellij, Linux, SSH) unchanged.
- Criterion benches gain budget assertions (first frame, stream advance,
  replay, RSS) per §6.6.

## 9. Final Judgment (the fair version)

- **codex-rs/tui** is the origin of the correct terminal mechanics and the
  deepest interaction layer, delivered pragmatically; its costs are monolith
  files, domain/presentation coupling, and zero theming/i18n. Its single
  greatest transferable idea is *one render projection* — which v2 takes.
- **jcode** is the performance and terminal-media benchmark, and proof that
  simplicity wins when you can afford its premise (app-owned screen). Its
  premise is wrong for coco's product target, but its caching discipline,
  budget culture, and negative result on splicing transfer directly. coco
  has already absorbed jcode pieces before (color downsampling, truncation,
  the `tui-ui` split itself).
- **coco-rs** has the strictest architecture of the three — enforced pure
  render seam, engine-authoritative transcript, TEA, module-size and test
  discipline, and the only accessible/international/multi-provider-neutral
  feature surface. Its churn was not aimlessness: it was the price of
  combining the hardest surface with the strictest state model, paid through
  one seam (stream↔scrollback reconciliation) and one split-brain (geometry).
  v2 removes both *by construction* and keeps everything else coco already
  got right.

## 10. Staged Implementation Plan (Scope B) — source-verified

Status: **Stages 0-4 IMPLEMENTED** on the current `feat/tui` line after
selective migration from `feat/tui2` with current-branch behavior preserved.
The modal split and cell-model re-home are implemented: `modal_pane/` owns
generic modal behavior plus `model_picker`, `team_roster`, `settings`, and the
dedicated `permissions_editor`; `transcript/cells.rs` owns `RenderedCell` /
`CellKind` / `SystemCellKind`; `transcript/derive.rs` owns `message_to_cells`
and tool-cell helpers; `state::TranscriptView` remains only the per-session
derived-cell container. Stage 4 is closed. Scope C remains deferred as a
separate optimization track.

A same-day adversarial review hardened Stage 1: (a) the soundness pin was
strengthened — the pre-existing prefix-stability test only compared
stable-prefix renders against *larger stable prefixes*, while the anchored
finalize relies on stable-prefix vs **full-document** render (unstable tail
included); `test_stable_lines_are_row_prefix_of_full_committed_render` now
pins exactly that relation (passes — including setext/loose-list/partial-line
traps); (b) the anchored finalize moved to `transcript/emission.rs` as the
pure `finalize_after_stream_prefix` (the §6.4 target — `emission` now owns the
suffix-vs-replay decision; per-frame state and terminal I/O stay in
`surface/history_driver`); (c) `PendingStreamPrefix` lost its redundant
`source_prefix_len` and moved next to the watermark in `transcript/stream.rs`
(`PreparedStreamAppend.watermark` is now derived, not duplicated);
(d) `StreamRenderKey::committed` takes `(styles, width, syntax)` directly — no
more fake-empty-source key construction; (e) the U5 in-flight render goes
through `render_in_flight_assistant_markdown` (same renderer, streaming flag
set) so mermaid layout runs once at finalize instead of per delta;
(f) deprecated `model.rs` (Stage 4 item) deleted early — zero non-test
consumers.

A second (independent-agent) review wave added: (g) **cross-turn watermark
fix** — event coalescing can fold `MessageAppended(turn N)` + turn N+1's
first deltas into one draw, so `SurfaceStreamDriver` never observes the
`streaming == None` gap that clears its watermark; the length-only
`emitted_valid` check could then re-attribute turn N's watermark to turn N+1
and silently skip the new turn's leading scrollback rows (a case the deleted
fingerprints used to self-heal via replay). A controller reset now
invalidates the watermark outright (identity over size); pinned by
`watermark_does_not_survive_source_replacement`. (h) the in-flight render
uses a **single-slot memo** instead of the shared committed map — per-delta
content hashes were flooding the map with dead snapshots and wholesale-
clearing legitimate committed entries at the cap. (i) the soundness pin
gained blockquote/table/late-reference-link traps and a narrow-width pass.
Second honest deviation: U5's "fold `ActiveTranscriptCell::Streaming` into
the transcript owner" was **not** done — the active-cell enum stays in
`presentation/transcript.rs` and `render_streaming` on `ChatWidget`; it is
part of the same Stage-2+ ownership story as the cell model (the in-flight
render does go through the shared committed renderer, which is the
load-bearing half). Known accepted trade: the fallback path renders the full
in-flight document per delta (memo dedupes repeat frames; reveal pacing
bounds the rate), and mermaid diagrams appear at finalize in the fallback
while the native stable region may show them mid-stream. This section
supersedes the recon workflow's U1–U6 draft after two rounds of source-level
cross-validation:
the recon's "U1 engine segmentation fix" is **void** (a no-op in both
tool-exec modes, §6.2.1), and the deletion scope is **narrower** than the
recon's "delete the verify half" (the finalize markdown render survives —
it produces the suffix rows; only the rasterized fingerprint compare and
its accumulation delete, §6.2).

### 10.1 Stage 0 — gates (cheap; all unblocked)

1. **Source-contract pin test** (inference layer; *no engine behavior
   change*). Home: `services/inference/src/stream.test.rs`, alongside
   `snapshot_preserves_text_tool_text_interleaving` (`:412-478`). Pin: for
   each `AssistantContent::Text` part, `part.text` equals the byte concat of
   that part's `TextDelta` run (`TextStart/Delta/End` accumulate per-id,
   `stream.rs:188-232`), empty parts dropped symmetrically
   (`engine.rs:1134` / `derive.rs:107`). Cover: single run;
   `Text→Tool→Text` 3-part shape; cancel turn (single Text part from
   `response_text`, `engine_stream_consume.rs:434-460`). Document the
   `emit_stream` error-swallow (`engine_stream_consume.rs:260`,
   `#[must_use]` bool discarded) as the known divergence the TUI
   anchor+replay already guards. Part identity is erased at the
   `StreamEvent::TextDelta` level, which is why this pin cannot live in
   `app/query`.
2. **Doc registration** — `docs/coco-rs/CLAUDE.md` Document Map + File
   Index rows, `ui/terminal-surface-design.md` document-map row (done with
   this revision).
3. **Zero-churn acceptance baseline**: the native-surface snapshot set
   rendered through `testing::render_native_surface_to_string` (~17 `.snap`
   files incl. `completed_parallel_tool_batch`, the streaming and
   thinking-collapsed snapshots) must be byte-identical across Stage 1
   (`cargo insta pending-snapshots -p coco-tui` empty for these paths). The
   ToolBatch named-header change (§6.5) landed *before* Stage 1 precisely
   so this baseline already includes it.

### 10.2 Stage 1 — commit-sized units (each lands green except U3 mid-edit)

**U1 — `transcript/` skeleton + re-home (pure move; implemented on current `feat/tui`).**
New files `transcript/{mod,cells,render,stream,emission}.rs`. Moves, logic
verbatim: the render core of `streaming/render_controller.rs:75-251`
(projection, render-key reset/invalidations, `streaming_cursor_line`, the
`StreamRenderKey/Input/Projection/Region/Mode` types) and
`StreamHistoryWatermark` (`surface/stream.rs:71-76`) → `transcript/stream.rs`;
`render_finalized_history_lines` + `HistoryLineRenderOptions` +
`HistoryReplayCache` + replay entries (`surface/history_lines.rs`) →
`transcript/render.rs` (`render_committed_assistant_markdown` and its
content-addressed memo in `widgets/chat/render_assistant.rs:80` stay the
leaf — called, never forked); `RenderedCell`, `CellKind`, `SystemCellKind`,
`message_to_cells`, and the tool-cell helpers now live under
`transcript/{cells,derive}.rs`; the tool-commit helpers
`committable_prefix_len` / `find_forward_unconsumed_tool_result` /
`engine_message_start` (`history_driver.rs:673-716`) → `transcript/cells.rs`,
which documents the ordering + batch/pairing projection
(`presentation/transcript.rs`) as its contract. Callers to repoint:
`surface/controller.rs`, `surface/stream.rs`, `surface/history_driver.rs`,
and the thread-local `STREAM_RENDER_CONTROLLER` in `widgets/chat/mod.rs:66`
(the non-native fallback — a second consumer that must not vanish).

**U2 — `HistoryEmissionTracker` → `transcript/emission.rs`, verbatim.**
Zero logic edits (the I-1 exactly-once authority); repoint
`surface/controller.rs` imports.

**U3 — delete the rasterized reconciliation (the core; one commit, tree
broken mid-edit, green at end).**
- DELETE: `EmittedStreamPrefix.line_fingerprints` (`surface/stream.rs:34`;
  the struct collapses to a bare watermark carrier);
  `PendingStreamPrefix.line_fingerprints` (`:68`; struct shrinks to
  `{source_prefix, source_prefix_len, line_prefix_len, render_key}`); the
  stream-driver fingerprint accumulation (`stream.rs:138-149`) and its
  imports (`:15-16`); the finalize fingerprint compare
  (`history_driver.rs:630-650`); the test
  `stream_append_fingerprints_accumulate_incrementally_to_full_prefix`.
- KEEP inside the same finalize function (`append_candidate_lines_after_
  stream_prefix`, which *becomes* the anchored finalize rather than being
  deleted): anchor location with the same-message thinking skip
  (`:537-558`), text-cell and thinking-only-group checks (`:559-581`),
  source `starts_with` (`:582-594`), render-key gate (`:595-608`), the
  full-text markdown render (`:610-618` — produces the suffix), the
  `line_prefix_len` sanity guard (`:620-629`), suffix compose +
  `Line::default()` separator + canonical post-text order (`:652-669`).
- KEEP elsewhere: `line_fingerprint.rs` and the session-header usage
  (`history_driver.rs:45,143,439`); `commit_stream_append` (`:293-321`,
  now storing the shrunk prefix struct); all replay paths.
- Compile cascade to fix in the same commit: `surface/controller.rs`
  `PreparedStreamAppend` field reads (`.rows` / `.expected_rows()` /
  `.watermark` survive; `:16,60,79-81,101,132,271,329,353,363-367`);
  `terminal.rs:38,417-419` perf-log row counts. Rewrite the finalize-verify
  tests in `history_driver.test.rs` / `controller.test.rs` to assert
  anchor-match→suffix-append and mismatch→replay without fingerprint
  assertions; keep every other test verbatim (tool pairing, render-key
  replay, header dedup, §6.1 presentation pins, C1/A4 geometry).

**U4 — collapse `NativeSurfaceController` to one prepare pass.**
Merge the `stream_prepare`/`history_prepare` stages and stats; keep
`NativeSurfaceFramePlan::guaranteed_append_rows()` and
`history_tail_reveal_rows` **verbatim** (the Stage-2 geometry contract:
`commit_native_viewport_geometry`'s `backed_rows = reveal + append`,
`terminal.rs:833`); keep `fill_history_tail_gap`, `reset`, and the
`draw_with_plan_at_frame` signature (the testing harness has 40+ call
sites).

**U5 — route the live tail through `render_cell` (the only NEW code).**
The in-flight stream renders as a synthetic in-flight `AssistantText` cell
through the same render path, retiring the thread-local streaming renderer
(`widgets/chat/mod.rs:503-533`) and folding
`ActiveTranscriptCell::Streaming` into the transcript owner — this closes
invariant §6.7-2 ("no streaming-only renderer exists"), which is currently
aspirational. The non-native `width == 0` fallback
(`build_live_tail_lines`) stays. Guarded by the streaming snapshots.

### 10.3 Stage boundaries (explicitly not Stage 1)

- **Stage 2 — DONE** (same branch, after Stage 1): the seat/pin decision
  moved into `coco_tui_ui::engine::seat` —
  `SurfaceTerminal::seat_viewport(SeatInputs) -> SeatDecision` anchors on the
  owned viewport top and consumes the engine-internal unclamped history
  extent (`history_backs_row`) so no clamped proxy can reach the pin
  predicate (I-V2 by construction); `flowing_seats_flush` (I-V1) is an
  engine method; the shrink/reveal arbitration (`commit_seat`) and height
  clamp moved with it. The shell's `sync_main_surface_area` now supplies
  typed intent (desired height, `NATIVE_VIEWPORT_MIN/MAX` policy bounds,
  tail-reveal/append backing) and applies the returned decision;
  `commit_native_viewport_geometry` / `native_viewport_geometry_with_max` /
  `native_viewport_height` / `flowing_viewport_seats_flush` and the
  `NativeViewportPin/Geometry/Commit*` types are deleted from `terminal.rs`.
  The pure seat math tests moved to `engine/seat.test.rs` (including a
  decision-level C1 pin, `seat_stays_pinned_when_unclamped_extent_backs_
  pinned_row`); the frame-level C1/A4 suite stays in `terminal.test.rs`
  driving the full `sync → commit → tail fill → emission → draw` path
  (I-V4). Deliberately NOT moved: overlay/alt-screen policy (shell-side,
  alt frames don't seat — N3 by construction), `interactive_viewport_*`
  height policy (reads `AppState`), the `HistoryTailCache` and
  `fill_history_tail_gap` (shell data + I/O execution — they feed
  `SeatInputs.tail_reveal_rows` and execute the engine's
  `reveal_tail_rows` verdict), and `surface/viewport.rs` (AppState-coupled
  view code, barred from the engine by the seam).
- **Stage 3 — DONE**: `bottom_pane/` local view stack and `modal_pane/`
  behavior split (**B1-B4 implemented; B5 closed by disposition**).
  Landed: `app/tui/src/bottom_pane/{mod,permission,question,plan}.rs` —
  the routing layer (`route_approve/deny/confirm/nav/filter[_backspace]`,
  one match per command instead of eight-way matches in each free
  function) plus per-surface behavior modules;
  `app/tui/src/modal_pane/{mod,model_picker,team_roster,settings,
  permissions_editor}.rs` — modal routing plus per-modal maps/behavior for the
  model picker, team roster, settings, and the current-branch `/permissions`
  editor. `update/interaction.rs` is now the precedence shell only:
  prompt-first for approve/deny/filter/nav, modal-first for confirm, and
  autocomplete before prompt/modal routing. The Confirmation/Question key maps
  moved out of `keybinding_bridge` onto `bottom_pane`; modal key maps moved
  onto `modal_pane`. The skills, agents, and plugin dialog interceptors remain
  in `update/` until those surfaces are migrated. B5 is closed by
  disposition: after the modal/prompt split there is no remaining central
  viewport prompt-rendering match that needs to be migrated to complete
  Stage 3.

  Original survey + target shape (kept for context): Current state: the
  prompt stack already exists and is sound — `state/interaction.rs` owns
  `PanePromptState` (8 variants) with the attention-safety `priority()`
  ordering; the sprawl is `update/interaction.rs` (1,713 LoC), where one
  `nav()` / `confirm()` / `filter()` family matches over every prompt kind
  AND the modal surfaces (model picker effort cycling, team-roster mode
  cycling, settings toggles, session browser filtering). After the Stage-4
  cascade fold, the key-routing pipeline is already three explicit layers
  (reserved exits → resolver → per-surface nav maps → composer), so Stage 3
  is narrower than the original sketch assumed.

  **Target shape — TEA-compatible, NOT codex's `Box<dyn View>` object
  stack** (§5 keeps TEA as a retained asset; an object stack with
  self-owned mutable state would fork `AppState`'s single mutable source):
  a `bottom_pane/` module where each prompt/surface is one submodule
  implementing one trait of pure update functions
  (`handle_nav` / `handle_confirm` / `handle_filter` over `&mut` its OWN
  state struct + a typed effect return), with the existing priority stack
  re-homed as the module's `PaneStack`. The shared `SurfacePrev/Next/
  Confirm/Filter` TuiCommands stay; dispatch goes through one
  `route_to_focused_surface` match instead of today's eight-way matches
  inside each free function.

  Commit units: **B1** trait + `PaneStack` re-home (pure move of
  `state/interaction.rs` stack logic); **B2** one commit per prompt kind
  (8 small moves out of `update/interaction.rs`); **B3** modal-surface
  odds-and-ends (model picker / settings / roster / session browser) into
  their surfaces; **B4** collapse the bridge's per-surface special cases
  (`map_question_key` etc.) into the surfaces' own key handlers, making
  the routing order `focused surface → resolver → composer → residual`
  literal in one function; **B5** closed by disposition as above. Acceptance: the
  existing `update/interaction.test.rs` suite ports per-unit with zero
  behavior change; native-surface + prompt snapshots stay byte-identical.
- **Stage 4 — DONE**: ~~legacy keybinding cascade folded into
  `coco-keybindings` defaults~~ — the audit found six of the cascade's
  global arms were already dead (the resolver shadowed ctrl+l /
  ctrl+shift+f / ctrl+s / ctrl+g / shift+tab / the platform paste key);
  the live arms became six documented coco-extension actions
  (`app:forceQuit|help|commandPalette|settings`,
  `chat:toggleSystemReminders|togglePlanMode`) plus second default
  bindings on existing actions (ctrl+f, ctrl+m, the mirror paste key),
  all now user-rebindable. The hardcoded residue is only what cannot be
  a binding: per-surface navigation maps, readline editing, `?`-on-empty
  (must fall through to typing), PageUp/PageDown, F6.
  ~~Deprecated `model.rs`~~ (deleted in the Stage-1 review round — zero
  non-test consumers); ~~the `reveal_all` doc-comment drift~~ (fixed:
  `advance_display` doc corrected, `reveal_all` is `#[cfg(test/testing)]`
  with an honest doc).
- **Scope C (deferred; value reduced by cross-validation)** — per-segment
  pending prefixes at the `history_driver.rs:308` single-slot overwrite;
  buys back replays only on the §6.2.1 minority shape.

### 10.4 Verification cadence

`just quick-check` per unit; focused `cargo test -p coco-tui` (streaming /
history_driver / controller filters) + the §10.1-3 zero-churn snapshot
check for U3/U5; one `just pre-commit` at the very end, user-initiated,
per repository rules.

### 10.5 Readiness

Stages 0-4 are **done** (see the §10 status note). The recon's "BLOCKER"
finding was void (§6.2.1) as predicted, and the zero-churn acceptance gate
held: the ~17 native-surface `.snap` files stayed byte-identical (the one
regenerated snapshot, `transcript_modal_parallel_glob_results`, was a
**pre-existing** stale modal snapshot the `tool_batch_name_summary` landing
had missed — outside the native-surface acceptance set, unrelated to this
refactor). The only remaining track in this document is Scope C, deferred
until performance or UX evidence justifies eliminating replay for
within-message multi-text turns.
