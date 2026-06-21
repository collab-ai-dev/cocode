# vercel-ai (Rust mirror of Vercel AI SDK)

Rust port of the [Vercel AI SDK](https://github.com/vercel/ai) v4 type
system and provider crates. Sits below `services/inference` and is the
single seam between coco-rs and AI provider HTTP/streaming protocols.

## TS upstream

| Field | Value |
|-------|-------|
| **Mirror baseline commit** | [`ffd70859f`](https://github.com/vercel/ai/commit/ffd70859f9907121cddf4f1ff81fc17d6f6a510e) — *Version Packages (beta) (#13770)*, 2026-03-23 |
| **Phase 0 catch-up commit** | [`4d58048f9`](https://github.com/vercel/ai/commit/4d58048f9e1782ace6f07fc410324bd2dab0a8fa) — *Version Packages (canary) (#14924)*, 2026-05-01 |
| **Source repo** | `vercel/ai` |
| **Spec version** | v4 (LanguageModelV4 / ProviderV4 trait family) |

The mirror was originally bootstrapped at baseline `ffd70859f`. Phase 0
(commits `5fea386cc` + `98db2151b`) caught up a curated set of
TS-parity fixes observed against `4d58048f9`. The Phase 0 deltas are
spot-fixes — **the mirror is NOT a complete sweep** to current HEAD.
See `Phase 0 catch-up scope` below for what was caught.

## Scope — what's mirrored vs intentionally skipped

| TS package | Rust crate | Status | Notes |
|-----------|-----------|:------:|-------|
| `@ai-sdk/provider` | `vercel-ai/provider` | ✅ | Trait family, LanguageModelV4 stream parts, content types, errors |
| `@ai-sdk/provider-utils` | `vercel-ai/provider-utils` | ✅ | ~40 of ~60 helpers; TS-environment-only (Web Streams, Buffer, AbortSignal) collapsed into idiomatic Rust |
| `@ai-sdk/ai` | `vercel-ai/ai` | ✅ (subset) | `generate_text` / `stream_text` / `generate_object` / `stream_object` / `embed` / `rerank` / `generate_image` / `generate_speech` / `generate_video` / `transcribe` + middleware / registry / model handles. **Skipped subdirs**: `agent/` (coco-query owns the loop), `ui/` + `ui-message-stream/` (Vercel RSC runtime), `upload-file/` + `upload-skill/` (per coco-rs scope) |
| `@ai-sdk/anthropic` | `vercel-ai/anthropic` | ✅ | Messages API; cache-control validator; `inference_geo`; `sanitize_json_schema` |
| `@ai-sdk/openai` | `vercel-ai/openai` | ✅ | Chat + Responses + Completion + Embedding + Image + Speech + Transcription |
| `@ai-sdk/openai-compatible` | `vercel-ai/openai-compatible` | ✅ | Chat + Completion + Embedding + Image; `StreamingToolCallTracker` integration |
| `@ai-sdk/google` | `vercel-ai/google` | ✅ | Generative AI; per-modality token detail |
| `@ai-sdk/bytedance` | `vercel-ai/bytedance` | ✅ | Seedance video; Dreamina 2.0 IDs |
| All other 40+ TS provider packages (alibaba, amazon-bedrock, azure, cohere, deepseek, fireworks, gateway, groq, mistral, perplexity, replicate, togetherai, xai, etc.) | — | ❌ | **Not in mirror scope** — coco-rs ships the providers above. New providers are added on demand. |
| `@ai-sdk/rsc`, `@ai-sdk/react`, `@ai-sdk/svelte`, `@ai-sdk/vue`, `@ai-sdk/angular`, `@ai-sdk/langchain`, `@ai-sdk/llamaindex`, `@ai-sdk/workflow` | — | ❌ | UI / framework adapters — out of scope for a CLI/SDK runtime |
| `@ai-sdk/codemod`, `@ai-sdk/test-server`, `@ai-sdk/devtools`, `@ai-sdk/valibot` | — | ❌ | Tooling / dev-only |

### Why skills/files are skipped
`upload-skill/` and `upload-skill-file/` in TS are part of the Vercel
"Skills" SDK feature. coco-rs has its own skills system
(`/coco-rs/skills/`) that is a markdown-workflow runtime, not a
code-uploaded artifact registry. The two have nothing in common — the
TS SDK is intentionally not mirrored.

## Phase 0 catch-up scope (what `5fea386cc` + `98db2151b` actually shipped)

These are deltas applied on top of baseline `ffd70859f`, sourced from
inspection of `vercel/ai` commits up to `4d58048f9`. Not a complete
sweep — only items with concrete user-visible value.

| Area | Delta | TS reference behavior |
|------|-------|-----------------------|
| `is_step_count` | Strict equality (`steps.len() == n`) instead of `>=` | `packages/ai/src/generate-text/stop-condition.ts` |
| `prepare_step` provider_options | Deep merge via `merge_provider_options(call, step)` instead of pick | `packages/ai/src/util/merge-objects.ts` + `generate-text.ts:630` |
| Prototype-pollution filter | `__proto__` / `constructor` / `prototype` dropped during deep merge | `merge-objects.ts:43-46` |
| Runtime / tool context | Non-breaking `Arc<dyn Any>` (TS uses generic `RUNTIME_CONTEXT`) | `generate-text.ts:RUNTIME_CONTEXT` |
| Stream `ToolCall` / `ToolResult` | v4 `LanguageModelV4ToolCall` (input: String) / `LanguageModelV4ToolResult` (result: JSONValue) | `language-model-v4-stream-part.ts` |
| OpenAI-compatible streaming tool-calls | Outer `pendingToolCalls` buffer + `StreamingToolCallTracker` (arguments-before-name semantics) | `packages/openai-compatible/src/chat/openai-compatible-chat-language-model.ts:466-524` |
| OpenAI-compatible options key | camelCase > raw > `openaiCompatible` precedence + deprecation warning | TS `getProviderOptions` precedence |
| Anthropic `inference_geo` | New option (`"us"` / `"global"`) | `@ai-sdk/anthropic` |
| Anthropic `sanitize_json_schema` | Wrap schema before `output_config` | `@ai-sdk/anthropic` |
| Google per-modality token detail | `prompt_tokens_details` + `candidates_tokens_details` on usage | `@ai-sdk/google` |
| ByteDance Dreamina | Seedance 2.0 model IDs | `@ai-sdk/bytedance` |
| Legacy cleanup | Deleted `vercel_ai_provider::tool::{ToolCall, ToolResult}` (input: JSONValue, output: JSONValue) — superseded by v4 types | (cleanup, no TS counterpart) |

### Items observed in TS upstream but NOT yet mirrored

Surfaced by `git log packages/{ai,provider}/ ffd70859f..4d58048f9` (254
non-merge commits in the mirrored packages). Worth a future sweep:

- `feat(ai): add sensitiveContext / sensitiveRuntimeContext` (#14757, #14777) — security-tier runtime context partition; coco-rs has no equivalent partitioning need today
- `feat(ai): generic tool approval function` (#14690) + automatic tool approval (#14643) — would couple to coco-permissions if pursued; permissions classifier is more capable already
- `feat(ai): add a ModelCall start/end event` (#14706) — additional callback surface; coco's `CoreEvent` covers similar telemetry
- `feat(provider): change file part data property to be tagged with a type and remove the image part type` (#14733) — Rust shape already tagged via `SharedV4FileData::{Data, Url, Reference, Text}`; Phase 1 D4 also dropped redundant Image* variants from `ToolResultContentPart`
- `feat: distinguish provider-defined and provider-executed tools` (#14635) — already covered by `LanguageModelV4ToolCall.provider_executed`
- `feat(mcp): propagate the server name through dynamic tool parts` (#14813) — MCP integration owned by `services/mcp`, not vercel-ai
- `fix(vertex): use correct import for token generator` (#14919) — Vertex provider not in mirror scope
- `feat(vertex): add grok models to vertex provider` (#14883) — Vertex not mirrored
- Multiple `chore: konsistent`-driven refactors making provider patterns consistent — cosmetic, low priority

### Phase 1 catch-up applied (commits 9dcb46428 + later)

Spec-alignment fixes layered on top of Phase 0 — see `Phase 1 catch-up
scope` below.

## Phase 1 catch-up scope

| Area | Delta | TS reference |
|------|-------|--------------|
| `LanguageModelV4Response.timestamp` typed | `Option<chrono::DateTime<Utc>>` (was `Option<String>`) | `LanguageModelV4ResponseMetadata.timestamp: Date` |
| `LanguageModelV4Response.id` field | Added — flatten of `ResponseMetadata.id` | `LanguageModelV4ResponseMetadata.id?` |
| `ToolResultContentPart` cleanup | Drop Image* variants (image vs file via `media_type`); add `media_type` to FileUrl; rename `FileId{file_id}` → `FileReference{provider_reference}`; `SharedV4ProviderReference` type alias added | `LanguageModelV4ToolResultOutput.content[]` |
| OpenAI Chat streaming → tracker | Replace inline `InProgressToolCall` state with `StreamingToolCallTracker`. anthropic + google provider streams keep their own SSE state machines (wire shapes don't match the tracker's OpenAI-delta assumption) | `processStreamingToolCalls` |
| `parse_provider_options` helper | Generic `parse_provider_options::<T>(provider, opts)` + `_with_fallback(...)` in provider-utils — consolidates per-provider ad-hoc `extract_*_options` patterns | `parseProviderOptions` |
| `resolve_provider_reference` helper | `resolve_provider_reference(&map, provider) -> Option<&str>` for cross-provider file-ID lookup | `resolveProviderReference` |
| #14752 reject system in messages by default | New `standardize_prompt_with_options(..., allow_system_in_messages: bool)`; default `standardize_prompt` enforces strict (security: prompt injection) | `standardizePrompt({allowSystemInMessages})` |
| #14789 OpenAI Responses preserve namespace | `function_call.namespace` field added; propagated to `provider_metadata.openai.namespace` on tool-call + tool-input-end (both batch and streaming paths) | `openai-preserve-namespace-on-function-call` |
| #14863 OpenAI image options typed | New `OpenAIImageGenerationOptions` (+ `moderation`) and `OpenAIImageEditOptions` (+ `inputFidelity`) on top of base `OpenAIImageProviderOptions` (+ `background`, `output_format`, `output_compression`) | `OpenAIImageModelOptions` schemas |

#14844 (`allowSystemInMessages` preserved across retries) is implicitly
handled — the rejection path lives in `standardize_prompt`, which
runs once before the retry loop. No flag to forget.

## Intentional spec deviations (Rust > TS — keep, document)

These differ from `@ai-sdk/provider` v4 by design. **Wire-format
interop with TS readers requires an adapter** (coco-rs doesn't ship
messages directly to TS backends; the provider crates re-construct
wire bodies from these typed representations).

| Area | TS shape | Rust shape | Why Rust is better |
|------|----------|-----------|-------------------|
| `LanguageModelV4Message::System.content` | `string` | `Vec<UserContentPart>` | Supports multi-segment system messages (Anthropic per-segment `cache_control` breakpoints, system-level file/image attachments). TS limitation forces downgrade to user messages. |
| `Developer` role | not present (provider translates from `system`) | first-class enum variant | Eliminates provider-internal magic; caller intent is explicit. OpenAI Responses translates correctly; other providers treat as `system`. |
| Input-part metadata field naming | `providerOptions` (input parts) vs `providerMetadata` (output parts) — historical inconsistency | unified `provider_metadata` on shared `TextPart`/`FilePart`/`ReasoningPart`/`ToolCallPart`/`ToolResultPart` | Input/output parts share Rust struct; one field cleaner than TS's two-name historical accident. v4 output structs (`LanguageModelV4Text`, `LanguageModelV4File`, …) still use `provider_metadata` to match TS output direction. |
| `supportedUrls` | `PromiseLike \| Record` (sync OR async) | `fn -> HashMap` (sync only) | No current provider needs async; TS option is over-engineering. Add `supported_urls_async` later if a provider ever needs it. |
| sampling numerics | `number` (f64) for temperature/topP/etc.; `number` for topK | `f32` for samplers; `u64` for topK | Type-correct: temperature/topP/penalty fit f32 precision; topK is a positive integer. TS `number` is JS's single-numeric-type artifact. |
| `headers` | `Record<string, string \| undefined>` | `HashMap<String, String>` | Idiomatic Rust: omit-key vs explicit-undefined. Functionally equivalent. |
| `LanguageModelV4FunctionTool.type` literal | `type: 'function'` field | absent (carried by outer `LanguageModelV4Tool` enum tag) | Wire shape identical when serialized through the parent enum; simplifies the inner struct. |

## Per-crate quick map

| Rust crate | Purpose | CLAUDE.md |
|-----------|---------|:---------:|
| `provider` | LanguageModelV4 / EmbeddingModelV4 / ImageModelV4 / etc. trait family + content / stream / error types | ✅ |
| `provider-utils` | Fetch / response handlers / JSON helpers / schema / streaming-tool-call tracker / etc. | ✅ |
| `ai` | High-level `generate_text` / `stream_text` / etc. + middleware + registry + telemetry | ✅ |
| `anthropic` | Claude provider (Messages API; cache control; thinking; OAuth handled via inference seam) | ✅ |
| `openai` | OpenAI provider (Chat + Responses + Completion + Embed + Image + Speech + Transcribe) | ✅ |
| `openai-compatible` | Generic OpenAI-compatible provider (xAI / Groq / Together / etc.) | ✅ |
| `google` | Gemini / Generative AI provider | ✅ |
| `bytedance` | Seedance video provider | ✅ |

Each crate's `CLAUDE.md` has key types, TS source pointers, and any
crate-specific conventions.

## How to refresh the mirror

When you do a fresh sweep against TS upstream:

1. `cd {upstream repo} && git fetch && git log <baseline>..HEAD --no-merges -- packages/ai/ packages/provider/ packages/provider-utils/ packages/anthropic/ packages/openai/ packages/openai-compatible/ packages/google/ packages/bytedance/`
2. Triage each commit: feature port? bug fix? cosmetic refactor? skip?
3. Apply deltas; update this README's **Mirror baseline commit** field to the new pinned TS commit; update the "Items observed but NOT yet mirrored" section.
4. Run `just pre-commit` — there are 7000+ workspace tests; vercel-ai changes commonly trigger seam violations or provider-test regressions.

## Architectural rule

`services/inference` is the **only** crate allowed to depend on
`vercel-ai-*` crates directly (enforced by
`scripts/check-vercel-ai-seam.sh`). All upstream callers reach AI SDK
types via `coco_inference::*` aliases.
