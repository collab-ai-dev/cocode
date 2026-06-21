# Prompt Cache & Anthropic Beta Header Design

> Status: Target design
> Scope: `coco-rs/common/types/`, `coco-rs/common/config/`, `coco-rs/services/inference/`, `coco-rs/vercel-ai/anthropic/`, `coco-rs/app/query/`
> Sources: `services/api/claude.ts`, `utils/{betas,api}.ts`, `constants/betas.ts`, `services/api/promptCacheBreakDetection.ts`
> Owners: coco-inference + vercel-ai-anthropic + coco-types + coco-config (capability flags) + coco-query (intent assembly)
>
> **Source of truth.** This document owns the cross-crate prompt-cache feature
> design: capability declaration, beta-header injection, cache-marker
> placement algorithm, and TTL policy. Type definitions for new shared types
> (`PromptCacheConfig`, `BetaCapability`, `AccountKind`, `CacheTtl`,
> `CacheStrategy`) propagate to `crate-coco-types.md` after acceptance.
> Existing types (`CacheBreakDetector`, `CacheScope`, `Capability`,
> `ModelInfo`, `ApiClient`, `build_call_options`) remain owned by their
> respective crate docs; this design only specifies how they integrate.
>
> **Design stance.** Mirror TS Claude Code behavior at the wire level
> (cache marker positions, TTL latch semantics, beta header gates) while
> preserving coco-rs multi-provider invariants: provider concerns stay in
> `vercel-ai-<provider>`, `services/inference` stays provider-neutral, no
> Anthropic-shaped JSON leaks into other providers' namespaces.

## 0. TL;DR — Final Design

Prompt cache is a **two-axis Anthropic-only feature** in coco-rs:

```
                           Family gate           Capability gate
  ProviderApi::Anthropic   (wire shape supports  (this model declares
  family of APIs           cache_control blocks) prompt cache support)
                           │                     │
                           └──────── AND ────────┘
                                     │
                                     ▼
                       supports_prompt_cache() == true
                                     │
                                     ▼
       services/inference is OPAQUE PASS-THROUGH (no policy interpretation).
       It writes typed PER-CALL data to provider_options["anthropic"]:
         • cacheStrategy: { mode, requested_ttl, scope, skipCacheWrite }
         • agenticQuery: bool      (per-call: main agent loop vs helper)
         • querySource: String     (for 1h-TTL allowlist match)
         • requestedBetas: BTreeSet<BetaCapability>  (user top-up)
       SESSION-STABLE data flows via AnthropicConfig (set at provider
       construction by build_anthropic from RuntimeConfig.account.*):
         • account_kind: ApiKey | ClaudeAiSubscriber
         • in_overage:   bool       (subscriber overage flag)
       Plus the capability gate ApiClient::supports_prompt_cache().
       NO UserType / Entrypoint forwarded — Ant gates dropped (see §3.5).
       Session-stable separation prevents first-call latch corruption (R3-F3).
                                     │
                                     ▼
       vercel-ai-anthropic adapter owns ALL policy + wire translation:
         • cache_policy::resolve_ttl() — TS should1hCacheTTL mirror,
           latches eligibility + allowlist (NOT the final TTL)
         • beta_resolver::resolve() — TS getAllModelBetas + getMergedBetas
           mirror, memoized via OnceLock on AnthropicMessagesLanguageModel
         • cache_placement::compute_marker_index() — TS markerIndex algo,
           applies cache_control to last (or last-1) message
         • beta_capabilities::map_capability() + baseline_betas() merge
           into existing betas accumulator (sorted before header join)
         • CacheControlValidator enforces 4-breakpoint cap (existing)
                                     │
                                     ▼
       Wire: headers["anthropic-beta"] = "...,context-1m-2025-08-07,..."
             body.system[*].cache_control = { type: "ephemeral" }
             body.tools[*].cache_control  = { type: "ephemeral" }
             body.messages[N].cache_control = { type: "ephemeral", ttl: "1h" }
```

**Why this shape (vs alternatives considered):**

| Decision | Rejected alternative | Reason |
|---|---|---|
| Two-axis gate (`ProviderApi` AND `Capability`) | `ProviderApi::Anthropic` alone | Mirrors TS `getAPIProvider()` + per-model gates (`modelSupportsISP`, `getPromptCachingEnabled(model)` env switches per Sonnet/Opus/Haiku). Family alone cannot disable a single model. |
| `Capability::PromptCache` enum variant | Boolean flag on `ModelInfo` | `Capability` is the existing closed-list mechanism for model declarations. Same pattern as `ExtendedThinking`, `FastMode`, `StructuredOutput`. |
| `provider_options["anthropic"][cacheStrategy]` typed | `extra_body["cache_control"]` user-configured raw | High-level `cache_strategy` auto-places markers via TS-mirror algorithm; user low-level `cache_control` remains as escape hatch (existing `AnthropicProviderOptions.cache_control` field at `anthropic_messages_options.rs:132`). |
| `BetaCapability` enum + table mapping | User-supplied raw `Vec<String>` only | Typed enum encodes which betas exist. The **adapter** (`vercel-ai-anthropic::beta_resolver`) maps `Capability` → `BetaCapability` → wire string, NOT inference; `services/inference::cache_convert` only forwards the *user-requested-top-up* (`requested_betas` from `PromptCacheConfig`) verbatim through `provider_options`. Raw `anthropic_beta: Option<Vec<String>>` retained as escape hatch (existing field at `anthropic_messages_options.rs:144`). |
| `cache_policy` + `beta_resolver` modules live in `vercel-ai-anthropic` | Either crate could host them | `coco-rs/services/inference/CLAUDE.md:3` explicitly lists Anthropic prompt-cache + beta policy as **must not** live in inference; `utils/betas.ts` and `claude.ts` are listed at `:13` as "Intentionally NOT ported here". `services/inference` is opaque pass-through; the adapter owns policy. |
| Beta resolution memoized on `AnthropicMessagesLanguageModel` (provider-instance state) | `ApiClient.resolved_betas: OnceLock<…>` (inference-side) | Same lifecycle equivalence (one language-model per `(provider, model)` resolution; hot-reload rebuilds), but keeps memoization next to the consumer that uses it, in line with the boundary above. |
| Eligibility + allowlist latched (NOT the final TTL) | Latch the resolved `CacheTtl` in a `OnceLock` | Mirrors TS `should1hCacheTTL` precisely: `getPromptCache1hEligible` and `getPromptCache1hAllowlist` are session-state, but the per-`querySource` allowlist match runs every call (`claude.ts:393-433`). Latching the final TTL would force every later request to the first call's decision, regardless of `querySource`. |
| Account / agentic / overage / querySource flow through `provider_options` as typed data | Read globals/env inside `vercel-ai-anthropic` | The adapter must remain stateless w.r.t. the host process; coco-rs has no globals like TS `process.env`. Data is computed by the host (e.g., `app/query`), forwarded through `QueryParams` and `provider_options` opaquely by `services/inference`, interpreted by the adapter. |
| Marker placement: TS algorithm (1 message-level marker on last msg, system per-block via `cacheScope`) | Naive "last system + last tool + last message" | TS places exactly **one** message-level marker (or last-1 for `skipCacheWrite`); system blocks are per-block via `cacheScope: 'global' \| 'org' \| null`; tools opt-in individually. The 4-breakpoint cap is enforced by `CacheControlValidator` as a safety net, not as the placement strategy. |
| All model-conditional logic flows through `Capability` enum + per-call flags | TS-style model-name string match (`is_haiku`, `claude-3-*` patterns) | coco-rs is multi-LLM; matching on Anthropic-internal model name conventions doesn't generalize. The semantic of TS `!isHaiku` is "this model runs the main agent loop, not helper calls" — coco-rs encodes that via the per-call `agentic_query` flag instead. The semantic of `modelSupportsISP` / `has1mContext` etc. is "this model supports feature X" — coco-rs encodes that via `Capability::InterleavedThinking` / `Context1m` etc. on `ModelInfo`. **No `is_haiku` heuristic anywhere.** |
| Ant-gated betas not ported (`cli-internal-2026-02-09`, `summarize-connector-text-*`, ant-only `context-management` opt-in) | Forwarding `UserType::Ant` to gate them | `Ant` is an Anthropic-internal experimental flag. coco-rs is an open multi-LLM SDK; real users never set it, and surfacing the concept in our wire format for these betas pollutes the API for no public-user benefit. We **don't port these betas at all**. **No `UserType::Ant` cleanup proposed (R3-F7)** — the variant is still actively consumed by `coco-rs/skills/src/bundled.rs:144` and `coco-rs/core/permissions/`; this design simply does not depend on it. |
| `Entrypoint` not forwarded by this design | Forwarding it as a typed wire field | `Entrypoint` only gated `cli-internal` in TS; with that beta dropped, no remaining gate uses it. Keep `coco-types::Entrypoint` for telemetry/logging; just don't pull it into `provider_options.anthropic`. |
| `RedactThinking` gates on `provider_topology == FirstParty` — NOT on `UserType::Ant` | `user_type::Ant` gate | TS `betas.ts:270-277` actually gates `redact-thinking-2026-02-12` on `includeFirstPartyOnlyBetas` (which is `getAPIProvider() === 'firstParty' \|\| 'foundry'`), not on `USER_TYPE === 'ant'`. An earlier draft conflated the two. The gate is "first-party Anthropic API" — captured by `ProviderTopology::FirstParty` (the only topology this iteration ships) — not by user category. |
| Drop `Capability::RedactThinking` — fold into `Capability::InterleavedThinking` | Keep separate variant | TS `betas.ts:272` gates `redact-thinking` on `modelSupportsISP(model)` — the same predicate as `interleaved-thinking`. Models that support ISP also support redact-thinking; it's a display/UI variant of the same underlying feature. One capability is enough. |
| Inject `capabilities: AnthropicModelCapabilities` (bool struct) into `AnthropicConfig` at provider construction | Look up `ModelInfo` from the registry inside the adapter | The adapter has no handle to `coco-config`; the only way it learns model facts is via `AnthropicConfig`. The provider factory (`services/inference/src/model_factory.rs::build_language_model_from_runtime` → `build_anthropic` at lines 196-217) already has `ResolvedModel` (so `ModelInfo.capabilities: Option<Vec<Capability>>`) — translating into the adapter-side bool struct in `model_factory.rs::anthropic_caps_from` keeps the dependency arrow correct (config → vercel-ai-anthropic, never the other way) and honors the F8 invariant that `vercel-ai-anthropic` cannot import `coco_types::Capability`. The bool-struct shape is preferred over `Vec<adapter_enum>` because the per-model fact set is small (5 flags today) and matches the existing `supports_*: Option<bool>` pattern on `AnthropicConfig`. |
| Internal `provider_options.anthropic` keys are stripped before raw shallow-merge | Trust the typed `extract_anthropic_options` to ignore them | `extract_anthropic_options` (`anthropic_messages_options.rs:214-228`) deliberately keeps every input key in `raw` for forward-compat with unknown user fields; that map is then shallow-merged into the request body at line 772. Without an explicit deny-list, `cacheStrategy` / `requestedBetas` / `agenticQuery` / `querySource` would ship to Anthropic, which would either reject the body or silently ignore them. The design adds an `INTERNAL_ANTHROPIC_OPTION_KEYS` constant (4 entries; `accountKind`/`inOverage` are NOT in it because R3-F3 moved them to session-stable `AnthropicConfig`) that the extractor strips from the raw map. |
| `provider_topology: ProviderTopology { FirstParty }` field on `AnthropicConfig` | Conflate auth mode (`AccountKind`) with endpoint topology | `AccountKind` answers "how is the user paying?" (`ApiKey` / `ClaudeAiSubscriber`). TS gates `shouldIncludeFirstPartyOnlyBetas` and `shouldUseGlobalCacheScope` on the *endpoint topology* (`getAPIProvider() === 'firstParty' \|\| 'foundry'` vs `'firstParty'` only — `betas.ts:215, :227`). Bedrock / Foundry / Vertex / proxy are deferred (Non-Goal §2): `ProviderTopology` ships with **one variant** (`FirstParty`) — kept as an enum (not collapsed to a bool) so the future Bedrock PR adds a variant without touching every gate site. |
| `build_call_options` invoked exactly once per `query()` call | Build twice (Phase 1 detector + Phase 2 retry loop) | Detector hash and retry body MUST agree, otherwise the recorded "before" snapshot mismatches the actual wire body and break detection silently misfires. The refactor caches `(LanguageModelV4CallOptions, BTreeMap<String, Value>)` in a local before the retry loop, recomputes nothing inside the loop. |
| `session_context_to_extra_body` is no-op when `cache_strategy` is absent or `Disabled` | Always emit `agenticQuery` / `querySource` per call | Otherwise `query_source` change re-hashes `extra_body_hash` even for callers that never enabled cache — failing the `query_source_change_does_NOT_change_hash_when_strategy_disabled` test in §14.1 and inflating cache-break false positives. The session context is *load-bearing only when caching is on*; gate it accordingly. |

**Pattern parity with existing `ThinkingLevel` / `context_management`:** typed data in `coco-inference` Layer 2, JSON across the crate boundary via `provider_options[<namespace>]`, deserialized into `AnthropicProviderOptions` in the provider crate. The new `cache_convert.rs` (in inference) mirrors `thinking_convert.rs`; new `cache_policy.rs` + `beta_resolver.rs` (in `vercel-ai-anthropic`) own all Anthropic-specific policy.

**Implementation cost:** ~6 new files (1 in `services/inference`: `cache_convert.rs`; 5 in `vercel-ai-anthropic`: `cache_policy.rs`, `beta_resolver.rs`, `cache_placement.rs`, `beta_capabilities.rs`, `system_block_scope.rs`); ~5 new fields on `AnthropicProviderOptions`; **5 new `Capability` enum variants** (`PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`, `TokenEfficientTools`); net +700–900 LoC of impl + tests.

## 1. Goals

In priority order:

1. **TS behavioral fidelity** — cache marker positions, TTL latch semantics, beta header gates, cache-break detection thresholds match the TS source exactly.
2. **Provider isolation** — Anthropic-specific wire knowledge (cache_control JSON shape, beta header strings, marker placement algorithm) lives only in `vercel-ai-anthropic`. `services/inference` stays provider-neutral.
3. **Capability-driven** — whether a model supports prompt cache is declarative on `ModelInfo.capabilities`, not hardcoded by name match.
4. **Multi-provider safe** — non-Anthropic providers never see `cache_strategy` or `beta_capabilities` keys in their `provider_options` namespace; no Anthropic JSON leaks.
5. **Memoization parity** — beta resolution is computed once per `ApiClient` lifecycle (mirrors TS `memoize(model)`); 1h TTL eligibility is latched per session (mirrors TS `setPromptCache1hEligible`).
6. **Escape hatches preserved** — existing `AnthropicProviderOptions.cache_control` and `.anthropic_beta` raw fields remain functional for advanced users and forward-compat with new betas.

## 2. Non-Goals

- **No port of TS GrowthBook integration.** TS reads `tengu_prompt_cache_1h_config` allowlist from a remote feature gate. coco-rs reads the same shape from a local config slot (`~/.coco/config.json`); remote feature-gate plumbing is out of scope.
- **No Bedrock / Vertex / Foundry routing in this design.** TS distinguishes 5 endpoint topologies (`firstParty` / `foundry` / `vertex` / `bedrock` / `proxy`) and splits some betas into `extraBodyParams` for Bedrock (`betas.ts:371-384`). coco-rs ships only `ProviderTopology::FirstParty` in this iteration; **no `Bedrock` variant**, no Bedrock auth flow, no `bedrock_1h_env` field, no Bedrock branch in `cache_policy::resolve_ttl`, no Bedrock split in `beta_resolver`. When Bedrock auth eventually lands, that PR adds back the variant + `AccountKind::Bedrock` + `bedrock_1h_env` + the TTL Bedrock branch + the beta split — all five together so half-implementations are unrepresentable.
- **No Ultraplan / advanced ant-only paths.** Same exclusions as `crate-coco-inference.md`.
- **No new cache-break detection algorithm.** Existing `CacheBreakDetector` (owned by `crate-coco-inference.md`) auto-handles new keys via `canonical_extra_body_hash`. This design only verifies threshold parity (5% drop + `MIN_CACHE_MISS_TOKENS`) with TS `promptCacheBreakDetection.ts`.
- **No port of TS `addCacheBreakpoints` second-message logic for `consumedCacheEdits` / `consumedPinnedEdits`.** Those are micro-compaction artifacts owned by `crate-coco-compact.md`; integration deferred to a follow-up doc.

## 3. TS Reference Behavior (Specification)

These are the load-bearing behaviors from the TS source. TS file paths are
relative to the TS project's `src/` directory. coco-rs must mirror them.

### 3.1 Cache marker placement

`claude.ts:3089-3105` — `addCacheBreakpoints` core:

```typescript
const markerIndex = skipCacheWrite ? messages.length - 2 : messages.length - 1
const result = messages.map((msg, index) => {
  const addCache = index === markerIndex
  if (msg.type === 'user') {
    return userMessageToMessageParam(msg, addCache, enablePromptCaching, querySource)
  }
  return assistantMessageToMessageParam(msg, addCache, enablePromptCaching, querySource)
})
```

**Exactly one** message-level marker per request. `addCache=true` is true for exactly one index.

`claude.ts:588-631` — `userMessageToMessageParam`: when `addCache=true`, marker attaches to the **last content block** of the message.

`claude.ts:633-668` — `assistantMessageToMessageParam`: same, but skip if last block is `'thinking'` / `'redacted_thinking'` / connector-text.

`utils/api.ts:321-359` — system blocks use **per-block** `cacheScope: 'global' | 'org' | null`. Multiple system blocks may each carry a marker independently.

`utils/api.ts:228-230` — tool schemas opt-in individually via `toolToAPISchema(opts.cacheControl)`.

### 3.2 TTL resolution with session latch

`claude.ts:358-373` — `getCacheControl({ scope, querySource })`:

```typescript
return {
  type: 'ephemeral',
  ...(should1hCacheTTL(querySource) && { ttl: '1h' }),
  ...(scope === 'global' && { scope }),
}
```

`claude.ts:393-433` — `should1hCacheTTL`:

```typescript
function should1hCacheTTL(querySource?: QuerySource): boolean {
  if (getAPIProvider() === 'bedrock' && isEnvTruthy(process.env.ENABLE_PROMPT_CACHING_1H_BEDROCK)) {
    return true
  }
  let userEligible = getPromptCache1hEligible()              // session-state read
  if (userEligible === null) {                               // first call
    userEligible = process.env.USER_TYPE === 'ant' ||
                   (isClaudeAISubscriber() && !currentLimits.isUsingOverage)
    setPromptCache1hEligible(userEligible)                   // LATCH
  }
  if (!userEligible) return false
  let allowlist = getPromptCache1hAllowlist()
  if (allowlist === null) {
    allowlist = getFeatureValue_CACHED_MAY_BE_STALE('tengu_prompt_cache_1h_config', {}).allowlist ?? []
    setPromptCache1hAllowlist(allowlist)                     // LATCH
  }
  return querySource !== undefined &&
         allowlist.some(p => p.endsWith('*')
           ? querySource.startsWith(p.slice(0, -1))
           : querySource === p)
}
```

**Critical invariant:** mid-session changes to `USER_TYPE`, subscription state, or overage flag must NOT change the 1h TTL decision after the first request. Otherwise the TTL flip invalidates ~20K tokens of cached prefix.

### 3.3 Cache enable per-model

`claude.ts:333-356` — `getPromptCachingEnabled(model)`:

```typescript
if (isEnvTruthy(process.env.DISABLE_PROMPT_CACHING)) return false
if (isEnvTruthy(process.env.DISABLE_PROMPT_CACHING_HAIKU) && model === getSmallFastModel()) return false
if (isEnvTruthy(process.env.DISABLE_PROMPT_CACHING_SONNET) && model === getDefaultSonnetModel()) return false
if (isEnvTruthy(process.env.DISABLE_PROMPT_CACHING_OPUS) && model === getDefaultOpusModel()) return false
return true
```

No model→capability map; **all Claude models** (including `claude-3-*`) support prompt cache. The TS function returns `true` unconditionally unless an env var disables a specific model variant (Haiku / Sonnet / Opus default). coco-rs mirrors this by declaring `Capability::PromptCache` on every Claude model in the builtin registry; per-model disable is a user override in `~/.coco/models.json` (which can drop the capability), not a hardcoded model-name check.

### 3.4 Beta header memoization

`betas.ts:234` — `getAllModelBetas` and `getModelBetas` are `memoize(model => ...)`. Same model returns same array for the session; `clearBetasCaches()` invalidates. (TS also has `getBedrockExtraBodyParamsBetas` at `betas.ts:371-384`, which moves a subset of betas from headers to `extraBodyParams` for Bedrock — this code path is **not ported** because Bedrock is deferred §2.)

`betas.ts:397-428` — `getMergedBetas(model, { isAgenticQuery })`:

```typescript
const baseBetas = [...getModelBetas(model)]
if (options?.isAgenticQuery) {
  if (!baseBetas.includes(CLAUDE_CODE_20250219_BETA_HEADER)) {
    baseBetas.push(CLAUDE_CODE_20250219_BETA_HEADER)
  }
  if (process.env.USER_TYPE === 'ant' &&
      process.env.CLAUDE_CODE_ENTRYPOINT === 'cli' &&
      CLI_INTERNAL_BETA_HEADER &&
      !baseBetas.includes(CLI_INTERNAL_BETA_HEADER)) {
    baseBetas.push(CLI_INTERNAL_BETA_HEADER)
  }
}
const sdkBetas = getSdkBetas()
return sdkBetas?.length ? [...baseBetas, ...sdkBetas.filter(b => !baseBetas.includes(b))] : baseBetas
```

### 3.5 Beta source matrix

From `betas.ts:234-368` and `constants/betas.ts`. The "Gate (TS)" column is what TS does literally; the "Gate (coco-rs translation)" column is how coco-rs encodes the same semantic without model-string matching or Ant gates.

