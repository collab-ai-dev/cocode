# Hermes-Agent Release-Log Sweep → coco-rs Absorption List

Status: analysis complete (2026-07-10) — **superseded by
[hermes-opt-0724.md](hermes-opt-0724.md)** (2026-07-24), which re-verified
every item below against current coco-rs, absorbed the v0.19.0 window, and
re-prioritized. This file remains the evidence record for v0.2.0→v0.18.2.
Scope: all 21 hermes-agent releases, v2026.3.12 (v0.2.0) → v2026.7.7.2 (v0.18.2)
Related: [../tool-result-offload-v2-design.md](../tool-result-offload-v2-design.md) (P0 #7 below),
[../cc-catchup-roadmap-v2.md](../cc-catchup-roadmap-v2.md) (model-card item merges with P1 #6),
[README.md](README.md) (earlier architecture-level comparison in this directory, 2026-07-02),
[plans/](plans/) (per-item development plans derived from this analysis)

## 1. Method

- Source: the full GitHub release log of `NousResearch/hermes-agent`
  (github.com/NousResearch/hermes-agent, ~700 KB of notes across 21
  releases), pulled via `gh api` and read in its entirety by four
  parallel extraction agents. Ambiguous mechanisms were verified against
  a hermes source checkout at commit `a7f65e3bc` (`v2026.7.7.2` + 161
  commits); hermes citations throughout use repo-relative paths (e.g.
  progressive tool disclosure's 10% threshold gate in
  `tools/tool_search.py`, `search_files` densification in
  `tools/file_operations.py`, the date-only timestamp rationale in
  `agent/system_prompt.py`, adaptive subprocess polling constants in
  `tools/code_execution_tool.py`).
- Cross-check: every "coco-rs might lack this" hypothesis that mattered
  was verified against the coco-rs source by three read-only agents —
  31 facts total, each with file:line evidence. Verdicts below cite that
  evidence. **12 suspected gaps turned out to be already present**; they
  are listed in §2 so they don't get re-flagged later.
- Skipped categories: desktop/dashboard chrome, messaging-platform
  adapters (Telegram/Discord/...), installers/packaging, model-catalog
  additions, Python-specific fixes with no transferable idea.

Hermes's optimization work runs on four axes across the 21 releases:

1. **Token/context economy** — truncate-and-store, progressive tool
   disclosure, prompt-cache byte stability, budget scaling.
2. **Compaction hardening** — anti-thrashing, language/temporal
   anchoring, iterative summaries, trigger accuracy.
3. **Weak-model tool-call robustness** — repair, guardrails, nudges.
   (Directly relevant to coco-rs's multi-provider positioning.)
4. **Agent quality loops** — `/goal` judged loops, verify-on-stop,
   self-improvement forks, kanban orchestration.

coco-rs is already ahead of or at parity with most of the
infrastructure on axes 1–2. The genuinely absorbable items concentrate
on axes 3–4 plus a handful of cheap compaction-prompt and tool-output
fixes.

## 2. Already Present in coco-rs — Do NOT Re-Flag

All verified with file:line evidence (2026-07-10, branch
`feat/multisession-phaseb`).

| Hermes mechanism | Hermes release | coco-rs equivalent (evidence) |
|---|---|---|
| Date-only system timestamp for prompt-cache stability | v0.15 #27675 | `currentDate` reminder is `%Y-%m-%d` only (`app/query/src/engine_prompt.rs:875`, `core/system-reminder/src/generators/user_context.rs:60`); no minute-precision anywhere in the cached prefix |
| Cross-session 1 h Anthropic prompt cache | v0.14 #23828 | `AdapterCacheTtl::{FiveMinutes,OneHour}` + `prompt_cache.allowlist` / `COCO_PROMPT_CACHE_ALLOWLIST` (`vercel-ai/anthropic/src/cache_placement.rs:51`, `cache_policy.rs:43`) |
| `/compress <focus>` guided compaction | v0.9 #8017 | `/compact [instructions]` → `CompactRequest.custom_instructions` → "Additional Instructions" in the summary prompt (`commands/src/handlers/compact.rs:54`, `services/compact/src/prompt.rs:319`) |
| Per-model-family execution-discipline prompt blocks (GPT/Grok/GLM) | v0.8 #5595, v0.15 #27797 | Per-family `base_instructions` templates: `gpt5_4_prompt.md`, `gpt5_3_codex_prompt.md`, `gemini_prompt.md` (`common/config/src/builtin/{openai,google}.rs`) |
| Cross-provider reasoning replay hygiene (strip foreign-issuer reasoning, signature 400s) | v0.12 #15749, v0.15 #33156 | Convert seams drop foreign signatures (`vercel-ai/anthropic/src/messages/convert_to_anthropic_messages.rs:860`); normalize filters orphaned thinking + signature-mismatched merges (`core/messages/src/normalize.rs:1229,779`) |
| Per-category context breakdown (`/usage`) | v0.18 #55204 | `/context` with `ContextCategoryKind` grid (`common/types/src/context_usage.rs:14`, `app/tui/src/presentation/context_usage.rs`) |
| MCP parallel-safety declaration (per **server** flag) | v0.14 #26825 | Finer: per **tool** via `readOnlyHint` annotation (`core/tools/src/tools/mcp_tools.rs:728`) feeding the safe-concurrent/unsafe-queued executor lanes |
| Exit-code interpretation (grep 1 = no matches) | v0.8 #5144 | `coco_shell::semantics::interpret_command_result` (`exec/shell/src/semantics.rs:22`) |
| Malformed tool-call JSON → corrective retryable tool_result | v0.4 #2342 | `parse_with_repair` + `<tool_use_error>...retry with valid JSON` (`app/query/src/tool_input_parse.rs:16`, `tool_call_preparer.rs:181`) |
| LSP semantic diagnostics on every write | v0.14 #24168 | `notify_save` on Write/Edit/ApplyPatch + `LspDiagnosticsAdapter` system-reminder with drain-on-read dedup (`core/tools/src/tools/write.rs:320`, `app/query/src/reminder_adapters.rs:41`) |
| Compaction anti-thrashing loop-breaker | v0.11 #10063 | Rapid-refill breaker (3 compacts within 3 turns trips) + 3-consecutive-failure breaker (`services/compact/src/reactive.rs:124`, `types.rs:60`) |
| Compaction trigger counts system prompt + tool schemas | v0.13 #18265 | Primary path anchors on API-billed usage (`MessageHistory::tokens_with_last_usage`, `core/messages/src/history.rs:541`), which includes everything; local estimate only as marker-unset fallback |
| Paste collapse into placeholder | v0.15 #32087 | `LARGE_PASTE_CHAR_THRESHOLD = 1000` pill + expand-at-submit (`tui-ui/src/paste.rs`) — threshold is a const, not user-configurable (acceptable) |

Also at parity via different designs: MoA (`app/cli/src/bin_handlers/moa.rs`),
checkpoints/rewind, worktree isolation, fast mode, workflow/swarm
orchestration (judge panels, verifiers), `@file` references, message
queueing during runs, ToolSearch progressive disclosure itself (hermes
v0.16 #34493 — coco has the deferred-tools + ToolSearch pipeline with
provider-native variants), Anthropic server-side context editing
(`COCO_COMPACT_API_CLEAR_TOOL_RESULTS`), and SIGTERM→SIGKILL escalation
(closed on `feat/multisession-phaseb`).

## 3. P0 — Cheap, Direct Wins

Ordered by suggested implementation sequence. Items 1–2 are a two-line
prompt PR; 3, 5, 6 are small `app/query` / `core/tools` changes; 4 adds
a config knob.

### 3.1 Compaction summary language preservation — ABSENT

Hermes v0.11 #12556. The summary must be written in the conversation's
language. All three coco summary templates
(`services/compact/src/prompt.rs`: `BASE_COMPACT_TEMPLATE:92`,
`PARTIAL_COMPACT_TEMPLATE:176`, `PARTIAL_COMPACT_UP_TO_TEMPLATE:240`)
are English-only with no language rule — a Chinese conversation
compacts into an English summary, degrading continuation quality for
CJK users. Fix: one instruction line in the shared directive.

### 3.2 Compaction temporal anchoring — PARTIAL

Hermes v0.17 #41102: inject the current date into the compaction prompt
and require completed actions to be restated as **dated past-tense
facts** ("Sent the proposal email to John on <date>") so a resumed
session doesn't re-execute completed work; omit the rule entirely if
date resolution fails (never an empty placeholder). coco has a
re-confirm guard ("do not start … old requests that were already
completed without confirming", `prompt.rs:107`) but no date injection
and no past-tense rule. Fix: a few lines in `prompt.rs`; thread the
date in from the caller (the compact crate deliberately reads no env).

### 3.3 Empty-response nudge/retry — ABSENT

Hermes v0.9 #6488 (retry ×3 with an injected nudge), v0.8 #5278
(reasoning-only responses accepted as "(empty)" instead of retry
looping), v0.11 #10472 (an empty response *after substantive tool
calls* continues the loop instead of ending the turn). In coco a clean
empty response (no content, no tool calls) just ends the turn
(`app/query/src/engine_terminal.rs:368`); thinking-only text is dropped
(`engine_stream_consume.rs:599`) and only feeds stream-error retry
decisioning. The only retry path today is structured-output mode
(`engine_terminal.rs:227`). This matters most for the weak-model /
OpenAI-compatible tail of coco's provider matrix. Fix: bounded retry
(≤3) with a nudge user-message in `handle_no_tool_calls_terminal`,
plus a special case for reasoning-only responses.

### 3.4 Warning-first tool-call loop guardrails — ABSENT

Hermes v0.13 #18227: a side-effect-free per-turn controller hashes
exact tool calls and classifies idempotent vs mutating; identical
failing call → warn after 2 / block after 5; same-tool repeated
failures → warn after 3 / halt turn after 8; warnings are on by default
and never prevent execution; hard-stop is explicit opt-in; a blocked
call synthesizes exactly one synthetic tool result rather than
erroring. coco has nothing of this kind — and notably
`Tool::inputs_equivalent` (`core/tool-runtime/src/traits.rs`) already
exists with **zero call sites**, a ready-made seam. The related-but-
different mechanisms (permission `DenialTracker`, StructuredOutput
retry cap) don't cover model-driven repetition burn. Fix: per-turn
observation map in the executor, warning text appended to the repeated
call's tool result; `tool.loop_guardrail` config with `warn` (default)
/ `block` levels.

### 3.5 Edit failure closest-match feedback — ABSENT (feedback half)

Hermes v0.11 #13435 ("did you mean" on patch mismatch). coco's Edit
already runs silent fuzzy recovery internally (quote-normalized and
whitespace-tolerant matching, `core/tools/src/tools/edit.rs:429-440`);
but when every fallback fails the model gets a bare
`old_string not found in {file_path}` (`edit.rs:411,443`) and must
spend another Read round. Fix: on failure, surface the best near-miss
(a few lines of the closest region, or the whitespace-normalized match
position) in the error text. The matching machinery already exists;
this is error-message plumbing only.

### 3.6 Bash output ANSI stripping — ABSENT

Hermes v0.4 #2115 (strip at the source). coco's Bash result path does
blank-line trim + truncation only (`core/tools/src/tools/bash.rs:710`,
`:1593`); the repo's single `strip_ansi` lives in the TUI statusline
(`app/tui/src/status_bar/runtime.rs:318`), not the tool path. Colored
tool output (cargo, jest, eslint …) wastes tokens and occasionally
confuses models. Fix: strip ANSI escapes in the capture path before
truncation. (When the offload seam from
[../tool-result-offload-v2-design.md](../tool-result-offload-v2-design.md) §7
lands for Bash, strip *before* window+persist so artifacts are clean
too.)

### 3.7 Micro-compact recovery pointer — covered elsewhere

Hermes persists cleared tool results with a `<persisted-output>`
pointer; coco's micro-compact replaces content with the plain
`"[Old tool result content cleared]"` string
(`services/compact/src/types.rs:85`) — no recovery path. This gap is
**already addressed by the Tool Result Offload v2 design** (§6:
micro-compact skips pointer-bearing results; offloaded results retain
their on-disk reference). No separate work item; listed here only for
traceability.

## 4. P1 — Medium Cost, Reliability / Ecosystem Wins

### 4.1 MCP `list_changed` refresh + keepalive ping — both ABSENT

Hermes v0.6 #3812 (dynamic tool discovery), v0.17 #49221 (keepalive for
short-TTL HTTP sessions), v0.13 MCP robustness batch. coco's only
`ClientHandler` logs `on_tool_list_changed` and does nothing
(`services/rmcp-client/src/logging_client_handler.rs:96`); tool lists
refresh only on reconnect (`services/mcp/src/discovery.rs:217`). No
periodic ping exists; HTTP/SSE session death is handled reactively via
404 → `SessionExpired404` reconnect (`rmcp_client.rs:101`). Silent
tool-list drift and mid-conversation session expiry are real failure
modes hermes burned multiple releases on. Note: coco already has the
cache-safe *presentation* half (deferred-tools delta reminders), so the
work is transport-side only.

### 4.2 ToolSearch deferral threshold gate — ABSENT

Hermes v0.16 #34493 (verified in `tools/tool_search.py`): on every
tools-array assembly, if the deferrable tools would consume < 10%
(`threshold_pct`) of the model's context window, disclosure is a no-op
and the full array passes through — small tool sets never pay the
tool_search round-trip. coco's deferral is a static per-tool boolean
(`should_defer()`, `core/tool-runtime/src/traits.rs:619`) gated only by
`Feature::ToolSearch` (`core/tool-runtime/src/registry.rs:385`). Fix:
estimate deferred-schema tokens at assembly (an estimator already
exists in inference for per-model schema filtering) and skip deferral
below a window-relative threshold.

### 4.3 Grep content-output densification — worth porting

Hermes v0.17 #47866 (verified in `tools/file_operations.py`): when a
content search returns ≥ 5 matches, replace the per-hit
path-repeated form with a path-grouped block — path printed once, then
`  <line>: <content>` rows. Lossless (every path/line/byte preserved),
exploits ripgrep's path-ordered output; below 5 matches keep the
verbose form. Hermes ran a "headroom evaluation" first and concluded
this was the **only** output densification worth shipping. coco emits
`path:line:content` with the path repeated on every hit
(`core/tools/src/tools/grep.rs:888-912`) — 20-40% waste on deep-path
monorepos. Fix: format-only change in `format_content` with the ≥ 5
threshold.

### 4.4 Zero-LLM scheduled scripts — ABSENT

Hermes v0.13 #19709 (`no_agent` cron), v0.11 #12373 (`wakeAgent` gate),
v0.14 #21881 (`watchers` skill): a scheduled job runs a script only;
empty stdout → silent, non-empty stdout → delivered (or used to wake
the agent with the output as context). Zero-token monitoring for the
common "poll something, tell me if it changed" case. coco's
`CronCreateInput` has only `cron`/`prompt`/`recurring`/`durable`
(`core/tools/src/tools/scheduling.rs:80`) and every firing enqueues an
agent turn (`app/cli/src/cron_tick.rs:177`). Fix: optional
`script` field on the job; the tick driver runs it and only enqueues a
turn (with stdout attached) when output is non-empty.

### 4.5 Session full-text search — PARTIAL

Hermes v0.15 #27590 rebuilt `session_search` from aux-LLM summarization
(~$0.30/call, 30–90 s, occasionally confabulating hits) to a single
deterministic FTS tool with three arg-inferred modes
(discovery/scroll/browse) — ~20 ms, zero cost, "4,500× faster". Their
v0.12 work added trigram FTS5 so CJK queries work. coco has no session
search tool and the resume picker filters titles only
(`app/tui/src/modal_pane/mod.rs:680`); the memory/dream prompt tells
the model to grep transcript JSONL by hand
(`memory/src/prompt/builders.rs:249`). Fix (if/when wanted):
deterministic search over session JSONL (ripgrep-backed is fine; a
trigram index only if latency demands), exposed as a tool and/or
resume-picker content filter. **Anti-lesson applies: never LLM-back
this** (§6.2).

### 4.6 Model retirement metadata + startup warning — ABSENT

Hermes v0.15 #29277: model-card retirement dates + startup/doctor
detection + one-shot guided config migration; no silent 404s after a
vendor retires a model. coco's `ModelCard`
(`common/model-card/src/schema.rs:2-18`) has **no**
deprecation/retirement field at all, so nothing can act on it. Merge
this with the existing cc-catchup-roadmap-v2 P1 model-card item (coco's
own running id resolution) rather than doing it separately.

### 4.7 Per-model reasoning floor for stream stall detection — PARTIAL

Hermes v0.18 #52845: stall detectors get a per-model minimum timeout
floor for reasoning models so long thinking phases aren't killed as
hung requests. coco has the machinery (`StreamProcessor` idle 60 s /
stall 30 s, `vercel-ai/ai/src/stream/processor.rs:21`) but the
inference default **disables** idle timeout globally precisely to
protect slow reasoning streams (`services/inference/src/stream.rs:604`)
— trading away hang detection for every model. Fix: keep a short idle
timeout as default and give reasoning-capable models a high per-model
floor (model-card is the natural home for the flag).

## 5. P2 — Strategic Items (Own Design Doc Each)

### 5.1 `/goal` standing judged loop + verify-on-stop

Hermes's largest sustained quality investment, v0.13 → v0.18:

- **Ralph loop** (v0.13 #18262, verified in `hermes_cli/goals.py`): a
  free-form user goal survives across turns; after each turn a small
  judge call on an auxiliary model asks "is the goal satisfied?"; if
  not, a continuation prompt is fed back **as a plain user message** —
  no system-prompt mutation, no toolset swap, prompt cache stays
  intact. Bounded by a turn budget (default 20). Judge failures are
  fail-open (continue; budget is the backstop). A real user message
  preempts and pauses the loop, with a re-judge afterward. State
  persists in the session store so resume picks it up.
- **Completion contracts + verify-on-stop** (v0.18 #50501, #52285): the
  user states what "done" looks like; the loop judges against that
  evidence contract. Coding work is verified by *running the project's
  canonical checks* (tests/lint, detected per-repo and persisted in an
  evidence ledger) rather than trusting the model's claim; a
  `pre_verify` hook allows custom checks; doc-only edits skip
  verification (cost gate); defaults are surface-aware.
- **`/subgoal`** (v0.14 #25449) layers extra criteria mid-run;
  **`/goal wait <pid>`** (v0.18 #50503) parks the loop on a background
  OS process instead of a timer.

coco has hooks (Stop), durable `task_list`, and workflows — but no
standing judged loop. Landing sketch: goal state on the session, a
judge call at the `ContinueReason` seam using an aux `ModelRole`,
continuation injected through the existing `CommandQueue`
(`QueueOrigin` distinguishes it), turn budget + fail-open + user
preemption semantics copied as-is. Verify-on-stop composes with the
existing Stop-hook infrastructure.

### 5.2 Background-fork token discipline

Hermes v0.18 #49252 and v0.15 #29704: the post-turn self-improvement
fork (a) routes to a cheap auxiliary model, (b) consumes a **context
digest instead of replaying the whole conversation**, (c) adapts its
cadence — "costs a fraction of what it used to"; and the fork inherits
the parent's exact tool configuration so the serialized `tools[]` is
byte-identical and the provider prompt-cache prefix stays valid.
coco's skill-learning review fork already routes to `ModelRole::Memory`
(user directive); the digest-not-replay and tools[]-byte-parity ideas
are worth auditing against the fork's current request shape.

### 5.3 Browser automation — decide, don't drift

Hermes treats the browser as a first-class surface (CDP supervisor,
local-Chromium auto-spawn for LAN targets, persistent-connection JS
eval at 180× the naive cost, cloud-metadata floor + post-navigation
SSRF recheck). coco has none of it. This is a product-scope decision,
not an absorption item: either schedule a design effort or add it to
the explicit non-goals list so it stops resurfacing in gap analyses.

## 6. Anti-Lessons — Things Hermes Proved We Should NOT Do

1. **Never extend secret redaction into tool I/O.** Hermes redacted
   tool outputs; secret-shaped false positives were substituted into
   patch content and API payloads, corrupting them. The default was
   flipped OFF in v0.12 (#16794), re-enabled in v0.13 only after
   context-aware fixes (#21193), and they were still fixing URL
   query-param false positives in v0.15 (#34029). coco's
   `secret-redact` is used for logs/telemetry only — keep that
   boundary.
2. **Never LLM-back session search.** Hermes's aux-LLM `session_search`
   cost ~$0.30/call, took 30–90 s, and sometimes confabulated results
   not in the hit list. The deterministic replacement was 4,500× faster
   at equal-or-better quality (v0.15 #27590).
3. **History-injected pressure warnings need freshness management.**
   Stale iteration-budget warnings left in history conditioned models
   to avoid tools in later turns; hermes had to ship history scrubbing
   for their own injected markers (v0.5 #3528). If coco ever injects
   budget/pressure text into tool results, pair it with expiry.
4. **Provider content filters constrain injected-marker phrasing.**
   Azure flagged `[SYSTEM:`-prefixed injected text as injection attack;
   hermes renamed all markers to `[IMPORTANT:` (v0.12 #16114). Bear
   this in mind for any new coco reminder/steer markers on
   OpenAI-compatible endpoints.
5. **Don't clone OAuth grant files across profiles** — providers
   revoke the sibling (hermes reverted exactly this, v0.18 #51732).

## 7. Suggested Sequencing

1. **PR 1 (prompt-only):** §3.1 language preservation + §3.2 temporal
   anchoring — two small edits in `services/compact/src/prompt.rs`
   plus a date parameter from the caller; test via existing prompt
   snapshot tests.
2. **PR 2 (loop robustness):** §3.3 empty-response nudge + §3.5 Edit
   closest-match + §3.6 ANSI strip.
3. **PR 3 (guardrails):** §3.4 warning-first loop guard with
   `tool.loop_guardrail` config.
4. **P1 batch, in value order:** §4.1 MCP transport (list_changed +
   keepalive) → §4.3 Grep densification → §4.2 ToolSearch threshold →
   §4.4 zero-LLM cron → §4.5 session FTS → §4.6 retirement metadata
   (merged with model-card work) → §4.7 reasoning floor.
5. **P2:** write a design doc for §5.1 (`/goal` + verify-on-stop)
   before any code; audit §5.2 against the skill-learning fork; make an
   explicit go/no-go call on §5.3.
