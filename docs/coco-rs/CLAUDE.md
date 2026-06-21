# coco-rs Plan Documentation

TS-first Rust code agent. TS defines architecture; Rust best practices for implementation.

## Principles

1. **TS-first**: Each TS source module maps to a Rust crate. TS source is the specification.
2. **Rust for details**: snafu errors, CancellationToken, Arc sharing, trait-based tools.
3. **Copy only base infra from cocode-rs**: error, otel, utils (24), vercel-ai (8). Everything else rewrites from TS.
4. **provider-sdks removed**: vercel-ai handles all provider abstraction.
5. **Every cocode-rs choice requires justification**: Rust-only, No TS equiv, Rust superior, or HYBRID.

## Document Map (Source of Truth)

Each piece of information has exactly one owner. No duplication across docs.

| Information | Owner (single source) | Other docs must NOT redefine |
|------------|----------------------|------------------------------|
| TS dir -> Rust crate mapping | `ts-to-rust-mapping.md` | crate plans reference, not copy |
| TS utils file -> Rust target | `ts-utils-mapping.md` | |
| Multi-provider types (ModelRole, ProviderApi, Capability) | `crate-coco-types.md` | multi-provider-plan.md shows usage, not definition |
| `PartialProviderConfig`, `ProviderConfig`, `PartialModelInfo`, `ModelInfo`, `ProviderModelOverride`, `ProviderClientOptions`, `ModelRoles`, `ModelRegistry`, `ResolvedModel`, `RuntimeConfig`, `RedactedSecret`, `PositiveTokens`, `PositiveCount` | `crate-coco-config.md` | multi-provider-plan.md shows usage and three-layer flow, not field-level definition |
| `ApiClient`, `ProviderClientFingerprint`, `build_call_options`, `thinking_convert::to_extra_body`, `PerCallOverrides`, retry, files API, bootstrap | `crate-coco-inference.md` | multi-provider-plan.md shows architecture, not struct fields |
| `build_language_model_from_runtime` (provider construction) | `app/cli/src/model_factory.rs` (no separate doc) | multi-provider-plan.md §6 |
| Multi-provider architecture (flow, beta headers) | `multi-provider-plan.md` | |
| File ownership (which crate owns which config file) | `config-file-map.md` | crate plans reference, not copy |
| ToolId, AgentTypeId, ToolName, SubagentType — identity enums | `crate-coco-types.md` | other docs use by name |
| Tool trait, ToolUseContext, StreamingToolExecutor | `crate-coco-tool.md` | |
| Tool input enums: GrepOutputMode, ConfigAction, LspAction | `crate-coco-tools.md` | |
| Context enums: Platform, ShellKind | `crate-coco-context.md` | |
| MCP types, config, client, auth, channels | `crate-coco-mcp.md` | |
| Permission evaluation pipeline, auto-mode/yolo classifier, denial tracking | `crate-coco-permissions.md` | |
| Plan mode lifecycle, plan files, enter/exit/reentry, TS mirror map | `plan-mode-architecture.md` | crate-coco-tools references, not redefines |
| Error handling architecture (3-layer model, error flow) | `CLAUDE.md` (this file) | crate docs show per-crate error types, not architecture |
| ToolError, ValidationResult, error_code conventions | `crate-coco-tool.md` | |
| Directory structure, dependency graph, phases | `coco-rs-plan.md` | |
| Gap tracking | `audit-gaps.md` | |
| OTel 6 层模型 (L0-L5), span 层级, 应用事件, 业务 metrics, exporter | `crate-coco-otel.md` | |
| Coordinator mode, swarm, team management, backends | `crate-coco-coordinator.md` | |
| Vim state machine, motions, operators, text objects | `crate-coco-vim.md` | |
| Voice recording, STT WebSocket, keyterms, hold-to-talk | `crate-coco-voice.md` | |
| Assistant session history pagination | `crate-coco-assistant.md` | |
| Remote session WS, SDK message adapter, upstream proxy | `crate-coco-remote.md` | |
| Steering: mid-turn message queue, CommandQueue, QueryGuard, attachment injection | `crate-coco-query.md` | |
| Agent loop refactor architecture: `ToolCallRunner`, `ToolContextFactory`, `ModelRuntime`, tool-call invariants, streaming/non-streaming semantics, AgentTool/SkillTool integration points | `agent-loop-refactor-plan.md` | crate plans own stable type definitions; this doc owns the cross-cutting migration plan |
| Subagent invocation architecture: definition store, TS-first source precedence, AgentTool prompt/filter behavior, runtime ownership, background/worktree/fork phases | `subagent-refactor-plan.md` | `crate-coco-tools.md` owns the tool schema; this doc owns subagent runtime architecture |
| Skills/commands/plugins parity gaps, including `/compact` command behavior and SkillTool command-source bridge | `parity-skills-commands-plugins.md` | crate plans reference gap details; this doc owns TS-verified parity deltas |
| Whole-codebase TS→Rust parity **fix tracker** (per-finding status + fix sketch + TS/Rust refs, baseline HEAD `748580242`) | `parity-fix-tracker.md` | single self-contained tracker; predecessors (deep-review / open-followup / findings.json) folded in and deleted |
| Prompt cache break detection, CacheScope, CacheBreakDetector | `crate-coco-inference.md` | |
| Message mutation pipeline (`MessagePass` trait, `run_message_passes`, normalize + compact pass catalog, drift-detection invariant) | `message-pipeline.md` | `coco-rs/core/messages/CLAUDE.md` references; `coco-rs/services/compact/CLAUDE.md` references |
| AgentTool architecture: spawn, fork, worktree, tool filtering, agent-as-task | `crate-coco-tools.md` | |
| **Tool Result Budget** (Level 1 per-tool persist + Level 2 per-message budget) | `tool-result-budget-plan.md` | crate-coco-tool-runtime / crate-coco-tools / crate-coco-query / crate-coco-compact reference, not redefine |
| **ToolSearch** deferred-tool lazy loading: query DSL, scoring, multi-provider promotion via `discovered_tool_names` | `tool-search-design.md` | crate-coco-tools / crate-coco-tool-runtime reference, not redefine |
| Skills loading, SkillDefinition, SkillManager, bundled registry | `crate-coco-skills.md` | |
| Hooks: HooksSettings, HookMatcher, HookCommand, HookExecutor, AsyncHookRegistry | `crate-coco-hooks.md` | |
| Background task execution: TaskState, isBackgrounded, task output, notification, PlanFileManager | `crate-coco-tasks.md` | |
| Memory entry management: MemoryManager, staleness, recall, auto-extraction | `crate-coco-memory.md` | |
| Plugin loading: PluginManifest, PluginManager, contributions | `crate-coco-plugins.md` | |
| Keybinding resolution: 18 contexts, 50+ actions, chord, platform defaults | `crate-coco-keybindings.md` | |
| Per-crate plan (dependencies, modules, data definitions) | `crate-coco-{name}.md` | |
| Current prioritized gap fix plan | `current-gap-fix-plan.md` | `audit-gaps.md` remains the historical/deferred tracker; this doc owns near-term fix ordering and stale-gap cleanup |
| Permission/sandbox hardening — refactor background, decisions, status & prioritized TODO (resume handoff) | `permission-sandbox-hardening.md` | owns the permission/sandbox parity-fix status + remaining work; crate internals stay in `crate-coco-{permissions,sandbox,shell}.md` |
| TUI v2 target architecture: three-way comparison (coco/codex-rs/jcode), anchored finalize without fingerprint reconciliation, engine-owned seating, staged Scope B implementation plan | `ui/tui-v2-design.md` | `ui/single-render-path-refactor.md` and `ui/terminal-surface-design.md` describe the current/landed state and reference, not redefine, the v2 target |

