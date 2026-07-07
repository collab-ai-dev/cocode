# coco-inference

Thin multi-provider LLM client wrapper over `vercel-ai`. Generic retry, usage aggregation, cache-break detection, thinking-level conversion — and **nothing Anthropic-specific** (auth/OAuth/prompt-cache/rate-limit policy live in `vercel-ai-anthropic`, not here).

**Not in scope here** (these belong in `vercel-ai-anthropic` or are Anthropic-only concerns): OAuth, policy limits, rate-limit messaging, Claude.ai limits, auth helpers, beta resolution.

## Key Types

- `ModelRuntimeRegistry`, `ModelRuntime`, `QueryParams`, `QueryResult` — unified LLM call surface over `Arc<dyn LanguageModelV4>` slots
- `RetryConfig` — generic cross-provider retry
- `UsageAccumulator` — token/cost accumulation
- `CacheBreakDetector`, `CacheBreakResult`, `CacheState` — prompt-cache boundary detection
- `StreamEvent`, `synthetic_stream_from_content` — streaming primitives
- `InferenceError`, `ErrorLog`, `RequestLog`, `ResponseLog`, `KnownGateway`
- `InferenceError::RateLimited.long_context_credits_required` — provider-neutral
  flag set from adapter-owned diagnostics when Anthropic rejects 1M context for
  missing usage credits. Raw Anthropic message matching stays in
  `vercel-ai-anthropic`.
- `StopReason` and other DTO names (Message, content parts, ProviderOptions, FinishReason, Usage, ProviderMetadata, ReasoningLevel) **are not re-exported here** — they live in `common/llm-types` (DTO seam). This crate owns runtime types only (`LanguageModel` trait, CallOptions, GenerateResult, Provider trait)
- `merge_provider_options`, `provider_base_options` — provider option merging
- `generate_tool_schemas`, `filter_schemas_by_model`, `estimate_schema_tokens` — tool schema pipeline
- `cache_convert::to_extra_body` — provider-neutral pass-through emission of `cacheStrategy` / `requestedBetas` (Anthropic-only consumer today)
- `cache_convert::session_context_to_extra_body` — pass-through emission of `agenticQuery` / `querySource`, gated on non-disabled cache strategy
- `build_call_options_with_extra` — returns `(LanguageModelV4CallOptions, BTreeMap<String, Value>)` so the cache-break detector hashes the merged map directly
- `prompt_layout::{SystemPromptPart, put_system_prompt_parts, build_prompt_layout_from_prompt}` — consumes the single prompt-parts envelope carried on the System message under `provider_options["prompt_layout"].system_parts`. Anthropic receives one `system[]` text block per part with `CacheHint::Breakpoint` mapped to `cache_control: { type: "ephemeral" }`; OpenAI/Gemini flatten the same parts into their native top-level instruction slots. Cache-break hashing is derived from that final layout, including block boundaries and cache-control metadata.
- `ProviderClientFingerprint` — extended with `runtime_state_digest` over `account` + `prompt_cache` + per-provider `provider_options` map; settings-reload that flips any of these triggers a turn-boundary client rebuild (design §19.3 attack γ). Per-provider scoping means a knob flip on one Anthropic instance doesn't churn an unrelated instance's client.
- `QueryParams.fallback_min_context_window` — optional per-call fallback guard.
  When set, `ModelRuntime` skips fallback slots whose known
  `ModelInfo.context_window` is smaller than the requested floor; slots
  without `ModelInfo` remain eligible for custom/test models.

## Call path — bypasses `vercel-ai/ai` SDK layer

[`ModelRuntime::query_once`] / [`ModelRuntime::open_stream`] call
`self.model.do_generate` / `do_stream` **directly** on the
`Arc<dyn LanguageModelV4>` (provider adapter). coco-rs does NOT
route through `vercel_ai::generate_text` / `stream_text` in
production paths — `grep` confirms only `vercel-ai/ai/tests/live/`
reaches those entry points. Anything that lives inside
`vercel-ai/ai/src/generate_text/` is **dead for coco-rs**.

Tool-input handling lives in three layers spread across crates,
each owning a distinct concern:

- **wire parsing — provider adapter + inference seam**
  (`vercel-ai-openai`, `vercel-ai-openai-compatible`,
  `vercel-ai-anthropic`, `vercel-ai-google`, then
  `coco-inference::ToolInputWireState`). Provider adapters carry only
  wire strings and metadata; `services/inference` records the parsed/raw
  state on `ToolCallSegment.input_state` so `app/query` does not reparse
  already-parsed inputs. Empty / whitespace-only input becomes
  `ToolInputWireState::Empty`; clean or repaired JSON becomes
  `ParsedJson(Value)`; provider-marked unrecoverable raw input becomes
  `UnrecoverableRaw { raw, error }` and is copied to
  `ToolInputInvalidReason::JsonParseFailed`; raw strings that may be
  legitimate freeform/custom tool payloads remain
  `RawStringAllowed { raw }` and are left for tool-runtime coercion.
- **schema validation — `app/query/src/tool_input_validate.rs`**.
  `validate_tool_call` runs `Value::String` recovery only for calls that
  were not already classified as wire-invalid, then JSON Schema
  validation via the existing
  `coco_tool_runtime::ToolSchemaValidator` (called pre-PreToolUse
  hook for raw input; the existing post-hook
  `validate_effective_input_or_complete_error` at
  `tool_call_preparer.rs` keeps catching hook-rewritten input).
  Sets `ToolCallPart.invalid_reason` to the structured variant
  (`SchemaViolation` / `NoSuchTool` / `JsonParseFailed`) so error wrap
  picks the wrap prefix by `match`, not string compare.
- **error wrap — `app/query/src/tool_call_preparer.rs::prepare_one_pending_tool_call`**.
  `tc.invalid` → synthetic
  `tool_result(is_error: true, content: "<tool_use_error>{prefix}: ...</tool_use_error>")`
  via `complete_tool_call_with_error_mode`. The agent loop's
  next turn carries the structured error back to the main LLM and
  the model self-corrects — there is no LLM repair callback, and
  there is no static repair retry; recovery is the agent loop itself.

If you find yourself adding tool-input parsing or validation
logic to `vercel-ai/ai/src/generate_text/`, you almost certainly
want `app/query` instead.

