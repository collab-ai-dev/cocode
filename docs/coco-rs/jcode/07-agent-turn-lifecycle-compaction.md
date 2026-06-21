# Agent Turn Lifecycle & Compaction: jcode vs coco-rs

Scope: the agent's per-turn drive loop (stream drain, tool execution,
mid-turn steering, interrupts) and the context-compaction subsystem
(triggers, strategies, emergency recovery, token estimation). All claims
below were re-verified against source on both trees; `file:line`
references are load-bearing.

jcode (`/lyz/codespace/3rd/jcode`) is an independent, performance-focused
harness. coco-rs (`/lyz/codespace/codex/coco-rs`) is a behavior-faithful
Rust port of Anthropic's Claude Code. They diverge by design; a
difference is judged on engineering merit *for coco-rs's stated goals*,
not treated as an automatic deficiency.

---

## jcode approach

### Turn loop — implemented three times

jcode's `Agent` struct has its turn loop written out in three parallel
`impl Agent` files with near-identical bodies:

- `src/agent/turn_loops.rs::run_turn` (~1098 LoC) — blocking/print mode
  (`run_once`, REPL, subagents).
- `src/agent/turn_streaming_broadcast.rs::run_turn_streaming` (~1014 LoC)
  — server broadcast channel.
- `src/agent/turn_streaming_mpsc.rs::run_turn_streaming_mpsc` (~1279 LoC)
  — per-client mpsc; the richest variant (interleaved input +
  background-tool detach).

Each is one `loop { build messages → open stream → drain StreamEvents →
push assistant msg → execute tools → inject soft-interrupts → continue }`.
The broadcast and mpsc injection-point / tool-exec sections are
line-for-line duplicated (e.g. broadcast `:824-948` ≈ mpsc `:909-948`).

### Stream drain — zero-spin async

The mpsc loop wraps both stream-open and per-event reads in
`tokio::select!` against a keepalive ticker and
`graceful_shutdown.notified()` (`turn_streaming_mpsc.rs:168-213`,
`:256-280`). The keepalive ticker is `interval_at` with
`MissedTickBehavior::Skip` (`streaming.rs:15-20`), 30s prod / 50ms test.
`InterruptSignal` (`crates/jcode-agent-runtime/src/lib.rs:31-69`) fuses an
`AtomicBool` (sync read) with a `tokio::sync::Notify` (async wake), so
interrupt polling never busy-spins — `notified()` short-circuits when
already set. This is a clean async pattern.

### Per-tool execution and detach

mpsc spawns *each tool* in its own `tokio::spawn` task
(`turn_streaming_mpsc.rs:1021-1025`) and `select!`s the join handle
against `background_tool_signal.notified()` and
`graceful_shutdown.notified()` with `biased` (`:1037-1068`). Three live
behaviors result during a running tool:

- **Alt+B detach** — the running JoinHandle is transferred to a
  background registry (`background::global().adopt`, `:1178-1180`;
  `background.rs:459-503`), a status file is written, and a `bg`-tool
  handle `tool_result` is injected so the turn keeps going (`:1182-1205`).
- **Server reload** — aborts inflight tools with a 750ms `bash` handoff
  grace (`:1051-1063`) and writes *resumable* interrupted results
  (`reload_interrupted_tool_result`, `:3-34`): wait-like tools
  (`bg`, `swarm await_members`) get a non-error "rerun with the same
  input" message; others get an error.
- **Normal completion** — proceeds.

### Soft interrupt — the headline differentiator

`docs/SOFT_INTERRUPT.md` + `src/agent/interrupts.rs` (~458 LoC) +
`src/soft_interrupt_store.rs` (~121 LoC). Messages typed during
generation are queued (`queue_soft_interrupt`, `interrupts.rs:122-152`)
into an `Arc<std::sync::Mutex<Vec<SoftInterruptMessage>>>`
(`jcode-agent-runtime/src/lib.rs:20`) that lives **outside** the agent
lock, so a server thread can enqueue without blocking the running turn.
Injection happens only at three API-safe points:

- **Point B** — no tool calls / turn complete
  (`turn_streaming_mpsc.rs:849-863`).
- **Point C** — urgent abort between tools: writes
  `[Skipped: user interrupted]` stub `tool_results` for *every* remaining
  tool **before** injecting, so `tool_use`/`tool_result` pairing stays
  valid (`:911-948`, exactly as `docs/SOFT_INTERRUPT.md:107-125`
  mandates).
- **Point D** — after all tools, before the next API call (`:895-905`).

Messages are source-tagged (`User`/`System`/`BackgroundTask`) so injected
content renders with the right role, grouped by consecutive source, and
**persisted to disk** (`pending-soft-interrupts/<session>.json`,
`soft_interrupt_store.rs:91-107`) so queued steering survives a self-dev
rebuild (restored in `restore_session`, `turn_execution.rs:443`).

Note: `messages_for_provider()` is called inside the loop each iteration
(`agent.rs:587-660`, invoked at `turn_streaming_mpsc.rs:54`), so each
continuation re-builds and re-sends the *full* history — identical to
coco-rs. The real soft-interrupt win is **not** "avoid full-context
re-send"; it is avoiding the hard cancel-and-restart that discards the
in-progress assistant generation and pays a fresh, wasted round-trip
(`SOFT_INTERRUPT.md:28-34`).

### Response recovery (robustness for OpenAI-compatible providers)

`src/agent/response_recovery.rs` (~199 LoC):

