# coco-compact — Crate Plan

Directory: `services/compact/` (matches TS `services/compact/`)
TS source: `services/compact/compact.ts` (60K), `services/compact/microCompact.ts` (500+), `services/compact/grouping.ts` (64), `services/compact/postCompactCleanup.ts` (78), `services/compact/apiMicrocompact.ts` (154), `services/compact/timeBasedMCConfig.ts` (44), `services/compact/sessionMemoryCompact.ts` (630), `services/compact/autoCompact.ts` (351), `services/compact/prompt.ts` (374), `services/compact/compactWarningState.ts` (18)

## Dependencies

```
coco-compact depends on:
  - coco-types    (Message, UserMessage, SystemMessage, AttachmentMessage, TokenUsage, ContextEditStrategy)
  - coco-config   (CompactConfig, AutoCompactConfig, ApiNativeConfig, MicroCompactConfig,
                   SessionMemoryConfig, ExperimentalConfig, TimeBasedMcConfig)
  - coco-messages (message filtering, normalization helpers)
  - coco-context  (file/plan attachment helpers for post-compact reinjection)
  - coco-error
  - vercel-ai-provider (LlmMessage / content-part aliases via coco-types)
  - tokio

coco-compact does NOT depend on:
  - coco-tool / coco-tools (concrete tool knowledge — discovered via `extract_discovered_tool_names`)
  - coco-inference (LLM call delivered as a `summarize_fn` callback parameter)
  - any app/ crate
  - process env (all env vars folded into `CompactConfig` upstream)
```

Note: `compact_conversation` and `partial_compact_conversation` take a
generic typed `summarize_fn` callback, not an `&ApiClient`. The callback
receives `CompactSummaryAttempt { messages, context_messages,
summary_request, prompt_kind, pre_compact_tokens, max_summary_tokens }`
and returns `CompactSummaryResponse`. The caller
(`app/query::QueryEngine`) owns the LLM execution details: cache-sharing
compact forks when available, and a structured no-tools direct call as
fallback.

## Configuration

Two distinct config types — **do not conflate**:

1. **Global settings:** `coco_config::CompactConfig` lives in
   `common/config/src/compact_settings.rs`. Folds defaults +
   `Settings.compact` overlay + `COCO_COMPACT_*` env overrides into a
   single resolved struct, exposed on `RuntimeConfig.compact`.
2. **Per-call run-options:** `coco_compact::CompactRunOptions` in
   `services/compact/src/compact.rs`. Carries the knobs that vary per
   `compact_conversation` invocation: `max_summary_tokens`,
   `context_window`, `keep_recent_rounds`, `custom_prompt`,
   `suppress_follow_up`, `trigger`.

The crate **does not read env vars at runtime.** All env names use the
`COCO_*` prefix per the root `CLAUDE.md` "Code Hygiene" rule —
TS-style names (`CLAUDE_CODE_*` / unprefixed) are intentionally not
honored.

```rust
// common/config/src/compact_settings.rs (re-exported from coco_config root)
pub struct CompactConfig {
    pub auto: AutoCompactConfig,
    pub micro: MicroCompactConfig,
    pub api_native: ApiNativeConfig,            // Anthropic-only
    pub session_memory: SessionMemoryConfig,
    pub experimental: ExperimentalConfig,       // history_snip / staged_compact / display_collapses
}

pub struct AutoCompactConfig {
    pub enabled: bool,                          // Settings: compact.auto.enabled
    pub disabled_by_env: bool,                  // Env: COCO_COMPACT_DISABLE
    pub auto_disabled_by_env: bool,             // Env: COCO_COMPACT_DISABLE_AUTO
    pub context_window_override: Option<i64>,   // Env: COCO_COMPACT_AUTO_WINDOW
    pub pct_override: Option<f64>,              // Env: COCO_COMPACT_AUTO_PCT_OVERRIDE
    pub blocking_limit_override: Option<i64>,   // Env: COCO_COMPACT_BLOCKING_LIMIT
}
impl AutoCompactConfig { fn is_active(&self) -> bool; }

pub struct MicroCompactConfig {
    pub enabled: bool,
    pub keep_recent: i32,                       // Default 5
    pub time_based: TimeBasedMcConfig,          // gap_threshold_minutes / keep_recent
}

pub struct ApiNativeConfig {
    pub clear_tool_results: bool,               // Env: COCO_COMPACT_API_CLEAR_TOOL_RESULTS
    pub clear_tool_uses: bool,                  // Env: COCO_COMPACT_API_CLEAR_TOOL_USES
    pub max_input_tokens: i64,                  // Env: COCO_COMPACT_API_MAX_INPUT_TOKENS (default 180_000)
    pub target_input_tokens: i64,               // Env: COCO_COMPACT_API_TARGET_INPUT_TOKENS (default 40_000)
}

pub struct SessionMemoryConfig {
    pub enabled: bool,                          // Env: COCO_COMPACT_SESSION_MEMORY_{ENABLE,DISABLE}
    pub min_tokens: i64,
    pub min_text_block_messages: i32,
    pub max_tokens: i64,
    pub max_summary_chars: i64,
}

pub struct ExperimentalConfig {
    pub history_snip: HistorySnipConfig,        // self-designed re-port (TS strip-only)
    pub staged_compact: StagedCompactConfig,    // self-designed re-port (TS strip-only)
    pub display_collapses: DisplayCollapseConfig, // 4 TUI-only message folds (default on)
}
```

