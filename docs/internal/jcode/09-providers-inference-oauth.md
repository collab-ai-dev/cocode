# Providers / Inference / OAuth / Multi-Account: jcode vs coco-rs

Source-level comparison of how each harness reaches an LLM: provider abstraction,
credential resolution, OAuth, multi-account handling, and failover. All claims
below are verified against source; file:line references are to the trees at
`/lyz/codespace/3rd/jcode` (jcode) and `/lyz/codespace/codex/coco-rs` (coco-rs).

The two projects sit at opposite ends of a spectrum. jcode **owns the entire
request/auth/transport path itself** — there is no third-party SDK; every
provider is a hand-written transport behind one fat trait, backed by a large
static catalog. coco-rs is a **faithful port of the Vercel AI SDK** with a
deliberately thin, provider-agnostic wrapper. The differences below mostly trace
back to that single architectural fork, plus coco-rs's documented non-goals.

---

## jcode approach

**One fat `Provider` trait + a `MultiProvider` dispatcher.**
`crates/jcode-provider-core/src/lib.rs:48` defines `Provider` — an object-safe,
~40-method `async` trait. Beyond `complete()` / `complete_split()` it carries
provider *capability* knobs as trait methods directly: `reasoning_effort` /
`set_reasoning_effort`, `service_tier` / `available_service_tiers`, `transport` /
`set_transport`, `premium_mode` (Copilot request conservation,
`PremiumMode::{Normal, OnePerSession, Zero}` at `lib.rs:322-328`),
`native_compaction_mode` / `native_compact` (provider-side compaction),
`handles_tools_internally`, `model_routes()` (a unified picker), `fork()`
(independent mutable clone), and `native_result_sender` (provider-delegated tool
execution, `NativeToolResult` at `lib.rs:333-375`). `complete_split(system_static,
system_dynamic)` (`lib.rs:61-72`) is an explicit cache seam: the static prefix
stays cacheable, dynamic context is injected as a trailing message via
`messages_with_dynamic_system_context`.

**Stringly-typed dispatch over 14 credential routes.** A single `MultiProvider`
holds one `Arc<dyn Provider>` per `ActiveProvider` variant — `Claude`, `OpenAI`,
`Copilot`, `Antigravity`, `Gemini`, `Cursor`, `Bedrock`, `OpenRouter`
(`selection.rs`). `ModelRouteApiMethod` (`lib.rs:438-549`) enumerates **14
distinct credential/transport routes** (`ClaudeOAuth`, `AnthropicApiKey`,
`OpenAIOAuth`, `OpenAIApiKey`, `OpenRouter`, `OpenAiCompatible { profile_id }`,
`Copilot`, `Cursor`, `Bedrock`, `CodeAssistOAuth`, `AntigravityHttps`,
`RemoteCatalog`, `Current`, `Other`). The wire form is a string; `ModelRouteApiMethod::parse`
(`lib.rs:457-485`) re-parses it at module boundaries — saved catalogs round-trip
as strings, and routing decodes them per call.

**Provider catalog breadth (README "40+" — VERIFIED).**
`crates/jcode-provider-metadata/src/catalog.rs` declares **exactly 32
`OpenAiCompatibleProfile` consts** (`grep -c` = 32: opencode, opencode-go, zai,
kimi, 302ai, deepseek, groq, mistral, perplexity, togetherai, deepinfra,
fireworks, minimax, xai, lmstudio, ollama, chutes, cerebras, nvidia-nim,
huggingface, nebius, scaleway, stackit, baseten, cortecs, …) plus **46
`LoginProviderDescriptor` entries** spanning CLI/TUI/server/auto-init/auth-status
surfaces. Each profile is a `const` struct `{id, display_name, api_base,
api_key_env, env_file, setup_url, default_model, requires_api_key}` — e.g.
`OPENCODE_PROFILE` at `catalog.rs:6-15`. `src/provider_catalog.rs` adds per-profile
static model lists (`:217-406`), key-based endpoint switching (MiniMax `sk-cp-*`
→ China base), localhost no-auth detection, and `provider add` named-profile
config writing `[providers.X]` into `~/.jcode/config.toml`.

**Claude.ai-subscription OAuth as a first-class inference path (VERIFIED, deep).**
This is the part most consequential for a fair comparison, and it is implemented
end-to-end, not just documented. The wire contract is **defined** in
`crates/jcode-provider-core/src/anthropic.rs`:
- Reversible tool-name remap (`anthropic_map_tool_name_for_oauth` /
  `_from_oauth`, `:32-63`): `bash→Bash, read→Read, write→Write, edit→Edit,
  glob→Glob, grep→Grep, subagent→Agent, schedule→ScheduleWakeup,
  skill_manage→Skill`.
