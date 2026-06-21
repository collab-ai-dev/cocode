# vercel-ai-openai-compatible

Generic OpenAI-compatible provider for any API following the OpenAI protocol (xAI, Groq, Together, Fireworks, DeepSeek, etc.).

## SDK Spec

Implements the `@ai-sdk/openai-compatible` v4 specification.

## Key Types

- `OpenAICompatibleProvider`, `OpenAICompatibleProviderSettings`, `create_openai_compatible()`
- `OpenAICompatibleConfig`, `SupportedUrlsFn`
- `MetadataExtractor`, `StreamMetadataExtractor` — per-call metadata extension hook
- Models: `OpenAICompatibleChatLanguageModel`, `OpenAICompatibleCompletionLanguageModel`, `OpenAICompatibleEmbeddingModel`, `OpenAICompatibleImageModel`

## Differences from `vercel-ai-openai`

- No OpenAI-specific features: capabilities detection, organization/project headers, Responses API.
- Extensibility hooks: `MetadataExtractor` trait, `transform_request_body`, `query_params`.
- Generic API key env var (not hardcoded to `OPENAI_API_KEY`); callers set `api_key_env_var` in settings.
- Reasoning support in responses via `reasoning_content` / `reasoning` fields.
- Default `language_model()` routes to Chat Completions (not Responses).
- **`extra_body` deep-merge escape hatch (F1 doctrine).** Per-call extras deep-merge over typed body writes via `merge_json_value`; extras win at final-merge priority. Each model's options struct (`OpenAICompatibleChatProviderOptions`, `…CompletionProviderOptions`, `…ImageProviderOptions`) carries `#[serde(flatten)] extra` + implements `ExtractExtras`. **Namespace resolution is 3-level** (the documented openai-compatible exception): `camelCase(provider)` → `raw(provider)` → `"openaiCompatible"` via `provider_options_key::get_effective_provider_options` — most-specific wins. The typed/extras split itself still uses the shared `ExtractExtras` trait. `null` in extras is a no-op (skips, does NOT unset). Single source of truth: `services/inference/CLAUDE.md` "Design Notes".