Resolution order (`CompactConfig::resolve`):
1. `Default::default()`
2. Apply `Settings.compact` (settings.json overlay)
3. Apply env overrides (`EnvSnapshot.is_truthy(EnvKey::CocoCompact*)`)
4. `finalize()` — invariants (`commit_at_pct ≥ stage_at_pct`, …)

`coco_compact` consumers receive the resolved config from
`RuntimeConfig.compact` and pass it down by reference; **no env reads
inside the crate**.

## Data Definitions

```rust
pub struct CompactSummaryAttempt {
    pub messages: Vec<Message>,
    pub context_messages: Vec<Message>,
    pub summary_request: String,
    pub prompt_kind: CompactSummaryKind,
    pub pre_compact_tokens: i64,
    pub max_summary_tokens: i64,
}

pub struct CompactSummaryResponse {
    pub summary: String,
}

pub enum CompactOutcome {
    Applied,
    Skipped,
    Failed,
}

pub struct CompactResult {
    pub boundary_marker: Message,
    pub raw_summary: Option<String>,
    pub summary_messages: Vec<Message>,
    pub attachments: Vec<AttachmentMessage>,
    pub messages_to_keep: Vec<Message>,
    pub hook_results: Vec<Message>,
    pub pre_compact_tokens: i64,
    pub post_compact_tokens: i64,
    pub true_post_compact_tokens: i64,
    pub trigger: CompactTrigger,
}

pub struct RecompactionInfo {
    pub is_recompaction: bool,
    pub turns_since_previous: i32,
    pub auto_compact_threshold: i64,
}

pub enum CompactError {
    LlmCallFailed { source: ApiError },
    TokenBudgetExceeded { actual: i64, limit: i64 },
    Cancelled,
    StreamRetryExhausted { attempts: i32 },
}

pub struct MicrocompactResult {
    pub messages_cleared: i32,
    pub tokens_saved_estimate: i64,
    pub was_time_triggered: bool,
}

/// API-level context management strategy
pub enum ContextEditStrategy {
    /// Clear tool results from older turns (API 2025-09-19)
    ClearToolUses {
        trigger: Option<i64>,
        keep_recent: Option<ToolUseKeep>,
        clear_inputs: ClearToolInputs,
        exclude_tools: Vec<ToolName>,
        exclude_tool_strs: Vec<String>,
    },
    /// Clear thinking/reasoning from older turns (API 2025-10-15)
    ClearThinking { keep: ThinkingKeep },
}

pub enum ClearToolInputs {
    All,
    SpecificTools(Vec<ToolName>),
    None,
}

pub enum ThinkingKeep {
    Recent { turns: i32 },
    All,
}

pub struct ToolUseKeep { pub value: i32 }
```

## Multi-Provider Strategy

Three layers, picked by capability:

1. **Client-side micro-compact** — provider-agnostic mutation of old
   tool result content. Breaks the prompt cache; works everywhere.
