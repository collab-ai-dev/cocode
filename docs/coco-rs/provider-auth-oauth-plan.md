# Provider Auth & Subscription Login — Design Plan

**Status:** Design (v2 — adversarially reviewed). **Scope:** Add interactive
OAuth *login* + *subscription inference* to coco-rs, starting with **OpenAI /
ChatGPT subscription** (the `chatgpt.com/backend-api/codex` Responses route). The
design is provider-generic so Anthropic (Claude Max) and Gemini (Code Assist)
subscriptions slot in later behind the same machinery.

> Companion analysis: [`jcode/09-providers-inference-oauth.md`](jcode/09-providers-inference-oauth.md)
> flagged this as the single most consequential functional gap (it focuses on
> Anthropic). **This is the OpenAI-first implementation design.** It deliberately
> *adds* the `services/oauth/` surface the original port marked a non-goal — the
> user has explicitly opted into login, and backward compatibility is disregarded.

**Provenance of file:line anchors.** `coco-rs/…` and `codex-rs/…` anchors are
verified against the in-repo trees (codex-rs is the in-repo reference impl).
`jcode` anchors point at the **external reference tree** (`agents/jcode`, outside
this repo) and the in-repo analysis doc `docs/coco-rs/jcode/09-…md`; treat them as
secondary corroboration, not in-repo evidence. Every coco-rs integration claim
below was re-verified by an adversarial review pass (§18 logs the corrections).

---

## 1. Goal & non-goals

**Goal.** `coco login openai` opens a browser, the user signs in with their
ChatGPT account, and from then on coco drives inference through the user's
**ChatGPT subscription** (no API key, no per-token billing) — with automatic,
transparent token refresh. `coco logout openai` / status round it out. Custom
(BYO) OpenAI-compatible providers keep working; subscription is one more
*credential mode* on the existing OpenAI provider family.

**Non-goals (this iteration).** Multi-account rotation (designed to grow into it,
§11, but v1 is single-account-per-provider); provider-side plan/usage probes
(companion R5); Anthropic/Gemini wire contracts (framework generic, only OpenAI
implemented); FedRAMP routing (`X-OpenAI-Fedramp`, §5 note); Anthropic cloud
routes (Bedrock/Vertex/Foundry).

---

## 2. The shape of the problem (verified constraints)

These facts from current source determine the design.

1. **The OpenAI provider's auth is a *synchronous* per-request header closure.**
   `OpenAIProvider::new` builds `headers: Arc<dyn Fn() -> HashMap<String,String> + Send + Sync>`
   once and clones it into every model's `OpenAIConfig`
   (`vercel-ai/openai/src/openai_provider.rs:56,76-103,114-122`). It is invoked
   *fresh* per request (`OpenAIConfig::get_headers`, `openai_config.rs:35-37`) and
   written verbatim to reqwest (`vercel-ai/provider-utils/src/api.rs:87-91`).
   **There is no async/`Future` auth path** — OAuth refresh *cannot* `await`
   inside the closure. Refresh must happen out-of-band into shared state the
   closure reads synchronously.

2. **The Responses model already composes the codex URL for free.** It POSTs to
   `self.config.url("/responses")` (`responses/openai_responses_language_model.rs:481,972`);
   `OpenAIConfig::url` appends the path unless `full_url`/suffix says otherwise
   (`openai_config.rs:26-32`). With `base_url = "https://chatgpt.com/backend-api/codex"`
   this yields `…/codex/responses`. **No URL-composition change needed.**

3. **`ProviderConfig` has no credential dimension beyond api-key.** It carries
   `env_key: String` (mandatory) + optional `api_key`; `resolve_api_key()` =
   env-var-or-config-string (`common/config/src/provider/mod.rs:296-299`).
   `OpenAIProviderSettings` exposes only `api_key/organization/project/headers` —
   **no bearer slot** (`openai_provider.rs:29-47`).

4. **`services/inference/auth.rs` is Anthropic-decorative and never reaches the
   wire.** `resolve_auth`'s `AuthMethod` feeds only the SDK *account display*
   block (`app/cli/.../cli_bootstrap.rs::auth_method_to_account`), **not**
   `model_factory`. `OAuthTokens::needs_refresh` exists but **nothing calls a
   refresh executor** (verified: struct field only).

5. **Every OAuth building block already ships and is a workspace dep.**
   `services/rmcp-client` does a loopback PKCE flow with `tiny_http`
   (`perform_oauth_login.rs:9-10,233-276`), keyring-or-file storage via
   `coco-keyring-store` (`oauth.rs`), the `oauth2` crate, `sha2`. Workspace
   already declares `arc-swap`, `oauth2=5`, `tiny_http=0.12`, `webbrowser`,
   `base64`, `rand`, `sha2`, `url` (`coco-rs/Cargo.toml`). The codex reference
   (`codex-rs/login`) gives the exact OpenAI endpoints.

> **`model_factory` lives in `services/inference/src/model_factory.rs`** (NOT
> app/cli — the docs/coco-rs CLAUDE.md map is stale). `services/inference` already
> deps all four `vercel-ai-*` provider crates (`Cargo.toml:22-25`), so the
> integration changes below land *inside* coco-inference, not the CLI.

The reference contracts (verified against `codex-rs`; jcode matches):

| Thing | Value |
|---|---|
| Issuer | `https://auth.openai.com` (`login/src/server.rs:54`) |
| Authorize / Token | `{issuer}/oauth/authorize` · `{issuer}/oauth/token` (`server.rs:518,729`) |
| Client ID | `app_EMoamEEZ73f0CkXaXp7hrann` (`manager.rs:929`) |
| PKCE | S256; verifier = 64 random bytes URL-safe-no-pad; challenge = SHA-256(verifier) (`pkce.rs:13-22`) |
| Loopback | `http://localhost:1455/auth/callback` (fallback `1457`) (`server.rs:55-57,156`) |
| Scope | `openid profile email offline_access api.connectors.read api.connectors.invoke` (`server.rs:497-498`) |
| Authorize extras | `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `state`, `originator=codex_cli_rs` (`server.rs:505-508`) |
| **Code→token** | **form-urlencoded** `grant_type=authorization_code&code&redirect_uri&client_id&code_verifier` (`server.rs:738-745`) |
| **Refresh** | **JSON** (`Content-Type: application/json`) body `{client_id, grant_type:"refresh_token", refresh_token}` → `{id_token?, access_token, refresh_token?}` (`manager.rs:829-833,915-926`) |
| Inference base (subscription) | `https://chatgpt.com/backend-api/codex` (`model-provider-info/src/lib.rs:37`) — Responses API, identical body |
| Inference headers (subscription) | `Authorization: Bearer <access>`, `ChatGPT-Account-ID: <acct>` (exact casing; **omit if absent**), `originator: codex_cli_rs` (`bearer_auth_provider.rs:32-42`, `default_client.rs:232-234`) |
| `account_id` source | id_token JWT claim `https://api.openai.com/auth → chatgpt_account_id` (`token_data.rs:77,96`) |
| `store` | codex sets `store = is_azure_responses_endpoint()` → **`false` for the codex backend** (`core/src/client.rs:761`); `stream:true` |

