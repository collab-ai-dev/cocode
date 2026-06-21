# coco-query

Multi-turn agent loop driver. Orchestrates the full turn cycle: prompt build,
LLM call, tool execution, compaction, command-queue drain, budget/continue
decisions. Emits `coco_types::CoreEvent` directly (no intermediate event enum).

## Key Types

| Type | Purpose |
|------|---------|
| `QueryEngine` | Orchestrator: owns tool/command registries, model runtime registry, state |
| `QueryEngineConfig` | max_turns, total_token_budget (session cap), permission_mode, context_window, streaming_tool_execution, bypass_permissions_available, fallback_model, plan_mode_settings. Per-call `max_output_tokens` lives on `ModelInfo`, not here. |
| `QueryResult`, `ContinueReason` | Loop control: `NextTurn`, `ReactiveCompactRetry`, `MaxOutputTokensEscalate`, `MaxOutputTokensRecovery`, `StopHookBlocking`, `TokenBudgetContinuation`, `CollapseDrainRetry` |
| `SessionBootstrap` | Initial system prompt, messages, cost tracker |
| `BudgetTracker`, `BudgetDecision` | Token budget; 3-continuation cap, 90% threshold, diminishing-returns stop |
| `CommandQueue`, `QueuedCommand`, `QueuePriority`, `QueueOrigin` | `Now`/`Next`/`Later`; FIFO within priority; per-item `Uuid` for id-based removal; `Human`/`Coordinator`/`TaskNotification`/`Channel` origin drives framing prose |
| `StreamAccumulator` | `AgentStreamEvent` → `ServerNotification::ItemStarted/Updated/Completed` with `ThreadItem` tool mapping |
| `agent_adapter::*` | Bridges `QueryEngine` to tool invocations and subagent spawn callbacks |
| `plan_mode_reminder::*` | Plan-mode steady-state reminder cadence (Full/Sparse/Reentry) |
| `single_turn::*` | One-shot turn execution (no loop) |
| `emit::*` | `CoreEvent` emission helpers |

## Turn Lifecycle

```
1.  Build system prompt (context)                  [coco-context]
2.  Normalize messages for API                     [coco-messages]
3.  ModelRuntime.open_stream(QueryParams)          [coco-inference]
4.  Parse response; extract tool calls             [engine.rs]
5.  ToolExecutor: safe concurrent / unsafe queued  [coco-tool-runtime]
6.  HookRegistry PreToolUse / PostToolUse          [coco-hooks]
7.  Tool results → MessageHistory                  [coco-messages]
8.  Check ContinueReason
       - NextTurn / TokenBudgetContinuation → loop
       - ReactiveCompactRetry → compact then retry [coco-compact]
       - MaxOutputTokensEscalate (per-model ceiling via `ModelInfo.max_output_tokens_escalate`) / Recovery / StopHookBlocking / CollapseDrainRetry
9.  Drain CommandQueue → attachment messages (User w/ system-reminder wrap)
10. Goto 1 if tools remain; else emit TurnEnded(Completed)
```

## Emitted CoreEvent Variants

Protocol: `TurnStarted` (runner-emitted, once per cycle — see
`engine_session.rs`), `TurnEnded` (discriminated outcome:
`Completed`/`Failed`/`Interrupted`/`MaxTurnsReached`/`BudgetExhausted`),
`CompactionStarted`, `ContextCompacted`, `Error` (budget nudge),
`QueueStateChanged`, `CommandQueued`, `CommandDequeued`.

**Cycle TurnId contract.** Runners (`tui_runner`, `sdk_runner`,
`QueryEngineRunner`, harnesses) generate one `TurnId::generate()` per
user-prompt cycle and pass it into `engine.run_with_messages` /
`run_with_events`. `engine_session::run_internal_with_messages` emits
`TurnStarted` with that id; every internal `TurnEnded` emission inside
the engine and the engine_session error path reuses the same id. The
per-round `turn_id` (`format!("turn-{n}")`) survives only as a log
correlation field — it never reaches the wire.
Stream: `TextDelta`, `ThinkingDelta`, `ToolUseQueued`, `ToolUseStarted`,
`ToolUseCompleted`.
(See `docs/coco-rs/event-system-design.md` for full catalog.)

## Steering (Mid-Turn Injection)

Users can type while the LLM is working. In coco-rs the queue is
**`SessionRuntime`-scoped** (`runtime.command_queue`) because `QueryEngine` is
rebuilt per turn — `SessionRuntime::wire_engine` calls
`engine.with_command_queue(self.command_queue.clone())` so every turn observes
the same `Arc`-shared queue.

**Enqueue path.** While streaming, the TUI dispatch (`tui_runner`) routes typed
input through `UserCommand::QueueCommand`, which constructs a `QueuedCommand`
(default priority `Next`, origin `Human`), pushes it onto the runtime queue,
and emits `ServerNotification::CommandQueued { id, preview }`. Each item has
a `Uuid` for id-based removal.

