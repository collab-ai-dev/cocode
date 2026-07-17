# TUI Single Render-Path Convergence

Status:

- **Policy A landed** and remains the conceptual baseline (§2).
- **Policy B has landed** (`8778909429 tui: stream stable assistant rows into
  native history`) and is the default native-scrollback streaming policy:
  stable assistant markdown rows enter native scrollback mid-stream through
  the committed renderer plus a stable-prefix watermark.
- **Post-landing hardening landed 2026-06-10** — see §14 Implementation
  Status: the leading-thinking presentation/verify alignment (removes the
  every-turn structural replay and its visible transcript duplication),
  O(delta) incremental line-hash fingerprints, a borrowed stream projection
  (no per-frame line-vector clone), background syntect grammar prewarm, and
  the frame-stage perf instrumentation that diagnosed all of the above.
- **Backward compatibility is intentionally disregarded.** This design removes
  legacy provisional machinery instead of preserving old behavior behind a
  toggle.

This document specifies the native scrollback streaming model for `coco-tui`.
Policy B allows stable assistant markdown rows to be emitted while an answer
is still streaming, while the mutable tail remains in the retained viewport.
It must not restore the old provisional implementation from `#160 perf(tui):
streamline native history rendering` (commit `95cea8827`).

The design is scoped to the streaming -> scrollback seam. It does **not**
redefine terminal mechanics (`native-scrollback-architecture.md`), the
transcript-cell contract (`codex-rs-tui-comparison.md` sections 1-2 and 4), or
the console product shape (`agent-console-design.md`). Those remain the owners
of their contracts; this document references them by name.

## 1. Why this exists

`native-scrollback-architecture.md` **Core Decision #4** is explicit:

> Only finalized history enters native scrollback. Streaming text, running
> tools, ... remain in the interactive viewport. When a stream/tool/message
> finalizes, it is emitted as history rows **exactly once**.

Policy A restored that contract and is now the stable baseline. Policy B is a
documented change from that baseline because long in-flight answers can produce
more stable markdown than the bounded viewport can usefully retain. If product
behavior requires those stable rows to move into native scrollback before the
message finalizes, the implementation must be a single-render-path streaming
model, not the previous provisional/finalize reconciliation path.

The old provisional path had two coupled defects in `app/tui/src/surface/`:

1. **Two independent render paths had to produce byte-identical lines.**
   Streaming-stable rows and finalized history rows were rendered by different
   paths, with `ChatWidget::native_history_append_compatible()` acting as a
   compatibility shim. Nothing at runtime proved the rows still matched.
2. **Finalize reconciled by line count rather than row content.**
   `consolidated_final_tail_lines` appended
   `render_finalized_history_lines(whole).skip(provisional.line_count)`. The
   guard checked render-key equality and source prefix shape, but it never
   verified that the first `provisional.line_count` finalized rows were the
   rows already committed.

That class of defect must remain deleted. Policy B explicitly rejects:

- dual committed/provisional render paths;
- count-based repair such as `skip(line_count)`;
- early `ToolUse` header commits before matching results exist;
- replay, resize, or theme handling that can solidify unstable stream rows.

## 2. Policy A Baseline

Policy A remains the stable committed-history baseline:

- live streaming is viewport-only;
- finalized assistant messages enter native scrollback after `MessageAppended`
  commits them to the transcript;
- finalized append and replay share
  `render_committed_assistant_markdown(source, opts)`;
- `HistoryEmissionTracker` tracks finalized transcript message UUIDs;
- no native scrollback writes occur during streaming.

Policy B builds on this by adding progressive streaming commits for assistant
markdown only. It does not weaken the Policy A committed renderer invariant:
finalized append and replay still use the same committed assistant markdown
renderer.

## 3. Policy B Target Architecture

Policy B uses the codex-style shape that Policy A deliberately deferred:

- **One source-backed assistant markdown render vector.** The stream controller
  renders the accumulated assistant markdown source into one logical
  `rendered_lines` projection for the current render key. (As built it is a
  cached stable-region vector plus a mutable-tail vector exposed as borrowed
  slices — see §7; `rendered_lines[..]` notation below denotes their
  concatenation.)
