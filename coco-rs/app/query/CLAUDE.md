# coco-query

Multi-turn agent loop driver. Orchestrates the full turn cycle: prompt build,
LLM call, tool execution, compaction, command-queue drain, budget/continue
decisions. Emits `coco_types::CoreEvent` directly (no intermediate event enum).

## Key Types

| Type | Purpose |
|------|---------|
| `QueryEngine` | Orchestrator: owns tool/command registries, model runtime registry, state |
| `QueryEngineConfig` | max_turns, total_token_budget (session cap), permission_mode, streaming_tool_execution, bypass_permissions_available, fallback_model, plan_mode_settings. Per-call `max_output_tokens` lives on `ModelInfo`, not here. |
| `QueryResult`, `ContinueReason` | Loop control: `NextTurn`, `ReactiveCompactRetry`, `MaxOutputTokensEscalate`, `MaxOutputTokensRecovery`, `StopHookBlocking`, `TokenBudgetContinuation`, `CollapseDrainRetry` |
| `SessionBootstrap` | Initial system prompt, messages, cost tracker |
| `BudgetTracker`, `BudgetDecision` | Token budget; 3-continuation cap, 90% threshold, diminishing-returns stop |
| `CommandQueue`, `QueuedCommand`, `QueuePriority`, `QueueOrigin` | `Now`/`Next`/`Later`; FIFO within priority; per-item `Uuid` for id-based removal; `QueueOrigin` drives framing prose. Every mutation advances a watch revision + activity timestamp for event-driven host supervision. |
| `StreamAccumulator` | `AgentStreamEvent` → `ServerNotification::ItemStarted/Updated/Completed` with `ThreadItem` tool mapping |
| `agent_adapter::*` | Bridges `QueryEngine` to tool invocations and subagent spawn callbacks |
| `plan_mode_reminder::*` | Plan-mode steady-state reminder cadence (Full/Sparse/Reentry) |
| `single_turn::*` | One-shot turn execution (no loop) |
| `engine_finalize_turn` / `engine_finalize_tail` | Turn finalization + continuation, memory-reminder, suggestion, rate-limit helpers |
| `engine_compaction` / `engine_compaction_full` | Manual/session-memory entry paths; full-compaction execution |

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
`engine_session.rs`), `TurnEnded` (outcome:
`Completed`/`Failed`/`Interrupted`/`MaxTurnsReached`/`BudgetExhausted`),
`CompactionStarted`, `ContextCompacted`, `Error` (budget nudge),
`QueueStateChanged`, `CommandQueued`, `CommandDequeued`. Stream:
`TextDelta`, `ThinkingDelta`, `ToolUseQueued`, `ToolUseStarted`,
`ToolUseCompleted`. (Full catalog: `docs/internal/event-system-design.md`.)

**Cycle TurnId contract.** Hosts (the local-AppServer `turn/start` handler →
`SessionTurnExecutor` in `app/agent-host`, harnesses) mint one `TurnId` per
user-prompt cycle and pass it into `engine.run_with_messages` /
`run_with_events`. `engine_session::run_internal_with_messages` emits
`TurnStarted` with that id; every internal `TurnEnded` (and the
engine_session error path) reuses it. The per-round `turn_id`
(`format!("turn-{n}")`) is a log correlation field only — never on the wire.

## Tool Input Pipeline

`tool_input_pipeline` is the crate-private boundary for assistant-emitted
tool input: provider wire state → `ToolCallPart` input → observable
normalization → tool lookup → `coco_tool_runtime::ValidatedInput`.
Permission checks, hooks' updated-input re-validation, and tool execution
all receive that proof-carrying newtype, never a bare `serde_json::Value`.
Provider wire parsing stays here / in services-inference; `coco-tool-runtime`
must not depend on provider wire state or command registries.

## Steering (Mid-Turn Injection)

Users can type while the LLM is working. The queue is
**`SessionRuntime`-scoped** (`runtime.command_queue`) because `QueryEngine`
is rebuilt per turn — `SessionRuntime::wire_engine` calls
`engine.with_command_queue(...)` so every turn sees the same `Arc`-shared
queue.