| Beta string | Gate (TS) | Gate (coco-rs translation) |
|---|---|---|
| `claude-code-20250219` | `!isHaiku` (base) ∪ `isAgenticQuery` (topup) — combined `!isHaiku \|\| isAgenticQuery` | **`agentic_query`** alone. TS uses `!isHaiku` as a model-role heuristic ("Haiku is the small/fast helper, doesn't run the main agent loop"). coco-rs has explicit roles (`ModelRole::Main` vs `ModelRole::Fast`) and a per-call `agentic_query` flag; helper calls (compaction, title gen, classify) pass `false` and skip this beta. No model-string match. |
| `cli-internal-2026-02-09` | `(USER_TYPE === 'ant' && ENTRYPOINT === 'cli') && (!isHaiku \|\| isAgenticQuery)` | **NOT PORTED.** Anthropic-internal experimental gate; real coco-rs users never opt in. Dropping it removes both the `UserType::Ant` and `Entrypoint::Cli` dependencies from this design. |
| `summarize-connector-text-*` | `USER_TYPE === 'ant' && firstParty && ...` | **NOT PORTED.** Same reason. |
| ant-only `context-management` opt-in (`USE_API_CONTEXT_MANAGEMENT && USER_TYPE === 'ant'`) | as written | **NOT PORTED.** Capability-derived `context-management-2025-06-27` (model-driven branch in TS) is ported via `Capability::ContextManagement`; the ant-only opt-in branch is not. |
| OAuth beta | `isClaudeAISubscriber()` |
| `context-1m-2025-08-07` | `has1mContext(model)` |
| `interleaved-thinking-2025-05-14` | `!DISABLE_INTERLEAVED_THINKING && modelSupportsISP(model)` |
| `redact-thinking-2026-02-12` | `includeFirstPartyOnlyBetas + modelSupportsISP + !nonInteractive + !showThinkingSummaries` — `includeFirstPartyOnlyBetas` = `(provider == firstParty \|\| foundry) && !DISABLE_EXPERIMENTAL_BETAS` (`betas.ts:215`). coco-rs gate: `provider_topology == FirstParty && experimental_betas_enabled && capabilities.contains(InterleavedThinking) && !non_interactive && !show_thinking_summaries`. **Foundry not modeled** in this iteration — only `FirstParty` of the two TS topologies that pass `includeFirstPartyOnlyBetas` is represented (Non-Goal §2). No separate `Capability::RedactThinking` — folded into `InterleavedThinking` since TS uses the same predicate. |
| `prompt-caching-scope-2026-01-05` | `getAPIProvider() == 'firstParty' && !DISABLE_EXPERIMENTAL_BETAS` (`betas.ts:227-232`; **strict firstParty**, not foundry). coco-rs gate: `provider_topology == FirstParty && experimental_betas_enabled`. Since coco-rs only models `FirstParty` (Bedrock/Foundry/Vertex deferred §2), the gate matches redact-thinking by coincidence — the gate semantics remain distinct on a future Foundry expansion. |
| `context-management-2025-06-27` | `shouldIncludeFirstPartyOnlyBetas() && modelSupportsContextManagement(model)` (TS `betas.ts:307-311`). coco-rs gate: `provider_topology == FirstParty && experimental_betas_enabled && capabilities.context_management`. **Two emission sites** — body path (typed `context_management` field, `anthropic_messages_language_model.rs:710`) AND tool path (`anthropic.memory_20250818` registration, `prepare_tools.rs:236`); both must call the shared `beta_resolver::should_emit_context_management` predicate (Round-3 Finding 2). |
| `structured-outputs-2025-12-15` | model + feature gate |
| `token-efficient-tools-2026-03-28` | mutually exclusive with structured-outputs |
| `web-search-2025-03-05` | Vertex-only with Claude 4+ |
| `fast-mode-2026-02-01` | fast-mode active |
| `tool-search-tool-2025-10-19` / `advanced-tool-use-2025-11-20` | provider-dependent |
| `effort-2025-11-24`, `task-budgets-2026-03-13` | feature gates |

**Bedrock split** (`betas.ts:371-384`, where `INTERLEAVED_THINKING`, `CONTEXT_1M`, `TOOL_SEARCH_BETA_HEADER_3P` move from headers to `extraBodyParams`) is **not ported** in this iteration — see Non-Goal §2.

### 3.6 Cache-break detection

`promptCacheBreakDetection.ts:494` — break threshold:

```typescript
const tokenDrop = prevCacheRead - cacheReadTokens
if (cacheReadTokens >= prevCacheRead * 0.95 || tokenDrop < MIN_CACHE_MISS_TOKENS) {
  state.pendingChanges = null
  return  // not a break
}
```

**Two simultaneous conditions** to flag a break:
1. `cacheReadTokens < prevCacheRead * 0.95` (>5% drop)
2. `tokenDrop >= MIN_CACHE_MISS_TOKENS` (absolute threshold)

Tracked per-(querySource, agentId). Haiku excluded.

## 4. coco-rs Current State

What's already in place (no changes required):

### 4.1 In `vercel-ai/anthropic/`

| Concern | File:line | Notes |
|---|---|---|
| `CacheControlValidator` (4-breakpoint cap) | `cache_control.rs:1-130` | `MAX_CACHE_BREAKPOINTS = 4`, threaded through message conversion + tool prep |
| `AnthropicProviderOptions` typed extension slot | `messages/anthropic_messages_options.rs:122-149` | Already has `cache_control: Option<CacheControlConfig>` and `anthropic_beta: Option<Vec<String>>` raw escape hatches |
| `extract_anthropic_options` (canonical + custom-name namespaces) | `messages/anthropic_messages_options.rs:171-229` | Returns `(typed, raw_btreemap)` |
| Beta accumulator | `messages/anthropic_messages_language_model.rs:413-763` | `betas: HashSet<String>`, accumulated from feature flags + raw user betas, emitted at line 763 |
| Cache_control attachment per-block | `messages/convert_to_anthropic_messages.rs:189-340` | System / user-text / user-file / tool-result / assistant-text / tool-call all have hooks |
| Function tool cache_control | `messages/prepare_tools.rs:114-124` | Uses validator |
| Usage parsing | `messages/convert_anthropic_usage.rs:13-85` | `cache_creation_input_tokens` / `cache_read_input_tokens` mapped to unified `Usage.InputTokens` |

### 4.2 In `services/inference/`

| Concern | File:line | Notes |
|---|---|---|
| `ApiClient` carries `model_info: Option<ModelInfo>` and `fingerprint: ProviderClientFingerprint` | `client.rs:108-158` | Both fixed at construction; hot-reload rebuilds the client |
| `build_call_options` — single ProviderOptions write site | `build_call_options.rs:67-165` | Lane A (typed) + Lane B (`extra_body` merge) + Lane C (Anthropic context_management) + namespace wrap |
| `canonical_namespace_key(api, provider_name)` | `build_call_options.rs:177-184` | Family-based for Anthropic/OpenAI/Gemini; instance-name for compat SDKs |
| `merge_into_extra` deep merge with proto-pollution filter | `build_call_options.rs:186-204` | Auto-handles new keys without registration |
| `thinking_convert::to_extra_body(level, api)` | `thinking_convert.rs:27-89` | Pattern to mirror for cache |
| `supports_server_side_context_edits()` | `client.rs:257-259` | Existing capability gate; pattern to mirror |
| `provider_options_namespace()` | `client.rs:267-276` | Returns hardcoded namespace per `ProviderApi` |
| `CacheBreakDetector` + `canonical_extra_body_hash` | `cache_detection.rs:774-784` | Provider-agnostic; auto-detects new `extra_body` keys |
| `UsageAccumulator` | `usage.rs` | Already aggregates `cache_read_input_tokens` / `cache_creation_input_tokens` |

### 4.3 In `common/types/` and `common/config/`

| Type | File:line | Notes |
|---|---|---|
| `Capability` enum (11 variants) | `common/types/src/provider.rs:138-152` | Closed list; serde `snake_case`; **needs new variants** |
| `ProviderApi` enum (6 variants) | `common/types/src/provider.rs:9-32` | `Anthropic`, `Openai`, `Gemini`, `Volcengine`, `Zai`, `OpenaiCompat` |
| `ModelInfo.capabilities: HashSet<Capability>` | `common/config/src/model/mod.rs` | Per-model declarations |
| Builtin model registry | `common/config/src/model/registry.rs:216-447` | `seed_builtin_models()` — claude-sonnet-4-6, claude-opus-4-7, claude-haiku-4-5 do **not** declare prompt-cache yet |
| `RuntimeConfig.features: Features` | `common/types/src/features.rs:64-122` | Already has `PromptCacheBreakDetection` variant (gates the detector itself, separate from this design) |

## 5. Three TS-Fidelity Corrections

This design replaces three earlier-considered approaches that diverged from TS:

### 5.1 Marker placement is NOT "last system + last tool + last message"

**Wrong:** unconditionally place markers on the last system block, last tool, and last user message.

**Correct (TS-mirror):**
- **Messages:** exactly one marker, on `messages[N-1]` (or `messages[N-2]` for `skipCacheWrite`), on the last content block, skipping if last block is thinking-typed.
- **System:** per-block decision via `cacheScope`. A system array of [attribution-header, cli-prefix, dynamic-context] yields zero, one, or two markers depending on each block's `cacheScope`.
- **Tools:** opt-in per tool — caller decides which tool definitions carry `cache_control`. Not "the last tool".
- **Cap:** the 4-breakpoint limit is a safety net enforced by `CacheControlValidator`, not the placement algorithm.

### 5.2 Beta resolution is memoized, owned by the adapter

**Wrong (v1):** memoize on `ApiClient` (in `services/inference`).

**Correct (TS-mirror):** TS uses `memoize(model => ...)` so `getAllModelBetas` returns the same `string[]` for the entire session. coco-rs achieves the same shape by storing resolution on the **provider-instance** struct, not on `ApiClient`:

```rust
// in coco-rs/vercel-ai/anthropic/src/messages/anthropic_messages_language_model.rs
pub struct AnthropicMessagesLanguageModel {
    model_id: String,
    config: Arc<AnthropicConfig>,             // existing — extended (§10.0)
    resolved_betas: OnceLock<ResolvedBetas>,  // NEW — mirrors TS memoize(getAllModelBetas)
    cache_policy: CachePolicy,                // NEW — owns eligibility/allowlist latches
}
```

`AnthropicMessagesLanguageModel` instance lifetime ≈ `ApiClient` lifetime (one per `(provider, model)` resolution; hot-reload rebuilds both via `ProviderClientFingerprint`). Behaviorally identical to TS, but respects the boundary: `services/inference/CLAUDE.md:3` requires Anthropic policy to live in `vercel-ai-anthropic`, so the memo lives there too.

**Capability access path.** The adapter cannot reach into `coco-config` for `ModelInfo.capabilities` (would invert the dependency arrow). Instead, the provider factory copies the resolved capability vec into `AnthropicConfig` at construction (§10.0); the adapter reads `self.config.capabilities` — a plain `Vec<Capability>`, no cross-crate import.

### 5.3 Eligibility and allowlist latch — but NOT the final TTL

**Wrong (v1):** "first decision freezes for the rest of the session" — implemented as `OnceLock<CacheTtl>`.

**Why wrong:** that latches the entire `CacheTtl` decision, including the per-`querySource` allowlist match. First call's `querySource` would dictate every later call's TTL. A non-allowlisted first call forces every subsequent call to 5 minutes; an allowlisted first call forces 1 hour onto sources that should not get it.

**Correct (TS-mirror, `claude.ts:393-433`):** TS latches **only** two pieces of session state:

1. **Eligibility** (`getPromptCache1hEligible` / `setPromptCache1hEligible`) — derived once from `USER_TYPE === 'ant'` or `(isClaudeAISubscriber && !inOverage)`. Frozen on first call so mid-session overage flips don't invalidate ~20K-token server-side caches.
2. **Allowlist** (`getPromptCache1hAllowlist` / `setPromptCache1hAllowlist`) — the `tengu_prompt_cache_1h_config.allowlist` array, frozen on first read so disk-cache updates of the feature gate don't change it mid-request.

The **per-call match** (`allowlist.some(p => match(querySource, p))`) is recomputed every call, varying with each request's `querySource`.

```rust
// Correct shape: latches inside the policy struct, not on ApiClient
pub(crate) struct CachePolicy {
    eligible_1h: OnceLock<bool>,        // session latch
    allowlist:   OnceLock<Vec<String>>, // session latch
}
// resolve_ttl(querySource, ...) is called per-request; the OnceLocks above
// short-circuit re-derivation but the per-querySource match is fresh.
```

No `OnceLock<CacheTtl>` exists anywhere in the design.

## 6. Layer Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│ Layer A — User intent                       (common/types)             │
│   PromptCacheConfig { mode, ttl, scope, requested_betas, …}            │
│   AccountKind { ApiKey, ClaudeAiSubscriber }             (NEW)         │
│       (Bedrock variant deferred §2 — added with Bedrock auth PR)       │
│   CacheTtl { FiveMinutes, OneHour }                      (NEW)         │
│   CacheScope { Org, Global }                             (NEW)         │
│   PromptCacheMode { Disabled, Auto, Manual }             (NEW)         │
│   BetaCapability { Context1m, InterleavedThinking, … }   (NEW)         │
│   No UserType or Entrypoint dependency — Ant gates dropped (§3.5).     │
└────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Layer B — Capability declaration             (common/types + config)   │
│   Capability::PromptCache | Context1m | InterleavedThinking |          │
│                ContextManagement | TokenEfficientTools                 │
│   ModelInfo.capabilities — declared per-model in builtin registry      │
│   (3 builtin claude-4 models seeded; older / non-builtin claude        │
│    variants take the user-override path via ~/.coco/models.json)       │
│   redact-thinking-2026-02-12 is NOT a separate Capability — adapter    │
│   emits the beta when InterleavedThinking + FirstParty + interactive.  │
└────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Layer C — services/inference  (PASS-THROUGH ONLY, no policy)           │
│   ApiClient::supports_prompt_cache() — family AND capability gate      │
│   QueryParams { cache, agentic, query_source } — request-level inputs  │
│   build_anthropic reads RuntimeConfig.account.* + RuntimeConfig.       │
│     anthropic_knobs.* + RuntimeConfig.prompt_cache.*, writes them to   │
│     AnthropicConfig at provider construction (session-stable, R3-F3).  │
│   cache_convert::to_extra_body(cfg, api) — emits typed PER-CALL keys   │
│       to provider_options["anthropic"]; non-Anthropic providers       │
│       skipped. NO accountKind / inOverage emitted (session-stable).    │
│   build_call_options Lane B: deep-merge into existing flat extra map.  │
│   Detector: hash the merged extra_body map (refactor §12) so per-call  │
│       keys are tracked. (Session-stable AnthropicConfig fields require │
│       a separate detector input — see §9.7.4 R3-F5.)                  │
│   ✗ NO cache_policy, NO beta_resolver, NO ResolvedBetas, NO TTL latch. │
│   ✗ NO is_haiku / model-string heuristic anywhere.                     │
│   ✗ NO UserType / Entrypoint forwarding — Ant gates dropped.           │
└────────────────────────────────────────────────────────────────────────┘
                          │ JSON across crate boundary (per-call only)
                          ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Layer D — vercel-ai-anthropic  (POLICY + MEMO + WIRE)                  │
