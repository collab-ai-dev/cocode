# vercel-ai-anthropic

Anthropic (Claude) provider for Vercel AI SDK v4 — Messages API.

## SDK Spec

Implements the `@ai-sdk/anthropic` v4 specification. All Anthropic-specific SDK concerns (prompt caching, beta headers, OAuth, policy limits, 529 retry, cache breakpoint detection) belong in **this crate**, not in `coco-inference` — see the "Multi-Provider SDK" design decision in the workspace `CLAUDE.md`.

## Key Types

- `AnthropicProvider`, `AnthropicProviderSettings`, `anthropic()` (default), `create_anthropic()`
- `AnthropicConfig` — session-stable resolved request config (carries capabilities, topology, knobs, allowlist, account_kind, in_overage)
- `AdapterAccountKind`, `AnthropicModelCapabilities`, `ProviderTopology` — adapter-local mirrors of `coco_types` enums (kept here so `vercel-ai-anthropic` stays L0)
- `AnthropicProviderOptionsConfig`, `parse_provider_options`, `ProviderOptionsError` — adapter-owned schema for `ProviderConfig.provider_options`. Four typed bool knobs (`experimental_betas`, `disable_interleaved_thinking`, `show_thinking_summaries`, `non_interactive`) parsed from the opaque per-instance `BTreeMap<String, Value>`; `deny_unknown_fields` catches typos at startup.
- `AnthropicMessagesLanguageModel` — Messages API implementation
- `CacheControlValidator` — validates `cache_control` breakpoints (max 4 per request, positional rules)
- `CachePolicy` — `OnceLock` 1h-TTL eligibility + allowlist latches (R3-F3 session-stable)
- `ResolvedBetas`, `resolve_betas`, `should_emit_context_management` — single source of truth for which betas a request emits
- `map_capability`, `CLAUDE_CODE_BASELINE` — typed enum → kebab-case Anthropic header string
- `compute_marker_index_post_group`, `build_cache_control_value`, `attach_marker_at` — auto cache-marker placement
- `forward_anthropic_container_id_from_last_step` — carries `container_id` across multi-step conversations (for tool_use containers)

## Modules

- `anthropic_provider` — provider + settings + factory
- `anthropic_config` — session-stable resolved request config
- `anthropic_error` — provider-specific error mapping
- `anthropic_metadata` — provider metadata extraction
- `messages` — `AnthropicMessagesLanguageModel` (the language model impl)
- `tool` — Anthropic-specific tool types (computer_use, bash, text_editor, web_search, web_fetch, code_execution, etc.)
- `cache_control` — breakpoint validator
- `provider_options` — adapter-owned schema for `ProviderConfig.provider_options`; parses the per-instance opaque knob map into `AnthropicProviderOptionsConfig`
- `cache_policy` — 1h-TTL eligibility latch + per-call allowlist match
- `cache_placement` — auto-marker placement on last user content block (design §10.3)
- `beta_resolver` — capability + topology + knob → wire header set; central source of truth (R3-F2)
- `beta_capabilities` — typed enum ↔ Anthropic kebab-case header string
- `forward_container_id` — container_id forwarding helper

## Conventions

