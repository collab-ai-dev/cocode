# coco-inference — Crate Plan

TS source: `services/api/claude.ts` (3419 LOC), `services/api/withRetry.ts` (550 LOC), `services/api/filesApi.ts` (748 LOC), `services/api/bootstrap.ts` (141 LOC), `services/api/dumpPrompts.ts` (227 LOC), `utils/auth.ts` (2002 LOC), `services/oauth/`, `services/tokenEstimation.ts`, `services/claudeAiLimits.ts`, `services/rateLimitMessages.ts`

## Dependencies

```
coco-inference depends on:
  - coco-types   (TokenUsage, ModelUsage, ProviderApi, ModelSpec, Capability)
  - coco-config  (ProviderConfig, ModelInfo, ModelRoles, RuntimeConfig, RedactedSecret,
                  PositiveTokens, ProviderClientOptions — for build_call_options +
                  ProviderClientFingerprint)
  - coco-error   (ApiError)
  - vercel-ai-provider (LanguageModelV4 trait)
  - vercel-ai-anthropic, vercel-ai-openai, vercel-ai-google, etc. (provider impls)
  - reqwest, tokio (HTTP, async)
  - blake3 (ProviderClientFingerprint digest)

coco-inference does NOT depend on:
  - coco-tool, coco-tools (no tool knowledge)
  - coco-messages (no message history)
  - any app/ crate
```

## Modules

```
coco-inference/src/
  client.rs               # ApiClient wrapper around LanguageModelV4 (carries fingerprint)
  fingerprint.rs          # ProviderClientFingerprint — turn-boundary coherence check + runtime_state_digest
  build_call_options.rs   # build_call_options(), build_call_options_with_extra() — Layer 2 per-request builder
  cache_convert.rs        # Pass-through emission of cacheStrategy / requestedBetas / agenticQuery / querySource
  cache_detection.rs      # CacheBreakDetector + 5%/2_000-token TS-parity threshold
  thinking_convert.rs     # ThinkingLevel → flat extra_body keys (camelCase, per provider)
  query.rs                # queryWithStreaming(), queryWithoutStreaming()
  retry.rs                # Generic exponential backoff + auth retry + persistent mode
  errors.rs               # Error classification (retryable, auth, prompt-too-long)
  usage.rs                # TokenUsage accumulation (ModelUsage from coco-types)
  files_api.rs            # File upload/download API (500MB limit, retry, path security)
  dump_prompts.rs         # Non-blocking debug trace for API requests/responses
  logging.rs              # Per-request logging (usage, TTFT, latency)
  token_estimation.rs     # Token counting for budget decisions
  bootstrap.rs            # Lazy-fetch org-specific config (model options, client data)
  http.rs                 # Auth headers, user-agent, OAuth retry wrapper
```

Provider concerns (auth, OAuth, beta headers, prompt-cache breakpoints, 529 retry,
rate-limit messaging, Claude.ai policy limits) live in `vercel-ai-<provider>` —
not here. See workspace `CLAUDE.md` "Multi-Provider Boundaries".

## Data Definitions

### Client (vercel-ai based, unlike TS's @anthropic-ai/sdk)

```rust
/// Unlike TS which uses @anthropic-ai/sdk directly, Rust uses vercel-ai trait
/// for multi-provider support (Anthropic, OpenAI, Google, etc.).
///
/// Carries a `ProviderClientFingerprint` so QueryEngine can detect at turn
/// boundaries when role-rebinding via hot-reload requires a fresh client
/// (multi-provider-plan.md §11.1).
pub struct ApiClient {
    model:       Arc<dyn LanguageModelV4>,   // from vercel-ai-provider
    fingerprint: ProviderClientFingerprint,
    config:      ApiClientConfig,
}

impl ApiClient {
    pub fn fingerprint(&self) -> &ProviderClientFingerprint { &self.fingerprint }
}

/// Construction-time config, captured for retry/logging visibility.
/// Per-call concerns live in `LanguageModelV4CallOptions`, NOT here.
pub struct ApiClientConfig {
    pub provider:       String,                  // = ProviderConfig.name
    pub max_retries:    i32,                      // default: 10
    pub custom_headers: BTreeMap<String, String>, // ordered, snapshot-stable
    pub proxy:          Option<ProxyConfig>,
}
```

### `ProviderClientFingerprint` — turn-boundary coherence

