# coco-rs Subagent Refactor Plan

本文档设计 `coco-rs` 的 subagent 重构方案。目标是把当前分散在
`AgentTool`、`AgentHandle`、swarm、skills、slash command、query adapter 中的
部分实现收敛为一个可维护的 subagent runtime。

参考输入：

| 来源 | 路径 | 用途 |
|------|------|------|
| 当前 docs | `docs/coco-rs/agent-loop-refactor-plan.md` | 已提出 `AgentRuntime`、`SkillRuntime`、`ToolCallRunner` 等大方向 |
| 当前 docs | `docs/coco-rs/crate-coco-tools.md` | 描述 `AgentTool` 应具备的动态 agent 列表、fork、background、tool filtering |
| 当前 docs | `docs/coco-rs/crate-coco-types.md` | 记录 `AgentDefinition`、`SubagentType`、`AgentIsolation` 等类型目标 |
| 当前 docs | `docs/coco-rs/crate-coco-commands.md` | 记录 slash command registry 和 `/agents` 命令方向 |
| TS reference | `tools/AgentTool` | AgentTool 行为规格 |
| 当前 Rust | `coco-rs/core/tools/src/tools/agent.rs` | 现有 `AgentTool` 外壳 |
| 当前 Rust | `coco-rs/core/tool/src/agent_handle.rs` | `AgentHandle` trait 边界 |
| 当前 Rust | `coco-rs/app/state/src/swarm_agent_handle.rs` | 当前 AgentHandle 实现和同步执行路径 |
| 当前 Rust | `coco-rs/app/query/src/agent_adapter.rs` | child QueryEngine adapter |
| 当前 Rust | `coco-rs/common/types/src/command.rs` | slash command `agent` 字段 |
| cocode-rs | `cocode-rs/core/subagent` | 可吸收的 manager、filter、background、typed DTO 设计 |

## Source Of Truth

本计划采用 TS-first 策略。

| 优先级 | 来源 | 决策 |
|--------|------|------|
| 1 | `tools/AgentTool` | 行为规格；字段、prompt 文案结构、可见性过滤、result shape、background 语义以这里为准 |
| 2 | 当前 `coco-rs` 架构 | crate 边界、`AgentHandle`、`AgentQueryEngine`、`QueryEngine` 主循环应尽量保留 |
| 3 | `cocode-rs/core/subagent` | 只作为优化实现参考，不能覆盖 TS 行为语义 |

当 TS 与 `cocode-rs/core/subagent` 不一致时，默认选择 TS 行为。例如 TS active agent
选择是按来源优先级覆盖同名 agent，不是默认 array union merge；`cocode-rs` 的 merge
能力可以作为未来 extension，但不能成为 parity 阶段默认行为。

## Executive Summary

当前 `coco-rs` 已有 subagent 的大部分外部形状：

| 能力 | 当前状态 | 主要缺口 |
|------|----------|----------|
| `AgentTool` schema | 已有 | 只把 JSON 转发给 `AgentHandle`，没有动态 agent prompt、定义解析、allowed agent 限制 |
| `AgentHandle` trait | 已有 | trait 足够作为工具层和 runtime 层的边界，但实现仍不完整 |
| 同步 subagent | 部分可跑 | child config 为空 system prompt、空 allowed tools、无 definition、无 skills/MCP/hooks |
| background subagent | ✅ **已实现 (2026-05-15)** | 真实 `tokio::spawn` 三处：主执行 (`coordinator/src/agent_handle/spawn.rs:1278`)、event drain (`:1121`)、summary timer (`:1157`)；与 task registry 集成，`mark_completed()` / `mark_failed()` 标记终态 |
| custom agent loader | 分散重复 | `agent_spawn.rs` 和 `agent_advanced.rs` 有重复解析逻辑，字段和大小写不完全一致 |
| built-in agent | 部分定义 | built-in 缺完整 system prompt、tool policy、permission policy，名称大小写也存在不一致风险 |
| tool filtering | ✅ **已实现 (2026-05-15)** | 三层过滤：request 时抽取 (`spawn.rs:800`)、config 时 `ToolFilter::new().narrow_with(parent)` (`app/query/src/agent_adapter.rs:201-209`)、discovery 时 `tool_filter.allows()` 门控 (`core/tool-runtime/src/registry.rs`) |
| slash command 指定 agent | 类型已存在 | `PromptCommandData.agent` 没有完整接入执行路径 |
| TUI/CLI 注入 AgentHandle | 不完整 | 普通 session 仍可能落到 `NoOpAgentHandle` |

重构后的目标架构：

```text
AgentTool / Slash Command / Skill Fork
  -> AgentHandle
  -> AgentRuntime
  -> AgentDefinitionStore
  -> SubagentManager
  -> AgentQueryEngineAdapter
  -> Child QueryEngine with filtered ToolRegistry
```

核心原则：

| 原则 | 决策 |
|------|------|
| Tool 层要薄 | `AgentTool` 负责 schema、输入校验和结果格式，不能拥有 runtime 语义 |
| Definition 只有一个来源 | built-in、plugin、userSettings、projectSettings、flagSettings、policySettings 都进入 `AgentDefinitionStore`；SDK 是显式 extension |
| Runtime 统一决策 | model、permission、tools、MCP、skills、background、worktree、fork 都由 `AgentRuntime` 决定 |
| Tool filtering 必须真实生效 | 过滤后的工具必须用于 child `ToolRegistry`，不是只写在 config 里 |
| Background 必须真实执行 | `AsyncLaunched` 必须对应一个已启动、可查询、可取消、可读 output 的任务 |
| Slash command 直接触发 runtime | `/build` 这类命令不应先变成自然语言再等模型调用 `AgentTool` |
| TS parity first | `cocode-rs` 只能优化实现形态，不能改变 TS 字段、prompt、result 和 lifecycle 语义 |

## Refactor Goals

### Functional Goals

| 编号 | 目标 | 说明 |
|------|------|------|
| G1 | 支持 built-in subagent | TS parity 内置 `general-purpose`、`statusline-setup`、feature-gated `explore`、`plan`、`verification`、非 SDK `claude-code-guide` |
| G2 | 支持 custom subagent | 从 user/project/plugin/flag/policy 加载 agent markdown/json 定义；SDK definitions 作为显式 extension |
| G3 | 支持 slash command 指定 agent | prompt command 可通过 `agent` 字段指定触发哪个 agent |
| G4 | 支持真实 background subagent | ✅ **DONE 2026-05-15** — `tokio::spawn` 三处真实执行，与 `AgentTaskRegistry` 集成，AgentSummary 30s 定时轮询；详见 `coordinator/src/agent_handle/spawn.rs:1034-1407` |
| G5 | 支持 child tool filtering | ✅ **DONE 2026-05-15** — 三层过滤生效：request → config → discovery，`ToolFilter::narrow_with(parent)` 保证子不能放宽父的 deny |
| G6 | 支持 model/permission inheritance | 复刻 TS parent-to-child permission 和 model 继承规则 |
| G7 | 支持 worktree isolation | foreground worktree 已有雏形，需纳入 runtime；background worktree 可分阶段支持 |
| G8 | 支持 fork context | `fork` agent 能继承父会话上下文，且防止递归 fork |
| G9 | 支持 MCP/skills/hooks 的 agent scope | agent 定义可声明所需 MCP、预加载 skills、agent lifecycle hooks |
| G10 | 保持 AgentHandle crate 边界 | `coco-tools` 不依赖 app/state/query，仍通过 trait 调用 runtime |

### Non-Goals

| 编号 | 非目标 | 理由 |
|------|--------|------|
| N1 | 第一阶段不实现 remote isolation | TS ant build 有 CCR remote，当前 `coco-rs` 没有同等 remote runtime，保持显式错误更安全 |
| N2 | 不重写整个 QueryEngine | 本计划只要求 child QueryEngine 构造可被 agent runtime 驱动 |
| N3 | 不把 swarm teammate 和 subagent 强行合并 | teammate 是长期会话/邮箱语义，standalone subagent 是任务语义，应共享 runtime 能力但保留入口差异 |
| N4 | 不复制 TS 目录结构 | 保留 Rust trait、typed config、crate boundary 的实现风格 |
| N5 | 不让模型隐式决定 slash command agent | slash command 已经是用户显式意图，应直接路由 |

## Current State Analysis

### Existing Docs

`docs/coco-rs/agent-loop-refactor-plan.md` 已经提出一个正确的大方向：

| 设计点 | 当前文档判断 | 本计划处理 |
|--------|--------------|------------|
| `AgentRuntime` | 应在 `AgentHandle` 后面支撑真实 subagent 执行 | 细化为 `AgentRuntime + AgentDefinitionStore + SubagentManager` |
| `AgentQueryEngine` | 已有 trait，可复用 | 保留 trait，扩展 config 和 adapter 行为 |
| `NoOpAgentHandle` | 普通 session 不应使用 | 在 bootstrap 阶段注入真实 handle |
| background | 需要真实 lifecycle | 吸收 `cocode-rs` manager 的 background 设计 |
| tools | 需要 child tool filtering | 明确要求构造过滤后的 `ToolRegistry` |

`docs/coco-rs/crate-coco-tools.md` 描述了理想 `AgentTool`：

| 能力 | 文档目标 | 现状 |
|------|----------|------|
| 动态列出 agent | `AgentTool::prompt` 根据 active agents 输出 | 现有 `AgentTool` 没有 override dynamic prompt |
| custom/plugin agent | loader 读取多个来源 | 当前 loader 分散且没有统一 store |
| fork/worktree/background | AgentTool 支持多路径 | fork 注释中承认未接线，background 未真实执行 |
| tool filtering | 支持 agent-specific tools | helper 存在但没有贯穿 query runtime |

`docs/coco-rs/crate-coco-commands.md` 和 `common/types/src/command.rs` 已有 slash command 的关键类型基础：

| 字段 | 含义 | 用法 |
|------|------|------|
| `PromptCommandData.allowed_tools` | command scope 可用工具 | 可传给 agent runtime 作为额外限制 |
| `PromptCommandData.model` | command model override | 可映射到 `AgentSpawnRequest.model` |
| `PromptCommandData.context` | `Inline` 或 `Fork` | `Fork` 可触发 subagent/fork context |
| `PromptCommandData.agent` | 指定 agent | 本计划要求接入 `AgentRuntime.spawn` |

### TypeScript AgentTool Behavior

TS `AgentTool` 是行为规格，不是结构模板。需要保留的行为：

| 行为 | TS 位置 | Rust 目标 |
|------|---------|-----------|
| 输入字段 | `AgentTool.tsx` input schema (`AgentTool.tsx:82-125`) | 保留 `prompt`、`description`、`subagent_type`、`model`、`run_in_background`、`name`、`team_name`、`mode`、`cwd`、`isolation`。注意条件可见性：`cwd` 仅在 `feature('KAIROS')` 时暴露给模型；`run_in_background` 在后台任务关闭或 fork enabled 时被 `.omit()` |
| 动态 prompt | `prompt({ agents, tools, allowedAgentTypes })` | `AgentTool::prompt` 使用 `AgentDefinitionStore` 快照 |
| agent definition 过滤 | required MCP、permission denied、allowed types | runtime 和 prompt catalog 都应用同一过滤策略 |
| teammate path | `teamName && name` | 继续由 swarm teammate 处理 |
| fork path | omitted `subagent_type` + fork enabled | 加入 `ForkContext` 到 request/config |
| model resolution | `getAgentModel` | 由 runtime 根据 definition、request、parent model 决定 |
| isolation | explicit input overrides definition | 由 runtime 统一决策，remote 显式 unsupported |
| sync execution | `runAgent` async iterator | `AgentQueryEngineAdapter` 驱动 child QueryEngine |
| async execution | `runAsyncAgentLifecycle` | `SubagentManager` 启动 tokio task，写 output |
| result shape | completed/async/remote/teammate variants | `AgentSpawnResponse` 和 AgentTool JSON 输出对齐 |
| permission | AgentTool 本身 read-only delegate | tool permission pipeline 保持外层语义，child permission 由 runtime 设置 |