│   AnthropicProviderOptions (per-call) {                                │
│     cache_strategy, agentic_query, query_source, requested_betas       │
│   }                                                                    │
│   AnthropicConfig (session-stable, set by build_anthropic) {           │
│     capabilities, provider_topology, experimental_betas_enabled,       │
│     disable_interleaved_thinking, show_thinking_summaries,             │
│     non_interactive, prompt_cache_allowlist,                           │
│     account_kind, in_overage   ← R3-F3 session-stable                  │
│   }                                                                    │
│   cache_policy.rs       — TS should1hCacheTTL mirror; latches          │
│       eligibility + allowlist (NOT the final TTL).                     │
│   beta_resolver.rs      — capability + account-driven betas, memoized  │
│       on AnthropicMessagesLanguageModel.resolved_betas.                │
│   cache_placement.rs    — TS markerIndex algorithm.                    │
│   beta_capabilities.rs  — BetaCapability enum → wire string + sort     │
│       betas before joining for stable header order. Only per-call      │
│       gate is agentic_query (claude-code baseline). No model strings,  │
│       no Ant gates.                                                    │
│   system_block_scope.rs — per-block cacheScope plumbing.               │
│   Existing CacheControlValidator + 4-cap untouched.                    │
│   Existing escape hatches (cache_control, anthropic_beta) preserved.   │
│   AnthropicMessagesLanguageModel holds OnceLock state for memoization. │
└────────────────────────────────────────────────────────────────────────┘
```

## 7. Layer A — Type Definitions

New types live in `coco-types` since they are shared across `coco-config`, `services/inference`, and `app/query` (≥3 crates per the doc-map rule). **`vercel-ai-anthropic` does NOT import `coco_types`** — per the layer rules, vercel-ai/* is L0 and cannot depend on coco-* (L1+). The adapter defines structurally-equivalent local mirror types (`AdapterCacheTtl`, `AdapterCacheScope`, `AdapterCacheMode`, `AdapterAccountKind`, `AdapterBetaCapability`, `AnthropicModelCapabilities`); the boundary is JSON, mirroring the existing `ThinkingLevel ↔ ThinkingConfig` precedent in `services/inference/src/thinking_convert.rs`. The translator lives in `services/inference/src/model_factory.rs::build_anthropic` (the only crate that legitimately holds both sides of the boundary).

`coco-rs/common/types/src/cache.rs` (new):

```rust
use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    #[default]
    Disabled,
    /// Provider auto-places cache markers per its strategy.
    /// Anthropic: TS-mirror algorithm (last message + per-block system + opt-in tools).
    Auto,
    /// Caller controls placement via SystemPromptBlock::CacheBreakpoint hints.
    /// (Defined in provider-prompt-role-architecture.md.)
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    #[default]
    Org,
    Global,  // requires firstParty + prompt-caching-scope-2026-01-05 beta
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PromptCacheConfig {
    pub mode: PromptCacheMode,
    /// User-requested TTL. Adapter may downgrade to FiveMinutes when not eligible
    /// (TS should1hCacheTTL semantics). Adapter never upgrades.
    pub ttl: CacheTtl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CacheScope>,
    /// User-requested beta top-up. Adapter merges with capability-derived betas.
    /// Mirrors TS getSdkBetas() input to getMergedBetas.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub requested_betas: BTreeSet<BetaCapability>,
    /// TS skipCacheWrite — shifts marker to messages[N-2] for fire-and-forget queries.
    #[serde(default)]
    pub skip_cache_write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BetaCapability {
    Context1m,
    InterleavedThinking,
    ContextManagement,
    StructuredOutputs,
    TokenEfficientTools,
    FastMode,
    PromptCachingScope,    // global cache scope; firstParty-only
    RedactThinking,        // emitted by adapter; capability gate is InterleavedThinking + first-party
    Advisor,
    // Future-proof: never add a beta here without a TS source citation.
    // Ant-only betas (cli-internal-2026-02-09, summarize-connector-text-*, afk-mode-2026-01-31)
    // are intentionally NOT enumerated — coco-rs does not surface Anthropic-internal experimental
    // gates to public users.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// Direct API key (ANTHROPIC_API_KEY).
    #[default]
    ApiKey,
    /// OAuth subscriber (Claude.ai login). Drives OAuth beta + 1h-TTL eligibility.
    ClaudeAiSubscriber,
    // Bedrock variant intentionally NOT present in this iteration —
    // the adapter has no Bedrock endpoint plumbing today (Finding F3,
    // Non-Goal §2). When Bedrock auth lands, that PR adds back
    // AccountKind::Bedrock together with ProviderTopology::Bedrock,
    // AnthropicConfig::bedrock_1h_env, and the cache_policy::resolve_ttl
    // Bedrock branch — all in one PR so a half-implementation is
    // unrepresentable.
}
```

**`AccountKind` is auth/billing only** — no `Ant`, no `Bedrock` variant in this iteration.

### 7.1 What this design deliberately does NOT consume from coco-types

- **`coco_types::UserType`** — the existing enum has variants `Human`, `Api`, `Ant`. `Ant` is an Anthropic-internal experimental flag with no public semantics; this design does not gate any beta or TTL on it. The `cli-internal-2026-02-09`, `summarize-connector-text-*`, and ant-only `context-management` opt-in betas are **not ported**. coco-rs publishes a multi-LLM SDK; users never have a reason to identify as Ant.
- **`coco_types::Entrypoint`** — only relevant in TS for `cli-internal-2026-02-09`. With that beta dropped, no remaining gate consults the entrypoint. The enum stays in `coco-types` for telemetry / logging; this design simply doesn't pull it into `provider_options.anthropic`.

**No follow-up cleanup (Round-3 Finding 7).** This design **does not** consume `UserType::Ant`, but other in-tree consumers remain: `coco-rs/skills/src/bundled.rs:144` (ant-only skill bundle), `coco-rs/core/permissions/src/dangerous_rules.rs:25` + `mode_transition.rs:138` + `setup.rs:233` + `shell_rules.rs:265` (the `is_ant_user` plumbing through dangerous-rule strip). An earlier draft proposed deleting `UserType::Ant`; that proposal is **withdrawn** because deletion would break those existing consumers. The correct framing is "this design does not depend on `UserType::Ant`; existing consumers are not invalidated and not in scope here."

`Capability` enum extension (`coco-rs/common/types/src/provider.rs:138-152`) — additions to the existing 11 variants:

```rust
pub enum Capability {
    // ... existing 11 variants ...

    /// Model supports Anthropic-style cache_control breakpoints.
    PromptCache,

    /// Model supports 1M-token context window (`context-1m-2025-08-07` beta).
    Context1m,

    /// Model supports interleaved thinking blocks (`interleaved-thinking-2025-05-14` beta).
    /// Also gates `redact-thinking-2026-02-12` per TS `betas.ts:272`
    /// (`modelSupportsISP` predicate is shared between the two betas).
    InterleavedThinking,

    /// Model supports server-side context management (`context-management-2025-06-27` beta).
    /// Required for `services/inference::supports_server_side_context_edits` to apply.
    ContextManagement,

    /// Model supports token-efficient tool format (`token-efficient-tools-2026-03-28` beta).
    /// Mutually exclusive with structured outputs.
    TokenEfficientTools,
}
```

**Doc-update consequence:** `crate-coco-types.md` Capability list grows by 5 variants (`PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`, `TokenEfficientTools`). This is a non-breaking serde change (new variants). No `Capability::RedactThinking` — the redact-thinking beta uses the `InterleavedThinking` capability + adapter-side first-party check.

## 8. Layer B — Capability Declaration

Declared per-model in `seed_builtin_models()` at `coco-rs/common/config/src/model/registry.rs:216-447`. **Only the canonical models coco-rs ships with are seeded here**; every other model (older Claude variants, third-party Claude proxies, future versions) goes through the user-override path.

### 8.1 Builtin model seeds

| Builtin model | Capabilities added (delta to existing) |
|---|---|
| `claude-sonnet-4-6` | `PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement` |
| `claude-opus-4-7`   | `PromptCache`, `InterleavedThinking`, `ContextManagement` |
| `claude-haiku-4-5`  | `PromptCache`, `ContextManagement` |
| GPT / Gemini / xAI / ByteDance / etc. | (no `PromptCache` capability — wire shape doesn't support it via this design) |

The **table is exhaustive for builtins** — coco-rs does not enumerate every Claude variant. Older models (`claude-3-5-sonnet`, `claude-3-haiku`, etc.) and future models declare their capabilities through the user-override path below.

### 8.2 Justification per capability

Capabilities are property statements about a model, not heuristics on its name:

| coco-rs capability | TS source-of-truth function | What it actually means |
|---|---|---|
| `Capability::PromptCache` | `getPromptCachingEnabled(model)` (`claude.ts:333-356`) | Wire supports `cache_control` blocks. TS returns true for **all** Claude models (incl. 3.x); env vars selectively disable per-default-model. |
| `Capability::Context1m` | `has1mContext(model)` | Model accepts the `context-1m-2025-08-07` beta. |
| `Capability::InterleavedThinking` | `modelSupportsISP(model)` (`betas.ts:92-112`) | Model supports interleaved thinking blocks. TS excludes `claude-3-*` here — but that's **InterleavedThinking-specific**, not prompt-cache. |
| `Capability::ContextManagement` | `modelSupportsContextManagement(model)` (`betas.ts:125-139`) | Model supports `context-management-2025-06-27` beta. |
| `Capability::TokenEfficientTools` | feature-gated in TS | Model supports `token-efficient-tools-2026-03-28` beta. |

Note: TS `redact-thinking-2026-02-12` reuses the `modelSupportsISP` predicate (`betas.ts:272`) — i.e., the same models that support interleaved-thinking. coco-rs gates it on `Capability::InterleavedThinking + provider_topology == FirstParty + !non_interactive + !show_thinking_summaries`; no separate capability variant.

### 8.3 Override path for non-builtin models

Users using a model not in §8.1 (e.g., `claude-3-5-sonnet`, a third-party Anthropic-compatible proxy, a self-hosted Claude finetune, a future Claude version not yet recognized) declare capabilities via `~/.coco/models.json`:

```jsonc
{
  "providers": {
    "anthropic": {
      "models": {
        "claude-3-5-sonnet": {
          "capabilities": ["text_generation", "streaming", "tool_calling", "prompt_cache"]
        }
      }
    }
  }
}
```

`ModelRegistry::try_resolve` (Layer 2 user overlay, see `crate-coco-config.md`) merges this with the builtin seed. **No model-name pattern matching** lives in coco-rs source code — claude-3 vs claude-4 vs custom-claude-tuned are all the same path: `ModelInfo.capabilities.contains(...)`. To disable prompt cache on a specific model (TS `DISABLE_PROMPT_CACHING_HAIKU=1` analog), drop `prompt_cache` from that model's `capabilities` in `~/.coco/models.json`.

## 9. Layer C — services/inference (Pass-Through Only)

`services/inference` MUST stay Anthropic-policy-free per the crate's own invariant (`coco-rs/services/inference/CLAUDE.md:3, :13`). It does only three things for prompt cache:

1. Capability gate (`supports_prompt_cache()`) — declarative gate based on `ProviderApi` + `Capability::PromptCache`.
2. Wire-key emission (`cache_convert::to_extra_body`) — pass-through serialization of `PromptCacheConfig` and account context to `provider_options["anthropic"]`. **Zero policy interpretation.**
3. Detector hash refactor — make `CacheBreakDetector` see all merged provider-options keys, not just `context_management`.

No `cache_policy`, `beta_resolver`, `ResolvedBetas`, or any TTL latch lives here.

### 9.1 ApiClient capability gate

Add to `coco-rs/services/inference/src/client.rs` (sibling to existing `supports_server_side_context_edits` at line 257-259):

```rust
impl ApiClient {
    /// Two-axis gate: provider family supports cache_control wire shape AND
    /// model declares prompt-cache capability.
    ///
    /// Mirrors TS: getAPIProvider() check (family) + getPromptCachingEnabled(model)
    /// (per-model env switches, encoded as Capability::PromptCache).
    pub fn supports_prompt_cache(&self) -> bool {
        if !matches!(self.fingerprint.api, ProviderApi::Anthropic) {
            return false;
        }
        // ModelInfo.capabilities is `Option<Vec<Capability>>` (model/mod.rs:50).
        // None = unknown model: be permissive at this gate so the mock/test
        // path keeps working; the actual emission is gated again in the
        // adapter (§10.4) by `self.config.capabilities.prompt_cache`, which
        // is False for unknown models — so the real wire-level guard never
        // depends on the permissiveness here.
        self.model_info
            .as_ref()
            .map(|m| m.capabilities.as_ref()
                .is_some_and(|caps| caps.contains(&Capability::PromptCache)))
            .unwrap_or(true)  // mock/test path
    }
}
```

### 9.2 Wire-key emission (pure pass-through)

New file `coco-rs/services/inference/src/cache_convert.rs` — emits typed JSON keys to `provider_options["anthropic"]`. This is the **only** new module in `services/inference`. It performs zero policy decisions.

```rust
use std::collections::BTreeMap;
use coco_types::{ProviderApi, PromptCacheConfig, PromptCacheMode, AccountKind};
use serde_json::{json, Value};

/// Pass-through emission of cacheStrategy + requestedBetas.
/// Returns empty map for non-Anthropic providers (other namespaces stay clean).
pub(crate) fn to_extra_body(
    cfg: &PromptCacheConfig,
    api: ProviderApi,
) -> BTreeMap<String, Value> {
    if cfg.mode == PromptCacheMode::Disabled || !matches!(api, ProviderApi::Anthropic) {
        return BTreeMap::new();
    }
    let mut m = BTreeMap::new();
    m.insert("cacheStrategy".into(), json!({
        "mode": cfg.mode,
        "ttl":  cfg.ttl,                  // requested TTL; adapter may downgrade
        "scope": cfg.scope,
        "skipCacheWrite": cfg.skip_cache_write,
    }));
    if !cfg.requested_betas.is_empty() {
        m.insert("requestedBetas".into(), serde_json::to_value(&cfg.requested_betas).unwrap());
    }
    m
}

/// Pass-through emission of per-call session context. The adapter consumes
/// these as opaque data and applies its own policy.
/// No userType / entrypoint — see §3.5 (Ant-gated betas not ported).
/// No accountKind / inOverage — those are session-stable and live on
/// AnthropicConfig, not in per-call provider_options (Round-3 Finding 3).
///
/// **Gated on a non-disabled cache strategy** (Finding 4 fix). Without this
/// gate, `query_source` would re-hash `extra_body_hash` for callers that
/// never enabled caching, breaking the
/// `query_source_change_does_NOT_change_hash_when_strategy_disabled` test.
/// The session context is load-bearing only when caching is on.
pub(crate) fn session_context_to_extra_body(
    cache_cfg:    Option<&PromptCacheConfig>,
    agentic:      bool,
    query_source: Option<&str>,
    api:          ProviderApi,
) -> BTreeMap<String, Value> {
    if !matches!(api, ProviderApi::Anthropic) {
        return BTreeMap::new();
    }
    // Finding 4 gate: no cache → no session context.
    let active = matches!(cache_cfg, Some(c) if c.mode != PromptCacheMode::Disabled);
    if !active {
        return BTreeMap::new();
    }
    let mut m = BTreeMap::new();
    m.insert("agenticQuery".into(), Value::Bool(agentic));
    if let Some(qs) = query_source {
        m.insert("querySource".into(), Value::String(qs.into()));
    }
    m
}
```

Caller in §9.4 passes only `cache_cfg`, `agentic_query`, `query_source` — `account_kind` / `in_overage` are NOT forwarded per-call (Round-3 Finding 3); they reach the adapter via `AnthropicConfig` set at provider construction.

### 9.3 PerCallOverrides extension

Add to `coco-rs/services/inference/src/build_call_options.rs:46-65`:

```rust
pub struct PerCallOverrides {
    // ... existing 7 fields ...

    /// Layer A user intent. Translated to provider-specific keys via cache_convert.
    pub cache_strategy: Option<PromptCacheConfig>,

    /// Per-call agentic flag — gates `claude-code-20250219` baseline.
    pub agentic_query: bool,

    /// Per-call query source — for 1h-TTL allowlist match (TS parity).
    pub query_source:  Option<String>,

    // **Round-3 Finding 3:** `account_kind` and `in_overage` are NOT
    // per-call — they're session-stable on `AnthropicConfig`, set by
    // `build_anthropic` from `RuntimeConfig.account.*` at provider
    // construction. Including them as per-call fields would let a
    // missing first call silently corrupt the eligibility latch.
}
```

No `beta_capabilities: Vec<BetaCapability>` field in `PerCallOverrides` — beta resolution happens entirely in the adapter. Inference only forwards `cfg.requested_betas` (user top-up) inside `cacheStrategy`. No `user_type` or `entrypoint` — Ant-gated betas dropped (§3.5).

### 9.4 build_call_options merge

Inject after the existing thinking merge (after line 125 in `build_call_options.rs`):

```rust
// EXISTING: Lane A (typed sampling) at lines 82-91
// EXISTING: Lane A2 (typed reasoning) at lines 93-111
// EXISTING: Lane B (extra_body merge) at lines 113-125

// NEW: cache_strategy
if let Some(ref cache_cfg) = per_call.cache_strategy {
    for (k, v) in cache_convert::to_extra_body(cache_cfg, api) {
        merge_into_extra(&mut extra, &k, &v);
    }
}

// NEW: session context (emit only when cache is active)
for (k, v) in cache_convert::session_context_to_extra_body(
    per_call.cache_strategy.as_ref(),
    per_call.agentic_query,
    per_call.query_source.as_deref(),
    api,
) {
    merge_into_extra(&mut extra, &k, &v);
}

// EXISTING: Lane C (Anthropic context_management) at lines 135-155
// EXISTING: namespace wrap at lines 157-162
```

`session_context_to_extra_body` emits only `agenticQuery` and (optionally) `querySource`. There is no `accountKind` / `inOverage` key — those are session-stable on `AnthropicConfig` (Round-3 Finding 3). There is no `userType` / `entrypoint` key — coco-rs deliberately does not surface those (§7.1).

### 9.5 QueryParams extension

Add to `coco-rs/services/inference/src/client.rs:32-80`:

```rust
pub struct QueryParams {
    // ... existing 9 fields ...

    /// User intent for prompt caching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<PromptCacheConfig>,

    /// Agentic loop flag (gates `claude-code-20250219`).
    #[serde(default)]
    pub agentic: bool,
}
```

`account_kind` and `in_overage` do NOT flow through `ApiClient` or `QueryParams` (Round-3 Finding 3) — they are read directly by `build_anthropic` from `RuntimeConfig.account.*` and stored on `AnthropicConfig`, where the adapter's `cache_policy::resolve_ttl` reads them. `query_source` already exists on `QueryParams`.

### 9.6 ApiClient::query integration

In `client.rs:514-572` (`build_options`), construct `PerCallOverrides`:

```rust
fn build_options(&self, params: &QueryParams) -> LanguageModelV4CallOptions {
    let Some(info) = self.model_info.as_ref() else {
        return /* legacy mock path */;
    };

    let per_call = PerCallOverrides {
        cache_strategy: params.cache.clone(),
        agentic_query:  params.agentic,                     // per-call
        query_source:   params.query_source.clone(),        // per-call
        thinking_level: params.thinking_level.clone(),
        max_output_tokens: /* ... */,
        context_management: params.context_management.clone(),
        ..Default::default()
    };

    build_call_options(
        info, self.fingerprint.api, &self.fingerprint.provider,
        &per_call, params.prompt.clone(), params.tools.clone(),
    )
}
```

`ApiClient` does NOT gain `account_kind` / `in_overage` fields (Round-3 Finding 3). Those are session-stable: read by `build_anthropic` directly from `RuntimeConfig.account.*` at provider construction and stored on `AnthropicConfig`. No `OnceLock` on `ApiClient`; no `resolve_ttl` call here. The data is forwarded; the adapter resolves.

### 9.7 Detector hash refactor + query flow (Finding 3 + Finding 5 fix)

#### 9.7.1 What's wrong today

`build_prompt_state_input` at `client.rs:622-697` only hashes `params.context_management`:

```rust
let extra_body_hash = params
    .context_management
    .as_ref()
    .map(canonical_extra_body_hash)
    .unwrap_or(0);
```

`cache_control_hash: 0` and `betas: Vec::new()` are also hardcoded. Any new per-call key in `provider_options["anthropic"]` (`cacheStrategy`, `agenticQuery`, `querySource`, `requestedBetas`) is invisible to the detector. (Session-stable `AnthropicConfig` fields like `account_kind`/`in_overage` are tracked through `ProviderClientFingerprint` instead — see §12.2.)

The earlier doc claim that `canonical_extra_body_hash` "auto-handles new keys" was technically true at the *function* level but wrong at the *input* level — the function only sees `context_management`, not the merged flat extra map.

#### 9.7.2 The query() flow today and why a single refactor isn't enough

Verified at `client.rs:290-388`:

```text
query(params)                            ← entry
├── Phase 1: detector record
│   └── if detector exists:
│       layout = build_prompt_layout_from_prompt(params.prompt, ...)   // reads params, NOT call options
│       input  = build_prompt_state_input(self, params, query_source, layout_hashes)
│       detector.record_prompt_state(input)
└── loop {
        do_query(params)                 ← retry loop
        └── options = self.build_options(params)   // build_call_options invoked HERE
            model.do_generate(options)
    }
```

The detector hash is built **before** the retry loop. `build_options` runs **inside** the retry loop, on every attempt. A naïve "make `build_call_options` return `(call, merged_extra)`" doesn't fix anything because Phase 1 doesn't have a call to thread the tuple through.

#### 9.7.3 Refactor: build once, share between detector and retries

Move the call-options construction out of `do_query` and into `query`, computed exactly once before Phase 1. Pass the merged flat map to the detector; pass the call options into the retry loop.

```rust
pub async fn query(&self, params: &QueryParams) -> Result<QueryResult, InferenceError> {
    let start = std::time::Instant::now();
    let mut attempt = 0;

    // NEW: build call options once. Same options reused across retries
    // and used as the input fed to detector hashing — no drift possible.
    let (call_options, merged_extra) = self.build_options_with_extra(params);

    // Phase 1: snapshot prompt state (now reads merged_extra, not just context_management)
    if let Some(detector) = &self.cache_break_detector
        && let Some(query_source) = params.query_source.as_deref()
    {
        let layout = /* ... unchanged ... */;
        let layout_hashes = layout.as_ref().and_then(|l| l.prompt_hash_inputs.as_ref());
        let input = build_prompt_state_input(
            self, params, query_source, layout_hashes,
            &merged_extra,                                 // NEW
        );
        detector.lock().await.record_prompt_state(input);
    }

    loop {
        match self.do_query_with_options(&call_options).await {
            Ok(mut result) => { /* ... existing post-call detection ... */ }
            Err(e)         => { /* ... existing retry logic ... */ }
        }
    }
}

// New helper: build_options_with_extra returns the tuple.
// Old build_options remains as a thin shim returning .0 for callers
// that don't need the hash input (mock/test paths).
fn build_options_with_extra(&self, p: &QueryParams)
    -> (LanguageModelV4CallOptions, BTreeMap<String, Value>);

// New retry-side method takes pre-built options instead of params:
async fn do_query_with_options(&self, options: &LanguageModelV4CallOptions)
    -> Result<QueryResult, InferenceError>;
```

`build_call_options` itself returns `(LanguageModelV4CallOptions, BTreeMap<String, Value>)`; the second element is the merged flat extra map captured **after** Lane B merge but **before** namespace wrap. `build_options_with_extra` is the ApiClient adapter that calls `build_call_options` once.

`build_prompt_state_input` accepts `&BTreeMap<String, Value>` and hashes via `canonical_extra_body_hash(serde_json::to_value(&map))` — the existing canonical-hash function handles arbitrary key sets.

#### 9.7.4 What this fixes vs what it doesn't

Fixes:
- Detector sees every cache-relevant key without per-feature plumbing.
- Detector hash and the actual retry body cannot diverge — both come from the same `merged_extra` and same `call_options`.
- Retry never recomputes call options (small perf win, more importantly removes a class of "params changed mid-retry" bugs).

Does *not* fix (out of scope):
- Mid-retry param mutation by callers that hold `&mut QueryParams` — but `query` takes `&QueryParams` so that path was already closed.
- Mock/test code paths that construct `LanguageModelV4CallOptions` directly: those bypass `build_options` and pass `&BTreeMap::new()` to `build_prompt_state_input`, preserving current behavior. They forfeit accurate cache-break detection, which is the existing trade-off — explicitly documented at the top of `query` (Layer-2 caveat).

This refactor is independent of the prompt-cache feature itself; it can land first and immediately strengthens the `context_management` hash too.

## 10. Layer D — vercel-ai-anthropic (Policy + Memo + Wire)

This is where ALL Anthropic-specific behavior lives: TTL policy, beta resolution + memoization, marker placement, wire-string mapping. Per `coco-rs/services/inference/CLAUDE.md:3, :13`.

### 10.0 AnthropicConfig extension (Findings 1 / 5 / 8 / 10 / 11 / 13 fix)

`AnthropicConfig` (`coco-rs/vercel-ai/anthropic/src/anthropic_config.rs`) lives in `vercel-ai-anthropic`, which **has zero `coco-*` deps** (verified §19.2 F8). Every field on this struct must be a primitive or an adapter-locally-defined type — coco-types `Capability` cannot appear here.

Extend with **adapter-locally-defined** fields populated at provider construction:

```rust
// in coco-rs/vercel-ai/anthropic/src/anthropic_config.rs

pub struct AnthropicConfig {
    // ... existing 7 fields, unchanged ...

    /// Resolved per-model capability bools. Set by the provider factory
    /// (services/inference/src/model_factory.rs::build_anthropic) from
    /// `ResolvedModel.info.capabilities`. All-false = "unknown model"
    /// safe default — no capability betas emitted, no auto marker.
    pub capabilities: AnthropicModelCapabilities,

    /// Endpoint topology — distinct from auth (`AdapterAccountKind`).
    /// Drives `shouldIncludeFirstPartyOnlyBetas` (FirstParty only) and
    /// `shouldUseGlobalCacheScope` (FirstParty only). When Bedrock auth
    /// lands, the Bedrock-specific beta split applies on top.
    pub provider_topology: ProviderTopology,

    /// Mirrors TS env `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`. coco-rs
    /// reads from `~/.coco/config.json` `experimental_betas: bool`
    /// (default true). Default value here is `true` so first-party betas
    /// are emitted unless explicitly disabled.
    pub experimental_betas_enabled: bool,

    /// **TS-mirror runtime knobs that govern beta gating.** Mirrors what
    /// TS reads from `process.env.DISABLE_INTERLEAVED_THINKING`,
    /// `getInitialSettings().showThinkingSummaries`, and
    /// `getIsNonInteractiveSession()` respectively (`betas.ts:258-274`).
    /// Set by the provider factory from RuntimeConfig — this is the
    /// **source of truth** for both the memoized `beta_resolver::resolve`
    /// (consumes `disable_interleaved_thinking`) and the per-call merge
    /// in §10.4 (consumes `show_thinking_summaries` + `non_interactive`
    /// for the RedactThinking gate). Finding F5.
    pub disable_interleaved_thinking: bool,
    pub show_thinking_summaries:      bool,
    pub non_interactive:              bool,

    /// 1h-TTL allowlist patterns. Each entry is either an exact match
    /// for `query_source`, or a `prefix*` glob (single trailing wildcard).
    /// Source: `~/.coco/config.json` `prompt_cache.allowlist: Vec<String>`
    /// (TS reads `tengu_prompt_cache_1h_config.allowlist` from GrowthBook;
    /// see Open Question §16.2 for the future remote-feature-gate hook).
    /// Finding F5.
    pub prompt_cache_allowlist: Vec<String>,

    /// **Session-stable** account/billing identity (Round-3 Finding 3).
    /// Sourced from `~/.coco/config.json` `account.kind` via
    /// `RuntimeConfig.account` (Open Question §16.1). MUST live on the
    /// session-stable config — NOT on per-call `AnthropicProviderOptions` —
    /// because `cache_policy::resolve_ttl` latches eligibility on first
    /// call and a missing first-call value would silently corrupt every
    /// later subscriber request for the lifetime of this language model.
    pub account_kind: AdapterAccountKind,

    /// **Session-stable** subscriber overage flag (Round-3 Finding 3).
    /// Same reasoning as `account_kind` — TS treats this as
    /// `getCurrentLimits().isUsingOverage` which is session state in
    /// practice (`claude.ts:407-413`). When the user's overage status
    /// flips mid-session, the session reload path (`SettingsWatcher`)
    /// rebuilds `RuntimeConfig` and the next provider construction
    /// picks up the new value; the in-flight `OnceLock` keeps the
    /// pre-flip latch (TS parity — TS only reads it on first call).
    pub in_overage: bool,
}

/// Adapter-side capability flags. Set once at construction; read by
/// `beta_resolver` and `cache_placement`. Bool-per-feature beats a Vec
/// of strings (no string matching) and beats a parallel enum (no
/// duplicated taxonomy when only 5 boolean toggles are needed). Same
/// shape as the existing `supports_native_structured_output: Option<bool>`
/// pattern on `AnthropicConfig`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnthropicModelCapabilities {
    pub prompt_cache:           bool,
    pub context_1m:             bool,
    pub interleaved_thinking:   bool,
    pub context_management:     bool,
    pub token_efficient_tools:  bool,
}

