# vercel-ai-openai

OpenAI provider for Vercel AI SDK v4. Covers Chat Completions, Responses, Completions, Embeddings, Images, Speech, Transcription APIs.

## SDK Spec

Implements the `@ai-sdk/openai` v4 specification.

## Key Types

- `OpenAIProvider`, `OpenAIProviderSettings`, `openai()` (default), `create_openai()` (custom)
- `OpenAIConfig`, `OpenAIModelCapabilities`, `SystemMessageMode`, `get_capabilities()`
- Models: `OpenAIChatLanguageModel`, `OpenAIResponsesLanguageModel`, `OpenAICompletionLanguageModel`, `OpenAIEmbeddingModel`, `OpenAIImageModel`, `OpenAISpeechModel`, `OpenAITranscriptionModel`

## Conventions

- `provider.language_model(id)` defaults to the Responses API (not Chat); call `provider.chat(id)` explicitly for Chat Completions.
- Reads `OPENAI_API_KEY` by default; `OpenAIProviderSettings` overrides org/project/baseURL/headers.
- Capabilities detection (reasoning, system message handling, tool-choice flavor) lives in `openai_capabilities` — applied per model at request time.
- **`extra_body` deep-merge escape hatch (F1 doctrine).** `provider_options["openai"]` extras deep-merge over typed body writes via `merge_json_value`; extras win at final-merge priority. Both `OpenAIChatProviderOptions` and `OpenAIResponsesProviderOptions` carry `#[serde(flatten)] extra` + implement `ExtractExtras`, parsed via shared `extract_namespaced(po, "openai", "openai")`. `null` in extras is a no-op (skips, does NOT unset). Upstream callers (`coco_inference::thinking_convert`) inject camelCase signals (e.g. `reasoningSummary`) through this same namespace. Single source of truth: `services/inference/CLAUDE.md` "Design Notes".