> **Two content-types on one endpoint:** code-exchange is form-urlencoded;
> refresh is JSON. Do not form-encode the refresh request.

> **Late host resolution (codex insight, `model-provider-info/src/lib.rs:236-248`):**
> the subscription path is *not a different API* — same Responses body, only
> `base_url` + auth headers differ, resolved live per request. coco achieves the
> same via the existing **live per-request header closure**.

---

## 3. Architecture overview

Three new pieces + integrations. Honors both governing rules: **wire contract in
the provider crate** (root `CLAUDE.md` Multi-Provider Boundaries) and **`vercel-ai-*`
stay faithful `@ai-sdk` ports with no `coco-*` deps** (enforced by
`scripts/check-vercel-ai-seam.sh`). Credential *acquisition/management* (login,
store, refresh) is a new cross-cutting service; the *wire shape* stays in
`vercel-ai-openai`.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ app/cli   `coco login|logout openai` (generalize the stub) + /login (P2)   │
│           constructs the AuthService(Arc) at session bootstrap, hands the   │
│           resolver to RoleClientCache + create_api_client + subagent/side   │
├──────────────────────────────────────────────────────────────────────────┤
│ services/                                                                   │
│   provider-auth  ★NEW (L2)★  PKCE+loopback+paste login, flow descriptors    │
│   (coco-provider- (OpenAI first), provider-scoped keyring/file store, the   │
│    auth)          single-instance TokenCell (ArcSwap), serialized refresh   │
│                   executor, JWT claims; impl coco_inference::                │
│                   ProviderCredentialResolver. DEPENDS ON coco-inference.    │
│                                                                             │
│   inference (L2)  trait ProviderCredentialResolver + neutral SubscriptionCreds; │
│                   RoleClientCache stores the resolver; model_factory.        │
│                   build_openai adapts SubscriptionCreds → vercel_ai_openai:: │
│                   OpenAIAuth::ChatGptSubscription (inference already deps     │
│                   vercel-ai-openai, so this conversion is seam-legal)        │
├──────────────────────────────────────────────────────────────────────────┤
│ vercel-ai/openai (L0)  ★CHANGE★  OpenAIAuth enum on OpenAIProviderSettings;  │
│                   header closure handles subscription mode; OpenAIConfig     │
│                   .chatgpt_subscription flag → Responses model defaults      │
│                   store:false (+ encrypted_content include). Owns codex      │
│                   base-URL + header-name + originator constants. No coco dep. │
├──────────────────────────────────────────────────────────────────────────┤
│ common/config (L1)  ★CHANGE★  ProviderConfig.auth: ProviderAuth              │
│                   {ApiKey | OAuth{flow}}; from_partial relaxes env_key for   │
│                   OAuth; builtin `openai-chatgpt`; fingerprint digests auth  │
│ common/types  ★CHANGE★  OAuthFlowId enum (closed set, schema-gated)          │
└──────────────────────────────────────────────────────────────────────────┘
```

**Data flow (steady state):**
```
coco login openai → PKCE+loopback → token exchange → StoredCredential
                  → keyring (or ~/.coco/auth/openai-chatgpt.json, 0600, atomic)

session bootstrap → AuthService loads StoredCredential → ONE process-stable
                    TokenCell (ArcSwap<TokenSnapshot>) per provider
                  → spawns serialized refresh task (wakes ~60s before exp)
                  → resolver handed to RoleClientCache + create_api_client

each request → model_factory.build_openai: auth=OAuth{OpenAiChatGpt}
             → SubscriptionCreds supplier (reads cell) → OpenAIAuth::ChatGptSubscription
             → header closure reads cell snapshot → Bearer + ChatGPT-Account-ID + originator
             → Responses POST to https://chatgpt.com/backend-api/codex/responses (store:false)
```

The closure reading a live `ArcSwap` snapshot is the lynchpin: **token refresh,
re-login, and (future) account switch are transparent** — provided the
single-TokenCell invariant (§8) holds.

---

## 4. New crate: `coco-provider-auth` (`services/provider-auth`)

**Layer:** L2. **Error tier:** Tier-3 (snafu + `coco-error`).
**Depends on:** `coco-inference` (the `ProviderCredentialResolver` trait +
neutral `SubscriptionCreds` it implements — this is the *inversion direction*:
the impl depends on the abstraction's crate; **`coco-inference` does NOT depend
back on `coco-provider-auth`**, so no cycle), `coco-types` (`OAuthFlowId`),
`coco-config` (paths, `EnvKey`, `RedactedSecret`), `coco-error`,
`coco-keyring-store`, `coco-async-utils`, and externally `reqwest`, `oauth2`,
`sha2`, `base64`, `rand`, `tiny_http`, `webbrowser`, `url`, `arc-swap`, `tokio`.
**Does NOT depend on** any `vercel-ai-*` crate (would trip `check-vercel-ai-seam.sh`)
— the credential carrier at the trait boundary is coco-neutral (§7.1).

> **Workspace plumbing** (don't forget): add `services/provider-auth` to
> `[workspace] members`, add `coco-provider-auth.path` to `[workspace.dependencies]`,
> bump the `services/ = 7` count comment to `8` in `coco-rs/Cargo.toml`, and add
> the crate to the docs/coco-rs CLAUDE.md L2 layer table + Crate Count table.

### 4.1 Modules

```
src/lib.rs          AuthService (public façade) + ProviderCredentialResolver impl
   flow/{mod,pkce,loopback,paste,device_code}.rs   login orchestration
   descriptor.rs    OAuthFlowDescriptor + static catalog (OpenAI first)
   store.rs         StoredCredential, CredentialStore (keyring + file fallback)
   token_cell.rs    TokenCell = Arc<ArcSwap<TokenSnapshot>>, single instance/provider
   refresh.rs       serialized refresh executor (Semaphore(1) per cell)
   jwt.rs           base64url JWT claims (account_id, plan, email, exp)
   resolver.rs      ProviderCredentialResolver impl over the live TokenCells
   error.rs         ProviderAuthError (snafu, StatusCode)