/// Endpoint family — currently single-variant by design (Bedrock /
/// Foundry / Vertex / proxy support all deferred — see Non-Goal §2).
/// The enum is kept (not collapsed to a `bool is_first_party`) so a
/// future Bedrock PR adds a variant without touching every gate site
/// — `matches!(topology, ProviderTopology::FirstParty)` predicates
/// already in place stay correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTopology {
    /// `api.anthropic.com` (firstParty). Gets all firstParty-only betas
    /// (`redact-thinking-2026-02-12`, `prompt-caching-scope-2026-01-05`,
    /// `context-management-2025-06-27`) and global cache scope.
    FirstParty,
}
```

**Why a bool struct, not `Vec<&str>` or a parallel enum.** Five orthogonal boolean toggles. A `Vec<String>` would invite typo-bugs (`"context-1m"` vs `"context_1m"` vs `"context1m"`) and forces every consumer to do `.iter().any(|s| s == ...)`. An adapter-side `enum AnthropicCapability` parallel to `coco_types::Capability` would mean two enums to keep in sync for every future capability — and give us nothing the bool struct doesn't.

**Why explicit `ProviderTopology`, not derived from `base_url`.** `base_url` is a free-form string. Substring matching for `"anthropic.com"` would mis-classify proxies (FedStart deployments, custom OAuth, staging hosts). The provider factory already knows the topology from `ProviderConfig`; passing it explicitly is zero string parsing.

**Boundary location: services/inference/src/model_factory.rs (Finding 9).** This is the only site that holds both `coco_types::Capability` and `AnthropicModelCapabilities`. Translation is a small `From<&[Capability]>` impl in `services/inference` (allowed: `L2 → L1`):

```rust
// in coco-rs/services/inference/src/model_factory.rs (or a sibling helper)
fn anthropic_caps_from(coco_caps: Option<&Vec<coco_types::Capability>>)
    -> vercel_ai_anthropic::AnthropicModelCapabilities
{
    use coco_types::Capability::*;
    let mut out = vercel_ai_anthropic::AnthropicModelCapabilities::default();
    if let Some(caps) = coco_caps {
        for c in caps {
            match c {
                PromptCache          => out.prompt_cache = true,
                Context1m            => out.context_1m = true,
                InterleavedThinking  => out.interleaved_thinking = true,
                ContextManagement    => out.context_management = true,
                TokenEfficientTools  => out.token_efficient_tools = true,
                _ => {} // other capabilities (FastMode, Vision, …) not consumed by adapter
            }
        }
    }
    out
}
```

**Three-hop threading (Finding 10 + 13 + 14):** new fields land on `AnthropicProviderSettings` first (the user-facing knob), get stored on `AnthropicProvider`, then re-emitted by `make_config()`. Each hop is mechanical — one line of struct init per field — but all three are required.

```rust
// in coco-rs/vercel-ai/anthropic/src/anthropic_provider.rs

pub struct AnthropicProviderSettings {
    // ... existing 9 fields, unchanged ...
    pub capabilities:                  AnthropicModelCapabilities,    // NEW
    pub provider_topology:             ProviderTopology,              // NEW
    pub experimental_betas_enabled:    bool,                          // NEW
    pub disable_interleaved_thinking:  bool,                          // NEW (Finding F5)
    pub show_thinking_summaries:       bool,                          // NEW (Finding F5)
    pub non_interactive:               bool,                          // NEW (Finding F5)
    pub prompt_cache_allowlist:        Vec<String>,                   // NEW (Finding F5)
    pub account_kind:                  AdapterAccountKind,            // NEW (R3-F3 session-stable)
    pub in_overage:                    bool,                          // NEW (R3-F3 session-stable)
}

impl Default for AnthropicProviderSettings {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            capabilities:                  AnthropicModelCapabilities::default(),  // all-false
            provider_topology:             ProviderTopology::FirstParty,
            experimental_betas_enabled:    true,
            disable_interleaved_thinking:  false,
            show_thinking_summaries:       false,
            non_interactive:               false,
            prompt_cache_allowlist:        Vec::new(),
            account_kind:                  AdapterAccountKind::ApiKey,
            in_overage:                    false,
        }
    }
}

pub struct AnthropicProvider {
    // ... existing 7 fields, unchanged ...
    capabilities:                  AnthropicModelCapabilities,    // NEW (stored)
    provider_topology:             ProviderTopology,              // NEW (stored)
    experimental_betas_enabled:    bool,                          // NEW (stored)
    disable_interleaved_thinking:  bool,                          // NEW (stored)
    show_thinking_summaries:       bool,                          // NEW (stored)
    non_interactive:               bool,                          // NEW (stored)
    prompt_cache_allowlist:        Vec<String>,                   // NEW (stored)
    account_kind:                  AdapterAccountKind,            // NEW (R3-F3 stored)
    in_overage:                    bool,                          // NEW (R3-F3 stored)
}

impl AnthropicProvider {
    fn make_config(&self) -> Arc<AnthropicConfig> {
        Arc::new(AnthropicConfig {
            // ... existing 7 fields ...
            capabilities:                  self.capabilities,
            provider_topology:             self.provider_topology,
            experimental_betas_enabled:    self.experimental_betas_enabled,
            disable_interleaved_thinking:  self.disable_interleaved_thinking,
            show_thinking_summaries:       self.show_thinking_summaries,
            non_interactive:               self.non_interactive,
            prompt_cache_allowlist:        self.prompt_cache_allowlist.clone(),
            account_kind:                  self.account_kind,                 // R3-F3 session-stable
            in_overage:                    self.in_overage,                   // R3-F3 session-stable
        })
    }
}
```

**Provider factory wiring — actual file path + signature change:**

`build_anthropic` today takes 3 params: `(provider_cfg, api_model, timeout_secs)` (`model_factory.rs:196-200`). To populate the new `AnthropicProviderSettings` fields, it needs **two** more inputs: the resolved `ModelInfo` (Finding 14) AND a runtime-knobs source. The cleanest shape passes the whole `runtime: &RuntimeConfig` since the caller already has it (`build_language_model_from_runtime:97-124` line 110 already resolves `model_info` from `runtime.model_registry`); routing two scalars through a third struct buys nothing.

```rust
// in coco-rs/services/inference/src/model_factory.rs::build_anthropic
// (line 196-217 today; new signature adds runtime + model_info — Round-3 Finding 1)
fn build_anthropic(
    runtime:       &RuntimeConfig,                // NEW: source of `prompt_cache.*` and adapter knobs
    provider_cfg:  &ProviderConfig,
    api_model:     &str,
    timeout_secs:  i64,
    model_info:    Option<&ModelInfo>,            // NEW: already resolved at call site (line 110-113)
) -> anyhow::Result<Arc<dyn LanguageModel>> {
    let opts = &provider_cfg.client_options;
    let pc   = &runtime.prompt_cache;             // see §16a.2 for the new RuntimeConfig section
    let ak   = &runtime.anthropic_knobs;          // see §16a.3 for the new RuntimeConfig section
    let acc  = &runtime.account;                  // see Open Question §16.1

    let settings = vercel_ai_anthropic::AnthropicProviderSettings {
        // ... existing 9 fields, unchanged ...
        capabilities: anthropic_caps_from(
            model_info.and_then(|mi| mi.capabilities.as_ref()),
        ),
        provider_topology:             vercel_ai_anthropic::ProviderTopology::FirstParty,
        experimental_betas_enabled:    ak.experimental_betas,         // bool, default true
        disable_interleaved_thinking:  ak.disable_interleaved_thinking,
        show_thinking_summaries:       ak.show_thinking_summaries,
        non_interactive:               ak.non_interactive,
        prompt_cache_allowlist:        pc.allowlist.clone(),
        account_kind:                  account_kind_to_adapter(acc.account_kind),  // R3-F3 session-stable
        in_overage:                    acc.in_overage,                              // R3-F3 session-stable
    };
    let provider = vercel_ai_anthropic::create_anthropic(settings);
    provider.language_model(api_model)
        .map_err(|e| anyhow::anyhow!("anthropic provider `{}`: {e}", provider_cfg.name))
}
```

The two callers in `model_factory.rs` (`build_language_model_from_runtime:117` and `build_api_client:150` via the former) need the corresponding two-line update to forward `runtime` + `model_info.as_ref()`. Other providers (`build_openai`, `build_google`, `build_openai_compat`) are untouched — only Anthropic consumes the new sections.

There is no `derive_topology` helper in this iteration — Bedrock support is deferred (per user instruction; see Non-Goal §2). The provider factory just constructs `ProviderTopology::FirstParty` directly. When Bedrock lands, this becomes a small match on a new typed `ProviderConfig` field.

The caller `build_language_model_from_runtime` already resolves `model_info` at line 110-113; it just needs to pass it down to `build_anthropic`.

**Empty caps = "unknown model" safe default.** When `ModelInfo.capabilities` is `None`, `anthropic_caps_from` returns `AnthropicModelCapabilities::default()` (all-false). The adapter then emits no capability betas and applies no auto cache marker. Users who want caching on a non-builtin Claude model declare capabilities explicitly in `~/.coco/models.json` per §8.3 — same path that exists today for `extended_thinking`, `fast_mode`, etc.

**Memoization caveat (Finding 11).** `make_config()` produces a fresh `Arc<AnthropicConfig>` each `language_model()` call, so `OnceLock<ResolvedBetas>` on `AnthropicMessagesLanguageModel` is per-instance. For the primary `ApiClient` path (one instance per `ProviderClientFingerprint`), this matches TS's `memoize(model)` semantics in practice. Callers who construct multiple instances of the same model intentionally (rare) get fresh memoization each time — documented divergence, not a bug. A test in §14.2 (`betas_memoized_within_single_language_model_instance`) asserts memoization within one instance lifetime.

**Bedrock support is entirely deferred** (per user instruction). No `AccountKind::Bedrock`, no `ProviderTopology::Bedrock` variant, no `bedrock_1h_env` field, no Bedrock branch in `cache_policy::resolve_ttl`, no Bedrock split in `beta_resolver`. When Bedrock auth lands, that PR adds all five together — `AccountKind::Bedrock + ProviderTopology::FirstParty` is unrepresentable today, so the half-implementation risk Finding F3 flagged is closed by construction.

### 10.1 AnthropicProviderOptions extension (Finding 8 fix — adapter-side types only)

Extend `coco-rs/vercel-ai/anthropic/src/messages/anthropic_messages_options.rs:122-149`. **All new field types are adapter-locally-defined** (mirrors §19.2 F8 fix; precedent: existing `thinking: Option<ThinkingConfig>` where `ThinkingConfig` is local to this file). No `coco_types::*` import.

```rust
// in coco-rs/vercel-ai/anthropic/src/messages/anthropic_messages_options.rs

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicProviderOptions {
    // ... existing 13 fields, unchanged ...

    /// Auto-place markers, choose TTL/scope.
    pub cache_strategy: Option<CacheStrategy>,

    /// User-requested beta top-up (TS `getSdkBetas` equivalent).
    /// Wire format is a list of camelCase beta tags — adapter-side enum
    /// `AdapterBetaCapability` parses them.
    pub requested_betas: Option<Vec<AdapterBetaCapability>>,

    /// Per-call agentic flag — gates `claude-code-20250219` baseline.
    pub agentic_query: Option<bool>,

    /// Query source — matched against the 1h-TTL allowlist per call.
    pub query_source: Option<String>,

    // **Round-3 Finding 3:** `account_kind` and `in_overage` are NOT
    // per-call fields. They live on `AnthropicConfig` (session-stable)
    // because `cache_policy::resolve_ttl` latches eligibility on the
    // first call — a missing first-call value would default-corrupt the
    // latch for the whole session. See §10.0.
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStrategy {
    pub mode: AdapterCacheMode,
    /// Caller-requested TTL. Adapter may downgrade based on eligibility.
    pub ttl: AdapterCacheTtl,
    #[serde(default)]
    pub scope: Option<AdapterCacheScope>,
    #[serde(default)]
    pub skip_cache_write: bool,
}

/// Adapter-side mirror of `coco_types::PromptCacheMode`. Same wire shape;
/// no shared type (`vercel-ai-anthropic` cannot import `coco-types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheMode { Disabled, Auto, Manual }

/// Adapter-side mirror of `coco_types::CacheTtl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheTtl { FiveMinutes, OneHour }

/// Adapter-side mirror of `coco_types::CacheScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCacheScope { Org, Global }

/// Adapter-side mirror of `coco_types::AccountKind`. No `Bedrock` variant
/// in this iteration (Finding F3) — TTL Bedrock branch and `bedrock_1h_env`
/// field are deferred to the same PR that adds Bedrock auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterAccountKind {
    #[default]
    ApiKey,
    ClaudeAiSubscriber,
}

/// Adapter-side mirror of `coco_types::BetaCapability`. The serde
/// `rename_all = "snake_case"` lines up with the JSON boundary that
/// `services/inference::cache_convert` writes (e.g.,
/// `"requestedBetas": ["context_1m"]`); this is the **internal coco-rs
/// boundary** between inference and the adapter, NOT the Anthropic wire.
/// The adapter's `beta_capabilities::map_capability` then translates
/// each enum variant into the actual Anthropic header string (kebab-case
/// + date suffix, e.g. `"context-1m-2025-08-07"`). Two distinct hops:
/// JSON-snake → Rust enum → Anthropic-kebab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterBetaCapability {
    Context1m,
    InterleavedThinking,
    ContextManagement,
    StructuredOutputs,
    TokenEfficientTools,
    FastMode,
    PromptCachingScope,
    RedactThinking,
    Advisor,
}
```

**Wire-format equivalence (round-trip parity).** `services/inference::cache_convert::to_extra_body` serializes `coco_types::PromptCacheConfig` to a JSON value (mode/ttl/scope are snake_case enum strings; the camelCase wire field name is `skipCacheWrite`, set by serde rename on the adapter `CacheStrategy` struct, mirroring TS `getCacheControl` output). `extract_anthropic_options` deserializes that same JSON into `AnthropicProviderOptions { cache_strategy: Option<CacheStrategy> }` where `CacheStrategy` is the adapter-side type defined above. Both ends agree on the wire shape for every variant — but they share **zero Rust types**. A round-trip test in §14 locks the wire format on both sides:

```rust
// in services/inference/src/cache_convert.test.rs
#[test]
fn cache_strategy_wire_format_matches_adapter_struct() {
    let coco_side = coco_types::PromptCacheConfig {
        mode: PromptCacheMode::Auto,
        ttl:  CacheTtl::OneHour,
        scope: Some(CacheScope::Org),
        skip_cache_write: false,
        requested_betas: BTreeSet::new(),
    };
    let json = cache_convert::to_extra_body(&coco_side, ProviderApi::Anthropic)
        .get("cacheStrategy").cloned().unwrap();

    // The same JSON value parses cleanly into the adapter-side type:
    let adapter_side: vercel_ai_anthropic::CacheStrategy =
        serde_json::from_value(json).expect("wire format mismatch");
    assert_eq!(adapter_side.mode, vercel_ai_anthropic::AdapterCacheMode::Auto);
    assert_eq!(adapter_side.ttl,  vercel_ai_anthropic::AdapterCacheTtl::OneHour);
}
```

This test belongs in `services/inference` (the only crate that imports both sides). Without it, drift between the coco-types enum's serde rename and the adapter enum's serde rename would silently break the boundary.

The 6 new fields go through the existing `or()` merge chain at `anthropic_messages_options.rs:232-255` — one line each, mechanical. **No** `user_type` or `entrypoint`. Anthropic-internal betas (`cli-internal-2026-02-09`, `summarize-connector-text-2025-08-22`) are never surfaced through this struct (§3.5 / §7.1).

### 10.1.5 Strip internal keys before raw shallow-merge (Finding 2 fix)

`extract_anthropic_options` (`anthropic_messages_options.rs:200-228`) currently keeps **every** key from the inbound JSON in `raw`, by design — that lets users pass forward-compat fields like a not-yet-modeled `extended_thinking_v3` straight through to the Anthropic body. At line 772 the raw map is shallow-merged into the request body via `vercel_ai_provider_utils::shallow_merge_object(&mut body, raw_provider_options)`.

If we naïvely add `cacheStrategy`, `requestedBetas`, `agenticQuery`, `querySource` to `AnthropicProviderOptions`, those keys also land in `raw` and ship to `api.anthropic.com/v1/messages` — where they are unknown, will be either rejected (400) or silently ignored, and at minimum will appear in any wire-level observability (request dumps, proxy logs). (`accountKind` and `inOverage` are NOT in this list — Round-3 Finding 3 moved them to session-stable `AnthropicConfig` so they never enter per-call provider_options at all.)

**Fix:** maintain an explicit deny-list of internal-only keys; strip them from the raw map after typed extraction, before the function returns:

```rust
// in anthropic_messages_options.rs, near the top
const INTERNAL_ANTHROPIC_OPTION_KEYS: &[&str] = &[
    "cacheStrategy",
    "requestedBetas",
    "agenticQuery",
    "querySource",
];

// inside extract_anthropic_options, after raw is built (~line 226):
for key in INTERNAL_ANTHROPIC_OPTION_KEYS {
    raw.remove(*key);
}
```

**Why a deny-list, not the inverse (allow-list of body-bound keys).** The TS-shaped fields that *should* pass through (`thinking`, `cache_control`, `mcp_servers`, `container`, `tool_streaming`, `effort`, `speed`, `anthropic_beta`, `context_management`, `inference_geo`, `disable_parallel_tool_use`, `send_reasoning`, `structured_output_mode`) are themselves still being added by ongoing TS-parity work; an allow-list would force every new typed field to also re-list itself in the allow-list. A deny-list of internal coco-rs-only signals is small (4 entries today, grows only when this design grows), additive, and decoupled from the existing typed-field surface.

**Test (in §14.2):** new test `wire_body_does_not_contain_internal_anthropic_keys` — set `cache_strategy` + `agentic_query` + `query_source` + `requested_betas` on `AnthropicProviderOptions`, call `get_args`, parse the resulting body JSON, assert none of the 4 keys appear at any depth.

**Compounding value:** the deny-list also prevents *typed user input* (e.g., a settings.json that mistakenly carries `cacheStrategy: ...`) from reaching the wire — a defense-in-depth nicety beyond the immediate adapter-level concern.

### 10.1a Cache TTL policy

New file `coco-rs/vercel-ai/anthropic/src/cache_policy.rs`. Mirrors TS `should1hCacheTTL` (`claude.ts:393-433`). State (eligibility + allowlist latches) lives inside the struct; `AnthropicMessagesLanguageModel` holds one instance per language-model.

```rust
use std::sync::OnceLock;
use crate::messages::anthropic_messages_options::{AdapterAccountKind, AdapterCacheTtl};

#[derive(Default)]
pub(crate) struct CachePolicy {
    eligible_1h: OnceLock<bool>,
    allowlist:   OnceLock<Vec<String>>,
}

impl CachePolicy {
    /// Per-call resolution. Latches eligibility + allowlist on first call
    /// **for which the inputs are observable**; the per-querySource match
    /// recomputes every call (TS-mirror).
    ///
    /// **No Bedrock branch in this iteration (Finding F3).** TS gates Bedrock
    /// 1h TTL on `getAPIProvider() == 'bedrock'` + `ENABLE_PROMPT_CACHING_1H_BEDROCK`
    /// env (`claude.ts:399-403`). coco-rs has no Bedrock endpoint wiring, no
    /// `AccountKind::Bedrock` (dropped per §7), no `bedrock_1h_env` field —
    /// the Bedrock branch lands together with Bedrock auth in a follow-up PR.
    ///
    /// **First-call inputs MUST be present (Round-3 Finding 3).** The
    /// signature takes `account: AdapterAccountKind` (NOT `Option`) and
    /// `in_overage: bool` (NOT `Option`). It is the caller's
    /// responsibility — and §10.0 prose enforces — that these are
    /// session-stable on `AnthropicConfig`, so they are always present
    /// at the first call. **Reasoning:** if either field could be
    /// missing on the first call, `unwrap_or_default()` (used by an
    /// earlier draft) would silently default to `AccountKind::ApiKey` /
    /// `in_overage = false`, latch eligibility = `false`, and
    /// **permanently** prevent any later subscriber call from seeing 1h
    /// TTL — even after the actual fields arrive. By forcing `account`
    /// + `in_overage` onto `AnthropicConfig` (not on per-call
    /// `AnthropicProviderOptions`), this corruption is unrepresentable.
    pub fn resolve_ttl(
        &self,
        requested:    AdapterCacheTtl,
        account:      AdapterAccountKind,                // session-stable, sourced from AnthropicConfig
        in_overage:   bool,                              // session-stable, sourced from AnthropicConfig
        query_source: Option<&str>,                      // per-call
        allowlist:    &[String],                        // Finding F5: simple slice, no closure
    ) -> AdapterCacheTtl {
        if requested == AdapterCacheTtl::FiveMinutes {
            return AdapterCacheTtl::FiveMinutes;
        }
        // Eligibility latch (claude.ts:407-413, with the Ant branch dropped).
        // TS: USER_TYPE === 'ant' || (isClaudeAISubscriber && !inOverage)
        // coco-rs: only the subscriber branch — see §7.1 (UserType::Ant not consumed).
        let eligible = *self.eligible_1h.get_or_init(|| {
            matches!(account, AdapterAccountKind::ClaudeAiSubscriber) && !in_overage
        });
        if !eligible { return AdapterCacheTtl::FiveMinutes; }
        // Allowlist latch (claude.ts:417-423) — clone the slice into a Vec
        // to satisfy OnceLock<Vec>; identical hash on subsequent calls.
        let latched_allowlist = self.allowlist.get_or_init(|| allowlist.to_vec());
        // Per-call match (NOT latched)
        match query_source {
            Some(qs) if matches_pattern(qs, latched_allowlist) => AdapterCacheTtl::OneHour,
            _ => AdapterCacheTtl::FiveMinutes,
        }
    }
}

fn matches_pattern(qs: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        if let Some(prefix) = p.strip_suffix('*') {
            qs.starts_with(prefix)
        } else {
            qs == p
        }
    })
}
```

### 10.1b Beta resolver

New file `coco-rs/vercel-ai/anthropic/src/beta_resolver.rs`. Capability-driven betas; memoized via `OnceLock` on `AnthropicMessagesLanguageModel`.

```rust
// in coco-rs/vercel-ai/anthropic/src/beta_resolver.rs
// Adapter-internal — no coco-* imports.
use std::collections::BTreeSet;
use crate::anthropic_config::AnthropicModelCapabilities;
use crate::messages::anthropic_messages_options::AdapterBetaCapability;

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedBetas {
    /// Capability-derived (memoized, model-stable).
    pub capability_betas: BTreeSet<AdapterBetaCapability>,
}

pub(crate) fn resolve(
    caps: AnthropicModelCapabilities,        // bool struct, copy-cheap
    disable_interleaved_thinking: bool,      // mirrors TS env DISABLE_INTERLEAVED_THINKING
) -> ResolvedBetas {
    let mut capability_betas = BTreeSet::new();

    // Capabilities with NO topology gate (TS betas.ts:254-262):
    if caps.context_1m              { capability_betas.insert(AdapterBetaCapability::Context1m); }
    if caps.token_efficient_tools   { capability_betas.insert(AdapterBetaCapability::TokenEfficientTools); }
    if caps.interleaved_thinking && !disable_interleaved_thinking {
        capability_betas.insert(AdapterBetaCapability::InterleavedThinking);
    }

    // NB: `context_management` is gated on first-party-topology + experimental
    // (TS betas.ts:307-311 `shouldIncludeFirstPartyOnlyBetas() && modelSupportsContextManagement(...)`).
    // Topology lives on AnthropicConfig, not here, so emission moves to the
    // per-call merge in §10.4 — see Finding F2.
    // Do NOT add it to capability_betas; that map is reserved for betas with
    // no topology gate (memoization-safe).

    ResolvedBetas { capability_betas }
}
```

`show_thinking_summaries` and `non_interactive` are NOT consumed here — they gate `RedactThinking`, which is per-call (depends on whether *this* request is interactive) and therefore lives in the §10.4 merge. The resolver is reserved for memoization-safe inputs (model + session-stable env), so the parameter list stays narrow on purpose. Finding F5.

**Shared cross-site predicates (Round-3 Finding 2).** Several betas have **two emission sites** in the adapter (e.g., `context-management-2025-06-27` is added by both the body path at `anthropic_messages_language_model.rs:710` and the memory-tool path at `prepare_tools.rs:236`). To prevent drift, the resolver exposes named predicates that both call sites import:

```rust
// in beta_resolver.rs
use crate::anthropic_config::{AnthropicConfig, ProviderTopology};