### Rules

- Type definitions appear in exactly one `crate-coco-*.md` file. Other docs reference by name only.
- `coco-rs-plan.md` contains overview-level code snippets for illustration. These are NOT authoritative — the crate-level doc is.
- When a crate plan and `coco-rs-plan.md` conflict, the crate plan wins.
- `audit-gaps.md` tracks what is intentionally deferred (P1/P2/P3), not what is wrong.

## Type Ownership (Canonical Locations)

### coco-types (zero internal deps)

Owns all enum/struct definitions that are shared across 3+ crates:

```
Message, UserMessage, AssistantMessage, StopReason, MessageKind, NormalizedMessage
PermissionMode, PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionDecision
CommandBase, CommandType, CommandAvailability, CommandSource
ToolName (43 builtin variants, Copy), ToolId { Builtin(ToolName) | Mcp | Custom }
ToolResult<T>, ToolProgress   (ToolInputSchema is NOT here — it's the self-validating newtype in coco-tool-runtime)
SubagentType (7 builtin variants, Copy), AgentTypeId { Builtin(SubagentType) | Custom }
TaskType, TaskStatus, TaskStateBase, TaskHandle
SessionId, AgentId, TaskId
HookEventType (27 variants), HookOutcome, HookResult
SandboxMode
TokenUsage, ModelUsage
ThinkingLevel { effort, budget_tokens, options: HashMap<String, Value> }, ReasoningEffort (6 levels)
ProviderApi, ModelRole, ModelSpec, Capability, ApplyPatchToolType, WireApi
PermissionDecisionReason, StreamingToolUse, StreamingThinking, TaskBudget
UserType, Entrypoint
```

Also re-exports vercel-ai types as version-agnostic aliases:
```
LlmMessage, LlmPrompt, UserContent, AssistantContent, ToolContent,
TextContent, FileContent, ToolCallContent, ToolResultContent, ReasoningContent
```
All crates use `coco_types::LlmMessage` — never `vercel_ai_provider::LanguageModelV4Message` directly.

Does NOT own: `ModelInfo` (coco-config), `ToolUseContext` (coco-tool), `HooksSettings` (coco-hooks), `QueryEngine` (coco-query), `AppState` (coco-state).

### coco-config

Owns structs that combine coco-types enums with config data:

```
ModelInfo, PartialModelInfo                     (uses Capability, ProviderApi from coco-types)
ProviderConfig, PartialProviderConfig           (uses ProviderApi, WireApi from coco-types)
ProviderModelOverride, PartialProviderModelOverride
ProviderClientOptions, PartialProviderClientOptions
ModelRegistry, ResolvedModel
RedactedSecret, PositiveTokens, PositiveCount   (Layer 1 invariants)
ModelRoles                                       (uses ModelRole from coco-types)
RuntimeConfig                                    (atomic snapshot — providers + roles + registry + tool_overrides)
GlobalConfig, Settings, SettingsWithSource, SettingSource
EnvOnlyConfig, RuntimeOverrides, ResolvedConfig
ModelAlias, FastModeState, SettingsWatcher, AutoModeConfig
```

### coco-tool

Owns the tool execution interface:

```
Tool trait, ToolUseContext, ToolError
StreamingToolExecutor, ToolRegistry, ToolBatch
DescriptionOptions, ValidationResult, InterruptBehavior
```

### coco-tools

Owns tool-specific input enums (not shared beyond tool implementations):

```
GrepOutputMode { Content, FilesWithMatches, Count }
ConfigAction { Get, Set, List, Reset }
LspAction { Definition, References, Diagnostics, Symbols, Hover }
```

### coco-context

Owns environment enums:

```
Platform { Darwin, Linux, Windows }
ShellKind { Bash, Zsh, Sh, PowerShell }
```

### coco-query

Owns the query loop, steering, and stream-to-item conversion:

```
QueryEngine, QueryEngineConfig, QueryConfig, QueryGates
BudgetTracker, BudgetDecision, ContinueReason
QueuedCommand, QueuePriority, CommandQueue, QueryGuard, PromptInputMode
InboxMessage, InboxStatus
StreamAccumulator (AgentStreamEvent → ServerNotification converter)
```

Note: The old `QueryEvent` enum has been deleted in Phase 0. QueryEngine now
emits `coco_types::CoreEvent` directly (3-layer Protocol/Stream/Tui).
See `event-system-design.md`.

