# vercel-ai-provider-utils

Shared utilities for implementing AI SDK v4 providers. Depends only on `vercel-ai-provider` for types.

## SDK Spec

Implements the `@ai-sdk/provider-utils` v4 specification.

## Key Types

API / fetch: `post_json_to_api[_with_client][_and_headers]`, `post_stream_to_api[_with_client][_and_headers]`, `get_from_api[_with_client]`, `ApiError`, `ApiResponse`, `ByteStream`, `DefaultErrorHandler`, `ErrorHandler`, `Fetch`, `FetchOptions`.

Response handlers: `ResponseHandler`, `JsonResponseHandler`, `StreamResponseHandler`, `TextResponseHandler`.

Headers / URL / media: `combine_headers`, `extract_header`, `normalize_headers`, `is_url_supported`, `parse_data_url`, `DataUri`, `parse_data_uri`, `MediaType`, `media_type_from_extension`, `without_trailing_slash`, `with_trailing_slash`, `normalize_url`, `build_user_agent`, `FormData`, `strip_extension`, `strip_specific_extension`.

Loading: `load_api_key`, `load_optional_api_key`, `load_setting`, `load_optional_setting`, `LoadAPIKeyError` (re-exported from provider).

JSON / schema: `parse_json`, `parse_json_event_stream`, `Schema`, `ValidationError`, `as_schema`, `json_schema`, `json_schema_from_type`, `schema_from_type`, `add_required_fields`, `merge_into_schema`, `GeneratedSchema`, `inject_json_instruction[_with_description]`, `inject_json_array_instruction`, `create_json_response_instruction`.

Tooling: `dynamic_tool`, `execute_tool`, `ExecutableTool`, `SimpleTool`, `ToolExecutionOptions`, `ToolRegistry`, `ToolMapping`, `generate_tool_call_id`, `parse_tool_call_id`.

Reasoning / validation: `map_reasoning_to_provider_budget`, `map_reasoning_to_provider_effort`, `is_custom_reasoning`, `validate_model_id`, `validate_tool_name`, `validate_download_url`, `is_valid_download_url`, `DownloadUrlError`, `download_file`.

IDs / timing / encoding: `generate_id`, `delay`, `parse_retry_after`, `convert_base64_to_bytes`, `convert_bytes_to_base64`, `convert_to_base64`, `get_error_message`, `VERSION`.

## Conventions

- Async-first: all I/O supports `CancellationToken`.
- Errors propagate as `AISdkError` from provider crate.
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
