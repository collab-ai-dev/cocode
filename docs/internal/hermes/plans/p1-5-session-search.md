# P1-5 — Deterministic Session Full-Text Search

Status: not started · Size: M · Owner crates: `coco-tools` (tool),
`coco-session` (search backend), `coco-tui` (picker filter)

## Problem

coco has no session search. Verified: no model-callable session-search
tool exists in `core/tools/src/tools`; the resume picker filters
**labels only** (`app/tui/src/modal_pane/mod.rs:680-687`); the only
transcript search is prompt guidance telling the memory/dream agent to
grep JSONL by hand (`memory/src/prompt/builders.rs:249-250,328`).
"What did we decide about X last week?" requires manual grep.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.5.28 (v0.15.0) #27590 — the rebuild that replaced an
aux-LLM summarization tool (~$0.30/call, 30–90 s, occasionally
confabulating results not in the hit list) with a deterministic tool:
"4,500× faster" (discovery ~20 ms, scroll ~1 ms, zero cost).

- **Single-shape tool, mode inferred from args** —
  `tools/session_search_tool.py`, `session_search()` (:619-740),
  docstring (:634-643) — note **four** shapes:

  ```python
  """Single-shape tool. Mode inferred from which args are set.

  Discovery: pass ``query``.
  Scroll:    pass ``session_id`` + ``around_message_id``.
  Read:      pass ``session_id`` (no anchor) — dumps the whole session.
  Browse:    pass nothing.
  ```

  Scroll takes precedence over query (:678-686, "explicit anchor beats
  any query").
- **No LLM, FTS-backed**: tool description :756 "FTS5-backed retrieval
  over the SQLite message store. No LLM calls"; browse helper
  `_list_recent_sessions` (:261, "no LLM calls, no FTS5"); discovery
  `_discover` (:507, "FTS5 + anchored window + bookends per hit. Single
  call."). Store: `hermes_state.py:5-11` (FTS5 virtual table).
- CJK: hermes needed trigram FTS5 (release v2026.4.30 #16651) because
  FTS5's default tokenizer can't segment CJK — a ripgrep-style substring
  scan avoids that entire problem class.

## Design

**Deterministic by construction — no LLM anywhere in this feature**
(anti-lesson: hermes measured the LLM version slower, costlier, and
confabulating).

### Backend (`app/session`, new `search` module)

coco sessions are JSONL transcripts on disk (not SQLite), so v1 is a
scan, not an index:

1. `SessionSearchQuery { query: Option<String>, session_id: Option<SessionId>, around_message_id: Option<String>, limit: i64 }`.
2. Discovery: walk session files (newest-first, bounded by `limit`
   sessions and a wall-clock budget), substring/regex match over
   message text content (skip binary/attachment payloads), return hits
   grouped by session: `{session_id, title, mtime, hits: [{message_id,
   role, excerpt, line_no}]}` with excerpts cut UTF-8-safely
   (`coco_utils_string::take_bytes_at_char_boundary`, ~200 bytes,
   match-centered). Substring scan handles CJK natively — no tokenizer,
   sidestepping hermes's trigram retrofit.
3. Scroll: given `session_id` + `around_message_id`, return a window of
   N messages around the anchor (the "anchored window" shape).
4. Read: `session_id` alone → paginated whole-session dump (respect the
   tool result budget — this can be large; the Level 1/2 budget system
   handles overflow naturally).
5. Browse: no args → recent sessions list (id, title, mtime, message
   count) — data the resume picker already derives.
6. Performance: pure streaming scan with early-exit; if profiling later
   shows real corpora too slow, add a tantivy index as v2 — do not
   build the index speculatively.

### Tool (`core/tools`)

- `SessionSearch` tool with the four arg-inferred shapes (single tool,
  minimal schema — hermes's design point: no `mode` param, no config,
  no companion skill). Scroll precedence over query, mirroring hermes.
- `is_concurrency_safe = true` (read-only); default
  `max_result_size_bound` (Level 1 persistence applies if a dump is
  huge).
- Exclude the **current** session's in-flight transcript from discovery
  (the model already has it; matching it produces noise).
- Description states explicitly: deterministic search, results are
  verbatim excerpts, use Read/scroll for more context.

### TUI (`app/tui`)

- Extend the resume picker (`SessionBrowser`) filter: when the filter
  string is prefixed with `/` (or a keybinding toggles content mode),
  route through the same backend's discovery shape and show
  session+excerpt rows. Reuses the backend verbatim — no second
  implementation.

## Implementation steps

1. Backend module + unit tests over fixture JSONL sessions.
2. Tool + schema + registry wiring (+ `should_defer()` = true — this is
   a good ToolSearch-deferred candidate).
3. TUI picker content mode.
4. `just test-crate coco-session` + `coco-tools`; TUI snapshot for the
   picker.

## Tests

- Discovery finds a known string across 3 fixture sessions, grouped,
  newest first; current session excluded.
- CJK query matches CJK transcript content; excerpts are valid UTF-8.
- Scroll returns the exact anchored window; anchor+query → scroll wins.
- Read paginates and respects the result budget (oversized dump gets
  the Level 1 `<persisted-output>` treatment automatically).
- Browse with zero sessions → empty result, no error.

## Risks / non-goals

- Large corpora scan latency: bounded walk + early exit; index only as
  evidence-driven v2.
- Privacy: search is local-only; no content leaves the machine.
- Non-goals: LLM summarization of results (anti-lesson); cross-machine
  session sync; searching subagent transcripts (v2 if wanted).