- The OAuth beta header (`ANTHROPIC_OAUTH_BETA_HEADERS`, `:2`) is an 8-beta
  string — `claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,
  context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,
  advanced-tool-use-2025-11-20,effort-2025-11-24` — with a `[1m]`-suffix variant
  adding `context-1m-2025-08-07` (`:5`, `anthropic_oauth_beta_headers` at `:24-30`).
- Stainless client fingerprint headers (`anthropic_stainless_arch` / `_os`, `:65-80`).

And the contract is **applied** at the transport in `src/provider/anthropic.rs`:
`API_URL_OAUTH = "https://api.anthropic.com/v1/messages?beta=true"` (`:45`),
`CLAUDE_CLI_USER_AGENT = "claude-cli/2.1.123 (external, sdk-cli)"` (`:48`), an
`is_oauth` flag threaded through `format_messages` / `format_content_blocks` /
`format_tools` (`:776, :816, :935, :1020`) so tool names are remapped on the wire
and a hardcoded Claude-Code tool catalog (`Agent/Bash/Edit/Glob/Grep/Read`) is
substituted under OAuth, plus `apply_oauth_attribution_headers` (`:71`),
`format_messages_with_identity` (an identity system block, `:1194`), and
`temperature` pinned to `1.0` on the OAuth path when thinking is inactive (`:638-644`)
to match the Claude Code client fingerprint. Scope enforcement:
`claude_scopes_have_inference` (`src/auth/oauth.rs:70-94`) rejects tokens minted
from the console authorize endpoint that lack `user:inference`.

**Session-warmup OAuth preflight.** Before the first inference call jcode runs an
`AtomicBool`-gated handshake (`ensure_oauth_preflight`, `src/provider/anthropic.rs:244-366`,
`oauth_preflight_done` flag at `:451`): GETs to
`/api/claude_cli/bootstrap`, `/api/oauth/account/settings`, `/api/claude_code_grove`
and a POST to `/api/eval/...`, all carrying the `oauth-2025-04-20` beta header and
the `claude-cli` UA. It fires once per session.

**Mid-request OAuth token refresh.** `get_oauth_access_token`
(`src/provider/anthropic.rs:668`) refreshes the Claude token when within ~5 min of
expiry and re-checks `claude_scopes_have_inference` on the refreshed credential
(`:686`); `refresh_claude_tokens_for_account` (`src/auth/oauth.rs:1132`) is the
executor.

**OpenAI / Codex routing (VERIFIED).** `src/provider/openai.rs` switches base URL
on subscription presence: `is_chatgpt_mode` (`:671`) is true when `refresh_token`
is non-empty or `id_token` is present → `CHATGPT_API_BASE =
"https://chatgpt.com/backend-api/codex"` (`:29`) with `originator: codex_cli_rs` +
`chatgpt-account-id` header; otherwise the public OpenAI Responses endpoint. Uses
the Responses API throughout.

**Multi-account (`/account`).** Per-provider account files store
`Vec<{label, access_token, refresh_token, id_token, account_id, expires_at, email}>`
(`src/auth/claude.rs` — `AnthropicAccount` struct `:49`, `list_accounts:242`,
`active_account_label:248`, `set_active_account:259`, `upsert_account:274`;
`src/auth/codex.rs` for OpenAI). Generic logic in `src/auth/account_store.rs`
handles canonical relabeling (`claude-1`, `claude-2`), active-account override,
upsert, and next-label. `refresh_state.rs` persists per-provider
last-attempt/success/error to `auth-refresh-state.json`.

**Cross-provider + cross-account failover (the headline differentiator).**
`failover.rs::classify_failover_error_message` (`:69-137`) maps an error string to
`FailoverDecision::{None, RetryNextProvider, RetryAndMarkUnavailable}`:
context/413 → retry next provider but **don't** sideline; rate/quota/credit/billing/429/402
→ retry **and** mark the credential unavailable; auth/401/403/forbidden/unauthorized
→ retry **and** mark unavailable; default → terminal. `contains_independent_status_code`
(`:57-67`) avoids matching digits inside other numbers (tested: "model version 4130"
≠ 413, `:176`). The loop (`src/provider/mod.rs:405-465`) walks
`selection::fallback_sequence(active)`, and **before** crossing providers it tries
same-provider account rotation (`account_failover.rs`): `same_provider_account_candidates`
(`:42-97`) ranks the other accounts of the same provider by `max(five_hour_ratio,
seven_day_ratio)`, skipping exhausted/errored ones. A structured
`ProviderFailoverPrompt` (`failover.rs:5-30`) carries `from/to` provider+label,
estimated input chars/tokens, and reason so the UI can explain the switch.

