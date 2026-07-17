# coco-otel — Crate Plan

TS source: `utils/telemetry/` (9 files, ~4K LOC), `services/analytics/` (8 files, ~4K LOC),
`utils/telemetryAttributes.ts`, `utils/stats.ts` (1061 LOC), `utils/statsCache.ts`,
`utils/log.ts`, `utils/errorLogSink.ts`, `utils/debug.ts`, `utils/sinks.ts`,
`utils/fileOperationAnalytics.ts`, `services/internalLogging.ts`,
`services/toolUseSummary/`, `services/api/logging.ts`

cocode-rs source: `common/otel/` (13 source files) — L0-L1 层直接复用

## Strategy: HYBRID (6 层模型)

cocode-rs 的 export 管道 (L0) 和基础事件 (L1) 质量优秀，直接复用。
TS 的 span 层级 (L2)、应用事件 (L3)、业务 metrics (L4)、自定义 exporter (L5) 需要从 TS 移植。
L6 运营控制 (sampling/killswitch/opt-out) 暂不实现。

| 层级 | 来源 | 状态 |
|------|------|------|
| L0: Export 管道 (OTLP gRPC/HTTP, TLS, Statsig, InMemory) | cocode-rs | **复用** |
| L1: 基础事件 (7 个核心事件: conversation_starts, user_prompt, tool_decision, tool_result, api_request, sse_event, sse_event_completed) | cocode-rs | **复用** |
| L2: Span 层级 (interaction→llm_request→tool→hook→user_input) | TS `sessionTracing.ts` | **新增** |
| L3: 应用事件 (~53 个事件类型: query/session/config/oauth/mcp) | TS `analytics/`, `api/logging.ts` | **新增** |
| L4: 业务 metrics (token/cost/LOC/session/active_time/PR/commit) | TS `instrumentation.ts` | **新增** |
| L5: 自定义 exporter (BigQuery, 1P Event Logging, Perfetto) | TS `bigqueryExporter.ts`, `firstPartyEventLogger.ts`, `perfettoTracing.ts` | **新增** |
| L6: 运营控制 (event sampling, killswitch, metrics opt-out, PII safety) | TS `sinkKillswitch.ts`, `growthbook.ts`, `config.ts` | **P1 新增** (TS production 已实现) |

## Dependencies

```
coco-otel depends on:
  - coco-error            (ErrorExt for telemetry_msg)
  - opentelemetry, opentelemetry_sdk, opentelemetry-otlp
  - tracing, tracing-subscriber, tracing-opentelemetry
  - opentelemetry-appender-tracing
  - reqwest (HTTP exporter + BigQuery)
  - serde, serde_json, chrono, tokio

coco-otel does NOT depend on:
  - coco-types            (no message/tool types — events use &str/Value)
  - coco-config           (settings passed in via OtelSettings struct)
  - coco-inference        (no LLM calls)
  - any core/app crate
```

## Data Definitions

### L0: Export 管道 (from cocode-rs, 直接复用)

```rust
/// 已有 — cocode-rs common/otel/src/config.rs
pub struct OtelSettings {
    pub environment: String,
    pub service_name: String,
    pub service_version: String,
    pub home_dir: PathBuf,
    pub exporter: OtelExporter,        // logs
    pub trace_exporter: OtelExporter,  // traces
    pub metrics_exporter: OtelExporter,// metrics
}

pub enum OtelExporter {
    None,
    Statsig,
    OtlpGrpc { endpoint: String, headers: HashMap<String, String>, tls: Option<OtelTlsConfig> },
    OtlpHttp { endpoint: String, headers: HashMap<String, String>, protocol: OtelHttpProtocol, tls: Option<OtelTlsConfig> },
}

pub enum OtelHttpProtocol { Binary, Json }

pub struct OtelTlsConfig {
    pub ca_certificate: Option<AbsolutePathBuf>,
    pub client_certificate: Option<AbsolutePathBuf>,
    pub client_private_key: Option<AbsolutePathBuf>,
}

/// 已有 — cocode-rs common/otel/src/otel_provider.rs
pub struct OtelProvider {
    pub logger: Option<SdkLoggerProvider>,
    pub tracer_provider: Option<SdkTracerProvider>,
    pub tracer: Option<Tracer>,
    pub metrics: Option<MetricsClient>,
}

/// 已有 — cocode-rs common/otel/src/metrics/client.rs
pub struct MetricsClient(Arc<MetricsClientInner>);  // Clone, thread-safe

/// 已有 — cocode-rs common/otel/src/metrics/timer.rs
pub struct Timer { ... }  // Drop impl auto-records duration
```

