# vercel-ai-anthropic

Anthropic (Claude) provider for Vercel AI SDK v4 — Messages API.

## SDK Spec

Implements the `@ai-sdk/anthropic` v4 specification. Baseline commit, mirror
scope, and intentional deviations: see [`../README.md`](../README.md). All
Anthropic-specific SDK concerns (prompt caching, beta headers, OAuth, policy
limits, 529 retry, cache breakpoint detection) belong in **this crate**, not
in `coco-inference` — see "Multi-Provider Boundaries" in the workspace
`CLAUDE.md`.

## Key Types (module)

| Type | Purpose |
|------|---------|
| `AnthropicProvider` + settings (`anthropic_provider`) | provider + factory: `anthropic()` (default) / `create_anthropic()` |
| `AnthropicMessagesLanguageModel` (`messages`) | the Messages API impl |
| `AnthropicConfig` (`anthropic_config`) | session-stable resolved request config (capabilities, topology, knobs, allowlist, account_kind, in_overage) |
| `AnthropicProviderOptionsConfig` + `parse_provider_options` (`provider_options`) | adapter-owned schema for `ProviderConfig.provider_options` — see "Per-instance knobs" |
| `AdapterAccountKind` / `AnthropicModelCapabilities` / `ProviderTopology` | adapter-local mirrors of `coco_types` enums (keeps the crate L0) |
| `CacheControlValidator` (`cache_control`) | max 4 `cache_control` breakpoints per request; positional rules (system → last_user → last_assistant) |
| `CachePolicy` (`cache_policy`) | `OnceLock` 1h-TTL eligibility + allowlist latches (session-stable) |
| `ResolvedBetas` / `resolve_betas` / `should_emit_context_management` (`beta_resolver`) | single source of truth for which betas a request emits |
| `map_capability`, `CLAUDE_CODE_BASELINE` (`beta_capabilities`) | typed enum ↔ kebab-case Anthropic header string |
| `compute_marker_index_post_group` / `attach_marker_at` (`cache_placement`) | auto cache-marker placement on last user content block |
| `forward_anthropic_container_id_from_last_step` (`forward_container_id`) | carries `container_id` across multi-step conversations (tool_use containers) |
| `tool` | Anthropic-specific tool types (computer_use, bash, text_editor, web_search, web_fetch, code_execution, …) |
| `anthropic_error` / `anthropic_metadata` | error mapping / provider-metadata extraction |

## Invariants

- **L0 layer rule:** the crate cannot import `coco-*` types. Inputs cross the
  boundary as adapter-local mirror enums (`AdapterAccountKind`,
  `AdapterCacheMode`, `AdapterCacheTtl`, `AdapterCacheScope`,
  `AdapterBetaCapability`) with **identical wire JSON** to `coco_types::*`.
  Translation happens in `services/inference::model_factory::build_anthropic`.
- **Single source of truth for context-management:** body insert / memory tool
  / `context-management-2025-06-27` beta header all gate on
  `beta_resolver::should_emit_context_management`. Half-emitted state is
  structurally impossible.
- **Internal-only signals never reach the wire:** the four internal signals
  (`cacheStrategy` / `requestedBetas` / `agenticQuery` / `querySource`) are
  typed fields on `AnthropicProviderOptions`, so the typed parse consumes them
  and `#[serde(flatten)] extra` captures only unrecognized keys (the
  structural replacement for the old `INTERNAL_ANTHROPIC_OPTION_KEYS`
  blacklist).
- **`extra_body` deep-merge escape hatch (F1 doctrine):**
  `provider_options["anthropic"]` (canonical) + `provider_options[<custom-prefix>]`
  (for renamed instances like `"my-proxy"`) extras deep-merge over typed body
  writes via `merge_json_value`; extras win at final-merge priority. Parsed
  via shared `extract_namespaced(po, "anthropic", provider_prefix)`. `null`
  in extras is a no-op (skips, does NOT unset). Per-key deep merge handles
  nested-struct fields independently (e.g. custom `cache_strategy.ttl` can
  override canonical `cache_strategy.mode`). Single source of truth:
  `services/inference/CLAUDE.md` "Design Notes".
- **Deterministic beta header:** `betas` is a sorted `BTreeSet` in
  `ResolvedBetas`; the wire header is `sort_unstable + join(',')` — byte-stable
  across runs.
- **Long-context credits errors are adapter-owned diagnostics:**
  `anthropic_error` detects Anthropic's 1M-context usage-credit rejection
  messages and sets the internal `LONG_CONTEXT_CREDITS_REQUIRED_HEADER` on
  `APICallError.response_headers`. That marker is not sent on the wire;
  `coco-inference` translates it into a typed rate-limit flag.

## Extended thinking

Exposed through `ProviderOptions` (`budget_tokens`, interleaved) — mapped from
`coco_types::ThinkingLevel` by `coco-inference::thinking_convert`.

- **No provider-layer fallback for `budget_tokens`:** when
  `provider_options["anthropic"]["thinking"]` arrives as `{"type":"enabled"}`
  without a budget, the wire body emits that shape verbatim (no key, no
  warning) and `max_tokens` stays at the model's `max_output_tokens`.
  ModelInfo is the single source of truth for budget: endpoints that require
  it (Anthropic first-party) MUST declare an explicit budget per
  `ThinkingLevel` in the registry; endpoints that don't (e.g. DeepSeek
  anthropic-compat) leave it `None`.
- **`ThinkingConfig::Disabled` serializes to
  `body["thinking"] = {"type":"disabled"}`** — the wire actively carries the
  explicit-off toggle (previously the variant was parsed but silently
  dropped).
- **Two parallel ways to set `body["output_config"]`:** the typed `effort`
  knob (`AnthropicProviderOptions.effort`) adds the `effort-2025-11-24` beta
  header; the convert-layer raw `output_config` key via extras deep-merge does
  not. coco-rs uses the extras path so DeepSeek-anthropic-compat (which
  rejects the beta) gets a clean wire body.
- **Adaptive thinking is pre-gated by the convert layer:**
  `thinking_convert::to_extra_body` only emits
  `provider_options["anthropic"]["thinking"] = {"type":"adaptive"}` when the
  registry declares `Capability::AdaptiveThinking`. The adapter's local
  `supports_adaptive_thinking` (model-name pattern) is only consulted by the
  typed-reasoning fallback path (`resolve_anthropic_reasoning_config`), which
  fires when `provider_options.thinking` is unset and `call.reasoning` is set
  — coco-rs always sets `provider_options.thinking` directly and bypasses that
  path.

## Per-instance knobs

Behavior knobs live under `ProviderConfig.provider_options` (opaque
`BTreeMap<String, Value>`), never in `coco-config`. Schema is owned here in
`provider_options.rs` (`deny_unknown_fields` catches typos at startup): four
typed bools — `experimental_betas`, `disable_interleaved_thinking`,
`show_thinking_summaries`, `non_interactive`. settings.json shape:
`providers.anthropic.provider_options.{…}`.
`model_factory::build_anthropic` calls `parse_provider_options` and threads
them into `AnthropicProviderSettings`. There are intentionally **no
`COCO_ANTHROPIC_*` env vars** — settings.json is canonical.

## Conventions

- Reads `ANTHROPIC_API_KEY` by default; settings allow OAuth token / custom
  headers.