**Drain path.** At turn boundaries (after a turn finishes, before the next API
request), `engine_finalize_turn` calls `drain_command_queue_into_history`. Each
queued item becomes one `Message::Attachment(AttachmentKind::QueuedCommand)`
carrying a User-role LLM message. The body is **double-wrapped**:

1. `wrap_command_text(prompt, origin)` — origin-specific framing prose
   (e.g. "The user sent the following message while you were working:").
2. `wrap_in_system_reminder(...)` — outer `<system-reminder>` XML tags.

Attachments are API-visible (`AttachmentMessage::api`) so they render in the
transcript and reach the model on the next turn. The `messages::normalize`
pass `smoosh_system_reminder_into_tool_result` then folds the wrapped User
message into the preceding Tool message when present, preserving Anthropic's
strict tool_use/tool_result adjacency.

**No mid-turn `Now` drain.** Interleaving `Now`-priority items mid-turn is
intentionally unsupported — it would break tool_use/tool_result pairing on
non-streaming providers. All priorities are honored at the turn-boundary drain
in FIFO-within-priority order.

**Clear semantics.** `SessionRuntime::clear_conversation` is a full reset and
wipes the queue so in-flight queued commands from the pre-clear session cannot
surface in the post-clear transcript.

**E2E coverage.** `app/query/tests/steering.rs` runs a real `QueryEngine`
against a mock model: a producer task enqueues during turn 1, the test asserts
the wrapped attachment lands in history, the second turn's prompt contains the
steering marker, lifecycle events fire (`CommandQueued` → `CommandDequeued`),
and the final response references the marker (proving the model acted on it).

## Forks vs Subagents vs Main Loop

Three distinct spawn paths share the same `query()` engine. They differ
in **who invokes**, **what state isolates**, and **how the result
surfaces**:

- **Main loop** — user-facing session. Owns `MessageHistory`, the
  cache slot, `ToolAppState`, `CommandQueue`. Persistent across turns.
- **Fork** (`forked_agent.rs`, dispatched by
  `app/cli::fork_dispatcher`) — fire-and-forget side query that
  **shares the parent's prompt cache** via `CacheSafeParams`. 9
  variants enumerated by `coco_types::ForkLabel`:
  `prompt_suggestion`, `side_question`, `compact`, `extract_memories`,
  `session_memory_{auto,manual}`, `agent_summary`, `auto_dream`,
  `speculation`. Lifecycle: dispatch → run → return result → die.
  Never mutates parent transcript. Uses `ForkContextOverrides`
  (in `fork_context.rs`) for per-call isolation: auto agent_id,
  fresh `DenialTrackingState`, fresh `query_chain_id` + `query_depth`
  bump (capped at 16), `allowed_write_roots` fence, `require_can_use_tool`
  toggle.
- **Subagent** (`AgentTool` model-spawned via
  `coco_tool_runtime::AgentHandle`) — full multi-turn child engine,
  may run for hours, lives in `task_runtime`. Different cache
  contract: child has its own cache key. Inherits permission rules
  but builds fresh `MessageHistory`.

Forks are **structurally subagents** (same `createSubagentContext`-equivalent
isolation primitive) but framework-spawned (post-turn / timer / slash)
rather than model-spawned. Per-fork tool gating goes through the
`CanUseToolHandle` callback at `core/tool-runtime/src/execution.rs`
step 3.5.

### `ForkedAgentOptions::for_label` cache-safe defaults

The conservative shape preserves the parent's prompt cache:
`max_turns=Some(1)`, `transcript_mode=Disabled`, `skip_cache_write=true`,
`effort=None`, `max_output_tokens=None`. **Do not set
`max_output_tokens`** on cache-shared forks — PR #18143 incident:
`effort: 'low'` dropped cache hit rate from 92.7% → 61% (45× spike
in cache writes) by changing `budget_tokens`. The inference layer
logs `tracing::warn!` when this field is `Some` so any regression
leaves a trail.

### promptSuggestion 9-step guard + 12-rule filter

Post-turn promptSuggestion runs through `prompt_suggestion::try_generate_suggestion`:

