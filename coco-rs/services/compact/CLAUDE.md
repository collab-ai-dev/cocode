# coco-compact

Context compaction strategies: full LLM-summarized, micro (tool-result clearing), API-native server-side editing, reactive (prompt-too-long), session-memory, auto-trigger, and the wire serializer for Anthropic `context_management`.

**Scope split:** this crate stays provider-agnostic — message selection, stripping, PTL retry, boundary construction, post-compact message assembly. `app/query` owns model execution, fork/cache behavior, tools, hooks, and app-state deltas. Post-compact task-status rehydration is also not owned here: compaction sets `QueryEngine::pending_just_compacted`; the next system-reminder pass calls the CLI `TaskRuntime` source.

## Feature Status

Live by default:

- **Tool Result Budget** (Level 1 + 2) — first line of defense before any compaction strategy. Config: `coco_config::CompactConfig.tool_result_budget` — `(enabled, per_message_bytes, persist_records)` defaults `(true, None, true)`; `None` scales the cap to the model window (`coco_tool_runtime::scaled_per_message_bytes`), `Some(n)` pins a fixed cap. Per-tool thresholds live on `Tool::max_result_size_bound()`. Runtime owners: `coco-tool-runtime::{tool_result_storage,tool_result_offload}` + `coco-query`. Over-cap results are windowed (head+tail) with a recoverable `<persisted-output>` pointer. See `docs/internal/tool-result-offload-v2-design.md`.

Inert — staged but not active:

- `HISTORY_SNIP` — no runtime caller reads `compact.experimental.history_snip.*` (default off).
- `CONTEXT_COLLAPSE` — data types in `staged.rs` kept for transcript interop; `with_staged_ledger` has zero production callers, so `apply_collapses_if_needed` is unreachable.
- `display_collapses` — config defaults `true`, but no renderer consults them yet (pending reducers listed at `app/tui/src/widgets/chat/mod.rs::build_lines`).
- `compact.experimental.staged_compact.*` — default off.

Micro opt-ins, both default `false`:

- `compact.micro.count_based_enabled` — gates `micro_compact()` count-based clearing in the autocompact threshold path and `/compact` flow.
- `compact.micro.clear_file_unchanged_stubs_enabled` — gates per-turn `[file unchanged]` stub rewrite.

## Anti-Echo Compaction Directive

**Deliberate deviation from the TS upstream `prompt.ts` templates — do NOT "fix" back to byte-parity.** The summarization request travels as a trailing user-role message (cache-sharing fork constraint), so a literal non-Claude summarizer can echo the whole request into the summary. Three layers, all in this crate, all cache-safe:

1. `prompt.rs` wraps the request in `<compaction_directive>` sentinel tags with a meta-frame and defines section 6 spatially (excludes prior summaries and `<system-reminder>` attachments).
2. `format_compact_summary` scrubs echoed directive spans — **bounded**: a span is deleted wholesale only when its interior carries a `DIRECTIVE_BODY_MARKERS` substring; otherwise only the tags are dropped and the interior survives. An empty post-scrub result falls back to bare-tag stripping so a pure echo can never yield an empty summary.
3. `summary_guard` emits warn-only anomaly telemetry (`compact summary anomaly detected`) at the `call_with_ptl_retry` choke point — no control-flow change.

The sentinel must not collide with `<system-reminder>`: that prefix would be folded into a preceding tool_result by the normalize smoosh pass on the fork path (regression-pinned in `core/messages/src/normalize.test.rs`).

## Multi-Provider Strategy

Three layers, picked at runtime by provider capability:

1. **Client-side micro-compact** (`micro::micro_compact`, `micro_advanced::*`) — provider-agnostic; rewrites old tool results to `[Old tool result content cleared]` placeholders. **Pointer-bearing results are skipped** (`is_pointer_bearing`) — clearing a `<persisted-output>` reference frees nothing and destroys the only pointer to offloaded data. Invalidates the prompt cache.
2. **API-native server-side editing** (`api_compact::get_api_context_management` + `serialize::encode_anthropic_context_management`) — Anthropic-only. Produces `Vec<ContextEditStrategy>` (`clear_tool_uses_20250919` / `clear_thinking_20251015`), serialized to the camelCase JSON `vercel-ai-anthropic` expects. Preserves the prompt cache.
3. **Full LLM summarization** (`compact::compact_conversation`) — provider-agnostic final fallback.