### L1: 基础事件 (from cocode-rs, 直接复用)

```rust
/// 已有 — cocode-rs common/otel/src/lib.rs
pub struct OtelEventMetadata {
    pub conversation_id: String,
    pub auth_mode: Option<String>,
    pub account_id: Option<String>,
    pub account_email: Option<String>,
    pub provider: String,
    pub model: String,
    pub log_user_prompts: bool,
    pub app_version: &'static str,
    pub terminal_type: String,
}

pub struct OtelManager {
    pub metadata: OtelEventMetadata,
    pub session_span: Span,
    pub metrics: Option<MetricsClient>,
    pub metrics_use_metadata_tags: bool,
}

pub enum ToolDecisionSource { Config, User }

/// 已有事件方法 (otel_manager.rs):
/// - conversation_starts()
/// - user_prompt()
/// - tool_decision()
/// - tool_result() + log_tool_result() + log_tool_failed()
/// - record_api_request() + log_request()
/// - sse_event() + sse_event_completed() + sse_event_failed()
/// - counter() + histogram() + record_duration() + start_timer()
```

### L2: Span 层级体系 (from TS `sessionTracing.ts`, 新增)

6 span types in parent-child hierarchy:

```
interaction (claude_code.interaction)          ← root, per user prompt
  ├── llm_request (claude_code.llm_request)    ← per API call
  ├── tool (claude_code.tool)                  ← per tool invocation
  │     ├── tool.blocked_on_user               ← permission wait
  │     └── tool.execution                     ← actual execution
  └── hook (claude_code.hook)                  ← per hook execution
```

```rust
/// Span 管理器 — 维护 interaction→child 的嵌套关系
/// Uses tokio task-local storage (Rust equivalent of TS AsyncLocalStorage)
/// Weak references for GC-friendly tracking; 30-min TTL with 60s eviction
pub struct SpanManager {
    interaction_span: Option<Span>,
    tool_span: Option<Span>,  // current tool (for hook parenting)
}

impl SpanManager {
    /// Root span: user submits prompt
    /// Attributes: user_prompt, user_prompt_length, interaction.sequence
    pub fn start_interaction(&mut self, user_prompt: &str, sequence: i64) -> Span;
    pub fn end_interaction(&mut self, duration_ms: i64);

    /// LLM API request (child of interaction)
    /// Start attrs: model, speed (fast/normal), query_source (agent name)
    /// End attrs: input_tokens, output_tokens, cache_read/creation_tokens,
    ///            success, status_code, error, attempt, ttft_ms, has_tool_call
    /// MUST pass specific span to end (parallel requests can overlap)
    pub fn start_llm_request(&self, model: &str, fast_mode: bool) -> Span;
    pub fn end_llm_request(span: Span, metadata: LlmRequestEndMetadata);

    /// Tool invocation (child of interaction)
    /// Attributes: tool_name, custom tool_attributes
    /// Contains nested blocked_on_user and execution sub-spans
    pub fn start_tool(&self, tool_name: &str, attributes: Option<Value>) -> Span;
    pub fn end_tool(span: Span, duration_ms: i64, result_tokens: Option<i64>);

    /// Permission wait (child of tool)
    /// End attrs: decision, source, duration_ms
    pub fn start_blocked_on_user(parent: &Span) -> Span;
    pub fn end_blocked_on_user(span: Span, decision: &str, source: &str);

    /// Tool execution time (child of tool, separate from permission time)
    /// End attrs: success, error, duration_ms
    pub fn start_tool_execution(parent: &Span) -> Span;
    pub fn end_tool_execution(span: Span, success: bool, error: Option<&str>);

    /// Hook execution (child of tool or interaction)
    /// Parent: current tool_span if exists, else interaction_span
    /// Attrs: hook_event, hook_name, num_hooks, hook_definitions
    /// End attrs: num_success, num_blocking, num_non_blocking_error, num_cancelled
    pub fn start_hook(&self, hook_name: &str, event: &str, num_hooks: i32) -> Span;
    pub fn end_hook(span: Span, metadata: HookEndMetadata);
}
```

