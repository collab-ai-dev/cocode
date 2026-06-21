# vercel-ai-google

Google Generative AI (Gemini) provider for Vercel AI SDK v4. Supports language, embedding, image, and video generation.

## SDK Spec

Implements the `@ai-sdk/google` v4 specification.

## Key Types

- `GoogleGenerativeAIProvider`, `GoogleGenerativeAIProviderSettings`, `google()` (default), `create_google_generative_ai()`
- Models: `GoogleGenerativeAILanguageModel` + config, `GoogleGenerativeAIEmbeddingModel` + config, `GoogleGenerativeAIImageModel` + config, `GoogleGenerativeAIVideoModel` + config
- Settings: `GoogleGenerativeAIImageSettings`, `GoogleGenerativeAIVideoSettings`
- Error handling: `GoogleErrorData`, `GoogleFailedResponseHandler`
- Prompt conversion: `convert_to_google_generative_ai_messages`, `ConvertOptions`, `convert_json_schema_to_openapi_schema`, `convert_usage`, `GoogleUsageMetadata`, `get_model_path`, `is_supported_file_url`, `map_finish_reason`
- Tools: `prepare_tools`, `PreparedTools` + Google-specific tool constructors

## Conventions

- Reads `GOOGLE_GENERATIVE_AI_API_KEY` by default.
- Prompt format diverges from OpenAI — content parts convert to Gemini `parts` array in `convert_to_google_generative_ai_messages`.
- JSON schema in tool definitions is rewritten to OpenAPI schema via `convert_json_schema_to_openapi_schema` before sending (Gemini has stricter schema rules).
- **`extra_body` deep-merge escape hatch (F1 doctrine).** `provider_options["google"]` (canonical) + `provider_options["vertex"]` (custom for Vertex) extras deep-merge over typed body writes via `merge_json_value`; extras win at final-merge priority. `#[serde(flatten)] extra` on `GoogleLanguageModelOptions` implements `ExtractExtras`, parsed via shared `extract_namespaced(po, "google", &provider_options_name)`. `null` in extras is a no-op (skips, does NOT unset — omit the key instead). Producers augmenting nested writes (`coco_inference::thinking_convert` for Gemini emits `{generationConfig: {thinkingConfig: {includeThoughts: true}}}`) MUST emit the wire-correct nested shape, never a flat root key — a flat `{thinkingConfig: ...}` would clobber the typed write under deep-merge. Single source of truth: `services/inference/CLAUDE.md` "Design Notes".