**Enqueue path.** While a turn is active, the TUI driver
(`app/cli/src/tui/driver.rs`) routes typed input through
`UserCommand::QueueCommand` →
`coco_agent_host::session_queue::enqueue_human_prompt` (priority `Next`,
origin `Human`) and emits `CommandQueued { id, preview }`. When idle,
`SubmitInput` starts a fresh turn via the local AppServer bridge's
`turn/start`; a still-busy handler re-enqueues instead of dropping.

**Drain path.** At turn boundaries (turn finished, before the next API
request), `engine_finalize_turn` calls `drain_command_queue_into_history`.
Each item becomes one `Message::Attachment(AttachmentKind::QueuedCommand)`
carrying a User-role LLM message, **double-wrapped**:
`wrap_command_text(prompt, origin)` (origin-specific framing prose) then
`wrap_in_system_reminder(...)` (outer `<system-reminder>` tags). Attachments
are API-visible (`AttachmentMessage::api`): they render in the transcript
and reach the model next turn. The `messages::normalize` pass
`smoosh_system_reminder_into_tool_result` then folds the wrapped User
message into the preceding Tool message when present, preserving Anthropic's
strict tool_use/tool_result adjacency.

**No mid-turn `Now` drain.** Interleaving `Now`-priority items mid-turn is
intentionally unsupported — it would break tool_use/tool_result pairing on
non-streaming providers. The single production drain is the turn-boundary
one, capped at priority `Next`; `Later` items (background task
notifications) drain only when a Sleep tool ran that batch.

**Clear semantics.** `SessionRuntime::clear_conversation` is a full reset and
wipes the queue so pre-clear queued commands cannot surface post-clear.
E2E coverage: `app/query/tests/steering.rs` (real engine + mock model).

## Forks vs Subagents vs Main Loop

Three spawn paths share the same `query()` engine, differing in **who
invokes**, **what state isolates**, and **how the result surfaces**:

- **Main loop** — user-facing session. Owns `MessageHistory`, the cache slot,
  `ToolAppState`, `CommandQueue`. Persistent across turns.
- **Fork** (`forked_agent.rs`, dispatched by
  `app/agent-host/src/integrations/fork_dispatcher.rs`) — fire-and-forget
  side query that **shares the parent's prompt cache** via `CacheSafeParams`.
  Labels enumerated by `coco_types::ForkLabel`. Never mutates parent
  transcript. `ForkContextOverrides` (`fork_context.rs`) gives per-call
  isolation: auto agent_id, fresh `DenialTrackingState`, fresh
  `query_chain_id` + `query_depth` bump (counts toward the shared subagent
  depth cap), `allowed_write_roots` fence, `require_can_use_tool` toggle.
- **Subagent** (`AgentTool` model-spawned via
  `coco_tool_runtime::AgentHandle`) — full multi-turn child engine, may run
  for hours, lives in `task_runtime`. Own cache key. Inherits permission
  rules but builds fresh `MessageHistory`.

Forks are **structurally subagents** (same isolation primitive) but
framework-spawned (post-turn / timer / slash) rather than model-spawned.
Per-fork tool gating goes through the `CanUseToolHandle` callback in
`core/tool-runtime`'s execution pipeline.

### `ForkedAgentOptions::for_label` cache-safe defaults

The conservative shape preserves the parent's prompt cache:
`max_turns=Some(1)`, `transcript_mode=Disabled`, `skip_cache_write=true`,
`effort=None`, `max_output_tokens=None`. **Do not set `max_output_tokens`**
on cache-shared forks — PR #18143 incident: `effort: 'low'` dropped cache hit
rate from 92.7% → 61% (45× spike in cache writes) by changing
`budget_tokens`. Inference logs `tracing::warn!` when the field is `Some`.

Compact summarization tags both the cache-sharing fork and the direct
no-tools fallback as `query_source = "compact"` (shares the main-thread
cache-break key; classifies compact fallback/retry), passes the active
model's context window as `fallback_min_context_window` (capacity fallback
can't pick a smaller-window model), and leaves `thinking_level = None`
(some providers reject an explicit off-toggle).

### promptSuggestion guard + filter