- `parse_text_wrapped_tool_call` / `recover_text_wrapped_tool_call`
  (`:4-100`) salvage tool calls the model emitted as raw text
  `to=functions.NAME{json}` — a documented failure mode on
  OpenRouter/Kimi/Groq-style endpoints. The recovery scans for the
  `to=functions.` marker, stream-parses the first valid JSON object,
  prefers a parse whose suffix is empty (`:45-47`), and is gated on
  `tool_calls.is_empty() && !text.trim().is_empty()` (`:61`). Wired into
  the loop at `turn_loops.rs:648` and `turn_streaming_mpsc.rs:756-778`
  (emitting synthetic `ToolStart`/`Input`/`Exec`).
- `should_continue_after_stop_reason` (`:102-118`) auto-continues on
  **any** abnormal stop_reason by substring-matching `incomplete`,
  `max_output_tokens`, `max_tokens`, `length`, `trunc`, `commentary`;
  `maybe_continue_incomplete_response` (`:126-167`) re-prompts up to
  `MAX_INCOMPLETE_CONTINUATION_ATTEMPTS` with a "continue exactly where
  you left off" nudge.
- `filter_truncated_tool_calls` (`:169-198`) drops `null`-input tool
  calls from `max_tokens` cut-offs and repairs the persisted assistant
  message (`remove_tool_use_blocks`).
- `repair_missing_tool_outputs` (`agent.rs:709+`) scans incrementally
  (`tool_output_scan_index`) for `tool_use` blocks lacking a matching
  result and injects stubs before each API call.

It also forces another turn when the model emitted only images, so it can
inspect generated visual context (`turn_streaming_mpsc.rs:834-843`).

### Compaction

`crates/jcode-compaction-core` (~647 LoC pure helpers) +
`src/compaction.rs::CompactionManager` (~1556 LoC) +
`src/agent/compaction.rs` (~287 LoC glue). The manager **does not own
messages** — the caller passes `&[Message]` and the manager tracks
`compacted_count` (leading messages already summarized). It keeps an
incremental `active_message_chars` rolling estimate updated on every
`notify_message_added_blocks` (`compaction.rs:204-211`), dirtied on
legacy adds / restore / micro-mutation.

**Three modes** (`jcode-config-types`, defaults at `lib.rs:245-260`:
`lookahead_turns=15`, `ewma_alpha=0.3`, `proactive_floor=0.40`,
`min_samples=3`, `stall_window=5`, `min_turns_between_compactions=10`,
`topic_shift_threshold=0.45`):

- **Reactive** (default) — compact at 80% of a 200K budget.
- **Proactive** — EWMA of per-turn token deltas projected
  `lookahead_turns` ahead vs the 80% threshold
  (`should_compact_proactively`, `compaction.rs:450-484`).
- **Semantic** — embeds last assistant text per turn, detects topic shift
  by cosine similarity of old-half vs new-half mean embeddings
  `< topic_shift_threshold` (`:497-537`), and does relevance-scored
  keep-set selection (`semantic_cutoff`, `:549-611`) so highly-relevant
  old messages stay out of the summary range. Falls back to proactive
  when embeddings are unavailable.

A universal `anti_signals_block` guard (`:399-441`) suppresses any
proactive/semantic trigger when: already compacting / below
`proactive_floor` / too few samples / token growth stalled / inside the
cooldown.

Compaction runs **in the background**: `maybe_start_compaction_with`
(`:838-856`) spawns a `tokio::spawn`, publishes
`BusEvent::CompactionFinished`, and the result is swapped in
non-blockingly by `check_and_apply_compaction_with` (`:996-1106`), which
polls `task.is_finished()` then `block_on`s only the finished task and
advances `compacted_count += pending_cutoff` clamped against the
caller-owned vec (`:1033-1036`) — so messages appended while summarizing
are preserved.

At ≥95% (`CRITICAL_THRESHOLD`), `ensure_context_fits` (`:864-909`)
performs a **synchronous hard compact** (`hard_compact_with`,
`:1307-1408`): progressively halves `turns_to_keep` from 10 down to 2
using a precomputed suffix-char prefix-sum, and builds an **emergency
summary** that data-mines dropped messages for tool names + file
references (`jcode-compaction-core::build_emergency_summary_text`,
`lib.rs:372-415`). The breadcrumb is conservatively scoped:
`looks_like_file_reference` (`lib.rs:446-458`) excludes `http` and only
matches known extensions (`.rs`/`.ts`/`.py`/`.toml`/`.json`) and
`src/`/`./` prefixes, capped at 30 (`:410`). `safe_compaction_cutoff`
(`jcode-compaction-core:207-260`) never splits a `tool_use` from its
`tool_result`.

On any context-limit API error,
`try_auto_compact_after_context_limit` (`agent/compaction.rs:110-175`)
string-matches ~12 English error phrases (`:90-104`:
"context length", "token limit", "prompt is too long", …), runs a
synchronous hard compact, resets cache tracker + provider session, and
the loop retries (capped `MAX_CONTEXT_LIMIT_RETRIES=5`). It also handles
OpenAI native compaction (`encrypted_content` passthrough, oversized
payload discard with text fallback). The manager lowercases the provider
name to decide cache accounting (`:230-244`).

### README claim cross-check (this module)

The headline "14ms time-to-first-frame / 245× faster than Claude Code"
(README:183-211) is a **TUI/PTY startup** metric, not an agent-loop
property — nothing in `turn_loops.rs` / `compaction.rs` substantiates a
per-turn latency advantage. The "semantic-vector agent memory / memory
graph / ambient consolidation" (README:264-266) **is** real and wired in:
`build_memory_prompt_nonblocking_shared` (`prompting.rs:20-48`) takes the
last turn's pending memory and spawns the next retrieval non-blockingly,
injected as an ephemeral suffix to preserve the cache prefix
(`turn_loops.rs:42-67`). The Semantic compaction mode genuinely uses
embeddings + cosine similarity (verified `compaction.rs:497-611`). No
"1800× mermaid" or "1000 fps" claim touches this module.