### L3: 应用事件 (from TS `analytics/`, 新增)

```rust
/// 事件记录器 — 统一的事件发送接口
/// TS: logEvent() / logEventAsync() / logOTelEvent()
impl OtelManager {
    /// 通用事件记录 (对应 TS logEvent)
    pub fn log_event(&self, event_name: &str, metadata: &Value);

    // --- Query 生命周期 ---
    pub fn query_before_attachments(&self, ...);
    pub fn query_after_attachments(&self, ...);
    pub fn query_error(&self, error: &str, ...);
    pub fn auto_compact_succeeded(&self, tokens_before: i64, tokens_after: i64);
    pub fn model_fallback_triggered(&self, from_model: &str, to_model: &str);
    pub fn streaming_tool_execution_used(&self, used: bool);

    // --- Session 生命周期 ---
    pub fn session_init(&self, ...);
    pub fn session_started(&self, startup_ms: i64, ...);
    pub fn session_exit(&self, total_cost_usd: f64, total_turns: i64, ...);

    // --- API 成功详情 (扩展现有 record_api_request) ---
    pub fn api_success(&self, metadata: ApiSuccessMetadata);
    pub fn api_error_detailed(&self, metadata: ApiErrorMetadata);

    // --- Config 变更 ---
    pub fn config_changed(&self, key: &str, ...);
    pub fn config_model_changed(&self, old_model: &str, new_model: &str);
    pub fn thinking_toggled(&self, enabled: bool);

    // --- OAuth 流程 ---
    pub fn oauth_flow_start(&self, provider: &str);
    pub fn oauth_success(&self, provider: &str);
    pub fn oauth_error(&self, provider: &str, error: &str);

    // --- MCP 事件 ---
    pub fn mcp_start(&self, server_name: &str);
    pub fn mcp_add(&self, server_name: &str);
    pub fn mcp_error(&self, server_name: &str, error: &str);

    // --- Tool 细粒度 (扩展现有 tool_decision/tool_result) ---
    pub fn tool_cancelled(&self, tool_name: &str, call_id: &str);
    pub fn tool_progress(&self, tool_name: &str, call_id: &str, ...);
    pub fn tool_permission_allowed(&self, tool_name: &str, source: ToolDecisionSource);
    pub fn tool_permission_rejected(&self, tool_name: &str, reason: &str);
}

/// API 成功事件的完整元数据
/// TS: tengu_api_success 有 30+ 字段
pub struct ApiSuccessMetadata {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub duration_ms: i64,
    pub duration_ms_including_retries: i64,
    pub ttft_ms: i64,
    pub attempt: i64,
    pub cost_usd: f64,
    pub stop_reason: String,
    pub provider: String,
    pub request_id: Option<String>,
    pub query_source: String,
    pub fast_mode: bool,
    pub text_content_length: i64,
    pub thinking_content_length: i64,
}

/// API 错误事件的完整元数据
pub struct ApiErrorMetadata {
    pub model: String,
    pub error: String,
    pub status: Option<i32>,
    pub error_type: String,
    pub duration_ms: i64,
    pub attempt: i64,
    pub provider: String,
    pub request_id: Option<String>,
    pub client_request_id: Option<String>,
}
```

### L4: 业务 Metrics (from TS `instrumentation.ts`, 新增)