2. **API-native context_management** — Anthropic-only server-side
   clearing that preserves cache. Dispatch gate is
   `coco_inference::ApiClient::supports_server_side_context_edits()`
   (true only when `ProviderApi::Anthropic`). The strategies are
   built by `get_api_context_management(&ApiContextOptions)` and
   serialized to wire JSON by
   `encode_anthropic_context_management(&[ContextEditStrategy])`.
   `coco-query` then stuffs the value into
   `QueryParams.context_management`, and `services/inference::build_call_options`
   places it under `provider_options["anthropic"]["contextManagement"]`
   where the Anthropic provider's `extract_anthropic_options` reads
   it (camelCase → snake_case happens inside that provider).
3. **Full LLM summarization** — `compact_conversation` fallback.

`coco-compact` itself **never inspects the provider**: it produces
strategy descriptions and the encoder, nothing more.

## Core Logic

### Full Compaction (from `compact.ts`)

```rust
/// Summarize conversation via LLM call, preserving recent context.
/// 1. Strip images (replace with [image] marker to prevent prompt-too-long)
/// 2. Strip re-injectable attachments (skills, agents)
/// 3. Call typed summarizer with structured messages plus summary_request
/// 4. Build summary messages
/// 5. Re-inject current file state (up to 5 files, 50K tokens budget)
/// 6. Re-inject active skills (25K token budget) — currently delivered
///    via `coco_system_reminder::InvokedSkillsGenerator` on the next turn
/// 7. Execute post-compact hooks (PreCompact / PostCompact in `coco-hooks`)
/// 8. Notify `CompactionObserverRegistry`
pub async fn compact_conversation<F, Fut>(
    messages: &[Message],
    options: &CompactRunOptions,                // per-call: trigger, custom_prompt, …
    summarize_fn: F,
    attachment_fn: Option<PostCompactAttachmentFn>,
) -> Result<CompactResult, CompactError>
where
    F: Fn(CompactSummaryAttempt) -> Fut,
    Fut: Future<Output = Result<CompactSummaryResponse, String>>;

const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;
const POST_COMPACT_TOKEN_BUDGET: i64 = 50_000;
const POST_COMPACT_MAX_TOKENS_PER_FILE: i64 = 5_000;
const POST_COMPACT_MAX_TOKENS_PER_SKILL: i64 = 5_000;
const POST_COMPACT_SKILLS_TOKEN_BUDGET: i64 = 25_000;
```

### Manual `/compact` Path

```rust
// app/query::QueryEngine
pub async fn run_manual_compact(
    &self,
    history: &mut MessageHistory,
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    custom_instructions: Option<String>,
);
```

The slash-command handler (`coco_commands::handlers::compact`) emits a
single sentinel line `__COCO_COMPACT_NOW__ <args>` so runners (TUI /
SDK) can detect the request and call `run_manual_compact`. Slash
handlers themselves don't hold a `QueryEngine` reference, so the
sentinel is the bridge.

### Micro Compaction (from `microCompact.ts`)

