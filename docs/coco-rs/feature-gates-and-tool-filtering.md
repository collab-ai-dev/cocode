# Feature Gates 与 Tool 过滤管线设计

> Status: Design proposal
> Scope: `coco-rs/`
> Owners: features registry + tool-runtime
> Last updated: 2026-04-26

## 1. 设计目标

1. **集中化能力开关**：把分散在 `MemoryConfig.enabled` / `SandboxConfig.enabled` / `RetrievalConfig.enabled` … 等十几处独立 bool 收口到单一 `Features` 注册表。
2. **粗粒度 Feature**：每个 Feature 对应一个用户可感知的"大能力"；子细节下沉到对应 `*Config` struct，**不**展开成多个 Feature。
3. **多层 tool 过滤管线**：LLM 看到的 tool schema 是多层 filter 的交集；Feature gate 只是其中一层，与 ModelRole / PermissionMode / Agent allow-deny / MCP 可用性正交组合。
4. **向 LLM 节省 token**：稳定但可选的工具（WebSearch / WebFetch）支持显式关闭以从 system prompt 中移除其 schema。
5. **不考虑向后兼容**：直接删除现有分散字段，不保留 legacy alias。

## 2. 非目标

- **不收口政策/安全开关**：`disable_all_hooks`、`allow_managed_permission_rules_only`、`disable_bypass_mode`、`strict_plugin_only_customization` 等是 enterprise policy，不是用户 feature，留在 `Settings` 顶层。
- **不收口"配置即启用"子系统**：`hooks` / `plugins` / `skills` / `telemetry` / `cron` —— 配了就生效、没配就停，不需要额外开关。
- **不收口运行时状态**：`AppState` 中的 `is_busy` / `mcp_connected` / `has_compacted` 等是会话状态，不是 config。
- **不收口 UI 偏好**：`syntax_highlighting_disabled` / `auto_title` / `file_checkpointing_enabled` 等留在 `Settings`/`SessionSettings` 作为运行参数。
- **不收口 sub-system 内部子开关**：`MemoryConfig.extraction_enabled` / `RetrievalConfig.reranker.enabled` / `SystemReminderConfig` 的 51 个 reminder 子开关 —— 这些是子系统内部参数，由各自 `*Config` 自治。

## 3. Feature 注册表

### 3.1 判定规则

> **是否应是 Feature？**
> - 关掉它会导致整个用户能力消失？→ 候选
> - 是 enterprise policy？→ **否**，留 Settings
> - 配了 config 就生效、不配自然失效？→ **否**，不需要 gate
> - 是基础设施（关掉就崩塌）？→ **否**，永远开
> - 是会话状态或派生量？→ **否**，不是 config
> - 是子系统内部子开关？→ **否**，留各自 Config
> - 是纯 UI 偏好？→ **否**，留 Settings

剩下两类：

| 类型 | Feature | 用途 |
|------|---------|------|
| **Token-economy gate** | Stable, default=true | 用户可关掉以从 LLM schema 移除工具节省 token |
| **Lifecycle gate** | UnderDevelopment, default=false | 实验/未稳定能力，驱动 `/experimental` 菜单 |
| **行为/安全 gate** | Stable, default 视风险 | 影响安全/资源/隔离边界 |

### 3.2 最终列表

```rust
pub enum Feature {
    // Token-economy gate（Stable，default=true，用户可关省 token）
    WebSearch,
    WebFetch,
    TaskV2,                      // V2 task tooling vs V1 TodoWrite — 互斥

    // 行为/安全 gate（Stable，default=false 安全保守）
    Sandbox,

    // 核心子系统 opt-out gate（Stable，default=true，仅用于显式关闭）
    PlanMode,

    // /experimental 菜单（UnderDevelopment，default=false）
    AutoMemory,
    Retrieval,
    AgentTeams,
    Worktree,
    Lsp,
}
```

完整 spec（节选 stable + experimental 主项；其余 skill/command 子 gate 见 `common/types/src/features.rs`）：