```rust
/// Identity for the cached `Arc<dyn LanguageModelV4>` inside `ApiClient`.
/// QueryEngine compares (current vs. cached) at turn start; mismatch → rebuild.
/// Atomic with `tool_overrides` because both come from the same
/// `Arc<RuntimeConfig>` snapshot captured at turn start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderClientFingerprint {
    pub provider:               String,         // ProviderConfig.name
    pub api:                    ProviderApi,
    pub api_model_name:         String,         // resolved per-(provider, model)
    pub base_url:               String,
    pub wire_api:               WireApi,
    pub client_options_digest:  [u8; 32],       // blake3 over typed ProviderClientOptions
    pub timeout_secs:           i64,
    pub api_key_origin_digest:  [u8; 32],       // detects rotated keys; non-reversible
}

impl ProviderClientFingerprint {
    /// Compute from the live RuntimeConfig + role binding. Reads RedactedSecret
    /// values via `.expose()` only inside the digest hasher; never copies the
    /// raw secret elsewhere.
    pub fn compute(rc: &RuntimeConfig, role: ModelRole) -> Result<Self, ConfigError>;
}
```

### Query Parameters

```rust
pub struct QueryParams {
    pub prompt: LlmPrompt,  // LanguageModelV4Prompt from coco-types (not raw MessageParam)
    pub model: String,
    pub max_tokens: i64,
    pub system: Option<SystemPrompt>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub tool_choice: Option<ToolChoice>,
    pub temperature: Option<f64>,
    pub thinking_level: Option<ThinkingLevel>,  // unified: replaces both TS effort + thinking
    pub custom_headers: HashMap<String, String>,  // beta headers
    pub enable_prompt_caching: bool,
    pub fast_mode: bool,
    pub task_budget: Option<TaskBudget>,
    pub output_format: Option<OutputFormat>,
}

// ThinkingLevel and ReasoningEffort are defined in coco-types. Not redefined here.
// TaskBudget is defined in coco-types (shared with coco-query).
```

### Adaptive Thinking & Effort Resolution (from `utils/thinking.ts`, `utils/effort.ts`)

```rust
/// Adaptive thinking: only Opus 4.6 + Sonnet 4.6 (allowlist).
/// Runtime override via 3P model capability override for custom providers.
pub fn model_supports_adaptive_thinking(model: &str) -> bool;

/// Effort resolution chain (first wins):
/// 1. CLAUDE_CODE_EFFORT_LEVEL env var (unless "unset"/"auto")
/// 2. appState.effort_value (user-set via /effort command)
/// 3. Per-model defaults (Opus 4.6 Pro/Max/Team → "medium")
/// Returns None → resolves to "high" at API layer.
pub fn resolve_applied_effort(
    model: &str,
    app_state_effort: Option<EffortValue>,
) -> Option<EffortValue>;

/// Numeric effort (0-255 ANT scale, ant-only):
///   0-50 → Low, 50-85 → Medium, 85-100 → High, >100 → Max
/// Non-ants get "high" fallback.
pub fn convert_effort_value_to_level(value: EffortValue) -> EffortLevel;
```

### `thinking_convert::to_extra_body`

```rust
/// Reasoning support — typed `ThinkingLevel` → flat camelCase keys that match
/// each Layer-3 provider's typed-options struct (multi-provider-plan.md §7.4).
///
/// Output is provider-neutral flat keys — the same shape a user would write
/// directly into `models.json::extra_body`. There is no separate code path
/// for "typed thinking" vs "user extras"; they share the merge in
/// `build_call_options`.
///
/// | Provider   | extra_body keys produced                                              |
/// |------------|------------------------------------------------------------------------|
/// | Anthropic  | { "thinking": { "type": "enabled", "budgetTokens": <n> } }              |
/// | OpenAI     | { "reasoningSummary": "auto", "include": ["reasoning.encrypted_content"]}|
/// | Google     | { "thinkingConfig": { "includeThoughts": true, "thinkingBudget": <n> } } |
/// | OpenAI-cmpt| { "reasoningEffort": "high" }                                           |
pub mod thinking_convert {
    /// Convert ThinkingLevel to provider-specific flat camelCase keys.
    /// `provider_name` selects the per-provider mapping.
    pub fn to_extra_body(
        level:         &ThinkingLevel,
        provider_name: &str,
    ) -> BTreeMap<String, JSONValue>;

    /// Map ReasoningEffort to vercel-ai ReasoningLevel enum.
    /// Used by `build_call_options` for the `LanguageModelV4CallOptions.reasoning` field.
    pub fn effort_to_reasoning_level(effort: ReasoningEffort) -> Option<ReasoningLevel>;
}
```

### `build_call_options` — Layer 2 per-request builder