**Live usage probes.** `src/usage/provider_fetch.rs` fetches real plan windows per
account: Anthropic 5h/7d rate windows (`fetch_anthropic_usage_for_token:17`,
refreshes the token if within 5 min of expiry), OpenAI usage windows, OpenRouter
`/key` + `/credits` (`:260`), Copilot quotas (`:379-497`). `usage/model.rs` exposes
`AccountUsageProbe` (`current_exhausted`, `best_available_alternative`,
`all_accounts_exhausted`, `switch_guidance:320-374`) consumed by failover.

**Stub note.** `src/subscription_catalog.rs:8` pins `DEFAULT_JCODE_API_BASE =
"https://subscription.jcode.invalid/v1"` and the curated list pins to a `Stealth`
upstream "until a cache-capable route exists" (`:64`). The "jcode subscription"
router product is a placeholder, not a shipping billing backend.

---

## coco-rs approach

**Faithful Vercel AI SDK port behind a thin generic wrapper.** Concrete providers
live in standalone L0 crates `vercel-ai-{anthropic,openai,openai-compatible,
google,bytedance}`, each matching `@ai-sdk/*` v4 with no `coco-*` dependency.
`services/inference` is *deliberately* provider-agnostic: it holds an
`Arc<dyn LanguageModelV4>` and owns only generic retry, usage aggregation,
cache-break detection, and thinking-level conversion (`services/inference/CLAUDE.md`).
Provider concerns — OAuth, betas, prompt-cache, 529 retry, rate-limit policy —
are kept inside the per-provider crates by design.

**Single binding point, exhaustive match.**
`services/inference/src/model_factory.rs::build_language_model_from_runtime`
(`:100-132`) matches exhaustively over the four `ProviderApi` arms — `Anthropic`,
`Openai`, `Gemini` via direct SDKs; `Volcengine` / `Zai` / `OpenaiCompat` collapse
into one `build_openai_compat` arm with the runtime instance name as `provider_id`.
Adding a variant is a compile error. `warn_unused_client_options` (`:54-89`) flags
misapplied per-provider knobs (e.g. `auth_token` on a non-Anthropic instance).
`build_api_client` (`:145`) wires a per-slot `CacheBreakDetector` and a
`ProviderClientFingerprint` (`:162`) for turn-boundary hot-reload coherence.

**Builtin provider breadth: 7.** `common/config/src/builtin/` ships six vendor
modules — `anthropic`, `openai`, `google`, `volcengine` (ByteDance), `zai`,
`deepseek` — and `deepseek` expands to two provider instances (`deepseek-openai`
+ `deepseek-anthropic`, `builtin/deepseek.rs:1`), for **7 builtin providers**
total (`builtin/mod.rs:103-108`). Users add arbitrary OpenAI-compatible providers
by hand-editing `providers.<name>` in config (base_url + env_key + client_options
+ provider_options). The scope is the documented multi-LLM target: Anthropic
FirstParty, OpenAI, Gemini, ByteDance, generic OpenAI-compatible.

**Auth.** `services/inference/src/auth.rs` resolves
`AuthMethod::{ApiKey, OAuth(OAuthTokens), Bedrock, Vertex, Foundry}` from env +
config with a documented priority chain (`ANTHROPIC_AUTH_TOKEN` →
`ANTHROPIC_API_KEY` → api_key_helper → cloud env → stored OAuth, `resolve_auth:127-178`).
`OAuthTokens` carries `{access_token, refresh_token, expires_at, subscription_type,
org_uuid}` with `is_expired` / `needs_refresh` (5-min skew, `:72-82`) and atomic
file persistence (`save_oauth_tokens:230`). `get_api_key_from_helper` runs a shell
command with a 5-min in-process cache. The Bedrock/Vertex/Foundry arms exist for
env *detection only* — `model_factory` never dispatches on them, per the documented
non-goal. **There is no interactive OAuth login flow for LLM providers** (no PKCE,
no loopback callback, no device flow), **no multi-account**, and — verified —
**no token-refresh executor**: `grep` of `auth.rs` finds `refresh_token` only as a
struct field; there is no refresh endpoint, no `client.post`, no `oauth/token` call.
The `subscription_type` / `org_uuid` fields imply tokens can be *imported*, but
minting them and refreshing them is out of scope.

