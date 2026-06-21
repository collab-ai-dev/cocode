# coco-tui-markdown

Grammar-accurate markdown rendering for the coco TUI: CommonMark + GFM via
`pulldown-cmark` 0.13.x, fenced code highlighted by `syntect`, emitted as owned
`Vec<Line<'static>>` for the native-scrollback engine.

Tier-2 leaf (thiserror/error-free; **no** `coco-error`/`snafu`/public-`anyhow`).
Depends UP on `coco-tui-ui` for the shared `Theme` / `UiStyles` /
`SyntaxHighlighting` types — `coco-tui-ui` must never depend back.

## Key API

- `render_markdown(text, MarkdownOptions, Option<&LeadMarker>) -> Vec<Line<'static>>`
  — the entry point. `LeadMarker` makes the assistant turn glyph (`⏺`) a
  first-class input applied structurally on the first line; **never** a
  caller-side first-span string-match.
  For a marker-free render, pass `None`: `render_markdown(text, MarkdownOptions::new(styles, width, syntax), None)`.

## Invariants

- **Output contract**: logical prose lines with a `body_indent`-column left
  margin; prose wraps downstream at paint time (`Paragraph::wrap`). Code fences
  wrap internally because their gutter frame must stay within
  `MarkdownOptions.width`.
- **Colors come only from `UiStyles`.** syntect token *scopes* are classified
  (`highlight.rs`) and mapped onto `code_*` theme tokens — syntect's `.tmTheme`
  palette is dropped at the dependency level so highlighting follows live theme
  switches + capability downsampling.
- **Block boundaries flush pending inline content** (`block_gap` / `emit_raw_line`)
  so a tight list item's text followed by a nested block, or a nested list's
  parent text, is never dropped or merged. Regression-pinned in `lib.test.rs`.
- syntect's `SyntaxSet` is built once via an immutable `OnceLock`; a size guard
  (512 KB / 10k lines) falls back to plain text on oversized input.

## Mermaid feature

`[features] mermaid = ["dep:coco-tui-mermaid"]`, **default-off**. When enabled, a
` ```mermaid ` fence is rendered to box-drawing cells via `coco-tui-mermaid`;
on `None` (unsupported/illegible) or with the feature off it falls back to the
verbatim code fence. `app/tui` opts in.

## Modules

`lib.rs` (Writer + public API), `highlight.rs` (syntect scope→token), plus
companion `*.test.rs`.