### coco-inference

Owns (in addition to retry, files API, bootstrap):

```
ApiClient (carries ProviderClientFingerprint), ApiClientConfig
ProviderClientFingerprint                                        (turn-boundary coherence, §11.1 of multi-provider-plan)
build_call_options module                                        (Layer 2 per-request builder; single ProviderOptions write site)
PerCallOverrides                                                 (per-turn deltas: temperature, top_p, top_k, max_output_tokens, thinking, extra_body)
thinking_convert module                                           (ThinkingLevel → flat camelCase BTreeMap<String,JSONValue>; provider-aware mapping)
CacheScope, CacheBreakDetector, CachePromptState, CacheBreakEvent
```

There is no `ModelHub` / `ProviderFactory` / `RequestBuilder` / `request_options_merge`.
Provider construction lives in `app/cli/src/model_factory.rs::build_language_model_from_runtime`.
The 5-step `RequestBuilder` pipeline is replaced by `build_call_options`
(per-call) + the model_factory entry (per-client). See multi-provider-plan.md
§14 "Replaces / removed from prior versions".

Note: ThinkingLevel and ReasoningEffort are in coco-types (shared across config/inference/query).
ThinkingLevel.options (HashMap) carries provider-specific thinking extensions (data-driven, no typed fields).
ReasoningSummary enum removed — now a string value in ThinkingLevel.options.

## Canonical Names (Resolved Inconsistencies)

These names are final. All docs must use these exact names.

| Canonical name | NOT this | Reason |
|---------------|----------|--------|
| `PermissionDecision` | ~~PermissionResult~~, ~~PermissionDecision::Allowed~~ | Defined in coco-types with variants `Allow`, `Ask`, `Deny` |
| `check_permissions` (plural) | ~~check_permission~~ | Matches TS `checkPermissions()` |
| `ProviderApi` | ~~ApiProvider~~ | Canonical enum in coco-types — wire-protocol family. **Anthropic cloud-credential variants (Bedrock / Vertex / Foundry) are explicit non-goals** — coco-rs targets Anthropic FirstParty, OpenAI, Google, ByteDance, and generic OpenAI-compatible. Distinct from provider **instance identifier** (`ProviderConfig.name: String`); see multi-provider-plan.md §2.2. |
| `ApiClient` | ~~Arc<dyn LanguageModelV4>~~ directly | QueryEngine holds `Arc<ApiClient>` which wraps vercel-ai internally |
| `ThinkingLevel` | ~~EffortLevel~~, ~~ThinkingConfig~~, ~~ThinkingParams~~ | Single unified struct in coco-types. Fields: effort (ReasoningEffort) + budget_tokens + options (HashMap). Provider-specific thinking params (interleaved, reasoningSummary, includeThoughts) go through options, not typed fields. Evolved from cocode-rs `common/protocol/src/thinking.rs`. |
| `model_id` | ~~slug~~ | Model identifier field name. Aligns with vercel-ai `model_id()` and industry convention. |
| `ToolResult<T>` | ~~ToolResult~~ unparameterized | Generic in coco-types. Tool trait uses `ToolResult<Value>` |
| `ToolId` | ~~tool_name: String~~ for identity | `ToolId { Builtin(ToolName) \| Mcp { server, tool } \| Custom(String) }`. Use for identity fields. Permission patterns stay `String` (ToolPattern). |
| `AgentTypeId` | ~~agent_type: String~~ | `AgentTypeId { Builtin(SubagentType) \| Custom(String) }`. Same pattern as ToolId. |

## Dependency Layer Rules

```
L0  utils/* (24)                           — no internal deps
L0  vercel-ai/* (8)                        — no internal deps
L1  coco-types                             — vercel-ai-provider (L0 types only)
L1  coco-error                             — no internal deps
L1  coco-otel                              — coco-error
L1  coco-config                            — coco-types, coco-error, utils/*
L2  coco-inference                         — coco-types, coco-config, coco-error, vercel-ai-*
L2  coco-sandbox                           — coco-types (SandboxMode)
L3  coco-messages                          — coco-types, coco-error
L3  coco-context                           — coco-types, coco-config, coco-error, utils/git
L3  coco-permissions                       — coco-types, coco-config, coco-inference, coco-error
L3  coco-tool                              — coco-types, coco-config, coco-error
L3  coco-shell                             — coco-types, utils/shell-parser, coco-sandbox
L3  coco-compact                           — coco-types, coco-inference, coco-messages
L3  coco-mcp                               — coco-types, coco-config
L3  coco-tools                             — coco-tool, coco-shell, coco-mcp, coco-lsp, coco-permissions
L4  coco-commands, coco-skills, coco-hooks — coco-types, coco-tool (ToolRegistry)
L4  coco-tasks, coco-memory, coco-plugins  — coco-types, coco-tool, coco-inference
L5  coco-state                             — coco-types, coco-config, coco-tool
L5  coco-query                             — coco-types, coco-config, coco-inference, coco-tool, coco-context, coco-messages, coco-compact, coco-permissions, coco-hooks, coco-state
L5  coco-session, coco-tui, coco-cli       — everything
--- v2/v3 (added on top of v1 layer rules) ---
L3  coco-vim                               — (none — pure state machine)
L5  coco-coordinator                       — coco-types, coco-config, coco-permissions, coco-tool, coco-error
L5  coco-voice                             — coco-config, coco-error
L5  coco-assistant                         — coco-types, coco-config, coco-error
L5  coco-remote                            — coco-types, coco-config, coco-error
```

### Circular Dependency Prevention

- `coco-tool` does NOT depend on `coco-tools` (concrete implementations). ToolRegistry is filled by coco-cli at startup.
- `coco-tools` does NOT depend on `commands/`, `skills/`, `tasks/`. Interaction is via callback closures in `ToolUseContext`.
- `coco-config` does NOT depend on `coco-hooks`. The `Settings.hooks` field uses `serde_json::Value`, not typed `HooksSettings`. Each feature crate deserializes its own section.
- `coco-query` does NOT depend on `coco-tools`. Concrete tools are injected via `ToolRegistry` at runtime.
- `coco-permissions` depends on `coco-inference` (L2) for the auto-mode classifier LLM calls. This makes coco-permissions L3 (not L2). The classifier calls ApiClient to run the two-stage XML classification.