- **One stable-prefix watermark.** The controller tracks how many rendered rows
  have already entered native scrollback. The invariant is
  `emitted_stable_len <= stable_line_len <= rendered_lines.len()`.
- **Native stream append emits only the stable delta.** The append is always
  `rendered_lines[emitted_stable_len..stable_line_len]`; after a successful
  native insert, `emitted_stable_len = stable_line_len`.
- **The live viewport tail starts after the stable watermark.** The retained
  viewport renders `rendered_lines[emitted_stable_len..]`, plus any
  streaming-only adornments. Already emitted stable rows must not reappear in the
  viewport tail.
- **Finalized append and replay use the same committed assistant markdown
  renderer.** Finalization either appends a verified suffix or triggers replay
  from source. Replay always renders through
  `render_committed_assistant_markdown`, not through a streaming-only renderer.

The committed scrollback content is therefore a prefix slice of a single render
projection, with source and render-key metadata sufficient to decide whether a
final suffix append is valid. There is no provisional ledger and no
cross-render `skip(count)` reconciliation.

### 3.1 Stream Append Lifecycle

For each assistant text delta:

1. `StreamingState.visible_content()` remains the source input.
2. `StreamRenderController` renders the current source using the committed
   assistant markdown renderer configuration for the current width, theme,
   syntax, and render key.
3. `coco_tui_markdown::stable_prefix_end` identifies the conservative source
   boundary that can be considered stable.
4. The controller maps that source boundary to `stable_line_len` in the current
   rendered vector.
5. `SurfaceStreamDriver` prepares the live tail and an optional
   `PreparedStreamAppend` containing only the stable delta.
6. `SurfaceHistoryDriver` inserts the stable delta into native scrollback and
   records a source-backed pending prefix fingerprint: one `u64` content hash
   per rendered line (`RenderedLineFingerprint`). The fingerprint vector is
   accumulated incrementally — each advance hashes only the delta lines and
   extends the fingerprints of the already-committed prefix (which advance
   only when the insert actually commits), so per-advance cost is O(delta),
   not O(prefix).

On final assistant `MessageAppended`, `SurfaceHistoryDriver` compares the
pending prefix fingerprints against the committed whole-message render (line
hashes — no row rasterization of the prefix is needed). If the prefix matches
exactly, it appends only the suffix. If the prefix does not match, it discards
the streaming watermark and replays from source. It never repairs by skipping
a count of rows.

### 3.2 Viewport Tail

The live viewport tail starts at the stable watermark. It may contain mutable
markdown, cursor/thinking adornments, preview-only mermaid behavior, and other
streaming-only UI. Those rows are not committed to native scrollback until they
become stable under the committed renderer and cross the watermark.

The retained viewport must account for native stream append rows when computing
bottom-pinned geometry. If stable rows are emitted while the viewport is pinned
to bottom, the visible position should remain coherent rather than duplicating
rows or jumping.

## 4. Prior Failure Lessons

Policy B exists because the old implementation mixed incompatible ideas. The
new implementation must preserve these lessons as requirements:

- Old provisional rows used a streaming renderer while finalized history used
  another renderer.
- Finalize reconciled with line counts instead of row-content verification.
- Parallel tool headers could be committed before matching results existed.
- Replay, resize, and theme changes could solidify rows that were still
  unstable.
- Height-only viewport changes caused unnecessary replay and visible flicker.

These are design constraints, not bugs to patch locally after the fact.

## 5. Markdown Rendering Contract

Policy B stable rows must use
`render_committed_assistant_markdown(source, opts)`. The stream path may cache
or slice results, but rows inserted into native scrollback must be rows produced
by the committed assistant markdown renderer for the active render key.

`coco_tui_markdown::stable_prefix_end` remains the conservative source boundary.
The following content remains held back in the mutable tail until it is
unambiguously stable:

