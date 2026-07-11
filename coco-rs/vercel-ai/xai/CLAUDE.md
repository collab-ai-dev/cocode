# vercel-ai-xai

xAI (Grok) provider for Vercel AI SDK v4. Chat Completions + Responses +
multimodal (image / video / speech / transcription) surfaces.

## SDK Spec

Faithful port of `@ai-sdk/xai`, covering Chat Completions, the Responses API,
and the multimodal surfaces (image / video / speech / batch + streaming
transcription).
Built on the coco-rs `vercel-ai-provider` traits and `vercel-ai-provider-utils`
transport (shared `reqwest::Client` + typed `ResponseHandler` + shared
`SseDecoder`), not a pluggable `fetch`.

## Key Types

- `XaiProvider`, `XaiProviderSettings`, `create_xai()`
- `XaiConfig` — per-model shared config (provider id, base URL, lazy headers, client)
- Models: `XaiChatLanguageModel`, `XaiResponsesLanguageModel`, `XaiImageModel`,
  `XaiVideoModel`, `XaiSpeechModel`, `XaiTranscriptionModel`
- Options (all namespace `"xai"`): `XaiChatProviderOptions`,
  `XaiResponsesProviderOptions`, `XaiImageProviderOptions`,
  `XaiVideoProviderOptions`, `XaiSpeechProviderOptions`,
  `XaiTranscriptionProviderOptions`
- Error: `XaiFailedResponseHandler`, `XaiErrorData` (union: `{error:{message,type}}` | `{code,error}`)

## Why a dedicated crate (vs. `openai-compatible`)

xAI is OpenAI-wire but diverges enough to warrant its own model impl:

- **Reasoning** surfaces through `reasoning_content` (response + stream deltas),
  emitted *after* text in `do_generate` (matches TS order). Streaming drops
  exact-duplicate consecutive reasoning deltas.
- **`max_completion_tokens`** instead of `max_tokens`.
- **Unsupported params warn**: `topK`, `frequencyPenalty`, `presencePenalty`,
  `stopSequences` are rejected by xAI's chat API — they emit a warning and are
  omitted from the body.
- **`reasoning_effort`** (provider option, namespace `"xai"`): passthrough incl.
  the literal `none`. Mapping from a top-level `ReasoningLevel`
  (`minimal|low → low`, `medium → medium`, `high|xhigh → high`, `off → none`)
  is gated by `supports_reasoning_effort(model_id)` — the `grok-4.20*reasoning`
  family rejects the param for every value, so a warning is emitted and no
  effort is sent. An explicit `reasoningEffort` option always wins.
- **`logprobs` / `top_logprobs`** (`logprobs` is only ever `true` or absent;
  `top_logprobs` implies it) and **`parallel_function_calling`**.
- **`json_schema`** response format is always sent with `strict: true` (no
  `structuredOutputs` gate, unlike groq).
- **Live-Search `citations`** (array of URLs) surface as `source` URL parts in
  both `do_generate` and `do_stream` (the stream delivers them in the final
  chunk).
- **Streaming tool calls arrive complete** in a single delta (id + name + full
  arguments, like Mistral) — each is emitted as
  `tool-input-start → tool-input-delta → tool-input-end → tool-call` with no
  cross-delta accumulation.

## Defaults

- Base URL `https://api.x.ai/v1`; API key from `XAI_API_KEY`.
- `language_model()` routes to Chat Completions (the coco-rs convention; the
  upstream `languageModel` default routes to the Responses API — see
  Non-goals). Embedding models return `NoSuchModelError`; `image_model()` /
  `video_model()` / `speech_model()` / `transcription_model()` route to the
  multimodal surfaces below.

## Conventions

- Usage conversion (`convert_xai_chat_usage`) treats `reasoning_tokens` as
  *additive* to `completion_tokens` (`output.total = completion + reasoning`,
  `output.text = completion`), matching the TS. Unlike the TS it surfaces the
  `cached_tokens` cache-read bucket via `InputTokens` so cost tracking stays
  consistent across coco providers; when `cached_tokens > prompt_tokens` the
  API is reporting them exclusively, so the total is `prompt + cache`.
- Multimodal tool results (`ToolResultContent::Content`) degrade non-text parts
  to a `[… omitted]` marker rather than dumping raw base64 into the prompt — a
  coco-wide convention that deviates from the TS (which `JSON.stringify`s the
  whole content array).
- Tool input schemas are sanitized via `remove_additional_properties_false`
  (xAI structured outputs reject `additionalProperties: false`).
- Streaming uses the shared `SseDecoder`; a malformed / error chunk surfaces as
  an `Error` stream part and sets the raw `finish_reason` to `"error"` (→ unified
  `Other`), matching the TS reference and the openai / openai-compatible /
  google / anthropic majority — the `Error` part is the real signal, not the
  finish reason.