```rust
const FEATURES: &[FeatureSpec] = &[
    FeatureSpec { id: WebSearch,  key: "web_search",  stage: Stable,            default_enabled: true  },
    FeatureSpec { id: WebFetch,   key: "web_fetch",   stage: Stable,            default_enabled: true  },
    FeatureSpec { id: TaskV2,     key: "task_v2",     stage: Stable,            default_enabled: true  },
    FeatureSpec { id: Sandbox,    key: "sandbox",     stage: Stable,            default_enabled: false },
    FeatureSpec { id: PlanMode,   key: "plan_mode",   stage: Stable,            default_enabled: true  },
    FeatureSpec { id: AutoMemory, key: "auto_memory", stage: UnderDevelopment,  default_enabled: false },
    FeatureSpec { id: Retrieval,  key: "retrieval",   stage: UnderDevelopment,  default_enabled: false },
    FeatureSpec { id: AgentTeams, key: "agent_teams", stage: UnderDevelopment,  default_enabled: false },
    FeatureSpec { id: Worktree,   key: "worktree",    stage: UnderDevelopment,  default_enabled: false },
    FeatureSpec { id: Lsp,        key: "lsp",         stage: UnderDevelopment,  default_enabled: false },
];
```

**`Feature::TaskV2` 特殊语义**：这是**互斥开关**，不是单纯 token-economy。开 → `TaskCreate`/`TaskGet`/`TaskList`/`TaskUpdate` 暴露给模型、`TodoWrite` 隐藏；关 → 反之。`TaskOutput` 与 `TaskStop` 操作的是后台任务命名空间（Bash `run_in_background`、agent spawn），与 V1/V2 plan-item 正交，永不被这个 gate 影响。对应 TS `isTodoV2Enabled()` (`utils/tasks.ts:133-139`)。

**`Feature::PlanMode` 特殊语义**：**默认开**的 opt-out gate，专为"明确不需要 plan mode"的用户省 token（reminder + 工具 schema）。关 → 整个 plan-mode 子系统对该会话失效，四处协同执行：
1. **工具**（Layer 1 `is_enabled`）：`EnterPlanMode` / `ExitPlanMode` 从模型工具集中隐藏。
2. **Reminder**：三个 plan-mode 生成器（`plan_mode` / `plan_mode_exit` / `plan_mode_reentry`）短路返回 `None`（`GeneratorContext.plan_mode_feature_enabled` 由引擎从 `features` 写入）。即使 resume 的 transcript 仍带 `permission_mode == Plan` 也不再注入。
3. **TUI 进入路径**：`PermissionMode::next_in_cycle` 新增 `plan_available` 形参——Shift+Tab 循环跳过 `Plan` 这一档，`Tab`（`toggle_plan_mode`）成为 no-op（已在 Plan 的会话仍可退出）。
4. **`/plan` 命令**：`dispatch_plan` 直接提示"plan mode disabled"，不翻转模式。

调用时 `coco_permissions` 的 `PermissionMode::Plan` 分支保留不动——它只在模式真的是 Plan 时生效，而上面四处保证默认会话进不去 Plan。`PlanModeConfig.workflow / phase4_variant / verify_execution` 仍是子系统内部细节，与本 gate 正交。

### 3.3 与原分散字段的对应

| 删除字段 | 替换为 | 子开关去向 |
|----------|-------|-----------|
| `MemoryConfig.enabled` | `Feature::AutoMemory` | `extraction_enabled` / `team_memory_enabled` / `relevant_memories_enabled` / `skip_index` 留 `MemoryConfig` |
| `SandboxConfig.enabled` | `Feature::Sandbox` | `mode` / `allow_network` / `excluded_commands` 留 `SandboxConfig` |
| `RetrievalConfig.enabled` | `Feature::Retrieval` | `reranker.enabled` / `query_rewrite.enabled` / `repo_map.enabled` / `watch.enabled` 留 `RetrievalConfig` |
| `WorktreeConfig.enabled` | `Feature::Worktree` | — |
| `SwarmConfig.enabled` | `Feature::AgentTeams` | swarm 内部参数留 `SwarmConfig` |
| `LspConfig.enabled` | `Feature::Lsp` | 各 LSP server 配置留 `LspConfig` |
| `WebSearchConfig.enabled` | `Feature::WebSearch` | `provider` / `api_key` / `max_results` 留 `WebSearchConfig` |
| `WebFetchConfig.enabled` | `Feature::WebFetch` | `timeout_secs` / `max_content_length` / `user_agent` 留 `WebFetchConfig` |