```rust
/// Construct a fresh `LanguageModelV4CallOptions` per turn. This is the boundary
/// where Layer 1's flat `extra_body` is wrapped under `ProviderConfig.name` for
/// Layer 3 (multi-provider-plan.md §7).
///
/// Single point of `ProviderOptions.0` write across the entire codebase.
pub fn build_call_options(
    info:          &ModelInfo,            // already merged through L0/L1/L2
    provider_name: &str,                  // = ProviderConfig.name
    per_call:      &PerCallOverrides,     // CLI/TUI runtime overrides for this turn
    prompt:        LlmPrompt,
    tools:         Option<Vec<LanguageModelV4Tool>>,
) -> LanguageModelV4CallOptions;

/// Per-turn overrides assembled from CLI flags, TUI commands, and active state.
#[derive(Debug, Clone, Default)]
pub struct PerCallOverrides {
    pub temperature:        Option<f32>,
    pub top_p:              Option<f32>,
    pub top_k:              Option<PositiveCount>,
    pub max_output_tokens:  Option<PositiveTokens>,
    pub thinking_level:     Option<ThinkingLevel>,
    pub extra_body:         BTreeMap<String, JSONValue>,   // wins over info.extra_body
}
```

Implementation contract (no `as u64` casts; deterministic merge order):

```rust
let mut call = LanguageModelV4CallOptions::new(prompt);
call.tools = tools;
// PositiveTokens/PositiveCount → u64 via infallible `From`.
call.temperature       = per_call.temperature.or(info.temperature);
call.top_p             = per_call.top_p     .or(info.top_p);
call.top_k             = per_call.top_k     .or(info.top_k).map(u64::from);
call.max_output_tokens = per_call.max_output_tokens
                            .or(Some(info.max_output_tokens))
                            .map(u64::from);

let thinking = per_call.thinking_level.as_ref().or(info.default_thinking());
if let Some(t) = thinking { call.reasoning = Some(t.effort.into()); }

let mut extra = info.extra_body.clone();                              // BTreeMap
for (k, v) in &per_call.extra_body { extra.insert(k.clone(), v.clone()); }
if let Some(t) = thinking {
    for (k, v) in thinking_convert::to_extra_body(t, provider_name) {
        extra.insert(k, v);
    }
}
if !extra.is_empty() {
    let mut po = ProviderOptions::default();
    po.set(provider_name, extra);                                     // Layer 2 wrap
    call.provider_options = Some(po);
}
call
```

### Resolution Flow

```
User /effort high
  → ModelInfo.resolve_thinking_level(ThinkingLevel::high())
    → returns full ThinkingLevel { effort: High, options: { "reasoningSummary": "auto" } }
  → PerCallOverrides.thinking_level = Some(resolved_level)
  → build_call_options(info, &provider_cfg.name, &per_call, prompt, tools):
    → Lane A:  temperature/top_p/top_k/max_output_tokens (typed)
    → Lane A2: thinking.effort  → call.reasoning
    → Lane B:  info.extra_body ∪ per_call.extra_body ∪ thinking_convert(...)
               → wrapped under provider_options[<provider_cfg.name>]
  → call_options handed to model.do_generate(call_options)
  → Layer 3 (vercel-ai provider) extracts provider_options[self.provider()],
    parses typed-known keys + shallow-merges leftover keys into wire body
    (multi-provider-plan.md §7.3).
```

### Query Options (from `claude.ts`, 3419 LOC)

```rust
/// Full query options (matches TS Options type)
pub struct QueryOptions {
    pub model: String,
    pub tool_choice: Option<ToolChoice>,
    pub is_non_interactive_session: bool,
    pub extra_tool_schemas: Vec<ToolDefinition>,
    pub max_output_tokens_override: Option<i64>,
    pub fallback_model: Option<String>,
    pub callback_tx: Option<mpsc::UnboundedSender<QueryCallback>>,  // replaces FnOnce trait object
    pub query_source: QuerySource,
    pub agents: Vec<AgentDefinition>,
    pub allowed_agent_types: Option<Vec<String>>,
    pub has_append_system_prompt: bool,
    pub enable_prompt_caching: bool,
    pub skip_cache_write: bool,
    pub temperature_override: Option<f64>,
    pub thinking_level: Option<ThinkingLevel>,
    pub mcp_tools: Vec<ToolDefinition>,
    pub has_pending_mcp_servers: bool,
    pub fast_mode: bool,
    pub advisor_model: Option<String>,
    pub task_budget: Option<TaskBudget>,
    pub output_format: Option<OutputFormat>,
}

/// Streaming vs non-streaming selection:
/// - queryModelWithStreaming(): yields StreamEvent/AssistantMessage/SystemAPIErrorMessage
/// - queryModelWithoutStreaming(): collects single AssistantMessage
/// - Fallback: on streaming 529, switches to non-streaming with adjusted max_tokens
/// - Timeout: 120s (CCR remote) or 300s (local), overridable via API_TIMEOUT_MS
/// Returns a stream of events. Stream items are Result to handle mid-stream errors
/// (auth expiry, connection reset, context overflow detected mid-response).
pub async fn query_model_with_streaming(
    options: &QueryOptions,
    messages: &[Message],
    system: &SystemPrompt,
    tools: &[ToolDefinition],
) -> Result<impl Stream<Item = Result<StreamEvent, ApiError>>, ApiError>;

pub async fn query_model_without_streaming(
    options: &QueryOptions,
    messages: &[Message],
    system: &SystemPrompt,
    tools: &[ToolDefinition],
) -> Result<AssistantMessage, ApiError>;
```