```rust
/// 业务级 metric 名称常量
/// TS: meter.createCounter("claude_code.*")
pub mod metric_names {
    pub const SESSION_COUNT: &str = "coco.session.count";
    pub const TOKEN_USAGE: &str = "coco.token.usage";
    pub const COST_USAGE: &str = "coco.cost.usage";
    pub const LINES_OF_CODE: &str = "coco.lines_of_code.count";
    pub const ACTIVE_TIME: &str = "coco.active_time.total";
    pub const PULL_REQUEST_COUNT: &str = "coco.pull_request.count";
    pub const COMMIT_COUNT: &str = "coco.commit.count";
    pub const CODE_EDIT_DECISION: &str = "coco.code_edit_tool_decision.count";
}

/// TS metric tag 键常量
pub mod tag_keys {
    pub const MODEL: &str = "model";
    pub const TOKEN_TYPE: &str = "token_type";  // input, output, cached
    pub const TOOL_NAME: &str = "tool";
    pub const SUCCESS: &str = "success";
    pub const QUERY_SOURCE: &str = "query_source";
    pub const DECISION_TYPE: &str = "decision_type";
}

impl OtelManager {
    // --- Token & Cost ---
    pub fn record_token_usage(&self, model: &str, input: i64, output: i64, cached: i64);
    pub fn record_cost(&self, model: &str, cost_usd: f64, tool_name: Option<&str>);

    // --- Code changes ---
    pub fn record_lines_of_code(&self, lines_added: i64, decision: &str);
    pub fn record_commit(&self, source: &str);
    pub fn record_pull_request(&self, source: &str);

    // --- Session ---
    pub fn record_session_start(&self);
    pub fn record_active_time(&self, ms: i64);
}
```

### L5: 自定义 Exporter (from TS, 新增)

```rust
/// BigQuery metrics exporter — 企业级 metrics 导出
/// TS: bigqueryExporter.ts → https://api.anthropic.com/api/claude_code/metrics
pub struct BigQueryExporter {
    endpoint: String,
    api_key: String,
    export_interval: Duration,  // 5 minutes
}

impl PushMetricExporter for BigQueryExporter { ... }

/// 1P Event Logging exporter — 第一方事件日志
/// TS: firstPartyEventLogger.ts → https://api.anthropic.com/api/event_logging/batch
pub struct FirstPartyEventExporter {
    endpoint: String,
    chunk_size: i64,     // 200 events per batch
    retry_config: RetryConfig,  // quadratic backoff
    disk_fallback: PathBuf,     // failed events persisted to disk
}

/// Perfetto tracing — Chrome Trace Event 格式输出
/// TS: perfettoTracing.ts → ~/.coco/traces/trace-<session-id>.json
pub struct PerfettoTracer {
    output_path: PathBuf,
    write_interval: Duration,
}

impl PerfettoTracer {
    pub fn start_interaction(&mut self, prompt: &str);
    pub fn end_interaction(&mut self);
    pub fn start_llm_request(&mut self);
    pub fn end_llm_request(&mut self);
    pub fn start_tool(&mut self, tool_name: &str);
    pub fn end_tool(&mut self);
}
```

### Beta Tracing (L5, optional)

```rust
/// Beta 详细追踪 — 捕获完整 prompt/response 内容 (截断 60KB)
/// TS: betaSessionTracing.ts → ENABLE_BETA_TRACING_DETAILED + BETA_TRACING_ENDPOINT
/// 仅在企业环境启用，发送到 Honeycomb 等后端
pub struct BetaTracer {
    enabled: bool,
    endpoint: String,
    max_content_size: usize,  // 60KB, Honeycomb limit
}

impl BetaTracer {
    pub fn add_interaction_attributes(&self, span: &Span, prompt: &str);
    pub fn add_llm_request_attributes(&self, span: &Span, context: &Value, messages: &[Value]);
    pub fn add_llm_response_attributes(&self, span: &Span, metadata: &Value);
    pub fn add_tool_input_attributes(&self, span: &Span, tool_name: &str, input: &Value);
    pub fn add_tool_result_attributes(&self, span: &Span, tool_name: &str, result: &Value);
    fn truncate_content(content: &str, max_size: usize) -> &str;
}
```

### L6: 运营控制 (from TS, P1 新增 — TS production 已全面实现)

TS has all L6 controls running in production. Previously marked "deferred" but elevating to P1.