- tables;
- open fenced code blocks;
- partial lines;
- setext headings whose underline may still arrive;
- unresolved reference links.

Mermaid previews or other streaming-only behavior may exist only in the live
mutable tail. Once rows cross the native scrollback boundary, they are committed
assistant markdown rows and must match finalized/replayed committed history.

Any width, theme, syntax, or render-key change invalidates the watermark. The
driver must reset the stream watermark and replay from source instead of trying
to reinterpret already emitted rows. Height-only viewport changes do not change
the render key and must not trigger replay or visible flicker.

## 6. Tool Call Boundary

Policy B v1 applies only to assistant text markdown. It does not stream
`ToolUse`, `ToolResult`, running tool activity, or active tool cells into native
scrollback.

Tool boundaries remain transcript-commit boundaries:

- `ToolUse` headers are never emitted mid-stream.
- `ToolUse` and `ToolResult` history continues through
  `committable_prefix_len`.
- An orphan `ToolResult` does not block the prefix.
- An unresolved `ToolUse` blocks the prefix.
- Duplicate call IDs require one result per `ToolUse`; one result cannot satisfy
  multiple tool uses with the same call ID.

This avoids the old failure mode where parallel tool headers could enter native
scrollback before the matching results existed.

### 6.1 Leading Thinking Cells

An assistant message with reasoning projects to `[AssistantThinking…,
AssistantText, …]` transcript cells, but the streamed rows that enter native
scrollback are always **text** rows. Two coordinated rules keep the committed
prefix the leading rows of the group:

- **Presentation** (`push_text_first_assistant_group`): a message's leading
  thinking cells render AFTER its first text cell, **independent of what
  follows the text** (tool calls included). The rule being suffix-independent
  is load-bearing twice over: a message renders identically whether it is
  projected mid-turn (the committable slice ends at the text because its tool
  uses are still unresolved) or after its results pair, and the streamed text
  rows are the group's leading rows under both incremental append and full
  replay.
- **Verification** (`append_candidate_lines_after_stream_prefix`): the
  pending stream prefix anchors to the message's text cell. The verify skips
  the same-message leading-thinking run, verifies the prefix against the text
  cell, and composes the suffix as `[text remainder, separator, thinking
  cells, rest]` — matching the presentation order row-for-row. A thinking run
  whose text belongs to a different message (thinking-only group) does not
  reorder and falls back to replay.

Before this alignment landed, every thinking+text turn structurally failed
verification (`pending_stream_prefix_next_cell_not_assistant_text`) and
forced a full replay per turn. Because `clear_owned_scrollback` can only
clear rows still inside the owned on-screen region, each replay re-inserted
the full transcript below the unreachable scrolled-out copy — the visible
"prompt rendered twice, then gone" duplication. With the alignment, replay is
reserved for genuine invalidation (width / theme / syntax / display-mode
changes, header changes, real prefix mismatches).

## 7. Rust Shape (as built)

Keep the data model small and purpose-specific. The landed structs
(`app/tui/src/`):

```rust
// streaming/render_controller.rs — borrows the controller's cached vectors;
// consumers clone exactly the slices they need (no per-frame rebuild).
struct StreamRenderProjection<'a> {
    stable_lines: &'a [Line<'static>], // committed-renderer output
    tail_lines: &'a [Line<'static>],   // mutable-tail render
    stable_source_len: usize,
    render_key: StreamRenderKey,
    render_key_invalidated: bool,
}

// surface/stream.rs — watermark + fingerprints bundled so they cannot
// desync; advances ONLY when the native insert commits.
struct EmittedStreamPrefix {
    watermark: StreamHistoryWatermark, // source_len + line_len + render_key
    line_fingerprints: Vec<RenderedLineFingerprint>,
}

struct PreparedStreamAppend {
    rows: HistoryRows,                 // pre-rasterized stable delta
    prefix: PendingStreamPrefix,
    watermark: StreamHistoryWatermark,
}

// surface/stream.rs → consumed by surface/history_driver.rs at finalize.
struct PendingStreamPrefix {
    source_prefix: String,
    source_prefix_len: usize,
    line_prefix_len: usize,
    render_key: StreamRenderKey,
    line_fingerprints: Vec<RenderedLineFingerprint>,
}

// surface/line_fingerprint.rs — u64 content hash per rendered line (line
// style + alignment + span content/styles). Process-local only. Shared with
// the session-header fingerprint path.
struct RenderedLineFingerprint(u64);
```