### Retry Configuration (from `withRetry.ts`, 550 LOC)

```rust
pub struct RetryConfig {
    pub max_retries: i32,                // default: 10
    pub base_delay_ms: i64,              // 500ms exponential base
    pub max_529_retries: i32,            // 3 consecutive before fallback
    pub persistent_max_backoff_ms: i64,  // 5 min (300_000)
    pub persistent_reset_cap_ms: i64,    // 6 hours (21_600_000)
}

pub struct RetryContext {
    pub max_tokens_override: Option<i64>,
    pub model: String,
    pub thinking_level: Option<ThinkingLevel>,
    pub fast_mode: bool,
}

/// Two-layer retry engine:
///
/// Layer 1 — Standard retry (exponential backoff):
///   Non-retriable (throw immediately):
///     404, 405, 408, 410, 413, 418, 429+ (after max retries)
///     User abort (APIUserAbortError)
///     Non-foreground 529 errors (background queries bail early)
///   Retriable (retry with backoff):
///     5xx errors, 429 rate limits, 529 overloaded
///     Connection errors (ECONNRESET, EPIPE) — disable keep-alive on retry
///
/// Layer 2 — Auth-aware retry:
///     401 API key invalid → clear cache, get fresh client
///     401/403 OAuth token revoked → handleOAuth401Error() → refresh → retry
///     Bedrock auth error → clear AWS credentials cache
///     Vertex auth error → trigger GCP credential refresh
///
/// Fast-mode aware:
///     On 429/529 with fast_mode active:
///     - Retry-After <= 60s: wait and retry (preserve prompt cache)
///     - Retry-After > 60s: trigger fast-mode cooldown (switch model)
///
/// Persistent retry (ANT-only, UNATTENDED_RETRY):
///     Indefinite retry on 429/529
///     Backoff capped at 5 min, reset cap at 6 hours
///     Chunked sleep (heartbeat yields every 30s for session keep-alive)
///
/// Context overflow fallback:
///     400 "context window exceeded" → reduce max_tokens for retry
///     Ensures >= 3000 output tokens + thinking budget
pub async fn with_retry<F, Fut, T>(
    f: F,
    config: RetryConfig,
    cancel: CancellationToken,
) -> Result<T, CannotRetryError>
where
    F: Fn(&RetryContext) -> Fut,
    Fut: Future<Output = Result<T, ApiError>>;
```

### Error Classification (from `errors.ts`)

```rust
pub enum ApiErrorKind {
    RateLimit,         // 429
    Overloaded,        // 529
    PromptTooLong,     // 400 with specific message
    MediaSizeError,    // 400 with media size message
    AuthError,         // 401, 403
    BadRequest,        // 400 (other)
    ServerError,       // 500, 502, 503
    ConnectionError,   // timeouts, ECONNRESET
    UserAbort,         // User cancelled
    ContextOverflow,   // 400 "context window exceeded"
}

pub fn classify_error(status: u16, body: &str) -> ApiErrorKind;
pub fn is_retryable(kind: &ApiErrorKind) -> bool;
pub fn parse_prompt_too_long_tokens(msg: &str) -> Option<(i64, i64)>;  // (actual, limit)
```

### Token Usage (from `usage.ts`)

```rust
// TokenUsage, ModelUsage — imported from coco-types (not redefined here)

pub fn accumulate_usage(total: &mut ModelUsage, delta: &TokenUsage);
pub fn detect_gateway(headers: &HeaderMap, base_url: &str) -> Option<KnownGateway>;
// Known gateways: litellm, helicone, portkey, cloudflare, kong, braintrust, databricks
```

## Authentication System (from `auth.ts`, 2002 LOC)

### API Key Sources (Priority Order)

```rust
pub enum ApiKeySource {
    AnthropicApiKey,     // ANTHROPIC_API_KEY env var
    FileDescriptor,      // CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR
    ApiKeyHelper,        // Async cached helper with TTL + SWR
    Keychain,            // macOS Keychain or ~/.coco/.globalConfig
}

/// Validates format: alphanumeric + dashes/underscores only
pub async fn get_api_key() -> Option<(String, ApiKeySource)>;
```

### OAuth Token Management