### TS Agent Definition Contract

TS `AgentDefinition` 的字段需要逐项映射，不能只保留 name/prompt/tools。

| TS 字段 | 来源 | Rust 字段建议 | 语义 |
|---------|------|---------------|------|
| `agentType` | markdown `name` 或 JSON key | `agent_type` | canonical spawn id |
| `whenToUse` | markdown `description` 或 JSON `description` | `description` 或 `when_to_use` | AgentTool prompt 中展示的用途描述 |
| `tools` | frontmatter/JSON | `allowed_tools` | allow-list；`undefined` 表示使用默认（系统过滤后的全部工具）。**`["*"]`** 在 TS `parseAgentToolsFromFrontmatter` 中并未被特殊处理为通配符（只是普通字符串），coco-rs 不应自行赋予 `*` wildcard 语义；如果未来需要 wildcard，应作为显式 extension 并文档化 |
| `disallowedTools` | frontmatter/JSON | `disallowed_tools` | deny-list；在 allow-list 后继续过滤 |
| `skills` | frontmatter/JSON | `skills` | child agent 启动前预加载的 skills |
| `mcpServers` | frontmatter/JSON | `mcp_servers` | agent-scoped MCP server refs，支持 name ref 和 inline config |
| `hooks` | frontmatter/JSON | `hooks` | agent lifecycle 内注册，结束后卸载 |
| `color` | frontmatter only | `color` | TUI grouped display、agent color map、analytics metadata |
| `model` | frontmatter/JSON | `model` | request model override > definition model > parent model；`inherit` 表示 parent model |
| `effort` | frontmatter/JSON | `effort` | string effort level 或 integer |
| `permissionMode` | frontmatter/JSON | `permission_mode` | child default permission mode，仍受 parent trust mode 继承规则影响 |
| `maxTurns` | frontmatter/JSON | `max_turns` | child agent turn limit |
| `filename` | markdown path | `filename` | diagnostics and `/agents show` provenance |
| `baseDir` | loader | `base_dir` | source path/provenance |
| `criticalSystemReminder_EXPERIMENTAL` | built-in | `critical_system_reminder` | 每个 user turn reinject 的短提醒 |
| `requiredMcpServers` | built-in/custom metadata | `required_mcp_servers` | AgentTool prompt/call 可见性和 readiness check |
| `background` | frontmatter/JSON | `background` | definition 默认后台运行；request 可显式覆盖 |
| `initialPrompt` | frontmatter/JSON | `initial_prompt` | prepended to first user turn，slash commands 可工作 |
| `memory` | frontmatter/JSON | `memory_scope` | `user`、`project`、`local`；memory enabled 时还会补 Read/Edit/Write |
| `isolation` | frontmatter/JSON | `isolation` | `worktree` 或 ant-only `remote` |
| `pendingSnapshotUpdate` | managed agents | `pending_snapshot_update` | managed/policy agent metadata，可先保留为 optional |
| `omitClaudeMd` | built-in/custom | `omit_claude_md` | read-only agents 省略 CLAUDE.md hierarchy，节省 token |
| `source` | loader | `source` | `built-in`、`plugin`、`userSettings`、`projectSettings`、`flagSettings`、`policySettings` |
| `getSystemPrompt` | built-in/custom/plugin closure | stored prompt renderer | built-in 是动态 prompt，custom/plugin 是 markdown body closure |

字段命名注意：

| 注意点 | 决策 |
|--------|------|
| TS `description` 在 agent definition 中进入 `whenToUse` | Rust 可以保留 `description` 字段，但 AgentTool prompt 必须按 `whenToUse` 语义使用 |
| AgentTool input `description` 是 task description | 不要和 agent definition 的 `description/whenToUse` 混用 |
| markdown body 是 system prompt | 不等同于 `initialPrompt`；`initialPrompt` 是第一轮 user prompt 前缀 |
| built-in agents 没有静态 `systemPrompt` 字段 | Rust built-in 应支持动态 prompt renderer，至少能接收 `ToolUseContext.options` |

### TS AgentTool Prompt Contract

TS `prompt.ts` 的 prompt 结构是行为的一部分。

| Prompt 细节 | Rust 目标 |
|-------------|-----------|
| 先按 MCP requirement 过滤 agents | AgentTool prompt 和 spawn call 使用同一 MCP readiness 逻辑 |
| 再按 permission deny 过滤 agents | `Agent(foo)` deny rule 应隐藏并禁止该 agent |
| 再按 `allowedAgentTypes` 过滤 | `Agent(type1,type2)` 限制要影响 prompt 和 runtime |
| agent list 行格式 | `- {agentType}: {whenToUse} (Tools: {toolsDescription})` |
| tools description | allow+deny 同时存在时展示 allow-list minus deny-list |
| no restrictions | 展示 `All tools` |
| deny only | 展示 `All tools except ...` |
| allow only | 展示具体工具列表 |
| optional attachment listing | 支持将 agent list 放到 system-reminder/attachment，减少 tool schema cache bust |
| coordinator mode | 使用 slim prompt，不包含完整 usage examples |
| non-coordinator mode | 包含 when-not-to-use、usage notes、background、worktree、parallel agents、fork prompt guidance |
| fork enabled | 说明 omit `subagent_type` 表示 fork，且 fork prompt 是 directive，不要重复背景 |
| in-process teammate | prompt 中隐藏或禁用 `run_in_background`、`name`、`team_name`、`mode` |

`coco-rs` 可以分两步落地：

| 阶段 | 行为 |
|------|------|
| parity minimum | inline agent list，保持 TS 行格式和 usage notes |
| cache optimization | 后续实现 agent listing attachment，避免动态 tool description 频繁变更 |

### TS Color Contract

`color` 不是纯装饰字段。TS 在多个路径使用它：

| 使用点 | TS 位置 | 语义 |
|--------|---------|------|
| loading active agents | `loadAgentsDir.ts:370` | 初始化 active agent 的 color map |
| teammate spawn | `AgentTool.tsx:288` | spawn 前按 selected agent color 设置 grouped UI color |
| normal subagent spawn | `AgentTool.tsx:414` | selected agent 有 color 时调用 color manager |
| telemetry | `AgentTool.tsx:423` (`tengu_agent_tool_selected`) | 直接读取 `selectedAgent.color`（不走 `getAgentColor`），attributes 至少含 agentType/color/source/model |
| grouped progress UI | `UI.tsx:696, 786` | 同一类 agent 的进度行使用稳定颜色 |
| agent settings UI | `components/agents/AgentDetail.tsx:40`, `AgentEditor.tsx:71` | `/agents show` 等入口的 color 显示 |
| swarm banner | `components/PromptInput/useSwarmBanner.ts:120` | 团队 banner 使用 agent color |
| unified suggestions | `hooks/unifiedSuggestions.ts:92` | 自动补全列表上色 |

Rust 落地要求：

| 要求 | 说明 |
|------|------|
| `AgentDefinition.color` 必须保留 | 不能只在 TUI 临时 hash |
| color values 要 validate | 只接受内置 color set，非法值忽略并记录 validate warning |
| runtime response/status 带 color | TUI、SDK、tasks 可以统一显示 |
| color assignment 要稳定 | 同一 agent type 在同一 session 内颜色一致 |

### TS AgentTool Result Contract

`mapToolResultToToolResultBlockParam` 的文本也影响模型行为。

| Status | 必须保留的语义 |
|--------|----------------|
| `completed` | 返回 content；非 one-shot built-in 附带 `agentId`、SendMessage 续接提示、usage trailer、worktree path/branch |
| `async_launched` | 明确“不要重复该 agent 的工作”；如果 parent 有 Read/Bash，给 `output_file` 和可读进度说明 |
| `remote_launched` | 当前 coco-rs 不支持时显式错误；未来支持时必须返回 task/session/output file |
| `teammate_spawned` | 返回 teammate id、name、team name、mailbox instruction |
| empty completed content | 插入显式 marker。**TS parity 必须使用精确字符串**：`(Subagent completed but returned no output.)` (`AgentTool.tsx:1347-1350`)，避免模型误判没有结果 |
| one-shot built-ins | TS one-shot 集合是 **case-sensitive** `{'Explore', 'Plan'}` (`constants.ts:9-12`)，跳过 SendMessage continuation trailer。Rust 实现必须维持完全相同的大小写匹配；不可改成 `{'explore','plan'}` |

TS `loadAgentsDir.ts` 需要吸收的定义加载语义：

| 设计 | Rust 落地 |
|------|-----------|
| markdown frontmatter + body prompt | frontmatter `name` -> `agentType`；frontmatter `description` -> `whenToUse`；body -> `getSystemPrompt()` |
| JSON agent | JSON key -> `agentType`；JSON `prompt` -> `getSystemPrompt()` |
| `name` 必填 | markdown 缺失 name 时跳过或进入 failed definitions |
| `description` 必填 | 缺失 description 的 agent 不进入 active set |
| source precedence | built-in < plugin < userSettings < projectSettings < flagSettings < policySettings |
| active/all/failed | store 对外提供三类结果，便于 `/agents validate` |
| `tools`/`disallowedTools` | 进入统一 tool filtering |
| `model: inherit` | 映射到 parent model |
| `effort` | 支持 string effort levels 和 integer |
| `color` | 只接受合法 `AgentColorName` |
| `initialPrompt` | 保留为 first-turn user prompt prefix，不覆盖 system prompt |
| `memory` | 支持 `user`、`project`、`local`，memory 开启时自动补 memory file tools |
| `background` | 成为默认 background 行为，request 可覆盖 |
| `isolation` | 成为默认 isolation，request 可覆盖 |

TS `runAgent.ts` 需要吸收的运行语义：

| 设计 | Rust 落地 |
|------|-----------|
| agent-specific MCP servers | runtime 启动 child 前确保 MCP ready，完成后清理 agent-scoped MCP (`runAgent.ts:95-200, 818`) |
| permission inheritance | TS 默认仅 parent `bypassPermissions` 和 `acceptEdits` 覆盖 child；`auto` 仅在 `feature('TRANSCRIPT_CLASSIFIER')` 开启时才被保留覆盖 (`runAgent.ts:421-434`)。Rust 落地需以 feature gate 表达，不可无条件继承 `auto` |
| filtered tools | child tool list 来自 definition + background + permission |
| system prompt 构造 | definition body、skills、memory、tool list、env 信息组合 |
| subagent hooks | agent lifecycle 注册和卸载 hook |
| transcript | foreground/background 都应记录可审计输出 |
| cancellation | sync child 跟随 parent cancel，background child 独立 cancel |

### Current coco-rs Implementation

`coco-rs/core/tools/src/tools/agent.rs` 当前优点：

| 优点 | 说明 |
|------|------|
| schema 已覆盖主要字段 | 与 TS AgentTool 大体一致 |
| `AgentTool` concurrency safe | 允许同一 turn 启动多个 agent |
| 通过 `ToolUseContext.agent` 调用 | crate dependency 边界正确 |
| remote isolation 显式错误 | 当前 build 不支持 remote 时不静默降级 |
| permission mode inheritance 有初步逻辑 | 使用 `resolve_subagent_mode` |