The ownership boundary is binding:

- `StreamingState.visible_content()` remains the source input.
- Do not store ratatui `Line`s, terminal rows, row fingerprints, or emitted
  counters in `AppState`.
- `StreamRenderController` owns source-backed markdown rendering and watermark
  decisions.
- `SurfaceStreamDriver` prepares the live tail and optional stream append.
- `SurfaceHistoryDriver` commits native rows and tracks pending prefix
  fingerprints.
- `HistoryEmissionTracker` remains for finalized transcript message UUIDs only.

Rust implementation rules:

- no `unwrap()` in production code;
- prefer enums and newtypes over ambiguous booleans;
- keep modules focused, and split files that exceed local size guidance;
- add tests before changing behavior in known regression areas.

## 8. Component-Level Change Set

All rows below have landed (see §14 for the post-landing hardening). Paths are
under `coco-rs/app/tui/src/`.

| Symbol / file | Action | Notes |
|---|---|---|
| `streaming/render_controller.rs` | Extend | Own the source-backed markdown render vector, stable source boundary mapping, and `StreamHistoryWatermark`. |
| `surface/stream.rs` / `SurfaceStreamDriver` | Extend | Prepare `MarkdownStableTail` and optional `PreparedStreamAppend`; keep live tail in the retained viewport. |
| `surface/history_driver.rs` | Extend | Insert stream stable deltas, track `PendingStreamPrefix`, verify final prefix fingerprints, and replay on mismatch. |
| `surface/history_lines.rs` | Keep committed path | Finalized assistant text continues through `render_committed_assistant_markdown`. |
| `HistoryEmissionTracker` | Keep scoped | Track finalized transcript message UUIDs only; do not reuse it for streaming watermark state. |
| `committable_prefix_len` logic | Keep for tools | Tool history remains transcript-bound and paired before commit. |
| `tui-ui/src/engine/terminal.rs` `insert_history_rows`, `clear_owned_scrollback` | Keep | Terminal primitives remain the insertion/replay mechanism. |

Do not reintroduce deleted Policy A cleanup targets:

- `ProvisionalStreamLedger` / `CommittedStablePrefix`;
- `PreparedProvisionalAppend`;
- `mark_stable_appended` / `forget_stable_appended` dances;
- `emit_provisional_stream`;
- `consolidated_final_tail_lines`;
- `ProvisionalFinalizationGuard`;
- finalize-only `skip(provisional.line_count)`;
- `ChatWidget::native_history_append_compatible()`.

## 9. Invariants

1. Stable streaming rows inserted into native scrollback are produced by
   `render_committed_assistant_markdown`.
2. Native stream append inserts only
   `rendered_lines[emitted_stable_len..stable_line_len]`.
3. The live viewport tail starts at `emitted_stable_len`.
4. Final assistant history appears exactly once after finalize.
5. Final suffix append requires exact pending-prefix fingerprint match.
6. Prefix mismatch triggers replay from source, never count-based repair.
7. Width, theme, syntax, and render-key changes reset the watermark and replay.
8. Height-only viewport changes do not replay or flicker.
9. Tool headers/results remain transcript-bound and are not emitted by Policy B
   stream markdown append.
10. Replay never marks unresolved tools or stream prefixes as finalized.
11. A message's leading thinking cells render after its text under the native
    presentation, independent of what follows the text — committable-slice and
    full-transcript renders agree (§6.1).
12. The finalize verify anchors the pending prefix to the message's text cell,
    skipping its same-message leading-thinking run; any other shape replays.
