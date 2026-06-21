# coco-compact

Context compaction strategies: full LLM-summarized, micro (tool-result clearing), API-native server-side editing, reactive (prompt-too-long), session-memory, auto-trigger, and the wire serializer for Anthropic `context_management`.

**Inert by default — features staged but not yet active:**

- `Tool Result Budget` (Level 1 + 2) — **the first line of defense** before any
  compaction strategy runs. Phase 0 config staged on
  `coco_config::CompactConfig.tool_result_budget` (`enabled` / `per_message_chars` /
  `persist_records`); runtime owners are `coco-tool-runtime::tool_result_storage`
  (Level 1 — pending) and `coco-query` (Level 2 wiring — pending).
  `coco-tools::BashTool` carries a Bash-only stub (`temp_dir()`, no
  `<persisted-output>` wrapper). See `docs/coco-rs/tool-result-budget-plan.md`.
  Feature gates: `tengu_satin_quoll` (per-tool override), `tengu_hawthorn_window`
  (per-message char cap), `tengu_hawthorn_steeple` (Level 2 enable).
- `HISTORY_SNIP` — no runtime caller reads `compact.experimental.history_snip.enabled`;
  the field is staged for a future implementation.
- `CONTEXT_COLLAPSE` (`marble_origami`) — data types in `staged.rs` (kept for
  transcript-format interop); runtime ledger never installed in production
  (`with_staged_ledger` has zero callers), `apply_collapses_if_needed` reachable
  only via `is_collapse_active()` whose first AND-clause is always false.
- `display_collapses` — config defaults stay `true`, but no renderer consults them
  yet; see the comment at `app/tui/src/widgets/chat/mod.rs::build_lines` for the
  list of pending reducers (`collapseTeammateShutdowns`, `collapseHookSummaries`,
  `collapseBackgroundBashNotifications`, `collapseReadSearchGroups`).

Four opt-in flag groups on `CompactConfig` track the future implementations:

- `compact.experimental.history_snip.{enabled, auto_pct, model_invocable}` — default off
- `compact.experimental.staged_compact.{enabled, stage_at_pct, commit_at_pct, persist_to_transcript}` — default off
- `compact.experimental.display_collapses.{read_search, hook_summaries, background_bash, teammate_shutdowns}` — default on (gates pending reducers)
- `compact.tool_result_budget.{enabled, per_message_chars, persist_records}` — default `(false, 200_000, true)`. Feature gates: `tengu_hawthorn_steeple` (enable) + `tengu_hawthorn_window` (cap override). Per-tool override (`tengu_satin_quoll`) maps to `Tool::max_result_size_chars()` after the Phase 1.B `ResultSizeBound` migration.

`compact.micro` carries two additional opt-ins:

- `compact.micro.count_based_enabled` — default `false`. Gates
  `coco_compact::micro_compact()` count-based clearing in the
  autocompact threshold path and `/compact` flow.
- `compact.micro.clear_file_unchanged_stubs_enabled` — default `false`.
  Gates per-turn `[file unchanged]` stub rewrite.

## Implementation Status

The compact crate stays provider-agnostic. It performs message
selection, stripping, PTL retry, boundary construction, and post-compact
message assembly; `app/query` owns model execution, fork/cache behavior,
tools, hooks, and app-state deltas.

Implemented behaviors:
- Full and partial LLM compaction call a typed summarizer with
  `CompactSummaryAttempt` rather than rendering the conversation into a
  single legacy prompt string. The attempt separates `messages` (the
  selected slice being summarized) from `context_messages` (the structured
  API/fork context), matching TS partial `from` behavior. On PTL retry,
  partial `from` truncates the full API context, not just the tail
  summary slice. The legacy `render_summary_prompt_for_debug` remains
  only for diagnostics.
- `QueryEngine` runs full/partial summaries through a cache-sharing
  `ForkLabel::Compact` fork with deny-all tool policy when available,
  falling back to a structured direct call with `tools = None`.
- Full, partial, and session-memory compaction all restore post-compact
  context in the query layer: files, plan, skills, plan-mode reminder,
  async-agent reminders, SessionStart hook output, deferred-tool/agent/MCP
  deltas, observer cleanup, and cache-break notification.
- Compact-triggered SessionStart hook aggregate output is preserved:
  `initialUserMessage` is inserted into the rewritten history and
  `watchPaths` is forwarded to the CLI runtime's FileChanged watcher.
- Partial post-compact assembly respects direction-specific order:
  `from`/Newest writes boundary → kept prefix → summary, while
  `up_to`/Oldest writes boundary → summary → kept tail.