`coco-rs/core/tools/src/tools/agent.rs` 当前缺口：

| 缺口 | 影响 |
|------|------|
| 没有 `AgentTool::prompt` 动态 agent 列表 | 模型不知道有哪些 custom/built-in agent 及其用途 |
| 不解析 agent definition | `subagent_type` 只是字符串，runtime 无法拿到 prompt/tools/model policy |
| 不处理 allowed agent types | `Agent(type1,type2)` 这类限制无法生效；可兼容解析 `Task(type)` |
| fork 注释承认未接线 | 无法实现 TS fork subagent 行为 |
| result 缺少部分提示语义 | background result 没有完整 output file/read guidance |

`coco-rs/core/tool/src/agent_handle.rs` 当前优点：

| 优点 | 说明 |
|------|------|
| trait 边界正确 | 工具层不依赖 app/state/query |
| request/response typed | 适合作为 runtime API |
| 已有 status/query/output/cancel 方法 | background lifecycle 可以在现有 trait 上补全 |

`coco-rs/core/tool/src/agent_handle.rs` 当前缺口：

| 缺口 | 影响 |
|------|------|
| request 缺少 fork context | AgentTool 无法把 parent messages 传给 runtime |
| request 的 `run_in_background` 是 bool | 无法区分“未指定”和“显式 false”，不利于 definition default |
| response 不含 definition/source/color | TUI/status 展示能力不足 |
| response 不含 output delta metadata | background output 增量读取不够清晰 |

`coco-rs/app/state/src/swarm_agent_handle.rs` 当前优点：

| 优点 | 说明 |
|------|------|
| 已能区分 teammate 和 standalone subagent | 保留 teammate path |
| sync path 可调用 `AgentQueryEngine` | runtime 可以复用这条执行通道 |
| worktree foreground 有初步实现 | 可纳入统一 isolation 策略 |
| 状态 tracking 已有 `SubAgentState` | 可迁移或由 manager 接管 |

`coco-rs/app/state/src/swarm_agent_handle.rs` 当前缺口：

| 缺口 | 影响 |
|------|------|
| ~~background path 不执行 child query~~ | ✅ **CLOSED 2026-05-15** — 真实 `tokio::spawn` 在 `coordinator/src/agent_handle/spawn.rs:1278`；event drain + summary timer 配套 |
| ~~`AgentQueryConfig.allowed_tools` 为空~~ | ✅ **CLOSED 2026-05-15** — `app/query/src/agent_adapter.rs:201-209` 构造 `ToolFilter` 并 `.narrow_with(parent)` |
| `AgentQueryConfig.system_prompt` 为空 | child 没有 agent-specific prompt |
| `max_turns` 为空 | definition/request turn limit 不生效 |
| `session_id` 为空 | transcript/session lineage 不完整 |
| `bypass_permissions_available` 固定 false | parent permission inheritance 不完整 |
| fork messages 固定空 | fork path 不可用 |

`coco-rs/app/query/src/agent_adapter.rs` 当前优点：

| 优点 | 说明 |
|------|------|
| 已实现 `AgentQueryEngine` | child query 不需要重新发明入口 |
| 能 `run_with_messages` | fork context 可基于此扩展 |
| 统计 tool result 数 | 可作为初步 tool_use_count |

`coco-rs/app/query/src/agent_adapter.rs` 当前缺口：

| 缺口 | 影响 |
|------|------|
| `allowed_tools` 没有用于构造 ToolRegistry | 工具安全边界无效 |
| child config 大量使用默认值 | 与 parent/session/model/runtime 配置脱节 |
| 没有注入 agent runtime handle | nested agent 控制和 allowed subagent types 不完整 |
| 没有 skills/MCP/hooks 参数 | custom agent 声明无法生效 |

### cocode-rs Subagent Design To Absorb

`cocode-rs/core/subagent` 的优秀设计点：

**重要**：本计划绝不是 port `cocode-rs/core/subagent` crate。该 crate 共 5,353 LoC，仅
`definition.rs` (302)、`loader.rs` (375)、`filter.rs` (196) 和 `definitions/*` 的部分内容
属于 `coco-subagent` 候选范围。`manager.rs` (1,289)、`transcript.rs`、`signal.rs`、
`background.rs` 必须放在 `app/state`，因为它们依赖 `tokio::spawn`、`tokio::fs`、
`mpsc::Sender<CoreEvent>` 以及一个进程级 `Lazy<RwLock<HashMap<…>>>`（`signal.rs:31-32`），
全部违反 `coco-subagent` 的 pure-logic 约束。

| 设计 | 吸收方式 | 目标 crate |
|------|----------|------------|
| `AgentSource::priority(self) -> u8` | 移植方法本身（cocode-rs `definition.rs:113-137`，BuiltIn=0…CliFlag=5）；默认排序必须匹配 TS source precedence | `coco-subagent` |
| `AgentDefinition::merge_with` | 只能作为 opt-in extension 参考；TS parity 默认是同名 agent 按来源覆盖，不做 array union (`loadAgentsDir.ts:212` Map.set) | 不默认启用 |
| typed `SpawnInput` (cocode-rs `spawn.rs:7-69`) | 替代跨模块传递过多 positional/loosely typed 参数 | `coco-state` |
| `SubagentManager` owns instances + 5-status enum | 管理 running/completed/failed/backgrounded/killed (cocode-rs `manager.rs:26-32`) | `coco-state`（不进 `coco-subagent`） |
| `AgentExecuteParams` (cocode-rs `manager.rs:123-187`) | child query 的单一参数对象 | `coco-state::AgentRuntime` |
| background output file | background agent 有明确 output path | `coco-state::SubagentManager` |
| 每实例 cancellation token | `AgentInstance.cancel_token` 字段 (cocode-rs `manager.rs:101`)。**注意**：cocode-rs 没有 manager 拥有的 token map；其 `signal.rs:31-32` 的进程级全局 `Lazy<RwLock<HashMap>>` 是反例，不要复制 | `coco-state::SubagentManager` |
| `BackgroundOrigin` (cocode-rs `manager.rs:46-53`) | 区分显式后台、signal 后台、timeout 后台 | `coco-state` |
| `read_deltas` (cocode-rs `manager.rs:1215-1259`) | 支持增量读取 output，避免重复通知 | `coco-state::SubagentManager` |
| `mark_notified` (cocode-rs `manager.rs:1262-1269`) | 避免父 agent 重复收到完成通知 | `coco-state::SubagentManager` |
| GC terminal instances (cocode-rs `manager.rs:1129-1156`) | 控制长期 session 内存 | `coco-state::SubagentManager` |
| four-layer tool filtering (cocode-rs `filter.rs:53-153`) | system blocked、allow-list、deny-list、background safe | `coco-subagent::filter` |
| 嵌套 agent 限制：`Agent(...)` 与 `Task(...)` 双形别名 | TS `LEGACY_AGENT_TOOL_NAME = 'Task'` 永久作为 alias (`constants.ts:3`、`AgentTool.tsx:228`)，permission spec parser 同一 regex `^([^(]+)(?:\(([^)]*)\))?$` 同时匹配 (`permissionSetup.ts:324-325`)。Rust 必须**双形并行**接受，不可单向 canonicalize 或重写用户规则 | `coco-subagent::filter` |

不建议直接照搬的点：

| 设计 | 不直接照搬原因 | coco-rs 方案 |
|------|----------------|-------------|
| `cocode_protocol` 类型 | `coco-rs` 已有 `coco_types` | 映射到 `coco_types::AgentDefinition` |
| 独立 execute callback 全部语义 | `coco-rs` 已有 `AgentQueryEngine` trait | callback 内部调用 `AgentQueryEngine` |
| `.cocode` 路径硬编码 (cocode-rs `loader.rs:325`) | `coco-rs` 使用 `~/.coco` 和 project conventions | 使用 `.coco`，兼容 `.claude` |
| 工具名 `Task`（cocode-rs `filter.rs:13-17` 仅识别 `Task`）| `coco-rs` 工具名是 `Agent` | 双形并行：tool 主名 `Agent`，permission rule 同时支持 `Agent(...)` 与 `Task(...)`，与 TS 一致 |
| 进程级全局 signal map (cocode-rs `signal.rs:31-32`) | 是 anti-pattern，难测试且跨 session 共享 | 用 per-session/per-runtime owned map |
| cocode-rs 内置 `bash`、`code-simplifier`，且使用 `general` / `statusline` 简称 | 偏离 TS 命名 (`general-purpose` / `statusline-setup`)，且 TS 没有 `verification` 之外的扩展 | 不要复制 cocode-rs 的 `definitions/*` 目录；TS-parity 内置必须从 `tools/AgentTool/built-in/*.ts` 重新派生 |

## Design Decisions

### D1. AgentTool Remains Thin

`AgentTool` 不应该直接加载 markdown、不应该创建 QueryEngine、不应该管理 background task。

原因：

| 原因 | 说明 |
|------|------|
| crate 分层 | `coco-tools` 不能依赖 app/query/state |
| 可测试性 | runtime 可以用 fake `AgentQueryEngine` 测试 |
| 行为一致性 | slash command、Skill fork、AgentTool 都应走同一 runtime |
| 安全性 | tool filtering 和 permission inheritance 必须集中处理 |

`AgentTool` 应保留的职责：

| 职责 | 说明 |
|------|------|
| input schema | 对外暴露模型可调用字段 |
| basic validation | `prompt` 非空、字段类型合法 |
| permission pre-normalization | parent-to-child mode 可保留，但最终由 runtime 再校验 |
| call `ctx.agent.spawn_agent` | 不跨 crate 直接调用 app runtime |
| format tool result | 把 `AgentSpawnResponse` 转成模型可读 JSON/text |

### D2. Runtime Owns Definition Resolution

`subagent_type` 不能只是字符串。runtime 必须把它解析成 definition。

解析顺序：

| 输入 | 结果 |
|------|------|
| `subagent_type = Some(name)` | 查找 active definition `name` |
| `subagent_type = None` 且 fork enabled | 使用 fork subagent path |
| `subagent_type = None` 且无 fork | 默认 `general-purpose` |
| slash command `agent = Some(name)` | 直接作为 `subagent_type` |
| skill `context = Fork` 且 `agent = Some(name)` | 使用该 agent |

找不到 definition 的行为：

| 场景 | 行为 |
|------|------|
| 模型调用 `Agent` 指定不存在 agent | 返回 model-visible failure |
| slash command 指定不存在 agent | 用户可见错误，不进入模型 |
| custom definition parse failed | `/agents validate` 显示 failed file 和错误 |

### D3. Definition Store Is The Single Source Of Truth

新增 `AgentDefinitionStore`。所有 TS-compatible built-in/custom/plugin/flag/policy
agent 都进入它。SDK session agents 可以作为 `coco-rs` extension；CLI supplied agents
应映射到 TS-compatible `flagSettings`，二者都不应改变 TS parity 的
默认来源顺序。

TS parity 来源优先级：

| Priority | Source | 说明 |
|----------|--------|------|
| 0 | `built-in` | bundled agent |
| 1 | `plugin` | plugin contribution |
| 2 | `userSettings` | user-level agents |
| 3 | `projectSettings` | project-level agents |
| 4 | `flagSettings` | JSON/CLI flag supplied agents |
| 5 | `policySettings` | managed/policy agents |

Active selection rule:

| 规则 | 说明 |
|------|------|
| 同名 agent 按来源顺序覆盖 | later source wins，接近 TS `getActiveAgentsFromList` 的 Map overwrite 语义 |
| 不默认 union merge arrays | `cocode-rs` 的 array union merge 是可选优化，不是 TS parity |
| `allAgents` 保留全部定义 | 用于 `/agents show` 展示 source chain 和诊断 |
| `activeAgents` 只暴露最终可用定义 | AgentTool prompt 和 runtime 只使用 active definitions |
| `failedFiles` 单独返回 | parse/load 失败不污染 active set |

可选 extension：

| Extension | 约束 |
|-----------|------|
| SDK session agents | 可以映射为 `sdkSettings`，但默认优先级必须显式记录 |
| CLI ad-hoc agents | 可以映射为 `flagSettings`，与 TS flag semantics 对齐 |
| `cocode-rs` style merge | 只能作为 opt-in overlay 模式；默认不启用，避免和 TS 覆盖语义冲突 |

### D4. Child Tool Filtering Must Build A Real ToolRegistry

仅把 `allowed_tools` 放进 `AgentQueryConfig` 不够。child `QueryEngine` 构造 tool catalog 时必须只能看到过滤后的 tools。

过滤层：

| Layer | 规则 |
|-------|------|
| 1 | system blocked tools 默认移除 |
| 2 | MCP tools 可按策略 bypass 普通 allow/deny |
| 3 | definition allow-list 非空时只保留 allow-list |
| 4 | definition deny-list 移除指定 tools |
| 5 | background agent 只保留 async-safe tools |
| 6 | slash command `allowed_tools` 可作为额外 intersection |
| 7 | `Agent(type)` 限制 nested subagent types |

默认 system blocked tools 建议：

| Tool | 原因 |
|------|------|
| `Agent` | 防止无限递归，除非显式 `Agent(type)` |
| `AskUserQuestion` | background/sync child 不应随意打断用户 |
| `TaskStop` | 防止 child 随意停止 sibling/parent |
| `EnterPlanMode` | child plan mode 应由 runtime 设置，不由模型切换 |
| `ExitPlanMode` | plan mode 时可例外允许 |

### D5. Background Means Real Execution

> **Status (2026-05-15)**: implemented. `AgentSpawnStatus::AsyncLaunched`
> backs a real `tokio::spawn` in `coordinator/src/agent_handle/spawn.rs:1278`
> with the registry entry, event-drain task, and summary timer all live.
> Section retained below as the design rationale.

`AgentSpawnStatus::AsyncLaunched` 必须代表一个已经启动的任务。

background agent 需要的最小状态：

| 字段 | 说明 |
|------|------|
| `agent_id` | 稳定 ID |
| `agent_type` | definition name |
| `status` | running/completed/failed/killed |
| `output_file` | 模型和用户可查询输出 |
| `cancel_token` | 支持 stop |
| `started_at` | status 展示和 timeout |
| `completed_at` | GC |
| `last_read_offset` | delta reads |
| `parent_notified` | 避免重复通知 |
| `background_origin` | explicit/signal/timeout |

最小行为：

| 行为 | 说明 |
|------|------|
| spawn | 创建 instance、output file、tokio task |
| write | child 输出或最终摘要写入 output file |
| status | `query_agent_status` 返回运行或终态 |
| output | `get_agent_output` 读取完整或增量输出 |
| cancel | `background_agent`/stop 通过 token 控制 |
| cleanup | terminal 后按 TTL GC |

### D6. Slash Command Agent Is A Direct Runtime Invocation

用户输入 `/build` 且 command 定义了 `agent: build` 时，不应先让主模型理解“请调用 AgentTool”。

直接路由的优点：

| 优点 | 说明 |
|------|------|
| 确定性 | 用户指定 agent 就启动该 agent |
| 节省 token | 不需要多一轮主模型决策 |
| 权限清晰 | command 的 `allowed_tools/model/context` 可以直接进入 spawn config |
| 便于测试 | slash command fake runtime 即可断言 |

执行策略：

| Command 类型 | 行为 |
|--------------|------|
| local command | 保持本地执行 |
| prompt command + no agent + Inline | 展开 prompt 注入主会话 |
| prompt command + no agent + Fork | 使用默认 fork/general-purpose agent |
| prompt command + agent + Inline | 建议仍走 agent，因为 `agent` 是强信号 |
| prompt command + agent + Fork | 启动指定 agent，并带 fork context |

### D7. Keep Teammate Separate But Reuse Runtime Pieces

teammate 与 standalone subagent 的差异：

| 维度 | Standalone subagent | Teammate |
|------|---------------------|----------|
| 生命周期 | 任务级，完成即终止 | 会话级，持续存在 |
| 通信 | 通过 AgentTool result/output | 通过 mailbox/team |
| 状态 | subagent manager | swarm team manager |
| tool policy | definition filter | teammate config + definition filter |
| execution | child QueryEngine | in-process teammate loop |

建议：

| 决策 | 说明 |
|------|------|
| `SwarmAgentHandle` 保留 teammate path | 避免破坏现有 team 功能 |
| standalone path 委托 `AgentRuntime` | 不再在 `SwarmAgentHandle` 拼 child config |
| teammate 也可使用 `AgentDefinitionStore` | 解析 model/tools/system prompt，但执行仍走 swarm teammate runner |

### D8. Relationship: coco-subagent, AgentTool, AgentTeam

`coco-subagent` 不是新的 AgentTool，也不是 AgentTeam runtime。它是
AgentTool subagent 和 AgentTeam teammate 都可以复用的 definition/catalog/prompt/filter
规则库。

#### Type Ownership Contract

`AgentDefinition` 类型本身应该定义在 `coco-rs/common/types/src/agent.rs`
并通过 `coco_types::AgentDefinition` re-export。当前代码已经是这个方向，
应继续保留。

原因：

| Reason | Explanation |
|--------|-------------|
| cross-crate DTO | AgentTool、AgentTeam、slash command、SDK bootstrap、state runtime 都需要读同一种 definition |
| dependency direction | `coco-types` 是底层 foundation crate；`coco-subagent`、`coco-tools`、`coco-state` 都可以依赖它 |
| avoid duplicate schemas | 如果 `coco-subagent` 自己定义 `AgentDefinition`，AgentTeam 复用时会出现转换层和字段漂移 |
| wire compatibility | SDK/NDJSON/command output 如果暴露 agent info，应从同一个 DTO 投影 |

边界：

| Layer | Should contain | Should not contain |
|-------|----------------|--------------------|
| `coco-types` | serializable DTO/enum: `AgentDefinition`, `AgentTypeId`, `SubagentType`, `AgentIsolation`, `MemoryScope`, `AgentColorName`, `AgentSource` if consumers need source metadata | filesystem loading, source precedence algorithm, validation logic, prompt rendering, tool filtering |
| `coco-subagent` | `AgentDefinitionStore`, `AgentCatalogSnapshot`, `LoadedAgentDefinition`/record metadata, validation diagnostics, TS-first source precedence, prompt/filter helpers | a second `AgentDefinition` schema |
| AgentTeam/swarm runtime | consumes `coco_types::AgentDefinition` or `coco-subagent` snapshots for selected teammate metadata | private teammate-only agent definition parser |

Precise wording: AgentTeam should not “reuse `coco-subagent`'s `AgentDefinition`”
because `coco-subagent` should not own that type. AgentTeam should reuse
`coco_types::AgentDefinition`; it may obtain those definitions from
`coco_subagent::AgentDefinitionStore`.

#### Ownership Contract

| Component | Owns | Does not own |
|-----------|------|--------------|
| `coco-subagent` | `AgentDefinition` loading, source precedence, built-in catalog, custom agent validation, AgentTool prompt rendering, tool filter planning, allowed agent type parsing | QueryEngine execution, tokio task lifecycle, background output files, worktree cleanup, mailbox/team roster, TUI state |
| `coco-tools::AgentTool` | input schema, shallow validation, routing by input shape, model-visible result formatting | markdown loading, source precedence, child QueryEngine construction, background task management, team lifecycle |
| `coco-state::AgentRuntime` | resolving a subagent request into an executable `AgentSpawnPlan`, permission/model/isolation/tool policy, foreground/background subagent lifecycle delegation | parsing custom agent files, rendering AgentTool catalog prompt, TeamCreate/TeamDelete semantics |
| `coco-state` swarm/team modules | team files, teammate identity, mailbox, roster, teammate spawn/kill/view lifecycle | AgentTool subagent catalog semantics; duplicate custom agent parser |
| `coco-query::AgentQueryEngineAdapter` | constructing and running filtered child QueryEngine | definition source precedence, teammate roster, AgentTool schema |

#### AgentTool Routing Contract

AgentTool should route by input shape before choosing runtime:

| Input shape | TS behavior | coco-rs target |
|-------------|-------------|----------------|
| `team_name/name` present and swarm enabled | spawn teammate, return `teammate_spawned` | route to swarm teammate spawn path; may resolve `subagent_type` through `coco-subagent` for selected definition metadata |
| `team_name/name` present but current caller is teammate | reject nested teammate spawn | keep TS error; tell caller to omit `name/team_name` to spawn a subagent |
| no teammate spawn and `subagent_type` present | run selected AgentTool subagent | resolve active definition from `coco-subagent`, then call `AgentRuntime` |
| no teammate spawn and `subagent_type` omitted with fork enabled | fork child context | construct fork request and call `AgentRuntime` |
| no teammate spawn and `subagent_type` omitted with fork disabled | default `general-purpose` | resolve default active definition and call `AgentRuntime` |

`coco-tools::AgentTool` may depend on `coco-subagent` only for pure helpers such as
prompt rendering, canonicalizing allowed agent types, and formatting catalog diagnostics.
It must receive snapshots or handles from bootstrap/runtime; it must not scan `~/.coco/agents`
or project agent directories itself.

#### AgentTeam / Teammate Contract

AgentTeam uses the word `agent`, but the runtime entity is a teammate, not an
AgentTool subagent.

| Property | AgentTool subagent | AgentTeam teammate |
|----------|--------------------|--------------------|
| Identity | generated subagent id | stable `agentName@teamName` identity |
| Lifetime | task-scoped; terminal after result | session/team-scoped; can become idle and receive more work |
| Communication | tool result, background output, optional SendMessage continuation | mailbox, team file, teammate messages, viewable transcript |
| State owner | `SubagentManager` | swarm/team manager and task state |
| Spawn trigger | `AgentTool` without teammate shape, slash command `agent`, skill fork | `AgentTool` with `team_name + name`, `TeamCreate`, team tooling |
| Definition usage | required; default `general-purpose` when omitted | optional; many teammates run as general-purpose without a custom definition |

Allowed sharing between AgentTeam and `coco-subagent`:

| Shared item | Rule |
|-------------|------|
| `AgentDefinition` lookup | allowed; teammate `agent_type` can select custom prompt/model/tools/color |
| tool filter calculation | allowed; final teammate tools must still apply teammate-specific restrictions |
| model/effort defaults | allowed; team spawn model override still wins where TS does |
| color metadata | allowed; used for grouped UI and teammate display |
| validation diagnostics | allowed; `/agents validate` should cover definitions usable by both subagents and teammates |

Forbidden coupling:

| Coupling | Reason |
|----------|--------|
| `coco-subagent` importing swarm modules | would make definition catalog depend on team runtime |
| `SubagentManager` owning teammates | teammates are long-lived and mailbox-addressable; they are not terminal child tasks |
| AgentTeam implementing its own agent markdown parser | would drift from TS-first source precedence and custom agent validation |
| AgentTool directly constructing teammate or child QueryEngine configs | keeps Tool layer from owning runtime semantics |