13. Fingerprint accumulation is O(delta) per watermark advance and advances
    only on a committed insert; a prepared-but-uncommitted append never
    mutates driver state.

## 10. Test Plan

Update or add tests for these behaviors:

- streaming stable markdown appends to native scrollback;
- live tail excludes already emitted stable rows;
- final assistant message appears exactly once;
- finalized suffix append requires prefix fingerprint match;
- prefix mismatch triggers replay, not skip-based repair;
- width, theme, syntax, or render-key change during stream resets the watermark;
- markdown tables, open fences, partial lines, setext headings, and unresolved
  reference links stay mutable;
- parallel tool calls do not emit headers until all results are paired;
- orphan `ToolResult` does not block, unresolved `ToolUse` blocks, and duplicate
  call IDs require one result per use;
- replay does not mark unresolved tools or stream prefixes as finalized;
- bottom-pinned viewport geometry includes stream stable append rows;
- height-only viewport changes do not cause replay or flicker.

Landed alongside the §6.1 / §7 hardening (`surface/history_driver.test.rs`,
`surface/stream.test.rs`):

- `driver_finalizes_stream_prefix_for_thinking_text_turn_without_replay` —
  thinking+text turn appends the verified suffix; thinking renders after text;
- `driver_stream_suffix_append_matches_full_replay_for_thinking_turn` —
  incremental stream-suffix append is row-identical to a full replay of the
  same cells (the strongest §9-4/5 pin);
- `driver_requires_replay_when_thinking_run_lacks_same_message_text` —
  thinking-only groups still replay;
- `finalized_native_history_renders_text_before_thinking_when_tools_follow` —
  the §6.1 presentation rule with trailing tool calls;
- `stream_append_fingerprints_accumulate_incrementally_to_full_prefix` —
  incremental fingerprints equal a from-scratch fingerprint of the same
  prefix.

Keep the useful Policy A coverage:

- finalized append output equals replay output line-for-line at the same width,
  theme, syntax, and source;
- live streaming renderer differences do not affect committed append/replay
  identity;
- replay after finalize renders from source through the committed renderer.

Follow the testing split in `codex-rs-tui-comparison.md` section P0: ratatui
`TestBackend` for buffer snapshots, byte-capturing VT100 backend for
terminal-control assertions.

Verification commands from `coco-rs/`:

```bash
cargo test -p coco-tui streaming
cargo test -p coco-tui history_driver
cargo test -p coco-tui
just quick-check
```

Before commit:

```bash
just pre-commit
```

## 11. codex-rs Reference Map

| Concern | codex-rs source | coco target |
|---|---|---|
| Single rendered-line vector + watermark | `streaming/controller.rs:31,58,204` | Policy B source-backed assistant markdown vector + `StreamHistoryWatermark` |
| Source-backed finalized cell | `history_cell/*` `AgentMarkdownCell`; `app/agent_message_consolidation.rs` | `AssistantText` `TranscriptCell` + `render_committed_assistant_markdown` |
| Newline-gated markdown accumulation | `markdown_stream.rs` `MarkdownStreamCollector` | stream controller source accumulation + `stable_prefix_end` |
| Emit-once history insertion | `insert_history.rs` | `insert_history_rows` + pending-prefix fingerprint verification |
| Resize = re-render from source | `streaming/controller.rs:231`, `app/resize_reflow.rs` | watermark reset + `replay_all_capped` |

Reuse policy and attribution requirements are owned by
`codex-rs-tui-comparison.md` section Reuse Policy. Port ideas behind coco-owned
types; do not depend on `codex-*` crates.

## 12. Risks and Rollback

- **Risk: native scrollback streaming changes the terminal contract.** This is
  intentional for Policy B, but it must stay limited to assistant markdown
  stable rows.
- **Risk: prefix verification forces replay more often than expected.** Prefer
  replay to silent duplication or dropped rows; optimize only after correctness
  is pinned by tests.
- **Risk: markdown stability is too conservative.** Keep the conservative
  boundary first. Loosen `stable_prefix_end` only with focused markdown tests.