---

## coco-rs approach

### Turn loop — one loop for all entrypoints

A single `QueryEngine::run_session_loop` (`app/query/src/engine.rs`,
~2200 lines) drives every entrypoint — main loop, SDK turn, subagent,
fork. There is exactly one turn loop, not three. It emits
`coco_types::CoreEvent` over one `mpsc::Sender` with 3-layer dispatch
(Protocol/Stream/Tui), so SDK/TUI/server all consume the same event
stream (`docs/coco-rs/crate-coco-query.md:144-180`).

Turn structure (`engine.rs:741` outer `loop`): top-of-loop cancellation
check → build prompt + reminders → `ApiClient` streaming open → inner
`loop` draining `StreamEvent`s via `tokio::select!` against
`self.cancel.cancelled()` (`:1453-1469`) → reconstruct assistant message
from `turn_snapshot` (preserving per-part `provider_metadata`,
`:1428-1451`) → push to `MessageHistory` → execute tools →
`finalize_turn_post_tools` → check `ContinueReason` → loop. Tool
execution has two paths: streaming (`StreamingToolExecutor` /
`StreamingHandle::commit_flush`, `:2338-2375`, safe tools concurrent /
unsafe queued) or non-streaming `ToolCallRunner` (`:2667-2690`).

### Typed stop-reason state machine (no string-sniffing)

Abnormal stop_reasons are routed by a typed `coco_messages::StopReason`
(8 variants, a re-export of extended `vercel_ai_provider::UnifiedFinishReason`),
set **once** at the provider-adapter seam — there is no string parsing
anywhere (the old `helpers::parse_stop_reason` was deleted). Per
`app/query/CLAUDE.md` "Abnormal stop_reason → synthetic api_error":

- **`ContentFilter`** (unified bucket: Anthropic `refusal`, OpenAI
  `content_filter`, Google `SAFETY`/`RECITATION`) — terminates; retry
  cannot change a policy decision.
- **`ContextWindowExceeded`** (Anthropic extended-context beta) →
  `handle_context_overflow` (reactive compaction). Never escalates
  output budget — raising output can't help when *input* exceeds the
  window.
- **`MaxTokens`** (Anthropic `max_tokens`, OpenAI `length`, Google
  `MAX_TOKENS`) → output-budget recovery: escalate to 64k, then inject a
  resume-nudge up to `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` times.

All three push a synthetic `api_error` assistant message for transcript
provenance. `ContinueReason` (`config.rs`, `engine.rs`) has the variants
`NextTurn`, `ReactiveCompactRetry`, `MaxOutputTokensEscalate`,
`MaxOutputTokensRecovery`, `StopHookBlocking`, `TokenBudgetContinuation`,
`CollapseDrainRetry`.

### Budget

`budget.rs::BudgetTracker` caps 3 continuations, stops at token-budget
exhaustion, stops on diminishing returns (3+ continuations with both
`last_delta_tokens` and `current_delta_tokens` < 500,
`budget.rs:61-62,115-116`), and nudges at 90%. The deltas are tracked
solely for the diminishing-returns stop — never for proactive
compaction.

### Steering — turn-boundary drain only (deliberate)

`CommandQueue` (`command_queue.rs`, `SessionRuntime`-scoped
`Arc<Mutex<Vec<QueuedCommand>>>` + `Notify`) holds messages typed during
streaming (priority `Now`/`Next`/`Later`, FIFO within priority, per-item
`Uuid`). It is drained **only at the turn boundary** by
`drain_command_queue_into_history` (`helpers.rs:59-95`) inside
`finalize_turn_post_tools`, double-wrapped: `wrap_command_text(origin)`
framing prose + `wrap_in_system_reminder(...)` (`helpers.rs:124-143`),
landing as `Message::Attachment(QueuedCommand)`.

coco-rs deliberately does **not** do a mid-turn `Now` drain —
`command_queue.rs:12-16` and the engine document this: an earlier
mid-turn drain inserted a User message between `tool_use` and
`tool_result` and broke Anthropic 400 pairing on non-streaming
providers, so all priorities collapse to the turn-boundary drain. The
`Now` priority enum value still exists (`command_queue.rs:32`).

### Interrupt — hard cancel only

A pure `CancellationToken` is threaded through all layers
(`engine.rs:1453-1469`). Cancel mid-stream drops the stream, the
`JoinSet` aborts inflight safe tools, and the top-of-loop check emits a
`UserInterruption` system message + terminal Turn event. There is no
soft-interrupt / no-cancel injection — interrupt is hard.

### Compaction

The provider-agnostic `services/compact` crate owns
selection/stripping/PTL/boundary only; `app/query` owns model execution,
fork/cache behavior, hooks, and app-state deltas. Exactly three documented
generic strategies (`crate-coco-compact.md`):

1. **Auto / full LLM** — `finalize_turn_post_tools`
   (`engine_finalize_turn.rs:680-708`) checks
   `should_auto_compact` (threshold = `effective_window − 13K`, a pure
   instantaneous comparison, `auto_trigger.rs:106-116`), optionally runs
   count-based `micro_compact` (default off, `micro.rs:62`), then
   SM-first → `try_full_compact` (`engine_compaction.rs:1083`), which runs
   a cache-sharing `ForkLabel::Compact` fork with deny-all tools,
   post-compact file/plan/skill re-injection, observers, and
   PreCompact/PostCompact hooks. **This call is awaited inline**
   (`engine_finalize_turn.rs:688-695`) — the threshold-crossing turn pays
   the summarization round-trip before the next turn proceeds.
