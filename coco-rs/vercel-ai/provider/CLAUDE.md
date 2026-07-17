# vercel-ai-provider

Standalone type definitions matching `@ai-sdk/provider` v4. Zero dependencies on other coco crates.

## SDK Spec

Implements the `@ai-sdk/provider` v4 specification. Baseline commit, mirror
scope, Phase 0/1 catch-up, and intentional spec deviations: see
[`../README.md`](../README.md).

## Coco-rs extensions

- **`UnifiedFinishReason` is 8 variants, not 6.** Extended with `StopSequence` (refinement of the spec's `Stop`) and `ContextWindowExceeded` (refinement of the spec's `Length`). Both express provider information that the spec routes through `FinishReason.raw`; coco-rs folds them into the typed enum so the entire workspace can match on a single `coco_inference::StopReason` (re-exported from this crate) without anyone parsing wire strings. See `language_model/v4/finish_reason.rs` for the multi-LLM mapping table.
- **Snake_case wire format**, not kebab-case. Variants serialize as `"end_turn"` / `"max_tokens"` / `"tool_use"` / `"stop_sequence"` / `"model_context_window_exceeded"` / `"content_filter"` / `"error"` / `"other"`. The renames from spec names (`stop` → `end_turn`, `length` → `max_tokens`, `tool-calls` → `tool_use`) align with coco-rs's SDK protocol and transcript JSON, which have always used those names. Backward-compat `FinishReason::stop()` / `length()` / `tool_calls()` constructors and `is_stop()` / `is_length()` / `is_tool_calls()` helpers are kept as aliases.
- **`UnifiedFinishReason::is_normal` / `is_abnormal`** drive the abnormal-stop_reason warn path in `coco-inference`. Higher layers `match` on the variant directly — there is intentionally no `is_max_tokens_family` umbrella helper because `MaxTokens` and `ContextWindowExceeded` take different recovery paths (output-budget escalate + resume nudge vs. reactive compaction); a family predicate would invite recombining them.
- **Streaming transcription (real-time WebSocket STT):** `TranscriptionModelV4::do_stream` is an **optional** defaulted trait method (rejects with an unsupported-functionality error unless overridden, mirroring the spec's `doStream?`). Stream parts are kebab-case tagged (`stream-start` / `transcript-delta` / `transcript-partial` / `transcript-final` / `response-metadata` / `finish` / `raw` / `error`). Implemented by `vercel-ai-xai` (`/stt` WebSocket).

## Key type families

Browse `src/` for the full surface — the families below are the entry points.

| Family | Anchors |
|--------|---------|
| Model traits | `LanguageModelV4`, `EmbeddingModelV4`, `ImageModelV4`, `SpeechModelV4`, `TranscriptionModelV4`, `RerankingModelV4`, `VideoModelV4`, `ProviderV4` |
| Language-model call | `LanguageModelV4CallOptions` → `…GenerateResult` / `…StreamResult`; prompt = `Vec<LanguageModelV4Message>` (typed `User` / `Assistant` / `Tool` / `System`) |
| Content parts | `UserContentPart` / `AssistantContentPart` / `ToolContentPart` over shared `TextPart` / `FilePart` / `ReasoningPart` / `ToolCallPart` / `ToolResultPart` |
| Stream parts | granular ID'd events: `TextStart/Delta/End`, `ReasoningStart/Delta/End`, `ToolInputStart/Delta/End`, `ToolCall`, `ToolResult` |
| Finish / usage | `FinishReason` (unified + raw), `UnifiedFinishReason`, `Usage`, `InputTokens` / `OutputTokens` |
| Middleware | `LanguageModelV4Middleware`, `EmbeddingModelV4Middleware`, `ImageModelV4Middleware` |
| Errors | `AISdkError` family (`APICallError`, `TypeValidationError`, …) — thiserror; standalone, no `coco-error` dep |

## v4 Conventions

- Method naming: `do_generate`, `do_stream`, `do_embed` (v4 prefix).
- Provider extensibility: `ProviderOptions` / `ProviderMetadata` carry `serde_json::Value` — intentional extension point for unknown provider fields (the one `Value` use that does NOT violate the "typed structs over JSON values" rule).