- `CompactResult.raw_summary` preserves the raw summarizer output for
  PostCompact hooks; formatted continuation text stays only in
  `summary_messages`.
- Auto LLM compaction records `CompactOutcome` and trips the session
  failure breaker after `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`.
- `strip_images_from_messages` traverses `Message::ToolResult` content
  arrays so image bytes cannot re-trip prompt-too-long during summary.
- `wrap_system_reminder` delegates to
  `coco-messages::wrapping::wrap_in_system_reminder`, keeping the wrapper
  format canonical.

P0–P1 normalization ports (see `docs/coco-rs/audit-gaps.md`):
- `sanitize_error_tool_result_content`, `smoosh_system_reminder_into_tool_result`,
  `filter_orphaned_thinking_only_messages`, `filter_trailing_thinking_from_last_assistant`,
  `filter_whitespace_only_assistant_messages`, `ensure_non_empty_assistant_content`
  — all live in `core/messages/src/normalize.rs` and run in the canonical
  order inside `normalize_messages_for_api`.
- `createPlanModeAttachmentIfNeeded` (Round 10c) — `post_compact_plan_mode.rs::create_plan_mode_attachment_if_needed` renders the Full-variant
  reminder text and emits an `AttachmentKind::PlanMode` message; engine
  snapshots `permission_mode == Plan` + plan settings pre-compact.
- `createAsyncAgentAttachmentsIfNeeded` (Round 10c) — `post_compact_async_agents.rs::create_async_agent_attachments` renders
  one `task_status` reminder per filtered running agent; engine snapshot
  via `QueryEngine::with_running_tasks` builder + `snapshot_async_agents_for_post_compact`.
- `RecompactionInfo` (Round 10c) — `CompactRunOptions.recompaction_info`
  plumbs the struct; `QueryEngine::last_compact_state` + `turn_counter`
  populate it; `CompactResult.is_recompaction` is now driven by it.

`HISTORY_SNIP`, `CONTEXT_COLLAPSE`, and `CACHED_MICROCOMPACT` paths
remain disabled or staged per the root architecture rules.

## Configuration

The crate **does not read environment variables.** All env vars are
folded into `coco_config::CompactConfig` at startup by
`CompactConfig::resolve(&Settings, &EnvSnapshot)`. Threshold helpers,
the API-native strategy builder, and the session-memory compactor all
take config refs (`&AutoCompactConfig`, `&CompactApiNativeConfig`,
`&SessionMemoryConfig`).

Per-call run-options (summary token budget, keep-recent rounds, the
`CompactTrigger` label) live in the separate
[`CompactRunOptions`](src/compact.rs) struct — distinct from the
global config struct above.