Implementation implication: `AgentHandle::spawn_agent` can remain the common callback boundary,
but the concrete `SwarmAgentHandle` should split immediately into two branches:

```text
AgentTool input
  -> if teammate shape: swarm_teammate::spawn_teammate(...)
  -> else: AgentRuntime::spawn_subagent(...)
```

Both branches can call `coco-subagent` for definition resolution, but only the subagent
branch uses `SubagentManager`.

## Target Architecture

### Module Layout

Review 结论：建议新增一个纯逻辑 crate，但不要把 runtime 放进去。推荐 crate
package name 是 `coco-subagent`，路径是 `coco-rs/core/subagent`。

命名决策：

| 候选名 | 结论 | 原因 |
|--------|------|------|
| `coco-subagent` | recommended | 准确描述功能域：由主 agent 触发的 task-scoped 子 agent；不会暗示 owning main query agent 或 swarm teammate runtime |
| `coco-agent` | not recommended | 过宽，容易被误用来放主 agent loop、TUI agent state、swarm teammate、runtime execution 等逻辑 |
| `coco-agent-runtime` | reject | runtime 必须留在 `app/state`，否则会引入 `coco-query`、tasks、worktree、event sink 等上层依赖 |
| `coco-agent-definitions` | reject | 太窄；该 crate 不只是 definition，还包含 prompt、filter、source precedence、validation、snapshot |
| `coco-agent-tool` | reject | `AgentTool` 应保持 thin entry；crate 不是 tool 实现层 |
| `coco-subagent-core` | reject for now | `core/subagent` 路径已经表达 core 层；crate package 名不需要重复 `core` |

术语说明：TS 和用户配置仍使用 `agent`、`agents`、`agentType`、`/agents`，
Rust 类型也继续使用 `AgentDefinition`。crate 名使用 `subagent` 是为了明确
执行语义和依赖边界：它服务于 child agent/catalog/planning，不拥有主 agent loop。

`subagent` 是否过窄：不会。这里的 crate 不命名为 `coco-agent`，正是为了避免把
三种不同运行实体混在一起。

| TS/runtime entity | 触发入口 | 生命周期 | 是否属于 `coco-subagent` scope |
|-------------------|----------|----------|-------------------------------|
| main agent / standalone agent | CLI/TUI/session main loop | session-level | no；属于 `coco-query` + `coco-state` session runtime |
| AgentTool subagent | `tools/AgentTool` without teammate spawn | task-scoped；foreground/background；完成即终止 | yes；definition catalog、prompt、filter、spawn planning 属于该 crate |
| Agent Team teammate | `tools/AgentTool` with `team_name + name` or team tools | team/session-level；有 identity、mailbox、roster、viewable transcript | partial；只复用 `AgentDefinition` 解析和 tool/model prompt 策略，spawn/lifecycle 属于 swarm/team runtime |

TS 里也不是把 team member 当作普通 AgentTool subagent。`tools/AgentTool` 在
`teamName && name` 时走 `spawnTeammate()` 并返回 `teammate_spawned`；
否则才继续解析 `selectedAgent` 并运行普通 subagent。`tools/AgentTool/prompt`
也明确限制 teammate 不能再 spawn teammate，只能省略 `name/team_name` 去 spawn
subagent。analytics 侧还单独区分 `agentType: 'teammate' | 'subagent' | 'standalone'`。

因此 crate 的精确边界是：`coco-subagent` 管 agent definition/catalog/filter/prompt
这些可复用规则；`coco-state` 继续管 standalone main agent、AgentTool subagent
execution、background task、swarm teammate lifecycle。team 里的 agent 应在 Rust
类型上优先叫 `Teammate`，不要把它并入 `SubagentManager`。

独立 crate 评估：

| 判断 | 说明 |
|------|------|
| definition/prompt/filter 会被多个 crate 使用 | AgentTool prompt、slash command、runtime、tests 都需要同一套逻辑 |
| 这些逻辑是纯 TS parity 规则 | parser、source precedence、prompt formatting、tool filtering 不需要 app state |
| 放在 `app/state` 会让 `commands` 和 `query` 依赖变重 | `/agents` 和 prompt options 不应反向依赖 swarm runtime |
| 放在 `core/tools` 会污染 Tool 实现层 | `AgentTool` 应保持 thin entry，不应拥有 loader/store |
| 放在 `common/types` 会让 types crate 变胖 | `types` 应只放 DTO/enum，不放 loader、frontmatter、catalog、prompt renderer |
| 文件规模预计超过 500 LoC | 按仓库 module size 约束，独立 crate 更合理 |

独立 crate 的主要收益：

| 收益 | 说明 |
|------|------|
| 单一行为来源 | TS-first source precedence、definition validation、AgentTool prompt、tool filter 只实现一次 |
| 可单测 | 不启动 QueryEngine、不初始化 AppState，就能覆盖 custom agent parser、built-in catalog、allowedAgentTypes |
| 降低横向耦合 | `commands` 可以做 `/agents list/show/validate`，但不需要依赖 `coco-state` |
| 降低 runtime 风险 | `app/state` 只消费已解析的 catalog/plan，不重复处理 YAML、source order、prompt text |
| 便于迁移 | 可以先让旧 runtime 调用新 catalog，再逐步替换 background/fork/tool filtering |

独立 crate 的风险和约束：

| 风险 | 约束 |
|------|------|
| 变成 god crate | 只允许 pure logic；任何 tokio task、QueryEngine、AppState mutation、event emit 都禁止进入 |
| 和 `coco-types` 职责重叠 | `coco-types` 放 wire/data types；`coco-subagent` 放 loading/resolution/rendering/validation |
| 和 `coco-tools` 职责重叠 | `coco-tools` 只保留 `AgentTool` schema、basic validation、调用 `AgentHandle`、format result |
| 和 `coco-state` 职责重叠 | `coco-state` 拥有 `AgentRuntime`、`SubagentManager`、background lifecycle、worktree、output files |
| 早期迁移成本 | 第一阶段只迁 loader/store/prompt/filter，不同时移动 execution path，避免大爆炸重构 |

推荐布局：

```text
coco-rs/common/types/src/agent.rs
  AgentDefinition, AgentSource, AgentIsolation, MemoryScope, AgentColorName

coco-rs/core/subagent/
  Cargo.toml
  src/lib.rs
  src/definition_store.rs
  src/frontmatter.rs
  src/builtins.rs
  src/prompt.rs
  src/filter.rs
  src/snapshot.rs
  src/validation.rs
  TS-first pure logic:
    definition loading
    source precedence
    active/all/failed snapshots
    AgentTool prompt formatting
    color validation
    tool filtering
    allowedAgentTypes extraction
  no dependency on app/query/state/tools runtime

coco-rs/core/tools/src/tools/agent.rs
  AgentTool thin protocol entry

coco-rs/app/query/src/agent_adapter.rs
  AgentQueryEngine implementation backed by QueryEngine

coco-rs/app/state/src/agent_runtime.rs
  AgentRuntime, AgentRuntimeConfig, AgentSpawnPlan

coco-rs/app/state/src/subagent_manager.rs
  instances, background tasks, output files, cancellation, deltas

coco-rs/app/state/src/swarm_agent_handle.rs
  AgentHandle impl, routes teammate or standalone runtime

coco-rs/commands/src/...
  /agents command and prompt command agent routing
```

New crate scope:

| 放入 `coco-subagent` | 不放入 `coco-subagent` |
|---------------------|------------------------|
| TS AgentDefinition source precedence | tokio task spawning |
| markdown/json parser | `AgentQueryEngine` execution |
| built-in agent catalog | worktree create/cleanup |
| AgentTool prompt formatter | app state mutation |
| color validation and stable color data | task notifications |
| tool filtering and `Agent(type)` restrictions | mailbox/team runner |
| validation diagnostics | hooks/MCP connection lifecycle execution |

Dependency rule:

```text
coco-types
  <- coco-subagent
  <- coco-tools / coco-commands / coco-state / coco-query consumers

coco-subagent must not depend on:
  coco-tools
  coco-query
  coco-state
  app crates
```

Workspace integration:

| File | Change |
|------|--------|
| `coco-rs/Cargo.toml` members | add `"core/subagent"` under core crates |
| `coco-rs/Cargo.toml` workspace deps | add `coco-subagent = { path = "core/subagent" }` |
| `coco-rs/core/subagent/Cargo.toml` | package name `coco-subagent` |
| consumers | add workspace dependency only where required: `coco-tools`, `coco-commands`, `coco-state`, maybe `coco-query` |

Suggested `coco-subagent` dependencies:

| Dependency | Reason |
|------------|--------|
| `coco-types` | `AgentDefinition` DTOs, permission/model/tool enums |
| `coco-frontmatter` | markdown frontmatter parsing |
| `serde`, `serde_json` | TS-compatible markdown/json definition loading |
| standard `Vec` + `BTreeMap` first | preserve deterministic snapshots without adding a new dependency; add an ordered-map crate only if insertion order becomes required |
| `thiserror` or crate-local error enum | loader/validation diagnostics without pulling app errors |
| `tracing` | load/validation diagnostics only |

Avoid dependencies (**hard rules — enforce in CI on the first PR**):

| Dependency | Why not |
|------------|---------|
| `tokio`、`tokio-util` | 无 background task、watcher、cancellation、async runtime；任何 `tokio::spawn`/`mpsc`/`tokio::fs` 出现在该 crate 都视作回归 |
| `coco-tool` / `coco-tools` | would invert the intended thin AgentTool boundary |
| `coco-query` | QueryEngine execution belongs to app/query and app/state |
| `coco-state` | AppState and lifecycle ownership stay above core |
| `coco-commands` | commands consume catalog APIs; catalog must not know commands |

CI guard: 第一个 PR 即在 `coco-rs/core/subagent/Cargo.toml` 上加 `# DO NOT ADD tokio`
注释，并增加 workspace lint（或 `cargo deny`/`cargo udeps` 集成）确保 `tokio*` 不出现
在 `coco-subagent` 的依赖图。

Allowed side effects: `coco-subagent` 可使用同步 `std::fs` 在被显式调用 `load()`/`reload()`
时读取 definition 文件；不允许 spawn tokio task、保持 watcher、写 background output、
管理 worktree、持有 event sink 或 cancellation token。Watcher 必须由 `app/state` 拥有，
变化时调用 `AgentDefinitionStore::reload()`（同步）。

Recommended public API surface:

| API | Purpose | Primary consumers |
|-----|---------|-------------------|
| `AgentDefinitionStore` | load built-in/plugin/user/project/flag/policy definitions and expose snapshots | `coco-state`, `/agents` commands |
| `AgentCatalogSnapshot` | immutable active/all/failed view for one turn or command invocation | `coco-tools`, `coco-commands`, `coco-state` |
| `AgentLoadReport` | diagnostics with source, path, and validation error | `/agents validate`, tests |
| `AgentToolPromptRenderer` | render TS-style dynamic AgentTool prompt lines | `coco-tools` |
| `AgentDefinitionValidator` | enforce required `name`/`description`, color enum, source metadata | loader and tests |
| `AgentToolFilter` | apply allow-list, deny-list, memory/background restrictions, `Agent(type)` restrictions | `coco-state`, `coco-query` adapter |
| `AllowedAgentTypes` | parse and canonicalize nested `Agent(type)` restrictions; optionally parse `Task(type)` compatibility | `coco-tools`, `coco-state` |
| `BuiltinAgentCatalog` | provide TS parity built-ins and `coco-rs` extension built-ins behind explicit flags | store initialization |