- Wire schemas in `chat/xai_api_types.rs` are a minimal subset — only fields the
  impl reads — so upstream API additions don't break parsing.

## Responses API

The Responses surface (`responses/`) is a second, opt-in model reached via
`XaiProvider::responses(model_id)` (sub-provider id `xai.responses`). It posts
to `/responses` and mirrors the OpenAI Responses wire format: typed `input`
items, `output` items, and `response.*` SSE lifecycle events. Default routing
stays on Chat Completions — `language_model()` and the runtime never touch
Responses unless a caller explicitly asks for it.

Key types: `XaiResponsesLanguageModel`, `XaiResponsesProviderOptions`
(namespace `"xai"`: `reasoningEffort` / `reasoningSummary` / `logprobs` /
`topLogprobs` / `store` / `previousResponseId` / `include`).

How it differs from the Chat surface:

- **Request shape.** `max_output_tokens` (not `max_completion_tokens`);
  reasoning is a nested `{ effort, summary }` object; structured output goes to
  `text.format` (json_schema always `strict: true`); `store: false` auto-appends
  `reasoning.encrypted_content` to `include` so reasoning tokens round-trip under
  Zero Data Retention.
- **Typed `input` items.** The prompt converts to `input_text` / `input_image`
  (+ `imageDetail`) / `input_file` (`file_url` or Files-API `file_id`) parts;
  assistant `text` / `tool-call` / `reasoning` round-trip their `itemId` and
  `reasoningEncryptedContent` through `provider_metadata["xai"]`; tool results
  become `function_call_output`. Non-image inline `data` and `text` file parts
  are hard errors; every other unsupported shape degrades to an `other` warning.
- **Reasoning** streams via `response.reasoning_summary_*` /
  `response.reasoning_text.*` (start/delta/end blocks keyed by `item_id`), not a
  `reasoning_content` field. In `do_generate` the reasoning summary is preferred
  over the raw `content` channel.
- **Usage** (`convert_xai_responses_usage`) treats `reasoning_tokens` as
  *inclusive* in `output_tokens` (`output.text = output − reasoning`), unlike the
  chat converter's additive treatment. The `input_tokens` cache-read
  inclusive/exclusive branch matches chat, surfaced via `InputTokens`.
- **Server-side agentic tools** are supported here: provider-defined tools map
  by id (`xai.web_search` → `web_search`, `xai.x_search` → `x_search`,
  `xai.code_execution` → `code_interpreter`, `xai.view_image`, `xai.view_x_video`,
  `xai.file_search`, `xai.mcp`). They surface as provider-executed `tool-call`
  parts (and, for `file_search`, a `tool-result`). Forcing a server-side tool via
  `tool_choice` warns and drops the choice (xAI only forces function tools).
- **Finish reason** uses the Responses `status` vocabulary
  (`completed` → EndTurn, `max_output_tokens` → MaxTokens, …); a `function_call`
  output forces `ToolUse`. Malformed chunks and `error` events surface an `Error`
  stream part and set the raw `finish_reason` to `"error"` (→ unified `Other`),
  matching the chat surface. A **`response.failed`** event is different — it is a
  *server-declared* failure, so a missing/unmappable `incomplete_details.reason`
  maps to unified `Error` (raw `"error"`), matching the TS; an `Error` part also
  surfaces the failure message (a coco addition, mirroring the openai Rust
  model).

## Multimodal surfaces

Four additional model surfaces (`image/`, `video/`, `speech/`,
`transcription/`), reached via `XaiProvider::image(id)` / `video(id)` /
`speech(id)` / `transcription(id)` and the matching `ProviderV4` methods.
Sub-provider ids: `xai.image` / `xai.video` / `xai.speech` /
`xai.transcription`. Each surface splits body construction into a pure
`plan_*` function (unit-tested) and the HTTP call; all reuse
`XaiFailedResponseHandler` so errors surface identically to the chat /
responses transports. Wire schemas are minimal subsets, matching the chat
convention.

- **Image** (`XaiImageModel`, `grok-imagine-image` family):
  `POST /images/generations`, or `POST /images/edits` when input `files` are
  provided (files become plain URLs / `data:` URIs in a JSON body — xAI image
  edit is not multipart). Always requests `response_format: "b64_json"`; when
  the API returns URLs anyway, each is downloaded and surfaced as
  `ImageData::Base64` (the TS returns raw bytes — base64 is the equivalent
  data-bearing form here). `max_images_per_call = 3`. Warns on `size` (use
  `aspectRatio`), `seed`, `mask`. Options: `aspect_ratio` / `output_format` /
  `sync_mode` / `resolution` (`1k|2k`) / `quality` (`low|medium|high`) /
  `user` — snake_case keys, unlike the camelCase chat options (upstream is
  inconsistent here; we mirror it). `providerMetadata.xai` carries per-image
  `revisedPrompt` + `costInUsdTicks`.
