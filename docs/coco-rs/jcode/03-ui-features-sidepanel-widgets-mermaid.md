# UI Features: Side Panels, Info Widgets, Mermaid, Inline Images: jcode vs coco-rs

Scope: four TUI capabilities where the two harnesses diverge most visibly — a
persistent agent-writable side panel, negative-space status widgets, an inline
Mermaid diagram renderer, and inline terminal image output. All claims below are
verified against source on both sides with `file:line` citations. jcode is an
independent lineage; coco-rs faithfully mirrors Anthropic's Claude Code TUI
(single-column transcript). A difference is judged on engineering merit for
coco-rs's stated goals, not treated as an automatic deficiency.

---

## jcode approach

jcode's TUI is built around four distinct, source-backed mechanisms.

### A. Info widgets packed into negative space

The headline UX idea: render the chat transcript first, then measure the
*unused* horizontal space on each visible row and bin-pack status widgets into
those empty rectangles so they never displace conversation text.

- **Per-row free-width measurement.** `src/tui/ui.rs:1483 line_left_margins_for_area()`
  computes `area_width - line.width()` for each rendered `Line`, honoring
  `Alignment::{Left,Center,Right}`, producing a `Vec<u16>` of free widths indexed
  by row.
- **Margins → 2-D bin-packer.** Those vectors become `Margins { right_widths,
  left_widths, centered }` (`src/tui/info_widget_layout.rs:18-26`; in centered mode
  both margins populate, in left-aligned only the right). `calculate_placements()`
  (`info_widget_layout.rs:39`) scans contiguous runs where `width >=
  MIN_WIDGET_WIDTH(24)` and `height >= MIN_WIDGET_HEIGHT(5)` via
  `find_all_empty_rects()` (`info_widget_layout.rs:240`), clamps to
  `MAX_WIDGET_WIDTH(40)`, and greedily places widgets, scoring for the widget's
  `preferred_side()` and smallest leftover area.
- **Anti-flicker.** Phase 1 keeps previously-placed widgets stable with a
  `STICKY_WIDTH_TOLERANCE(4)` hysteresis (`info_widget_layout.rs:15`, `still_fits`
  check at `:105-107`); Phase 2 fills the rest. This explicit machinery exists
  *because* margin-packing is prone to widgets jumping sides as content reflows.
- **Widget catalog.** `WidgetKind` (`info_widget.rs:73-101`) is a full multi-widget
  system: Overview, WorkspaceMap, Todos, ContextUsage, MemoryActivity, SwarmStatus,
  BackgroundTasks, Compaction, UsageLimits, KvCache, ModelInfo, Diagrams,
  AmbientMode, Tips, GitStatus. Each carries `priority()`, `preferred_side()`,
  `min_height()`. `Diagrams` is the *highest* priority (`= 0`, info_widget.rs:108);
  most status widgets land on `Side::Right` (info_widget.rs:129-150). An `Overview`
  widget *merges* small cards (`is_overview_mergeable`) into one paged box that
  cycles sub-views every `PAGE_SWITCH_SECONDS=30`. `InfoWidgetData` (info_widget.rs)
  is a wide struct fed from session state — todos, context, 5h/7d/Spark
  subscription windows, per-turn KV-cache miss attribution, memory graph
  nodes/edges, git, swarm, ambient. `MemoryActivity` even renders a force-directed
  memory graph (`info_widget_graph.rs`). Sub-renderers live across
  `info_widget_{git,model,usage,todos,memory_render,swarm_background,tips}.rs`
  (≈66 KB `info_widget.rs` + a dedicated layout solver).

### B. Side panel: persistent, agent-writable, file-linked, exposed as a tool

- **Tool surface.** `src/tool/mod.rs:170-171` registers tool name `"side_panel" =>
  SidePanelTool::new`, so the *model itself* can write/append markdown pages, link a
  live on-disk file, focus/delete pages, and use the panel as a diff viewer.
- **Storage.** State is JSON-persisted under
  `~/.jcode/side_panel/<session_id>/index.json` (`session_dir` side_panel.rs:314,
  `state_file` :319), with one `<page_id>.md` per page, via
  `crate::storage::read_json`/`write_json_fast`.
