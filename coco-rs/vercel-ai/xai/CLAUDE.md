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
Baseline commit, mirror scope, and intentional deviations: see
[`../README.md`](../README.md).

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

## Authentication and defaults

- `XaiConnection::ApiKey` owns both endpoint (default `https://api.x.ai/v1`) and key
  (fallback `XAI_API_KEY`); `XaiConnection::GrokSubscription` is a mutually exclusive
  connection profile.
- Grok subscription bearers are host-bound to
  `https://cli-chat-proxy.grok.com/v1`. Each request reads the live token and
  emits the proxy auth, client identity/mode/version, and model-override headers.
  The builtin uses the Responses API; `coco login grok` acquires its credential
  through xAI device authorization.
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

A second, opt-in surface (`responses/`, sub-provider id `xai.responses`) via
`XaiProvider::responses(model_id)`, posting to `/responses` in the OpenAI
Responses wire format (typed `input` items, `output` items, `response.*` SSE
lifecycle). Default routing stays on Chat Completions — `language_model()`
never touches Responses unless a caller explicitly asks. Key types:
`XaiResponsesLanguageModel`, `XaiResponsesProviderOptions` (namespace `"xai"`:
`reasoningEffort` / `reasoningSummary` / `logprobs` / `topLogprobs` / `store`
/ `previousResponseId` / `include`).

Differences from the Chat surface that matter when editing:

- **Request shape**: `max_output_tokens` (not `max_completion_tokens`);
  reasoning is a nested `{ effort, summary }` object; structured output goes
  to `text.format` (json_schema always `strict: true`); `store: false`
  auto-appends `reasoning.encrypted_content` to `include` so reasoning
  round-trips under Zero Data Retention.
- **Typed `input` items**: the prompt converts to `input_text` / `input_image`
  (+ `imageDetail`) / `input_file` (`file_url` or Files-API `file_id`) parts;
  assistant `text` / `tool-call` / `reasoning` round-trip their `itemId` and
  `reasoningEncryptedContent` through `provider_metadata["xai"]`; tool results
  become `function_call_output`. Non-image inline `data` and `text` file parts
  are hard errors; every other unsupported shape degrades to an `other`
  warning.
- **Reasoning** streams via `response.reasoning_summary_*` /
  `response.reasoning_text.*` blocks keyed by `item_id`, not a
  `reasoning_content` field; `do_generate` prefers the reasoning summary over
  the raw `content` channel.
- **Usage** (`convert_xai_responses_usage`): `reasoning_tokens` is *inclusive*
  in `output_tokens` (`output.text = output − reasoning`) — unlike the chat
  converter's additive treatment. The `input_tokens` cache-read
  inclusive/exclusive branch matches chat.
- **Server-side agentic tools** map by id (`xai.web_search` → `web_search`,
  `xai.x_search`, `xai.code_execution` → `code_interpreter`, `xai.view_image`,
  `xai.view_x_video`, `xai.file_search`, `xai.mcp`) and surface as
  provider-executed `tool-call` parts (plus a `tool-result` for
  `file_search`). Forcing a server-side tool via `tool_choice` warns and drops
  the choice (xAI only forces function tools).
- **Finish reason** uses the Responses `status` vocabulary (`completed` →
  EndTurn, `max_output_tokens` → MaxTokens, …); a `function_call` output
  forces `ToolUse`. Malformed chunks / `error` events → `Error` part + raw
  `"error"` (unified `Other`), matching chat. **`response.failed` is
  different** — a *server-declared* failure, so a missing/unmappable
  `incomplete_details.reason` maps to unified `Error` (raw `"error"`),
  matching the TS; an `Error` part also surfaces the failure message (a coco
  addition, mirroring the openai Rust model).

## Multimodal surfaces

Four additional surfaces (`image/`, `video/`, `speech/`, `transcription/`) via
`XaiProvider::image(id)` / `video(id)` / `speech(id)` / `transcription(id)`
(sub-provider ids `xai.image` / `xai.video` / `xai.speech` /
`xai.transcription`). Each splits body construction into a pure `plan_*`
function (unit-tested) + the HTTP call; all reuse `XaiFailedResponseHandler`;
wire schemas are minimal subsets, matching the chat convention. The
ship-a-bug invariants (per-endpoint options/defaults are readable from each
module):

- **Image**: `POST /images/generations`, or `/images/edits` when input files
  are present — a JSON body in both cases (xAI image edit is **not**
  multipart). Always requests `b64_json`; URL responses are downloaded and
  surfaced as `ImageData::Base64`. `max_images_per_call = 3`. Image options
  are snake_case, unlike the camelCase chat options (upstream inconsistency,
  mirrored deliberately).
- **Video**: async create → poll (`request_id`, bounded by `pollTimeoutMs`
  default 600s / `pollIntervalMs` default 5s). The `VideoModelV4` result
  carries no warnings/metadata channel, so TS warnings and `providerMetadata`
  are dropped. Unknown option keys pass through to the body (loose upstream
  schema) via `#[serde(flatten)] extra` + `merge_json_value`.
- **Speech** (`POST /tts`): unknown codecs warn and fall back to mp3;
  `bitRate` is mp3-only (warns otherwise); `instructions` is unsupported
  (warns).
- **Transcription, batch** (`do_transcribe`): multipart `POST /stt` with
  scalar option fields first and the audio **`file` field last** — an xAI
  requirement.
- **Transcription, streaming** (`do_stream`,
  `transcription/xai_transcription_stream.rs`): real-time WebSocket STT
  (`wss://…/stt?<params>`). On the server's `transcript.created` greeting a
  driver task pumps audio frames as binary messages (then
  `{"type":"audio.done"}`) while forwarding `transcript.partial` /
  `transcript.final` / `transcript.done` as
  `TranscriptionModelV4StreamPart`s. `multichannel` requires `channels` (hard
  error); `format` is batch-only (warns); an unrecognized input media type
  falls back to PCM (warns); errors are terminal. This surface required
  adding the streaming-transcription spec to `vercel-ai-provider` (defaulted
  `TranscriptionModelV4::do_stream` + stream types).
- Speech and transcription endpoints take **no model field** — upstream pins
  `""`; pass `""` for parity (the id is response metadata only).

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

- **Builtin providers** (`common/config/src/builtin/xai.rs`): both instances
  set `api = ProviderApi::Xai`. `xai` keeps API-key auth at
  `https://api.x.ai/v1`; `grok` uses OAuth subscription auth at
  `https://cli-chat-proxy.grok.com/v1` with `wire_api = Responses`. Users
  declare Grok models against either instance (no builtin model rows).
- **Model-factory dispatch** (`services/inference::model_factory`): a
  dedicated `ProviderApi::Xai` arm routes to `build_xai` →
  `vercel_ai_xai::create_xai` (unlike groq, which is name-checked inside the
  `OpenaiCompat` arm). The `services/inference` dep is seam-allowed
  (`check-vercel-ai-seam.sh` permits `vercel-ai-*` deps only in
  `services/inference` and `common/llm-types`).
- **`provider_options` namespace**: `ProviderApi::Xai` maps to the `"xai"`
  namespace in `build_call_options`, exactly the namespace this crate reads —
  so `reasoningEffort` / `logprobs` / `topLogprobs` /
  `parallel_function_calling` round-trip.