Do not expose APIs that execute agents. The first execution-owning type remains
`coco-state::AgentRuntime`; the first query-owning type remains `coco-query::AgentQueryEngineAdapter`.

### Runtime Components

```text
AgentDefinitionStore
  lives in coco-subagent
  loads built-ins/custom/plugin/flag/policy
  exposes active/all/failed snapshots
  validates definitions

AgentRuntime
  lives in app/state
  resolves AgentSpawnRequest into AgentSpawnPlan
  resolves model/permission/isolation/tools/system prompt
  calls SubagentManager for lifecycle

SubagentManager
  lives in app/state
  tracks instances
  starts foreground/background execution
  owns output files and cancel tokens
  exposes status/output/delta APIs

AgentQueryEngineAdapter
  creates filtered child QueryEngine
  runs prompt or fork messages
  returns AgentQueryResult
```

### Request Flow

AgentTool foreground flow:

```text
Model calls AgentTool
  -> AgentTool validates prompt
  -> AgentHandle.spawn_agent(request)
  -> SwarmAgentHandle routes standalone
  -> AgentRuntime.resolve(request)
  -> AgentDefinitionStore.get_active(type)
  -> filter tools
  -> build AgentQueryConfig
  -> SubagentManager.run_foreground
  -> AgentQueryEngineAdapter.execute_query
  -> child QueryEngine runs with filtered ToolRegistry
  -> AgentSpawnResponse::Completed
  -> AgentTool returns model-visible result
```

AgentTool background flow:

```text
Model calls AgentTool with run_in_background=true
  -> AgentRuntime resolves spawn plan
  -> SubagentManager creates instance and output file
  -> tokio task starts child AgentQueryEngine
  -> parent immediately receives AsyncLaunched
  -> child writes final output and terminal status
  -> query_agent_status/get_agent_output read from manager
```

Slash command flow:

```text
User enters /build
  -> command registry resolves PromptCommandData
  -> if agent is Some("build"), call AgentRuntime.spawn directly
  -> no extra model turn required
  -> result is rendered to TUI/SDK as command result or task handle
```

Fork flow:

```text
AgentTool request has no subagent_type and fork is enabled
  -> AgentTool or runtime constructs ForkContext from ToolUseContext.messages
  -> AgentSpawnRequest carries fork_context_messages
  -> AgentRuntime selects fork/general definition
  -> AgentQueryEngineAdapter.run_with_messages(parent messages + new prompt)
```

## Data Model Changes

### `AgentDefinition`

Current `coco_types::AgentDefinition` is close enough to keep. Suggested additions or normalization:

| Field | Action | Reason |
|-------|--------|--------|
| `source` | add TS-compatible `AgentSource` | needed for priority overwrite and `/agents show` |
| `when_to_use` | add or alias from `description` | TS prompt line uses `whenToUse`; markdown field is `description` |
| `system_prompt` or body prompt renderer | add explicit field/renderer | current `initial_prompt` naming is ambiguous; TS body/JSON prompt is system prompt |
| `initial_prompt` | keep separate | TS `initialPrompt` is first user turn prefix, not system prompt |
| `hooks` | add typed agent hook definitions | support agent-scoped lifecycle hooks |
| `memory` | normalize from `memory_scope` | align with custom frontmatter |
| `background` | keep | default background mode |
| `required_mcp_servers` | keep | prompt visibility and runtime readiness |
| `allowed_tools` | keep but normalize naming | markdown uses `tools`; type can keep `allowed_tools` |
| `disallowed_tools` | keep | deny-list |
| `color` | keep and validate | TS uses color in loading, spawn UI, grouped progress, telemetry |
| `effort` | keep | TS supports effort levels and integer effort |
| `omit_claude_md` | keep | TS read-only built-ins use it as token optimization |
| `use_exact_tools` | keep if needed | supports TS exact tool list behavior |

Recommended Rust source enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AgentSource {
    #[default]
    BuiltIn,
    Plugin,
    UserSettings,
    ProjectSettings,
    FlagSettings,
    PolicySettings,
}

impl AgentSource {
    /// Priority for conflict resolution. Higher wins. Mirrors cocode-rs
    /// `definition.rs:113-137` and TS `getActiveAgentsFromList` map-overwrite
    /// order in `loadAgentsDir.ts:203-216`.
    pub fn priority(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Plugin => 1,
            Self::UserSettings => 2,
            Self::ProjectSettings => 3,
            Self::FlagSettings => 4,
            Self::PolicySettings => 5,
        }
    }
}
```

Optional `coco-rs` extensions such as SDK-provided agents should not be inserted silently
between these TS sources. If added, document the exact priority and expose it in
`/agents show`.

Recommended color enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentColorName {
    Red,
    Blue,
    Green,
    Yellow,
    Purple,
    Orange,
    Pink,
    Cyan,
}
```

### `AgentSpawnRequest`

Recommended changes:

| Field | Current | Proposed |
|-------|---------|----------|
| `run_in_background` | `bool` | `Option<bool>` internally or add `run_in_background_set` |
| `fork_context_messages` | missing | add `Vec<serde_json::Value>` |
| `allowed_agent_types` | missing | add `Option<Vec<String>>` for nested restrictions |
| `source` | missing | optional callsite source for observability |
| `command_name` | missing | optional slash command provenance |

Why `run_in_background` should become tri-state:

| Input | Meaning |
|-------|---------|
| absent | use definition default |
| false | force foreground |
| true | force background |

If public schema compatibility requires bool, `AgentTool` can still accept bool and runtime can use an internal `SpawnInput` with `Option<bool>`.

### `AgentQueryConfig`

Recommended additions:

| Field | Need |
|-------|------|
| `allowed_tools` | already exists, must be applied |
| `tool_registry_filter` | optional explicit allow-list if registry filtering is easier by predicate |
| `skills` | preload agent skills |
| `mcp_servers` | agent-scoped MCP readiness |
| `hooks` | lifecycle hooks |
| `memory_scope` | memory prompt and write policy |
| `system_prompt_suffix` | critical reminders |
| `use_custom_prompt` | replace default prompt |
| `task_type_restrictions` | nested Agent restrictions |
| `parent_session_id` | lineage |
| `output_file` | tee background output |
| `cancel_token` | cancellation |

### `AgentSpawnPlan`

Introduce a runtime-internal named struct:

```rust
pub struct AgentSpawnPlan {
    pub agent_id: String,
    pub agent_type: String,
    pub definition: AgentDefinition,
    pub prompt: String,
    pub description: Option<String>,
    pub model: String,
    pub effort: Option<ThinkingLevel>,
    pub permission_mode: PermissionMode,
    pub background: bool,
    pub isolation: AgentIsolation,
    pub color: Option<AgentColorName>,
    pub cwd_override: Option<PathBuf>,
    pub system_prompt: String,
    pub initial_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_agent_types: Option<Vec<String>>,
    pub max_turns: Option<i32>,
    pub fork_context_messages: Vec<serde_json::Value>,
    pub output_file: Option<PathBuf>,
}
```

目的：

| 目的 | 说明 |
|------|------|
| 避免重复解析 | definition/model/tools 只解析一次 |
| 便于测试 | unit test 可以断言 plan |
| 降低 coupling | manager 只执行 plan，不再读 request |
| 便于日志 | trace 中输出 plan summary |

## Custom Agent Format

推荐 markdown 格式：

```md
---
name: build
description: Runs builds, tests, and reports actionable failures
tools: [Bash, Read, Grep]
disallowedTools: [Write, Edit]
color: red
model: inherit
effort: medium
permissionMode: default
maxTurns: 20
background: true
isolation: worktree
initialPrompt: "Start by identifying the project build system."
memory: project
skills: []
mcpServers: []
requiredMcpServers: []
---
You are a build verification agent.

Run the smallest useful build and test commands for the current change.
Report exact failing commands, key errors, and likely owner files.
Do not edit files unless the definition explicitly allows write tools.
```

Supported locations:

| Location | Source | Notes |
|----------|--------|-------|
| `~/.coco/agents/*.md` | UserSettings | user-wide custom agents |
| `<project>/.coco/agents/*.md` | ProjectSettings | project-specific agents |
| `<project>/.claude/agents/*.md` | ProjectSettings compatibility | optional compatibility with TS/Claude Code |
| plugin contribution | Plugin | loaded through plugin manifest |
| JSON/CLI flag agents | FlagSettings | TS-compatible flag supplied agents |
| managed policy agents | PolicySettings | highest-priority managed definitions |
| SDK bootstrap | extension | optional `coco-rs` extension; priority must be explicitly documented |

Validation rules:

| Rule | Behavior |
|------|----------|
| missing `name` | failed definition |
| missing `description` | failed or inactive definition |
| duplicate source/name | TS parity overwrite by source priority |
| unknown tool | warning or failed depending on strict mode |
| unknown model alias | runtime error at spawn or validate warning |
| invalid color | ignore color and record validate warning |
| invalid effort | ignore effort and record validate warning |
| invalid isolation | failed definition |
| invalid permissionMode | failed definition |
| empty body | invalid for JSON `prompt`; markdown body should warn or fail depending strict mode |

## Built-in Agents

TS parity built-ins:

| Agent | Source behavior | Notes |
|-------|-----------------|-------|
| `general-purpose` | always included unless built-ins are disabled in noninteractive SDK mode | default when `subagent_type` is omitted and fork is disabled |
| `statusline-setup` | always included unless built-ins are disabled in noninteractive SDK mode | TS built-in carries `color: orange` |
| `explore` | feature-gated | read-only exploration; TS uses slim context patterns such as `omitClaudeMd` |
| `plan` | feature-gated | planning/read-only behavior |
| `claude-code-guide` | included for non-SDK entrypoints | guide/help agent; TS uses restrictive permission behavior |
| `verification` | feature-gated | TS built-in carries `color: red`, `background: true`, and disallowed tools |

`coco-rs` extension built-ins:

| Agent | Decision | Reason |
|-------|----------|--------|
| `build` | optional extension, not TS parity | User requirement asks for build subagent; implement as a bundled `coco-rs` agent or project template, and label it as `coco-rs`-specific |
| `review` | optional extension unless TS adds one | Useful for code review workflows, but not part of current TS `getBuiltInAgents()` list |

Naming decision (TS parity is **case-sensitive**):

TS 当前 built-in `agentType` 使用混合大小写：`Explore` / `Plan` 大写首字母
(`exploreAgent.ts:65`、`planAgent.ts:74`)，其余 `general-purpose`、
`statusline-setup`、`verification`、`claude-code-guide` 全小写。这不是历史遗留，
而是被多处 case-sensitive 逻辑直接消费：

| 影响点 | TS 位置 | 后果（如果 Rust 单方面小写化）|
|--------|---------|-----------------------------|
| One-shot built-ins 集合 `{'Explore','Plan'}` | `constants.ts:9-12` | SendMessage trailer 跳过逻辑失效，`explore`/`plan` 会错误地附带 continuation trailer |
| 用户 permission 规则 `Agent(Explore)` | `permissionSetup.ts:324-325` | 用户既有规则不再匹配，规则需要迁移工具 |
| 遥测 `tengu_agent_tool_selected.agentType` | `AgentTool.tsx:423` | 与 ant 端遥测面板的 cohort 切片不一致 |