### 3.4 不进 Feature 的常见误归项（说明性反例）

| 项 | 不归 Feature 的原因 |
|----|--------------------|
| `Hooks` | 配了 `hooks` 字段即启用 |
| `Plugins` | `plugins/` 目录有 `PLUGIN.toml` 即加载 |
| `Skills` | `skills/` 目录有 `.md` 即注册 |
| `Telemetry` | `telemetry.sinks` 非空即上报 |
| `Cron` | 创建过 schedule 才生效 |
| `SystemReminders` | 基础设施常开；51 子开关在 `SystemReminderConfig` |
| `StreamingTools` | 基础设施常开 |
| `AutoCompact` | 留 `CompactConfig.auto_trigger.enabled` 子字段，与 threshold 同级 |
| `FileCheckpointing` | 留 `Settings.file_checkpointing_enabled` 运行参数 |
| `AutoTitle` | UX 偏好，留 `SessionSettings.auto_title` |
| `ShellSnapshot` | debug-only 选项，留 `ShellConfig.disable_snapshot` |
| `ToolResultPersistence` | 性能优化，留 `ToolConfig.enable_result_persistence` |
| `AutoBackgroundOnTimeout` | 操作偏好，留 `ToolConfig.auto_background_on_timeout` |
| `disable_all_hooks` | enterprise policy，留 `Settings` 顶层 |

## 4. 数据结构

```rust
// coco-rs/common/types/src/features.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Stable,
    UnderDevelopment,
    Experimental {
        name: &'static str,
        menu_description: &'static str,
        announcement: &'static str,
    },
    Deprecated,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature { /* 8 项见 §3.2 */ }

impl Feature {
    pub fn key(self) -> &'static str { self.info().key }
    pub fn stage(self) -> Stage { self.info().stage }
    pub fn default_enabled(self) -> bool { self.info().default_enabled }
}

#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    pub id: Feature,
    pub key: &'static str,
    pub stage: Stage,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    enabled: BTreeSet<Feature>,
}

impl Features {
    pub fn with_defaults() -> Self { /* 从 FEATURES 表读 default_enabled */ }
    pub fn enabled(&self, f: Feature) -> bool { self.enabled.contains(&f) }
    pub fn enable(&mut self, f: Feature) -> &mut Self { ... }
    pub fn disable(&mut self, f: Feature) -> &mut Self { ... }
    pub fn apply_map(&mut self, m: &BTreeMap<String, bool>) { ... }
    pub fn normalize_dependencies(&mut self) { /* 当前为空，扩展点 */ }
}

pub fn all_features() -> impl Iterator<Item = &'static FeatureSpec>;
pub fn feature_for_key(key: &str) -> Option<Feature>;
pub fn is_known_feature_key(key: &str) -> bool;
```

## 5. 配置形态（JSON）

`~/.coco/settings.json` 或 `.claude/settings.json`：

```json
{
  "features": {
    "web_search": true,
    "web_fetch": true,
    "sandbox": false,
    "auto_memory": false,
    "retrieval": false,
    "agent_teams": false,
    "worktree": false,
    "lsp": false
  },

  "memory": {
    "extraction_enabled": true,
    "team_memory_enabled": false,
    "relevant_memories_enabled": true,
    "skip_index": false
  },

  "retrieval": {
    "reranker":      { "enabled": true,  "backend": "local" },
    "query_rewrite": { "enabled": true,  "translation": true, "expansion": false },
    "repo_map":      { "enabled": true },
    "watch":         { "enabled": false }
  },

  "sandbox": {
    "mode": "workspace_write",
    "allow_network": false,
    "excluded_commands": []
  },

  "web_search": {
    "provider": "tavily",
    "api_key":  "tvly-...",
    "max_results": 10
  },

  "web_fetch": {
    "timeout_secs": 30,
    "max_content_length": 5000000
  },

  "compact": {
    "auto_trigger": { "enabled": true, "threshold": 0.85 },
    "token_budget_continuation": true
  },

  "plan_mode": {
    "workflow": "five_phase",
    "phase4_variant": "trim",
    "verify_execution": false
  },

  "telemetry":       { "sinks": ["otel"] },
  "hooks":           { "PreToolUse": [] },
  "system_reminder": { "plan_mode": true, "todo_reminder": true, "relevant_memories": true }
}
```