```rust
pub struct ApiOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub scopes: Vec<String>,
    pub subscription_type: Option<SubscriptionType>,
    pub rate_limit_tier: Option<String>,
}

pub enum SubscriptionType { Max, Pro, Enterprise, Team }

/// Primary source: CLAUDE_CODE_OAUTH_TOKEN env var (inference-only, no refresh)
/// Secondary: OAuth file descriptor (CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR)
/// Fallback: Secure storage (macOS Keychain or .credentials.json)
/// Cached via memoize — cleared on disk mutation detection
pub async fn get_oauth_tokens() -> Option<ApiOAuthTokens>;
```

### Token Refresh Pipeline

```rust
/// THREAD SAFETY: Uses fs2::FileExt for cross-process file locks + tokio::sync::Notify
/// for in-process deduplication. Both are needed because:
/// - File lock: prevents thundering herd across CLI processes (multi-tab)
/// - Notify: prevents redundant refresh within same process (multi-task)
///
/// Flow (atomic check-then-refresh under lock):
/// 1. Check in-flight dedup: if another task refreshing same token, await its Notify
/// 2. Check local expiration (skip if force=true)
/// 3. Acquire file lock (~/.coco/.token.lock via fs2::try_lock_exclusive)
///    - Max 5 retries with 1-2s exponential backoff if locked by another process
/// 4. Re-read from storage under lock (detect other-process refreshes)
/// 5. If token changed since step 2 → another process refreshed, reuse it
/// 6. Otherwise call refresh_oauth_token() with optional scope override
/// 7. Store refreshed tokens; clear caches; release lock; notify waiters
///
/// In-process dedup implementation:
///   static IN_FLIGHT: Lazy<Mutex<HashMap<String, Arc<Notify>>>> = ...;
pub async fn check_and_refresh_token_if_needed(force: bool) -> Result<(), AuthError>;

/// 401 error handling (atomic check-then-refresh to avoid TOCTOU):
/// 1. Acquire file lock (same lock as refresh pipeline)
/// 2. Re-read from storage under lock
/// 3. If stored token != failed_token → another process already refreshed (reuse)
/// 4. If stored token == failed_token → force refresh under same lock hold
/// 5. Release lock; notify in-process waiters
/// Key: steps 2-4 are under single lock hold (no TOCTOU gap)
pub async fn handle_oauth_401_error(failed_token: &str) -> Result<(), AuthError>;
```

### AWS/GCP Auth Refresh

```rust
/// Run user-provided commands for interactive auth.
/// Both check STS caller-identity first; skip refresh if valid.
/// 3-minute timeout; streams output in real-time.
/// Credentials cached with TTL (AWS: 1h, GCP: 1h).
/// Security: Check workspace trust before executing project-settings commands.
pub async fn aws_auth_refresh(command: &str) -> Result<AwsCredentials, AuthError>;
pub async fn gcp_auth_refresh(command: &str) -> Result<GcpCredentials, AuthError>;

pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}
```

### OAuth 2.0 PKCE Flow (from `services/oauth/` — client.ts 18K LOC, auth-code-listener.ts 6.6K LOC, crypto.ts 566 LOC)

```rust
/// 7-step OAuth 2.0 PKCE flow:
/// 1. Generate PKCE: code_verifier (random 43-128 chars) + code_challenge (SHA-256)
/// 2. Build authorization URL with: client_id, redirect_uri, scope, state, code_challenge
/// 3. Open browser (automatic) OR display URL (manual fallback)
/// 4. Start localhost callback server (auth-code-listener) on random port
/// 5. Wait for authorization code redirect
/// 6. Exchange code + code_verifier → access_token + refresh_token
/// 7. Store tokens in secure storage (macOS Keychain or .credentials.json)
///
/// Profile fetch: After token exchange, fetch user profile for subscription type + rate limit tier.
/// Scope: "user:inference", "user:profile" (optional: "org:read")
pub async fn perform_oauth_flow() -> Result<ApiOAuthTokens, AuthError>;

/// Authorization code callback server.
/// Listens on localhost:{random_port}/callback.
/// Returns auth code to caller, then shuts down.
pub async fn start_auth_code_listener() -> Result<String, AuthError>;

/// PKCE crypto (from crypto.ts).
pub fn generate_code_verifier() -> String;    // 43-128 random alphanumeric
pub fn generate_code_challenge(verifier: &str) -> String;  // SHA-256 → base64url
```

### HTTP Utilities (from `http.ts`, 137 LOC)

```rust
/// Auth header selection:
/// - OAuth subscriber: Authorization: Bearer {token} + anthropic-beta: OAUTH_BETA
/// - API key user: x-api-key: {key}
pub fn get_auth_headers() -> AuthHeaders;

/// Retry on 401/403 with token refresh:
pub async fn with_oauth_401_retry<T>(
    request: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError>;

/// User-Agent format:
/// "claude-cli/{VERSION} ({USER_TYPE}, {ENTRYPOINT}, agent-sdk/{VERSION}, client-app/{APP})"
pub fn build_user_agent() -> String;
```

