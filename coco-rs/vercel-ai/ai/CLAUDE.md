# vercel-ai

High-level SDK matching `@ai-sdk/ai` v4 (generate_text / stream_text / generate_object / embed / rerank / generate_image / generate_speech / generate_video / transcribe). Builds on `vercel-ai-provider` types + `vercel-ai-provider-utils` helpers.

## SDK Spec

Implements the `@ai-sdk/ai` v4 specification. Anthropic-specific concerns (OAuth, policy limits, 529 retry, etc.) belong in `vercel-ai-anthropic`, not here — see the "Multi-Provider SDK" design decision in the workspace `CLAUDE.md`.

## Key Types

Core functions: `generate_text`, `stream_text`, `generate_object`, `stream_object`, `embed`, `embed_many`, `rerank`, `generate_image`, `generate_speech`, `generate_video`, `transcribe`.

Options / results: `GenerateTextOptions`, `GenerateTextResult`, `StreamTextOptions`, `StreamTextResult`, `GenerateObjectOptions`, `GenerateObjectResult`, `StreamObjectOptions`, `EmbedOptions`, `EmbedManyOptions`, `RerankOptions`, `GenerateImageOptions`, `GenerateSpeechOptions`, `GenerateVideoOptions`, `TranscribeOptions`.

Callbacks: `GenerateTextCallbacks`, `StreamTextCallbacks`, `OnStartEvent`, `OnStepStartEvent`, `OnStepFinishEvent`, `OnFinishEvent`, `OnChunkEvent`, `OnToolCallStartEvent`, `OnToolCallFinishEvent`, `OnErrorEvent` (via error module).

Output strategies: `Output`, `OutputMode`, `OutputStrategy`, `OutputSpec`, `text_output`, `object_output`, `array_output`, `choice_output`, `json_output`.

Prompt / content: `Prompt`, `PromptMessage`, `PromptUserMessage`, `PromptAssistantMessage`, `PromptToolMessage`, `PromptSystemMessage`, `PromptContent`, `PromptUserContent`, `PromptAssistantContent`, `PromptToolContentPart`, `PromptTextPart`, `PromptImagePart`, `PromptFilePart`, `PromptReasoningPart`, `PromptToolCallPart`, `PromptToolResultPart`, `PromptToolResultOutput`, `CallSettings`, `SystemPrompt`, `StandardizedPrompt`, `TimeoutConfiguration`.

Model handles: `LanguageModel`, `EmbeddingModel`, `ImageModelRef`, `SpeechModelRef`, `TranscriptionModelRef`, `VideoModelRef`, `RerankingModelRef` + `resolve_*_model[_with_provider]` functions.

Middleware: `wrap_language_model`, `wrap_embedding_model`, `wrap_image_model`, `wrap_provider`, `default_settings_middleware`, `default_embedding_settings_middleware`, `extract_json_middleware`, `extract_reasoning_middleware`, `simulate_streaming_middleware`, `add_tool_input_examples_middleware`, `DefaultSettings`, `DefaultEmbeddingSettings`, `EmbeddingMiddleware`, `ImageMiddleware`.

Registry / provider: `ProviderRegistry`, `ProviderRegistryOptions`, `create_provider_registry`, `custom_provider`, `CustomProviderOptions`, `set_default_provider` / `get_default_provider` / `clear_default_provider` / `has_default_provider`.

Stream processing: `StreamProcessor`, `StreamProcessorConfig`, `StreamMetrics`, `TextStreamPart`. `StreamProcessor` is a thin adapter — idle-timeout + ttft/stall metrics around a `Stream<LanguageModelV4StreamPart>`. **It does no content accumulation**: per-stream snapshots embed consumer policy (which parts matter, which metadata to preserve), so each consumer owns its own accumulator (e.g. `coco_inference::AssistantTurnSnapshot`).

Tool results / errors: `ToolCall`, `ToolResult`, `ToolOutput`, `ToolError`, `ToolCallOutcome`, `DynamicToolCall` / `DynamicToolResult`, `StaticToolCall` / `StaticToolResult`, `TypedToolCall` / `TypedToolResult`, `ToolCallRepairFunction`, `SmoothStream`.

Utilities: `RetryConfig`, `with_retry`, `RetryableError`, `RetrySettings`, `CancellationManager`, `SerialJobExecutor`, `SimulatedStream`, `consume_stream`, `cosine_similarity`, `create_download`, `merge_headers`, `prepare_headers`, `prepare_provider_headers`, `complete_partial_json`, `parse_partial_json[_with_repair]`, `extract_partial_value`, `is_deep_equal`, `DeepPartial`, `LogWarningsFunction`, `TelemetryIntegration`, `TelemetrySettings`.

Errors: `AIError`, `RetryError`, `NoObjectGeneratedError`, `NoImageGeneratedError`, `NoSpeechGeneratedError`, `NoVideoGeneratedError`, `NoTranscriptGeneratedError`, `NoSuchToolError`, `InvalidToolInputError`, `InvalidToolApprovalError`, `MissingToolResultsError`, `SchemaValidationError`, `UnsupportedModelVersionError`, `ToolCallRepairError`.

## Callbacks vs CoreEvent

Callbacks in `generate_text/callback.rs` fire at the **provider boundary** and are NOT bridged into `coco_types::CoreEvent`. The agent loop (`QueryEngine`) consumes them internally and re-emits `AgentStreamEvent` / `ServerNotification`. Trace correlation uses shared `session_id` / `turn_id` context. See `docs/coco-rs/event-system-design.md` §1.7 and plan WS-9.

## Idiom Mapping

| Concept | Rust |
|---------|------|
| `Promise<T>` | `impl Future<Output = T>` |
| `ReadableStream<T>` | `Pin<Box<dyn Stream<Item = T>>>` |
| Generic type constraints | Trait bounds |
| Union types | Enums |
| `Record<string, T>` | `HashMap<String, T>` |
| `AbortSignal` | `CancellationToken` |