| Issue | Decision |
|-------|----------|
| Canonical built-in IDs | **保留 TS 大小写**：`Explore`、`Plan`、`general-purpose`、`statusline-setup`、`verification`、`claude-code-guide`。不允许单方面小写化 |
| 现有 `SubagentType::as_str()` 全小写 | 在 Phase 1 修正为 TS-parity 大小写；为旧值保留 alias parser，但**输出端**永远输出 canonical 大小写 |
| Alias 行为 | 输入端接受 `explore`/`plan` 等小写（向用户友好），spawn 时立即 canonicalize；输出端（result、telemetry、prompt list、permission rule 显示）始终使用 TS 大小写 |
| Tool schema examples | 使用 TS canonical 大小写示例 (`Explore`、`Plan`)；在描述中说明小写为接受的 alias |
| One-shot 集合 | Rust 常量必须是 `["Explore", "Plan"]` 字面量；测试断言完全相同 |

## Slash Command Integration

### Command Definition

Prompt command metadata already supports agent routing:

```rust
pub struct PromptCommandData {
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub context: CommandContext,
    pub agent: Option<String>,
    pub thinking_level: Option<String>,
    pub hooks: Vec<String>,
}
```

Recommended markdown/frontmatter command example:

```md
---
name: build
description: Run build verification in a subagent
agent: build
context: fork
allowed_tools: [Bash, Read, Grep]
model: inherit
---
Verify the current change. Use targeted commands first.
Arguments from the user:
{{args}}
```

### Runtime Behavior

Slash command execution matrix:

| Command | `agent` | `context` | Behavior |
|---------|---------|-----------|----------|
| `/help` | none | local | existing local handler |
| `/explain` | none | inline | inject prompt into current turn |
| `/plan` | `plan` | fork | spawn `plan` agent with fork context |
| `/build` | `build` | fork | spawn `build` agent with fork context |
| `/review` | `review` | fork | spawn `review` agent |
| `/agents list` | local | local | list active definitions |
| `/agents run build ...` | `build` resolved by args | fork/default | direct runtime spawn |

### `/agents` Command

Replace placeholder output with real commands:

| Command | Behavior |
|---------|----------|
| `/agents` | alias for `/agents list` |
| `/agents list` | show active agents with source, background, isolation, model |
| `/agents show <name>` | show active definition and source chain |
| `/agents validate` | show failed files, unknown tools, invalid fields |
| `/agents reload` | reload definition store |
| `/agents run <name> <prompt>` | spawn selected agent |
| `/agents paths` | show searched directories |

Output format should be concise in TUI and structured in SDK/NDJSON where available.

## Refactor Plan

### Phase 0: Invariants And Test Harness

Goal: lock down expected behavior before moving code.

Deliverables:

| Deliverable | Files |
|-------------|-------|
| Add fake `AgentQueryEngine` tests | `coco-rs/app/state/src/...` |
| Add fake definition store tests | new store module |
| Add AgentTool response shape tests | `coco-rs/core/tools/src/tools/agent.test.rs` |
| Add slash command routing tests | `coco-rs/commands` or `app/query` command path |

Invariants:

| Invariant | Test |
|-----------|------|
| unknown agent returns clean failure | spawn `missing-agent` |
| background spawn starts execution | fake engine records call after `AsyncLaunched` |
| child allowed tools are applied | fake child registry sees only allowed tools |
| slash command with `agent` bypasses main model | fake runtime sees spawn call |
| normal session has real `AgentHandle` | bootstrap test checks not `NoOpAgentHandle` |
| **TS parity invariants** (lock these on day 0) | |
| one-shot set is case-sensitive `{"Explore","Plan"}` | Rust constant matches `constants.ts:9-12` 字节相同；小写输入不命中 |
| empty-content marker exact text | 字面量 `(Subagent completed but returned no output.)` (`AgentTool.tsx:1347-1350`) |
| AgentTool prompt 行格式 | `- {agentType}: {whenToUse} (Tools: {toolsDescription})` (`prompt.ts:43-46`) |
| 工具描述格式 | "All tools" / "All tools except X, Y" / explicit list 三分支与 TS 一致 (`prompt.ts:15-37`) |
| `Agent(...) ∪ Task(...)` regex | `^([^(]+)(?:\(([^)]*)\))?$` 同时匹配两种 tool 名 (`permissionSetup.ts:324-325`) |
| permission inheritance 默认覆盖范围 | 仅 `bypassPermissions`、`acceptEdits`；`auto` 仅在 feature gate 开启时覆盖 |
| schema 条件可见性 | `cwd` 仅在 `feature('KAIROS')` 暴露；`run_in_background` 在后台禁用或 fork enabled 时 omit |

### Phase 1: Unify Agent Definition Loading

Goal: one loader/store for built-in and custom agents.

Tasks:

| Task | Details |
|------|---------|
| Add `AgentSource` | likely in `coco-rs/common/types/src/agent.rs` |
| Add `coco-subagent` crate | pure TS-first definition/prompt/filter/validation logic |
| Add `AgentDefinitionStore` | in `coco-subagent`, not app/state |
| Implement source overwrite | match TS active selection semantics: later source wins by agent type |
| Replace duplicate loaders | deprecate direct use of `agent_spawn.rs` and `agent_advanced.rs` loader paths |
| Add typed markdown parser | serde frontmatter, camelCase support |
| Add alias normalization | `Explore` -> `explore`, `Plan` -> `plan` |
| Add failed definitions | path + error + source |
| Add built-in prompt renderers | support TS-style dynamic `getSystemPrompt` behavior |

Acceptance:

| Check | Expected |
|-------|----------|
| `~/.coco/agents/foo.md` loads | active definition `foo` exists |
| project overrides user | project definition wins for the same agent type |
| tools arrays overwrite | higher-priority active definition supplies the final tools list by default |
| invalid yaml reported | `/agents validate` can show it |
| built-in aliases resolve | `Explore` and `explore` map to same canonical agent |

### Phase 2: Dynamic AgentTool Prompt

Goal: model sees correct agent list and restrictions.

Tasks:

| Task | Details |
|------|---------|
| Extend `PromptOptions` | include active agent definitions, allowed agent types, failed count if useful |
| Override `AgentTool::prompt` | list available agents with descriptions |
| Apply prompt visibility filters | required MCP ready, allowed agent types, permission restrictions |
| Improve schema docs | lowercase examples and background guidance |
| Improve result messages | include output file and how to query background result |

Prompt content should include:

| Content | Reason |
|---------|--------|
| agent name | model needs valid `subagent_type` |
| description | model chooses correct agent |
| background default | model knows if async likely |
| isolation default | model understands worktree behavior |
| tool summary | model avoids asking write-capable work from read-only agent |

Acceptance:

| Check | Expected |
|-------|----------|
| custom agent appears in AgentTool prompt | yes |
| disallowed agent hidden by allowed types | yes |
| failed definitions not shown | yes |
| prompt is stable sorted | deterministic snapshots |

### Phase 3: AgentRuntime Spawn Planning

Goal: centralize request-to-plan logic.

Tasks:

| Task | Details |
|------|---------|
| Add `AgentRuntime` | holds definition store, query engine, tool registry metadata, config |
| Add `AgentSpawnPlan` | named internal plan object |
| Resolve definition | request type -> canonical definition |
| Resolve model | request override > definition model > parent model |
| Resolve permission | parent trust modes override definition, else request/definition/default |
| Resolve background | request override > definition background |
| Resolve isolation | request override > definition isolation > none |
| Resolve system prompt | definition prompt + skills/memory/tool list/env |
| Resolve tool filter | definition + background + slash command allowed tools |
| Resolve fork context | request fork messages or no fork |

Acceptance:

| Check | Expected |
|-------|----------|
| plan for `build` has build prompt | yes |
| request model override wins | yes |
| definition background used when request absent | yes |
| explicit foreground overrides background default | yes |
| tool filter result stable | yes |

### Phase 4: Child QueryEngine With Filtered Tools

Goal: child agent sees the correct environment.

Tasks:

| Task | Details |
|------|---------|
| Extend `QueryEngineFactory` | accept filtered tool registry or filter metadata |
| Apply `AgentQueryConfig.allowed_tools` | construct child `ToolRegistry` from parent registry subset |
| Propagate session/cwd | parent session id, child agent id, worktree cwd |
| Propagate permission | effective child `PermissionMode` |
| Propagate max turns | definition/request limit |
| Propagate model | resolved model string |
| Propagate agent handle | nested agents get restricted runtime |
| Propagate skill handle | child skills resolve correctly |
| Propagate hooks/MCP | if available in this phase |

Important implementation point:

```text
Wrong:
  QueryEngineConfig.allowed_tools = [...]
  QueryEngine still receives full ToolRegistry

Right:
  child_tool_registry = parent_tool_registry.filtered(allowed_tools)
  QueryEngine builds tool catalog only from child_tool_registry
```

Acceptance:

| Check | Expected |
|-------|----------|
| child prompt tool list only has allowed tools | yes |
| child cannot execute denied tool by name | unknown/disallowed tool result |
| background child excludes interactive tools | yes |
| nested Agent restrictions apply | `Agent(review)` only allows review |

### Phase 5: Real Background Subagent Lifecycle  **(LANDED 2026-05-15)**

Goal: make `AsyncLaunched` real.

Status: implemented in `coordinator/src/agent_handle/spawn.rs:1034-1407`.
Three `tokio::spawn` sites verified (main execution at line 1278, event drain
at 1121, summary polling at 1157). Task registry integration confirmed —
`register_agent_task()` → `mark_completed()` / `mark_failed()` on terminal
states. Worktree isolation in bg mode is intentionally rejected at line 1044
because cleanup timing cannot be guaranteed.

Tasks:

| Task | Details |
|------|---------|
| Add `SubagentManager` | absorb cocode manager concepts |
| Add instance map | id -> status/output/cancel metadata |
| Add output path policy | e.g. session task output dir |
| Spawn tokio task | child engine runs after response returns |
| Write output file | final output and optional progress |
| Implement status | `query_agent_status` returns running/completed/failed |
| Implement output | `get_agent_output` reads output |
| Implement cancel | cancel token kills background |
| Implement GC | terminal instances cleaned after TTL |
| Emit events | TUI/SDK can observe completion |

Minimum status enum:

```rust
pub enum AgentStatus {
    Running,
    Completed,
    Failed,
    Backgrounded,
    Killed,
}
```

Acceptance:

| Check | Expected |
|-------|----------|
| background fake engine is called | yes |
| output file is returned | yes |
| status transitions running -> completed | yes |
| failed child records error | yes |
| cancellation marks killed | yes |

### Phase 6: Worktree And Fork Integration

Goal: complete isolation and context paths.

Tasks:

| Task | Details |
|------|---------|
| Move worktree decision into runtime | `SwarmAgentHandle` no longer owns standalone worktree logic |
| Keep foreground cleanup policy | remove unchanged worktree, keep changed worktree |
| Background worktree policy（已定，不再 TBD） | 进入 terminal 状态前保留 worktree，path 写入 `AgentInstance` 供 `query_agent_status` 返回；进入 `Completed`/`Failed`/`Killed` 时若 worktree 无 git diff 则清理，否则保留并在 result/output 中带回路径——与 foreground 同策略，避免两套规则 |
| Add `ForkContext` to request/config | messages plus metadata |
| Prevent recursive fork | if already fork child, deny fork path |
| Preserve tool result pairs | fork messages must not break provider normalization |

Acceptance:

| Check | Expected |
|-------|----------|
| foreground worktree child runs in worktree cwd | yes |
| changed worktree path returned | yes |
| unchanged worktree cleaned | yes |
| fork child receives parent messages | yes |
| fork recursion denied | yes |

### Phase 7: Custom Subagents And `/agents`

Goal: expose definition management to users.