`coco-compact` never inspects providers — it produces strategy descriptions and exposes the encoder. `coco-query` checks the runtime snapshot before populating `QueryParams.context_management`; non-Anthropic slots always see `None` and rely on layers 1/3. The Anthropic 1M-context credits clamp lives upstream: `services/inference::ModelRuntime` clamps snapshot windows above `200_000` (`STANDARD_CONTEXT_WINDOW_TOKENS`) after the provider reports the credits rejection.

## Configuration

The crate **does not read environment variables.** All env vars are folded into `coco_config::CompactConfig` at startup by `CompactConfig::resolve(&Settings, &EnvSnapshot)`; helpers take config refs (`&AutoCompactConfig`, `&CompactApiNativeConfig`, `&SessionMemoryConfig`). Full field/default table: `common/config/src/compact_settings.rs` doc comments. Non-obvious points:

- `AutoCompactConfig::is_active()` is the canonical predicate fusing the user toggle with both env kill switches (`enabled && !disabled_by_env && !auto_disabled_by_env`).
- `tool_result_budget.enabled` defaults `true` — deliberate divergence from TS's GrowthBook-off fallback (product-policy comment in `compact_settings.rs`).
- Per-call run-options (summary token budget, keep-recent rounds, `CompactTrigger` label) live in [`CompactRunOptions`](src/compact.rs), distinct from the global config.

Env vars (all `COCO_*`; `CLAUDE_CODE_*` / unprefixed names are NOT honored — see `coco_config::EnvKey`):

| Env | Maps to |
|---|---|
| `COCO_COMPACT_DISABLE` / `COCO_COMPACT_DISABLE_AUTO` | `auto.disabled_by_env` / `auto.auto_disabled_by_env` (kill switches) |
| `COCO_COMPACT_AUTO_WINDOW` / `COCO_COMPACT_AUTO_PCT_OVERRIDE` / `COCO_COMPACT_BLOCKING_LIMIT` | `auto.{context_window_override,pct_override,blocking_limit_override}` |
| `COCO_COMPACT_MICRO_KEEP_RECENT` / `COCO_COMPACT_MICRO_TIME_BASED_KEEP_RECENT` | `micro.keep_recent` / `micro.time_based.keep_recent` |
| `COCO_COMPACT_API_{CLEAR_TOOL_RESULTS,CLEAR_TOOL_USES,MAX_INPUT_TOKENS,TARGET_INPUT_TOKENS}` | `api_native.*` |
| `COCO_COMPACT_SESSION_MEMORY_{ENABLE,DISABLE}` | `session_memory.enabled` |
| `COCO_COMPACT_TOOL_RESULT_BUDGET_{ENABLE,PER_MESSAGE_BYTES}` | `tool_result_budget.*` |
| `COCO_COMPACT_POST_COMPACT_MAX_FILES_TO_RESTORE` | `post_compact.max_files_to_restore` |

## QueryEngine Integration

`app/query::QueryEngine`:

