# P0-2 — Loop Robustness: Empty-Response Nudge · Edit Closest-Match · Bash ANSI Strip

Status: not started · Size: S (three independent fixes; may split into
separate commits) · Owner crates: `coco-query` (a), `coco-tools` (b, c),
`coco-utils-string` (c helper)

---

## (a) Empty-response nudge/retry

### Problem

A clean empty model response (no text, no tool calls) simply ends the
turn: `app/query/src/engine.rs:1060-1083` routes to
`handle_no_tool_calls_terminal` (`engine_terminal.rs:214`), which logs
"no tool calls, conversation complete" and returns `end_turn`
(:368-395). Thinking-only responses are not special-cased —
`reasoning_text` is dropped after the loop
(`engine_stream_consume.rs:599-601`). The only retry path today is
structured-output mode (`engine_terminal.rs:227-260`). Weak models
(OpenAI-compatible tail of coco's provider matrix) routinely emit empty
or reasoning-only responses mid-task.

### Hermes evidence (hermes-agent @ `a7f65e3bc`)

`agent/conversation_loop.py` — releases v2026.4.13 #6488, v2026.4.8
#5278/#5931, v2026.4.23 #10472:

- **Post-tool one-shot nudge** (:4853-4918, "the #9400 case"): fires
  when a `role=="tool"` message is among the last 5 (:4866-4869),
  one-shot per turn. Appends a synthetic assistant `"(empty)"` (to keep
  the message sequence valid) then a plain user nudge (:4906-4917):

  ```python
  "You just executed tool calls but returned an empty response. Please
  process the tool results above and continue with the task."
  ```

- **Thinking-only prefill continuation** (:4920-4952): structured
  reasoning present but no content → continue with the thinking
  prefilled, capped at 2 attempts.
- **Generic retry cap = 3** (:4954-4981):
  `agent._empty_content_retries < 3 … continue`; after exhaustion, a
  fallback-provider attempt (:4989-5013), then terminal `"(empty)"`
  sentinel (:5021-5060).
- **Scaffolding hygiene**: nudge/sentinel messages carry
  `_empty_recovery_synthetic` flags, are popped before a real final
  answer (:5120-5129) and scrubbed from persisted sessions
  (`run_agent.py:1690`).

### Design

In `handle_no_tool_calls_terminal`:

1. Detect *truly empty*: no non-whitespace text content AND no tool
   calls AND a clean stop reason (abnormal stops — MaxTokens /
   ContentFilter / ContextWindow — already have recovery in
   `engine_recovery.rs`; don't touch).
2. Retry with a nudge instead of ending the turn, tracked by
   `empty_response_retries: i32` on the turn state, cap **3**. Nudge is
   injected as a user-role message through the existing continuation
   seam (`ContinueReason`); text varies by context:
   - tool results present in this turn → hermes's post-tool wording
     (verbatim above);
   - otherwise → "You returned an empty response. Please respond to the
     request above."
3. Mark nudge messages with the existing meta/virtual message flags so
   they are filtered from the API payload after the turn resolves and
   never rendered to the user (coco's `is_meta`/`is_virtual` machinery
   replaces hermes's `_empty_recovery_synthetic` scrubbing).
4. Thinking-only responses (reasoning present, content empty): count as
   empty and nudge, but do NOT attempt hermes's provider-level thinking
   prefill continuation — that is a `vercel-ai-*` provider concern and
   an explicit non-goal here.
5. Config: `query.empty_response_nudge` — enum
   `EmptyResponsePolicy { Off, Nudge }`, default `Nudge`, consumed via
   `RuntimeConfig` (no new `Feature`; this is a sub-toggle).

### Tests

- Empty response ×1 then normal response → one nudge injected, turn
  completes, nudge absent from the persisted/normalized API view.
- Empty ×4 → three nudges then clean `end_turn` (no infinite loop).
- Reasoning-only response → nudged (not silently completed).
- `Off` policy → old behavior.

---

## (b) Edit failure closest-match hint

### Problem

When Edit's `old_string` misses entirely, the model gets a bare
`"old_string not found in {file_path}"`
(`core/tools/src/tools/edit.rs:411,443`) and must spend another Read
round. Internal fuzzy recovery already exists (quote-normalized
`find_actual_string`, whitespace `find_fuzzy_match` — `edit.rs:429-440`,
`edit_utils.rs`) but its knowledge is discarded on total failure.

### Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.4.23 #13435:

- Call site — `tools/file_operations.py:1582-1589` (`patch_replace`): on
  no-match, appends `format_no_match_hint(...)` to the error.
- `tools/fuzzy_match.py:932-950` `format_no_match_hint`: fires only when
  `match_count == 0` and the error is a genuine not-found (ambiguous
  multi-match deliberately excluded); prefixes
  `"\n\nDid you mean one of these sections?\n"`.
- `tools/fuzzy_match.py:870-929` `find_closest_lines`: anchors on the
  **first non-blank line** of `old_string`, scores every content line
  with `difflib.SequenceMatcher(...).ratio()`, keeps `ratio > 0.3`
  (:900-902), takes top 3, renders each as a line-numbered snippet with
  2 context lines (`f"{start + j + 1:4d}| …"`, :921), snippets joined by
  `"\n---\n"`.

### Design

1. New helper in `core/tools/src/tools/edit_utils.rs`:

   ```rust
   /// Best-effort "did you mean" hint for a failed exact match.
   /// Returns None when nothing scores above the similarity floor.
   pub fn closest_match_hint(old_string: &str, content: &str) -> Option<String>
   ```

   - Anchor = first non-blank line of `old_string` (char-based, no byte
     slicing).
   - Score each content line with a normalized similarity (use the
     `strsim` crate — normalized Levenshtein or Jaro-Winkler — check the
     workspace for an existing similarity dep first, per the utils-first
     rule; the existing fuzzy matcher in `edit_utils.rs`/`apply-patch`
     may already expose a usable score).
   - Floor 0.3, top 3, 2 context lines each, `{line:>4}| ` gutter,
     snippets joined by `\n---\n`.
   - Cap total hint size at ~1.5 KB via
     `coco_utils_string::take_bytes_at_char_boundary` (UTF-8 safety —
     CJK content is the common case here).
2. Wire into both bare-error sites (`edit.rs:411` and `:443`): append
   `\n\nDid you mean one of these sections?\n{hint}` only on the
   zero-match path. The multi-match path (:449-451) keeps its existing
   guidance — mirroring hermes's gate.
3. NotebookEdit and apply-patch are out of scope (apply-patch already
   has its own fuzzy pipeline).

### Tests

- Near-miss (whitespace/indent drift beyond fuzzy recovery) → hint
  contains the expected line numbers and 2 context lines.
- Nothing similar → no hint (bare error unchanged).
- Multi-match → unchanged error (no hint).
- CJK content near the size cap → no panic, valid UTF-8.

---

## (c) Bash output ANSI stripping

### Problem

Bash tool results keep ANSI escapes: the output path does only
blank-line trim + truncation (`core/tools/src/tools/bash.rs:710`,
`:1593`); the repo's only `strip_ansi` is the TUI statusline
(`app/tui/src/status_bar/runtime.rs:318`). Colored output (cargo, jest,
eslint) wastes tokens and — hermes's root-cause note — leads models to
copy escape sequences into file writes.

### Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.3.23 #2115 lineage; current implementation:

- `tools/ansi_strip.py:16-44` — full ECMA-48 regex (CSI incl.
  private-mode `?`/colon params, OSC with BEL/ST terminators,
  DCS/SOS/PM/APC, nF/Fp/Fe/Fs, 8-bit C1) with fast path
  `_HAS_ESCAPE = re.compile(r"[\x1b\x80-\x9f]")` (:32). Docstring:
  "prevents ANSI codes from entering the model's context — which is the
  root cause of models copying escape sequences into file writes."
- Call sites: `tools/terminal_tool.py:2721-2722` (strip after head/tail
  truncation, before secret redaction and exit-code interpretation);
  `tools/code_execution_tool.py:1070-1071,1495-1497`;
  `tools/process_registry.py:1083-1084` etc. (background output tails).

### Design

1. Add `strip_ansi(&str) -> Cow<'_, str>` to **`coco-utils-string`**
   (utils-first rule; it is a pure string transform needed by ≥2
   consumers). Implementation options, in preference order:
   - the `strip-ansi-escapes` crate (vte-based, well-maintained) as a
     workspace dep of `utils/string`;
   - hand-rolled ECMA-48 state machine mirroring hermes's class list,
     with a `memchr`-style fast path on `\x1b` / C1 bytes (return
     `Cow::Borrowed` when clean — the overwhelmingly common case).
2. Call sites:
   - Bash foreground result path (`bash.rs`, before `truncate_output` —
     stripping **before** truncation, unlike hermes's after, so escape
     bytes don't consume the output budget and truncation can't land
     mid-escape and leave garbage);
   - background task output tail (`disk_task_output` read path) — same
     rationale as hermes's `process_registry` sites;
   - leave PTY/TUI paths untouched (they render ANSI on purpose).
3. No config knob. This is unconditionally correct for model-facing
   output.

### Tests

- SGR color, OSC title (BEL and ST terminated), CSI private-mode, and
  8-bit C1 sequences all removed; plain text passes through borrowed.
- Truncation boundary: input where the old code would cut mid-escape now
  yields clean text.
- Snapshot of a colored `cargo test` fixture before/after.

---

## Rollout

Three commits in one PR (or split): (a) `feat(query): nudge-retry empty
model responses`, (b) `feat(tools): closest-match hint on Edit miss`,
(c) `feat(tools): strip ANSI from Bash results`. Each gated by
`just quick-check`; one `just pre-commit` at the end.