2. **Micro** (`micro.rs`) — clears old `COMPACTABLE_TOOLS` results to
   `[Old tool result content cleared]`, keeping the last N `tool_use`
   ids; plus a time-based variant (`evaluate_time_based_trigger`, gap
   > 60min, main-thread only).
3. **Reactive** (`reactive.rs` + `engine_finalize_turn.rs:114-338`) — on
   `prompt_too_long`/`ContextWindowExceeded`, `do_reactive_compact`
   computes a drop target (70% of effective window), then **branches on
   provider capability**: Anthropic queues a one-shot server-side
   `context_management` payload (cache-preserving, no local mutation);
   other providers do `api_microcompact` then `peel_head_for_ptl_retry`
   (drops oldest API-round groups). `peel_head_for_ptl_retry`
   (`reactive.rs:162-193`) returns the surviving Arc slice **directly**,
   with no summary of what was dropped; the synthetic `CompactResult`
   boundary marker carries token counts only (`engine_finalize_turn.rs:288-304`,
   `raw_summary: None`, `summary_messages: Vec::new()`).
   `ReactiveCompactState` is a circuit-breaker that trips after 3
   consecutive failures.

### Token estimation

`MessageHistory` keeps a `LastUsageMarker` (`history.rs:31-50`): the
previous API call's *actual billed total* + a char/4 estimate of only the
tail since the marker (`tokens_with_last_usage`, `:489-497`). The marker
is invalidated by any compaction/clear/rewind/in-place rewrite
(`invalidate_last_usage`, `:489-501`) — the same dirty-on-mutation
contract as jcode's `active_message_chars_dirty`, but anchored on a real
billed count rather than a chars/4 estimate of the whole prefix.

### Tool-pairing safety

`synthesize_missing_tool_results` (`normalize.rs:532`) forward-synthesizes
missing `tool_results` before the API call (the analog of jcode's
`repair_missing_tool_outputs`); `normalize_messages_for_api` runs 7
ordered passes incl. orphan-thinking filtering. The
`safe_compaction_cutoff` equivalent is implicit in
`group_messages_by_api_round`. Strip passes (`StripImages`,
`StripReinjectedAttachments`) run via the `MessagePass` pipeline with a
fast path (no clone when nothing mutates).

### Documented non-goals honored

No HISTORY_SNIP / CONTEXT_COLLAPSE runtime (staged inert), no ULTRAPLAN,
provider concerns in `vercel-ai-*` crates, compaction is exactly the 3
generic strategies.

---

## Head-to-head comparison

| Dimension | jcode | coco-rs | Verdict |
|---|---|---|---|
| Turn-loop count | 3 near-duplicate bodies (~3,400 LoC) | 1 unified loop + 3-layer `CoreEvent` | **coco-rs** (maintainability) |
| Stop-reason classification | substring-match ~12 phrases + `contains("length"\|"trunc"\|…)` | typed 8-variant `StopReason`, no string parsing | **coco-rs** (robust across providers/locales) |
| Mid-turn steering | soft interrupt, 3 API-safe inject points, urgent abort with stub results | turn-boundary drain only (deliberate) | **jcode richer** (UX); coco-rs sidesteps a provider constraint |
| Interrupt granularity | per-tool detach + reload-resume; per-tool grace window | whole-turn `CancellationToken`; `JoinSet` abort | **jcode better** for persistent-server mode |
| Full-compaction latency | background `tokio::spawn`, deferred swap-in | inline-awaited in `finalize_turn_post_tools` | **jcode better** (hides latency); coco-rs partly offset by cache-sharing fork |
| Compaction triggers | reactive + EWMA proactive + semantic topic-shift | reactive-to-threshold only | **jcode richer**; semantic collides with coco-rs non-goal |
| Overflow recovery | hard-compact ladder + content-mining breadcrumb | `peel_head` drops groups, no breadcrumb | **jcode more graceful** |
| Text-wrapped tool-call recovery | `to=functions.NAME{…}` salvage | none | **jcode** (in-scope gap for OpenAI-compat) |
| Token estimator | `active_message_chars` (chars/4) | `LastUsageMarker` (billed + tail) | **Tie** (coco-rs arguably more accurate) |
| Async drain pattern | `AtomicBool`+`Notify` zero-spin | `select!` on `CancellationToken` | **Tie** (both correct, no busy-wait) |
| Provider-coupling of compaction | manager calls embedding/config/session/provider | pure crate, capability-branched at the query layer | **coco-rs** (testable, layered) |
| Cache-aware reactive path | only OpenAI `encrypted_content` passthrough | Anthropic server-side `context_management` (no local mutation) | **coco-rs** |

The substantive jcode advantages cluster in steering/interrupt
expressiveness (soft interrupt, per-tool detach), latency-hiding
(background full compaction), trigger sophistication (EWMA/semantic), and
overflow graciousness (breadcrumb). The substantive coco-rs advantages
cluster in maintainability (single loop), correctness across providers
(typed stop reasons, layered compaction, cache-preserving reactive path),
and observability.

---

## Where coco-rs already matches or wins

1. **Single unified turn loop vs jcode's triplication.** coco-rs has
   exactly one `run_session_loop` serving main/SDK/subagent/fork
   (`engine.rs:741`). jcode maintains three near-identical ~1000-1300 LoC
   loop bodies; the broadcast and mpsc injection-point/tool-exec code is
   verified line-for-line duplicated (broadcast `:824-948` vs mpsc
   `:909-948`). Any fix to soft-interrupt or tool handling must land in
   three places in jcode. **coco-rs wins on engineering.**