**Anthropic OAuth-adjacent machinery (faithful, but not the full Claude Code OAuth
contract).** `vercel-ai-anthropic` emits the `claude-code-20250219` baseline beta
gated on the per-call `agentic` flag (`beta_resolver.rs:65-67`,
`CLAUDE_CODE_BASELINE`), and a deterministic `BTreeSet`-sorted beta header driven
by capabilities + topology + knobs (`beta_resolver.rs:56-133`). `AdapterAccountKind::
{ApiKey, ClaudeAiSubscriber}` exists but its *only* behavioral effect is 1h-cache-TTL
eligibility (`cache_policy.rs:90-92`: ApiKey → always eligible, ClaudeAiSubscriber →
only when `in_overage`). Verified absent in `vercel-ai/anthropic/src/`:
`oauth-2025-04-20`, `claude-cli` UA, `?beta=true`, the identity system prepend, and
any SDK→Claude-Code tool-name remap. The `AdapterBetaCapability` enum
(`messages/anthropic_messages_options.rs:158-177`) has **no** `oauth-2025-04-20`
variant at all. (The "You are Claude Code…" string in coco-rs lives only as a doc
comment in `core/context/src/prompt.rs:41`, referring to the unconditional base
system prompt — it is not an OAuth-gated wire injection.)

**Retry / fallback.** `services/inference/src/retry.rs` is generic exponential
backoff + jitter honoring server `retry-after`. `ApiClient::query` retries the
**same** client (no cross-provider switch inside the client), records usage per
`ProviderModelSelection`, and runs the post-call cache-break check. Cross-tier
fallback lives one layer up in `app/query/src/model_runtime.rs`: `ModelRuntime`
holds `slots[0]=primary, slots[1..]=fallbacks`, `advance()` steps forward after
`MAX_CONSECUTIVE_CAPACITY_ERRORS` (3) consecutive capacity errors
(`engine.rs:546, :1355`), with a half-open `attempt_probe_if_due` /
`finalize_probe` recovery state machine that periodically probes primary with
monotonic backoff (`model_runtime.rs:189-302`). The trigger fires on
`InferenceError::Overloaded` (503/529) **OR** `InferenceError::RateLimited` (429)
**OR** an `is_capacity_error_message` substring match
(`engine.rs:1320-1324`, `engine_helpers.rs:53-63`). Context-overflow routes to
reactive compaction (`engine.rs:1303-1318`). 401/403 map to
`AuthenticationFailed` (terminal, `errors.rs:167-170`); 402 falls through to the
catch-all `ProviderError` (`errors.rs:185`) — neither triggers fallback. Fallback
slots may point at different providers (the chain is `ModelRole` fallback specs),
so this is functionally cross-provider failover when configured.

**Usage tracking.** `services/inference/src/usage.rs::UsageAccumulator` accumulates
`TokenUsage` keyed by `ProviderModelSelection`. `core/messages/src/cost.rs::CostTracker`
converts tokens → USD. coco-rs **does** capture 429 `retry-after` into
`ToolAppState.rate_limits` (`reset_at_ms` / `retry_after_seconds`,
`engine_helpers.rs:82-107`) but never probes provider-side plan/rate windows
(5h/7d, account exhaustion) — consistent with the documented non-goal of dropping
`services/claudeAiLimits.ts` / `services/policyLimits/` / `services/rateLimitMessages.ts`.

**MCP OAuth (where coco-rs has a real, complete flow).**
`services/rmcp-client/src/{oauth.rs,perform_oauth_login.rs}` implements full OAuth
for MCP servers: PKCE via `rmcp::AuthorizationManager`, keyring-or-file credential
store, `StoredOAuthTokens` with `expires_at` + 30 s refresh skew, atomic persistence,
and `auth_status`. This proves the LLM-side gap is a deliberate scope choice, not a
missing capability.

---

## Head-to-head comparison

