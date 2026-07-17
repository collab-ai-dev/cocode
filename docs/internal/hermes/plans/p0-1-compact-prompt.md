# P0-1 — Compaction Prompt: Language Preservation + Temporal Anchoring

Status: not started · Size: XS (prompt-only) · Owner crate: `coco-compact` (+ one call-site change in `coco-query`)

## Problem

1. **Summary language.** All three coco summarization templates are
   English-only with no language rule — a Chinese conversation compacts
   into an English summary, degrading continuation quality for CJK
   users. Verified: `services/compact/src/prompt.rs`
   (`BASE_COMPACT_TEMPLATE` :92, `PARTIAL_COMPACT_TEMPLATE` :176,
   `PARTIAL_COMPACT_UP_TO_TEMPLATE` :240) contain no language/locale
   instruction; the only injection point is user-supplied
   `Additional Instructions` (:319-323).
2. **Temporal anchoring.** coco has a re-confirm guard ("Do not start on
   tangential requests or really old requests that were already
   completed…", `prompt.rs:107`) but never injects the current date and
   never asks for completed actions to be restated as dated past-tense
   facts — so a resumed/compacted session can re-issue already-completed
   actions phrased as open instructions.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.6.19 (v0.17.0), PR #41102; language rule from v2026.4.23
(v0.11.0), PR #12556.

- **Language rule** — `agent/context_compressor.py:1802-1803`, inside
  `_summarizer_preamble` (:1796-1808), shared by first-compaction and
  iterative-update prompts (used at :1906):

  ```python
  "Write the summary in the same language the user was using in the "
  "conversation — do not translate or switch to English. "
  ```

- **Date injection with clock-failure fallback** —
  `agent/context_compressor.py:1786-1791`: current date formatted
  `%Y-%m-%d` inside `try/except`, `_today_str = ""` on failure ("a clock
  failure must never block compaction", :1785).
- **Temporal anchoring rule** — `agent/context_compressor.py:1815-1824`,
  emitted only `if _today_str:`; else the rule is the empty string
  (:1825-1826 — "the summarizer is never handed an empty date
  placeholder"):

  ```python
  f"\nTEMPORAL ANCHORING: The current date is {_today_str}. When an "
  "action has already been carried out, phrase it as a completed, "
  "dated, past-tense fact rather than an open instruction. For "
  'example, rewrite "email John about the proposal" as "Sent the '
  f'proposal email to John on {_today_str}." Never leave a finished '
  "action worded as if it still needs doing…"
  ```

## Design

`services/compact` deliberately reads no environment and no clock (crate
CLAUDE.md invariant). Therefore the **date is threaded in by the
caller**, not computed inside the crate.

1. New input on the prompt assembly path:

   ```rust
   // services/compact/src/prompt.rs
   pub struct CompactPromptOptions<'a> {
       pub custom_instructions: Option<&'a str>, // existing param, folded in
       /// `%Y-%m-%d`, provided by the caller. None ⇒ the temporal
       /// anchoring rule is omitted entirely (mirror hermes: never emit
       /// an empty date placeholder).
       pub current_date: Option<&'a str>,
   }
   ```

   Change `get_compact_prompt(custom_instructions)` /
   `assemble_directive` to take `CompactPromptOptions`.

2. Two new constants in `prompt.rs`, injected into the shared directive
   so all three templates get them (do NOT paste per-template):

   - `LANGUAGE_RULE`: "Write the summary in the same language the user
     was using in the conversation — do not translate or switch to
     English." Always included.
   - `temporal_anchoring_rule(date) -> String`: the hermes text above
     with `{date}` inlined (`format!` with inline variables). Included
     only when `current_date` is `Some`.

3. Call sites (`app/query/src/engine_compaction.rs`, and any other
   `get_compact_prompt` caller — grep before coding) compute the date
   once per compaction with `chrono::Local::now().format("%Y-%m-%d")`,
   matching the existing date-only convention
   (`app/query/src/engine_prompt.rs:875`). Wrap so a formatting failure
   yields `None`, never a panic and never a blocked compaction.

## Implementation steps

1. Add `CompactPromptOptions` + the two rules; update `assemble_directive`.
2. Update all `get_compact_prompt` call sites (compile errors are the
   worklist — no back-compat shim, per repo policy).
3. Update/regenerate compact prompt snapshot tests.
4. `just quick-check` → `just test-crate coco-compact` (plus
   `coco-query` if its tests assert on prompt text).

## Tests

- Snapshot: all three templates contain the language rule.
- `temporal_anchoring_rule` present when date given; **absent** (no
  "TEMPORAL ANCHORING" substring, no dangling header) when `None`.
- Existing `custom_instructions` tests still pass (ordering: language
  rule and temporal rule are part of the fixed directive, custom
  instructions remain last).

## Risks / non-goals

- Prompt-cache: compaction requests are one-shot side queries; the date
  string varies daily but does not touch the main conversation's cached
  prefix. No cache risk.
- Non-goal: hermes's iterative-update summary structure (updating the
  previous summary instead of re-summarizing) — different mechanism,
  deliberately not in this PR.
