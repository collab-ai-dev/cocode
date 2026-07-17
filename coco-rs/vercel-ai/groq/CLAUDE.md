# vercel-ai-groq

Groq provider for Vercel AI SDK v4. Chat Completions + speech-to-text
Transcription, plus the `groq.browser_search` provider tool.

## SDK Spec

Faithful port of `@ai-sdk/groq`. Built on the coco-rs `vercel-ai-provider`
traits and `vercel-ai-provider-utils` transport (shared `reqwest::Client` +
typed `ResponseHandler`), not a pluggable `fetch`.
Baseline commit, mirror scope, and intentional deviations: see
[`../README.md`](../README.md).

## Key Types

- `GroqProvider`, `GroqProviderSettings`, `create_groq()`
- `GroqConfig` — per-model shared config (provider id, base URL, lazy headers, client, error handler)
- Models: `GroqChatLanguageModel`, `GroqTranscriptionModel`
- Options: `GroqChatProviderOptions` (namespace `"groq"`), `GroqTranscriptionProviderOptions`
- Tools: `GroqTools`, `browser_search()` → `LanguageModelV4ProviderTool` (id `groq.browser_search`)
- Error: `GroqFailedResponseHandler`, `GroqErrorData` (`{error:{message,type}}`)

## Why a dedicated crate (vs. `openai-compatible`)

Groq is OpenAI-wire but diverges enough to warrant its own model impl:

- **Reasoning** surfaces through the `reasoning` field (response + stream deltas),
  emitted *after* text in `do_generate` (matches TS order).
- **Streaming usage** arrives under `x_groq.usage`, not the top-level `usage`.
  This is the single load-bearing reason a config-only wrapper over
  `openai-compatible` is insufficient — its stream reads top-level `usage`.
- **Request options** (`provider_options["groq"]`): `reasoningFormat`,
  `reasoningEffort` (passthrough incl. `none`/`default`), `serviceTier`,
  `parallelToolCalls`, `user`, `structuredOutputs`, `strictJsonSchema`.
- **Reasoning-effort mapping** from a top-level `ReasoningLevel`:
  `minimal|low → low`, `medium → medium`, `high|xhigh → high`; `off` emits
  nothing. An explicit `reasoningEffort` option always wins.
- **`browser_search`** provider tool, honored only on
  `openai/gpt-oss-20b` / `openai/gpt-oss-120b` (warns otherwise).

## Defaults

- Base URL `https://api.groq.com/openai/v1`; API key from `GROQ_API_KEY`.
- `structuredOutputs` / `strictJsonSchema` default to `true`.
- `language_model()` routes to Chat Completions; `transcription_model()` is
  overridden; embedding/image models return `NoSuchModelError`.

## Conventions

- Usage conversion (`convert_groq_usage`) surfaces `prompt_tokens_details.cached_tokens`
  as the input `cache_read` bucket (via `from_inclusive_total`) — unlike the TS
  reference, which discards it — so cost tracking and every other coco provider
  stay consistent. Completion tokens split into text vs. reasoning; full raw
  usage is preserved in `Usage.raw`.
- Multimodal tool results (`ToolResultContent::Content`) degrade gracefully:
  text passes through, non-text parts become a `[… omitted]` marker (matching
  openai / openai-compatible) rather than dumping raw base64 into the prompt.
- Streaming uses the shared `vercel_ai_provider_utils::SseDecoder` for
  UTF-8-safe byte→line framing; a malformed / error chunk surfaces as an
  `Error` stream part and sets the raw `finish_reason` to `"error"` (→ unified
  `Other`), matching the coco provider majority — the `Error` part is the real
  signal, not the finish reason.
- Wire schemas in `chat/groq_api_types.rs` are intentionally a minimal subset —
  only fields the impl reads — so upstream API additions don't break parsing.

## Wiring (runtime-reachable)

Dispatched by name inside the model factory's `OpenaiCompat` arm:
`provider_cfg.name == coco_config::builtin::GROQ_PROVIDER` routes to
`build_groq` (`services/inference::model_factory`) — unlike xai, which has its
own dedicated `ProviderApi::Xai` variant. The `"groq"` provider-options
namespace comes from the instance name.