- Reads `ANTHROPIC_API_KEY` by default; settings allow OAuth token / custom headers.
- Cache control: enforce 4-breakpoint limit and positional rules (system → last_user → last_assistant) via `CacheControlValidator`.
- Extended thinking: exposed through `ProviderOptions` (budget_tokens, interleaved) — mapped from `coco_types::ThinkingLevel` by `coco-inference::thinking_convert`. **No provider-layer fallback for `budget_tokens`**: when `provider_options["anthropic"]["thinking"]` arrives as `{"type":"enabled"}` without a budget, the wire body emits the same shape verbatim (no key, no warning) and `max_tokens` is left at the model's `max_output_tokens`. ModelInfo is the single source of truth for budget; endpoints that require it (Anthropic first-party) MUST declare an explicit budget per `ThinkingLevel` in the registry. Endpoints that do not (e.g. DeepSeek anthropic-compat) leave it `None`. **`ThinkingConfig::Disabled` is serialized to `body["thinking"] = {"type":"disabled"}`** so the wire body actively carries the explicit-off toggle (previously the variant was parsed but silently dropped). The typed `effort` knob (`AnthropicProviderOptions.effort`) and the convert-layer raw `output_config` key are two parallel ways to set `body["output_config"]`: typed-knob path adds the `effort-2025-11-24` beta header; raw shallow-merge path does not. coco-rs convert layer uses the extras deep-merge path so DeepSeek-anthropic-compat (which doesn't accept the beta) gets a clean wire body. **Adaptive thinking is pre-gated by the convert layer**: `coco-inference::thinking_convert::to_extra_body` only emits `provider_options["anthropic"]["thinking"] = {"type":"adaptive"}` when the model declares `Capability::AdaptiveThinking` in the registry. The adapter's local `supports_adaptive_thinking` (via `get_model_capabilities` model-name pattern) is therefore only consulted by the typed-reasoning fallback path (`resolve_anthropic_reasoning_config`), which fires when `provider_options.thinking` is unset and `call.reasoning` is set — coco-rs always sets `provider_options.thinking` directly and bypasses that path.
- **L0 layer rule:** the crate cannot import `coco-*` types. Inputs cross the boundary as adapter-local mirror enums (`AdapterAccountKind`, `AdapterCacheMode`, `AdapterCacheTtl`, `AdapterCacheScope`, `AdapterBetaCapability`) with **identical wire JSON** to `coco_types::*`. Translation happens in `services/inference::model_factory::build_anthropic`.
- **Single source of truth for context-management:** body insert / memory tool / `context-management-2025-06-27` beta header all gate on `beta_resolver::should_emit_context_management`. Half-emitted state is structurally impossible (R3-F2).
- **Internal-only signals never reach the wire:** the four internal signals (`cacheStrategy` / `requestedBetas` / `agenticQuery` / `querySource`) are typed fields on `AnthropicProviderOptions`, so they're consumed by the typed parse and `#[serde(flatten)] extra` captures only unrecognized keys. Previously a hardcoded `INTERNAL_ANTHROPIC_OPTION_KEYS` blacklist stripped them — `#[serde(flatten)]` is the structural replacement. Extras now deep-merge onto the wire body via `merge_json_value`, so callers control nesting end-to-end.
- **`extra_body` deep-merge escape hatch (F1 doctrine).** `provider_options["anthropic"]` (canonical) + `provider_options[<custom-prefix>]` (custom for renamed instances like `"my-proxy"`) extras deep-merge over typed body writes via `merge_json_value`; extras win at final-merge priority. `#[serde(flatten)] extra` on `AnthropicProviderOptions` implements `ExtractExtras`, parsed via shared `extract_namespaced(po, "anthropic", provider_prefix)`. `null` in extras is a no-op (skips, does NOT unset). The previous hand-written per-`Option<T>` `.or()` chain in `merge_anthropic_options` is gone — per-key deep merge handles nested-struct fields more correctly (e.g. `cache_strategy.ttl` from custom can override canonical's `cache_strategy.mode` independently). Single source of truth: `services/inference/CLAUDE.md` "Design Notes".
- **Deterministic beta header:** `betas` is a sorted `BTreeSet` in `ResolvedBetas`; the wire header is `sort_unstable + join(',')` so output is byte-stable across runs (Finding 7).
- **Per-instance behavior knobs live under `ProviderConfig.provider_options`** (opaque `BTreeMap<String, Value>`), not in `coco-config`. Schema is owned here in `provider_options.rs` (`deny_unknown_fields`). Settings.json shape: `providers.anthropic.provider_options.{experimental_betas, disable_interleaved_thinking, show_thinking_summaries, non_interactive}`. `services/inference::model_factory::build_anthropic` calls `parse_provider_options` and threads the four typed bools into `AnthropicProviderSettings`. There are intentionally **no `COCO_ANTHROPIC_*` env vars** — settings.json is canonical.