- **Three page sources** (`jcode-side-panel-types/src/lib.rs`, a dedicated 93-LoC
  types crate defining the persisted schema): `Managed` (agent writes via
  `write_markdown_page`/`append_markdown_page`, side_panel.rs:15/35); `LinkedFile`
  (`load_markdown_file` links an on-disk `.md` and auto-refreshes when a content-hash
  revision changes — `linked_file_revision()` hashes path+len+mtime+readonly,
  side_panel.rs:374; refresh logic `:73-100`); and `Ephemeral`.
- **Hardening.** `validate_page_id` (side_panel.rs:323) rejects `..`, separators,
  non-ASCII, and caps length; `validate_markdown_source_path` restricts to
  `.md/.markdown/.mdown/.mkd/.mkdn`.
- **Rendering.** A dedicated pinned pane renders the focused page's markdown
  (`render_side_panel_markdown_cached_with_zoom`, ui_pinned.rs), with
  image/diagram area estimation in `ui_pinned_layout.rs`.

### C. Mermaid renderer: pure-Rust, browser-free — but NOT in the shipped default build

The pipeline is genuinely browser-free:

1. **Parse + layout + SVG.** `crates/jcode-tui-mermaid/Cargo.toml` depends on
   `mermaid-rs-renderer` (git tag `v0.2.1`, the author's own crate). `lib.rs:29-52`
   wires `parse_mermaid` → `compute_layout` → `render_svg` — Mermaid AST → layout
   → SVG, all in Rust.
2. **SVG → PNG.** `mermaid_svg.rs:265 write_output_png_cached_fonts()` uses
   `usvg::Tree::from_str` + `resvg::render` + `tiny_skia::Pixmap.save_png` with a
   cached font DB. No headless Chromium, no Node.
3. **PNG → terminal.** `ratatui-image` `StatefulProtocol` auto-detects
   Kitty/Sixel/iTerm2/halfblock (`lib.rs:48-52`).

Supporting machinery is substantial: adaptive PNG sizing from terminal width +
diagram complexity (`mermaid_svg.rs:40 calculate_render_size`), a disk+memory
**artifact cache keyed by content hash** (`mermaid_cache_render.rs`, ≈31 KB) — the
expensive cache sits at the PNG level, the right placement — async deferred
rendering with dedupe, and `DiagramDisplayMode::{None,Margin,Pinned}`
(`src/config.rs:8`) so a diagram renders either as the highest-priority `Diagrams`
info widget in the margin, or in a pinned pane.

**Critical caveat — it is not on by default.** The renderer is behind Cargo feature
`renderer` with `default = []` (jcode-tui-mermaid/Cargo.toml:8-9). The markdown
crate's `mermaid-renderer` feature is also `default = []` (jcode-tui-markdown/Cargo.toml:18-19),
and a workspace grep shows it is **only ever defined, never activated** by any
member. The root binary `jcode` declares `default = ["pdf", "embeddings"]` (root
Cargo.toml:227) — mermaid is absent. The Cargo.toml comment even notes the size API
needs `JCODE_MMDR_SIZE_API_AVAILABLE=1` plus a Cargo patch because the dep isn't
bumped. So the *shipped default* jcode build runs the fallback
(`markdown_mermaid_fallback.rs`, "Mermaid rendering is disabled"). jcode "does it
in source" but does **not** ship it on by default.

A forward-looking ADR (`docs/MERMAID_RENDERING_REDESIGN.md`) candidly states the
current path is "a state hub" coupling render+cache+placement+active-state through
globals/thread-locals, and proposes a cleaner staged pipeline it has not shipped.

### D. Inline images: hand-rolled terminal graphics protocols

`src/tui/image.rs` (≈444 LoC) implements three protocols directly with raw escape
sequences:

- **Kitty:** `\x1b_Ga=T,f=100,c=<cols>,r=<rows>,m=<more>;<b64>\x1b\\` chunked at
  4096 bytes (image.rs:271-327).
- **iTerm2:** `\x1b]1337;File=...;inline=1;width=<cols>:<b64>\x07` (image.rs:330).
- **Sixel:** shells out to ImageMagick `convert ... sixel:-` (image.rs:373), gated
  on a `HAS_IMAGEMAGICK` probe.

Protocol auto-detection via `KITTY_WINDOW_ID`/`TERM`/`TERM_PROGRAM`/`LC_TERMINAL`
(image.rs:37). PNG/JPEG/GIF/WebP dimensions are parsed by hand from headers
(`get_image_dimensions`, image.rs:174) for aspect-correct cell sizing. Crucially,
`src/tui/generated_image.rs` wires AI-generated images into a side-panel card with a
metadata summary (provider, revised prompt, dimensions, byte count) — so jcode has a
real terminal-image *output* path for model-returned images, not just diagrams.

### E. Markdown highlighting with a syntect cache + incremental streaming renderer

`crates/jcode-tui-markdown` (≈5.5K LoC) highlights code with **real syntect**:
`highlight_code()` (markdown_render_support.rs:202-211) builds
`HighlightLines::new(syntax, theme)` over `SYNTAX_SET`, with the dependency declared
as `syntect = { features = ["default-syntaxes", "default-themes", "regex-fancy"] }`
(jcode-tui-markdown/Cargo.toml:14). Because that path is genuinely expensive, jcode
wraps it in a content-hash cache: `HighlightCache { entries: HashMap<u64,
Vec<Line<'static>>> }` (lib.rs:347), `hash_code(code, lang)` (lib.rs:371), bounded
clear-on-full eviction at `HIGHLIGHT_CACHE_LIMIT` (lib.rs:362-366). The hit site
`highlight_code_cached()` (markdown_render_support.rs:174-199) clones cached lines on
a hit.

Separately — and this is the *actually* load-bearing streaming optimization — jcode
ships a true **incremental** renderer: `IncrementalMarkdownRenderer`
(markdown_incremental.rs:3) tracks a `last_checkpoint` "after complete block"
(`:8-13`), detects pure appends, and re-renders only text after the last completed
block (`:33-35`). It also exposes a queryable render-memory profiler
(`debug_memory_profile`, lib.rs) counting cache entries/lines/spans/text-bytes.

---

## coco-rs approach

coco-rs's TUI (`coco-rs/app/tui`) is a ratatui + Elm/TEA single-binary terminal UI
mirroring Claude Code's Ink/React UI. On the four module concerns:

### Side panels — not present (deliberately)

The documented layout (`docs/coco-rs/crate-coco-tui.md:408-425`) is a single
full-screen vertical stack: `header / main conversation / inline live activity
surface / input + queued-command area / status bar`, with modals/toasts overlaid.
The doc explicitly records that even the *right-side tool execution list was removed*
(`:421`) "to keep the transcript as the primary surface and avoid a second transient
projection of tool state" — a conscious move *away* from side projections.
`presentation/layout.rs` provides only centered-modal math (`centered_modal_area`
`:42`, `ModalBounds` `:12`), not a horizontal split-pane. There is no persistent,
agent-writable, or file-linked auxiliary panel, and no side-panel tool (grep over
`tools/src` returns 0). No `~/.coco/<session>/` side-panel storage path exists.

### Info widgets in negative space — not present (different layout model)

coco-rs has rich status surfaces, but they are *inline rows in the vertical stack*,
not margin-packed negative-space cards:

- `widgets/activity_panel.rs:35-62` renders a unified live-activity `Paragraph`
  (bordered `Block` with `Borders::TOP`) directly above the composer, resolved by
  `presentation::activity::TurnActivityView`.
- `widgets/{todo_panel, tool_panel, background_pills, queue_status_widget,
  status_indicator, settings_panel}.rs` are conventional bordered/inline widgets.

There is no per-row free-width measurement, no 2-D bin-packer, no `Side::{Left,Right}`
placement, no overview-merge, and no "get out of the way when there's no room"
behavior. The transcript width is the full frame width. Notably, the activity band is
**already 0-height when idle**: `inline_activity_height()`
(presentation/activity.rs:154-167) returns 0 for `TurnActivityView::None` or an empty
surface, and is row-budget-capped by width (`activity_row_budget`, `:181-189`). It
consumes rows only while activity/todos/queue are actively populated.

### Mermaid / inline diagrams — not present

Zero references to `mermaid`, `resvg`, `usvg`, `tiny_skia`, or `ratatui-image`
anywhere in coco-rs (verified grep `--include=*.rs --include=*.toml` returns **0**).
Fenced code blocks (including ` ```mermaid `) render as highlighted text in the
markdown widget. No SVG/PNG rendering, no diagram pane.

### Inline images — not present (send-only)

Image support is strictly *input*: `paste.rs::ImageData` / `clipboard.rs` capture
pasted/clipboard images and attach them to the user message sent to the model.
They are never rendered into the terminal; in the transcript a
`CellKind::UserAttachment` renders as a bare paperclip text placeholder
(widgets/chat/render_user.rs). Terminal graphics escape sequences exist only for
**desktop notifications** (`widgets/notification.rs`: iTerm2 OSC 9;1, Kitty OSC 99,
Ghostty OSC 777, plus tmux/screen and terminal-bell fallbacks), not image display.
So Kitty/iTerm2 are detected for *notifications*, not for inline image/diagram
rendering — a scope decision, and the notification path itself is high-quality.

### Markdown rendering (the one overlapping area) + native scrollback

`widgets/markdown.rs` + `widgets/chat/*` render markdown from the
engine-authoritative `&[RenderedCell]` derived view (`state/derive.rs
message_to_cells`, transcript invariant I-2). Two facts reshape the cost picture:

1. **No syntect.** coco-rs highlighting is `highlight_code_line()`
   (widgets/markdown.rs:500-573) — a hand-rolled single-pass O(n)-per-line char
   scanner over a small per-language keyword set. There is no `SyntaxSet`/`Theme`
   load, no regex engine, no oniguruma (grep for `syntect` in any Cargo.toml: **0**).
   It is far cheaper per call than jcode's syntect path. Highlighting is a toggle
   (`DisplaySettings SyntaxHighlighting`, `TuiCommand::ToggleSyntaxHighlighting`).
2. **Native terminal scrollback is the production surface.** Finalized cells are
   emitted exactly once — `render_finalized_history_lines(&cells[start..], options)`
   then `terminal.insert_history_lines(lines)` (surface/history_driver.rs:117-118) —
   into the terminal's own scrollback, with emitted UUIDs tracked
   (history_emitter, `mark_emitted_through` history_driver.rs:120). Scrolling does
   **not** re-render or re-highlight (the terminal owns scrollback). Full
   re-highlight (`replay_all_capped`, history_driver.rs:160-172) fires only on a
   `ReplayRequired` event — width change, viewport change, theme change, or
   header-fingerprint change.

A deliberate non-goal is documented: `state/transcript_view.rs:211-217` states
"field-level rendering data (markdown AST cache, diff hunks, etc.) is **not** stored
here per layer-hygiene rule." Per-cell `cached_lines`/`cached_height` exist for
layout and a `reasoning_metadata` side-cache exists, but there is no
content-hash *highlight* cache — and, given native scrollback, the only place
highlighting is recomputed per draw is the **live streaming assistant cell**
(`build_lines` → `build_lines_owned`, widgets/chat/mod.rs:148-168, called from the
interactive viewport surface/viewport.rs:368).

**Net:** of the four module concerns, coco-rs implements none of side-panels /
negative-space widgets / mermaid / inline images. This is consistent with its goal
(faithful Claude Code parity); upstream Claude Code's TUI is also a single-column
transcript without these features. coco-rs's inline activity/todo/tool panels cover
*some* of the same information needs as jcode's info widgets, but via a fundamentally
different layout model.

---

## Head-to-head comparison

| Concern | jcode | coco-rs | Verdict |
|---|---|---|---|
| Negative-space info widgets | 15-kind `WidgetKind` system + 2-D bin-packer into measured margins, sticky hysteresis, overview-merge (ui.rs:1483, info_widget_layout.rs:39) | Inline rows in a vertical stack; 0-height when idle (activity_panel.rs, activity.rs:154) | jcode strictly more capable on wide screens; degrades to *nothing* when margins collapse |
| Persistent agent-writable side panel | `SidePanelTool` (tool/mod.rs:170) + per-session JSON+`.md` storage + LinkedFile auto-refresh (side_panel.rs:374) | None; right-side panel deliberately removed (crate-coco-tui.md:421) | jcode has a capability coco-rs lacks entirely |
| Inline Mermaid | Pure-Rust parse→SVG→PNG→terminal pipeline **gated behind a non-default Cargo feature, not shipped** (Cargo.toml:8-9, root default `["pdf","embeddings"]`) | Renders ` ```mermaid ` as text | jcode has the mechanism in source; **neither ships it by default** |
| Inline image output | Kitty/iTerm2/Sixel hand-rolled + AI-image side-panel cards (image.rs, generated_image.rs) | Input-only; placeholder in transcript; escapes used only for notifications | jcode can *show* images; coco-rs has no output path |
| Markdown highlight cost | Real syntect + content-hash cache + incremental renderer (Cargo.toml:14, lib.rs:347, markdown_incremental.rs) | Hand-rolled keyword scanner, no cache; native scrollback gives O(1) on all finalized/scrolled content (markdown.rs:500, history_driver.rs:117) | Different cost models — see Optimizations |

**1. Negative-space info widgets — jcode strictly more capable on wide terminals.**
jcode shows live status (todos, context, subscription windows, KV-cache hit ratio
with miss attribution, memory graph, git, swarm, background tasks) without consuming
any transcript rows, by bin-packing into measured empty margins. coco-rs shows a
subset, always in dedicated rows above the composer. Perf trade: jcode pays a
per-frame O(rows) margin scan + greedy placement (cheap integer math, sticky-cached)
in exchange for zero vertical cost; coco-rs pays vertical space but no placement
compute. On wide terminals with short lines, jcode's information density is materially
higher. The asymmetry: jcode's advantage *inverts* on narrow terminals or when long
lines/code blocks fill the width — the margins collapse below `MIN_WIDGET_WIDTH=24`
and most widgets simply don't render, while coco-rs's inline rows still show status.

**2. Persistent agent-writable side panel — a genuine jcode product feature.**
The `SidePanelTool` lets the model maintain a running design doc / plan / dashboard
beside the chat, persisted per session and cheap to hydrate (tiny JSON index +
per-page `.md`). coco-rs has no equivalent surface or tool.

**3. Inline Mermaid — jcode has the mechanism; neither ships it.** jcode's
browser-free pipeline is real and verified (jcode-tui-mermaid/Cargo.toml,
lib.rs:29-52, mermaid_svg.rs:265). But it is behind a non-default Cargo feature never
activated by any workspace member, and the shipped `jcode` binary's default features
are `["pdf", "embeddings"]`. So in default builds *both* harnesses render ` ```mermaid `
as text. This is a scope expansion beyond both, not a feature jcode ships and coco-rs
lacks.

**4. Inline images — jcode renders, coco-rs sends.** jcode emits Kitty/iTerm2/Sixel
directly and surfaces AI-generated images. coco-rs's image path is input-only.

**5. Markdown highlight cost — different models, and the naive cache port is wrong.**
jcode's syntect cache is load-bearing *because* syntect is expensive. coco-rs uses a
flat keyword scanner *and* native scrollback, so finalized and scrolled content is
already O(1) (re-highlight fires only on resize/theme/viewport events — exactly the
events a content-hash cache would have to invalidate on). The only residual cost is
re-highlighting the *live* streaming cell each tick. See Optimizations for why an
incremental live-cell renderer, not a global hash cache, is the right fit.

**Resource framing.** jcode's UI features are largely *additive cost*. The mermaid
crate alone pulls in `resvg`/`usvg`/`tiny_skia`/`image`/`ratatui-image`
(jcode-tui-mermaid/Cargo.toml:19-27); rendering allocates PNG pixmaps and
image-protocol buffers; the side panel adds disk I/O; syntect pulls a regex engine.
coco-rs's leaner surface is part of why a Claude-Code-faithful TUI stays simple.

---

## Where coco-rs already matches or wins

**1. coco-rs's transcript-authority model is cleaner than jcode's render globals.**
coco-rs pins three invariants (`app/tui/CLAUDE.md`): I-1 `MessageHistory` is the
single source of truth (mutations emit explicit events), I-2 `TranscriptView.cells`
is a *pure derivation* from `&Message` (`derive::message_to_cells`), I-3 UI-only
state stays UI-only. This is precisely the discipline jcode's own ADR
(`docs/MERMAID_RENDERING_REDESIGN.md`) admits jcode lacks — it describes the current
mermaid path as "a state hub" where active diagrams are registered as a side effect
of render calls, cache keys depend on a thread-local aspect-ratio context, and
deferred rendering carries its own global dedupe/epoch queue. On rendering-pipeline
hygiene, coco-rs is ahead: jcode is fast but admits it is tangled and proposes an
unshipped redesign.

**2. coco-rs avoids a class of correctness hazards jcode carries.** Because coco-rs
renders status as deterministic inline rows derived from state, it has no
widget-placement flicker/oscillation problem. jcode needs explicit anti-flicker
machinery — `STICKY_WIDTH_TOLERANCE=4` and Phase-1 "keep prior placements"
(info_widget_layout.rs:89-157) — precisely because margin-packing makes widgets jump
sides as content reflows. That is complexity coco-rs doesn't pay.

**3. The negative-space "get out of the way" claim is true but situational, not a
strict win.** jcode's own constants gate widgets to `MIN_WIDGET_WIDTH=24`,
`MIN_WIDGET_HEIGHT=5`, `MAX_WIDGET_WIDTH=40` (info_widget_layout.rs:9-13). On narrow
terminals or full-width content, the margins collapse and widgets don't render — the
same information coco-rs would still show inline. coco-rs's inline rows degrade more
gracefully (status stays visible); they are also already 0-height when idle
(activity.rs:154-167), so the "wasted vertical space" critique only bites during
active turns.

**4. Native scrollback already gives coco-rs jcode's main caching benefit for free.**
jcode's `HighlightCache` exists to skip re-highlighting unchanged blocks during
re-renders. coco-rs sidesteps the problem class: finalized cells are emitted once into
terminal scrollback (history_driver.rs:117) and scrolling never re-highlights. jcode's
cache is a remedy for a re-render model coco-rs doesn't have on its hot path.

**5. The "1800x faster" headline is an external, unverifiable-from-this-repo number.**
The mechanism (no Chromium) is real and credible, but the *multiplier* lives in the
separate `mermaid-rs-renderer` repo, not here, and compares cold puppeteer startup vs.
warm in-process render. Fair to credit jcode for "browser-free pure-Rust mermaid
rendering"; not fair to treat "1800x" as a load-bearing engineering fact — especially
since the feature is off in the default build.

**6. coco-rs's notification escape-sequence path is on par.** For the *notification*
use of terminal escapes, `widgets/notification.rs` (iTerm2 OSC 9;1, Kitty OSC 99 with
title+body+focus action, Ghostty OSC 777, tmux/screen + bell fallbacks) matches
jcode's protocol detection. coco-rs simply chose not to extend escape-sequence
rendering to images/diagrams — scope, not quality.

**Bottom line:** three of the four features (side panel, negative-space widgets,
inline images/diagrams) are outside coco-rs's Claude-Code-parity scope by design, and
coco-rs's rendering layer is architecturally cleaner than jcode's self-acknowledged
state coupling. coco-rs is not "behind" by accident.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions surviving adversarial review are listed. Each notes whether it is
parity or a scope expansion beyond Claude Code.

### R1 (from M03-S1, nuanced → narrowed): incremental live-cell markdown renderer, NOT a global highlight cache

- **Why (jcode mechanism + coco-rs gap):** jcode caches syntect output by
  `hash(code, lang)` (jcode-tui-markdown/lib.rs:347-377) and additionally ships a true
  incremental renderer that re-renders only text after the last completed block
  (`IncrementalMarkdownRenderer`, markdown_incremental.rs:3-35). The analyst's original
  proposal — a global static LRU keyed by `(code_hash, lang, theme_id)` — is **refuted
  as the primary mechanism** on two grounds. (a) *Wrong cost model:* coco-rs has no
  syntect; highlighting is a flat char scanner (`highlight_code_line`,
  widgets/markdown.rs:500-573), far cheaper than the syntect path the cache exists to
  skip. (b) *Wrong render model:* coco-rs uses native terminal scrollback — finalized
  cells emit once (history_driver.rs:117) and scrolling never re-highlights; full
  replay fires only on resize/viewport/theme (history_driver.rs:160), exactly the
  events a hash key would invalidate on. The `transcript_view.rs:211-217` "no markdown
  AST cache" note is about the derived-cell struct (I-2 hygiene), not per-frame
  highlighting. The **only** residual cost is the live streaming assistant cell, which
  re-runs `build_lines` (and `highlight_code_line` over the growing fenced block) every
  stream tick via the interactive viewport (widgets/chat/mod.rs:148-168,
  surface/viewport.rs:368).
- **Concrete change:** First profile streaming with the keyword highlighter hot. If it
  proves hot (unlikely vs syntect, since it is a flat loop), add an incremental live-cell
  renderer in `app/tui/widgets/chat` mirroring jcode's last-safe-checkpoint approach —
  re-highlight only the changed tail block of the live cell, leaving finalized scrollback
  untouched. Do **not** add a global static LRU; the append-only emit path already gives
  O(1) on all finalized/scrolled content. (The hash+theme_id LRU only becomes worthwhile
  if coco-rs ever adopts syntect.)
- **Impact:** low–medium. **Effort:** low (profiling-gated; incremental renderer is
  medium if pursued). **Risk:** low — must scope to the interactive viewport draw only,
  never the native-scrollback emit path, to preserve I-2 and append-only invariants.
- **Non-goals:** respected. Not a parity feature (Claude Code has no such cache); a
  pure-internal perf change.

### R2 (from M03-S2, nuanced): opt-in inline-diagram (Mermaid) renderer behind a Feature gate, in a Standalone-layer crate

- **Why (jcode mechanism + coco-rs gap):** jcode's browser-free pipeline is verified —
  `mermaid-rs-renderer` + `resvg`/`usvg`/`ratatui-image` (jcode-tui-mermaid/Cargo.toml),
  parse→layout→SVG (lib.rs:29-52), SVG→PNG with cached fonts (mermaid_svg.rs:265),
  protocol auto-detect (lib.rs:48-52). coco-rs has zero such deps (grep: 0). **Re-label
  honestly:** because the renderer is behind a non-default Cargo feature never activated
  by any jcode workspace member (Cargo.toml:8-9; root default `["pdf","embeddings"]`),
  this is a scope **expansion beyond *both* harnesses' default behavior**, not parity
  with something jcode ships.
- **Concrete change:** Add a new **Standalone-layer** crate (e.g. `coco-tui-mermaid`,
  sibling to `retrieval`/`bridge`) owning `resvg`/`usvg`/`ratatui-image`, gated behind a
  new closed `coco_types::Feature::Diagrams` defaulting **OFF**. Render into a dedicated
  overlay/pane reusing `centered_modal_area` (presentation/layout.rs:42), **not** the
  inline transcript. The overlay placement is not merely preferable but effectively
  *mandatory*: coco-rs's base surface is native terminal scrollback (line-text,
  re-emitted on replay), so inline image cells in the transcript are architecturally
  awkward. Detect protocol once at startup; fall back to a plain highlighted code block
  when unsupported. Design as a pure `RenderRequest → RenderArtifact` pipeline from day
  one — heed jcode's own ADR warning against the "state hub". Cache at the PNG-artifact
  level (jcode's better cache placement, mermaid_cache_render.rs / mermaid_svg.rs:265),
  not the text-line level.
- **Impact:** medium. **Effort:** high — heavy new deps grow binary size and cold-build
  time; PNG pixmaps allocate; the upstream renderer is un-stabilized (size API needs a
  Cargo patch). **Risk:** medium-high; needs explicit product buy-in.
- **Non-goals:** respected *as an explicit opt-in expansion*. It is beyond Claude-Code
  parity, so it must be isolated and off by default.

### R3 (from M03-S3, nuanced → scoped): optional single sticky right-margin status chip on wide terminals

- **Why (jcode mechanism + coco-rs gap):** jcode measures trailing free width per
  rendered line (`line_left_margins_for_area`, ui.rs:1483) and bin-packs status into the
  empty margins (info_widget_layout.rs:39, find_all_empty_rects :240) with sticky
  hysteresis (`STICKY_WIDTH_TOLERANCE=4`, :15). coco-rs renders status only in vertical
  rows (activity_panel.rs:35-62; single-column stack, crate-coco-tui.md:413-425).
  **Narrowed:** the "wasted height on wide terminals" critique is only partly true — the
  band is **already 0-height when idle** (`inline_activity_height` returns 0 for
  `None`/empty, activity.rs:154-167). The real win is reclaiming the few activity rows
  **during active turns** on wide terminals, not eliminating an always-present band.
- **Concrete change:** A presentation-layer-only helper that, above a width threshold and
  behind a **default-off** display setting (Claude-Code-faithful by default), measures
  trailing free width of already-rendered viewport lines (port the `line.width()` vs
  `area_width` idea from ui.rs:1483) and draws a **single** sticky right column
  (e.g. context % / todo count). Start with one column + sticky tolerance, **not** jcode's
  full 2-D packer. **Hard constraint:** this must touch only the interactive viewport draw
  (`render_interactive_viewport`), never the native-scrollback emit path
  (history_driver.rs) — finalized lines live in the terminal's own scrollback and cannot
  carry live margin chips; doing otherwise breaks I-2 and the append-only invariant.
- **Impact:** low. **Effort:** medium. **Risk:** medium — negative-space rendering is
  flicker-prone (jcode needed Phase-1 sticky logic + tolerance); start minimal and behind
  a setting. Diverges from strict Claude Code TUI parity, so it must be opt-in.
- **Non-goals:** acceptable as opt-in; conflicts with strict single-column parity if ever
  made default, so it must not be.

### R4 (from M03-S4, confirmed): persistent agent-writable scratch/notes panel as a tool (overlay-rendered)

- **Why (jcode mechanism + coco-rs gap):** jcode registers `side_panel` →
  `SidePanelTool` (tool/mod.rs:170) implementing write/append/load-file with LinkedFile
  auto-refresh (side_panel.rs:15/35, linked_file_revision :374), persisted under
  `~/.jcode/side_panel/<session>/{index.json,<page>.md}` (:314/:319), path-hardened
  (validate_page_id :323). coco-rs has no notes/scratch/side-panel tool (grep over
  `tools/src`: 0), no auxiliary panel, and deliberately removed the right-side tool list
  (crate-coco-tui.md:421). Both halves of the gap hold; this is an OPTIONAL expansion, not
  a coco-rs deficiency against its stated goals.
- **Concrete change:** Add a coco-tool that writes/appends a per-session markdown notes
  document (typed schema in `coco-types` or the tool crate, `snake_case` serde, `Option`
  fields per type-safety rules), surfaced via a **toggleable full-screen overlay** reusing
  `centered_modal_area` (presentation/layout.rs:42) — **not** a split pane (avoids the
  layout-split + pinned-pane complexity jcode carries and keeps native scrollback
  untouched). Reuse `utils/file-encoding` + `frontmatter`; harden page ids exactly like
  jcode's `validate_page_id`. Gate behind a new closed `Feature` variant, default OFF.
  Two coco-rs-specific must-dos: **(a)** route persistence through `coco-session` (it owns
  `~/.coco/<session>` artifacts) rather than a fresh ad-hoc path; **(b)** run the new tool
  through the standard permission pipeline — it is a filesystem-writing tool.
- **Impact:** low (product feature, situational value). **Effort:** high — new persisted
  artifact + new tool surface (permissions, hooks). **Risk:** medium; it is a product
  expansion, not parity, so it needs explicit product buy-in.
- **Non-goals:** respected as an explicit opt-in; it expands beyond Claude-Code parity, so
  default-off + product buy-in are required.

### R5 (from verifier missed-finding): expose a TUI render-memory profiler to validate RAM claims

- **Why (jcode mechanism + coco-rs gap):** jcode ships a queryable
  markdown/highlight memory profiler (`debug_memory_profile`, jcode-tui-markdown/lib.rs,
  counting cache entries/lines/spans/text-bytes/estimate-bytes) plus a side-panel memory
  profile. coco-rs has no equivalent introspection of TUI render-memory. Given coco-rs's
  own performance posture, lightweight render-memory counters would let it validate its
  RAM behavior empirically rather than by assumption.
- **Concrete change:** Add an opt-in (debug-gated) counter in `app/tui` reporting cached
  line/span counts and approximate bytes for the derived-cell `cached_lines` and any
  live-cell buffers, exposed via an existing diagnostics path (e.g. Doctor overlay) — no
  new public lib API, no transcript-invariant impact.
- **Impact:** low. **Effort:** low. **Risk:** low. **Non-goals:** respected — internal
  diagnostics only.

---

## Rejected after adversarial review

- **M03-S1 as originally framed — "add a global content-hash syntax-highlight LRU because
  coco-rs re-highlights on every derivation and on scroll."** **Refuted.** The cocors-gap
  rests on a wrong cost model and a wrong render model. (a) coco-rs has no syntect — grep
  for `syntect` in any Cargo.toml returns 0; highlighting is a hand-rolled flat char
  scanner (`highlight_code_line`, widgets/markdown.rs:500-573), so a content-hash cache
  wins far less than it does over jcode's syntect path. (b) coco-rs's production surface is
  native terminal scrollback: finalized cells emit once
  (`render_finalized_history_lines(&cells[start..])` → `insert_history_lines`,
  history_driver.rs:117-118), scrolling never re-highlights, and full re-highlight fires
  only on width/viewport/theme/header changes (history_driver.rs:160) — exactly the events
  a `(code_hash, lang, theme_id)` key would have to be invalidated on, so the cache would
  mostly *miss* precisely when it would run. The surviving, narrowed need (live-cell
  streaming re-render) is addressed by R1 (incremental renderer), not a global LRU.