2. **Typed `StopReason`, no string-sniffing.** coco-rs sets one 8-variant
   enum at the provider-adapter seam and matches on it (`engine.rs:1443`,
   `app/query/CLAUDE.md`). jcode classifies context-limit errors by
   substring-matching ~12 English phrases (`agent/compaction.rs:90-104`)
   and continuation by `stop_reason.contains("incomplete"|"length"|…)`
   (`response_recovery.rs:108-118`). coco-rs is more robust across
   providers and locales and won't silently miss a reworded error.
   (Caveat: this same typed approach is also the root of the M07-S7 gap
   below — see recommendations.)

3. **Multi-provider compaction layering and a cache-aware reactive
   path.** coco-rs's reactive compaction branches on
   `supports_server_side_context_edits()` and, on Anthropic, queues a
   cache-preserving server-side `context_management` payload instead of
   mutating messages (`engine_finalize_turn.rs:197-223`,
   `crate-coco-compact.md` "Multi-Provider Strategy"). jcode's reactive
   recovery always mutates locally (hard compact / drop), invalidating
   the prompt cache; its only cache-aware path is the OpenAI
   `encrypted_content` passthrough, and it bakes provider strings into
   the manager (`agent/compaction.rs:230-244` lowercases the provider
   name to decide cache accounting). **coco-rs is cleaner and more
   cache-efficient.**

4. **Provider-agnostic, pure compaction crate.** `services/compact` takes
   a typed `summarize_fn` callback and config refs, never inspects a
   provider, and shares the `MessagePass` pipeline with a clone-free fast
   path. jcode's `CompactionManager` directly calls
   `crate::embedding::embed`, `crate::config::config()`,
   `crate::session::StoredCompactionState`, and `provider.native_compact`
   (`compaction.rs:1255-1517`) — far more coupled and harder to test in
   isolation. **coco-rs wins on testability/layering.**

5. **Token-estimation parity (jcode is NOT faster here).** Both keep an
   O(tail) incremental estimator dirtied on any mutation: jcode
   `active_message_chars` + `_dirty` (`compaction.rs:204-211`), coco-rs
   `LastUsageMarker` + `invalidate_last_usage` (`history.rs:489-501`).
   coco-rs's marker is arguably *more accurate* because it anchors on the
   provider's actual billed total from the last call, not a chars/4
   estimate of the whole prefix. Any "jcode is leaner at context
   accounting" implication does not hold.

6. **Richer, safer post-compact reconstruction.** coco-rs re-injects
   files (5 files / 50K budget), plan, skills, plan-mode reminder,
   async-agent reminders, SessionStart hook output, and runs an observer
   registry + Pre/PostCompact hooks after compaction
   (`crate-coco-compact.md` "QueryEngine Integration",
   `engine_finalize_turn.rs`). jcode's post-compaction work is limited to
   swapping the summary block and resetting cache/session state
   (`agent/compaction.rs:4-9`). **coco-rs preserves working context far
   more faithfully.**

7. **The "14ms / 245×" headline is not an agent-loop property.**
   README:183-211 is a PTY time-to-first-frame *startup* measurement,
   irrelevant to this module. Both harnesses stream incrementally and
   both gate on async signals without spinning. There is no per-turn
   latency advantage substantiated for jcode.

8. **Single-writer observability invariant.** coco-rs's `MessageHistory`
   enforces that every transcript mutation emits a wire event (I-1
   invariant, `history.rs:72-75`) so TUI/SDK stay coherent; tool progress
   is fanned out + throttled per TS parity (`engine.rs:609-644`). jcode
   publishes ad-hoc `Bus::global()` events with no such single-writer
   invariant.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions whose adversarial verdict was **confirmed** or
**nuanced** are listed. For nuanced items the correction is folded in.
All six respect coco-rs's documented non-goals.

### M07-S1 — Streaming-only mid-turn steering injection (no-cancel soft interrupt) [nuanced]

**Why.** jcode injects a user message mid-generation without cancelling,
at API-safe points: Point B for no-tools (`turn_streaming_mpsc.rs:849-863`),
Point C urgent-abort that writes stub `tool_results` for each remaining
tool before injecting (`:911-948`), Point D after a tool batch
(`:895-905`). The queue is genuinely lock-decoupled
(`jcode-agent-runtime/src/lib.rs:20`, pushed via `interrupts.rs:122-152`).
coco-rs removed mid-turn `Now` drain on purpose — it ran *before* the
non-streaming `ToolCallRunner` produced `tool_results`, inserting a User
message between `tool_use` and `tool_result` (Anthropic 400). Drain is
now turn-boundary-only (`command_queue.rs:12-16`,
`helpers.rs:59-95`); interrupt is hard-cancel only
(`engine.rs:1453-1469`).

**Correction folded in (verdict: nuanced).** The win is **not** "avoid
full-context re-send" — both harnesses re-send the full history on every
continuation (jcode `messages_for_provider` is called inside the loop).
The genuine, narrower benefit is **avoiding the hard cancel-and-restart**
that discards the in-progress assistant generation and pays a fresh,
wasted round-trip on every steer.

**Concrete change (`app/query`).** Add a *streaming-path-only*
Point-D-equivalent drain: after the `commit_flush` loop completes
(`engine.rs:2338-2390`, where all `ordered_messages` are already in
history so `tool_use`/`tool_result` adjacency is satisfied), drain
`Now`-priority queued commands. Never run it on the non-streaming
`ToolCallRunner` path. Reuse the existing double-wrap
(`wrap_command_text` + `wrap_in_system_reminder`) so injected items
render identically to the boundary drain. This adds a second drain *site*,
not a new priority (`Now` already exists at `command_queue.rs:32`). Gate
behind a new `Feature` variant (gate site `features.rs:64`).