- **Rollback:** revert the Policy B implementation to return to the landed
  Policy A baseline. Do not resurrect the old provisional path.

## 13. Out of scope

- Running tool activity and active tool cells.
- Streaming `ToolUse` or `ToolResult` rows into native scrollback.
- Non-native fallback streaming; it remains viewport-only.
- Pager/diff overlays, picker scaffolding, bottom-pane stack.
- Terminal primitive behavior outside the stream append/replay seam.
- A backward-compatibility toggle or non-native-scrollback fallback changes.

## 14. Implementation Status (2026-06-10)

Policy B landed in `8778909429`. Two instrumented production runs
(`tui.performance.enabled` + `tui=debug`) then drove a hardening pass; all of
it is verified by the §10 tests, `just quick-check`, and the full `coco-tui` /
`coco-tui-markdown` suites.

### Diagnosed and fixed

| Finding (measured) | Fix |
|---|---|
| Every thinking+text turn forced a full replay (`pending_stream_prefix_next_cell_not_assistant_text`, 2/2 turns in the run); each replay re-inserted the whole transcript below the unreachable scrolled-out copy — user-visible duplication | §6.1 presentation/verify alignment: suffix-independent text-before-leading-thinking reorder + thinking-aware prefix verify with row-identical-to-replay parity test |
| 30–100ms finalize frames — syntect compiles each grammar's regexes lazily on first parse (Markdown 78.7ms for an 8-line preview; the same cells re-rendered in 11ms once warm). `SyntaxSet` deserialization itself is ~0.7ms | `coco_tui_markdown::prewarm_highlighting()` (parse-only, no theme dependency; per-grammar + total timings under `tui::perf::init`) spawned once on a named background thread from `App::run` |
| Fingerprint cost O(full prefix) per watermark advance: full-prefix re-rasterization plus a per-CELL `String` deep copy (≈22k allocations per advance at 242 columns), then cloned again in `prepare_native_frame` | `RenderedLineFingerprint` is a `u64` content-hash newtype; fingerprints accumulate incrementally (O(delta)) and advance only on committed inserts (`EmittedStreamPrefix`); finalize verifies line hashes with no prefix rasterization; `prepare_native_frame` destructures instead of cloning |
| `render_projection` rebuilt + cloned the full stable+tail line vector every frame (~40 fps) while the viewport uses at most `STREAMING_LIVE_TAIL_CAP` rows | `StreamRenderProjection` borrows the controller's cached vectors; the live tail clones only the post-watermark slice, pre-trimmed to the display cap |

Ruled out by instrumentation: terminal backpressure (`begin_sync_update` max
0.5ms), event-fold overhead (redraw vs draw gap ≤0.1ms).

### Perf instrumentation (landed; gated on `tui.performance.enabled`)

- `prepare_native_frame` stage (renamed from the misleading
  `build_live_tail_lines`): `plan_us` / `stream_prepare_us` /
  `history_prepare_us` / `history_append_rows` / `stream_append_rows`.
- `history` stage gained `lines_build_us` (cell-line building, distinct from
  row rasterization `render_us`).
- `tui::perf::cell` — per-cell render >2ms logs `cell=tool_call:<name>` +
  `lines_added` + `duration_us`.
- `tui::streaming` — per-advance `append_rows_us` / `fingerprint_us` +
  finalize-verify `markdown_us` / `fingerprint_us` / `matched`.
- `tui::perf::init` — syntect set load + per-grammar prewarm timings.
- `begin_sync_update` stage (backpressure probe) and `draw_ms` in the redraw
  log (event-fold vs draw attribution).

### Accepted limitation

A full replay cannot clear rows that have already scrolled out of the owned
on-screen region — terminal scrollback above the screen is immutable, for any
implementation. Replay therefore duplicates that content. With the §6.1 fix
this is reachable only through genuine invalidation (width / theme / syntax /
display-mode / header changes), which are rare and user-initiated; the
long-term posture stays codex-aligned: scrollback is append-only, and replay
is the last resort, not a steady-state path.