**约定：**
- `features` 段：JSON object，`{ "<key>": <bool> }`，对应 `Feature::<X>`。
- 其他 section：subsystem 内部参数；**不再有顶层 `enabled` 字段**（开关由 `features` 决定，或由"配置即启用"决定）。
- env override：单一命名空间 `COCO_FEATURE_<UPPER_KEY>=1/0`（删除 `COCO_DISABLE_AUTO_MEMORY` / `COCO_SANDBOX_ENABLED` 等离散 env）。
- CLI override：`--enable web_search` / `--disable retrieval`。

## 6. Resolution 流程

在 `coco-rs/common/config/src/runtime.rs:build_runtime_config` 内：

```rust
let features = Features::with_defaults()
    .apply_map(&settings.features)                          // settings.json 的 [features] 段
    .apply_env(&env, "COCO_FEATURE_")                       // 单一 env 命名空间
    .apply_overrides(&runtime_overrides.feature_overrides)  // CLI --enable/--disable
    .normalize_dependencies();                              // 当前为空，预留扩展点
```

```rust
pub struct RuntimeConfig {
    // 现有字段不变（memory / sandbox / retrieval / web_search / ...），
    // 但其内部的 `enabled: bool` 字段已删除。
    pub memory:     MemoryConfig,
    pub sandbox:    SandboxConfig,
    pub retrieval:  RetrievalConfig,
    pub web_search: WebSearchConfig,
    pub web_fetch:  WebFetchConfig,
    // ...

    // 新增：集中化 Feature 注册表
    pub features: Features,
}
```

## 7. Tool 过滤管线

LLM 实际看到的 tool schema = **多层 filter 的交集**。每层职责单一，不交叉。

### 7.1 过滤序列（schema-time）

```
all registered tools
  ├─ Layer 1: Tool::is_enabled(ctx)         ← 基础要求（Feature gate / OS / 硬依赖）
  ├─ Layer 2: ToolOverrides (per-model)     ← 模型在基线上的工具差异（extra/excluded）
  ├─ Layer 3: Agent allow/deny              ← subagent 进一步限定
  ├─ Layer 4: MCP 运行时可用性              ← 外部依赖（断开时清出 registry）
  └─ → tool schema 注入 LLM
```

> **没有 PermissionMode 这一层（对齐 TS）。** Plan 模式**不**收窄模型看到的 tool schema —— 任何模式下都注入完整工具集。Plan 模式的"只读"约束在**调用时**由 `coco_permissions` 强制执行（只读 / 写会话计划文件 → allow；其他写 → ask，非交互下 deny），外加 `<system-reminder>` plan 附件做软提示。在 schema 层裁剪会把模型推进所必需的工具一并删掉（`ExitPlanMode`、`AskUserQuestion`、写计划文件），并与权限层重复执法。详见 §9 与 `core/permissions/src/evaluate.rs`。

### 7.2 顺序的语义解释

| Layer | 问题 | 输出收窄方向 |
|-------|------|-------------|
| 1 | 这个 tool 在当前 build/配置下存在吗？ | 编译/Feature/OS 层面是否可用 |
| 2 | 当前模型在基线之上有什么调整？ | `ToolOverrides` 声明的 `extra`（gpt-5 引入 `apply_patch`）+ `excluded`（gpt-5 拒绝 `Edit`）|
| 3 | subagent 配置允许吗？ | 在已存活集合里再筛 |
| 4 | 外部依赖在线吗？ | 仅 MCP tool 受影响 |

> **PermissionMode 不在这条流水线里。** Plan 模式不裁剪 schema（见 §7.1 注），写工具在任何模式下都对模型可见；是否放行由调用时的权限评估决定（§9）。

### 7.3 Registry 实现

```rust
// coco-rs/core/tool-runtime/src/registry.rs

fn passes_filter_pipeline(tool: &dyn DynTool, ctx: &ToolUseContext) -> bool {
    let id = tool.id();
    // Layer 1: Tool 自检（含 Feature gate）
    tool.is_enabled(ctx)
        // Layer 2: 模型在基线上的差异（excluded 集合）
        && ctx.tool_overrides.permits(&id)
        // Layer 3: agent allow/deny
        && ctx.tool_filter.allows(&id)
}
// 无 PermissionMode 层：Plan 只读在调用时由 coco_permissions 执法（§9）。
// MCP 运行时可用性通过断开时 `deregister_by_server` 清出 registry，不在此函数显式过滤。
```