## Permission vs Settings Priority (Two Separate Systems)

These are different systems with different priority semantics:

**Settings loading** (coco-config) — later source overrides earlier:
```
plugin(lowest) < user < project < local < flag < policy(highest)
```
Enterprise policy overrides everything. This is correct.

**Permission rule evaluation** (coco-permissions) — more-specific wins:
```
session(highest) > command > cliArg > flagSettings > localSettings > projectSettings > userSettings > policySettings(broadest)
```
Session rules are most specific (user just set them). Policy rules are broadest default. BUT: deny always wins immediately regardless of priority (step 1 of evaluation pipeline).

These are NOT contradictory. Settings control what values are loaded. Permission rules control which rule takes precedence when multiple rules match the same tool.

## ModelInfo Field Conventions

All numeric model fields use concrete types (not Option) with defaults:

```rust
pub struct ModelInfo {
    pub context_window: i64,             // NOT Option<i64> — every model has one
    pub max_output_tokens: i64,          // NOT Option<i64>
    pub capabilities: HashSet<Capability>,
    pub apply_patch_tool_type: ApplyPatchToolType, // defaults to None variant
    // ... other fields
}
```

If a field truly varies per-provider, use `Option` and document why.

## ToolResult and context_modifier

`ToolResult<T>` in coco-types is a plain data struct (no function pointers, no trait objects). It does NOT contain `context_modifier`.

Context modification after tool execution is handled by the `Tool` trait method in coco-tool:

```rust
trait Tool {
    fn modify_context_after(&self, result: &ToolResult<Value>, ctx: &mut ToolUseContext) {}
}
```

## Message Model: TS Architecture vs coco-rs Architecture

### TS 的实际结构：嵌套而非两套独立类型

TS **没有**两套独立的 Message 格式。它用**嵌套结构**：内部 Message 直接包裹 `@anthropic-ai/sdk` 的 `MessageParam`：

```typescript
// TS 的 UserMessage（types/message.js，构建时生成）
type UserMessage = {
    type: 'user',
    message: {                        // ← 直接嵌套 SDK 的 MessageParam
        role: 'user',
        content: ContentBlockParam[]  // ← @anthropic-ai/sdk 类型
    },
    // 以下是内部元数据（不发送到 API）
    uuid: UUID,
    isMeta?: true,           // 对 UI 隐藏，对模型可见
    isVirtual?: true,        // 不发到 API
    isCompactSummary?: true, // 压缩摘要标记
    permissionMode?: PermissionMode,
    origin?: MessageOrigin,
    ...
}

// AssistantMessage 同理：
type AssistantMessage = {
    type: 'assistant',
    message: BetaMessage,    // ← 直接是 SDK 的 BetaMessage 响应
    uuid: UUID,
    requestId?: string,
    isApiErrorMessage?: boolean,
    ...
}
```

`normalizeMessagesForAPI()` **不做类型转换**——它过滤 + 排序，返回的仍然是 `(UserMessage | AssistantMessage)[]`。发 API 时直接取 `.message` 字段。

差异极小：内部 Message = SDK MessageParam + 元数据包裹。

### coco-rs 的设计：直接包装 vercel-ai 类型（与 TS 模式一致）

vercel-ai 的 content types 和 `@anthropic-ai/sdk` 是**同一抽象层级**，1:1 对应：

| anthropic-ai/sdk (TS) | vercel-ai-provider (Rust) |
|---|---|
| `TextBlockParam` | `TextPart` |
| `ImageBlockParam` + `DocumentBlockParam` | `FilePart` (via media_type) |
| `ToolUseBlock` | `ToolCallPart` |
| `ToolResultBlockParam` | `ToolResultPart` |
| `ThinkingBlock` | `ReasoningPart` |

因此 coco-rs **复用 TS 的嵌套模式**：内部 Message 直接包裹 vercel-ai 类型，不需要独立 ContentBlock enum：

```rust
// coco-types re-exports vercel-ai types as version-agnostic aliases:
pub use vercel_ai_provider::LanguageModelV4Message as LlmMessage;
pub use vercel_ai_provider::LanguageModelV4Prompt as LlmPrompt;
pub use vercel_ai_provider::UserContentPart as UserContent;
pub use vercel_ai_provider::AssistantContentPart as AssistantContent;
// ... etc.

pub struct UserMessage {
    pub message: LlmMessage,   // User variant (alias for vercel-ai type)
    pub uuid: Uuid,
    pub is_meta: bool,
    pub is_virtual: bool,
    ...
}
```

**版本隔离**：所有 crate 通过 `coco_types::{LlmMessage, UserContent, ...}` 引用。
升级 vercel-ai v5 时只改 coco-types 的 re-export，其他代码零修改。

**零转换**：`normalize_for_api()` 只过滤消息，发 API 直接取 `.message`。

```
Internal: Message = LlmMessage + 元数据包裹
    ↓ normalize_for_api()（过滤，不做类型转换）
    ↓ 取 .message 字段
API: LlmPrompt (= Vec<LlmMessage>)
    ↓ provider.do_generate() / do_stream()
Wire: provider-specific JSON
```

### Reverse path: Stream → AssistantMessage

```rust
// coco-inference (L2)
/// vercel-ai stream → 内部 AssistantMessage
pub fn collect_stream_to_message(
    stream: impl Stream<Item = StreamPart>,
    event_tx: &mpsc::Sender<coco_types::CoreEvent>,
) -> (AssistantMessage, TokenUsage);
```

TS 中 API 返回的 `BetaMessage` 直接就是 `AssistantMessage.message`（零转换）。
coco-rs 中 vercel-ai `StreamPart` 需要组装成 `AssistantMessage`（有转换）。

## Error Handling: TS vs cocode-rs vs coco-rs

### 三层错误模型

