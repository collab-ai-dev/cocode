# coco-tui-mermaid

Renders Mermaid diagrams as Unicode **box-drawing cells** (not images). Reuses
`mermaid-rs-renderer`'s pure-Rust `parse_mermaid` → `compute_layout` half and
projects the geometric `Layout` onto a character grid — **no rasterization, no
terminal graphics protocol** — so output is ordinary `Vec<Line<'static>>` that
flows through native scrollback like any other text.

Tier-2 leaf (error-free). Depends UP on `coco-tui-ui` (for `UiStyles`) +
`ratatui` + `unicode-width` + the `mermaid-rs-renderer` git dep
(`default-features = false` → drops `cli`/clap and `png`/resvg/usvg/tiny-skia;
git rev pinned in the workspace manifest).

## Key API

`mermaid_to_lines(src, styles, width) -> Option<Vec<Line<'static>>>`. `None`
signals the caller to render the verbatim code fence.

## Invariants

- **Scope**: box-and-arrow graphs (`flowchart` / `classDiagram` /
  `stateDiagram` / `erDiagram` — all `DiagramData::Graph`). Sequence, chart
  types, parse errors, empty graphs, and layouts too dense or tall for the width
  return `None`. The verbatim fence is the universal fallback — **never worse
  than rendering the source**.
- **Never panics.** `compute_layout` reaches third-party ranking/routing code
  with internal unwrap/index sites, so the whole parse+layout+emit is wrapped in
  `catch_unwind` returning `None` on panic.
- **No graphics protocol / no alt-screen.** This is the whole reason for the
  cell approach — it composes with the cell-diff + scroll-region scrollback
  engine, unlike rasterized images.

## How it works

`grid.rs` is a styled `CellGrid`: box-drawing strokes merge at junctions
(`box_mask`/`mask_to_box`), wide glyphs reserve a shadow cell, trailing blanks
are trimmed. `lib.rs` quantizes float geometry to the grid (aspect-preserving,
height-capped at `MAX_ROWS`, with a density guard), draws rounded node boxes +
centered labels, routes edges as orthogonal strokes with explicit corner glyphs
(`╭╮╰╯`, not a merged `┼`) and arrowheads, and renders subgraph containers.