/// Body path AND memory-tool path both gate `context-management-2025-06-27`
/// emission on this single predicate. TS `betas.ts:307-311`:
/// `shouldIncludeFirstPartyOnlyBetas() && modelSupportsContextManagement(...)`.
pub(crate) fn should_emit_context_management(config: &AnthropicConfig) -> bool {
    matches!(config.provider_topology, ProviderTopology::FirstParty)
        && config.experimental_betas_enabled
        && config.capabilities.context_management
}
```

The test `both_paths_use_shared_resolver_predicate` (§14.2) asserts this function is the only place either site computes the gate (greps the source for inline expressions of the same predicate) — preventing future regression where someone refactors one site and forgets the other.

`redact-thinking-2026-02-12` is **first-party-only** (TS `betas.ts:268-277` gate is `getAPIProvider() ∈ {firstParty, foundry} + InterleavedThinking-capable`, NOT `USER_TYPE === 'ant'`). It is emitted from the per-call merge below (not memoized) because it depends on per-request runtime settings (`non_interactive`, `show_thinking_summaries`). This design folds it into the InterleavedThinking capability gate plus `provider_topology == FirstParty` — there is no separate `Capability::RedactThinking`.

### 10.2 Cache placement algorithm

New file `coco-rs/vercel-ai/anthropic/src/cache_placement.rs` — TS-mirror of `claude.ts:3089-3105` and `claude.ts:588-668`:

```rust
// in coco-rs/vercel-ai/anthropic/src/cache_placement.rs
// Adapter-internal — no coco-* imports.
use serde_json::{json, Value};
use crate::messages::anthropic_messages_options::{AdapterCacheTtl, AdapterCacheScope};

/// Per-call directive for auto-placement.
pub(crate) struct CacheMarkerStrategy {
    pub ttl:              AdapterCacheTtl,    // already resolved by cache_policy
    pub scope:            Option<AdapterCacheScope>,
    pub skip_cache_write: bool,
}

/// TS-mirror: claude.ts:3089 markerIndex algorithm — operates on the
/// **post-grouping** Anthropic-message array, NOT on the raw
/// LanguageModelV4Prompt. group_into_blocks merges adjacent same-role
/// messages, so prompt index ≠ wire index. Computing on raw prompt
/// would attach the marker to the wrong message.
///
/// Caller (convert_to_anthropic_messages_full) provides the assembled
/// `messages` after grouping, before serialization.
pub(crate) fn compute_marker_index_post_group(
    messages_post_group: &[serde_json::Value],
    strategy: &CacheMarkerStrategy,
) -> Option<usize> {
    if messages_post_group.is_empty() {
        return None;
    }
    let idx = if strategy.skip_cache_write {
        messages_post_group.len().checked_sub(2)?
    } else {
        messages_post_group.len() - 1
    };
    // TS claude.ts:653-655 — skip if the LAST content block of the target
    // message is reasoning/thinking. Anthropic rejects cache_control on
    // those blocks, so drop the marker rather than emitting an invalid one.
    if last_content_block_is_reasoning(&messages_post_group[idx]) {
        return None;
    }
    Some(idx)
}

fn last_content_block_is_reasoning(msg: &serde_json::Value) -> bool {
    msg.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.last())
        .and_then(|last| last.get("type"))
        .and_then(|t| t.as_str())
        .map(|t| t == "thinking" || t == "redacted_thinking")
        .unwrap_or(false)
}

/// TS-mirror: claude.ts:358-373 getCacheControl().
pub(crate) fn build_cache_control_value(
    ttl: AdapterCacheTtl,
    scope: Option<AdapterCacheScope>,
) -> Value {
    let mut v = json!({ "type": "ephemeral" });
    if matches!(ttl, AdapterCacheTtl::OneHour) {
        v["ttl"] = json!("1h");
    }
    if matches!(scope, Some(AdapterCacheScope::Global)) {
        v["scope"] = json!("global");
    }
    // AdapterCacheScope::Org is the implicit default — not written to wire (TS parity).
    v
}
```

**Integration point — concrete API change:**

`convert_to_anthropic_messages_full` (`convert_to_anthropic_messages.rs:170-175`) currently has 4 params (`prompt`, `send_reasoning`, `tool_name_mapping`, `cache_validator`). Add a 5th:

```rust
pub fn convert_to_anthropic_messages_full(
    prompt: &LanguageModelV4Prompt,
    send_reasoning: bool,
    tool_name_mapping: &ToolNameMapping,
    cache_validator: &mut CacheControlValidator,
    auto_marker: Option<CacheMarkerStrategy>,            // NEW
) -> ConvertedMessages { ... }
```

The function already does block grouping at line 181 (`group_into_blocks`) and emits `messages: Vec<Value>` (lines 289, 335). After the existing per-message loop completes — but **before** building `ConvertedMessages` — apply auto-placement post-hoc:

```rust
// After the existing for-loop builds `messages: Vec<Value>` (lines 185-342):
if let Some(strategy) = auto_marker.as_ref() {
    if let Some(target_idx) = compute_marker_index_post_group(&messages, strategy) {
        attach_cache_control_to_last_block(
            &mut messages[target_idx],
            build_cache_control_value(strategy.ttl, strategy.scope),
            cache_validator,                              // 4-cap check
        );
    }
}

ConvertedMessages { system, messages, warnings, betas }
```

`attach_cache_control_to_last_block` mutates the last content block's `cache_control` field, defensively skipping reasoning blocks one more time and going through `cache_validator.check_breakpoint()` so the 4-cap budget is consumed exactly once per added marker.

**Conflict policy — no overwrite, no merge (Finding F6).** When the target content block already carries a `cache_control` value (because the user pre-attached one through the existing low-level escape hatch `AnthropicProviderOptions.cache_control` — `anthropic_messages_options.rs:132` — or through a future `SystemPromptBlock::CacheBreakpoint` Manual-mode hint), the auto-marker is **dropped silently and the function early-returns**. Specifically:

1. `attach_cache_control_to_last_block` checks `block.get("cache_control").is_some()` *before* writing.
2. If true: no mutation, no `cache_validator.check_breakpoint()` call. The user's marker stays as-is.
3. If false: write the auto-marker JSON value, then call `cache_validator.check_breakpoint()` to consume one slot from the 4-cap budget.

This rule preserves three invariants that any other policy would violate:

- **User intent wins.** A caller who has explicitly attached `cache_control` to a block is signaling manual control; the high-level `cache_strategy::Auto` opting out at that exact placement is the least surprising outcome.
- **The 4-cap budget never inflates.** Merging two markers (e.g., taking `ttl` from one and `scope` from the other) would risk producing a single composite cache_control object that the validator counts as one slot but that semantically represents two intentions, blurring the cap accounting.
- **No silent overwrite.** Overwriting the user's marker would lose the user's chosen `ttl`/`scope`/etc. without any error. A user who wanted the auto-marker to win would set `cache_strategy.mode = Manual` and clear the per-block override; a user who wanted both wouldn't have provided the override in the first place.

The `auto_marker_does_not_double_mark_when_user_supplied_cache_control_present` test in §14.2 locks this behavior: pre-fill `messages[N-1]`'s last content block with `cache_control: { type: "ephemeral" }` on the user side, run the converter with `auto_marker = Some(strategy)`, assert the block's `cache_control` is unchanged AND that `cache_validator` reports one breakpoint consumed (the user's), not two.

**Why post-group, not in-loop:**

1. The in-loop body already passes `is_last_part`, `is_last_block`, `is_last_message` flags that reach `get_part_cache_control` (line 243) — but those flags are scoped to the block-level grouping. The TS algorithm targets `messages[N-1]` of the assembled wire array, which is `block N-1`'s last message's last non-reasoning part. Computing this in-loop would require threading the auto-marker decision through `get_part_cache_control`, `convert_user_part`, and `convert_assistant_part` — a bigger surface area that risks double-marking when user-supplied `cache_control` already exists on the same part.
2. Post-group, mutation operates on JSON values we own (already constructed). The per-part path is unchanged; user-supplied `cache_control` merges cleanly because `cache_validator` arbitrates both paths.

**Reasoning-block invariant.** If the target message's last content block is `thinking` or `redacted_thinking`, the marker is dropped (`compute_marker_index_post_group` returns `None`). TS does the same at `claude.ts:653-655`. The marker is not relocated to the second-to-last block — TS doesn't either; the message simply doesn't get a marker on this turn.

**4-cap safety:** `CacheControlValidator` already enforces `MAX_CACHE_BREAKPOINTS = 4` (cache_control.rs:7). The auto-placed marker goes through the same validator the per-block path uses, so the global budget is honored regardless of whether markers come from auto-placement, user `provider_options.cache_control`, or per-system-block scope.

**Where the strategy comes from.** `get_args` builds `CacheMarkerStrategy` from `anthropic_options.cache_strategy.mode == Auto` after `cache_policy.resolve_ttl(...)` returns the resolved TTL. Manual mode does not pass an `auto_marker`; placement comes from `SystemPromptBlock::CacheBreakpoint` hints (out of scope; see Open Question §16.3).

### 10.3 Beta capability translation

New file `coco-rs/vercel-ai/anthropic/src/beta_capabilities.rs`:

```rust
// in coco-rs/vercel-ai/anthropic/src/beta_capabilities.rs
// Adapter-internal — no coco-* imports.
use crate::messages::anthropic_messages_options::{AdapterAccountKind, AdapterBetaCapability};

pub(crate) fn map_capability(cap: AdapterBetaCapability) -> &'static str {
    match cap {
        AdapterBetaCapability::Context1m            => "context-1m-2025-08-07",
        AdapterBetaCapability::InterleavedThinking  => "interleaved-thinking-2025-05-14",
        AdapterBetaCapability::ContextManagement    => "context-management-2025-06-27",
        AdapterBetaCapability::StructuredOutputs    => "structured-outputs-2025-12-15",
        AdapterBetaCapability::TokenEfficientTools  => "token-efficient-tools-2026-03-28",
        AdapterBetaCapability::FastMode             => "fast-mode-2026-02-01",
        AdapterBetaCapability::PromptCachingScope   => "prompt-caching-scope-2026-01-05",
        AdapterBetaCapability::RedactThinking       => "redact-thinking-2026-02-12",
        AdapterBetaCapability::Advisor              => "advisor-tool-2026-03-01",
    }
}

const CLAUDE_CODE_20250219: &str = "claude-code-20250219";
const OAUTH_BETA:           &str = "oauth-2025-04-20";  // TS source: constants/oauth.ts:36

/// Account/role-derived baseline betas. **No model-string matching, no Ant gating.**
///
/// Translation from TS (`betas.ts:240-253`):
///
/// | TS gate                                  | coco-rs gate                                       |
/// |------------------------------------------|----------------------------------------------------|
/// | `!isHaiku \|\| isAgenticQuery`           | `agentic_query`                                    |
/// |   for `claude-code-20250219`             |   (helper calls pass agentic=false; main loop true)|
/// | `isClaudeAISubscriber()`                 | `matches!(account, ClaudeAiSubscriber)`            |
/// |   for OAuth beta                         |                                                    |
///
/// **Not ported** (Anthropic-internal, see §3.5):
/// - `cli-internal-2026-02-09` — TS gate `USER_TYPE === 'ant' && ENTRYPOINT === 'cli'`
/// - `summarize-connector-text-2025-08-22` — TS gate `USER_TYPE === 'ant'`
///
/// Why no `is_haiku` parameter: TS's `!isHaiku` is a heuristic for "this
/// model runs the main agent loop, not helper calls". coco-rs encodes that
/// directly via the per-call `agentic_query` flag set by the caller. Helper
/// calls (compaction, title generation, classification) explicitly pass
/// `agentic=false`; main agent calls pass `true`. This is provider-neutral
/// and works for any model, not just Anthropic models named "haiku".
pub(crate) fn baseline_betas(
    account: AdapterAccountKind,
    agentic: bool,
) -> Vec<&'static str> {
    let mut v = Vec::new();
    if agentic {
        v.push(CLAUDE_CODE_20250219);                          // betas.ts:240-242 + 397-405
    }
    if matches!(account, AdapterAccountKind::ClaudeAiSubscriber) {
        v.push(OAUTH_BETA);                                    // betas.ts:251-253
    }
    v
}
```

**Two orthogonal axes** drive baseline beta gating, neither of them model-string-derived:

| Axis | Type | Default | Source |
|---|---|---|---|
| Agentic vs helper | `agentic: bool` (per-call) | `false` | Caller (`coco-query` for main loop, `coco-compact`/title-gen for helpers) |
| Auth/billing mode | `AdapterAccountKind` (session) | `AdapterAccountKind::ApiKey` (no OAuth beta) | `coco-config::AccountKind` translated to `AdapterAccountKind` inside `services/inference::build_anthropic` and stored on `AnthropicConfig.account_kind` at provider construction (R3-F3). NOT carried per-call. |

`coco-rs`'s default user (no env, no special settings) yields `(agentic=true_or_false, account=ApiKey)` — gets `claude-code-20250219` only when running the agent loop, never gets the OAuth beta. This is the right behavior for an SDK that isn't an Anthropic-internal CLI.

**No `cli-internal` / `summarize-connector-text` here.** Both are Anthropic-internal (gated on `USER_TYPE === 'ant'` in TS); coco-rs deliberately does not consume `UserType::Ant` (§7.1). If a future Anthropic-internal fork of coco-rs needs them, they belong in a downstream patch, not in this crate.

### 10.4 get_args integration (adapter-side resolution + memoization)

`AnthropicMessagesLanguageModel` (the language-model struct holding instance state) gains memoized resolution + policy state:

```rust
pub struct AnthropicMessagesLanguageModel {
    // ... existing fields ...
    resolved_betas: OnceLock<beta_resolver::ResolvedBetas>, // model-stable, memoized
    cache_policy:   beta_resolver::CachePolicy,             // eligibility + allowlist latches
}
```

Inside `get_args` (`messages/anthropic_messages_language_model.rs:413-763`), after the existing user-raw `anthropic_beta` merge at line 745, run resolution + per-call merges:

```rust
// All identifiers below are adapter-side (no coco_* references).

// EXISTING: feature-derived betas (effort, fast-mode, MCP, container,
// compact, tool-streaming) — lines 590-737, MOSTLY unchanged. Exception:
// the unconditional context-management insert at line 710 is moved to
// the gated path below (Finding F2 / R3-F2 share one resolver predicate
// across the two emission sites).

// EXISTING: user-raw anthropic_beta merge — lines 743-745, unchanged

// NEW: memoized capability betas (mirrors TS memoize(getAllModelBetas))
let resolved = self.resolved_betas.get_or_init(|| {
    beta_resolver::resolve(
        self.config.capabilities,
        self.config.disable_interleaved_thinking,
    )
});
for cap in &resolved.capability_betas {
    betas.insert(beta_capabilities::map_capability(*cap).to_string());
}

// NEW: per-call baseline (claude-code-*, OAuth) — no model strings, no Ant gating.
// `account_kind` is session-stable on `self.config` (Round-3 Finding 3); only
// `agentic_query` is per-call.
{
    let agentic = anthropic_options.agentic_query.unwrap_or(false);
    for b in beta_capabilities::baseline_betas(self.config.account_kind, agentic) {
        betas.insert(b.to_string());
    }
}

// NEW: per-call topology-gated betas. The capability_betas set above
// covers only no-topology-gate cases; ContextManagement and RedactThinking
// require first-party + experimental (Finding F2).
let first_party_betas = matches!(self.config.provider_topology, ProviderTopology::FirstParty)
    && self.config.experimental_betas_enabled;

// ContextManagement — TS betas.ts:307-311.
// Note: TS also has an Ant-only opt-in branch (USE_API_CONTEXT_MANAGEMENT
// + USER_TYPE === 'ant') for the tool-clearing case; coco-rs does not port
// the Ant branch (§3.5). Only the model-driven branch
// (`thinkingPreservationEnabled = modelSupportsContextManagement(...)`) is wired.
//
// **Shared resolver predicate (Round-3 Finding 2).** This site and
// `prepare_tools.rs::handle_memory_tool` BOTH emit the same beta header.
// Both must call the same predicate `beta_resolver::should_emit_context_management`,
// otherwise drift is invisible: the body path could honor the gate while the
// memory-tool path leaks the beta whenever the memory tool is registered.
if beta_resolver::should_emit_context_management(self.config.as_ref()) {
    betas.insert(beta_capabilities::map_capability(AdapterBetaCapability::ContextManagement).into());
}

// RedactThinking — TS betas.ts:270-277.
// includeFirstPartyOnlyBetas && modelSupportsISP
//   && !showThinkingSummaries && !getIsNonInteractiveSession()
if first_party_betas
    && self.config.capabilities.interleaved_thinking
    && !self.config.show_thinking_summaries
    && !self.config.non_interactive
{
    betas.insert(beta_capabilities::map_capability(AdapterBetaCapability::RedactThinking).into());
}

// NEW: TTL resolution (per-call; latches eligibility + allowlist internally).
// **account_kind / in_overage come from self.config (session-stable), NOT from
// anthropic_options (per-call).** Round-3 Finding 3 — if these came from
// the per-call options, a first call missing them would default-latch
// eligibility=false and corrupt every subsequent subscriber request for
// the lifetime of this AnthropicMessagesLanguageModel.
let resolved_ttl = if let Some(ref strategy) = anthropic_options.cache_strategy {
    if matches!(strategy.mode, AdapterCacheMode::Disabled) {
        None
    } else {
        Some(self.cache_policy.resolve_ttl(
            strategy.ttl,
            self.config.account_kind,                          // §10.0 session-stable field
            self.config.in_overage,                            // §10.0 session-stable field
            anthropic_options.query_source.as_deref(),         // per-call (TS parity)
            &self.config.prompt_cache_allowlist,               // §10.0 field
        ))
    }
} else { None };

// NEW: scope downgrade + PromptCachingScope beta — both gated on FirstParty.
// TS gate for the beta itself (betas.ts:227-232): strict firstParty + !disable_experimental.
let resolved_scope = anthropic_options
    .cache_strategy
    .as_ref()
    .and_then(|s| s.scope)
    .filter(|sc| {
        // Org needs no beta and no gate — pass through.
        // Global must be downgraded to None unless first-party.
        *sc != AdapterCacheScope::Global || first_party_betas
    });
if matches!(resolved_scope, Some(AdapterCacheScope::Global)) {
    betas.insert(beta_capabilities::map_capability(AdapterBetaCapability::PromptCachingScope).into());
}

// NEW: user-requested beta top-up (TS getSdkBetas) — applied LAST so a user
// override always wins. The set is dedup'd by HashSet, so adding a beta
// twice is a no-op.
//
// **TS allowlist parity (Finding F4).** TS `betas.ts:33-37` defines
// `ALLOWED_SDK_BETAS = [CONTEXT_1M_BETA_HEADER]` — i.e., a third-party
// SDK consumer can only push `context-1m-2025-08-07` through `getSdkBetas`.
// Other betas (interleaved-thinking, fast-mode, structured-outputs, etc.)
// are first-party-only or capability-derived; SDK consumers who want them
// must set them through the raw escape hatch `anthropic_beta:
// Option<Vec<String>>` (§10.5), which carries no parity guarantee.
//
// coco-rs mirrors that allowlist: `requested_betas` accepts the full
// typed enum at the API surface for forward-compat, but the resolver
// drops everything except `AdapterBetaCapability::Context1m` here. Other
// variants are silently ignored — surfacing a warning would push policy
// into the host, and the user can always reach for `anthropic_beta` if
// they really need to bypass the gate.
if let Some(ref user_caps) = anthropic_options.requested_betas {
    for cap in user_caps {
        if matches!(cap, AdapterBetaCapability::Context1m) {
            betas.insert(beta_capabilities::map_capability(*cap).to_string());
        }
    }
}

// NEW: build the auto_marker strategy and thread it into convert_to_anthropic_messages_full.
//
// **Capability gate is enforced HERE** (Finding F1). The inference-side
// `ApiClient::supports_prompt_cache()` is permissive for unknown models
// (None = test/mock path); this adapter-side gate is the real guard. If
// the model never declared `Capability::PromptCache` in its registry
// entry, `self.config.capabilities.prompt_cache` is false (§10.0
// "empty caps = no auto marker") and no marker is placed regardless of
// what `cache_strategy.mode` requested.
let auto_marker = match (anthropic_options.cache_strategy.as_ref(), resolved_ttl) {
    (Some(s), Some(ttl))
        if matches!(s.mode, AdapterCacheMode::Auto)
        && self.config.capabilities.prompt_cache =>
    {
        Some(cache_placement::CacheMarkerStrategy {
            ttl,
            scope: resolved_scope,
            skip_cache_write: s.skip_cache_write,
        })
    }
    _ => None,
};
// Then later when the existing code calls convert_to_anthropic_messages_full,
// pass `auto_marker` as the new 5th argument (§10.2).