coco-rs 的错误处理融合 TS 设计和 cocode-rs 基础设施，形成三层模型：

```
┌─────────────────────────────────────────────────────────────────┐
│ L1: 系统级错误 (from cocode-rs) — StatusCode + ErrorExt        │
│     StatusCode (XX_YYY) + is_retryable() + retry_after()       │
│     虚拟栈追踪 (#[snafu(implicit)] Location)                    │
│     output_msg() 自动 cause chain 格式化                        │
│     用于: API 错误、IO 错误、网络错误、配置错误                   │
├─────────────────────────────────────────────────────────────────┤
│ L2: 工具验证错误 (from TS) — ValidationResult + error_code     │
│     Tool-local error_code（同一 code 在不同 tool 含义不同）      │
│     message 给模型看，error_code 给遥测用（模型不可见）           │
│     用于: validateInput() 阶段，在权限检查之前执行                │
├─────────────────────────────────────────────────────────────────┤
│ L3: 遥测安全层 (from TS, cocode-rs 无对应) — telemetry_msg()   │
│     区分用户消息和遥测消息，防止路径/代码泄露到分析系统           │
│     用于: 所有发送到 OTel 的错误消息                             │
└─────────────────────────────────────────────────────────────────┘
```

### 设计决策

**L1 复用 cocode-rs（比 TS 更好）：**

| 维度 | TS | cocode-rs | 结论 |
|------|-----|-----------|------|
| 错误分类 | 字符串匹配 + constructor name | 编译期 `StatusCode` 元数据 | cocode-rs 更安全 |
| 重试语义 | 手动判断 | `is_retryable()` trait + 编译期标记 | cocode-rs 更可靠 |
| 栈追踪 | JS runtime stacktrace | `#[snafu(implicit)] Location` 虚拟栈 | cocode-rs 更精确 |
| 用户消息 | 字符串拼接 | `output_msg()` 自动 cause chain | cocode-rs 更一致 |
| 错误类型 | 多 class 继承 (`ClaudeError`, `AbortError`, `ShellError`) | 统一 `ErrorExt` trait | cocode-rs 更统一 |

**L2 对齐 TS（保持遥测能力）：**

TS 的 `ValidationResult.errorCode` 是 tool-local 的遥测维度（无全局注册表）。
`StatusCode` 粒度太粗（只能区分 `InvalidArguments`，无法区分 "PDF 页码无效" vs "文件过大"）。
两者是不同层次，不矛盾：

```
StatusCode::InvalidArguments (02_001) → 路由维度: 不重试、记日志
error_code: 7 (FileReadTool)         → 分析维度: "PDF 页码格式错误最多"
```

**L3 需新增（TS 有，cocode-rs 无）：**

TS 的 `TelemetrySafeError` 防止文件路径和代码片段泄露到遥测系统。建议在 `ErrorExt` 增加：

```rust
trait ErrorExt {
    fn output_msg(&self) -> String;        // 用户 + 日志
    fn telemetry_msg(&self) -> String {    // OTel (脱敏)
        self.output_msg()                  // 默认同 output_msg
    }
}
```

### 错误流: 工具执行全路径

```
Tool use received from API stream
  ↓
1. Parse input (serde/JSON schema)
  ├─ Parse error → StatusCode::ParseError → <tool_use_error> to model
  │
2. validate_input() → ValidationResult
  ├─ Invalid { message, error_code }
  │   ├─ Model 看到: <tool_use_error>{message}</tool_use_error>
  │   ├─ OTel 记录: { tool_name, error_code, message.telemetry_msg() }
  │   └─ 不执行后续步骤
  │
3. check_permissions() → PermissionDecision
  ├─ Deny { message, reason }
  │   ├─ OTel: { tool_name, decision: "deny", reason }
  │   └─ <tool_use_error>{message}</tool_use_error> to model
  ├─ Ask → 交互式权限提示
  │
4. execute() → Result<ToolResult<Value>, ToolError>
  ├─ Ok(result) → tool_result block to model
  ├─ Err(tool_error)
  │   ├─ tool_error.status_code() → 决定是否重试
  │   ├─ tool_error.output_msg() → 用户看到
  │   ├─ tool_error.telemetry_msg() → OTel (脱敏)
  │   └─ <tool_use_error>{output_msg}</tool_use_error> to model
  │
5. PostToolUse hooks
  ├─ Hook failure → 记录但不影响 tool result
```

### TS 工具错误分类 (参考)

TS 的 `classifyToolError()` 策略（coco-rs 通过 StatusCode 自然实现）：

| TS 分类策略 | coco-rs 对应 |
|------------|-------------|
| `TelemetrySafeError` → 用预审消息 | `telemetry_msg()` (P2 新增) |
| `errno` code (ENOENT, EACCES) → 保留 | `StatusCode::FileNotFound`, `StatusCode::PermissionDenied` |
| `.name` property (minification-safe) | Rust 无 minification，enum variant 名天然稳定 |
| Fallback → generic "Error" | `StatusCode::Unknown` |

### 工具 error_code 约定

error_code 是 **tool-local** 的（TS 也没有全局注册表）。建议每个 tool 在实现时定义自己的 code 语义：

```rust
// FileReadTool 示例
impl Tool for FileReadTool {
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        if !path.exists() {
            return ValidationResult::Invalid {
                message: format!("File not found: {path}"),
                error_code: Some(1),  // 1 = resource not found
            };
        }
        if pdf_pages > MAX_PAGES {
            return ValidationResult::Invalid {
                message: format!("PDF exceeds {MAX_PAGES} page limit"),
                error_code: Some(8),  // 8 = parameter exceeds bounds
            };
        }
        ValidationResult::Valid
    }
}
```

code 含义由 tool 自行定义，不同 tool 的同一 code 可能含义不同。
遥测系统通过 `(tool_name, error_code)` 二元组进行分析。

## Crate Count