### 7.4 Tool::is_enabled 签名重构

```rust
// 旧
fn is_enabled(&self) -> bool { true }

// 新
fn is_enabled(&self, ctx: &ToolUseContext) -> bool { true }
```

Tool 自己声明前置条件（最常见就是 Feature gate）：

```rust
impl Tool for WebSearchTool {
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::WebSearch)
    }
}

impl Tool for WebFetchTool {
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::WebFetch)
    }
}

impl Tool for AgentTool {                 // subagent spawning
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::AgentTeams)
    }
}

impl Tool for EnterWorktreeTool {
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::Worktree)
    }
}

impl Tool for LspTool {
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::Lsp)
    }
}

// V1/V2 互斥示例（同一 gate 反向使用，TaskOutput/TaskStop 不重载）：
impl Tool for TaskCreateTool {       // 同样 TaskGet/TaskList/TaskUpdate
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::TaskV2)
    }
}

impl Tool for TodoWriteTool {
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        !ctx.features.enabled(Feature::TaskV2)
    }
}

// TaskOutput / TaskStop 不实现 is_enabled — 它们读后台任务命名空间
// (`ctx.task_handle`)，不是 V2 plan items，跨 V1/V2 共享。

// OS 限制示例（无 Feature 关联）：
impl Tool for AgentForkTool {
    fn is_enabled(&self, _ctx: &ToolUseContext) -> bool {
        cfg!(unix)
    }
}

// 大多数默认 tool 不需 override，使用默认 true
impl Tool for ReadTool { /* 不实现 is_enabled */ }
```

`Feature::AutoMemory` / `Feature::Retrieval` / `Feature::Sandbox` 不直接对应单一 tool，而是 subsystem 级 gate，由各自模块入口检查（不在 tool registry 这条路径上）。

### 7.5 ToolUseContext 新增字段

```rust
// coco-rs/core/tool-runtime/src/context.rs

pub struct ToolUseContext {
    // ... 现有字段（web_fetch_config / memory_config / sandbox_config / ...）

    // Features 注册表（Layer 1 用）
    pub features: Arc<Features>,

    // tool 过滤上下文（Layer 2-4 用）
    pub tool_overrides: Arc<ToolOverrides>,            // Layer 2
    pub permission_context: ToolPermissionContext,     // Layer 3 (mode 在内部)
    pub tool_filter: ToolFilter,                       // Layer 4
    pub mcp: McpHandleRef,                             // Layer 5（已存在）
}

// coco-types/src/tool_filter.rs — 类型定义见 §7.5b

pub struct ToolFilter {
    allowed: Option<HashSet<ToolId>>,  // None = 不限
    disallowed: HashSet<ToolId>,
}

impl ToolFilter {
    pub fn allows(&self, id: &ToolId) -> bool { /* disallowed 优先 */ }
}

/// 在通用基线上的**差异**：模型 extra 添加的 + 模型 excluded 排除的。
/// 序列化为 `{ "extra": [...], "excluded": [...] }`，用户可在 settings.json
/// 的 `providers.<name>.models.<id>.tool_overrides` 直接声明。
pub struct ToolOverrides {
    extra: HashSet<ToolId>,
    excluded: HashSet<ToolId>,
}

impl ToolOverrides {
    pub fn none() -> Self;                                  // 标识元
    pub fn with_extra(self, id: impl Into<ToolId>) -> Self;
    pub fn with_excluded(self, id: impl Into<ToolId>) -> Self;
    pub fn merge(self, other: &Self) -> Self;               // user override 叠加 builtin
    pub fn permits(&self, id: &ToolId) -> bool;             // !excluded
    pub fn is_extra(&self, id: &ToolId) -> bool;            // extra && !excluded
}
```

### 7.6 Subagent 上下文构造

subagent 只能在已启用的 Feature 集合内**继续筛**，不能放大父级 Feature 或 ToolOverrides。
父值通过 `AgentSpawnRequest.features` / `AgentSpawnRequest.tool_overrides`（in-process,
`#[serde(skip)]`）和 `AgentQueryConfig` 同名字段透传到子 engine：