Tasks:

| Task | Details |
|------|---------|
| Implement `/agents list` | read active store |
| Implement `/agents show <name>` | show active definition and sources |
| Implement `/agents validate` | show parse/load errors |
| Implement `/agents reload` | reload store |
| Implement `/agents run` | direct runtime spawn |
| Add autocomplete | use active agent names |
| Add docs examples | built-in and custom examples |

Acceptance:

| Check | Expected |
|-------|----------|
| `/agents list` shows built-ins | yes |
| custom file appears after reload | yes |
| invalid file appears in validate | yes |
| `/agents run build test this` starts build agent | yes |

### Phase 8: Slash Command Agent Routing

Goal: `agent` field becomes executable behavior.

Tasks:

| Task | Details |
|------|---------|
| Locate prompt command execution path | app/query or command queue integration |
| Add `AgentCommandExecutor` | maps `PromptCommandData` to `AgentSpawnRequest` |
| Pass args into prompt | append or template substitute |
| Apply command allowed tools | intersection with agent definition tools |
| Apply command model | request model override |
| Apply command context | Inline/Fork behavior |
| Render response | sync result or async task handle |

Acceptance:

| Check | Expected |
|-------|----------|
| command with `agent` calls runtime directly | yes |
| command without `agent` keeps old behavior | yes |
| command `allowed_tools` limits child tools | yes |
| command `model` overrides definition | yes |

### Phase 9: Bootstrap Wiring

Goal: normal sessions use real agent runtime.

Tasks:

| Task | Details |
|------|---------|
| Build definition store at startup | CLI/TUI/SDK |
| Build AgentRuntime | include parent config, tool registry, event sink |
| Build SwarmAgentHandle with runtime | teammate + standalone routing |
| Install `with_agent_handle` | QueryEngine creation |
| Install child QueryEngine factory | includes filtered registry support |
| Session bootstrap agents | send active agent list to prompt options/TUI |

Acceptance:

| Check | Expected |
|-------|----------|
| AgentTool in CLI no longer errors `not available` | yes |
| TUI autocomplete sees agents | yes |
| headless SDK can spawn agent | yes |
| tests assert no normal NoOpAgentHandle | yes |

### Phase 10: Cleanup And Extraction

Goal: remove duplicated code after behavior is stable.

Tasks:

| Task | Details |
|------|---------|
| Delete duplicate loader paths | after store is used everywhere |
| Consolidate tool filtering | one implementation |
| Update docs | `crate-coco-tools.md`, `crate-coco-commands.md` |
| Add migration notes | `.claude/agents` compatibility |
| Keep `coco-subagent` crate boundary clean | no app/query/state dependencies; runtime remains in app/state |

Acceptance:

| Check | Expected |
|-------|----------|
| no second markdown parser | yes |
| no second tool filter implementation | yes |
| docs reflect actual behavior | yes |

## Testing Strategy

### Unit Tests

| Area | Tests |
|------|-------|
| definition parser | valid markdown, missing name, missing description, invalid yaml |
| source overwrite | built-in/plugin/user/project/flag/policy priority |
| alias canonicalization | `Explore` -> `explore` |
| tool filtering | system blocked, allow-list, deny-list, background safe, MCP bypass |
| spawn planning | model/permission/background/isolation resolution |
| AgentTool result mapping | completed, async, failed, teammate |
| slash routing | command agent direct spawn |

### Integration Tests

| Scenario | Expected |
|----------|----------|
| TS built-in verification or `coco-rs` build extension sync | fake query engine returns output |
| custom agent from project dir | appears in active store and spawns |
| background agent | returns id/output file and eventually completed |
| worktree foreground | cwd override passed to child |
| fork context | parent messages passed to `run_with_messages` |
| denied tool | child registry excludes tool |
| `/agents run` | runtime called with selected type |

### Snapshot Tests

Snapshot candidates:

| Output | Reason |
|--------|--------|
| AgentTool dynamic prompt | stable prompt contract |
| `/agents list` | TUI/user-visible output |
| `/agents show` | active definition plus source chain display |
| background launch result | model-visible JSON/text |

### Command Workflow

Run scoped tests first:

```bash
cd coco-rs
just test-crate coco-tools
just test-crate coco-state
just test-crate coco-query
just test-crate coco-commands
```

Run broader checks before commit:

```bash
cd coco-rs
just fmt
just pre-commit
```

## Migration Plan

### Backward Compatibility

| Existing behavior | Migration |
|-------------------|-----------|
| `subagent_type: "Explore"` | accept alias, canonicalize to `explore` |
| `.claude/agents` | keep compatibility loader |
| `run_in_background: false` omitted | preserve foreground unless definition says background |
| unsupported `remote` | keep explicit failure |
| existing swarm teammate | route unchanged at first |

### Deprecation

| Item | Deprecation path |
|------|------------------|
| duplicate `agent_spawn.rs` parser | mark internal, replace callers, then delete |
| duplicate `agent_advanced.rs` discover logic | move useful pieces to store/filter |
| uppercase built-in constants | keep aliases, document lowercase canonical names |
| empty child system prompt | replaced by runtime-generated prompt |

## Risks And Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Tool filtering bug exposes unsafe tools | high | child registry filtering tests and deny-by-default for unknown tools |
| Background task leaks | medium | cancellation tokens, GC, terminal status tracking |
| Slash command path bypasses permissions | high | command spawn still uses runtime permission and filtered tools |
| Source overwrite surprises users | medium | `/agents show` source chain and explicit docs |
| Worktree cleanup removes useful changes | high | only cleanup unchanged worktrees; keep changed path in response |
| Fork context breaks provider message normalization | high | use existing message normalization and add tests with tool_use/tool_result pairs |
| MCP readiness blocks spawn indefinitely | medium | timeout and clear error |
| TUI/CLI bootstrap wiring diverges | medium | shared bootstrap builder for AgentRuntime |

## Deferred TS Files

`tools/AgentTool/` 目录还有以下 TS 源文件，本计划范围内**不**实现，但需要登记
以便后续阶段不被遗忘：

| TS 文件 | 用途 | 处理 |
|---------|------|------|
| `resumeAgent.ts` | resume 已有 background agent，把它的 transcript 接回 parent 会话 | P2：在 Phase 5 实现 background lifecycle 之后；接口预留 `resume_agent(id)` |
| `agentMemorySnapshot.ts` | spawn 时给 child 注入 parent memory 的 snapshot | P2：依赖 `coco-memory` 完成；先在 `AgentSpawnPlan` 留出 `memory_snapshot: Option<...>` |
| `agentMemory.ts` | child agent 写回 memory 的策略 | P2：随 `agentMemorySnapshot.ts` 一并实现 |
| `agentDisplay.ts` | TUI 端 agent 进度/标题行渲染 | P3：归属 `coco-tui`，不进入 `coco-subagent` 范围 |
| `UI.tsx` | TUI 端 AgentTool 渲染入口 | P3：归属 `coco-tui` |

## Telemetry Contract

复刻 TS `tengu_agent_tool_selected` 事件 (`AgentTool.tsx:423`) 必须保留以下属性，
否则 ant 端遥测面板的 cohort 切片会断裂：

| 属性 | 来源 | 注意 |
|------|------|------|
| `agentType` | `selectedAgent.agentType` | 大小写敏感，使用 TS canonical（`Explore`/`Plan` 大写） |
| `color` | `selectedAgent.color`（直接读取，**不**走 `getAgentColor`） | 验证后落 `AgentColorName` 之一；非法值不发 |
| `source` | `AgentSource` | 使用 TS string variant，例如 `userSettings` |
| `model` | resolved model id | spawn 后的最终值，包含 `inherit` 解析结果 |
| `background` | `bool` | 最终 background 决策，不是请求字段 |
| `is_teammate` | `bool` | 区分 subagent / teammate / standalone 三类 |

## Open Questions

| Question | Recommendation |
|----------|----------------|
| Should custom agent path be `.coco/agents` only or also `.claude/agents`? | Use `.coco/agents` canonical, `.claude/agents` compatibility |
| Should `build` be built-in or project template? | Implement as `coco-rs` extension built-in or bundled template, not TS parity |
| Should background be default for build? | Prefer `background: true` for build if output querying is solid; otherwise foreground until Phase 5 lands |
| Should `Agent(type)` or `Task(type)` be used in tools frontmatter? | **双形并行**，与 TS 一致：`AgentTool` 注册 `Agent` 主名 + `Task` alias (`AgentTool.tsx:228`)，permission rule parser 同时接受 `Agent(...)` 与 `Task(...)`，**不重写**用户既有规则 |
| Should plugin agents override user agents? | No; source priority should keep user/project higher than plugin |
| Should custom agents be hot-reloaded? | Store should support reload first; watcher can be later |

## Acceptance Criteria

The refactor is complete when all criteria pass:

| Criterion | Required |
|-----------|----------|
| TS built-in `verification` or `coco-rs` build extension agent can be spawned | yes |
| custom markdown agent can be loaded and spawned | yes |
| AgentTool prompt lists active agents with descriptions | yes |
| `run_in_background` starts a real child task | yes |
| `get_agent_output` returns background output | yes |
| child QueryEngine only sees filtered tools | yes |
| slash command with `agent` directly triggers that agent | yes |
| `/agents list/show/validate/run` work | yes |
| normal CLI/TUI sessions install real AgentHandle | yes |
| remote isolation remains explicit unsupported if not implemented | yes |
| duplicate loader/filter implementations are removed or no longer used | yes |

## Recommended Implementation Order

Use this exact order to reduce risk:

1. Add tests and fake runtime harness.
2. Add `AgentDefinitionStore` and built-in/custom loader.
3. Add dynamic `AgentTool::prompt`.
4. Add `AgentRuntime` spawn planning.
5. Make child QueryEngine use filtered ToolRegistry.
6. Move standalone `SwarmAgentHandle` path to `AgentRuntime`.
7. Implement real background execution.
8. Add `/agents` command.
9. Add slash command `agent` direct routing.
10. Add fork context and background worktree.
11. Remove duplicate loaders and update docs.

## Minimal First PR

If this needs to be split into small PRs, the first PR should be:

| Item | Scope |
|------|-------|
| `AgentDefinitionStore` | built-ins + project/user markdown loading |
| dynamic AgentTool prompt | active definitions only |
| `/agents list/validate` | no spawn yet |
| tests | parser, source overwrite, prompt snapshot |

Why this first:

| Reason | Explanation |
|--------|-------------|
| low runtime risk | no child execution changes |
| unlocks visibility | users can see what agents exist |
| removes duplicate loader pressure | future runtime uses one store |
| validates custom agent schema | catches format issues early |

Second PR:

| Item | Scope |
|------|-------|
| `AgentRuntime` planning | definition/model/tools/system prompt |
| filtered child registry | real safety boundary |
| sync spawn | foreground only |
| tests | fake `AgentQueryEngine` |

Third PR:

| Item | Scope |
|------|-------|
| background lifecycle | real async execution |
| status/output/cancel | manager APIs |
| `/agents run` | direct spawn |
| slash command agent | command routing |

## Final Design Summary

The desired end state is:

```text
Definitions are declarative.
AgentTool is a protocol entry.
Slash commands are deterministic triggers.
AgentRuntime makes all spawn decisions.
SubagentManager owns lifecycle.
Child QueryEngine runs with a real filtered registry.
Background agents are actual tasks, not placeholder responses.
```

This absorbs the strongest parts of `cocode-rs/core/subagent` while preserving
`coco-rs`'s existing `AgentHandle` boundary and QueryEngine architecture.