**Impact: high. Effort: high. Risk:** must *never* inject between a
`tool_use` and its `tool_result` on providers that enforce pairing — keep
it strictly at the post-batch streaming point. **Non-goal check:** honors
the documented non-goal, which is specifically about non-streaming
`tool_use`/`tool_result` pairing; the streaming post-batch point is
already pairing-safe.

### M07-S2 — Background (non-blocking) full LLM compaction with deferred swap-in [confirmed]

**Why.** jcode spawns LLM summarization on a `tokio::task` and the user
keeps working (`maybe_start_compaction_with`, `compaction.rs:838-856`);
the finished summary is swapped in non-blockingly at the next prompt
build (`check_and_apply_compaction_with`, `:996-1106`), with concurrency
handled by advancing `compacted_count` clamped against the caller-owned
vec (`:1033-1036`). coco-rs's `try_full_compact` is awaited **inline** in
`finalize_turn_post_tools` (`engine_finalize_turn.rs:688-695`;
`engine_compaction.rs:1083-1092` is a plain `.await`-ed `async fn`) — the
threshold-crossing turn pays the round-trip synchronously. (The two
`tokio::spawn`s in `engine_finalize_turn.rs` — tool-use-summary fork at
`:974`, prompt-suggestion fork at `:1130` — are unrelated side-forks, not
compaction.)

**Concrete change (`app/query`).** Add an optional async compaction handle
to `QueryEngine`: when `should_auto_compact` fires but tokens are still
below the blocking/critical limit, spawn `try_full_compact` (via the
existing `ForkLabel::Compact` fork) on a task and continue the turn; at
the next `finalize_turn_post_tools`/prompt-build, poll the handle and
apply the boundary+summary swap if finished. Keep the synchronous inline
path only for the ≥ blocking-limit case (the analog of jcode's
`CRITICAL_THRESHOLD` sync hard-compact). Lives entirely in `app/query`.

**Impact: high. Effort: high. Risk:** concurrency with mid-flight history
mutation — the swap must reconcile against messages appended while
summarizing. jcode keys on `compacted_count` against a caller-owned vec;
coco-rs would key on the `LastUsageMarker` anchor and must reset the
cache-break baseline + observers on apply. **Non-goal check:** clean — it
is a scheduling change to the existing full strategy, not a new strategy
or provider coupling.

### M07-S3 — Proactive EWMA token-growth compaction trigger [confirmed]

**Why.** jcode projects per-turn token growth forward and compacts
*before* the threshold (`should_compact_proactively`,
`compaction.rs:450-484`: EWMA of deltas with `ewma_alpha`, project
`current + ewma_delta * lookahead_turns` vs 80% threshold), guarded by
`anti_signals_block` (`:399-441`). coco-rs's `should_auto_compact`
(`auto_trigger.rs:106-116`) is a pure instantaneous threshold with no
token-history window or projection. `BudgetTracker` tracks deltas
(`budget.rs:61-62`) but only for the diminishing-returns stop, never for
proactive compaction (grep confirms no `token_history`/`ewma`/`lookahead`
in `services/compact` or `app/query`).

**Concrete change (`services/compact` + `app/query`).** Add an opt-in
pure fn `should_compact_proactively(&[token_samples], cfg)` to
`services/compact`, fed by a small rolling token-history window maintained
on `QueryEngine` from `usage.input_tokens.total` each turn. Reuse the
anti-signal-style guards. **Port the field names verbatim** from
`jcode-config-types/src/lib.rs:245-260`: `lookahead_turns=15`,
`ewma_alpha=0.3`, `proactive_floor=0.40`, `min_samples=3`,
`stall_window=5`, cooldown `min_turns_between_compactions=10`. **Defaults
OFF** to preserve TS-parity behavior. Frame it explicitly as a smarter
*trigger* for the existing full/auto strategy — **not** a 4th strategy —
so it stays inside the documented micro/full/reactive taxonomy. Do **not**
port jcode's semantic/embedding mode (`compaction.rs:497-537`); it depends
on the embedding subsystem that coco-rs exposes only as a non-default
`Feature`, and a similarity-based trigger collides with the "3 generic
strategies, no embedding-in-compaction" posture.

**Impact: medium. Effort: medium. Risk:** a mis-tuned EWMA could compact
too early and waste a summarization call — gate behind a setting and keep
the cooldown. **Non-goal check:** clean *as a trigger*; the semantic mode
is explicitly excluded as a non-goal collision.

### M07-S4 — Content-mining breadcrumb summary on reactive head-drop [confirmed]

**Why.** jcode's emergency hard-compact builds a summary that extracts
tool names and file paths from dropped messages
(`build_emergency_summary_text`,
`jcode-compaction-core/src/lib.rs:372-415`; `collect_emergency_summary_hints`
`:417-433`; `extract_file_mentions` `:435-444`), so the model isn't blind
to what was removed — and `looks_like_file_reference` (`:446-458`) is
already conservatively extension-allowlisted, excluding `http`. coco-rs's
`peel_head_for_ptl_retry` (`reactive.rs:162-193`) returns the surviving
slice with **no** summary of dropped content; the synthetic
`CompactResult` boundary marker is just
`create_compact_boundary_message(pre_tokens, post_tokens)` —
token counts only, `raw_summary: None` (`engine_finalize_turn.rs:288-304`).
The model is left blind to what was peeled.