| Dimension | jcode | coco-rs |
|---|---|---|
| Provider abstraction | One ~40-method `Provider` trait mixing transport + capability knobs + tool delegation (`lib.rs:48`) | `Arc<dyn LanguageModelV4>` from L0 SDK crates; thin provider-agnostic wrapper |
| Dispatch | 14 string-parsed `ModelRouteApiMethod` routes (`lib.rs:438`) | Exhaustive 4-variant `ProviderApi` match (`model_factory.rs:118`) — compile-checked |
| Builtin/preset providers | 32 OpenAI-compat presets + 46 login descriptors as `const` data | 7 builtins; arbitrary compat providers via hand-edited config |
| Claude.ai-subscription inference | First-class: tool remap + identity prepend + `oauth-2025-04-20` + UA + scope check + preflight + temp=1.0 | Not supported on the wire; only baseline `claude-code-20250219` beta; subscriber kind affects cache TTL only |
| OAuth token refresh | Mid-request refresh executor (`anthropic.rs:668`, `oauth.rs:1132`) | Models `needs_refresh()` but **no executor** — expired imported token just fails |
| Multi-account | `Vec<Account>` per provider, active-account override, relabel | One credential per provider instance (`ProviderConfig.api_key`) |
| Failover trigger | 3-family typed classifier (context / rate-billing-429-402 / auth-401-403) → 3 outcomes, with digit-boundary matcher | Capacity (529/503) + 429 + substring → advance model slot; 401/403/402 terminal |
| Credential sidelining | Marks provider-account unavailable, prefers others | None — `advance()` hops the whole model slot, no account concept |
| Live plan/usage probes | Real 5h/7d Anthropic, OpenAI, OpenRouter, Copilot per-account windows | None (token→USD only); does capture 429 reset_at_ms |
| MCP OAuth | n/a in this module | Complete PKCE + keyring + refresh (`rmcp-client`) |

**Where jcode is genuinely ahead and the mechanism matters:**

1. **Claude.ai-subscription as an inference path.** Without the tool remap +
   identity line + `oauth-2025-04-20` + `claude-cli` UA, the Messages API will not
   accept a Claude.ai OAuth bearer — a coco-rs user holding only a Max/Pro
   subscription cannot authenticate for inference; they need an API key. This is
   the single most consequential functional gap. It overlaps coco-rs's "no
   `services/oauth/`" non-goal only on the *login/minting* side; the *wire contract*
   itself unlocks an already-supported credential type (`AuthMethod::OAuth` import
   already exists at `auth.rs:44`).

2. **Multi-account + usage-aware rotation.** jcode keeps a session alive through
   per-account rate limits transparently by rotating to the same provider's next
   account ranked by live headroom *before* crossing providers
   (`account_failover.rs:42-97`). coco-rs surfaces the 429, or — if it matches the
   capacity substring — advances to a different *model slot*, never a different
   *account* of the same provider.

3. **Failover classification granularity.** jcode distinguishes 402/billing and
   401/auth and sidelines the bad credential; coco-rs treats 401/403 as terminal
   and has no "mark this credential unavailable" state.

**Where the gap is smaller than it looks:** jcode's 32 presets are mostly the same
generic OpenAI-compatible adapter with different `api_base` constants — the
engineering depth gap is far smaller than the count suggests. And jcode's
multi-account machinery is tied to its multi-session server/swarm story; coco-rs is
a single-user CLI mirroring Claude Code, where one-credential-per-provider is a
reasonable default rather than a deficiency.

**Perf/resource note.** jcode shares one `OnceLock<reqwest::Client>` across generic
providers (`lib.rs:384`, explicitly to avoid the ~10 ms TLS-init cost) and its
catalog is `const` data (zero alloc at rest). coco-rs builds a `reqwest::Client`
per provider instance but shares it across all calls to that provider and caches
`ApiClient` per role. Neither is materially heavier for this module. jcode's
usage-probe machinery adds periodic background HTTP that coco-rs does not do — a
feature, but also a per-session network cost.

---

## Where coco-rs already matches or wins

1. **Cleaner layering and SDK fidelity.** Provider concerns are isolated in
   standalone L0 `vercel-ai-*` crates with adapter-local mirror enums
   (`AdapterAccountKind`, `AdapterBetaCapability`) so the SDK can be upgraded
   without touching the agent loop; `services/inference` stays tool-agnostic and
   provider-agnostic. jcode's equivalent is the 40-method `Provider` trait
   (`lib.rs:48`) mixing transport, reasoning-effort, service-tier, premium-mode,
   native-compaction, transport-switching, and tool-execution-delegation into one
   object-safe trait — every new capability widens the trait and every provider
   must stub it. coco-rs's exhaustive 4-variant `ProviderApi` match is more
   maintainable than jcode's `ModelRouteApiMethod::parse` string dispatch
   (`lib.rs:457`).

2. **Typed prompt-cache-break detection — jcode has no equivalent.** coco-rs's
   per-slot `CacheBreakDetector` hashes the merged `extra_body`, attributes drops
   to TTL vs client-side change, exposes a suppression API for compaction/agent
   cleanup, and emits an OTel `coco_cache_break_total` counter
   (`services/inference/CLAUDE.md`). It even documents the PR #18143 incident
   (`effort: 'low'` dropped cache hit 92.7% → 61% by changing `budget_tokens`) and
   guards cache-shared forks against it (`app/query/CLAUDE.md`). jcode has
   `complete_split` for cache-*friendly prompt layout* but no break *detection* or
   telemetry.

