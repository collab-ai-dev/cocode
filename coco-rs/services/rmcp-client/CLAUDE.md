# coco-rmcp-client

MCP client over the official `rmcp` SDK: stdio + streamable-HTTP transports,
session recovery, and per-server OAuth login/persistence.

## Seam

`coco-mcp` owns coco business logic (server lifecycle, config scopes,
discovery caching, naming, channel permissions) and delegates all
protocol/transport concerns here: the client state machine, transport
construction, session recovery, the OAuth persistor, and elicitation plumbing.
The public API speaks `coco-mcp-types` DTOs; `rmcp` model types are converted
at the boundary by serde round-trip (`utils::convert_to_rmcp/convert_to_mcp` —
both type sets derive from the same MCP spec). The only deliberate `rmcp`
re-exports are `ElicitationAction` and the `Elicitation`/`ElicitationResponse`
aliases; keep other `rmcp` types out of public signatures.

## Client state machine (`rmcp_client.rs`)

`ClientState::Connecting { PendingTransport }` → `initialize()` handshake →
`Ready { RunningService, Option<OAuthPersistor> }`. `TransportRecipe` keeps
the constructor args so the transport can be rebuilt for recovery.

- Transports: stdio child process (`env_clear` + `DEFAULT_ENV_VARS` allowlist
  + configured vars; `program_resolver` fixes Windows `.cmd`/`.bat`
  resolution; child stderr is forwarded to logs) and streamable HTTP via the
  custom `SessionAwareHttpClient`, wrapped in rmcp's `AuthClient` when stored
  OAuth tokens exist.
- Session recovery: `SessionAwareHttpClient` maps a 404-with-session-id to the
  typed `SessionExpired404`; `run_service_operation` detects it by downcast
  (no string matching), rebuilds the transport from the recipe, re-handshakes,
  and retries the operation once. `session_recovery_lock` + `Arc::ptr_eq`
  dedupe concurrent recoveries.
- Elicitation flows through the injected `SendElicitation` callback in
  `LoggingClientHandler`, which also bumps `progress_epoch` on progress
  notifications (a liveness signal callers can poll).

## OAuth (per MCP server — distinct from `coco-provider-auth`)

- `OAuthPersistor` snapshots rmcp's `AuthorizationManager` credentials after
  every operation (`persist_if_needed`) and proactively refreshes near-expiry
  tokens before each call (`refresh_if_needed`).
- Storage: `OAuthCredentialsStoreMode` Auto/File/Keyring, keyed by server
  name + URL; keyring first, falling back to `<config_home>/.credentials.json`.
- Interactive login: `perform_oauth_login` (loopback callback) /
  `perform_oauth_login_return_url` (hands the authorize URL + a redirect-URL
  submitter to the UI). `determine_streamable_http_auth_status` probes RFC
  8414 well-known metadata → `McpAuthStatus`
  (BearerToken/OAuth/NotLoggedIn/Unsupported).

## Gotchas

- reqwest is pinned to 0.13 here (rmcp 1.7 requires it) while the workspace is
  on 0.12; it never crosses this crate's boundary, so the versions coexist.
- Errors: thiserror `RmcpClientError` that also implements
  `coco_error::ErrorExt`/`StackError` for `StatusCode` classification.
  `http_status()` exposes the exact HTTP status for 401/403 re-auth recovery;
  `is_retryable_discovery_error` gates discovery retries.
- `src/bin/` holds three test MCP servers (stdio + streamable HTTP) used by
  integration tests — not shipped functionality.