```rust
fn build_child_context(
    parent: &ToolUseContext,
    config: &AgentQueryConfig,
    def: &AgentDefinition,
) -> ToolUseContext {
    ToolUseContext {
        // Layer 4：subagent 自身的 allow/deny
        tool_filter: ToolFilter::new(
            def.allowed_tools.clone(),
            def.disallowed_tools.clone(),
        ),
        // Layer 1 + 2：直接 clone 父级 Arc，subagent 永远不能放大
        features: config.features.clone()
            .unwrap_or_else(|| Arc::new(Features::with_defaults())),
        tool_overrides: config.tool_overrides.clone()
            .unwrap_or_else(|| Arc::new(ToolOverrides::none())),
        ..parent.clone()
    }
}
```

> 注意：`unwrap_or_else` 分支只在没有父 ctx 的测试或孤立场景命中；生产路径
> 总是由 `AgentTool::execute` 从 `ctx.features` / `ctx.tool_overrides`
> 填好后传入。

## 8. 端到端 trace：gpt-5 + Plan mode

```
所有注册的 tool: { Read, Edit, Write, Bash, apply_patch, web_search, web_fetch, Task, ... }

Layer 1 (Tool::is_enabled, Feature gate):
  features.enabled(WebSearch) = true → 保留 web_search
  features.enabled(WebFetch)  = true → 保留 web_fetch
  features.enabled(AgentTeams) = false → 移除 Task
  其他无 Feature gate → 全部保留
  → { Read, Edit, Write, Bash, apply_patch, web_search, web_fetch, ... }

Layer 2 (ToolOverrides = gpt-5):
  builtin diff: extra={apply_patch}, excluded={Edit}
  → { Read, Write, Bash, apply_patch, web_search, web_fetch, ... }

Layer 3 (Agent allow_tools = None / 继承父):
  → 不变

Layer 4 (MCP):
  → 不变

最终注入 LLM 的 schema: { Read, Write, Bash, apply_patch, web_search, web_fetch, ... }
```

> **Plan 模式不裁剪 schema（对齐 TS）。** 写工具仍对模型可见；运行 `Write` /
> `Bash` 等会在调用时落到 `coco_permissions` 的 Plan 分支：只读 / 写会话计划文件
> 放行，其余写 → ask，非交互 → deny。见 §9 与 `core/permissions/src/evaluate.rs`。

## 9. Execute 二次校验

execute 前必须**重过一次** filter，因为 model 可能召回**已被过滤掉**的工具名（mocked / cached / multi-turn race / schema 不一致）：

```rust
// coco-rs/core/tool-runtime/src/streaming_executor.rs

async fn execute_call(name: &str, args: Value, ctx: &ToolUseContext) -> ToolCallResult {
    let Some(tool) = self.registry.get(name) else {
        return ToolCallResult::error("unknown tool");
    };

    // 二次校验：必须仍然通过 schema-time filter
    if !self.registry.definitions_for(ctx).iter().any(|t| t.name() == name) {
        return ToolCallResult::error(SyntheticToolError::ToolNotEnabledInContext);
    }

    tool.execute(args, ctx).await
}
```

## 10. /experimental 菜单

```rust
pub fn experimental_menu_entries() -> Vec<&'static FeatureSpec> {
    all_features()
        .filter(|spec| matches!(spec.stage, Stage::Experimental { .. }))
        .collect()
}
```

> 当前 Feature 表中没有 `Stage::Experimental` 项；所有未稳定的均为 `UnderDevelopment`。如需公开发布到 `/experimental` 菜单，将相应 `FeatureSpec.stage` 升级为 `Stage::Experimental { name, menu_description, announcement }`。

## 11. 落地步骤

1. **新建** `coco-rs/common/types/src/features.rs`，导出 §4 的数据结构
2. **新增** `RuntimeConfig.features: Features` 字段
3. **删除字段**（直接删，无兼容）：
   - `MemoryConfig.enabled` / `SandboxConfig.enabled` / `RetrievalConfig.enabled`
   - `WorktreeConfig.enabled` / `SwarmConfig.enabled` / `LspConfig.enabled`（如有）
   - `WebSearchConfig.enabled` / `WebFetchConfig.enabled`（如有）