```rust
/// Lightweight: clears old tool results without LLM call. Targets:
/// FileRead, Bash, Grep, Glob, WebSearch, WebFetch, FileEdit, FileWrite.
/// Time-based trigger fires when `now - last_assistant_ms >
/// gap_threshold_minutes` (default 60) and we're on the main thread.
pub fn micro_compact(messages: &mut [Message], keep_recent: usize) -> MicrocompactResult;
pub fn time_based_microcompact(
    messages: &mut [Message],
    config: &TimeBasedMcConfig,
    now_ms: i64,
    last_assistant_ms: Option<i64>,
    is_main_thread: bool,
) -> Option<MicrocompactResult>;
```

### Auto Compact (from `autoCompact.ts`)

```rust
pub fn should_auto_compact(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
) -> bool;
pub fn should_auto_compact_guarded(
    current_tokens: i64,
    context_window: i64,
    max_output_tokens: i64,
    cfg: &AutoCompactConfig,
    source: CompactQuerySource,
) -> bool;
```

`CompactQuerySource::{SessionMemory, Compact}` short-circuit the
guard so forked agents can't recursively trigger compaction.

### Reactive (PTL recovery)

```rust
pub fn should_reactive_compact(
    estimated_tokens: i64,
    config: &ReactiveCompactConfig,
    auto_cfg: &AutoCompactConfig,
) -> bool;
pub fn calculate_drop_target(
    current_tokens: i64,
    config: &ReactiveCompactConfig,
    auto_cfg: &AutoCompactConfig,
) -> i64;
pub fn peel_head_for_ptl_retry(messages: &[Message], tokens_to_free: i64) -> Option<Vec<Message>>;
pub fn api_microcompact(messages: &mut [Message], tokens_to_free: i64);
```

`ReactiveCompactState` is a circuit breaker that disables reactive
compaction after `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3` failures.

### Session Memory Compact (from `sessionMemoryCompact.ts`)

```rust
pub async fn compact_session_memory(
    messages: &[Message],
    config: &SessionMemoryConfig,
    session_memory_path: Option<&Path>,
) -> Result<CompactResult, CompactError>;
```

When enabled and the session memory file exists, this path replaces the
LLM summarizer with the pre-extracted memory text plus recent messages
selected by `select_memories_for_compaction`. Cheap (no API call) but
predicated on session-memory extraction having run.

### API-Native Builder (from `apiMicrocompact.ts`)

```rust
pub struct ApiContextOptions {
    pub has_thinking: bool,
    pub is_redact_thinking_active: bool,
    pub clear_all_thinking: bool,
    pub clear_tool_results: bool,
    pub clear_tool_uses: bool,
    pub trigger_threshold: i64,
    pub keep_target: i64,
}
impl ApiContextOptions {
    pub fn from_config(
        cfg: &CompactApiNativeConfig,
        has_thinking: bool,
        is_redact_thinking_active: bool,
        clear_all_thinking: bool,
    ) -> Self;
}

pub fn get_api_context_management(opts: &ApiContextOptions) -> Vec<ContextEditStrategy>;
pub fn encode_anthropic_context_management(strategies: &[ContextEditStrategy]) -> Option<Value>;
```

The encoder produces camelCase JSON that lines up with
`AnthropicProviderOptions` (`#[serde(rename_all = "camelCase")]`). The
`vercel-ai-anthropic` language model's `transform_context_management`
then snake-cases keys for the actual Anthropic API.

### Post-Compact Cleanup (Observer pattern)

TS uses a god function (`runPostCompactCleanup`) hardcoding 10+ cache
clears. coco-rs uses a registry:

```rust
#[async_trait]
pub trait CompactionObserver: Send + Sync {
    async fn on_compaction_complete(
        &self,
        result: &CompactResult,
        is_main_agent: bool,
    ) -> anyhow::Result<()>;
    async fn on_post_compact(&self, _new_messages: &[Message]) -> anyhow::Result<()> { Ok(()) }
}

pub struct CompactionObserverRegistry { /* … */ }
```

Each crate owning post-compact-invalidatable state registers its own
observer at startup (e.g. `coco-context::ContextCacheObserver`,
`coco-permissions::ApprovalsObserver`). `app/query::QueryEngine`
calls `notify_all` + `notify_post_compact` after successful full,
partial, session-memory, and reactive compaction paths.

### Pre / PostCompact Hooks

`app/query::QueryEngine::try_full_compact`:
1. `coco_hooks::orchestration::execute_pre_compact(registry, ctx, trigger_label, instructions)`
   — collects `new_custom_instructions` and `user_display_message`.
2. `merge_hook_instructions(orig, hook)` folds them into
   `compact::CompactConfig.custom_prompt`.
3. After successful summarization,
   `execute_post_compact(registry, ctx, trigger_label, raw_summary)`
   receives `CompactResult.raw_summary`, not the formatted continuation
   message. The hook result's `user_display_message` is carried on
   `CompactResult.user_display_message`.

## Compact Warning State (from `compactWarningState.ts`)

```rust
pub struct CompactWarningState {
    pub suppressed: AtomicBool,
}
```

Set after a successful compaction so the TUI doesn't redundantly warn
about stale pre-compact token counts. Pure state — no React/UI dep.

## Compact Prompt Generation (from `prompt.ts`)

```rust
pub fn get_compact_prompt(custom_instructions: Option<&str>) -> String;
pub fn get_partial_compact_prompt(...) -> String;
pub fn format_compact_summary(summary: &str) -> String;
pub fn get_compact_user_summary_message(...) -> String;
```

## Module Layout