```

### 4.2 Flow descriptor (data)

`OAuthFlowDescriptor { flow, issuer, authorize_path, token_path, revoke_path,
client_id, scope, default_port, fallback_port, callback_path, authorize_extra:
&[(k,v)], account_id_claim: &[&str] }`. OpenAI values from the §2 table. Adding
Anthropic/Gemini later = one descriptor + a wire mode in that provider crate.

**Test override.** The token-endpoint host is overridable for wiremock via a
**registered `EnvKey`** — add `CocoAuthOpenaiTokenUrl => "COCO_AUTH_OPENAI_TOKEN_URL"`
to `coco_config::EnvKey` (variant + `as_str` arm) and read it with
`coco_config::env::var(EnvKey::CocoAuthOpenaiTokenUrl)`. The root CLAUDE.md rule
is "add the variant to `EnvKey`; never `std::env::var` ad-hoc" — a bare string
through the `env::var` helper compiles but violates the convention.

### 4.3 Login orchestration (`flow/mod.rs`)

`login(flow, opts) -> Result<StoredCredential>`:
1. PKCE (S256) + 32-byte `state`.
2. `loopback::bind(default_port → fallback_port → ephemeral)` → `redirect_uri`
   from the bound addr (the rmcp-client pattern, `perform_oauth_login.rs:233-250`).
   `--no-browser`/non-TTY → skip binding, paste mode.
3. Authorize URL (descriptor + PKCE + state + extras), `webbrowser::open`.
4. Receive `GET /auth/callback?code&state`; **validate `state` first** (CSRF,
   codex `server.rs:289-309`), then require non-empty `code`.
5. `exchange_code` — POST `{token}` **form** body (§2 table).
6. Decode id_token claims (`account_id`, `plan_type`, `email`, `exp`).
7. Persist `StoredCredential`; `cell.store(snapshot)` on the process TokenCell.

**Headless/scriptable triad** (jcode parity): `--print-auth-url` emits the URL +
persists a pending `{verifier, state, redirect_uri}`; `--callback-url <url>`
completes by re-validating `state` + exchanging (OpenAI completion requires
`--callback-url`, not `--auth-code`, because state must be checked). 300s loopback
timeout → paste prompt. We do **not** mint an API key from the grant (codex's
optional RFC-8693 step, `server.rs:1121-1155`): the subscription path uses the
OAuth access token directly.

### 4.4 `StoredCredential` & `TokenSnapshot`

```rust
/// Persisted, provider-scoped. JSON in keyring (or file fallback).
/// Token fields MUST NOT leak via Debug — wrap in RedactedSecret OR give a manual
/// Debug printing "<redacted>" for access/refresh/id_token, mirroring
/// ProviderConfig's hand-written Debug (common/config/src/provider/mod.rs:138-153).
pub struct StoredCredential {
    pub flow: OAuthFlowId,
    pub access_token: RedactedSecret,
    pub refresh_token: Option<RedactedSecret>,
    pub id_token: Option<RedactedSecret>,   // raw JWT; re-derive claims on load
    pub account_id: Option<String>,         // chatgpt_account_id claim
    pub expires_at_ms: Option<i64>,         // epoch MILLIS (not seconds — §10)
    pub plan_type: Option<String>,          // display only
    pub email: Option<String>,
    pub login_epoch: u64,                   // bumped on fresh login, NOT on refresh
}

/// In-memory, lock-free, read each request. account_id lives here too so an
/// account switch is transparent (no client rebuild — §8). Manual Debug redacts.
#[derive(Clone)]
pub struct TokenSnapshot {
    pub access_token: String,
    pub account_id: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<i64>,
}
```

### 4.5 Storage (`store.rs`) — `AuthStorageBackend` trait (borrowed from codex)

Adopt codex's storage abstraction verbatim in spirit
(`codex-rs/login/src/auth/storage.rs:97-100`): a `trait AuthStorageBackend {
load/save/delete }` with four impls selected by a config store-mode —

```rust
trait CredentialBackend: Send + Sync {           // ≈ codex AuthStorageBackend
    fn load(&self, name: &str) -> Result<Option<StoredCredential>>;
    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()>;
    fn delete(&self, name: &str) -> Result<bool>;
}
// Keyring  — coco-keyring-store, service "Coco Provider Auth", account=provider name
// File     — ~/.coco/auth/<name>.json, 0600, atomic temp+rename
//            (save_oauth_tokens already does this, auth.rs:241-247; codex's
//            in-place truncate is a bug we do NOT copy)
// Auto     — keyring first, file fallback (the default; mirrors rmcp-client/oauth.rs)
// Ephemeral — in-memory only, for tests + `--no-persist` headless runs
```

The `Ephemeral` backend (codex `EphemeralAuthStorage`, `storage.rs:298`) makes the
store unit-testable without touching the real keyring/disk. Backend choice is a
config knob, not a hardcode. One credential per provider instance.

### 4.6 TokenCell + serialized refresh (`token_cell.rs`, `refresh.rs`)

`TokenCell` wraps `Arc<ArcSwap<TokenSnapshot>>`: `supplier()` →
`Arc<dyn Fn() -> Option<SubscriptionCreds>>` (lock-free read), `store(snap)`,
`snapshot()`.

- On `AuthService::new`, each logged-in provider loads its `StoredCredential` →
  one process-stable `TokenCell` + a `tokio` refresh task that sleeps to
  `expires_at_ms - 60_000`, refreshes, `store()`s, persists. Cancels with the
  session via `coco-async-utils`.
- **Refresh-lock invariant (load-bearing).** There are up to three refresh
  writers — the background task, the lazy turn-boundary check, and the future
  reactive-401 path (§12) — to the *same* cell. codex serializes refresh with a
  `Semaphore(1)` + a guarded reload-before-refresh because the **refresh token
  rotates and is single-use**: a concurrent double-refresh burns it and the second
  call gets `refresh_token_reused` → terminal (`manager.rs:1261,1682-1716,921-926`).
  `refresh.rs` MUST hold a per-cell `Semaphore(1)` and re-check `needs_refresh`
  after acquiring (double-checked) so only one refresh fires.
- `refresh()` = POST `{token}` **JSON** `{client_id, grant_type:"refresh_token",
  refresh_token}` → merge (preserve `account_id` if new id_token absent),
  recompute `expires_at_ms`, persist, `store()`. A 401
  (`refresh_token_expired/reused/invalidated`, codex `manager.rs:859-886`) →
  terminal `ProviderAuthError::SessionExpired` → prompt re-login.

---

## 5. `vercel-ai-openai` changes (the wire contract)

This crate **owns** the OpenAI-subscription wire facts. No `coco-*` dep
(Tier-2/L0 preserved): the new auth carrier is `std` (`Arc<dyn Fn>`), and the
"not logged in" failure surfaces as a *header omission* (→ a clean 401 the engine
maps), never a new `coco-error` variant.

```rust
// vercel-ai/openai/src/openai_auth.rs
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const DEFAULT_ORIGINATOR: &str = "codex_cli_rs";
const HDR_CHATGPT_ACCOUNT_ID: &str = "ChatGPT-Account-ID"; // exact casing, load-bearing
const HDR_ORIGINATOR: &str = "originator";