4. **删除散点 env**（`COCO_DISABLE_AUTO_MEMORY` / `COCO_SANDBOX_ENABLED` / …），统一 `COCO_FEATURE_*`
5. **重构 Tool trait**：`is_enabled(&self) -> bool` → `is_enabled(&self, ctx: &ToolUseContext) -> bool`
6. **`ToolUseContext`** 增加 `features` / `tool_overrides` / `tool_filter` 字段（`permission_mode` 通过 `permission_context` 已存在）
7. **`ToolRegistry::loaded_tools(ctx)` / `definitions(ctx)`** 实现 4 层 filter（Layer 5 通过 deregister 实现）
8. **`StreamingToolExecutor`** 在 execute 前加二次校验
9. **subagent spawn** 在 `AgentSpawnRequest` / `AgentQueryConfig` 透传父级 `features` / `tool_overrides`，`tool_filter` 由 subagent 自身的 `AgentDefinition.allowed_tools`/`disallowed_tools` 构造
10. **TUI 接入**：未来当某 Feature `Stage` 升级为 `Experimental` 时，`/experimental` 菜单自动呈现

## 12. 测试要点

| 测试 | 验证 |
|------|------|
| `Features::with_defaults` 与 `default_enabled` 表对齐 | 8 项默认值正确 |
| `apply_map` 处理未知 key | warn log 但不 panic |
| Layer 1 单层过滤 | Feature off → tool 不出现在 `definitions_for` 输出 |
| Layer 1+2 组合 | gpt-5 + WebSearch off → `apply_patch` 在但 `web_search` 不在 |
| Plan 不裁剪 schema | gpt-5 + Plan mode → 完整工具集（含写工具 / `apply_patch`）仍注入；只读约束改由调用时权限层验证 |
| Layer 1+2 不可放大父级 | subagent 的 `Features` / `ToolOverrides` 直接 clone 父级 Arc，无法启用父级未启用的 Feature |
| `ToolOverrides::merge` 语义 | builtin diff + user `tool_overrides`（settings.json）合并后，user `excluded` 赢过 builtin `extra` |
| Execute 二次校验 | 模拟 model 召回已过滤 tool → 返回 `ToolNotEnabledInContext` |
| MCP server offline | 仅 MCP tool 被过滤，本地 tool 不受影响 |

## 13. 兼容性与迁移

**不考虑向后兼容**。直接删除散点 bool 字段、散点 env 变量，要求用户配置升级到 `features` 段；旧 settings.json 启动时如带未知 key（例如旧版的 `memory.enabled`），由 `serde(deny_unknown_fields = false)` 静默忽略 + warn log 提示。

## 14. Open questions

1. `Sandbox` Feature 与 `SandboxConfig.mode` 的关系：是否需要把 `mode = "off"` 视为等价于 Feature off？目前规约：Feature 决定"沙箱子系统是否生效"，`mode` 决定"启用时多严"，两者正交保持。
2. `ToolOverrides` 如何从 model registry 读取：内置 registry 在 `coco-config/src/tool_overrides.rs::builtin_tool_overrides_for` 按 `model_id` pattern 匹配（今天只有 `gpt-5*`）；用户侧通过 `ModelInfo.tool_overrides` 在 settings.json 声明，由 `resolve_tool_overrides(model_id, info)` 在 builtin 之上 `merge`，user `excluded` 总是赢过 builtin `extra`。`ModelInfo` 接入 `RuntimeConfig` 通过 `ModelRegistry` plumbing 完成；`RuntimeConfig.tool_overrides: Arc<ToolOverrides>` 在 build 时为 Main role 一次性算好（multi-provider-plan.md §5.3）。
3. Per-role tool filter（如 `compact-model` 不该看到 `Bash`）：目前归 Layer 2 内部，后续若需要可独立成 Layer 2.5。
4. Layer 4 widening：当前 subagent 的 `tool_filter` 不与父级 `tool_filter` 求交集——agent definition 的 `allowed_tools` 可以包含父级排除掉的工具。Layer 1 + Layer 2 已经收口（subagent 永远不能放大），但 Layer 4 仍是松的，待独立 PR 修。