```
services/compact/src/
  lib.rs                     re-exports
  types.rs                   CompactResult / CompactError / constants
  compact.rs                 full LLM-summarized compaction
  micro.rs                   client-side tool-result clearing
  micro_advanced.rs          budget-aware in-place clears (clear_tool_uses/clear_thinking)
  api_compact.rs             ContextEditStrategy builder
  serialize.rs               ContextEditStrategy → camelCase Value (Anthropic wire)
  auto_trigger.rs            threshold helpers + time-based trigger
  reactive.rs                PTL recovery + circuit breaker
  session_memory.rs          session-memory-driven compaction
  grouping.rs                API-round message grouping
  post_compact_files.rs      file re-injection (5 files / 50K / 5K)
  post_compact_plan.rs       plan-file re-injection
  prompt.rs                  summarizer prompt templates
  observer.rs                CompactionObserver trait + registry
  tokens.rs                  token estimators (text / message / tool_result)
```

## Experimental Flags

The default behavior of every flag below mirrors **TS external (feature
stripped)**. Code is staged in-tree but inert until explicitly enabled
in `settings.json`.

- `experimental.history_snip` — placeholder for a future SnipTool
  analogue. TS external has no runtime (`feature('HISTORY_SNIP')` is DCE'd).
  No Rust callers consult `enabled` today; the config exists so types
  compile when the implementation lands. **Inert by default.**
- `experimental.staged_compact` — placeholder for a future
  `marble_origami` analogue. TS external has no runtime
  (`feature('CONTEXT_COLLAPSE')` is DCE'd) and only keeps the
  `marble-origami-{commit,snapshot}` types in `types/logs.ts` for
  transcript-format interop. The Rust ledger (`StagedCompactLedger`,
  `apply_collapses_if_needed`, etc.) is wired through `app/query` but
  the install hook (`with_staged_ledger`) has zero production callers,
  so the ledger is `None` at runtime. **Inert by default.**
- `experimental.display_collapses` — gate for a future port of the
  four `collapse*.ts` utilities (TUI-only message folding). These
  reducers exist in external `utils/` (port cleanly) but have
  not yet been ported; `app/tui/src/widgets/chat/mod.rs::build_lines`
  carries a TS-alignment-gap comment listing the four functions to
  wire here. Defaults stay `true` so behavior flips on automatically
  once the reducers land.

All three live behind `compact.experimental.*`; production defaults
match TS-feature-stripped behavior.

## Related: Tool Result Budget (NOT owned here)

Tool result budget is the **first line of defense** in the compact
capability cluster — it caps oversize tool output before any compaction
strategy runs — but TS keeps the implementation in `utils/toolResultStorage.ts`
(not `services/compact/`), and the Rust port follows that boundary:

- Level 1 (per-tool persistence with `<persisted-output>` wrapper + 2KB
  preview + session-scoped `tool-results/` dir) → owned by `coco-tool-runtime`.
- Level 2 (per-message aggregate budget via `ContentReplacementState` +
  `enforceToolResultBudget`) → owned by `coco-tool-runtime` (state types
  + enforcement fn) and wired by `coco-query`.
- Transcript records (`ContentReplacementRecord`) → owned by `coco-session`.

`coco-compact` only **shares the cleared-content marker**
(`TOOL_RESULT_CLEARED_MESSAGE = '[Old tool result content cleared]'`) with
the storage module — both micro-compact (this crate) and Level-1 persistence
use the same string. Re-export that constant from `coco-tool-runtime` and
consume it here; do not duplicate.

See [`tool-result-budget-plan.md`](../../docs/coco-rs/tool-result-budget-plan.md)
for the full two-phase implementation plan, owner routing, and
cache-stability invariants.

## Micro-compact opt-ins (TS-alignment defaults)

`MicroCompactConfig` carries two opt-in flags whose defaults mirror
TS external (no count-based mutation, no per-turn stub rewrite):

- `compact.micro.count_based_enabled` (default `false`) —
  count-based clearing of old tool results when the autocompact
  threshold fires or `/compact` runs. TS external `microcompactMessages`
  is a no-op outside `feature('CACHED_MICROCOMPACT')`; mirroring that,
  the Rust `micro_compact()` call sites now require this flag.
- `compact.micro.clear_file_unchanged_stubs_enabled` (default `false`)
  — per-turn `[file unchanged]` stub rewrite. No TS equivalent; opt-in.