3. **Deterministic, audited beta resolution.** coco-rs computes betas through one
   pure `resolve()` with a `BTreeSet` for byte-stable headers and a single
   `should_emit_context_management` predicate shared by every emission site
   (`beta_resolver.rs:56-133`, R3-F2 "half-emitted state is structurally
   impossible"). jcode uses two flat `const` header strings selected by `[1m]`
   suffix (`anthropic.rs:2-5`) — simpler, but not capability-gated per model, so a
   non-capable model can receive betas it does not support.

4. **Unified typed `StopReason` across all providers.** coco-rs maps every
   provider's finish reason to one 8-variant `UnifiedFinishReason` at the adapter
   seam, and `app/query` dispatches recovery on the enum (MaxTokens → output
   escalate; ContextWindowExceeded → reactive compaction) with zero wire-string
   parsing above the adapter (`app/query/CLAUDE.md`). This is cleaner than jcode's
   per-provider error-string scraping in `classify_failover_error_message`.

5. **Real OAuth where it is in scope (MCP).** The complete PKCE + keyring + refresh
   flow in `rmcp-client` shows the LLM-provider OAuth absence is a deliberate scope
   line, not an inability.

**jcode claims that do not fully hold in source:**

- **"jcode subscription" router product** — `subscription_catalog.rs:8` points at
  `https://subscription.jcode.invalid/v1` and pins to a `Stealth` upstream "until a
  cache-capable route exists" (`:64`). It is a placeholder/stub, not a shipping
  billing backend.
- **README perf headlines** (14 ms TTFF, 1000 fps, mermaid 1800×) are TUI/render
  claims with no bearing on this module; nothing in the provider/auth/usage code
  substantiates or relates to them.
- **Anthropic cloud routes** — jcode ships a `Bedrock` `ActiveProvider`; for coco-rs
  these are *explicit non-goals*, so jcode's breadth there is irrelevant to a fair
  comparison.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions whose adversarial verdict is **confirmed** or **nuanced** are
listed. Corrections from review are folded in.

### R1 (confirmed, HIGH impact / HIGH effort) — Implement the Claude Code OAuth wire contract so Claude.ai-subscription tokens work for inference

**Why.** jcode lets a Max/Pro subscriber drive inference with an OAuth token by
applying the full contract: reversible tool-name remap (`anthropic.rs:32-63`),
identity system prepend (`provider/anthropic.rs:1194`), the `oauth-2025-04-20` beta
(`anthropic.rs:2`), `claude-cli` UA + `?beta=true` (`provider/anthropic.rs:45-48`),
temperature pinned to 1.0 when thinking is off (`:638-644`), and a `user:inference`
scope gate (`auth/oauth.rs:70-94`). coco-rs emits only `claude-code-20250219`
(`beta_resolver.rs:65-67`), has no `oauth-2025-04-20` variant in
`AdapterBetaCapability`, no tool remap, no identity prepend, and no `claude-cli` UA
(all verified absent in `vercel-ai/anthropic/src/`). `AdapterAccountKind::
ClaudeAiSubscriber` affects only cache TTL (`cache_policy.rs:90-92`).

**Correction (from review).** coco-rs does **not** "reject OAuth tokens" — it has
no rejection check; it would send a non-conformant Bearer request that *Anthropic*
rejects with 401/400. The wire tool-name table must be derived from **coco-rs's own
canonical tool IDs**, which are already PascalCase (`Bash/Read/Edit/Grep/Glob`),
not copied from jcode's lowercase internal names — so for coco-rs the remap is
largely identity for core tools and needs entries only where a coco tool id
diverges from the OAuth allow-list.

**Concrete change.** In `vercel-ai-anthropic`, gate on
`AdapterAccountKind::ClaudeAiSubscriber` + an `auth_token` bearer present (both
already plumbed: subscriber kind at `prompt_cache_settings.rs`, threaded at
`model_factory.rs:225-228`; bearer at `anthropic_provider.rs:145-153`): (a) add an
`OauthContract` `AdapterBetaCapability` variant emitting `oauth-2025-04-20` in
`beta_resolver.rs`; (b) prepend the identity system block in
`convert_to_anthropic_messages` system assembly; (c) set the `claude-cli` UA and
`?beta=true` in the provider header closure / messages API URL; (d) remap + reverse
only the divergent tool names. Token **minting** stays out of scope (respects the
dropped `services/oauth/` non-goal); accept imported tokens via the existing
`AuthMethod::OAuth` path.

**Risk.** Touches the hottest wire path — a wrong remap breaks all tool calls.
**Non-goal note:** the *login UI* overlaps "no `services/oauth/`"; the *wire
contract* does not — it unlocks an already-supported credential type.

### R2 (confirmed, HIGH/HIGH, prerequisite for R1) — Add an OAuth token-refresh executor

**Why.** jcode auto-refreshes the Claude OAuth token mid-request when within ~5 min
of expiry and re-checks inference scope on the refreshed credential
(`provider/anthropic.rs:668-720` + `auth/oauth.rs:1132`). coco-rs models
`OAuthTokens::needs_refresh()` / `is_expired()` (`auth.rs:72-82`) but has **no
refresh executor and no refresh endpoint anywhere** (verified: `refresh_token` is a
struct field only). An expired imported token simply fails the request.

**Concrete change.** Add a refresh helper in `services/inference` (or the Anthropic
provider crate) that, when `needs_refresh()` is true and a `refresh_token` exists,
POSTs to the Anthropic token endpoint, persists the new `OAuthTokens` atomically
via the existing `save_oauth_tokens` (`auth.rs:230`), and retries the call once.
This is a hard prerequisite for R1 — subscription tokens are short-lived.

**Risk / non-goal.** Refreshing an *imported* token is mechanically the same class
as the dropped `services/oauth/` login flow but is narrower (no PKCE, no browser);
it is the minimum needed to make imported subscription tokens usable. Scope it to
refresh-only.

### R3 (nuanced, HIGH/MEDIUM) — Broaden the fallback trigger to a typed classifier (402/billing; optional 401/403 one-shot)

**Why.** jcode's `classify_failover_error_message` (`failover.rs:69-137`)
distinguishes three families with three outcomes and uses
`contains_independent_status_code` (`:57-67`) to avoid embedded-digit false
positives. coco-rs already advances the fallback slot on 503/529 **and** 429
(`engine.rs:1320-1324`) — so the "capacity-only" framing is too narrow.

**Correction (from review) — narrow the claim.** The real, defensible additions
are: (a) treat **402 / billing / insufficient-credit** as a failover-advance
trigger — port the curated substring set + the `contains_independent_status_code`
matcher, and add a 402 arm to `errors.rs::from_status` (it currently falls through
to the catch-all `ProviderError` at `:185`); (b) **optionally** make 401/403 a
one-shot advance-then-terminal rather than immediately terminal
(`errors.rs:167-170`). **Drop** the "sideline the failing credential" phrasing
here — coco-rs has no per-credential/account model, so until R4 lands "sidelining"
degenerates to a model-slot hop. Keep context-overflow on its existing
reactive-compaction path (`engine.rs:1303-1318`).

**Concrete change.** Add a typed `FailoverClass` method on `InferenceError` in
`services/inference` that inspects `StatusCode` + a curated substring set with the
ported digit-boundary matcher; have `engine.rs` consult it. Gate behind the
existing `FallbackRecoveryPolicy` and log the classification (mirror jcode's
`decision=...` at `mod.rs:432`).

**Risk.** A transient 401 advancing the slot jumps to a *different model*, not the
same model with a different account — undesirable for a recoverable 401. This risk
is precisely *why* it pairs naturally with R4.

**Secondary fix (from verifier, fold into R3).** coco-rs's
`is_capacity_error_message` (`engine_helpers.rs:53-63`) does naive substring
`contains` on `"(529)"` / `"status: 529"` with **no digit-boundary guard**, so a
model id or token count containing `529`/`503` could spuriously trip capacity
failover. Porting `contains_independent_status_code` removes this false-positive
class for free.

### R4 (confirmed, MEDIUM/HIGH — sequence after R3) — Multi-account support with rotation for API-key/token providers

**Why.** jcode stores `Vec<Account>` per provider (`auth/claude.rs:49,242-274`;
`auth/account_store.rs`) and rotates to the same provider's next account before
crossing providers (`account_failover.rs:42-97`). coco-rs carries a single
`api_key`/`env_key` per `ProviderConfig` (`provider/mod.rs:39-42`,
`resolve_api_key:296`) and a single `OAuthTokens` — no `Vec<Account>`, no
active-account selection.

**Correction (from review) — sequence and scope.** Land the **round-robin** variant
first; the usage-headroom-ranked variant depends on R5 (non-goal-tainted). Extend
`ProviderConfig` to an ordered `Vec` of credentials (`accounts: Vec<{label,
api_key|auth_token}>`), add the selected account label to `runtime_state_digest`
(the fingerprint already digests account state at `fingerprint.rs:202-216`) so a
rotation forces a turn-boundary client rebuild, and rotate on the R3
failover-advance signal **before** hopping the `ModelRuntime` model slot.

**Risk / non-goal.** coco-rs's one-credential-per-provider model is deliberate, so
this is a genuine new config dimension, not a like-for-like port. Judge worth
against coco-rs's single-user CLI focus — jcode's multi-account is tied to its
multi-session server/swarm story. The headroom-ranked variant requires R5 and
should not be the first step; round-robin respects all non-goals.

### R5 (confirmed-as-stated mechanism, **nuanced→DEFER**) — Provider-side plan/usage as a read-only diagnostic only

**Why.** jcode fetches real 5h/7d Anthropic windows, OpenAI/OpenRouter/Copilot
quotas per account (`usage/provider_fetch.rs:17-497`) and surfaces exhaustion +
switch guidance (`usage/model.rs:320-374`). coco-rs computes only USD from tokens
(`cost.rs`) and captures 429 `reset_at_ms` (`engine_helpers.rs:82-107`) but never
probes plan windows.

**Verdict — DEFER (respects_cocors_nongoals = false).** This **directly** re-creates
the intentionally dropped `services/claudeAiLimits.ts` / `services/policyLimits/` /
`services/rateLimitMessages.ts` surfaces; the Anthropic 5h/7d OAuth-window probe is
exactly the Claude.ai-limits surface the port excludes. It is acceptable **only** as
a strictly read-only, opt-in `/usage` diagnostic that (a) never alters request
shaping, (b) never emits rate-limit policy prose ("you are throttled, switch to X"),
and (c) is gated behind a `Feature` flag. Even then it has **no consumer** until
both R1 (OAuth-subscription auth) and R4 (multi-account ranking) land.

**Recommendation.** Do not build the provider-side probe now. If anything, surface
the data coco-rs *already* has — the 429 `reset_at_ms` in
`ToolAppState.rate_limits` — in a `/usage` view, which needs no new provider call
and no non-goal violation. Build the full per-provider/plan probe only if R1 + R4
are committed.

### R6 (verifier finding, LOW/LOW) — Carry provider-level switch rationale in the fallback notice

**Why.** jcode emits a structured `ProviderFailoverPrompt` with from/to
provider+label, estimated input chars/tokens, and reason
(`failover.rs:5-30`) so the UI explains *why* it switched. coco-rs emits only a
`model_fallback_notice` with original/fallback model ids and a typed
`ModelFallbackReason` (`engine.rs:1380-1389`, `model_runtime.rs:68-79`) — no
provider-level rationale or token-resend estimate.

**Concrete change.** Extend `emit_model_fallback_notice` / the `ModelFallbackReason`
payload to include the from/to provider label and (optionally) an estimated
resend-token count. Pure additive telemetry/UX; no behavioral risk; fully within
scope.

---

## Rejected after adversarial review

No suggestion in the analyst set was fully **refuted** — all five carried a
`confirmed` or `nuanced` verdict. The substantive *partial* rejections, surfaced
above inside the recommendations, were:

- **R3 — "coco-rs is capacity-only / 401-402 terminal."** *Rejected as framed.*
  coco-rs already advances on 429 *and* 503/529 (`engine.rs:1320-1324`). Only the
  402 trigger and the optional 401/403 one-shot survive; the "sideline the failing
  credential" phrasing is dropped (no account model until R4).

- **R5 — full provider-side usage probe as an actionable signal.** *Rejected /
  deferred.* It directly re-creates the documented dropped surfaces
  (`services/claudeAiLimits.ts`, `services/policyLimits/`,
  `services/rateLimitMessages.ts`) and has no consumer until R1 + R4 exist. Only a
  pure read-only `/usage` diagnostic over data coco-rs already holds is acceptable.

- **jcode "subscription router product."** *Rejected as a real capability.* The base
  URL is `https://subscription.jcode.invalid/v1` (`subscription_catalog.rs:8`),
  pinned to a `Stealth` stub upstream — a placeholder, not a shipping backend.

- **jcode Anthropic cloud routes (Bedrock/Vertex/Foundry breadth).** *Out of scope.*
  These are coco-rs's explicit documented non-goals; jcode's coverage there is not a
  coco-rs deficiency.