// EXISTING: header emission — line 760-763. PATCHED to sort before join (Finding 7).
let mut beta_str: Vec<&str> = betas.iter().map(String::as_str).collect();
beta_str.sort_unstable();                                   // PATCH: stable order
headers.insert("anthropic-beta".into(), beta_str.join(","));
```

**Stable ordering (Finding 7 fix):** Rust's `HashSet<String>` uses `RandomState` by default; `iter()` yields a different order per run. Sort before joining (or change `betas` to `BTreeSet<String>` for free sort). Without this fix, `extra_body_hash` on the merged provider options (§9.7) becomes flaky and snapshot tests intermittently break. The Anthropic server tolerates beta-order variation in the header itself, so this is a determinism fix, not a wire-correctness fix.

### 10.5 Existing escape hatches

Both retained, no changes:

- `AnthropicProviderOptions.cache_control: Option<CacheControlConfig>` (`anthropic_messages_options.rs:132`) — low-level user override. Composes additively with auto-placed markers; `CacheControlValidator` 4-cap applies.
- `AnthropicProviderOptions.anthropic_beta: Option<Vec<String>>` (`anthropic_messages_options.rs:144`) — raw beta string list, useful for new betas before they have a `BetaCapability` variant.

## 11. Wire Format

End-to-end JSON for an Anthropic call with prompt cache enabled:

```jsonc
// LanguageModelV4CallOptions.provider_options:
{
  "anthropic": {
    "cacheStrategy": {
      "mode": "auto",
      "ttl": "one_hour",                  // requested; adapter may downgrade per eligibility
      "scope": "org",
      "skipCacheWrite": false
    },
    "requestedBetas": ["context_1m"],     // user top-up; TS-allowlisted (Context1m only — F4)
    "agenticQuery": true,                 // main agent loop (helper calls send false)
    "querySource":  "repl_main_thread",
    // Note: NO "accountKind" / "inOverage" — those are session-stable on
    // AnthropicConfig, set by build_anthropic from RuntimeConfig.account.*
    // (R3-F3); they never appear in per-call provider_options.
    "thinking":          { "type": "enabled", "budgetTokens": 10000 },
    "contextManagement": { /* opaque, owned by coco-compact */ }
  }
  // No "openai" / "google" entries when api == Anthropic.
}
```

**Note:** there is no `betaCapabilities` key on the wire — beta resolution is fully internal to `vercel-ai-anthropic`. Only the user's `requestedBetas` top-up is forwarded; capability + baseline betas are computed in the adapter. Per Finding F4, the adapter applies a TS-mirror allowlist (`ALLOWED_SDK_BETAS = [Context1m]`) on the typed channel — any other variants in `requestedBetas` are silently dropped. To inject a non-Context1m beta through the SDK, callers must use the raw `anthropic_beta: Option<Vec<String>>` escape hatch (§10.5), which forwards verbatim with no parity guarantee.

**No `userType` / `entrypoint` keys.** coco-rs deliberately does not surface `UserType::Ant` (§7.1), and `Entrypoint` is only ever load-bearing for the dropped `cli-internal-2026-02-09` beta. Anthropic-internal forks that need them can extend `AnthropicProviderOptions` downstream.

After `vercel-ai-anthropic` consumes:

```http
POST /v1/messages
anthropic-version: 2023-06-01
anthropic-beta: claude-code-20250219,context-1m-2025-08-07,context-management-2025-06-27,
                interleaved-thinking-2025-05-14,oauth-2025-04-20

{
  "model": "claude-sonnet-4-6",
  "system": [
    { "type": "text", "text": "...", "cache_control": { "type": "ephemeral" } },
    { "type": "text", "text": "..." }
  ],
  "tools": [
    { "name": "Read", "input_schema": {...}, "cache_control": { "type": "ephemeral" } }
  ],
  "messages": [
    { "role": "user", "content": [...] },
    { "role": "assistant", "content": [...] },
    { "role": "user", "content": [
      { "type": "text", "text": "...", "cache_control": { "type": "ephemeral", "ttl": "1h" } }
    ] }
  ],
  "thinking": { "type": "enabled", "budget_tokens": 10000 },
  "context_management": {...}
}
```

## 12. CacheBreakDetector Integration

Owner: `crate-coco-inference.md` (referenced, not redefined here).

### 12.1 Why the detector currently misses new keys

`coco-rs/services/inference/src/client.rs:656-697` (`build_prompt_state_input`) computes:

```rust
let extra_body_hash = params
    .context_management
    .as_ref()
    .map(canonical_extra_body_hash)
    .unwrap_or(0);
// ...
PromptStateInput {
    cache_control_hash: 0,             // hardcoded 0
    betas: Vec::new(),                 // never populated
    extra_body_hash,                   // hashes context_management ONLY
    // ...
}
```

The detector input fields `cache_control_hash` and `betas` are never populated, and `extra_body_hash` only sees `params.context_management` — not the merged `extra_body` map produced by `build_call_options` Lane B.

So adding new keys (`cacheStrategy`, `agenticQuery`, `querySource`, `requestedBetas`) to `provider_options["anthropic"]` does **NOT** make them visible to the detector — contrary to what an earlier draft of this doc claimed. (`accountKind` and `inOverage` are not in this list either — Round-3 Finding 3 moved them to `AnthropicConfig`, where they're caught by `ProviderClientFingerprint` instead of the per-call hash.)

`canonical_extra_body_hash` itself is correctly key-agnostic; the bug is in what's fed to it.

### 12.2 Refactor (required by this design)

Make `build_call_options` return the merged flat `extra` map alongside the call options, and pass it to `build_prompt_state_input`:

```rust
// services/inference/src/build_call_options.rs
pub fn build_call_options(...) -> (LanguageModelV4CallOptions, BTreeMap<String, Value>) {
    // ... existing logic up through namespace wrap ...
    (call, extra_clone_before_wrap)
}

// services/inference/src/client.rs
fn build_prompt_state_input(
    client, params, query_source, layout_hashes,
    merged_extra: &BTreeMap<String, Value>,    // NEW
) -> PromptStateInput {
    let extra_body_hash = if merged_extra.is_empty() {
        0
    } else {
        let v = serde_json::to_value(merged_extra).unwrap_or(Value::Null);
        canonical_extra_body_hash(&v)
    };
    // ... rest unchanged ...
}
```

Mock paths that bypass `build_options` continue to pass `&BTreeMap::new()`, preserving current behavior on the test path.

**What this refactor actually tracks (R3-F5 — narrow, precise claim).** After §12.2 lands, the detector hash captures **the namespace-pre-wrap flat `merged_extra` map** that `build_call_options` produces. Specifically, this includes:

- ✅ Per-call typed inputs that flow through `cache_convert::to_extra_body` (`cacheStrategy`, `agenticQuery`, `querySource`, `requestedBetas`)
- ✅ Per-call typed inputs from other lanes (`thinking`, `context_management`, etc.) merged into `extra` before wrap
- ✅ User-supplied raw `extra_body` overrides

What this refactor does **NOT** track (deliberately, with rationale):

- ❌ **Adapter-resolved beta header.** The actual `anthropic-beta` string is composed inside `vercel-ai-anthropic::get_args` *after* the call options leave inference. Capability betas, OAuth beta, RedactThinking, PromptCachingScope are merged in there. Inference cannot see these without reaching across the layer boundary.
- ❌ **`AnthropicConfig` session-stable fields** (`capabilities`, `provider_topology`, `experimental_betas_enabled`, `disable_interleaved_thinking`, `show_thinking_summaries`, `non_interactive`, `account_kind`, `in_overage`, `prompt_cache_allowlist`). These don't appear on per-call options — they're set once at provider construction. A change to any of them goes through `RuntimeConfig` rebuild → `ProviderClientFingerprint` change → cache-clear is handled by `ProviderClientFingerprint::compute` + the existing turn-boundary coherence check, NOT by the per-call detector hash. (See multi-provider-plan §11.1.)
- ❌ **`prompt_layout`-side cache_control on system blocks.** The placement step §10.2 mutates the post-grouping wire body inside `convert_to_anthropic_messages_full`, after the inference-side hash is computed. Same boundary: inference cannot observe the placed marker.

This split is **correct by design**. Per-call data → detector hash (turn-by-turn break detection). Session-stable data → fingerprint rebuild (turn-boundary coherence). Adapter-internal final wire data → not tracked because the detector's job is to detect *user-visible cache invalidations*, which always trace to a user-observable input (model swap, TTL flip, querySource change, settings reload), not to a deterministic adapter computation.

**§14.1 test additions (R3-F5):**

- `extra_body_hash_changes_when_cache_strategy_ttl_flips` — turn-1 ttl=5min, turn-2 ttl=1h → different hashes
- `extra_body_hash_changes_when_query_source_changes_with_active_strategy` — already covered, now extended
- `extra_body_hash_unchanged_when_only_session_stable_field_changes` — invariant: rebuilding `AnthropicConfig` with a different `account_kind` does NOT change the per-call detector hash (correctly, because that path goes through the fingerprint instead)
- `fingerprint_changes_when_account_kind_changes` — confirms the cross-check: account_kind changes are caught by `ProviderClientFingerprint`, not by `extra_body_hash`

### 12.3 Threshold parity check

Verify that the existing detector matches TS thresholds at `promptCacheBreakDetection.ts:494`:

| Condition | TS | coco-rs (must verify in `cache_detection.rs`) |
|---|---|---|
| Drop fraction | `cacheReadTokens < prevCacheRead * 0.95` | Confirm 5%-drop constant |
| Absolute floor | `tokenDrop >= MIN_CACHE_MISS_TOKENS` | Confirm `MIN_CACHE_MISS_TOKENS` value matches TS |
| Tracking key | `(querySource, agentId)` | Confirm `CacheBreakDetector` keying |
| Excluded models | Haiku | Confirm exclusion |

Any divergence is a parity bug to fix in `crate-coco-inference.md`'s implementation, not this design doc.

## 13. Provider Isolation (Multi-Provider Safety)

The wire-format guarantee:

```
api == Anthropic   → provider_options["anthropic"] has cacheStrategy/requestedBetas/agenticQuery/querySource (per-call only; account/overage live on AnthropicConfig — R3-F3)
api == OpenAI      → provider_options["openai"]    is untouched by this design
api == Gemini      → provider_options["google"]    is untouched
api == Volcengine  → provider_options["volcengine"] is untouched
api == Zai         → provider_options["zai"]        is untouched
api == OpenaiCompat → provider_options[<instance>]  is untouched
```

Three guards enforce this:

1. **Capability gate:** `ApiClient::supports_prompt_cache()` returns false for non-Anthropic. Callers using this gate skip cache fields entirely.
2. **`cache_convert::to_extra_body` gate:** explicit `match api { ProviderApi::Anthropic => ..., _ => BTreeMap::new() }`. No keys emitted under non-Anthropic namespaces. (build_call_options.rs Lane B's namespace wrap only operates on the active provider's namespace, so even if a key escaped, it would never reach OpenAI's namespace.)
3. **Isolation invariant from `build_call_options.rs:177-184`:** `canonical_namespace_key` returns a single namespace string per call; flat `extra` is wrapped under that one namespace. No cross-namespace fan-out is possible by construction.

A safety test (see §14.1) asserts that emitting `PromptCacheConfig` with `ApiClient` over `ProviderApi::Openai` produces `provider_options["openai"]` with **zero** prompt-cache keys.

## 14. Test Strategy

Tests follow the `#[path = "<name>.test.rs"]` companion-file convention (CLAUDE.md mandate).

### 14.1 In `coco-rs/services/inference/src/`

- `cache_convert.test.rs` (new):
  - `anthropic_emits_camelcase_keys` — `cacheStrategy`, not `cache_strategy`
  - `non_anthropic_emits_no_keys` — OpenAI/Gemini/Volcengine all return empty maps
  - `disabled_mode_emits_no_keys`
  - `session_context_writes_account_agentic_overage_query_source` — verifies the 4 keys present (no `userType` / `entrypoint`)
  - `session_context_does_not_emit_user_type_or_entrypoint` — explicit guarantee for §7.1 / §11
  - `session_context_skipped_for_non_anthropic`
- `build_call_options.test.rs` (extend existing 9 cases):
  - `cache_strategy_per_call_writes_anthropic_namespace`
  - `cache_strategy_skipped_for_openai_namespace`
  - `account_context_does_not_leak_to_other_providers`
  - `merged_extra_returned_for_detector_input` (Finding 5 refactor)
- `cache_detection.test.rs` (extend):
  - `cache_strategy_change_invalidates_extra_body_hash`
  - `account_kind_change_invalidates_extra_body_hash`
  - `query_source_change_does_NOT_change_hash_when_strategy_disabled`

### 14.2 In `coco-rs/vercel-ai/anthropic/src/`

- `cache_policy.test.rs` (new — moved from inference):
  - `eligibility_latches_on_first_call` — flip overage state mid-session, eligibility frozen, but per-call match still recomputes (Finding 2 regression)
  - `allowlist_latches_on_first_call`
  - `non_eligible_returns_5min`
  - `allowlist_pattern_match_exact_and_wildcard`
  - `query_source_none_returns_5min`
  - `non_allowlisted_after_allowlisted_first_call_returns_5min` — explicit Finding 2 regression: ensures the per-call match is fresh, not latched
  - `api_key_account_eligibility_false` — coco-rs's typical user (`AccountKind::ApiKey`, not subscriber) never gets 1h TTL
  - `subscriber_in_overage_eligibility_false` — subscriber + overage → 5min
  - `subscriber_no_overage_eligibility_true` — the only path to 1h TTL in coco-rs (the TS `USER_TYPE === 'ant'` branch is intentionally not ported)
  - `account_kind_is_session_stable_not_per_call` — R3-F3 regression: build `AnthropicConfig` with `account_kind = ApiKey, in_overage = false`, make a first call, then construct a second `AnthropicConfig` with `account_kind = ClaudeAiSubscriber` and verify the second instance latches eligibility = true. Confirms there's no per-call corruption path: per-call options have NO account fields (compile-time guarantee).
  - `latch_is_per_language_model_instance_not_global` — F11 regression: two `AnthropicMessagesLanguageModel` instances for the same model id with different account_kind on their respective `AnthropicConfig` resolve TTL independently.
- `beta_resolver.test.rs` (new — moved from inference):
  - `capability_betas_memoized` — second call returns same `Arc`
  - `interleaved_thinking_disabled_when_settings_disable_set`
  - `unknown_model_with_user_declared_capabilities_works` — registry seed has only 3 builtins; user-supplied `claude-3-5-sonnet` ModelInfo with `[PromptCache]` produces correct capability set
- `cache_placement.test.rs` (new):
  - `marker_on_last_message_default`
  - `marker_on_second_to_last_when_skip_cache_write`
  - `no_marker_when_last_block_is_thinking` — TS claude.ts:653-655 fidelity
  - `no_marker_when_last_block_is_redacted_thinking`
  - `marker_index_uses_post_grouping_array_not_raw_prompt` — Finding 6: feed prompt with consecutive same-role messages (which `group_into_blocks` merges); assert marker lands on the merged wire message, not on the original prompt index
  - `disabled_mode_returns_none`
  - `empty_messages_returns_none`
  - `auto_marker_does_not_double_mark_when_user_supplied_cache_control_present` — auto + user-supplied `cache_control` on the same target message; `CacheControlValidator` consumes only one breakpoint
  - `auto_marker_respects_4_breakpoint_cap` — pre-fill 4 user breakpoints; auto marker is a no-op (validator emits Warning, not error)
- `convert_to_anthropic_messages.test.rs` (extend):
  - `auto_marker_strategy_attaches_cache_control_to_last_message`
  - `auto_marker_skipped_when_target_is_reasoning`
  - `none_auto_marker_preserves_existing_behavior` — Manual mode and absent strategy both leave messages untouched; regression guard for the `auto_marker: Option<...>` parameter
- `beta_capabilities.test.rs` (new):
  - `capability_to_string_table_complete` — every `BetaCapability` variant has a wire string
  - `baseline_non_agentic_api_key_no_betas` — non-agentic + `AccountKind::ApiKey` returns empty list
  - `baseline_agentic_api_key_emits_only_claude_code` — agentic + `ApiKey` → `[claude-code-20250219]`
  - `baseline_subscriber_non_agentic_emits_only_oauth`
  - `baseline_subscriber_agentic_emits_claude_code_and_oauth`
  - `baseline_no_cli_internal_ever` — exhaustive matrix over `(AccountKind, agentic)` confirms `cli-internal-2026-02-09` is never emitted from this crate
  - `baseline_no_model_string_match` — invariant test: passing model_id "claude-haiku-4-5" vs "claude-sonnet-4-6" does NOT change baseline output (no `is_haiku` heuristic)
- `anthropic_messages_options.test.rs` (extend):
  - `cache_strategy_field_round_trips`
  - `requested_betas_field_round_trips`
  - `agentic_query_query_source_round_trip` — only the 2 per-call session-context fields remain (R3-F3 moved account_kind/in_overage to AnthropicConfig)
  - `no_user_type_or_entrypoint_field_present` — schema invariant for §7.1
  - `no_account_kind_or_in_overage_in_per_call_options` — R3-F3 schema invariant: per-call options surface MUST NOT have these fields
  - `extract_strips_internal_keys_from_raw_map` — Finding 2: feed `{cacheStrategy: ..., agenticQuery: ..., thinking: ...}`; assert raw map contains `thinking` but not the 4 internal keys
  - `wire_body_does_not_contain_internal_anthropic_keys` — Finding 2 end-to-end: build a full `do_generate` body with all 4 internal fields set; serialize; assert none of `cacheStrategy` / `requestedBetas` / `agenticQuery` / `querySource` appear at any depth
- `anthropic_messages_language_model.test.rs` (extend):
  - `beta_header_is_sorted_for_stable_order` (Finding 7 regression)
  - `subscriber_emits_oauth_beta`
  - `non_agentic_call_excludes_claude_code_baseline_for_any_model` — replaces the old "haiku excludes claude-code" test; works for any model id including non-Anthropic-named ones
  - `agentic_call_includes_claude_code_baseline_for_any_model`
  - `requested_betas_topup_merges_with_capability_betas` — `Context1m` requested + `Context1m` capability declared → emitted once (HashSet dedup)
  - `requested_betas_filter_drops_non_context1m_variants` — Finding F4: `requested_betas = {Context1m, FastMode, InterleavedThinking}` only emits `context-1m-2025-08-07`. Other typed variants are silently dropped to match TS `ALLOWED_SDK_BETAS = [CONTEXT_1M_BETA_HEADER]`. Users wanting non-Context1m betas through the SDK must use the raw `anthropic_beta` escape hatch.
  - `raw_anthropic_beta_escape_hatch_unrestricted` — `requested_betas` is empty but `anthropic_beta = vec!["fast-mode-2026-02-01"]` is forwarded verbatim — confirms F4 only restricts the **typed** SDK channel, not the raw one
  - `redact_thinking_emitted_when_first_party_topology_and_isp_capable_and_interactive` — Finding 5: gate is `provider_topology == FirstParty && experimental_betas_enabled && capabilities.contains(InterleavedThinking) && !show_thinking_summaries && !non_interactive`
  - `redact_thinking_skipped_when_experimental_betas_disabled`
  - `redact_thinking_skipped_when_non_interactive_or_show_thinking_summaries` — covers the runtime-flag side of the gate (the topology side gets a test once a non-FirstParty variant exists)
  - `prompt_caching_scope_global_only_emitted_when_first_party_and_experimental` — Finding 5: setting `cache_strategy.scope = Global` with `experimental_betas_enabled = false` must NOT emit `prompt-caching-scope-2026-01-05` beta (or, equivalently, must downgrade scope to None). Once a non-FirstParty `ProviderTopology` variant exists (deferred §2), this test gains a topology-gate companion.
  - `capabilities_read_from_anthropic_config_not_model_id` — Finding 1: construct `AnthropicConfig` with `capabilities = [PromptCache, Context1m]` and `model_id = "totally-unknown"`; verify `context-1m-2025-08-07` appears in betas. Asserts no model-id heuristic anywhere in the resolution path.
  - `forbidden_strings_NEVER_appear_in_source` — meta test, **scoped to the new prompt-cache/beta-policy modules only** (`cache_policy.rs`, `beta_resolver.rs`, `cache_placement.rs`, `beta_capabilities.rs`, `system_block_scope.rs`). Searches each for `"haiku"`, `"user_type"`, `"UserType"`, `"cli-internal"`, `"summarize-connector-text"` literal occurrences (excluding doc-comments) and asserts zero matches. Crate-wide search would false-positive on legitimate model-capability strings elsewhere in the provider (e.g., `"haiku"` may appear in pre-existing `modelSupports*` style code or test fixtures); the invariant is about the new policy code, so the scope matches.
  - `adapter_does_not_import_coco_types` — **F8 invariant.** Walks `vercel-ai-anthropic/src/**/*.rs`, asserts no `use coco_` or `coco_types::` / `coco_config::` paths appear. Covers all current source plus regressions where someone reaches for `coco_types::Capability` etc. Excludes doc comments (the F8 fix doc itself contains many such references, but those live in design docs not in source).
  - `wire_round_trip_adapter_to_coco_types_parity` — **F8 round-trip companion.** For each cross-boundary type pair (`PromptCacheMode` / `AdapterCacheMode`, `CacheTtl` / `AdapterCacheTtl`, `CacheScope` / `AdapterCacheScope`, `AccountKind` / `AdapterAccountKind`, `BetaCapability` / `AdapterBetaCapability`), serialize each variant from the coco-types side, deserialize on the adapter side, assert structural equality. Locks the wire format independent of either enum's `serde(rename)` rules — drift in either crate is caught by failing the round-trip.
  - `unknown_anthropic_model_via_user_override_works` — pass a `ModelInfo` for `"my-claude-finetune"` with `[PromptCache, Context1m]` capabilities; verify both betas appear

### 14.3 Snapshot tests

Insta snapshot of a fully assembled `LanguageModelV4CallOptions` for a representative request, locking the wire format. One per provider (Anthropic with all features on, OpenAI without prompt cache, Gemini without prompt cache).

### 14.4 Integration tests (split per migration phase)

Currently `prompt_layout.rs:181` emits a single `AnthropicSystemBlock` with `cache_control: None` and `tool_schemas.rs:68` always sets `provider_options: None`. The full E2E with system + tool markers requires the plumbing additions in §15 steps 5a + 5b. So the integration test splits into two phases.

**Phase 1** — `prompt_cache_e2e_message_marker.rs` (lands with steps 1 + 2 + 3a + 3b + 4a + 4b + **4c** of §15 — Finding F7):

> **Why through 4c, not 1-3.** Steps 1-3 alone produce typed coco-types, capability declarations, and inference-side pass-through, but the wire body has no `cache_control` because no adapter code reads `cache_strategy` yet. Step 4a adds the adapter-side fields on `AnthropicConfig`; step 4b adds the deny-list (without it, internal keys leak to wire and the message-marker assertion is meaningless because the body is malformed); step 4c is the actual `cache_policy` + `beta_resolver` + `cache_placement` + `beta_capabilities` + `auto_marker` wiring that emits the marker. Phase 1's expected output `messages[N-1].cache_control` does not exist until 4c lands.

- `anthropic-beta` header contains expected betas (sorted, deterministic order)
- `messages[N-1]` carries `cache_control: { type: "ephemeral", ttl: "1h" }`
- `messages[N-2]` does NOT carry `cache_control` (skip_cache_write=false)
- Total `cache_control` markers ≤ 4
- `provider_options["openai"]` is empty (multi-provider isolation)

**Phase 2** — `prompt_cache_e2e_full.rs` (lands with steps 5a + 5b of §15, after system-block scope + tool opt-in plumbing):