**Concrete change (`services/compact` + `app/query`).** Add a pure helper
`build_dropped_breadcrumb(dropped: &[Arc<Message>]) -> String` mirroring
jcode's tool-name + file-mention extraction (no LLM call). In
`do_reactive_compact`, prepend it into the existing
`create_compact_boundary_message` text at `engine_finalize_turn.rs:289`
(rather than a separate system message, so it rides the existing
boundary-marker insertion and doesn't perturb the message-count invariants
the reactive observers assume). **Reuse coco-rs's existing
`coco_compact::extract_discovered_tool_names`** (confirmed at
`services/compact/src/types.rs:309`, exported `lib.rs:122`) for the "Tools
used" half, and keep the file-mention regex extension-allowlisted exactly
like jcode (`.rs`/`.ts`/`.py`/`.toml`/`.json`) to bound secret leakage.

**Impact: medium. Effort: low. Risk:** low — additive, no change to what
is dropped; only adds a marker. Keep the regex conservative. **Non-goal
check:** clean — additive, no provider coupling.

### M07-S5 — Text-wrapped tool-call recovery for OpenAI-compatible providers [confirmed]

**Why.** jcode salvages tool calls the model emits as plain text
`to=functions.NAME{json}` (`response_recovery.rs:4-100`), a real failure
mode on OpenRouter/Kimi/Groq-style endpoints, gated on
`tool_calls.is_empty() && !text.trim().is_empty()` (`:61`) and preferring
an empty-suffix parse (`:45-47`). coco-rs reconstructs assistant content
purely from the typed `turn_snapshot` at `StreamEvent::Finish`
(`engine.rs:1428-1451`); grep across `app/query`, `services/inference`,
and `vercel-ai` found **zero** `to=functions`/text-wrapped recovery.
coco-rs explicitly targets generic OpenAI-compatible providers
(root `CLAUDE.md`: xAI/Groq/Together via `vercel-ai-openai-compatible`),
so this failure mode is in scope.

**Concrete change (`app/query`, capability-gated).** Add a recovery pass
when `tool_calls.is_empty()` (the branch at `engine.rs:2395`): if
`response_text` contains a `to=functions.<name>{...json...}` marker, parse
it into a synthetic `ToolCall` and emit the corresponding
`ToolUseQueued`/`Started` stream events. Bound strictly to
empty-`tool_calls` + empty-suffix like jcode to avoid false positives on
prose.

**Correction folded in (gating mechanism).** coco-rs has no per-call
`ProviderApi` string at `engine.rs:2395` to branch on directly — **gate
via a model `Capability`** instead. The engine already reads
`info.has_capability(...)` for `ServerSideToolReference`/`ClientSideToolSearch`
(`engine.rs:2646,2652`), so declare a new
`Capability::TextWrappedToolCallRecovery` only on OpenAI-compatible model
cards; first-party Anthropic and OpenAI-Responses then never run the scan.
(Per `services/inference/CLAUDE.md`, a `synthetic_stream_from_content`-
adjacent collector or a normalize pass is an alternative home, but the
`engine.rs:2395` site is acceptable since the typed snapshot is the
history source.)

**Impact: medium. Effort: medium. Risk:** false-positive recovery if a
model legitimately writes that token in prose — the empty-`tool_calls` +
empty-suffix bound plus the per-capability gate contain this. **Non-goal
check:** clean — directly serves the in-scope OpenAI-compatible target.

### M07-S6 — Per-tool background detach + reload-resumable interrupted results [nuanced]

**Why.** jcode spawns each tool on its own `tokio::spawn`
(`turn_streaming_mpsc.rs:1021-1025`) and `select!`s
(`biased`) tool completion against `background_tool_signal`/`graceful_shutdown`
(`:1037-1068`) with a 750ms `bash` handoff grace on shutdown (`:1051-1063`).
Alt+B transfers the running handle to a background registry
(`:1178-1180`, `background.rs:459-503`) and injects a `bg`-tool handle
`tool_result` so the turn continues (`:1182-1205`).
`reload_interrupted_tool_result` (`:3-34`) returns non-error resumable
messages for wait-like tools. coco-rs's `StreamingToolExecutor` aborts
inflight tools on cancel (`JoinSet::shutdown`,
`executor_streaming.rs:163-173`, `engine.rs:1455-1466`) with no per-tool
"move to background and keep the turn alive" and no reload-resume
semantics.

**Correction folded in (verdict: nuanced — coco-rs has a partial
primitive).** Don't build a brand-new detach signal from scratch: coco-rs
*already* has a detach primitive for task-spawned **agent/shell** tasks —
`agent_handle.rs:193-209` (`auto_background_ms` + `AsyncLaunched`) and
`task_handle.rs:36-48,326` (`signal_detach`/`DetachOutcome`), keyed by
`task_id` and owned by `TaskRuntime`. The genuinely-absent mechanism is
lifting an *arbitrary in-flight foreground tool* (e.g. `Bash`) out of
`executor_streaming`'s shared `JoinSet` (`:97-99`) and handing it to
`TaskRuntime`.

**Concrete change (`core/tool-runtime` + `app/query`).** *Extend* the
existing `signal_detach`/`DetachOutcome` + `AsyncLaunched` path to also
cover a foreground tool running in the streaming `JoinSet`. The hard new
work is `JoinSet → TaskRuntime` handle handoff (coco-rs's `JoinSet` owns
the future; jcode owns a movable per-tool `JoinHandle`). Pair this with a
graceful-shutdown variant of the cancel path that, for wait-like/bg tools,
writes a typed *resumable* `tool_result` instead of a bare abort.

