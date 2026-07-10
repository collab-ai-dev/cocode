# P1-3 — Grep Content-Mode Densification (≥5 Matches → Path-Grouped)

Status: not started · Size: S · Owner crate: `coco-tools`

## Problem

coco's Grep content mode emits one line per match with the path repeated
on every hit: `format!("{}{sep}{}{sep}{}", file_path, line_number,
line_content)` → `path:line:content`
(`core/tools/src/tools/grep.rs:888-912`, `format_content`). On deep-path
monorepos the repeated path dominates the payload — 20-40% of the bytes
for typical searches.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.6.19 (v0.17.0) #47866. All in `tools/file_operations.py`:

- `_DENSIFY_MIN_MATCHES: ClassVar[int] = 5` (:251).
- `_densify_matches` (:253-279); returns `None` below the threshold
  (:265) → verbose form kept for small result sets.
- **Format** (:272-278):

  ```python
  for m in self.matches:
      if m.path != current_path:
          lines.append(m.path)
          current_path = m.path
      lines.append(f"  {m.line_number}: {m.content.rstrip()}")
  ```

- **Lossless** (docstring :258-260): "groups consecutive matches by path
  (path printed once, then `  <line>: <content>` rows), which is
  lossless — every path, line number, and content byte is preserved".
  Exploits ripgrep's path-ordered output.
- **Self-describing output**: `to_dict(densify=…)` (:281-297) emits a
  `matches_format` key describing the grouped shape so the model never
  has to guess the format.
- Context: hermes ran a "headroom evaluation" first (release note) and
  concluded this was the **only** output densification worth shipping —
  don't invent additional compression here.

## Design

1. In `format_content` (`grep.rs`), when `output_mode == Content` and
   total matches ≥ `DENSIFY_MIN_MATCHES` (const `5`, mirror hermes):
   group consecutive same-path lines — path once on its own line, then
   `  {line}: {content}` rows (2-space indent). Below 5 matches, keep
   today's verbose form byte-for-byte.
2. **Scope guard**: densify only plain match output. When context lines
   are requested (`-B`/`-A`/`-C` > 0), keep the verbose form in v1 —
   hermes handles context rows with a `-` separator, but coco's context
   lines interleave with match lines (`path-line-content`,
   `grep.rs:894-897`) and grouping them correctly is where the bugs
   live. Revisit only with evidence.
3. Prepend one header line when densified (the self-describing
   `matches_format` analog), e.g.:
   `Found {n} matches (grouped by file; "  <line>: <content>" rows)` —
   adjacent to the existing result framing, so downstream parsers and
   the model both know the shape.
4. `show_line_numbers=false` variant: grouped rows become
   `  {content}` — same rule as today's `path:content` form.
5. No config knob; behavior is deterministic on match count.

## Implementation steps

1. Restructure `format_content` to iterate (path, line, content) tuples
   it already has; add the grouped writer.
2. Update/extend the tool's format tests + any TUI renderer snapshot
   that displays grep results (check `app/tui` grep presentation — the
   TUI renders the tool result string as-is, so snapshots may shift).
3. `just test-crate coco-tools`; `cargo insta` cycle if TUI snapshots
   are touched.

## Tests

- 4 matches → verbose (unchanged, byte-identical to today).
- 5 matches across 2 files → grouped: each path once, indented rows,
  header line present.
- Reconstruction property: parse the grouped output back into
  (path, line, content) triples and assert equality with the verbose
  parse — this is the "lossless" claim as a test.
- Context-lines requested → verbose form regardless of count.
- Paths containing `:` and CJK content → grouping unaffected (grouping
  keys on the tuple, not on re-parsing the formatted line).

## Risks / non-goals

- Model familiarity: grouped grep output is common (ripgrep's own
  `--heading` default in terminals), and the header line removes
  ambiguity. Low risk.
- Non-goals: densifying `files_with_matches`/`count` modes (already
  dense); any lossy truncation (owned by the result-budget system);
  densifying other tools' outputs.