- `system[0]` carries `cache_control` (cacheScope-driven; mirrors `splitSysPromptPrefix`)
- Selected tools carry `cache_control` per `ToolSchemaSource.cache_control` opt-in
- Total `cache_control` markers ≤ 4 across messages + system + tools
- 1h TTL eligibility latch holds across mid-session overage flip (regression test for Finding 2)
- User-supplied unknown model with `Capability::PromptCache` declared still emits `cache_control` — exercises the override path that replaces hardcoded model-name lists (regression for the model-string-anti-pattern)
- Same wire output regardless of whether the model id contains `"haiku"` literal — invariant test for "no `is_haiku` heuristic anywhere"
- `cli-internal-2026-02-09` and `summarize-connector-text-2025-08-22` NEVER appear in the `anthropic-beta` header for any combination of `(AccountKind, agentic, model_id, request)` — invariant test for §7.1 (Ant-only betas not ported)

## 15. Migration Plan

| Step | Crate(s) | Files touched | Tests |
|---|---|---|---|
| 1. New shared types | `coco-types` | new `src/cache.rs`; extend `Capability` enum (5 variants: `PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`, `TokenEfficientTools`). No `RedactThinking` capability — folded into `InterleavedThinking` per §10.1b. No `UserType` / `Entrypoint` reuse. | serde round-trip in `cache.test.rs` |
| 2. Capability declarations | `coco-config` | `model/registry.rs:216-447` — add capabilities (`PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`) to the **3 builtin claude-4 models** per §8.1. **Do NOT** seed `claude-3-*` or any other variant — those go through the user-override path. | `registry.test.rs` builtin resolve assertions; user-override resolve test for an unknown model id |
| 3a. Detector hash refactor + query-flow consolidation (Finding 3 + Finding 5) | `services/inference` | `build_call_options.rs` returns `(call, merged_extra)`; `client.rs::query` builds options once before retry loop and feeds `merged_extra` to `build_prompt_state_input`; `do_query` accepts pre-built options | `cache_detection.test.rs` regression: `query_options_built_once_per_call`, `merged_extra_invalidates_hash` |
| 3b. Inference pass-through layer | `services/inference` | new `cache_convert.rs`; extend `build_call_options.rs:46-65, 113-125`, `client.rs:32-80, 514-572`. `session_context_to_extra_body` is no-op when `cache_strategy` absent or Disabled (Finding 4). No `cache_policy`, `beta_resolver`, `OnceLock` here. | §14.1 (incl. `query_source_change_does_NOT_change_hash_when_strategy_disabled`) |
| 4a. AnthropicConfig + AnthropicProviderSettings extension (Findings 1, 5, 8, 10, 13, 14, R3-F1, R3-F3) | `vercel-ai-anthropic` + `services/inference` | **In `vercel-ai-anthropic`:** extend `anthropic_config.rs:5-23` with `capabilities: AnthropicModelCapabilities`, `provider_topology: ProviderTopology`, `experimental_betas_enabled: bool`, `disable_interleaved_thinking: bool`, `show_thinking_summaries: bool`, `non_interactive: bool`, `prompt_cache_allowlist: Vec<String>`, **`account_kind: AdapterAccountKind` (R3-F3, session-stable)**, **`in_overage: bool` (R3-F3, session-stable)**; add the new adapter-side types (`AnthropicModelCapabilities`, `ProviderTopology`, `AdapterCacheMode/Ttl/Scope`, `AdapterAccountKind`, `AdapterBetaCapability`) — none of which import `coco_*`. Extend `anthropic_provider.rs::AnthropicProviderSettings` (lines 17-42), `AnthropicProvider` storage (lines 48-56), `make_config()` (lines 116-126) with the matching fields. **In `services/inference`:** change `build_anthropic` signature to `(runtime, provider_cfg, api_model, timeout_secs, model_info)` (R3-F1); update the two callers (`build_language_model_from_runtime:117` and `build_api_client:150` indirectly); add `model_factory.rs::anthropic_caps_from(Option<&Vec<Capability>>) -> AnthropicModelCapabilities` translator alongside; `provider_topology` is hardcoded `FirstParty` in this iteration (Bedrock deferred §2). | `model_factory.test.rs` translator regression; `anthropic_config.test.rs` defaults; `cache_strategy_wire_format_matches_adapter_struct` (round-trip test from §10.1); `build_anthropic_signature_takes_runtime_and_model_info` (R3-F1 regression) |
| 4b. Internal-keys deny-list (Finding 2) | `vercel-ai-anthropic` | add `INTERNAL_ANTHROPIC_OPTION_KEYS` const in `anthropic_messages_options.rs`; strip from `raw` before returning from `extract_anthropic_options` (line 226) | `extract_strips_internal_keys_from_raw_map`, `wire_body_does_not_contain_internal_anthropic_keys` (§14.2) |
| 4c. Anthropic adapter — policy + memo + wire | `vercel-ai-anthropic` | new `cache_policy.rs`, `beta_resolver.rs` (incl. `should_emit_context_management` shared predicate per R3-F2), `cache_placement.rs`, `beta_capabilities.rs`; extend `messages/anthropic_messages_options.rs:122-149, 232-255` with the **4 new per-call fields** (`cache_strategy`, `requested_betas`, `agentic_query`, `query_source` — `account_kind`/`in_overage` are NOT here per R3-F3); extend `messages/anthropic_messages_language_model.rs:413-763` (sort betas before join — Finding 7; replace unconditional context-management insert at line 710 with `beta_resolver::should_emit_context_management(...)` gate); patch `messages/prepare_tools.rs:236` (memory tool) to use the same `should_emit_context_management` predicate (R3-F2); extend `messages/convert_to_anthropic_messages.rs:170-175` with `auto_marker: Option<CacheMarkerStrategy>` parameter and post-grouping marker attachment (Finding 6) | §14.2 (incl. `marker_index_uses_post_grouping_array_not_raw_prompt`, `capabilities_read_from_anthropic_config_not_model_id`, redact-thinking topology gates, `memory_tool_does_not_leak_context_mgmt_beta_when_gate_closed` per R3-F2) |
| 5a. System-block scope plumbing | `services/inference` + `vercel-ai-anthropic` | extend `AnthropicSystemBlock` with `scope: Option<CacheScope>`; refactor `prompt_layout::build_prompt_layout_from_prompt` (currently emits one block with `cache_control: None` at line 181) to emit multiple blocks tagged by scope (mirrors TS `splitSysPromptPrefix`); adapter materializes `cache_control` from scope | `prompt_layout.test.rs` + `convert_to_anthropic_messages.test.rs` |
| 5b. Tool opt-in plumbing | `services/inference` | extend `ToolSchemaSource` with `cache_control: Option<CacheControlHint>`; `tool_schemas.rs:68-75` populates `provider_options` instead of `None` when hint present; existing `prepare_tools.rs:114-124` consumes it | `tool_schemas.test.rs` |
| 6. Detector threshold parity check | `services/inference` | verify constants in `cache_detection.rs` match TS 5% / `MIN_CACHE_MISS_TOKENS` (`promptCacheBreakDetection.ts:494`); fix if divergent | regression test if divergent |
| 6b. Existing context-management TS-divergence — **two emission points** (Finding F2 + Round-3 Finding 2) | `vercel-ai-anthropic` | `context-management-2025-06-27` is unconditionally inserted at **two** sites today: (a) `anthropic_messages_language_model.rs:710-714` (body-path: when the typed `context_management` field is present); and (b) `prepare_tools.rs:236` (tool-path: when the `anthropic.memory_20250818` tool is registered, since the memory tool *requires* the context-management beta to function). Patching only (a) leaves (b) leaking the beta whenever the memory tool is in scope — **the gate must apply to both, and they must share one resolver** to avoid drift. **Resolution:** introduce `beta_resolver::should_emit_context_management(config: &AnthropicConfig) -> bool` returning `first_party_betas && capabilities.context_management` (the same predicate as §10.4); call from both (a) and (b). For (b), if the predicate is false but the user nonetheless registered `anthropic.memory_20250818`, the tool itself stays in the request body but the beta is suppressed — which means Anthropic will reject the call. That's the desired failure mode (loud, not silent), preferable to leaking a gated beta. The behavior is documented as "the memory tool requires `experimental_betas == true && capabilities.context_management == true` on a first-party topology; otherwise its registration produces a 400 from Anthropic". | (a) `context_management_beta_skipped_when_experimental_betas_disabled` (body-path); (b) `memory_tool_does_not_leak_context_mgmt_beta_when_gate_closed` (tool-path); (c) `both_paths_use_shared_resolver_predicate` (asserts `should_emit_context_management` is the unique gate function called by both sites) |
| 7. Caller wiring | `app/query` + `coco-config` (R3-F4 schema additions) | (a) add `coco-config::AccountConfig` (`account_kind`, `in_overage`) + `RuntimeConfig.account` field; (b) add `RuntimeConfig.prompt_cache: PromptCacheRuntimeConfig` (allowlist) per §16a.2; (c) add `RuntimeConfig.anthropic_knobs: AnthropicRuntimeKnobs` (4 bools) per §16a.3; (d) add new sections to `coco-rs/common/config/src/sections/`; (e) extend `coco_config::EnvKey` with the 5 new `COCO_*` env vars per §16a; (f) update `RuntimeConfigBuilder` to fold each section (§16a.4); (g) thread `runtime` + `model_info` into `build_anthropic` callers (R3-F1); (h) `QueryParams.cache` + `QueryParams.agentic` already on per-call surface — no per-call session wiring needed for `account_kind`/`in_overage` (R3-F3). No `entrypoint`, no `user_type`. | E2E smoke Phase 1 (§14.4); `prompt_cache_section.test.rs` + `anthropic_knobs_section.test.rs` defaults + env override + settings.json layering |
| 8. E2E full integration | `services/inference` tests | E2E Phase 2 (§14.4) — system + tool markers, all the regression tests for Findings 2/3/4/7 | §14.4 |
| 9. Doc updates | `docs/coco-rs/` | update `crate-coco-types.md` with 5 new `Capability` variants + new types; update `crate-coco-inference.md` with `cache_convert` module + detector hash refactor + query-flow change (NOT `cache_policy` / `beta_resolver`); update `vercel-ai/anthropic/CLAUDE.md` with new `AnthropicConfig` fields + `INTERNAL_ANTHROPIC_OPTION_KEYS` deny-list + new typed fields + new modules; add this doc to `CLAUDE.md` File Index | n/a |

Step ordering: 1, 2, 3a are pure additions (no behavior change). Step 3b adds inference-side keys but they are dead until step 7 wires them. Step 4a is a pre-req for 4c (capabilities access); 4b is independent and can land first as a defensive fix. Step 4c reads from 3b emitted keys and emits the message-level `cache_control` marker — **this is the step that unlocks the Phase 1 E2E test** (Finding F7). Steps 5a + 5b extend coverage to system + tool markers (Phase 2 E2E). Step 6 may reveal a TS-parity bug to fix separately.

## 16. Open Questions

1. **Where do session-level fields populate?** `AccountKind` and `in_overage` come from `coco-config` (`~/.coco/config.json` `account.kind`, `account.in_overage`). They flow to the adapter via **`RuntimeConfig.account: AccountConfig` → read by `build_anthropic` → stored on `AnthropicConfig`** (Round-3 Finding 3). They are NOT carried per-call. Owner: `crate-coco-config.md` to add the `account` schema; this design references the values and pins the read site (provider construction, not per-request).
2. **`load_allowlist` source.** Initially read from `~/.coco/config.json` `prompt_cache.allowlist: Vec<String>`. Future: hookable for remote feature-gate (TS uses GrowthBook `tengu_prompt_cache_1h_config`).
3. **`SystemPromptBlock::CacheBreakpoint` integration.** Defined in `provider-prompt-role-architecture.md:543-625`. When `mode: Manual`, this design defers placement to those hints; when `mode: Auto`, this design's algorithm runs. Confirm the precedence with the prompt-layout owners.
4. ~~**OAuth beta exact string.** `OAUTH_BETA_HEADER` value not visible in our TS extracts.~~ **Resolved.** Confirmed via `constants/oauth.ts:36`: `export const OAUTH_BETA_HEADER = 'oauth-2025-04-20'`. Note: the constant lives in `constants/oauth.ts`, not `constants/betas.ts` — adjust the import in §10.3 to `use crate::constants::OAUTH_BETA_HEADER;` (or hard-code `"oauth-2025-04-20"` with a `// constants/oauth.ts:36` comment).
5. ~~**`UserType::Ant` follow-up cleanup.**~~ **Withdrawn (Round-3 Finding 7).** `UserType::Ant` is still actively consumed by `coco-rs/skills/src/bundled.rs:144` (ant-only skill bundle gating) and `coco-rs/core/permissions/src/dangerous_rules.rs:25` + `mode_transition.rs:138` + `setup.rs:233` + `shell_rules.rs:265` (dangerous-rule strip behavior). This design does not depend on `UserType::Ant`, but does not invalidate other in-tree consumers; **no cleanup ticket should be filed.** The earlier "follow-up cleanup" prose has been removed from §7.1, §17, and §18.

## 16a. RuntimeConfig schema additions (Round-3 Finding 4)

> **Schema note (R7-10, May 2026):** The `AnthropicRuntimeKnobs` section described
> below was the original Round-3 design and shipped with Round 7. It has since
> been **migrated** to `ProviderConfig.provider_options` (per-provider opaque
> map, parsed by `vercel-ai-anthropic::parse_provider_options`) for
> extensibility — see `audit-gaps.md` R7-10..R7-15 for the rationale and the
> exact removed/added surface. The `prompt_cache` section + `account` section
> below remain accurate; only the `anthropic_knobs` field is gone.

The provider factory wiring in §10.0 reads from `runtime.prompt_cache.*` and (historically) `runtime.anthropic_knobs.*` — neither field existed on `coco-rs/common/config/src/runtime.rs:45` before Round 7. This section specifies the additions as originally designed.

### 16a.1 New `RuntimeConfig` fields

Add two new fields to `RuntimeConfig` (alongside the existing `compact: CompactConfig`, `mcp: McpRuntimeConfig`, etc.):

```rust
// in coco-rs/common/config/src/runtime.rs
pub struct RuntimeConfig {
    // ... existing fields ...

    /// 1h-TTL allowlist + future remote-feature-gate hook. Source of
    /// truth for `cache_policy::resolve_ttl`'s allowlist parameter.
    pub prompt_cache: PromptCacheRuntimeConfig,

    /// Anthropic-specific runtime knobs that govern beta gating.
    /// **Anthropic-only** — keep these out of generic sections so a
    /// non-Anthropic provider crate has no path to read them.
    /// Mirrors what TS reads from `process.env.*` and
    /// `getInitialSettings().*` (`betas.ts:215, :258-274`).
    pub anthropic_knobs: AnthropicRuntimeKnobs,
}
```

### 16a.2 `PromptCacheRuntimeConfig` (in `coco-rs/common/config/src/sections/prompt_cache.rs`)

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PromptCacheRuntimeConfig {
    /// Each entry is either an exact `query_source` match or a
    /// `prefix*` glob. Default empty — eligibility latch can still
    /// short-circuit before the allowlist match.
    #[serde(default)]
    pub allowlist: Vec<String>,
}
```

| settings.json key | env var | RuntimeOverrides | Default |
|---|---|---|---|
| `prompt_cache.allowlist: string[]` | `COCO_PROMPT_CACHE_ALLOWLIST` (comma-separated) | `RuntimeOverrides.prompt_cache_allowlist: Option<Vec<String>>` | `[]` |

### 16a.3 `AnthropicRuntimeKnobs` (in `coco-rs/common/config/src/sections/anthropic_knobs.rs`)

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnthropicRuntimeKnobs {
    /// TS `!DISABLE_EXPERIMENTAL_BETAS` (`betas.ts:215`). Drives
    /// first-party-only beta inclusion (RedactThinking,
    /// PromptCachingScope, ContextManagement). Default: enabled.
    #[serde(default = "default_true")]
    pub experimental_betas: bool,

    /// TS `process.env.DISABLE_INTERLEAVED_THINKING`
    /// (`betas.ts:258-262`). Suppresses the
    /// `interleaved-thinking-2025-05-14` beta even on capable models.
    #[serde(default)]
    pub disable_interleaved_thinking: bool,

    /// TS `getInitialSettings().showThinkingSummaries`
    /// (`betas.ts:268-275`). Suppresses `redact-thinking-2026-02-12`
    /// when true (showing summaries inverts the redaction need).
    #[serde(default)]
    pub show_thinking_summaries: bool,

    /// TS `getIsNonInteractiveSession()` (`betas.ts:268-275`).
    /// Suppresses `redact-thinking-2026-02-12` for non-interactive
    /// runs (no human to consume thinking redaction).
    #[serde(default)]
    pub non_interactive: bool,
}

impl Default for AnthropicRuntimeKnobs {
    fn default() -> Self {
        Self {
            experimental_betas: true,
            disable_interleaved_thinking: false,
            show_thinking_summaries: false,
            non_interactive: false,
        }
    }
}

fn default_true() -> bool { true }
```

| settings.json key | env var | RuntimeOverrides | Default |
|---|---|---|---|
| `anthropic.experimental_betas: bool` | `COCO_ANTHROPIC_EXPERIMENTAL_BETAS` | `RuntimeOverrides.anthropic_experimental_betas: Option<bool>` | `true` |
| `anthropic.disable_interleaved_thinking: bool` | `COCO_ANTHROPIC_DISABLE_INTERLEAVED_THINKING` | `RuntimeOverrides.anthropic_disable_interleaved_thinking: Option<bool>` | `false` |
| `anthropic.show_thinking_summaries: bool` | `COCO_ANTHROPIC_SHOW_THINKING_SUMMARIES` | `RuntimeOverrides.anthropic_show_thinking_summaries: Option<bool>` | `false` |
| `anthropic.non_interactive: bool` | `COCO_ANTHROPIC_NON_INTERACTIVE` (also auto-derived from `!is_tty()` if unset) | `RuntimeOverrides.anthropic_non_interactive: Option<bool>` | `false` |

The `COCO_*` prefix is mandated by `CLAUDE.md` (every coco-owned env var uses `COCO_*`); the matching `EnvKey` variants are added to `coco_config::EnvKey` enum.

### 16a.4 `RuntimeConfigBuilder` integration

`build_runtime_config` (the single fold-site for settings + env + overrides) already merges per-section configs in pass order. Add two more lines:

```rust
prompt_cache: build_prompt_cache_section(&settings, &env_only, &overrides),
anthropic_knobs: build_anthropic_knobs_section(&settings, &env_only, &overrides),
```

Each section helper follows the existing `CompactConfig` pattern: layered defaults → settings.json → env → overrides → final struct. Tests live in `coco-rs/common/config/src/sections/prompt_cache.test.rs` and `anthropic_knobs.test.rs`, mirroring the existing `compact.test.rs`.

### 16a.5 Why two sections, not one

`PromptCacheRuntimeConfig` is **provider-agnostic** (an Anthropic alternative-API or a future caching-aware OpenAI extension can read the same allowlist).

`AnthropicRuntimeKnobs` is **Anthropic-specific** — the four bools encode TS-`betas.ts`-specific behavior that other providers don't have. Keeping them in their own section means non-Anthropic provider crates have no semantic path to read them; the field name `anthropic_knobs` makes the constraint visible at every read site.

## 17. Cross-Doc Updates Required After Acceptance

| Doc | Update |
|---|---|
| `docs/coco-rs/CLAUDE.md` | Add `prompt-cache-design.md` to the File Index table; add to Document Map under "Prompt cache feature design" → owner. The Document Map already lists `crate-coco-inference.md` as owner of `CacheBreakDetector` / `CacheScope`; this doc joins as owner of placement + beta + policy. |
| `docs/coco-rs/crate-coco-types.md` | Document 5 new `Capability` variants (`PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`, `TokenEfficientTools`) in the canonical `Capability` enum section; document new types `PromptCacheConfig`, `BetaCapability`, `AccountKind` (only `ApiKey`/`ClaudeAiSubscriber`; Bedrock variant deferred §2), `CacheTtl`, `CacheScope`, `PromptCacheMode`. Note that the *adapter-side* mirror types (`Adapter*`, `AnthropicModelCapabilities`, `ProviderTopology`) are owned by `vercel-ai-anthropic` and are not coco-types — they exist precisely because `vercel-ai-anthropic` cannot import `coco-types` (F8). Note that `UserType` and `Entrypoint` are NOT consumed by this design — but they remain actively consumed elsewhere in the tree (skills, permissions); no cleanup proposed (R3-F7). |
| `docs/coco-rs/crate-coco-inference.md` | Add subsection for new `cache_convert` module (pass-through emission); document detector hash refactor (Finding 5 fix); add `supports_prompt_cache()` to capability-gate section. **Do NOT** add `cache_policy` or `beta_resolver` modules — those live in `vercel-ai-anthropic`. |
| `docs/coco-rs/crate-coco-config.md` | Document `RuntimeConfig.account` field (`AccountKind`, `in_overage`); document `prompt_cache.allowlist` config slot. No `Entrypoint` field — see §7.1. |
| `coco-rs/vercel-ai/anthropic/CLAUDE.md` | Document new typed **per-call** fields on `AnthropicProviderOptions` (`cache_strategy`, `requested_betas`, `agentic_query`, `query_source` — 4 fields, no `account_kind`/`in_overage` per R3-F3, no `user_type`/`entrypoint`); document new **session-stable** fields on `AnthropicConfig` (`capabilities: AnthropicModelCapabilities`, `provider_topology: ProviderTopology`, `experimental_betas_enabled: bool`, `disable_interleaved_thinking: bool`, `show_thinking_summaries: bool`, `non_interactive: bool`, `prompt_cache_allowlist: Vec<String>`, `account_kind: AdapterAccountKind` (R3-F3), `in_overage: bool` (R3-F3) — Findings F1, F5, R3-F3); document `INTERNAL_ANTHROPIC_OPTION_KEYS` deny-list (Finding F2, **4 entries**); document the seven adapter-side mirror types (`AnthropicModelCapabilities`, `ProviderTopology`, `AdapterCacheMode`, `AdapterCacheTtl`, `AdapterCacheScope`, `AdapterAccountKind`, `AdapterBetaCapability`) and the F8 invariant that `vercel-ai-anthropic` imports zero `coco_*`; document new modules `cache_policy`, `beta_resolver` (incl. `should_emit_context_management` shared predicate per R3-F2), `cache_placement`, `beta_capabilities`; document `OnceLock` memoization on `AnthropicMessagesLanguageModel`. |
| `docs/coco-rs/multi-provider-plan.md` | Reference this doc from the prompt-cache section instead of duplicating. |
| `docs/coco-rs/audit-gaps.md` | Mark prompt-cache gap as resolved (was OPEN). |