#[derive(Clone)]
pub struct ChatGptCreds { pub access_token: String, pub account_id: Option<String> }

pub enum OpenAIAuth {
    /// Static API key; falls back to OPENAI_API_KEY. (default — current behavior)
    ApiKey(Option<String>),
    /// ChatGPT-subscription OAuth; Bearer + ChatGPT-Account-ID + originator,
    /// read fresh from `creds` per request (refreshed out-of-band).
    ChatGptSubscription { creds: Arc<dyn Fn() -> Option<ChatGptCreds> + Send + Sync>,
                          originator: Cow<'static, str> },
}
impl Default for OpenAIAuth { fn default() -> Self { Self::ApiKey(None) } }
```

- `OpenAIProviderSettings.api_key: Option<String>` → `auth: OpenAIAuth`. The
  struct keeps `#[derive(Default)]` (valid since `OpenAIAuth: Default`).
- Header closure (`openai_provider.rs:76-103`): `ApiKey` → unchanged; `ChatGptSubscription`
  → call `creds()`; if `Some` set `Authorization: Bearer …`, `originator`, and
  `ChatGPT-Account-ID` *iff* `account_id.is_some()`; emit no org/project. If
  `None` → no `Authorization` → 401 surfaced as "run `coco login openai`". Custom
  `headers` still merge last.
- **`store:false` (and the encrypted-content include).** coco's Responses model
  writes `body["store"]` only when `openai_options.store.is_some()`
  (`openai_responses_language_model.rs:319-321`) and auto-adds
  `reasoning.encrypted_content` **only when `store == Some(false)`** for reasoning
  models (`:410-413`) — the two are coupled, and *neither* is set by thinking-level
  conversion. So: add `OpenAIConfig.chatgpt_subscription: bool` (set when
  `OpenAIAuth::ChatGptSubscription`); the Responses model defaults `store:false`
  in subscription mode, which *also* unlocks the encrypted-content include. This
  keeps the body logic in the provider crate, no per-call-extras hack.
  (codex's `store = is_azure_responses_endpoint()` is `false` for the codex
  backend, `client.rs:761` — same outcome; do **not** force `store:false` for a
  hypothetical Azure-OpenAI provider.)

> **Why an enum, not `client_options.headers`.** That map is a *static* snapshot
> captured at provider construction (`openai_provider.rs:74`), so a refreshed
> token would go stale until a client rebuild. The closure-fed enum gives live
> tokens with zero per-request async and keeps the client identity-stable (§8),
> and keeps the originator/account-id header *names* inside the provider crate.

> **originator & FedRAMP.** codex sends `originator` on *every* request (default
> client header, `default_client.rs:232-234`); coco emits it only on the
> subscription branch — sufficient, because only the codex backend gates on it.
> codex also sends `X-OpenAI-Fedramp: true` for FedRAMP accounts
> (`bearer_auth_provider.rs:43-45`, id_token `chatgpt_account_is_fedramp`); **v1
> omits it** (explicit non-goal) — a FedRAMP-workspace login would misroute; this
> documents the cause.

`base_url` comes from `ProviderConfig` (= codex URL); `config.url("/responses")`
does the rest (§2.2). No URL code change.

---

## 6. `coco-config` + `coco-types` changes

### 6.1 `OAuthFlowId` (coco-types — closed set)

```rust
// common/types/src/provider.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]   // match existing provider enums
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowId { OpenAiChatGpt /* , AnthropicClaude, GeminiCodeAssist */ }
```

### 6.2 `ProviderConfig.auth`

```rust
// common/config/src/provider/mod.rs
#[derive(Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    #[default] ApiKey,                       // env_key → config api_key (current)
    OAuth { flow: OAuthFlowId },             // managed by coco-provider-auth, auto-refreshed
}
```

- `PartialProviderConfig` gains
  `#[serde(default, skip_serializing_if = "Option::is_none")] pub auth: Option<ProviderAuth>`
  (keeps the byte-stable round-trip test green, `mod.test.rs:128-162`).
- **`from_partial` change (concrete — else builtin crashes at startup).**
  `from_partial` currently hard-requires `env_key` Some, else `IncompleteProviderEntry`
  (`mod.rs:169-175`), and `ProviderConfig.env_key` is a non-`Option` `String`
  (`:108`). Branch on `partial.auth` **before** the env_key check: when
  `Some(OAuth{..})`, skip the requirement and set `env_key = String::new()`. Empty
  `env_key` flows harmlessly: `digest_api_key_origin` hashes empty bytes
  (`fingerprint.rs:174`), `resolve_api_key()` returns `None`, and `build_openai`
  never reads `env_key` (only `build_openai_compat` does, `model_factory.rs:390`,
  and openai-chatgpt routes through `build_openai`). Add a builtin-resolves test.
- `merge_partial` overlays `auth` only when `Some` (pattern of `api`).

### 6.3 Builtin `openai-chatgpt`

Add a second tuple to `builtin/openai.rs::providers()`:

```rust
("openai-chatgpt", PartialProviderConfig {
    api: Some(ProviderApi::Openai),
    auth: Some(ProviderAuth::OAuth { flow: OAuthFlowId::OpenAiChatGpt }),
    base_url: Some("https://chatgpt.com/backend-api/codex".into()),
    wire_api: Some(WireApi::Responses),
    // env_key omitted — from_partial defaults it to "" under OAuth (§6.2)
    ..Default::default()
})
```

**Models resolve automatically.** Builtin models are keyed by bare `model_id` and
merged into one global map; `ModelRegistry::resolve(provider, model_id)`
lazy-synths from it (`model/registry.rs:80-95`), so `("openai-chatgpt","gpt-5-5")`
resolves to the existing GPT-5.5 `ModelInfo` with no per-provider entry. The
*only* reason to add a `models` entry on this builtin is if the codex backend's
accepted ids **differ** from the public ones — then set
`PartialProviderConfig.models: Some({ "<id>": PartialProviderModelOverride {
api_model_name: Some("<codex-id>"), .. } })`. Verify accepted ids before assuming
zero work (gate §13.2).

---

## 7. `services/inference` integration

### 7.1 Resolver trait + neutral carrier (in `coco-inference`)

The trait return type **must be coco-neutral** (a `vercel_ai_openai` type would
force a vercel-ai dep onto `coco-provider-auth`, tripping the seam guard, or a
coco-inference re-export — defeating the inversion). So:

```rust
// services/inference/src/credentials.rs
/// Coco-neutral subscription credentials (no vercel-ai dep crosses the seam).
/// Provider-generic: OpenAI uses access_token + account_id (→ ChatGPT-Account-ID);
/// Anthropic later reads subscription_type for cache-TTL; new fields are additive.
#[derive(Clone)]
pub struct SubscriptionCreds {
    pub access_token: String,
    pub account_id: Option<String>,       // generic account identifier (OpenAI: chatgpt_account_id)
    pub subscription_type: Option<String>,// pro/plus/max/team (display + Anthropic cache-TTL later)
}

pub type SubscriptionCredsSupplier = Arc<dyn Fn() -> Option<SubscriptionCreds> + Send + Sync>;

/// Implemented by coco-provider-auth. coco-inference depends ONLY on this trait;
/// the auth service stays out of the inference dep graph (testable with a fake).
pub trait ProviderCredentialResolver: Send + Sync {
    /// Live subscription-credential supplier for an OAuth provider instance
    /// (None for api-key providers / not logged in).
    fn subscription_creds(&self, provider_name: &str) -> Option<SubscriptionCredsSupplier>;
}
```

`build_openai` (which already deps `vercel-ai-openai`) **adapts** the neutral
supplier into the wire type — the conversion lives where it's seam-legal:

```rust
// model_factory.rs build_openai (now takes resolver: Option<&Arc<dyn ProviderCredentialResolver>>)
let auth = match provider_cfg.auth {
    ProviderAuth::OAuth { flow: OAuthFlowId::OpenAiChatGpt } => {
        let supplier = resolver.and_then(|r| r.subscription_creds(&provider_cfg.name))
            .ok_or_else(|| /* NotLoggedIn — actionable */)?;
        vercel_ai_openai::OpenAIAuth::ChatGptSubscription {
            creds: Arc::new(move || supplier().map(|c|
                vercel_ai_openai::ChatGptCreds { access_token: c.access_token, account_id: c.account_id })),
            originator: vercel_ai_openai::DEFAULT_ORIGINATOR.into(),
        }
    }
    ProviderAuth::ApiKey => vercel_ai_openai::OpenAIAuth::ApiKey(provider_cfg.resolve_api_key()),
};
```

### 7.2 Threading hub = `RoleClientCache` (NOT `ModelRuntime`)

`ModelRuntime` (app/query) holds **pre-built** `Arc<ApiClient>` slots and never
calls `build_api_client` — wrong place. The construction hub is
`coco_inference::RoleClientCache` (`role_client_cache.rs:110`), which re-invokes
`build_api_client` for non-Main roles. So:

- Add `resolver: Option<Arc<dyn ProviderCredentialResolver>>` as a stored field on
  `RoleClientCache::new` (`:61`).
- Thread `Option<&Arc<dyn ProviderCredentialResolver>>` through
  `build_api_client` → `build_language_model_from_runtime` → `build_openai` and
  `build_fallback_clients_for_role`.
- Update the real call sites: `headless.rs:221`, `session_runtime.rs:1713,1782`
  (+ `RoleClientCache` construction at `:1193`), `side_query_impl.rs:77`,
  `agent_handle_factory.rs:113`. **Subagents/side-queries** build their own
  clients — they need the resolver too, else a subagent pointed at `openai-chatgpt`
  fails.

The Anthropic-decorative `resolve_auth`/`AuthMethod` machinery in `auth.rs` is
untouched.

### 7.3 Availability gates (HARD blockers — must change)

Two existing gates assume an api-key and silently break OAuth providers:

1. **`create_api_client` (the Main-client builder) downgrades to `MockModel` when
   `resolve_api_key().is_some()` is false** (`headless.rs:215-225`). An OAuth
   provider returns `None` → the session never reaches the real backend. Change:
   treat `ProviderAuth::OAuth` as "credential present iff the resolver reports a
   logged-in supplier." Requires the resolver in scope at `create_api_client`.
2. **The TUI model picker marks providers with no api_key and no auth_token as
   `MissingApiKey`/unavailable** (`tui_runner.rs:4884-4892`). An OAuth provider
   shows UNAVAILABLE even when logged in. Change `build_provider_statuses` to
   consult login state for `ProviderAuth::OAuth`.

### 7.4 Other integration points the design must touch

- **CLI dispatch.** `Commands::Login`/`Logout` are no-field Anthropic stubs
  dispatched at `main.rs:105-109`; generalizing to the §9 flag shape is a clap
  change to the `Commands` enum (`lib.rs:360-363`) + a dispatch rewrite.
- **SDK account display.** `cli_bootstrap.rs::auth_method_to_account` populates the
  SDK `account` block from `AuthMethod` (Anthropic-only). v1 decision: OpenAI OAuth
  surfaces *nothing* in the SDK account block (documented), or add an
  `SdkAccountInfo` from the login email/plan later.
- **Hot-reload.** `RoleClientCache` is a session-construction snapshot with a
  documented hot-reload gap (`role_client_cache.rs`). The `AuthService`/resolver
  `Arc` must **outlive** a `SettingsWatcher` `RuntimeConfig` reload — own it at
  session scope (not inside the rebuilt cache) and re-hand it on rebuild.

---

## 8. Client coherence / fingerprint

Two coupled points (the v1 draft's "no change needed" was half-right):

1. **Token refresh / account switch → no rebuild, by design.** The bearer **and**
   `account_id` are read live from the *single process TokenCell* inside the
   per-request closure. `fingerprint.rs:96-114` digests none of it, and
   `resolve_api_key()` is `None` for OAuth → `api_key_origin_digest` stable. So a
   refresh or in-place re-login serves new creds with no rebuild (matches codex
   "auth sampled live per request").
   - **Invariant (must be explicit):** `AuthService` is the single source of truth
     (codex's `AuthManager`, `manager.rs`) and owns exactly **one** TokenCell per
     provider for the process lifetime; refresh **and** interactive re-login both
     call `cell.store()` on that same cell, never replace it. It also exposes a
     `tokio::sync::watch` **change channel** (codex `AuthManager::auth_change_receiver`,
     `manager.rs:1256,1409`) so the TUI status line / model picker reflect
     login/logout/refresh without polling. An out-of-process `coco login` while a
     TUI session is live therefore needs an in-session relogin path (the `/login`
     slash command routing through the shared `AuthService` handle on `AppState`,
     which `store()`s into the live cell and ticks the watch) — otherwise relogin
     requires a restart.
2. **`ProviderConfig.auth` IS a new fingerprint input.** Flipping a provider's
   `auth` ApiKey↔OAuth while `base_url`/`client_options` stay identical would NOT
   rebuild the cached client today (`auth` is absent from the digest). Add a 1-byte
   `ProviderAuth` discriminant + `OAuthFlowId` into `digest_runtime_state` (or
   `digest_api_key_origin`), and add a `fingerprint.test.rs` case proving
   ApiKey→OAuth on the same provider name rebuilds. (Distinct names — `openai` vs
   `openai-chatgpt` — are already distinguished by the `provider` field; the digest
   entry covers the in-place-mutation case.)

---

## 9. CLI & slash-command surface

Generalize the existing Anthropic stubs (`app/cli/src/lib.rs:360-363`):

```rust
Login {
    provider: Option<String>,                // openai (anthropic/gemini later); omit → picker
    #[arg(long, alias = "headless")] no_browser: bool,
    #[arg(long, conflicts_with = "callback_url")] print_auth_url: bool,
    #[arg(long, conflicts_with = "print_auth_url")] callback_url: Option<String>,
    #[arg(long)] json: bool,
},
Logout { provider: Option<String> },
```

- `coco login openai` → `AuthService::login(OpenAiChatGpt, …)` → store → "Logged
  in as `<email>` (`<plan>`); models under `openai-chatgpt/*`."
- `coco logout openai` → `CredentialStore::clear` + best-effort `/oauth/revoke`
  (port codex `revoke.rs`).
- Fold provider auth status into `Status`/`Doctor`.
- **`/login` slash command (P2)** in `commands/`: opens the same flow from the TUI
  via the shared `AuthService` handle on `AppState` (the in-session relogin path
  §8 requires). Both surfaces share one `AuthService`.

---

## 10. Token lifecycle summary

| Phase | Trigger | Mechanism |
|---|---|---|
| Acquire | `coco login openai` | PKCE + loopback (or paste) → form token exchange → store |
| Load | session bootstrap | store → `TokenSnapshot` → **one** `TokenCell` + serialized refresher |
| Serve | each request | sync closure reads `ArcSwap` snapshot → headers |
| Refresh (proactive) | bg task, `exp - 60s` | JSON refresh grant → `store()` + persist (under Semaphore) |
| Refresh (lazy) | turn boundary, `needs_refresh` | same, double-checked under the lock |
| Refresh (reactive, P2) | 401 from codex backend | force refresh + retry once (§12) |
| Expire terminally | refresh 401 (`expired/reused`) | `SessionExpired` → prompt re-login |
| Revoke | `coco logout` | `/oauth/revoke` + clear store |

**Units gotcha (verified in coco source, `auth.rs:63,74-80`):** `expires_at` is
epoch **milliseconds** `i64`; `needs_refresh` uses a 5-min (`exp - 300_000`) skew.
The existing field is `expires_at` — `_ms` here is a naming clarification, not new
semantics. Mixing seconds breaks refresh timing.

---

## 11. Extension contract — adding a provider subscription

The design is built so that *login / refresh / store* are **provider-generic and
frozen**, and the only per-provider code is the **wire contract** (which lives in
that provider's `vercel-ai-*` crate, where the boundary rule says it belongs).
This mirrors codex's split — its `login` crate is the generic OAuth machine; the
per-provider wire shape lives in the model/provider layer (`auth_provider_from_auth`
→ `BearerAuthProvider`).

**Generic — ZERO change to add a provider** (all in `coco-provider-auth`): the PKCE
+ loopback + paste + device flow engine (data-driven off `OAuthFlowDescriptor`),
the `CredentialBackend` store (§4.5), the single-`TokenCell` + serialized refresh
(§4.6), the `ProviderCredentialResolver` impl, and the `coco login/logout` CLI.

**Per-provider — to add subscription X, exactly 5 steps:**
1. `OAuthFlowId::X` variant (coco-types, schema-gated).
2. One `OAuthFlowDescriptor` entry — **pure data** (issuer / authorize / token /
   revoke / client_id / scope / port / callback / authorize-extras / account-id
   claim path). No new flow code.
3. The wire mode in `vercel-ai-<X>` — the *only* real code, e.g.
   `AnthropicAuth::ClaudeSubscription` (the Claude-Code contract: `oauth-2025-04-20`
   beta + `claude-cli` UA + identity prepend + tool-name remap, companion R1) or
   `GoogleAuth::CodeAssist`. Each provider crate owns its own `*Auth` enum — the
   per-provider analog of codex's `BearerAuthProvider` / `AgentIdentityAuthProvider`.
4. One match arm in `model_factory.build_<X>`: `ProviderAuth::OAuth { flow: X }` +
   resolver `SubscriptionCreds` → that wire mode. Compile-checked by the existing
   exhaustive `ProviderApi` match.
5. A builtin provider instance (e.g. `anthropic-claude-max`, `gemini-code-assist`).

**Custom OpenAI-compatible providers** are unaffected (`auth: ApiKey` + `env_key`);
a future `coco provider add <name>` (jcode parity) is orthogonal to OAuth.

**The wire-auth seam — header-only today, request-signing tomorrow.** coco's
seam is the synchronous header closure, which can only *set headers*. codex's
`AuthProvider` trait (`codex-api/src/auth.rs:30-63`) is richer: `add_auth_headers`
for header-only providers **plus** an overridable `apply_auth(Request) -> Request`
for providers that must **sign the full request** (URL+headers+body, e.g. AWS
SigV4). coco's bearer-based subscriptions (OpenAI, Anthropic, Gemini) are all
header-only, so the closure suffices. If a future provider needs request-signing,
the extension axis is to add a request-mutation hook to that provider crate's
model (not the generic layer) — explicitly mirroring codex's `apply_auth`
override. Documented here so the limitation is a known, bounded extension point,
not a surprise.

**Multi-account (companion R4)** = `StoredCredential` → `Vec<Account>` + active
label; `TokenCell` keyed on the active account; switch stays transparent because
`account_id` lives in the snapshot (§8). codex's `account_failover` /
usage-ranked rotation is the reference if/when this lands.

---

## 12. Reactive 401 refresh (phase 2)

codex does one-shot 401 → reload → refresh → retry (`client.rs:1976-2010`,
`UnauthorizedRecovery`). coco's `retry.rs` classifies 401/403 (`is_auth_error`)
but treats them terminal. P2: when the active client is an OAuth-subscription
provider and a request 401s, `AuthService::force_refresh(name)` (under the §4.6
lock) then retry once before surfacing `AuthenticationFailed`. Seam: a refresh
callback into the retry loop, or handle at `ModelRuntime` where the resolver is in
scope. P1 is fine without it (background + turn-boundary refresh covers
hours-long tokens; turns are short).

---

## 13. Verification gates (confirm against source while implementing)

1. **`store:false` + encrypted-content (coupled).** Implement via
   `OpenAIConfig.chatgpt_subscription` (§5), not per-call `provider_options` —
   `ProviderConfig.provider_options` are *instance knobs* parsed at client
   construction (and `vercel-ai-openai` has **no** `parse_provider_options`), NOT
   the per-call body extras. Forcing `store:false` both satisfies the backend and
   triggers the existing `reasoning.encrypted_content` include
   (`openai_responses_language_model.rs:319-321,410-413`).
2. **Model ids.** Confirm which GPT-5.x ids `chatgpt.com/backend-api/codex`
   accepts; add `api_model_name` overrides on `openai-chatgpt` only where they
   diverge (§6.3).
3. **Breaking-change migration (~16 sites).** Replacing `OpenAIProviderSettings.api_key`
   with `auth: OpenAIAuth` breaks every constructor: `model_factory.rs:330`;
   `vercel-ai/openai/src/openai_provider.test.rs:6,16,25,36,47,57,69`;
   `vercel-ai/openai/tests/{chat_tool_input_repair,chat_stream_tool_input,responses_tool_input_repair}_wiremock.rs`;
   `vercel-ai/ai/tests/common/mod.rs:39,51,67,76,99,110`. Update all to
   `auth: OpenAIAuth::ApiKey(...)`. (Also the wiremock tests §14 depends on.)
4. **Live smoke test** (`/verify`, manual/gated) against a real ChatGPT login.

---

## 14. Testing strategy

- **`coco-provider-auth` unit:** PKCE determinism (seeded RNG hook), authorize-URL
  assembly, callback `state` mismatch rejected, **form** token-exchange + **JSON**
  refresh vs wiremock (via `EnvKey::CocoAuthOpenaiTokenUrl`), store round-trip
  (keyring + file), expiry/skew math (ms), JWT claim extraction, **serialized
  refresh** (two concurrent triggers → one HTTP call).
- **`vercel-ai-openai` wiremock:** a `ChatGptSubscription` provider POSTs to
  `…/codex/responses` with `Authorization`/`ChatGPT-Account-ID`/`originator` and
  `store:false`; refresh visible mid-test (closure returns updated token). Locks
  the wire contract like existing `*_wiremock.rs`.
- **`model_factory`:** fake `ProviderCredentialResolver` → asserts `build_openai`
  yields `ChatGptSubscription` for `auth=OAuth` and `ApiKey` otherwise; asserts no
  rebuild on token change but rebuild on auth-mode flip (§8.2 fingerprint test).
- **`coco-config`:** builtin `openai-chatgpt` resolves through `from_partial` with
  empty `env_key` (§6.2); byte-stable round-trip stays green with the new `auth` field.
- Companion `.test.rs` files, `pretty_assertions`, whole-object compares.

---

## 15. Phasing

1. **P1 — OpenAI subscription end to end (the ask).** `coco-provider-auth` (flow +
   OpenAI descriptor + store + single TokenCell + serialized background refresh +
   resolver) · `OpenAIAuth` + `chatgpt_subscription` in `vercel-ai-openai` ·
   `ProviderAuth` + `OAuthFlowId` + builtin `openai-chatgpt` + fingerprint digest ·
   `model_factory`/`RoleClientCache` threading + the §7.3 availability gates ·
   `coco login/logout openai` + status. Loopback + paste login.
2. **P2 — robustness & UX.** Reactive 401 refresh (§12); `/login` slash command +
   TUI picker + in-session relogin; device-code flow; richer status/doctor.
3. **P3 — breadth.** Anthropic (Claude Max) + Gemini behind the same machinery;
   multi-account (companion R4).

---

## 16. Security notes

- Tokens in OS keyring; file fallback `0600` + atomic write. **`StoredCredential`
  and `TokenSnapshot` MUST redact tokens in `Debug`** (RedactedSecret or manual
  Debug, mirroring `ProviderConfig` `mod.rs:138-153`) — `coco-secret-redact` only
  catches values that reach the scrubber; a `{:?}`/snafu capture bypasses it.
- `state` CSRF check before any code processing; loopback bound to `127.0.0.1`.
- `originator: codex_cli_rs` is *client impersonation* required for the codex
  backend to accept the token (first-party gating), analogous to the Claude-Code
  `claude-cli` UA. Load-bearing; document the dependency on OpenAI continuing to
  accept this client_id/originator. Loopback redirect URIs (`1455`/`1457`) must
  remain on OpenAI's allow-list (codex `server.rs:56`); pin via the descriptor and
  keep device-code as a fallback if OpenAI rotates them.

---

## 17. Open decisions (defaults chosen; flag to override)

1. **Reuse `ProviderApi::Openai`** (✔) — identical wire body; a new variant forces
   exhaustive-match churn (`model_factory.rs:118-131`, `fingerprint.rs:101-104`,
   `provider.rs` `as_str`). Credential mode lives on `ProviderConfig.auth`.
2. **Dependency-inverted resolver** (✔) — trait + neutral `SubscriptionCreds` in
   `coco-inference`; `coco-provider-auth` implements it (depends on coco-inference,
   not vice-versa; no vercel-ai dep crosses the seam). Testable with a fake.
3. **`store:false` via `OpenAIConfig.chatgpt_subscription`** (✔) — in the provider
   crate; the `provider_options` route does not reach the per-call body (§13.1).
4. **No API-key minting from the grant** (✔) — use the OAuth access token directly.
5. **`ProviderAuth` in the fingerprint** (✔) — correctness over the v1 "no change."

---

## 18. Adversarial-review corrections folded in (v1 → v2)

- **CRITICAL:** resolver trait now returns coco-neutral `SubscriptionCreds`
  (not `vercel_ai_openai::ChatGptCreds`) — the prior shape tripped
  `check-vercel-ai-seam.sh`; `coco-provider-auth` depends on `coco-inference`
  (inversion direction corrected).
- **CRITICAL:** threading hub is `RoleClientCache` (6+ call sites), not
  `ModelRuntime`; subagent/side-query builders included.
- **CRITICAL:** §7.3 availability gates (`create_api_client` MockModel downgrade;
  TUI picker `MissingApiKey`) — were unstated hard blockers.
- **CRITICAL:** `from_partial` env_key relaxation spelled out (builtin would crash
  at startup otherwise).
- **MAJOR:** `store:false` mechanism corrected (`OpenAIConfig` flag, not
  `provider_options`); coupled with the encrypted-content include.
- **MAJOR:** `ProviderConfig.auth` added to the fingerprint digest.
- **MAJOR:** ~16-site breaking-change migration enumerated (§13.3).
- **MAJOR:** models resolve via lazy-synth (not "zero models"); `api_model_name`
  override is the real lever.
- **MINOR:** refresh body is JSON not form; `model_factory` path corrected to
  services/inference; refresh-lock invariant (rotating single-use token); `EnvKey`
  registration; schema derives; `serde(skip_serializing_if)`; Debug redaction;
  originator/FedRAMP framing; jcode anchors relabeled as external.

---

## 19. Relationship to codex-rs (the reference impl)

codex-rs **natively supports the ChatGPT subscription** and is the in-repo
reference. This design absorbs codex's *architecture*, not just its endpoint
constants. The mapping:

| codex-rs pattern | coco-rs adoption | Where |
|---|---|---|
| OAuth endpoints / client_id / PKCE / scope / loopback ports | **Borrowed verbatim** (the wire facts are OpenAI's, not codex's invention) | §2 table |
| `login` crate = generic OAuth machine (PKCE, loopback, device-code, token exchange, refresh) | **Borrowed** as `coco-provider-auth`'s flow engine; reuses coco's own `tiny_http`/`oauth2`/`coco-keyring-store` (already in `rmcp-client`) | §4.1-4.3 |
| `AuthStorageBackend` trait (File / Keyring / Auto / Ephemeral) | **Borrowed** as `CredentialBackend` (incl. Ephemeral for tests) | §4.5 |
| `AuthManager` = single source of truth + `watch` change channel + Semaphore-serialized refresh + guarded-reload-before-refresh (account-id match) | **Borrowed** as `AuthService` + per-cell `Semaphore(1)` + `tokio::watch` notify | §4.6, §8 |
| `to_api_provider(auth_mode)` — host resolved **late, per request**, from the live auth mode | **Borrowed in spirit**: the live per-request header closure reading an `ArcSwap` gives the same "auth sampled per request, refresh transparent" property | §2, §8 |
| `AuthProvider` trait (`add_auth_headers` + overridable `apply_auth`) — credential→wire seam | **Adapted**: coco's seam is the per-provider header closure; the `apply_auth` (request-signing) axis is documented as the future extension point | §11 |
| `auth_provider_from_auth(&CodexAuth)` → `BearerAuthProvider` / `AgentIdentityAuthProvider` / `Unauthenticated` | **Adapted**: each `vercel-ai-<provider>` owns its own `*Auth` enum (`OpenAIAuth::{ApiKey,ChatGptSubscription}`); model_factory maps `ProviderAuth` → it | §5, §7 |
| `UnauthorizedRecovery` 401 state machine | **Borrowed** as P2 reactive-401 refresh-and-retry | §12 |
| RFC-8693 API-key mint from the grant (`obtain_api_key`) | **Deliberately dropped** — subscription uses the OAuth access token directly | §4.3 |

**Deliberate divergences (and why):**

1. **Separate provider instance (`openai-chatgpt`) vs. codex's host-switch on one
   `openai` provider.** codex keeps one provider and flips the host from the live
   auth mode (`to_api_provider`: `base_url=None` → chatgpt vs openai). coco makes
   the subscription an **explicit `providers.<name>` instance** with `auth: OAuth`
   and an explicit `base_url`. Rationale: coco's config is provider-instance-keyed;
   explicit beats implicit host-switching; and it lets an API-key `openai` and a
   subscription `openai-chatgpt` **coexist** so a `ModelRole` can target either.
   The companion R1 path for Anthropic follows the same shape (`anthropic-claude-max`).

2. **`vercel-ai-*` layering.** codex co-locates auth + transport in `model-provider`
   / `codex-api`. coco keeps the wire contract in the faithful `@ai-sdk` port
   (`vercel-ai-openai`, no `coco-*` dep, enforced by `check-vercel-ai-seam.sh`) and
   credential management in a separate L2 service. The seam between them is the
   coco-neutral `SubscriptionCreds` (§7.1) — codex doesn't need this firewall
   because it isn't mirroring an external SDK.

3. **No cloud routes / FedRAMP / agent-identity in v1.** codex ships Bedrock/Azure,
   `X-OpenAI-Fedramp`, and `AgentIdentity` auth. coco's documented non-goals exclude
   cloud routes; FedRAMP + agent-identity are noted extension points (§5, §11), not
   v1 scope.

4. **codex's in-place `auth.json` truncate** → coco uses **atomic temp+rename**
   (codex's truncate can corrupt on mid-write crash; §4.5).

**Net:** the generic OAuth machine, storage abstraction, single-source-of-truth +
notify, serialized refresh, and live-per-request auth are all taken from codex.
The divergences are where coco's multi-provider/SDK-mirror architecture and its
documented non-goals justify a different (cleaner-for-coco) shape — and every one
preserves the provider-generic extension contract (§11).