`prompt_suggestion::try_generate_suggestion` runs a 9-step guard (abort
checks, `assistant_turn_count < 2`, last-response API error, cache-cold
`parent_uncached_tokens > MAX_PARENT_UNCACHED_TOKENS` (10_000), suppress
reasons, generate, empty/`NONE`) then `should_filter_suggestion` — a 12-rule
filter with byte-faithful regexes and an `ALLOWED_SINGLE_WORDS` bypass. Rule
details live in `prompt_suggestion.rs`; the verbatim `SUGGESTION_PROMPT` is
`include_str!`'d from `prompt_suggestion_prompt.txt`.

### Abnormal stop_reason → synthetic `api_error` assistant message

On a non-clean `stop_reason`, `engine.rs::run_session_loop` synthesizes an
empty-content assistant message via
`helpers::build_abnormal_stop_api_error_message`, carrying the explanation on
`AssistantMessage.api_error.message`. Three branches feed it:

1. **`StopReason::ContentFilter`** — multi-LLM unified bucket (Anthropic
   `refusal`, OpenAI `content_filter`, Google `SAFETY`/`RECITATION`). No
   recovery — retry won't change a policy decision. Push partial + synthetic
   message, fall through to the natural end-of-turn exit. Message text is
   provider-agnostic (never names a vendor).
2. **`StopReason::ContextWindowExceeded`** — Anthropic-only finish reason on
   the extended-context beta (others report HTTP 400). Routes to
   `QueryEngine::handle_context_overflow` (reactive compaction), the same
   handler as the HTTP-400 stream-open and mid-stream sites. Pushes partial +
   synthetic message first (transcript provenance), then compacts →
   `ContinueReason::ReactiveCompactRetry`. **Never escalates
   `max_output_tokens`** — a bigger output budget can't help when the *input*
   exceeds the window.
3. **`StopReason::MaxTokens`** — output-token cap. Phase 1 retries with the
   model's opt-in `ModelInfo.max_output_tokens_escalate` ceiling (skipped
   when unset — a hardcoded 64k would 4xx on smaller models), read from the
   **post-plan-swap** client so plan-mode sessions escalate against the Plan
   role; phase 2 injects the resume-nudge meta message up to
   `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` times; phase 3 falls through. All three
   push the synthetic message so transcripts carry the truncation marker.

Layering: `coco-inference` is provider-agnostic and cannot construct
`coco_messages::Message`. The typed `FinishReason` `{ unified, raw }`
(`unified` = `vercel_ai_provider::UnifiedFinishReason`, re-exported as
`coco_messages::StopReason`) flows through `StreamEvent::Finish`;
`engine_stream_consume` logs `.raw` then **projects to `.unified`**, and the
engine threads the bare `StopReason` enum from that seam on. There is **one**
stop_reason enum in the workspace, set once at the provider-adapter seam;
`raw` is a transient diagnostic, never persisted; no string parsing anywhere.
`ContextWindowExceeded` and `MaxTokens` deliberately route to distinct
handlers (compaction vs output-budget escalate) — no `is_max_tokens_family`
umbrella predicate; they share neither recovery nor user-facing wording.

### Tool-use-summary side-fork (`ModelRole::Fast`)

After each tool batch `engine_finalize_turn::spawn_tool_use_summary`
optionally spawns a blocking Fast-role call producing a ≤30-char mobile-row
label (`tool_use_summary.rs`). Four gates, all in `spawn_tool_use_summary`:

1. `Feature::ToolUseSummary` enabled — **default off**: every tool-using turn
   costs an extra Fast-role call, and reasoning-class Fast models exhaust the
   budget on reasoning before any visible text (`stop_reason=length`, empty).
   Opt in via `features.tool_use_summary = true` with a non-reasoning Fast.
2. Model runtime registry wired (Fast role configured).
3. `agent_id.is_none()` — subagents don't surface in the mobile UI.
4. Tool batch non-empty.

`QueryParams.max_tokens` is intentionally `None` — defer to the Fast model's
own `max_output_tokens` from `ModelInfo`; hardcoding `64` is unsafe on
reasoning models. Non-clean terminations propagate through `coco-inference`'s
abnormal-stop_reason warn (see `services/inference/CLAUDE.md`).