**Impact: low. Effort: high. Risk:** significant tool-ownership-transfer
complexity for a feature only valuable in long-lived serve/connect mode
(concurrent-app-server WIP); must not regress the simple
`JoinSet::shutdown` cancel path. **Recommendation: defer** unless
serve/connect parity becomes a goal.

### M07-S7 — Continuation on the full abnormal-stop_reason family (verifier missed-finding) [strong]

**Why.** jcode auto-continues on *any* abnormal stop_reason — `incomplete`,
`length`, `trunc`, `commentary`, `max_output_tokens`, `max_tokens` — capped
at `MAX_INCOMPLETE_CONTINUATION_ATTEMPTS`, injecting a "continue exactly
where you left off" nudge (`response_recovery.rs:102-167`). coco-rs only
runs output-budget recovery for `StopReason::MaxTokens`
(`engine.rs:2205-2278` and `app/query/CLAUDE.md` branch 3). The risk: an
OpenAI-compatible provider that reports `length`/`incomplete` in a way
that does **not** map cleanly onto the unified `MaxTokens` bucket at the
adapter seam will silently *end* the turn rather than continue. This is
the flip side of the typed-`StopReason` win (#2 above): the typed enum is
robust *only if the provider adapter maps the wire reason correctly*; an
unmapped reason becomes a clean terminal stop with no continuation.

**Concrete change (`vercel-ai-openai-compatible` adapter + `app/query`).**
First, audit the OpenAI-compatible finish-reason mapping at the adapter
seam to confirm `length`/`incomplete`/truncation variants land on
`UnifiedFinishReason::MaxTokens` (the cleanest fix — keep the typed
boundary authoritative). Where a provider emits a reason that genuinely
cannot be classified, surface it as `MaxTokens` (output truncation) so the
existing recovery in `engine.rs:2205-2278` fires. Avoid re-introducing
substring matching in `app/query` — keep all reason interpretation at the
provider-adapter seam, consistent with coco-rs's "set once, no string
parsing" rule.

**Impact: medium. Effort: low-medium. Risk:** low — it tightens an
existing typed mapping rather than adding a new code path. **Non-goal
check:** clean and *reinforces* a non-goal (provider concerns live in
`vercel-ai-<provider>`; reason classification stays at the seam).

### M07-S8 — Proactively drop truncation-mangled (`null`-input) tool calls (verifier missed-finding) [moderate]

**Why.** jcode's `filter_truncated_tool_calls` (`response_recovery.rs:169-198`)
retains only `!input.is_null()` tool calls when the response was
truncated, and `remove_tool_use_blocks` repairs the persisted assistant
message — so a half-streamed, malformed `tool_use` is never executed.
coco-rs maps empty input to `{}` and lets schema validation reject it
later (`services/inference/CLAUDE.md`), but never *proactively* drops a
truncation-mangled `tool_use` before execution. The two behaviors usually
converge (both reject), but jcode avoids the wasted execution attempt and
the repaired-transcript path that keeps the persisted assistant message
consistent.

**Concrete change (`app/query`, scoped to the `MaxTokens` branch).** When
the turn finished on `StopReason::MaxTokens` and a reconstructed tool call
has null/empty input, drop it before dispatch (and repair the stored
assistant content), instead of forwarding a `{}`-input call into schema
validation. Scope strictly to the truncation branch to avoid touching the
normal-completion path.

**Impact: low-medium. Effort: low. Risk:** low — only changes behavior on
truncated turns; keep it gated on `MaxTokens` so a legitimately
zero-argument tool on a clean turn is unaffected. **Non-goal check:**
clean — a normalize/recovery refinement, no provider coupling.

---

## Rejected after adversarial review

No M07 suggestion was outright **refuted** — all six analyst suggestions
(M07-S1 … M07-S6) survived adversarial review as **confirmed** (S2, S3,
S4, S5) or **nuanced** (S1, S6), and are carried above with corrections
folded in. Two verifier missed-findings were promoted to recommendations
(M07-S7, M07-S8).

The following jcode mechanisms were considered and **deliberately not
recommended for port**, because each collides with a documented coco-rs
non-goal:

- **Semantic / embedding-driven compaction trigger** (jcode Semantic
  mode, `compaction.rs:497-611`) — relies on the embedding subsystem
  (a non-default coco-rs `Feature`) and a cosine-similarity topic-shift
  signal. coco-rs's compaction is intentionally *three generic strategies
  only* (micro / full-LLM / reactive) with no embedding dependency. The
  *EWMA proactive trigger* (M07-S3) was salvaged from this design because
  it is purely token-statistical and reframed as a trigger, not a fourth
  strategy; the embedding-based half is dropped.

- **Provider-string-driven cache accounting in the compaction manager**
  (jcode `agent/compaction.rs:230-244` lowercases the provider name)
  and **OpenAI `encrypted_content` native-compaction passthrough**
  (`agent/compaction.rs`) — these are provider concerns. coco-rs's
  layering rule keeps all provider-specific cache/compaction logic in the
  `vercel-ai-<provider>` crates, and its reactive path already does the
  *correct* version of this for Anthropic via the capability-gated
  server-side `context_management` payload (see "Where coco-rs already
  wins" #3). Porting jcode's provider-string branching into
  `services/compact` would regress coco-rs's provider-agnostic crate
  boundary.

- **Self-dev soft-interrupt disk persistence** (jcode
  `soft_interrupt_store.rs:91-107`, surviving a binary rebuild) — only
  meaningful for jcode's self-dev / long-lived serve loop. coco-rs's
  `CommandQueue` is in-memory and wiped on clear (`command_queue.rs`);
  cross-reload persistence is relevant only to serve/connect parity and is
  folded into the deferred M07-S6 (persistent-server) track, not a
  standalone recommendation.