All env vars use the `COCO_*` prefix (root `CLAUDE.md` → "Code
Hygiene"). `CLAUDE_CODE_*` / unprefixed names are NOT honored.

Layering inside `coco_config::CompactConfig`:

| Sub-config | Defaults | Settings key | Env |
|------------|----------|--------------|-----|
| `auto.enabled` | `true` | `compact.auto.enabled` | — (user toggle) |
| `auto.disabled_by_env` | `false` | — | `COCO_COMPACT_DISABLE` |
| `auto.auto_disabled_by_env` | `false` | — | `COCO_COMPACT_DISABLE_AUTO` |
| `auto.context_window_override` | `None` | `compact.auto.context_window_override` | `COCO_COMPACT_AUTO_WINDOW` |
| `auto.pct_override` | `None` | `compact.auto.pct_override` | `COCO_COMPACT_AUTO_PCT_OVERRIDE` |
| `auto.blocking_limit_override` | `None` | `compact.auto.blocking_limit_override` | `COCO_COMPACT_BLOCKING_LIMIT` |
| `micro.enabled` | `true` | `compact.micro.enabled` | — |
| `micro.keep_recent` | `5` | `compact.micro.keep_recent` | — |
| `micro.time_based.{enabled,gap_threshold_minutes,keep_recent}` | `false`/`60`/`5` | `compact.micro.time_based.*` | — |
| `api_native.clear_tool_results` | `false` | `compact.api_native.clear_tool_results` | `COCO_COMPACT_API_CLEAR_TOOL_RESULTS` |
| `api_native.clear_tool_uses` | `false` | `compact.api_native.clear_tool_uses` | `COCO_COMPACT_API_CLEAR_TOOL_USES` |
| `api_native.max_input_tokens` | `180_000` | `compact.api_native.max_input_tokens` | `COCO_COMPACT_API_MAX_INPUT_TOKENS` |
| `api_native.target_input_tokens` | `40_000` | `compact.api_native.target_input_tokens` | `COCO_COMPACT_API_TARGET_INPUT_TOKENS` |
| `session_memory.enabled` | `false` | `compact.session_memory.enabled` | `COCO_COMPACT_SESSION_MEMORY_{ENABLE,DISABLE}` |
| `session_memory.{min_tokens,min_text_block_messages,max_tokens,max_summary_chars}` | `10K`/`5`/`40K`/`100K` | `compact.session_memory.*` | — |
| `experimental.history_snip.*` | `false`/`0.7`/`false` | `compact.experimental.history_snip.*` | — |
| `experimental.staged_compact.*` | `false`/`0.6`/`0.85`/`false` | `compact.experimental.staged_compact.*` | — |
| `experimental.display_collapses.*` | all `true` | `compact.experimental.display_collapses.*` | — |
| `tool_result_budget.enabled` | `false` | `compact.tool_result_budget.enabled` | `COCO_COMPACT_TOOL_RESULT_BUDGET_ENABLE` |
| `tool_result_budget.per_message_chars` | `200_000` | `compact.tool_result_budget.per_message_chars` | `COCO_COMPACT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS` |
| `tool_result_budget.persist_records` | `true` | `compact.tool_result_budget.persist_records` | — |

`AutoCompactConfig::is_active()` is the canonical predicate that fuses
the user toggle with both env kill switches.

## Multi-Provider Strategy

Three layers, picked at runtime based on provider capability:

1. **Client-side micro-compact** (`micro::micro_compact`,
   `micro_advanced::*`). Provider-agnostic: rewrites old tool result
   content to `[Old tool result content cleared]` placeholders.
   Invalidates the prompt cache because it mutates messages, but works
   with any provider.

2. **API-native server-side editing** (`api_compact::get_api_context_management`
   + `serialize::encode_anthropic_context_management`). Anthropic-only.
   Produces `Vec<ContextEditStrategy>` describing
   `clear_tool_uses_20250919` / `clear_thinking_20251015` edits, then
   the serializer emits the camelCase JSON shape that
   `vercel-ai-anthropic`'s `transform_context_management` expects.
   Preserves the prompt cache because the API applies edits in place.
   Dispatch gate:
   The active runtime snapshot reports server-side context-edit support
   only when the resolved provider API is Anthropic.

3. **Full LLM summarization** (`compact::compact_conversation`). Final
   fallback when neither layer can recover enough budget. Provider-agnostic.

`coco-compact` itself never inspects providers — it produces strategy
descriptions and exposes the encoder. `coco-query` checks
the active runtime snapshot before populating `QueryParams.context_management`;
non-Anthropic runtime slots always see `None` there and rely on layer 1 / 3.

## QueryEngine Integration

`app/query::QueryEngine`:

- `finalize_turn_post_tools` reads `&self.config.compact.auto` for the
  guarded threshold check, runs `micro_compact` with
  `compact.micro.keep_recent`, and falls through to
  `try_full_compact(trigger=Auto)` when still over budget.
- `try_full_compact` runs `execute_pre_compact` → snapshot
  `FileReadState` → `compact_conversation` (with `custom_prompt`
  carrying merged hook + user instructions) → notify
  `CompactionObserverRegistry` → `execute_post_compact` →
  emit `ContextCompacted`.
- `run_manual_compact` is the public entry-point for `/compact`. The
  slash-command handler (`coco_commands::handlers::compact`) emits a
  `__COCO_COMPACT_NOW__ <args>` sentinel line that runners parse to
  decide whether to invoke this method.
- `do_reactive_compact` (PTL recovery) takes `&self.config.compact.auto`
  to honor `CLAUDE_CODE_AUTO_COMPACT_WINDOW` overrides via the shared
  `effective_context_window`.
- The Anthropic-only `context_management` payload is built per-turn in
  `engine.rs` from `compact.api_native` and attached to `QueryParams`;
  `services/inference::build_call_options` slots it into
  `provider_options["anthropic"]["contextManagement"]`.

## Key Types & Functions

- Run-options: `CompactRunOptions` (per-invocation parameters —
  `max_summary_tokens` / `context_window` / `keep_recent_rounds` /
  `custom_prompt` / `suppress_follow_up` / `trigger`). Distinct from
  the global `coco_config::CompactConfig` (settings struct).
- Results: `CompactResult`, `MicrocompactResult`, `TokenWarningState`,
  `CompactWarningState`, `CompactError`.
- Strategies: `ContextEditStrategy`, `ToolUseKeep`, `ThinkingKeep`,
  `ClearToolInputs`.
- Serializer: `encode_anthropic_context_management(&[ContextEditStrategy])
  -> Option<Value>` — `None` when input is empty so callers can omit
  the field entirely.
- Threshold helpers: `should_auto_compact`,
  `should_auto_compact_guarded`, `auto_compact_threshold`,
  `effective_context_window`, `calculate_token_warning_state` —
  **all take `&AutoCompactConfig`**.
- Reactive: `ReactiveCompactConfig`, `ReactiveCompactState`,
  `peel_head_for_ptl_retry`, `api_microcompact`,
  `should_reactive_compact(&ReactiveCompactConfig, &AutoCompactConfig)`,
  `calculate_drop_target(&ReactiveCompactConfig, &AutoCompactConfig)`.
- Time-based MC: `TimeBasedMcConfig` (re-exported from `coco_config`),
  `TimeBasedTrigger`, `evaluate_time_based_trigger`.
- Session memory: `SessionMemoryCompactConfig`, `compact_session_memory`,
  `select_memories_for_compaction`, `merge_similar_memories`.
- Post-compact attachments: `create_post_compact_file_attachments`,
  `create_plan_attachment_if_needed`,
  `create_plan_attachment_from_owned`. Skill re-injection
  (`POST_COMPACT_MAX_TOKENS_PER_SKILL` / `POST_COMPACT_SKILLS_TOKEN_BUDGET`)
  is currently driven by `coco_system_reminder::InvokedSkillsGenerator`
  on the next turn rather than a stand-alone helper here.
- Observers: `CompactionObserver` trait + `CompactionObserverRegistry`
  — each crate owning post-compact-invalidatable state registers its
  own observer at startup.
- Prompts: `get_compact_prompt`, `get_partial_compact_prompt`,
  `format_compact_summary`, `get_compact_user_summary_message`.
- Misc: `merge_hook_instructions`, `strip_images_from_messages`,
  `strip_reinjected_attachments`, `truncate_head_for_ptl_retry`,
  `extract_discovered_tool_names`, `estimate_tokens` /
  `estimate_tokens_conservative` / `estimate_message_tokens`.

## Canonical API Shape

All public mutation / transformation entries take **`&[Arc<Message>]`**
and return **`Vec<Arc<Message>>`** (or `Vec<LlmMessage>` at the wire
seam). Read-only utilities (`estimate_tokens`, `group_messages_by_api_round`,
`extract_discovered_tool_names`) keep `<M: Borrow<Message>>` generic
for flexibility with `&[&Message]` slices from internal iterators.

| Function | Signature |
|---|---|
| `compact_conversation` | `&[Arc<Message>] -> CompactResult` |
| `partial_compact_conversation` | `&[Arc<Message>] -> CompactResult` |
| `compact_session_memory` | `&[Arc<Message>] -> Option<CompactResult>` |
| `truncate_head_for_ptl_retry` | `&[Arc<Message>] -> Option<Vec<Arc<Message>>>` |
| `peel_head_for_ptl_retry` | `&[Arc<Message>] -> Option<Vec<Arc<Message>>>` |
| `build_post_compact_messages` | `&CompactResult -> Vec<Arc<Message>>` |
| `CompactResult.messages_to_keep` | `Vec<Arc<Message>>` |
| `CompactSummaryAttempt.messages` / `.context_messages` | `Vec<Arc<Message>>` |

**No `ArcInput` trait, no `_arc` function variants, no `<M: ArcInput + Borrow<Message>>`
composite bound.** The pre-pipeline-refactor band-aids are removed.

## Pipeline Architecture

Compact's two stripping passes (`StripImages`,
`StripReinjectedAttachments`) live in `compact_passes` and implement
[`coco_messages::pipeline::MessagePass`]. They are composed by the
canonical `run_compact_strip_pipeline(&[Arc<Message>]) -> Vec<Arc<Message>>`
helper, which all three compact entry points
(`compact_conversation`, `partial_compact_conversation`,
`compact_session_memory`) share verbatim. No more hand-written
"Arc → owned → mutate → Arc" boilerplate per entry — and no central
"`needs_X`" predicate to keep in sync with each pass body.

Fast path (no images, no expiring attachments) returns the input
Arc-vec via `to_vec()` (N×Arc::clone, zero `Message::clone`); slow
path materializes one `Vec<Message>`, runs both passes in order, and
re-wraps as Arc-vec. The per-message rewrite helper
`strip_one_message_for_media_if_needed` is shared between the
legacy owned-input `strip_images_from_messages` (used by tests +
backward-compat) and the `StripImages` pass body.

See [docs/coco-rs/message-pipeline.md](../../docs/coco-rs/message-pipeline.md)
for the cross-crate design (also drives the 7 normalize passes in
`coco-messages`).