**Why schema validation lives in `app/query`, not here**: `coco-inference` is
deliberately tool-agnostic — it carries no dependency on
`coco-tool-runtime` and no awareness of the per-tool JSON Schema
registry that drives validation. Other runtime callers
(compaction, side-queries, auto-mode classifier, title generation,
hook LLM) all pass `tools: None` and therefore have nothing to
validate against. schema validation sits at the only path that actually
executes tools (the agent loop's `tool_call_preparer`), where the
`ToolSchemaValidator` is already on `ToolUseContext`. The wire-level
wiremock tests under each `vercel-ai-*/tests/*_wiremock.rs` lock the
wire parsing contract; the end-to-end coverage of schema validation lives in
`app/query/tests/tool_input_error_chain.rs` +
`app/query/src/tool_input_validate.test.rs`.

**Anthropic streaming parse-once seam:** the adapter parses accumulated
`input_json_delta` only at `content_block_stop`, where it also injects
provider tool `type` fields for code-execution variants. Repairable input is
serialized back to the provider DTO string and becomes
`ToolInputWireState::ParsedJson` at the inference seam. Unrecoverable or
non-object Anthropic tool input is marked
`ToolInputInvalidReason::JsonParseFailed`, becomes
`ToolInputWireState::UnrecoverableRaw`, and `app/query` skips schema
validation so the model sees the JSON-parse failure prefix rather than a
secondary schema error. Freeform/raw-string tools are still decided by
`coco-tool-runtime::ValidatedInput` coercion, not by provider guessing.

## Design Notes

- **`extra_body` overrides typed writes by design — final-merge priority (F1 doctrine).** Per-call pipeline is: (1) typed channel writes typed body (e.g. `body.generationConfig.thinkingConfig` from the adapter's `resolve_thinking_config`), then (2) `provider_options[<ns>]` extras **deep-merge** over the body via `vercel_ai_provider_utils::merge_json_value`. Extras ALWAYS win — they are the user/integrator escape hatch. `thinking_convert.rs` is a typed-channel producer that uses extras as its transport; when its output keys overlap with the typed write at the same JSON path, **extras win by deep-merge contract** — this is intentional, not a coordination bug, and there is no plan to collapse the two writers. Producers augmenting a sibling typed write MUST emit the **nested wire-correct shape** (e.g. `{generationConfig: {thinkingConfig: {includeThoughts: true}}}`) — a flat root-level `{thinkingConfig: ...}` would clobber sibling typed writes after deep-merge. **Null in extras is a no-op** — `merge_json_value` treats `Value::Null` in overrides as "no override, preserve base" at every depth. To unset a key, omit it from extras (do NOT pass `null`). The "extras = escape hatch" contract is replicated in every per-provider options doc near the `#[serde(flatten)] extra` field; this bullet is the single source of truth.
- **`ProviderOptions` extraction is uniform and fail-fast across providers.** Every per-provider options struct (`GoogleLanguageModelOptions`, `AnthropicProviderOptions`, `OpenAIChatProviderOptions`, …) implements `vercel_ai_provider_utils::ExtractExtras` and is parsed via `vercel_ai_provider_utils::extract_namespaced(po, canonical_ns, custom_ns)`. The helper deep-merges canonical + custom namespace per-key with custom winning, deserializes into the typed struct, and splits out `#[serde(flatten)] extra` automatically. Missing namespaces still produce typed defaults, and unknown keys still flow through extras, but malformed shapes for typed fields return `ExtractNamespacedError` and abort request-body construction instead of silently dropping typed config. `openai-compatible` has a 3-level camelCase fallback (`camel(provider)` → `raw(provider)` → `openaiCompatible`) implemented in `provider_options_key::get_effective_provider_options` — that custom resolution is the documented exception; the typed/extras split still uses the shared `ExtractExtras` trait. **`shallow_merge_object` no longer exists** — every adapter deep-merges extras via `merge_json_value` so user-supplied nested paths compose with typed writes instead of clobbering whole subtrees.
- Thinking-level conversion (`thinking_convert`): `ThinkingLevel` → per-provider `ProviderOptions`. Signature is `to_extra_body(level, api, capabilities: &[Capability])` — `build_call_options` threads `info.capabilities.as_deref().unwrap_or(&[])` through. The `ProviderApi::Anthropic` arm has full coverage of `ReasoningEffort` via an exhaustive inner match: `Disable` → `thinking: {type: disabled}`; `Auto` → `thinking: {type: adaptive}` **only when `capabilities` contains `Capability::AdaptiveThinking`**, otherwise omitted (server default applies); `Minimal` → mapped to `Low`; `Low/Medium/High/XHigh` → emit BOTH `thinking: {type: enabled, budgetTokens?}` (legacy API, with budget when ModelInfo declares one) AND `output_config.effort` (new API, mapped via Anthropic's `Effort` enum: `Low/Medium/High` literal, `XHigh` → `"max"`). The `Openai` arm emits `reasoningSummary: "auto"` for `Auto` **and** every explicit level (only `Off` is skipped) — requesting the reasoning *summary* is independent of the effort level, matching codex's `summary.unwrap_or(Auto)` default. This is what makes "thinking" render for gpt-5 models, whose builtin `default_thinking_level` is `Auto`; coupling the summary to `is_explicit_level()` silently dropped reasoning text on the default path. The remaining arms (Gemini/OpenaiCompat) keep the `is_explicit_level()` gate — `Disable`/`Auto` emit nothing for them, and the capability slice is unused. The `output_config` write rides the extras deep-merge path — the convert layer never sets `AnthropicProviderOptions.effort`, so the Anthropic-specific `effort-2025-11-24` beta header is not added. Callers wanting that beta opt in by setting `provider_options["anthropic"]["effort"]` directly. **Adaptive thinking is gated by `Capability::AdaptiveThinking`** — declared in the registry for Claude Sonnet 4.6, Claude Opus 4.7, and DeepSeek V4 (anthropic-compat). Non-adaptive Claude models (Sonnet 4.5, Opus 4.5, Haiku 4.5) gracefully degrade to server-default when the user passes `--thinking auto`, preventing 400 errors. `level.options` is passed through unconditionally (including for `Disable`/`Auto`). **`budget_tokens` is faithfully forwarded — when `level.budget_tokens` is `None`, the typed Anthropic arm omits the `budgetTokens` key, and `vercel-ai-anthropic` likewise emits `{"type":"enabled"}` with no budget on the wire (no synthesized default, no `max_tokens` bump). Endpoints that require it must declare a budget at the `ModelInfo` layer.**
- `ApiClient` is crate-private. Public callers go through `ModelRuntimeRegistry`; model routing (`ModelRole` → `ModelSpec`) and explicit provider/model selection are resolved there.
- **Cache-strategy emission is pass-through, not policy.** This crate emits the typed signals (`cacheStrategy`, `requestedBetas`, `agenticQuery`, `querySource`) into `provider_options["anthropic"]`. All decisions about whether/how to act on them (1h-TTL eligibility latch, allowlist match, marker placement, beta-header gating) live in `vercel-ai-anthropic` (`cache_policy`, `cache_placement`, `beta_resolver`). The raw map lands in the merged `extra_body` with the underlying signal preserved verbatim — no re-encoding hop.
- **1M-context credits clamp is runtime state, not provider text parsing.**
  `ModelRuntime` latches `InferenceError::is_long_context_credits_required()`
  and clamps future `ModelRuntimeSnapshot.model_info.context_window` values
  above `200_000` down to `200_000` for the session. The adapter detects the
  Anthropic wording; inference only consumes the typed flag and applies the
  snapshot clamp.
- **Per-call fallback window floors are opt-in.** Ordinary agent calls keep
  the configured fallback chain unchanged. Compact summarization can set
  `fallback_min_context_window` to avoid falling from a large-window model to
  a smaller-window one while summarizing an already-full transcript.
- **Detector hashes the merged map.** `build_call_options_with_extra` snapshots the merged map BEFORE namespace-wrapping; `client::build_prompt_state_input` hashes the snapshot. Adding new pass-through keys later (e.g. a future `cacheBudget`) auto-tracks without touching the detector — no key-by-key plumbing required.
- **Single typed `StopReason` for the whole workspace.** `coco_inference::StopReason` is a re-export of the extended `vercel_ai_provider::UnifiedFinishReason` (8 variants — `EndTurn`, `StopSequence`, `ToolUse`, `MaxTokens`, `ContextWindowExceeded`, `ContentFilter`, `Error`, `Other`). Mapped exactly once at the provider-adapter seam (`vercel-ai-anthropic`, `-google`, `-openai`, …); higher layers (`coco-messages::StopReason`, `app/query`, `app/cli`) match on the enum directly with zero wire-string parsing. The deprecated subset enum that previously lived in `inference/src/logging.rs` is gone. See `vercel-ai/provider/src/language_model/v4/finish_reason.rs` for the multi-LLM mapping table.
- **`QueryResult.stop_reason` and `StreamEvent::Finish.stop_reason` carry the full `FinishReason` struct** (`{ unified, raw }`), propagated verbatim from the provider seam — **never** decomposed into the bare enum + a sibling `raw_stop_reason` string (that split is gone). These two are the **live carriers**: behavioral code matches on `.unified` (or the delegating `is_*` helpers); `.raw` is an **in-memory** diagnostic for **every** variant, read by the side-query `Other` mapping (`app/cli`) and surfaced in logs via `Display` (`other(compaction)`). `raw` is never persisted — `FinishReason` is `#[serde(transparent)]` over `unified` (a bare wire string, matching `@ai-sdk`'s `finishReason` on `LanguageModelV4StreamPart`), and the **persisted** types (`AssistantMessage`, `CompletedOutcome`, `ResponseLog`) store the bare `StopReason` projection, not this struct — `app/query` projects `.unified` at `engine_stream_consume` once the raw has been logged. The `ContextWindowExceeded` / `StopSequence` refinements are first-class `.unified` variants.
- **Abnormal `stop_reason` escalates to `warn`, never to error.** Both the blocking client (`client.rs::query`) and the streaming pipe (`stream.rs::stream_event_from_part` → `Finish` event log) emit a `warn!` line in addition to the regular `info`/`debug` line when `stop_reason` is not one of `stop` / `end_turn` / `tool-calls` / `tool_use` / `tool_calls` / `stop-sequence` / `stop_sequence` (see `is_abnormal_stop_reason`, re-exported from the crate root for side-fork callers). The warn carries `query_source`, `tokens_out`, and (blocking) `max_tokens` so ops can distinguish a `length` truncation caused by a tight per-call budget from a `content-filter` event. **Not an error.** `stop_reason` is a result field — `QueryResult.stop_reason` flows out to the caller intact, and `app/query` relies on receiving the typed variant to dispatch recovery: `MaxTokens` drives 2-phase `MaxOutputTokensEscalate` / `MaxOutputTokensRecovery`; `ContextWindowExceeded` routes to `handle_context_overflow` (reactive compaction). Returning an `InferenceError` for either would break that recovery and discard partial content. Callers that only need the text (e.g. `tool_use_summary` side-fork) re-warn at their layer when both `stop_reason` is abnormal **and** the extracted text is empty, so the failure mode is debuggable without diffing two log lines.