## File Upload/Download API (from `filesApi.ts`, 748 LOC)

### Configuration

```rust
pub struct FilesApiConfig {
    pub oauth_token: String,
    pub base_url: String,       // Default: https://api.anthropic.com
    pub session_id: String,
}
```

### Download Pipeline

```rust
/// Single file download with 3 retry attempts (500ms → 1s → 2s backoff).
/// Non-retriable: 404 (not found), 401 (auth), 403 (access denied)
/// Retriable: 5xx, timeouts, connection errors
/// Timeout: 60 seconds per attempt
pub async fn download_file(
    config: &FilesApiConfig,
    file_id: &str,
    dest_path: &Path,
) -> Result<DownloadResult, FilesApiError>;

/// Batch download with concurrency pool (default: 5 workers).
/// Worker pool pattern: spawns min(concurrency, files.len()) workers.
/// Maintains input order in results.
pub async fn download_session_files(
    config: &FilesApiConfig,
    files: &[FileReference],
    concurrency: usize,
) -> Vec<DownloadResult>;

/// Path security: canonicalize then verify prefix (prevents symlink + ".." escapes).
/// Steps:
/// 1. Join base + session_id + "uploads" + relative
/// 2. Canonicalize result (resolves symlinks, "..", ".")
/// 3. Canonicalize base separately
/// 4. Verify canonicalized result starts_with canonicalized base
/// 5. Reject on mismatch (PathError::Traversal)
pub fn build_download_path(base: &Path, session_id: &str, relative: &str) -> Result<PathBuf, PathError>;
```

### Upload Pipeline

```rust
/// Single file upload via multipart form-data.
/// Validation: size <= 500MB
/// Boundary: UUID (avoid collisions)
/// Retry: 3 attempts (500ms exponential backoff)
/// Non-retriable: 401, 403, 413 (file too large)
/// Timeout: 120 seconds (for large files)
pub async fn upload_file(
    config: &FilesApiConfig,
    path: &Path,
) -> Result<UploadResult, FilesApiError>;

/// Batch upload with same concurrency pool pattern.
pub async fn upload_session_files(
    config: &FilesApiConfig,
    files: &[PathBuf],
    concurrency: usize,
) -> Vec<UploadResult>;

/// List files created after a timestamp, with pagination.
/// Uses after_id cursor when has_more=true.
pub async fn list_files_created_after(
    config: &FilesApiConfig,
    after: DateTime<Utc>,
) -> Result<Vec<FileInfo>, FilesApiError>;
```

## Debug Prompt Dump (from `dumpPrompts.ts`, 227 LOC)

```rust
/// Non-blocking debug trace for API requests/responses.
/// Used by /issue command for diagnostics.
///
/// Architecture:
/// - createDumpPromptsFetch() wraps fetch function
/// - Deferred via spawn_blocking so parsing doesn't block API calls
/// - Cheap fingerprint first (model + tool names + system size)
/// - Skip full hash if fingerprint unchanged
///
/// File layout: ~/.coco/dump-prompts/{session_id}.jsonl
/// Entry types: init, system_update, message, response
pub fn create_dump_prompts_fetch(session_id: &str) -> impl Fn(Request) -> Future<Output = Response>;
```

## Bootstrap (from `bootstrap.ts`, 141 LOC)

```rust
/// Lazy-fetch org-specific config (model options, client data).
/// Skip conditions: privacy mode, 3rd-party provider, no usable auth, missing user:profile scope
/// Endpoint: {BASE_API_URL}/api/claude_cli/bootstrap
/// Timeout: 5 seconds
/// Caching: persists to ~/.coco/config.json, only writes if data changed (deep equality)
/// Auth: OAuth (requires user:profile scope) > API key fallback
/// Uses withOAuth401Retry() for automatic token refresh
pub async fn fetch_bootstrap_config() -> Option<BootstrapConfig>;

pub struct BootstrapConfig {
    pub client_data: Option<Value>,
    pub additional_model_options: Option<Vec<AdditionalModel>>,
}

pub struct AdditionalModel {
    pub value: String,        // model ID
    pub label: String,        // display name
    pub description: String,
}
```

## Policy Limits (from `services/policyLimits/`, P1 gap)

