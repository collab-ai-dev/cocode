# vercel-ai-provider-utils

Shared utilities for implementing AI SDK v4 providers. Depends only on `vercel-ai-provider` for types.

## SDK Spec

Implements the `@ai-sdk/provider-utils` v4 specification. Baseline commit,
mirror scope, and intentional deviations: see [`../README.md`](../README.md).

## Domains

One line per domain — browse `lib.rs` for the full surface.

- API / fetch: `post_json_to_api*` / `post_stream_to_api*` / `get_from_api*` + `ResponseHandler` family (`Json` / `Stream` / `Text`), `ApiError`.
- SSE framing: `SseDecoder` — see below.
- Headers / URL / media: `combine_headers` / `normalize_headers`, data-URI + media-type parsing, URL normalization, `FormData`.
- Loading: `load_api_key` / `load_setting` (+ `_optional` variants).
- JSON / schema: `parse_json`, `Schema` / `json_schema_from_type`, JSON-instruction injection helpers.
- Tooling: `dynamic_tool`, `execute_tool`, tool-call ID generate/parse, `StreamingToolCallTracker` (OpenAI-delta streaming tool-call accumulation, arguments-before-name safe).
- Reasoning / validation: `map_reasoning_to_provider_{budget,effort}`, model-ID / tool-name / download-URL validation.
- IDs / timing / encoding: `generate_id`, `delay`, `parse_retry_after`, base64 conversions.

## SseDecoder

Bounded UTF-8-safe byte→event decoder shared by every async SSE provider.
It owns event framing (`event:`, multi-line `data:`, CRLF, comments, EOF) and
rejects invalid UTF-8 or an oversized event before provider JSON decoding.
Distinct from the blocking `parse_json_event_stream` (a `std::io::Read`
iterator used off the async path).

## Conventions

- Async-first: all I/O supports `CancellationToken`.
- Errors propagate as `AISdkError` from the provider crate.
- Header handling canonicalizes keys via `normalize_headers` before combining.

## Coco-rs-specific deviations from the spec

- **`json_repair` module** — `parse_with_repair` /
  `parse_tool_arguments_or_empty` wrapping the third-party `llm_json`
  crate (markdown fence stripping, single-quote → double-quote,
  trailing-comma fix, Python-literal mapping, truncation completion).
  Not in the spec — coco-rs adds aggressive
  repair because it targets diverse OpenAI-compatible endpoints
  (GLM, Doubao, DeepSeek, Groq, xAI, Ollama) whose tool-call
  `arguments` strings are messier than first-party Anthropic /
  OpenAI output. Adapters in `vercel-ai-openai*` and
  `vercel-ai-anthropic`'s streaming `content_block_stop` branch
  call this helper inline; failure still falls back to
  `Value::Object({})` (schema validation reports
  specific missing fields on the next turn). See
  `services/inference/CLAUDE.md` "Call path" for the full 3-layer
  story. Parallel implementation `coco-utils-json-repair` lives one
  layer higher (`utils/`) and is used by `app/query` for schema-validation
  work; the duplication exists because layering forbids
  `vercel-ai-provider-utils` from depending on `coco-*` crates.
  Both wrappers delegate to `llm_json::repair_json` so drift is
  bounded.