- **Video** (`XaiVideoModel`, `grok-imagine-video` family): async create →
  poll. `POST /videos/generations` (or `/videos/edits` / `/videos/extensions`
  per the resolved mode) returns a `request_id`, polled via
  `GET /videos/{request_id}` until `done` / `failed` / `expired`, bounded by
  `pollTimeoutMs` (default 600s) at `pollIntervalMs` (default 5s) intervals.
  Mode resolution mirrors the TS: explicit `mode` wins, bare `videoUrl` →
  edit-video, bare `referenceImageUrls` (1-7) → reference-to-video. Edit mode
  drops `duration` + `resolution`; extend mode drops `resolution`. Unknown
  option keys pass through to the body (loose schema upstream) via
  `#[serde(flatten)] extra` + `merge_json_value`. The `VideoModelV4` result
  carries no warnings / metadata channel, so TS warnings (fps, seed, n > 1,
  unrecognized resolution) and `providerMetadata` (requestId, duration,
  costInUsdTicks, progress) are dropped.
- **Speech** (`XaiSpeechModel`): `POST /tts`, JSON body
  (`text` / `voice_id` default `eve` / `language` default `auto` /
  `output_format {codec, sample_rate?, bit_rate?}` / `speed` /
  `optimize_streaming_latency` / `text_normalization`), binary audio
  response. Codecs: mp3 (default) / wav / pcm / mulaw / alaw — unknown
  formats warn and fall back to mp3; `bitRate` is mp3-only (warns otherwise);
  `instructions` is unsupported (warns).
- **Transcription** (`XaiTranscriptionModel`): both paths of the upstream model.
  - **Batch** (`do_transcribe`): multipart `POST /stt` with scalar option
    fields first and the audio **`file` field last** (an xAI requirement).
    Options: `audioFormat` / `sampleRate` / `language` / `format` /
    `multichannel` / `channels` / `diarize` / `keyterm` (string or list,
    repeated field) / `fillerWords`. Response maps `words[]` → segments;
    empty-string `language` is treated as absent.
  - **Streaming** (`do_stream`, `transcription/xai_transcription_stream.rs`):
    real-time WebSocket STT. Opens `wss://…/stt?<params>` (built from the same
    options plus the `streaming` sub-object: `interimResults` / `endpointing` /
    `smartTurn` / `smartTurnTimeout`); on the server's `transcript.created`
    greeting a driver task pumps audio frames as binary messages (then
    `{"type":"audio.done"}`) while forwarding `transcript.partial` /
    `transcript.final` / `transcript.done` events as
    `TranscriptionModelV4StreamPart`s. `multichannel` requires `channels`
    (hard error); `format` is batch-only (warns); an unrecognized input media
    type falls back to PCM (warns). Errors are terminal (`error` event / socket
    error → stream `Err`). This required adding the streaming spec to
    `vercel-ai-provider` (`TranscriptionModelV4::do_stream` as a defaulted trait
    method + `TranscriptionModelV4Stream{Options,Part,Result}`).
- Speech and transcription endpoints take **no model field** — upstream
  exposes `speech()` / `transcription()` without a model id and pins `""`;
  pass `""` for parity (the id is response metadata only).

## Non-goals (deferred surfaces of `@ai-sdk/xai`)

Ported: **Chat Completions**, the **Responses API**, and the **multimodal
surfaces** (image / video / speech / batch **and streaming** transcription).
Not ported (available upstream, add later if needed): the **realtime voice
model** (`realtime/xai-realtime-model.ts` — the `experimental_realtime`
factory / `RealtimeModelV4` bidirectional voice conversation, distinct from
streaming STT) and the Files API. The deprecated Live-Search `searchParameters`
request option is intentionally omitted — xAI's endpoint now rejects it.

## Wiring (runtime-reachable)

This crate is dispatched at runtime:

- **Builtin provider** `xai` (`common/config/src/builtin/xai.rs`): `api =
  OpenaiCompat`, `base_url = https://api.x.ai/v1`, `env_key = XAI_API_KEY`. Users
  declare Grok models against it (no builtin model rows).
- **Model-factory dispatch** (`services/inference::model_factory`): the
  `OpenaiCompat` arm name-checks `coco_config::builtin::XAI_PROVIDER` and routes
  to `build_xai` → `vercel_ai_xai::create_xai`. The `services/inference` dep is
  seam-allowed (`check-vercel-ai-seam.sh` permits `vercel-ai-*` deps only in
  `services/inference` and `common/llm-types`).
- **`provider_options` namespace**: the instance name `xai` makes
  `canonical_namespace_key(OpenaiCompat, "xai")` wrap options under `"xai"`,
  exactly the namespace this crate reads — so `reasoningEffort` / `logprobs` /
  `topLogprobs` / `parallel_function_calling` round-trip.

The config layer keeps `api = OpenaiCompat`, so no `ProviderApi` variant is
added.