| Group | Count | Source | Version |
|-------|-------|--------|---------|
| common/ | 4 | error, otel, types, config | v1 |
| utils/ | 26 | 24 from cocode-rs + frontmatter + cursor | v1 |
| vercel-ai/ | 8 | from cocode-rs | v1 |
| services/ | 4 | inference, mcp, lsp, compact | v1 |
| core/ | 5 | messages, context, permissions, tool, tools | v1 |
| exec/ | 3 | shell, sandbox, process-hardening | v1 |
| root modules | 7 | commands, skills, hooks, tasks, memory, plugins, keybindings | v1 |
| app/ | 5 | query, state, session, tui, cli | v1 |
| standalone | 2 | bridge, retrieval | v1 |
| **v1 subtotal** | **64** | | |
| v2 features | 4 | coordinator, vim, voice, assistant | v2 |
| v3 features | 2 | remote (includes proxy) | v3 |
| v3 TBD | ~7 | computerUse, claudeInChrome, nativeInstaller, teamMemorySync, PromptSuggestion, tips, vcr | v3 |
| **Grand total** | **~77** | | |

Note: cocode-rs has 81 crates. coco-rs v1 has 64 (provider-sdks removed, core/ consolidated, features/ flattened). v2/v3 add ~13 more.

## TS Dir → Crate → Plan Doc Mapping

Every TS source directory maps to a Rust crate, and every crate has a plan doc.

| TS dir(s) | Rust crate | Plan doc | Version |
|-------------------|-----------|----------|---------|
| `types/` | `coco-types` | `crate-coco-types.md` | v1 |
| `constants/`, `utils/settings/`, `utils/model/`, `migrations/`, `services/remoteManagedSettings/`, `services/settingsSync/` | `coco-config` | `crate-coco-config.md` | v1 |
| `services/api/`, `utils/auth.ts`, `services/oauth/`, `services/policyLimits/`, `services/tokenEstimation.ts`, `services/rateLimitMessages.ts`, `services/claudeAiLimits.ts` | `coco-inference` | `crate-coco-inference.md` | v1 |
| `services/analytics/`, `utils/telemetry/` | `coco-otel` | `crate-coco-otel.md` | v1 |
| `services/mcp/` | `coco-mcp` | `crate-coco-mcp.md` | v1 |
| `services/compact/` | `coco-compact` | `crate-coco-compact.md` | v1 |
| `utils/messages/`, `history.ts`, `cost-tracker.ts` | `coco-messages` | `crate-coco-messages.md` | v1 |
| `context.ts`, `utils/claudemd.ts`, `utils/attachments.ts`, `services/AgentSummary/` | `coco-context` | `crate-coco-context.md` | v1 |
| `utils/permissions/` | `coco-permissions` | `crate-coco-permissions.md` | v1 |
| `Tool.ts`, `services/tools/`, `tools.ts` | `coco-tool` | `crate-coco-tool.md` | v1 |
| `tools/` (43 dirs) | `coco-tools` | `crate-coco-tools.md` | v1 |
| `utils/bash/`, `utils/Shell.ts`, `utils/shell/`, `tools/BashTool/{bashPermissions,bashSecurity,readOnlyValidation,commandSemantics,destructiveCommandWarning,shouldUseSandbox,modeValidation,sedEditParser}.ts` | `coco-shell` | `crate-coco-shell.md` | v1 |
| `commands/` (~56 dirs) | `coco-commands` | `crate-coco-commands.md` | v1 |
| `query/`, `QueryEngine.ts`, `utils/processUserInput/` | `coco-query` | `crate-coco-query.md` | v1 |
| `skills/`, `schemas/hooks.ts`, `utils/hooks/`, `tasks/`, `memdir/`, `services/extractMemories/`, `services/SessionMemory/`, `services/autoDream/`, `plugins/`, `services/plugins/`, `keybindings/` | `coco-skills`, `coco-hooks`, `coco-tasks`, `coco-memory`, `coco-plugins`, `coco-keybindings` | `crate-coco-modules.md` | v1 |
| `state/`, `bootstrap/`, `components/`, `screens/`, `ink/`, `outputStyles/`, `entrypoints/`, `cli/`, `server/` | `coco-state`, `coco-session`, `coco-tui`, `coco-cli` | `crate-coco-app.md` | v1 |
| `bridge/` | `coco-bridge` | `crate-coco-bridge.md` | v1 |
| `coordinator/`, `utils/swarm/` | `coco-coordinator` | `crate-coco-coordinator.md` | v2 |
| `vim/` | `coco-vim` | `crate-coco-vim.md` | v2 |
| `voice/`, `services/voice*.ts` | `coco-voice` | `crate-coco-voice.md` | v2 |
| `assistant/` | `coco-assistant` | `crate-coco-assistant.md` | v2 |
| `remote/`, `upstreamproxy/` | `coco-remote` | `crate-coco-remote.md` | v3 |
| `utils/computerUse/`, `utils/claudeInChrome/`, `utils/nativeInstaller/`, `services/teamMemorySync/`, `services/PromptSuggestion/`, `services/tips/`, `services/vcr.ts` | TBD | — | v3 |
| `buddy/`, `moreright/`, `native-ts/` | — | — | SKIP |

**Copy from cocode-rs** (38 crates, no plan doc needed): `coco-error`, 24 `utils/*`, 8 `vercel-ai/*`, `coco-sandbox`, `coco-process-hardening`, `coco-retrieval`
**HYBRID from cocode-rs** (1 crate, has plan doc): `coco-lsp` (cocode-rs base + TS diagnostic/plugin extensions → `crate-coco-lsp.md`)

## Config Home Directory

TS uses `~/.claude/` and `~/.claude.json`. coco-rs uses `~/.coco/` and `~/.coco.json`.

When referencing TS behavior in docs, always clarify which path is TS vs Rust:
- "TS: `~/.claude.json`" / "Rust: `~/.coco.json`"
- "TS: `~/.claude/settings.json`" / "Rust: `~/.coco/settings.json`"

## Review Checklist

When modifying any doc in this directory:

1. **Single source**: Does this doc own this information? Check the Document Map table above.
2. **No type redefinition**: If defining a struct/enum, verify it is not already defined in another crate doc.
3. **Dependencies stated**: Every crate doc must have an explicit `## Dependencies` section listing what it depends on and what it does NOT depend on.
4. **Layer compliance**: Verify the dependency does not violate the layer rules above.
5. **Canonical names**: Use the exact names from the Canonical Names table.
6. **TS source cited**: Every crate doc starts with `TS source: ...` listing the TS files it translates.
7. **No Option unless justified**: Numeric model fields use concrete types with defaults.
8. **Config isolation**: Feature crate config sections are `serde_json::Value` in Settings, deserialized by the owning crate.

## Known Gaps (from audit-gaps.md)

Previously deferred P1 gaps now documented (Round 3, April 2026):
- ~~Permissions auto-mode/yolo classifier~~ → **FIXED** in crate-coco-permissions.md
- ~~Auth system~~ → **FIXED** in crate-coco-inference.md (actual 2002 LOC, not 65K)
- ~~coco-messages: 114 functions~~ → **FIXED** in crate-coco-messages.md (15 categories)
- ~~coco-compact submodules~~ → **FIXED** in crate-coco-compact.md
- ~~coco-shell submodules~~ → **FIXED** in crate-coco-shell.md
- ~~coco-inference: claude.ts, withRetry.ts, filesApi.ts~~ → **FIXED** in crate-coco-inference.md

Round 4 factual errors fixed (April 2026, 35-area cross-verification):
- ~~crate-coco-skills.md: invented SHA-256 fingerprint~~ → **FIXED** (replaced with TS file extraction security model)
- ~~crate-coco-tools.md: cell_index:i32~~ → **FIXED** (cell_id:String + edit_mode + cell_type)
- ~~crate-coco-shell.md: security IDs 11-24 wrong~~ → **FIXED** (renumbered to match TS exactly, 23 IDs)
- ~~crate-coco-shell.md: timeout 120s~~ → **FIXED** (30 minutes)
- ~~crate-coco-shell.md: commandSemantics wrong~~ → **FIXED** (exit-code interpretation, not classification)
- ~~crate-coco-shell.md: modeValidation flag restriction~~ → **FIXED** (TS auto-allows all 7 commands without flag checks)
- ~~crate-coco-keybindings.md: fabricated action names~~ → **FIXED** (73 exact TS `namespace:camelCase` names)
- ~~crate-coco-context.md: FileHistoryState HashMap~~ → **FIXED** (ordered Vec + content-addressed files on disk)
- ~~crate-coco-context.md: fileHistory.ts 200 LOC~~ → **FIXED** (~1110 LOC)
- ~~crate-coco-tasks.md: slug from prompt~~ → **FIXED** (random word slug, not prompt-derived)
- ~~crate-coco-remote.md: 4001 exponential backoff~~ → **FIXED** (linear backoff)
- ~~crate-coco-app.md: IDE bridge direct WebSocket~~ → **FIXED** (MCP-based + CCR daemon + DirectConnect)

Remaining deferred — will be documented during implementation:

| Priority | Gap | Phase |
|----------|-----|-------|
| P2 | AppState: 60+ fields (remote, notifications, attribution, tungsten, speculation, plugins, MCP) | Phase 7 |
| P2 | ErrorExt::telemetry_msg() 遥测脱敏 (TS TelemetrySafeError 对应) | Phase 2 |
| P3 | coco-config cocode-rs patterns (ConfigSection trait, ConfigResolver) | Phase 2 |
| P3 | 工具执行 errno 保留 — IO 错误在 OTel 中保留操作系统级 errno | Phase 4 |
| P1 | coco-otel L2: span 层级体系 (6 span types documented in crate-coco-otel.md — implementation pending) | Phase 1 |
| P1 | coco-otel L3: 665 应用事件 (documented in crate-coco-otel.md — implementation pending) | Phase 3 |
| P1 | coco-otel L6: 运营控制 (sampling/killswitch/PII safety — elevated from deferred, documented) | Phase 3 |
| P2 | coco-otel L4: 业务 metrics (token/cost/LOC/session/active_time/PR/commit, 8+) | Phase 3 |
| P2 | coco-otel L5: 自定义 exporter (BigQuery/1P Event Logging/Perfetto/Beta tracing) | Phase 3 |

## React Hooks -> Rust Architecture

TS `hooks/` has 85 files. In React, hooks are the primary mechanism for connecting business logic to UI state. In Rust, these patterns translate differently:

| React pattern | Rust equivalent |
|--------------|-----------------|
| `useState` + `useEffect` with API calls | Struct method + tokio task |
| `useCallback` memoization | Regular function (no GC) |
| React Context provider | `Arc<RwLock<T>>` in AppState |
| `useRef` for mutable state | `&mut self` or interior mutability |
| Hook composition (hook calls hook) | Trait composition or struct delegation |

**Classification**: ~67 hooks are pure React UI wiring (no Rust port). **16 have core business logic** — see `ts-to-rust-mapping.md` "React Hooks with Business Logic" table.

**Translation rule**: Extract the business logic (state machine, algorithm, API call) into the target crate as a regular async function or struct method. The React-specific wiring (useState, useEffect, re-render triggers) is replaced by Rust's ownership model and tokio channels.

Key v1 hooks: `useTasksV2` (file watcher → tokio::fs), `useFileHistorySnapshotInit` (HashMap state), `useIDEIntegration` (bridge callbacks).
Key v2 hooks: `useSwarmInitialization` (coordinator), `useHistorySearch` (regex + typeahead), `useScheduledTasks` (tokio::time cron).

Similarly, `context/` (9 React Contexts) maps to fields in `coco-state::AppState` — React's Context.Provider pattern is replaced by `Arc<RwLock<AppState>>` shared across components.

## Previously Missing TS Mappings (now added to ts-to-rust-mapping.md)

Added in Round 2 review:
- 6 services/ files: awaySummary, diagnosticTracking, internalLogging, mcpServerApproval, preventSleep, claudeAiLimitsHook
- 4 utils/ subdirs: filePersistence, dxt, deepLink, background
- 2 voice files enumerated: voiceKeyterms, voiceStreamSTT
- 16 React hooks with business logic documented