```rust
/// Background-polled organization policy limits.
/// Polling interval: 1 hour (3,600,000 ms), setInterval with .unref()
/// ETag cache: SHA256 checksum of normalized restrictions → If-None-Match header
/// Disk cache: ~/.coco/policy-limits.json (0o600 permissions)
/// Session-level cache supplements disk cache for redundancy
///
/// Eligibility:
///   - Console users (API key): always eligible
///   - OAuth users: only Team and Enterprise subscribers with CLAUDE_AI_INFERENCE_SCOPE
///   - No 3P providers, no custom base URLs
///
/// Fail-open: returns true (allowed) if restrictions unavailable,
/// EXCEPT 'allow_product_feedback' which fails closed in essential-traffic-only mode.
pub struct PolicyLimitsManager {
    restrictions: Arc<RwLock<Option<HashMap<String, PolicyRestriction>>>>,
    etag: Arc<RwLock<Option<String>>>,
}

pub struct PolicyRestriction {
    pub allowed: bool,
}

impl PolicyLimitsManager {
    pub fn is_policy_allowed(&self, policy: &str) -> bool;
    pub fn is_eligible(&self) -> bool;
    pub async fn load(&self);          // called during CLI init
    pub async fn refresh(&self);       // on auth state change
    pub async fn wait_for_load(&self); // for sync code
}
```

## Auth Helpers (expanded from `utils/auth.ts`, P1 gap)

### API Key Helper

```rust
/// Command-based API key with SWR (stale-while-revalidate) pattern.
/// TTL: 5 minutes (default), configurable via CLAUDE_CODE_API_KEY_HELPER_TTL_MS
/// Cold cache: block on execution, deduplicate concurrent calls
/// Stale cache: return immediately, refresh in background (no-op on error if cache valid)
/// Execution: via shell with 10-minute timeout
/// Trust: only executes if trust accepted OR non-interactive session
/// Error sentinel: caches space ' ' on failure to prevent fallback to keychain
pub struct ApiKeyHelperCache {
    value: Option<String>,
    timestamp: i64,
    inflight: Option<JoinHandle<Option<String>>>,
    epoch: i64,  // bumped on clear to invalidate pending refreshes
}

impl ApiKeyHelperCache {
    pub async fn get(&self, is_non_interactive: bool) -> Option<String>;
    pub fn get_cached(&self) -> Option<String>;  // sync, may be stale
    pub fn prefetch_if_safe(&self);               // background warm-up
    pub fn clear(&mut self);                      // bump epoch
}
```

### Bare Mode (--bare flag)

```rust
/// Hermetic authentication — ignores most auth sources.
/// Allowed: ANTHROPIC_API_KEY env var, apiKeyHelper from --settings flag only
/// Blocked: OAuth tokens (env, FD, keychain, claude.ai), API keys from config/keychain,
///          3P auth (Bedrock/Vertex/Foundry)
/// Useful for: CI, hermetic builds, no-network scenarios
pub fn is_bare_mode() -> bool;
```

## Prompt Cache Break Detection (from `services/api/promptCacheBreakDetection.ts`, 727 LOC)

```rust
/// Anthropic prompt caching: cache_control block annotation.
/// Scope determines TTL and billing attribution.
pub enum CacheScope {
    Global,  // 5-minute TTL, standard billing
    Org,     // 1-hour TTL, org-level caching
}

/// 2-phase cache break detection algorithm.
/// Phase 1: Record state BEFORE API call.
/// Phase 2: Check response AFTER API call for cache degradation.
pub struct CacheBreakDetector {
    /// Per-source tracking (main_thread, sdk, agents).
    states: HashMap<QuerySource, CachePromptState>,
}

pub struct CachePromptState {
    // --- 16 hash dimensions tracked for cache break detection ---
    pub system_hash: u64,              // system prompt (stripped cache_control)
    pub tools_hash: u64,               // tool definitions (stripped cache_control)
    pub cache_control_hash: u64,       // WITH cache_control (catches TTL/scope flips)
    pub tool_names: Vec<String>,       // ordered tool name list
    pub per_tool_hashes: HashMap<String, u64>, // per-tool schema hash
    pub system_char_count: i64,
    pub model: String,
    pub fast_mode: bool,
    pub global_cache_strategy: CacheStrategy, // ToolBased, SystemPrompt, None
    pub betas: Vec<String>,            // sorted beta header list
    pub auto_mode_active: bool,
    pub is_using_overage: bool,        // latched session-stable
    pub cached_mc_enabled: bool,       // cache-editing beta header
    pub effort_value: Option<String>,
    pub extra_body_hash: Option<u64>,
    // --- Cache read tracking ---
    pub prev_cache_read_tokens: i64,
    pub reference_ttl: CacheTtl,
}

pub enum CacheTtl { FiveMinutes, OneHour }
pub enum CacheStrategy { ToolBased, SystemPrompt, None }

/// TTL latching: once isUsingOverage changes, cache TTL selection is latched
/// session-stable to prevent false cache breaks.
/// AutoMode sticky-on: AFK_MODE_BETA_HEADER presence latched to prevent false breaks.
///
/// Token billing:
///   cache_creation_input_tokens: counted when first cached
///   cache_read_input_tokens: counted on cache hits
///   Detection: >5% drop AND >2000 tokens absolute drop → cache break
///   Compaction exemption: sets cacheDeletionsPending flag
///
/// Per-tool schema hashing:
///   computePerToolHashes() maps tool_name → hash of stripped schema
///   MCP tools sanitized to 'mcp' to prevent filepath leakage
///   Diff: if tool exists in both old/new states, compare hashes for schema-only changes

impl CacheBreakDetector {
    /// Phase 1: Record prompt state before API call.
    /// Computes hashes of system, tools, and cache_control separately.
    pub fn record_prompt_state(
        &mut self,
        source: QuerySource,
        system: &SystemPrompt,
        tools: &[ToolDefinition],
        cache_control: &[CacheControlBlock],
    );

    /// Phase 2: Check response for cache break.
    /// Detects: 5% drop in cache_read_input_tokens → potential cache miss.
    /// Categorizes: TTL expiration (5min/1hour), system change, tools change.
    /// Emits telemetry event with break reason.
    pub fn check_response_for_cache_break(
        &mut self,
        source: QuerySource,
        usage: &TokenUsage,
    ) -> Option<CacheBreakEvent>;
}

pub struct CacheBreakEvent {
    pub source: QuerySource,
    pub reason: CacheBreakReason,
    pub cache_read_drop_pct: f64,
    pub prev_tokens: i64,
    pub curr_tokens: i64,
}

pub enum CacheBreakReason {
    TtlExpired { ttl: CacheTtl },
    SystemChanged,
    ToolsChanged,
    CacheControlChanged,
    Unknown,
}
```