## 18. Glossary

- **Family gate** — `match` on `ProviderApi` enum; coarse "this wire shape supports the feature".
- **Capability gate** — `ModelInfo.capabilities.contains(...)`; fine-grained per-model.
- **Memoization** — capability-derived betas computed once per `AnthropicMessagesLanguageModel` lifecycle; mirrors TS `memoize(model => getAllModelBetas)`. Lives in the adapter, not in `ApiClient`.
- **Latch** — eligibility (`OnceLock<bool>`) and allowlist (`OnceLock<Vec<String>>`) frozen on first request inside `CachePolicy`; mirrors TS `setPromptCache1hEligible` + `setPromptCache1hAllowlist`. **The final `CacheTtl` is NOT latched** — the per-`querySource` allowlist match runs every call (TS `claude.ts:393-433`).
- **Marker / breakpoint** — synonyms; an Anthropic `cache_control: { type: "ephemeral", ... }` JSON object attached to a content block, system block, or tool definition.
- **`skipCacheWrite`** — TS flag that shifts the message-level marker from `[N-1]` to `[N-2]`; used for fire-and-forget queries (e.g., title generation) so the main thread's cache prefix isn't disturbed.
- **`cacheScope`** — TS per-block scope hint on system blocks. `'global'` requires the `prompt-caching-scope-2026-01-05` beta and is firstParty-only; `'org'` is the default; `null` means "do not cache this block".
- **No-model-string-match invariant** — coco-rs source code does not pattern-match on model name strings (`is_haiku`, `claude-3-*`, etc.) for capability or beta gating. All model-conditional logic goes through `ModelInfo.capabilities` (declarative property statements) or per-call flags (`agentic_query`, `query_source`, etc.). Tested via the `forbidden_strings_NEVER_appear_in_source` meta test in §14.2.
- **No-Ant-awareness invariant** — coco-rs deliberately does not consume `UserType::Ant` or `Entrypoint` for this feature. Anthropic-internal experimental flags (`cli-internal-2026-02-09`, `summarize-connector-text-2025-08-22`) are not emitted from this crate, regardless of env vars or settings. The `redact-thinking-2026-02-12` beta is gated on the model's `InterleavedThinking` capability + `provider_topology == FirstParty` + interactive — TS-faithful (`betas.ts:268-277`), not Ant-gated.
- **`querySource`** — TS string identifier (`'repl_main_thread'`, `'sdk'`, `'hook_agent'`, `'agent:*'`, `'compact'`) used both for cache-break tracking keys and for 1h-TTL allowlist matching.

## 19. Self-Review (Reviewer-Mode Attack Pass)

After applying the 7 third-party findings, I re-attacked the design as a hostile reviewer and uncovered 5 secondary issues that the patches introduced or exposed. Each is now fixed in §10.0 / §10.4 above; this section documents the attacks and the resolutions for traceability.

| # | Attack | Finding | Resolution |
|---|---|---|---|
| A | "If `Capability::PromptCachingScope` isn't a model capability, where does the beta header come from when `cache_strategy.scope == Global`?" | Original §10.4 wired RedactThinking to topology but did not emit `prompt-caching-scope-2026-01-05` based on resolved scope. Wire body would carry `"scope": "global"` without the matching beta header — Anthropic would reject it. | §10.4 now derives `resolved_scope` from `cache_strategy.scope` filtered through topology, and conditionally inserts `BetaCapability::PromptCachingScope` into the betas set. |
| B | "If `experimental_betas_enabled = false`, but the user supplied `cache_strategy.scope = Global`, what reaches the wire?" | `cache_placement::build_cache_control_value` would emit `"scope": "global"` regardless of beta state. Without the matching `prompt-caching-scope-2026-01-05` header, Anthropic would 400. | §10.4 downgrades `cache_strategy.scope == Global` to `None` when `experimental_betas_enabled == false` (and, in a future Bedrock iteration, when `provider_topology != FirstParty`). The downgraded scope is what feeds `build_cache_control_value`. |
| C | "How does `cache_policy.resolve_ttl` read the per-session allowlist if it lives on `AnthropicConfig`?" | Earlier drafts hand-waved a `load_allowlist` closure or env-var read inside `resolve_ttl`. `AnthropicConfig` had no allowlist field, so the function had no source. | §10.0 adds `prompt_cache_allowlist: Vec<String>` as an explicit field on `AnthropicConfig`, populated by the provider factory from `RuntimeConfig.global.prompt_cache.allowlist`. §10.4 passes a `&[String]` slice into `resolve_ttl` as an explicit parameter (Finding F5). |
| D | "How does `get_args` actually pass the marker strategy down to `convert_to_anthropic_messages_full`?" | §10.2 said the converter takes a new `auto_marker` parameter, but §10.4 didn't show where that strategy is constructed or threaded. | §10.4 now constructs `cache_placement::CacheMarkerStrategy { ttl: resolved_ttl, scope: resolved_scope, skip_cache_write }` immediately after TTL resolution, and explicitly comments that this value is passed as the converter's 5th argument. |
| E | "When `ModelInfo.capabilities` is `None` (unknown model, partial entry), what does the adapter do?" | `unwrap_or_default()` produces an empty Vec; no capability betas emit; auto-marker silently no-ops. Was this intentional, and is it documented? | §10.0 now explicitly states this is the conservative-by-design path: empty capabilities = no caching/no thinking betas, mirroring "unknown model gets safe defaults". Users opt in by declaring capabilities in `~/.coco/models.json`. |

The structural choices that survived attack:
- **Two-axis gate (`ProviderApi` AND `Capability`)** — a single-axis gate would either over-broadly enable caching for any Anthropic model or require model-string matching.
- **Adapter owns policy** — moving policy back into `services/inference` would re-violate `services/inference/CLAUDE.md:3` and require importing TS-internal-only types into a multi-provider crate.
- **Eligibility/allowlist latched, not the final TTL** — latching the TTL was the original v1 bug and stayed fixed.
- **Beta resolution memoized per `AnthropicMessagesLanguageModel`** — moving memoization into a free `static memoize!` macro would not match the lifecycle of hot-reload (which rebuilds the language-model struct).
- **Build call options once per `query()`** — the pre-Finding-3 design recomputed inside the retry loop, a class of bug that's hard to test for.

Where this design is still load-bearing on assumptions worth re-validating before merge:
1. **`ModelInfo.capabilities: Option<Vec<Capability>>` rather than `HashSet<Capability>`.** All `.contains()` calls are O(n) over the Vec; n ≤ ~10 today, so it's fine, but a future capability explosion may motivate the switch. (Out of scope for this design.)
2. **`extract_anthropic_options` keeps every key in raw for forward-compat.** The deny-list (Finding 2) is the right local fix, but if the typed surface keeps growing, periodic audits should confirm the deny-list still covers every internal-only key.
3. **Provider factory is the only construction site for `AnthropicConfig`.** Tests that build `AnthropicConfig` directly (skipping the factory) must remember to set the full set of new fields: `capabilities`, `provider_topology`, `experimental_betas_enabled`, `disable_interleaved_thinking`, `show_thinking_summaries`, `non_interactive`, `prompt_cache_allowlist`, **`account_kind`** (R3-F3), **`in_overage`** (R3-F3) — recommend providing a `Default` impl with safe defaults (empty caps, FirstParty, true, false, false, false, empty Vec, ApiKey, false) so test code stays terse.

## 19.2 Second Reviewer Pass — Layer-Rule Audit

A second attack pass focused exclusively on dependency-graph compliance uncovered **7 additional findings (F8–F14)** that invalidated parts of the §10 design as written. The single root cause: `vercel-ai-anthropic`'s `Cargo.toml` lists no `coco-*` dependency, and `coco-rs/CLAUDE.md` codifies that as a layer invariant (`L0  vercel-ai/* (8) — no internal deps`). Every `coco_types::*` field this design proposed for adapter-owned structs (`AnthropicConfig`, `AnthropicProviderOptions`, `CachePolicy::resolve_ttl`, `beta_resolver::resolve`) violated that rule.

### F8 (CRITICAL) — Layer-rule violation: vercel-ai-anthropic cannot import coco-types

| Location | Violating field |
|---|---|
| §10.0 `AnthropicConfig` | `capabilities: Vec<coco_types::Capability>` |
| §10.1 `AnthropicProviderOptions` | `requested_betas: Option<Vec<coco_types::BetaCapability>>` |
| §10.1 `AnthropicProviderOptions` | `account_kind: Option<coco_types::AccountKind>` |
| §10.1 `CacheStrategy` | `mode: coco_types::PromptCacheMode`, `ttl: coco_types::CacheTtl`, `scope: Option<coco_types::CacheScope>` |
| §10.1a `CachePolicy::resolve_ttl` | `account: AccountKind`, `requested: CacheTtl` (both from coco_types) |
| §10.1b `beta_resolver::resolve` | `caps: &ModelInfoCapabilities` (typed alias for coco_types Capability set) |

**Verified by:**
- `coco-rs/vercel-ai/anthropic/Cargo.toml` — only `vercel-ai-provider`, `vercel-ai-provider-utils`, and external crates; **no `coco-*`**.
- `grep -rn "coco_types\|coco_config" coco-rs/vercel-ai/` — single match in `vercel-ai/ai/src/generate_text/callback.rs:27` is a *doc comment string*, not an `use` statement; no other adapter touches coco-types.
- `coco-rs/CLAUDE.md` Dependency Layer Rules: `L0  vercel-ai/* (8) — no internal deps` (line 191).

**Fix pattern — mirror `ThinkingLevel ↔ ThinkingConfig`** (verified in `services/inference/src/thinking_convert.rs:17-89`): coco-types owns the cross-crate type; the adapter defines a *structurally-equivalent* local type; the boundary is JSON. The existing `AnthropicProviderOptions::thinking: Option<ThinkingConfig>` proves the precedent — `ThinkingConfig` is locally defined in `vercel-ai-anthropic` (line 8-23 of `anthropic_messages_options.rs`), `coco_types::ThinkingLevel` is converted to a JSON map by `thinking_convert::to_extra_body`, and `extract_anthropic_options` deserializes via serde into the adapter-side type. The two ends never share a Rust type.

The same pattern applies for prompt cache: every coco-types field above gets an **adapter-side mirror enum/struct** with identical wire JSON, defined inside `vercel-ai-anthropic`. §10.0 / §10.1 / §10.1a / §10.1b are rewritten below to use these adapter-side types.

### F9 — `build_anthropic` lives in `services/inference`, not `app/cli`

§10.0 sketch points to `app/cli/src/model_factory.rs::build_language_model_from_runtime`. **Verified actual location** at `services/inference/src/model_factory.rs:97-124` (`build_language_model_from_runtime`) which dispatches to `:196-217` (`build_anthropic`). `app/cli/` has no `model_factory.rs`. Capability translation must happen in `services/inference`, which is allowed to import both `coco-types` (L1) and `vercel-ai-anthropic` (L0) — it's the only crate sitting on both sides of the boundary.

### F10 — `AnthropicProviderSettings` is the actual injection vehicle

`AnthropicConfig` is built by `AnthropicProvider::make_config()` (`anthropic_provider.rs:116-126`) from `AnthropicProviderSettings` (lines 17-42). Threading new fields onto `AnthropicConfig` alone is insufficient — they must also be added to `AnthropicProviderSettings`, propagated through `AnthropicProvider::new()` storage, and re-emitted by `make_config()`. That's three Rust definitions per new field, not one.

### F11 — `make_config()` rebuilds `Arc<AnthropicConfig>` per `language_model()` call

`anthropic_provider.rs:129` (`pub fn messages(&self, model_id: &str)`) calls `make_config()` every time, so each `AnthropicMessagesLanguageModel` instance gets a *fresh* `Arc<AnthropicConfig>`. Multiple calls to `provider.language_model("claude-sonnet-4-6")` produce **separate `OnceLock<ResolvedBetas>`** state — defeating the memoization §5.2 / §10.4 promised. TS's `memoize(model => ...)` is a process-global cache keyed by model_id; coco-rs's `OnceLock` on the language-model instance is per-instance. The behavioral mismatch is invisible for callers that hold one `Arc<dyn LanguageModelV4>` for the session (the common case via `ApiClient`), but it's load-bearing for any caller that constructs multiple instances of the same model.

**Resolution:** acceptable for the primary `ApiClient` flow (one instance per `ProviderClientFingerprint`, hot-reload rebuilds both). Documented as a known divergence — call out in §10.4 and add a regression test that asserts memoization within a single `AnthropicMessagesLanguageModel` lifetime, not across instances.

### F12 — `ProviderClientOptions` has no `bedrock` / `topology` field

§10.0 sketch reads `provider_cfg.client_options.bedrock`. **Verified absent**: `grep -rn "bedrock\|Bedrock" coco-rs/common/config/src/provider/` returns zero hits. `ProviderClientOptions` (`client_options.rs:72-87`) has only `headers`, `auth_token`, `organization_id`, `project_id`, `include_usage`, `full_url`, `supports_structured_outputs`. There is no source of truth for whether a given Anthropic instance is firstParty vs. Bedrock vs. proxy.

**Resolution:** for this iteration, the provider factory just constructs `ProviderTopology::FirstParty` directly in `build_anthropic` — there is no `derive_topology` helper, since the only valid value today is the literal. When Bedrock auth lands (currently a Non-Goal §2), that PR adds a topology field to `ProviderClientOptions` (or a parallel `ProviderTopology` field on `ProviderConfig`), introduces `derive_topology(&ProviderConfig) -> ProviderTopology`, and the call site changes from a literal to a function call — three lines, no churn elsewhere.

### F13 — Capability data must thread through three crates without sharing types

For `AnthropicConfig.capabilities` to be populated:

```
coco-config::ResolvedModel.info.capabilities: Option<Vec<Capability>>      [coco-types L1]
            │
            │  reads in services/inference (allowed: L2 imports L1)
            ▼
services/inference::build_anthropic translates →
            │
            │  passes adapter-side type as field of AnthropicProviderSettings
            ▼
vercel-ai-anthropic::AnthropicProviderSettings.capabilities: AnthropicModelCapabilities    [adapter-side]
            │
            │  stored on AnthropicProvider, copied into make_config()
            ▼
vercel-ai-anthropic::AnthropicConfig.capabilities: AnthropicModelCapabilities             [adapter-side]
```

The crossing point is `services/inference::build_anthropic` — it's the only place `coco_types::Capability` and `AnthropicModelCapabilities` co-exist. Translation is a small `From<&[Capability]>` impl on the adapter-side type **defined in `services/inference`** (since the adapter cannot import `coco-types` for the source), or equivalently a free function in `services/inference::cache_convert`.

Field shape choice (compared in §10.0): a **bool struct** (`prompt_cache: bool`, `context_1m: bool`, …) wins over a string list (typo-prone) or a parallel adapter enum (introduces type duplication for nothing). The adapter-side struct is small and has the same shape as the existing typed bools on `AnthropicConfig` (`supports_native_structured_output`, `supports_strict_tools`).

### F14 — `build_anthropic` doesn't currently see `model_info`

`build_anthropic(provider_cfg, api_model, timeout_secs)` at `model_factory.rs:196` takes 3 params, none of which is `model_info`. The caller `build_language_model_from_runtime:97-124` already has `model_info` resolved at line 110-113 — needs to be passed through. Trivial signature change; flagged because §10.0 implicitly assumed the data was already in scope.

### Compounding implication — `AnthropicProviderOptions::requested_betas` likewise

The same pattern applies to `requested_betas`: callers in `services/inference::cache_convert` emit a camelCase wire key (e.g., `"requestedBetas": ["context_1m", "fast_mode"]`) as `Vec<String>` (or `Vec<AdapterBetaCapabilityWire>` deserialized from kebab- or snake_case strings). The adapter parses into `Option<Vec<AdapterBetaCapability>>`. Conversion happens once at the boundary; coco-types `BetaCapability` is never imported into `vercel-ai-anthropic`.

---

The §10 sections that follow have been rewritten to honor F8 strictly. Every field of every adapter-owned struct is either a primitive, a `serde_json::Value`, or an adapter-locally-defined type. A grep meta-test (`adapter_does_not_import_coco_types`) is added to §14.2 to prevent regressions:

```rust
// in vercel-ai-anthropic/src/lib.test.rs (or a build-script)
#[test]
fn adapter_does_not_import_coco_types() {
    let crate_src = include_str!("..");  // walk via build script
    for line in crate_src.lines().filter(|l| l.starts_with("use ")) {
        assert!(!line.contains("coco_"), "vercel-ai-anthropic must not import coco_*; found: {line}");
    }
}
```

## 19.3 Third Reviewer Pass — Round-3 Findings (R3-F1…F7) Self-Attack

After applying R3-F1…F7 (factory wiring, second context-mgmt emission point, TTL latch corruption, RuntimeConfig schema, detector hash claim narrowing, stale wording, UserType::Ant cleanup withdrawal), I re-attacked the post-fix design as a hostile reviewer. **Five attack vectors** survived a round of attention; each is closed in the body of the doc and recapped here for traceability.

| # | Attack | Resolution |
|---|---|---|
| α | "If `account_kind`/`in_overage` are session-stable on `AnthropicConfig`, what happens when subscription state genuinely changes mid-session (e.g., user upgrades from ApiKey to Subscriber)?" | The runtime layer already rebuilds on settings reload (`SettingsWatcher` → `RuntimeConfigBuilder` → new `RuntimeConfig` → new `ApiClient` → new `AnthropicMessagesLanguageModel`). The fresh language-model gets a fresh `OnceLock`, so the new account_kind takes effect immediately on the *next* call. The in-flight latch on the *previous* instance is acceptable — it's the "TS-parity by-construction" reading: TS also doesn't re-derive eligibility mid-session because subscription transitions are themselves rare. The `account_kind_is_session_stable_not_per_call` test (§14.2) covers this. |
| β | "If `experimental_betas` defaults to `true` and a deployment forgets to set the env var to `false` for sensitive prod traffic, do we leak Anthropic-internal experimental betas?" | The first-party-only betas this design emits (`redact-thinking-2026-02-12`, `prompt-caching-scope-2026-01-05`, `context-management-2025-06-27`) are **published Anthropic public betas**, not Anthropic-internal flags. The Anthropic-internal-only ones (`cli-internal-2026-02-09`, `summarize-connector-text-2025-08-22`) are NEVER emitted from this crate (§7.1, §3.5). The default-true is right for any first-party Anthropic API user; non-first-party topologies (Bedrock when added) hard-suppress the gate regardless of `experimental_betas`. |
| γ | "If the runtime is rebuilt for an unrelated config change (say a new MCP server added), does the in-flight cache state survive?" | Yes, with a caveat: the `ProviderClientFingerprint` (multi-provider-plan §11.1) is computed from the resolved `ProviderConfig` only; settings unrelated to the Anthropic provider don't change the fingerprint. So the cached `Arc<ApiClient>` is reused, and the per-`AnthropicMessagesLanguageModel` `OnceLock` state persists. Caveat: changes to `runtime.account.*`, `runtime.prompt_cache.*`, or `runtime.anthropic_knobs.*` SHOULD invalidate the cached client. R3-F4 step 7 must extend `ProviderClientFingerprint::compute` to hash these three new sections — added to the §15 step 7 deliverable. The fingerprint test in `coco-inference` already runs over the full `ProviderConfig`; the same approach applies here, with three additional inputs. |
| δ | "Two emission sites for context-management share `should_emit_context_management`. What if a third future site (e.g., a hypothetical advisor-tool that also wants the beta) forgets to call it?" | The `forbidden_strings_NEVER_appear_in_source` meta test in §14.2 is scoped to the new prompt-cache modules; extend it (or add a new meta test) to assert the literal `"context-management-2025-06-27"` string appears **only** through `beta_capabilities::map_capability(AdapterBetaCapability::ContextManagement)` and NOT inline in any other call site. This is mechanically grep-able: `grep -rn "context-management-2025-06-27" coco-rs/vercel-ai/anthropic/src/` should return at most one hit (the constant table); any other line is a regression. Captured as test `context_management_string_centralized_in_beta_capabilities`. |
| ε | "The R3-F4 schema adds 5 new env vars (`COCO_PROMPT_CACHE_ALLOWLIST`, `COCO_ANTHROPIC_EXPERIMENTAL_BETAS`, `COCO_ANTHROPIC_DISABLE_INTERLEAVED_THINKING`, `COCO_ANTHROPIC_SHOW_THINKING_SUMMARIES`, `COCO_ANTHROPIC_NON_INTERACTIVE`). Are any of these duplicating existing env vars in `coco_config::EnvKey`?" | Verified via grep on `coco-rs/common/config/src/env/keys.rs`: none of the proposed names collide. `non_interactive` is the closest existing concept (other crates derive it from `!is_tty()` ad-hoc); the R3-F4 design takes that derivation as the **default** when the env var is unset, so the `COCO_ANTHROPIC_NON_INTERACTIVE` knob is the override path, not a re-implementation. Documented in §16a.3. |

The structural choices that survived attack (post-R3):

- **Two surfaces for adapter input** — per-call `AnthropicProviderOptions` for things that genuinely vary turn-to-turn (`cache_strategy`, `agentic_query`, `query_source`, `requested_betas`); session-stable `AnthropicConfig` for things that latch (`account_kind`, `in_overage`, `capabilities`, topology, env knobs). The 4-vs-9 split is now load-bearing for both R3-F3 (latch correctness) and the F11 memoization story.
- **Shared resolver predicates for cross-site betas** — `should_emit_context_management(&config)` is the unique gate function. Both call sites import it; a meta test prevents drift.
- **`build_anthropic` reads `runtime` directly** — passing the whole struct (rather than threading 5 separate scalars) keeps the call site terse and means future RuntimeConfig additions (Bedrock auth, etc.) don't churn the signature.
- **No env-var reads inside `vercel-ai-anthropic`** — the adapter only reads its own struct fields. All env-var resolution happens in `coco_config::build_runtime_config`. Mirrors the existing `compact: CompactConfig` pattern.

Where this design is still load-bearing on assumptions worth re-validating before merge (post-R3 additions):

4. **`ProviderClientFingerprint` extension.** Adding `runtime.account` / `runtime.prompt_cache` / `runtime.anthropic_knobs` to the fingerprint hash means a settings reload that changes any of them invalidates the cached client. Expected, but verify the existing fingerprint test covers the cardinality (5 new fields × 2 settings layers × env-or-not = ~30 path combinations to cover; lean on property-based testing if the matrix grows further).
5. **Default `experimental_betas: true`.** Reasonable for first-party API users, but a deployment that wants conservative behavior should know to set `COCO_ANTHROPIC_EXPERIMENTAL_BETAS=false`. Document this prominently in `crate-coco-config.md` rather than burying in §16a.3.