1. abort check (singleton in caller's hands)
2. `assistant_turn_count < 2` ⇒ `TooFewTurns`
3. last response was API error ⇒ `ApiError`
4. `parent_uncached_tokens > MAX_PARENT_UNCACHED_TOKENS (10_000)` ⇒ `CacheCold`
5. `get_suggestion_suppress_reason` (7 reason variants)
6. `generate_suggestion` (forks via `runForkedAgent` equivalent)
7. abort recheck
8. empty / `NONE` ⇒ `Empty`
9. `should_filter_suggestion` — 12 rules: `Done`, `MetaText`,
   `MetaWrapped`, `ErrorMessage`, `PrefixedLabel`, `TooFewWords`
   (with 17-word `ALLOWED_SINGLE_WORDS` bypass), `TooManyWords`,
   `TooLong`, `MultipleSentences`, `HasFormatting`, `Evaluative`,
   `ClaudeVoice`. Each rule has byte-faithful regex.

The verbatim `SUGGESTION_PROMPT` (30 lines) lives at
`prompt_suggestion_prompt.txt` and is `include_str!`'d.

### Abnormal stop_reason → synthetic `api_error` assistant message

When the LLM stream finishes with a non-clean `stop_reason`,
`engine.rs::run_session_loop` synthesizes a typed-signal assistant
message via `helpers::build_abnormal_stop_api_error_message`. The message
has empty content and carries the human-readable explanation on
`AssistantMessage.api_error.message`.

Three abnormal-stop branches feed this synthesizer:

1. **`StopReason::ContentFilter`** (multi-LLM unified bucket — Anthropic
   `refusal`, OpenAI `content_filter`, Google `SAFETY` / `RECITATION`).
   No recovery — retry won't change a policy decision. The engine
   pushes the partial real response + synthetic api_error message
   and falls through to the natural `tool_calls.is_empty()`
   end-of-turn exit. Provider-agnostic message text — does not name
   "Claude" / "Anthropic" / "OpenAI" since the unified bucket covers
   all of them.

2. **`StopReason::ContextWindowExceeded`** (Anthropic-only finish
   reason on the extended-context beta — every other provider reports
   this condition as an HTTP 400). Routes to
   `QueryEngine::handle_context_overflow` (reactive compaction),
   sharing the handler with the HTTP-400 stream-open and mid-stream
   sites so all three context-window signals converge. Pushes the
   partial assistant message + synthetic api_error first (transcript
   provenance), then compacts and continues with
   `ContinueReason::ReactiveCompactRetry`. **Never escalates
   `max_output_tokens`** — raising the output budget cannot help when
   the *input* already exceeds the window.

3. **`StopReason::MaxTokens`** (output-token cap — Anthropic
   `max_tokens`, OpenAI `length` / `max_output_tokens`, Google
   `MAX_TOKENS`). Output-budget recovery: phase 1 retries with the
   active model's opt-in `ModelInfo.max_output_tokens_escalate`
   ceiling (skipped when unset — multi-LLM safety: GPT-4 / Haiku
   would 4xx on a hardcoded 64k); phase 2 injects the resume-nudge
   meta message up to `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` times;
   phase 3 falls through. All three sub-branches push the synthetic
   message so transcripts carry the explicit truncation marker
   regardless of subsequent recovery state. Phase 1
   reads `ModelInfo` from the **post-plan-swap** client so plan-mode
   sessions escalate against the active Plan role, not Main.

Layering: `coco-inference` is provider-agnostic and cannot construct
`coco_messages::Message` — the typed `FinishReason` struct
(`{ unified, raw }`; `unified` = the 8-variant
`vercel_ai_provider::UnifiedFinishReason`, re-exported as
`coco_messages::StopReason`) flows through `StreamEvent::Finish`, where
`engine_stream_consume` logs `.raw` (`Display`, e.g. `other(compaction)`)
and then **projects to `.unified`**. From that seam on, the engine
threads the bare `StopReason` enum — `withhold_reason_for_stop`, the
`ContentFilter` check, and the committed `AssistantMessage` /
`CompletedOutcome` all use the projection, not the struct. There is
**one** stop_reason enum in the workspace, set once at the
provider-adapter seam (Anthropic / OpenAI / Google / ByteDance /
OpenAI-compat); `raw` is a transient in-memory diagnostic on the live
carriers (`QueryResult` / `StreamEvent::Finish`), never persisted. No
string parsing anywhere — the old `helpers::parse_stop_reason` was
deleted as part of the unification.

`ContextWindowExceeded` and `MaxTokens` are deliberately routed to
distinct handlers (compaction vs. output-budget escalate). There is
intentionally no `is_max_tokens_family` umbrella predicate — the two
variants share neither recovery strategy nor the user-facing wording
that `build_abnormal_stop_api_error_message` emits.

### Tool-use-summary side-fork (`ModelRole::Fast`)

After each tool batch `engine_finalize_turn::spawn_tool_use_summary`
optionally spawns a blocking Fast-role call that produces a ≤30-char
mobile-row label. Lives in `tool_use_summary.rs`. Four gates, all
enforced in `spawn_tool_use_summary`:

1. `Feature::ToolUseSummary` enabled — **default off**. Mobile-row UX
   polish; every tool-using turn costs an extra Fast-role blocking
   call, and reasoning-class Fast models (DeepSeek V4, Gemini Flash
   Thinking, …) exhaust the per-call budget on reasoning before any
   visible text — the call returns `stop_reason=length` with empty
   text. Users opt in via `settings.json` `features.tool_use_summary =
   true` once their Fast role is wired to a non-reasoning model.
2. model runtime registry wired (Fast role configured).
3. `agent_id.is_none()` — subagents don't surface in the mobile UI.
4. Tool batch non-empty.

`QueryParams.max_tokens` is intentionally `None` — defer to the Fast
model's own `max_output_tokens` from `ModelInfo`. Hardcoding `64` for
any model is unsafe on reasoning models. Non-clean terminations propagate
through `coco-inference`'s abnormal-stop_reason warn (see
`services/inference/CLAUDE.md`).