- `finalize_turn_post_tools` — guarded threshold check (`&config.compact.auto`), runs `micro_compact`, falls through to `try_full_compact(trigger=Auto)` when still over budget.
- `try_full_compact` — `execute_pre_compact` → snapshot `FileReadState` → `compact_conversation` (with `custom_prompt` = merged hook + user instructions) → notify `CompactionObserverRegistry` → `execute_post_compact` → emit `ContextCompacted`.
- `run_manual_compact` — public entry for `/compact`; the slash-command handler emits a `__COCO_COMPACT_NOW__ <args>` sentinel line that runners parse.
- `do_reactive_compact` (PTL recovery) — takes `&config.compact.auto` to honor `COCO_COMPACT_AUTO_WINDOW` overrides via the shared `effective_context_window`.
- The Anthropic-only `context_management` payload is built per-turn in `engine.rs` from `compact.api_native`; `services/inference::build_call_options` slots it into `provider_options["anthropic"]["contextManagement"]`.
- Full/partial summaries run through a cache-sharing `ForkLabel::Compact` fork (deny-all tools), falling back to a structured direct call with `tools = None`, `query_source = "compact"`, `thinking_level = None`.
- Auto LLM compaction records `CompactOutcome`; the session failure breaker trips after `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`, the rapid-refill breaker after three compactions inside three-turn windows. `AutoCompactAttemptDecision` is the typed gate result consumed by `app/query`.

## Key Types & Functions

| Symbol | Role |
|---|---|
| `CompactRunOptions` | Per-invocation params (budget / window / keep-recent / custom prompt / trigger) |
| `compact_conversation` / `partial_compact_conversation` / `compact_session_memory` | The three compact entry points |
| `CompactResult` | Output; `.raw_summary` preserved for PostCompact hooks, formatted text only in `summary_messages`; `.is_recompaction` driven by `CompactRunOptions.recompaction_info` |
| `CompactSummaryAttempt` | Typed summarizer input — separates `messages` (selected slice) from `context_messages` (structured API/fork context); PTL retry truncates the full context |
| `ContextEditStrategy` + `encode_anthropic_context_management` | API-native strategy description + serializer (`None` when input empty) |
| `should_auto_compact` / `should_auto_compact_guarded` / `auto_compact_threshold` / `effective_context_window` | Threshold helpers — all take `&AutoCompactConfig` |
| `resolve_auto_compact_window` / `resolve_precompute_arm` | Pure source-precedence helpers; runtime clientdata/feature-flag production not wired — callers thread values explicitly |
| `peel_head_for_ptl_retry` / `truncate_head_for_ptl_retry` / `should_reactive_compact` / `calculate_drop_target` | Reactive / PTL recovery |
| `evaluate_time_based_trigger` | Time-based micro-compact gate |
| `CompactionObserver` + `CompactionObserverRegistry` | Each crate owning post-compact-invalidatable state registers an observer at startup |
| `get_compact_prompt` / `get_partial_compact_prompt` / `format_compact_summary` | Prompts + summary extraction/scrub |
| `strip_images_from_messages` / `strip_reinjected_attachments` / `estimate_tokens*` | Stripping + estimation utilities (image strip traverses `Message::ToolResult` content arrays) |

**Canonical API shape:** all public mutation/transformation entries take `&[Arc<Message>]` and return `Vec<Arc<Message>>` (or `Vec<LlmMessage>` at the wire seam); read-only utilities stay `<M: Borrow<Message>>` generic. There is no `ArcInput` trait and no `_arc` function variants.

## Pipeline Architecture

The two stripping passes (`StripImages`, `StripReinjectedAttachments`) live in `compact_passes` and implement `coco_messages::pipeline::MessagePass`. All three compact entry points share the canonical `run_compact_strip_pipeline(&[Arc<Message>]) -> Vec<Arc<Message>>` — fast path (no images, no expiring attachments) returns the input Arc-vec via `to_vec()`; slow path materializes once, runs both passes in order, re-wraps. See [docs/internal/message-pipeline.md](../../../docs/internal/message-pipeline.md) for the cross-crate design (also drives the normalize passes in `coco-messages`).

Post-compact context restoration (files, plan, skills, reminders, SessionStart hook output, deferred deltas, observer cleanup, cache-break notification) happens in the query layer for all three entry points. Partial assembly is direction-specific: `from`/Newest writes boundary → kept prefix → summary; `up_to`/Oldest writes boundary → summary → kept tail. Skill re-injection budgets (`POST_COMPACT_*`) are driven by `coco_system_reminder::InvokedSkillsGenerator` on the next turn.