```rust
/// Per-event sampling configuration (from GrowthBook dynamic config)
/// TS: shouldSampleEvent() in firstPartyEventLogger.ts
pub struct EventSamplingConfig {
    /// Map of event_name → sample_rate (0.0-1.0). Missing = 100%.
    pub rates: HashMap<String, f64>,
}

impl EventSamplingConfig {
    /// Returns Some(sample_rate) if event should be logged, None if dropped.
    /// Default (no config): log all events.
    pub fn should_sample(&self, event_name: &str) -> Option<f64>;
}

/// Per-sink killswitch (from GrowthBook dynamic config)
/// TS: isSinkKilled() in sinkKillswitch.ts
/// Config key: intentionally obfuscated (TS: 'tengu_frond_boric')
pub struct SinkKillswitch {
    pub datadog_killed: bool,
    pub first_party_killed: bool,
}

/// Global analytics disable conditions
/// TS: isAnalyticsDisabled() in config.ts
pub fn is_analytics_disabled() -> bool {
    // Disabled when: test env, Bedrock/Vertex/Foundry provider,
    // or telemetry disabled (no-telemetry / essential-traffic-only)
}

/// PII safety markers — type-driven enforcement
/// TS: AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS (type: never)
/// Forces explicit developer intent via type casting to prevent accidental PII logging.
///
/// _PROTO_* prefixed keys: routed to privileged BQ columns (access-controlled),
/// stripped before Datadog fanout via strip_proto_fields().
///
/// Tool name sanitization: MCP tools logged as 'mcp_tool' by default.
/// Allowed to log real name only for: local-agent mode, claude.ai proxy MCPs,
/// official registry MCPs.
///
/// Tool input truncation: strings at 512 chars (128 preview), max 4KB JSON,
/// max 20 items per level, max depth 2.
```

### L3 Event Catalog (from TS — 665 unique event names)

Key event categories (all prefixed `tengu_`):

| Category | Count | Examples |
|----------|-------|---------|
| Query & Inference | 40+ | `api_query`, `api_success`, `api_error`, `api_retry`, `api_cache_breakpoints`, `model_fallback_triggered`, `token_budget_completed` |
| Tool Usage | 25+ | `tool_use_success`, `tool_use_error`, `tool_use_cancelled`, `tool_use_can_use_tool_allowed/rejected` |
| Hooks | 7 | `pre_tool_hook_error`, `post_tool_hook_error`, `pre_stop_hooks_cancelled` |
| Permissions | 12+ | `auto_mode_decision`, `auto_mode_outcome`, `permission_request_option_selected` |
| MCP | 30+ | `mcp_start`, `mcp_server_connection_succeeded/failed`, `mcp_oauth_flow_*`, `mcp_elicitation_*` |
| Session & Memory | 30+ | `started`, `init`, `exit`, `session_resumed`, `memdir_loaded` |
| Compact | 20+ | `auto_compact_succeeded`, `compact_failed`, `sm_compact_*` |
| OAuth & Config | 30+ | `oauth_success`, `oauth_error`, `oauth_token_refresh_*`, `config_changed` |
| Agent & Teams | 20+ | `agent_tool_completed`, `team_created/deleted` |
| Bridge & Remote | 35+ | `bridge_*`, `remote_create_session_*`, `teleport_*` |
| Plugins & Skills | 25+ | `plugin_installed`, `skill_loaded`, `marketplace_*` |
| File Operations | 15+ | `file_operation`, `file_read_dedup`, `atomic_write_error` |

Note: Event count corrected from "~53" (audit R5) to 665 actual unique event names.
Datadog allowlist: ~64 events (high-cardinality subset). 1P Logger: all events.

## cocode-rs 优势保留

以下 cocode-rs 特性优于 TS，保留不改：

| 特性 | 说明 |
|------|------|
| Tag 验证 | `validate_tag_key/value()` 严格校验字符合法性，TS 无此校验 |
| Timer + Drop | scope 结束自动记录 duration，比 TS 手动 start/end 更安全 |
| Mutex poison recovery | `unwrap_or_else(PoisonError::into_inner)` 防 panic 传播 |
| 类型安全 | `ToolDecisionSource` 枚举 vs TS string literal |
| TLS 完整支持 | mTLS (CA cert + client cert + key)，TS 依赖 SDK 默认 |
| InMemory exporter | 测试专用 exporter，TS 需要 mock |
| Statsig 内置 | 直接内置 Statsig endpoint 配置 |

## Module Layout

