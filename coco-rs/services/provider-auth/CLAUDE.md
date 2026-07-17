# coco-provider-auth

Interactive OAuth login + subscription-credential lifecycle for LLM providers
(ChatGPT, Gemini Code Assist, Grok subscriptions).

## Seam

This crate owns only generic machinery: PKCE + loopback authorization-code
flow (`flow.rs`), RFC 8628 device authorization (`device.rs`), provider-scoped
persistence (`store.rs`), the live credential cell (`token_cell.rs`), and the
serialized refresh executor (`refresh.rs` + `lib.rs`). The per-provider wire
contract stays in `vercel-ai-<provider>`; `model_factory` consumes credentials
through `coco_inference::ProviderCredentialResolver` (implemented by
`AuthService`). Adding a provider = a new `OAuthFlowDescriptor` const in
`descriptor.rs` (pure data: grant kind, encodings, refresh rotation,
account-id source) + wiring in the provider crate — no new engine code.
`OAuthFlowId` lives in `coco-types`; wired flows: `OpenAiChatGpt`,
`GeminiCodeAssist` (persistent refresh token, client_secret, userinfo email),
`XaiGrok` (device code; principal pair re-sent on refresh to keep a Team/Org
login).

## Invariants

- **Keyed by provider-INSTANCE name** (the `providers.<name>` config key), not
  by flow: two instances of one flow each get their own `TokenCell`, store
  entry, and refresher. `cell_for` refuses a name whose stored credential
  belongs to a different flow.
- **One `AuthService` per process** (`shared_auth_service` in
  `app/agent-host` `integrations/provider_login.rs`): one cell + one
  serialized refresher per instance, so a rotating single-use refresh token is
  never double-spent.
- **Never replace a `TokenCell`** — refresh AND re-login `store()` into the
  same arc-swap cell, so supplier closures handed out at provider construction
  keep serving live credentials without a client rebuild.
- **Refresh is double-locked and double-checked**: per-instance tokio
  `Semaphore` (in-process) + advisory file lock under `<config>/auth/`
  (`process_lock.rs`, cross-process); `login`/`import`/`logout` take the same
  locks. After acquiring, durable store state is re-read and adopted before
  deciding to refresh, so concurrent triggers collapse to one token exchange.
  `refresh_now` (reactive, after a 401) refreshes only if the rejected access
  token is still current.
- `login_epoch` bumps only on `login`/`import` (identity change) and is
  carried through refresh on the live snapshot.
- The background refresher holds only a `Weak<AuthService>`, wakes ~60s before
  expiry, exits on logout / terminal `SessionExpired` / service drop, and
  backs off on failure so it can never busy-spin.

## Storage

`CredentialBackend`: `FileBackend` (`<auth_dir>/<name>.json`, 0600, atomic
temp+rename), `KeyringBackend`, `AutoBackend` (keyring first, file fallback —
on save exactly ONE backend holds the credential), `EphemeralBackend`. Default
selection is by **build provenance**: `COCO_BUILD_OFFICIAL=1` (release
workflow only, via `build.rs`) → `AutoBackend`; any locally-built binary
(`--release` included) → file-only, because macOS keychain ACLs are
code-signature-keyed and a local build reading a release-created item pops a
modal prompt (this hung headless PTY e2e tests). An explicit
`CredentialStoreMode` overrides the heuristic.

## Security conventions

- Tokens are `<redacted>` in `Debug`; token-endpoint error bodies are never
  logged — only a sanitized RFC 6749 `error` identifier (bodies can echo the
  submitted refresh token).
- `COCO_AUTH_*` endpoint env overrides (wiremock seam) are honored only in
  debug builds, so a release binary cannot be redirected via environment.
- Provider-instance names are validated as flat slugs before any path join;
  `import.rs` (adopt an external codex `auth.json`) is explicit-path only and
  rejects symlinks/non-regular files before reading.

Error tier 3: snafu + `coco-error` (`ProviderAuthError`). `SessionExpired` =
refresh token dead, re-login required; the refresher treats it as terminal.