### Cache-Aware Microcompact

```rust
/// Microcompact strategies that preserve prompt cache:
/// - cache_edits: Clear tool results but preserve cache_control blocks
/// - cache_deletions: Remove messages but keep cache-critical prefix intact
/// Integration: coco-compact calls back to check cache state before compacting.
```

---

## Provider Construction

There is no `ModelHub` / `ProviderFactory` in coco-rs. Provider clients are
constructed by **`app/cli/src/model_factory.rs::build_language_model_from_runtime`**
from `RuntimeConfig` (multi-provider-plan.md §6) and cached inside `ApiClient`.

`QueryEngine` enforces coherence with hot-reload via the turn-boundary
`ProviderClientFingerprint` check:

```rust
impl QueryEngine {
    fn ensure_client_for_turn(
        &mut self,
        runtime: &RuntimeConfig,
        role:    ModelRole,
    ) -> Result<()> {
        let fp = ProviderClientFingerprint::compute(runtime, role)?;
        if self.api_client.fingerprint() != &fp {
            let new_client = ApiClient::build(runtime, role, &fp)?;
            self.api_client = Arc::new(new_client);   // release the old client
        }
        Ok(())
    }
}
```

The `tool_overrides` and `api_client` reads in the same turn both come from the
same `Arc<RuntimeConfig>` snapshot, so role rebinding cannot leave them
inconsistent (`Arc::ptr_eq` test invariant in multi-provider-plan.md §15.B).

## Core Logic

### Streaming Query

```rust
/// Main entry point for LLM queries.
/// Uses vercel-ai LanguageModelV4 trait (not Anthropic SDK directly).
///
/// Input: QueryParams.prompt is LanguageModelV4Prompt (from coco-messages::to_language_model_prompt).
/// Output: Stream of vercel-ai StreamPart events.
pub async fn query_streaming(
    client: &ApiClient,
    params: QueryParams,
    cancel: CancellationToken,
) -> Result<impl Stream<Item = StreamPart>, ApiError>;

pub async fn query_non_streaming(
    client: &ApiClient,
    params: QueryParams,
    cancel: CancellationToken,
) -> Result<AssistantMessage, ApiError>;

/// Reverse path: collect vercel-ai stream into internal AssistantMessage.
/// Called by QueryEngine.process_stream().
///
/// Assembly:
///   StreamPart::TextDelta        → accumulate into AssistantContentBlock::Text
///   StreamPart::ToolCallDelta    → accumulate into AssistantContentBlock::ToolUse (by id)
///   StreamPart::ReasoningDelta   → accumulate into AssistantContentBlock::Thinking
///   StreamPart::FinishReason     → set stop_reason
///   StreamPart::Usage            → extract TokenUsage
pub fn collect_stream_to_message(
    stream: impl Stream<Item = StreamPart>,
    event_tx: &mpsc::Sender<coco_types::CoreEvent>,
) -> (AssistantMessage, TokenUsage);
```

### Rate Limiting

```rust
pub struct RateLimitState {
    pub cooldown_until: Option<Instant>,
    pub consecutive_429s: i32,
}

pub fn check_rate_limit(state: &RateLimitState) -> Result<(), RateLimitMessage>;
pub fn format_rate_limit_message(retry_after: Duration) -> String;
```