```
common/otel/src/
  lib.rs                     # 复用 + 扩展 (add log_event, new metric methods)
  config.rs                  # 复用 (OtelSettings, OtelExporter)
  otel_provider.rs           # 复用 (OtelProvider, traceparent)
  otlp.rs                    # 复用 (OTLP exporter builders, TLS)
  metrics/
    mod.rs                   # 复用
    client.rs                # 复用 (MetricsClient)
    config.rs                # 复用 (MetricsConfig)
    error.rs                 # 复用 (MetricsError)
    timer.rs                 # 复用 (Timer)
    validation.rs            # 复用 (tag/metric name validation)
  traces/
    mod.rs                   # 复用
    otel_manager.rs          # 复用 + 扩展 (add L3 event methods)
  spans/                     # 新增: L2 span 层级
    mod.rs
    span_manager.rs          # SpanManager: interaction/llm/tool/hook/user_input spans
  events/                    # 新增: L3 应用事件
    mod.rs
    query_events.rs          # query lifecycle events
    session_events.rs        # session lifecycle events
    config_events.rs         # config change events
    oauth_events.rs          # OAuth flow events
    mcp_events.rs            # MCP events
  business_metrics.rs        # 新增: L4 业务 metrics
  exporters/                 # 新增: L5 自定义 exporter
    mod.rs
    bigquery.rs              # BigQuery exporter
    first_party.rs           # 1P event logging exporter
    perfetto.rs              # Perfetto tracing
  beta_tracing.rs            # 新增: L5 beta content tracing
  controls/                  # 新增: L6 运营控制
    mod.rs
    sampling.rs              # EventSamplingConfig, shouldSampleEvent
    killswitch.rs            # SinkKillswitch, isSinkKilled
    pii_safety.rs            # PII markers, PROTO field handling, tool name sanitization
```

## TS → Rust 文件映射

| TS 文件 | Rust 目标 | 层级 |
|---------|-----------|------|
| `utils/telemetry/sessionTracing.ts` | `spans/span_manager.rs` | L2 |
| `utils/telemetry/events.ts` | `events/mod.rs` | L3 |
| `utils/telemetry/instrumentation.ts` | `otel_provider.rs` (扩展) + `business_metrics.rs` | L0+L4 |
| `utils/telemetry/betaSessionTracing.ts` | `beta_tracing.rs` | L5 |
| `utils/telemetry/perfettoTracing.ts` | `exporters/perfetto.rs` | L5 |
| `utils/telemetry/bigqueryExporter.ts` | `exporters/bigquery.rs` | L5 |
| `utils/telemetry/logger.ts` | `otel_provider.rs` (已有 logger) | L0 |
| `services/analytics/index.ts` | `events/mod.rs` (logEvent API) | L3 |
| `services/analytics/firstPartyEventLogger.ts` | `exporters/first_party.rs` | L5 |
| `services/analytics/firstPartyEventLoggingExporter.ts` | `exporters/first_party.rs` | L5 |
| `services/analytics/sink.ts` | `events/mod.rs` (sink routing) | L3 |
| `services/analytics/datadog.ts` | `exporters/datadog.rs` | L5 |
| `services/analytics/metadata.ts` | `lib.rs` (OtelEventMetadata 已有) | L1 |
| `services/analytics/sinkKillswitch.ts` | `config.rs` (SinkKillswitch) | L6 |
| `services/analytics/config.ts` | `config.rs` (扩展) | L0 |
| `services/analytics/growthbook.ts` | `config.rs` (EventSamplingConfig) | L6 |
| `services/api/logging.ts` | `events/query_events.rs` | L3 |
| `services/api/metricsOptOut.ts` | `config.rs` (metrics opt-out) | L6 |
| `utils/telemetryAttributes.ts` | `lib.rs` (OtelEventMetadata 已有) | L1 |
| `utils/stats.ts` | `business_metrics.rs` | L4 |
| `utils/statsCache.ts` | `business_metrics.rs` | L4 |
| `utils/debug.ts` | `otel_provider.rs` (tracing 已覆盖) | L0 |
| `utils/sinks.ts` | `events/mod.rs` | L3 |
| `utils/fileOperationAnalytics.ts` | `events/mod.rs` | L3 |
| `services/internalLogging.ts` | `exporters/mod.rs` | L5 |
| `services/toolUseSummary/` | `events/mod.rs` | L3 |