## File Index

| File | What it is |
|------|-----------|
| `CLAUDE.md` | This file. Entry point. Resolves ambiguities. Review rules. |
| `coco-rs-plan.md` | Master plan: directory structure, dependency graph, phases, copy/rewrite decisions |
| `ts-to-rust-mapping.md` | Every TS source directory -> Rust crate (version, strategy) |
| `ts-utils-mapping.md` | All 338 TS `utils/*.ts` files -> Rust target |
| `multi-provider-plan.md` | Multi-LLM architecture: flow, beta headers, provider branching |
| `message-pipeline.md` | Unified message mutation pipeline: `MessagePass` trait, `run_message_passes` helper, normalize + compact pass catalog, drift-detection invariant, TS-parity mapping |
| `tool-result-budget-plan.md` | Two-level Tool Result Budget plan: Level 1 per-tool persistence (`<persisted-output>` + 2KB preview + session-scoped storage) and Level 2 per-message aggregate budget (`ContentReplacementState` + `enforceToolResultBudget`). Owners: `coco-tool-runtime` + `coco-query` + `coco-session` (re-routed from `coco-context`). |
| `parity-skills-commands-plugins.md` | Deep-review parity plan for `coco-skills` / `coco-commands` / `coco-plugins`: every P1/P2/P3 gap mirrors TS define/behavior/UI with file:line citations. Round 11 (May 2026). Cross-references `crate-coco-{skills,commands,plugins}.md` and `audit-gaps.md`. |
| `parity-fix-tracker.md` | **SINGLE SOURCE OF TRUTH / fix tracker** for whole-codebase TS→Rust parity: per-finding verified status (fixed/partial/open) + re-prioritized backlog, each open item carrying its TS ref, current Rust location, and fix sketch. Baseline re-validated against HEAD `748580242` (2026-06-05). Predecessor audit docs folded in and deleted. |
| `ui/agent-console-design.md` | Final agent-console product architecture: TEA / `AppState`, transcript, streaming, activity, bottom pane, prompts, pickers, pagers, effects, and verification. |
| `ui/terminal-surface-design.md` | Native terminal-surface constraints: native scrollback as the base surface, SurfaceTerminal, cursor/resize/suspend behavior, and terminal verification. |
| `ui/migration-roadmap.md` | Historical TUI migration notes; not the final product organization. |
| `ui/rendering-hardening-and-rollback.md` | TUI rendering-layer hardening: cursor pin (single decision point + post-draw queue) and suspend/resume (Ctrl+Z keystroke + libc::kill). Records why the stock-ratatui inline viewport experiment was rolled back. |
| `ui/native-scrollback-architecture.md` | Native terminal scrollback architecture for coco-tui, based on codex-rs custom terminal / insert-history / transcript-reflow design rather than stock ratatui inline viewport. |
| `ui/single-render-path-refactor.md` | Refactor plan to collapse the streaming→scrollback seam onto a single render path (codex-rs stable/tail watermark model), removing the `#160` provisional-commit / finalize-reconcile machinery and restoring native-scrollback Core Decision #4. |
| `ui/tui-v2-design.md` | TUI v2 target: fair three-way comparison (coco / codex-rs / jcode), root-cause of the streaming-seam churn, anchored finalize without rasterized fingerprint reconciliation (§6.2/§6.2.1, twice source-corrected), engine-owned viewport seating, and the staged Scope B implementation plan (§10). Supersedes the dual-projection model in `ui/single-render-path-refactor.md` as the forward design. |
| `config-file-map.md` | Every file coco-rs reads/writes, which crate owns it |
| `audit-gaps.md` | Gap analysis with fix status and priority |
| `current-gap-fix-plan.md` | Current prioritized fix plan for active gaps, plus stale audit rows to clean up before assigning implementation work. |
| `crate-coco-types.md` | Foundation types (Message, Permission, Tool, Task, Provider) |
| `crate-coco-config.md` | Settings, model config, provider config, effort, fast mode |
| `crate-coco-inference.md` | LLM client, retry engine, auth (OAuth/API key/AWS/GCP), files API, bootstrap, rate limiting |
| `crate-coco-tool.md` | Tool trait, executor, registry |
| `crate-coco-tools.md` | 40+ tool implementations |
| `crate-coco-mcp.md` | MCP server lifecycle, config, auth, channels (23 TS files, 12K LOC) |
| `crate-coco-query.md` | Multi-turn agent loop |
| `crate-coco-context.md` | System context, attachments, CLAUDE.md discovery |
| `crate-coco-messages.md` | Message creation (13), normalization (10), filtering (11), predicates (19), lookups (8), history, cost tracking |
| `crate-coco-permissions.md` | Permission evaluation pipeline, auto-mode/yolo classifier, denial tracking |
| `crate-coco-compact.md` | Context compaction (full, micro, auto, session memory), grouping, post-compact cleanup, API microcompact |
| `crate-coco-shell.md` | Shell execution, bash security, destructive warnings, sandbox decisions, mode validation (rewrite from TS 23K LOC) |
| `crate-coco-commands.md` | Slash command system |
| `crate-coco-modules.md` | Skills, hooks, tasks, memory, plugins, keybindings |
| `crate-coco-tui.md` | TUI: TEA architecture, 17 widgets, 41 message renderers, 14+ overlays, streaming, notification |
| `crate-coco-app.md` | State, session, CLI |
| `crate-coco-bridge.md` | IDE bridge: CCR daemon, DirectConnect, permission relay, transport |
| `crate-coco-lsp.md` | LSP: 6 server states, diagnostic dedup, crash recovery (HYBRID: cocode-rs base + TS extensions) |
| `crate-coco-coordinator.md` | Coordinator mode, swarm teams, backends (v2) |
| `crate-coco-vim.md` | Vim state machine, motions, operators (v2) |
| `crate-coco-voice.md` | Voice recording, STT, hold-to-talk (v2) |
| `crate-coco-assistant.md` | Session history pagination (v2) |
| `crate-coco-remote.md` | Remote session WS, upstream proxy (v3) |
