# Hermes 架构总览

> ← [返回索引](README.md) · 下一篇：[功能对比矩阵 →](02-feature-comparison.md)

---

## Hermes 架构总览

### 它是什么

Hermes(源码位于 `/lyz/codespace/3rd/hermes-agent/`)是 Nous Research 开源的、以 Python 编写的**自进化型个人助理 Agent**。它与 coco-rs 一样脱胎于 Claude Code 式的工具调用 Agent,但产品定位完全不同:coco-rs 是面向开发者的终端 CLI,而 Hermes 把自己包装成一个**常驻的、跨 20+ 聊天平台、能后台自我改进、能定时执行任务、能在 6 种终端后端里跑代码**的助理。它最鲜明的两个"卖点"是:(1) 真正闭环的自我改进(Curator + background review + 自建 skill);(2) IM 网关(单进程同时对接 Telegram/Discord/Slack/微信/飞书……)。

> 说明:本节的技术论断全部来自对该仓库的深读发现;许可证(MIT)与 "Nous Research" 归属属于项目背景常识,深读发现中仅有间接旁证(如 `tools/environments/managed_modal.py` 的 "Nous-managed Modal"、`scale_to_zero.py` 的 "NAS Labs" 开关),未逐字校验 LICENSE 文件。

### 进程模型:CLI + Gateway + TUI 三形态

Hermes 不是单一二进制,而是**同一份 `AIAgent` 运行时被三个入口复用**:

- **CLI**(`cli.py`,722KB):一次性/交互式命令行入口,直接在本进程构造 `AIAgent`。
- **Gateway**(`gateway/run.py`,~19,114 LoC / 921KB):长驻 asyncio 进程,`GatewayRunner` 在**一个事件循环上同时挂载 20+ 平台适配器**,把每条入站消息桥接到 `AIAgent`。这是 IM 网关、Cron 调度、Kanban 派发的宿主。
- **TUI**(`tui_gateway/` + `ui-tui/`):两进程设计——TypeScript(Ink/React)负责画屏,通过 newline-delimited JSON-RPC over stdio 拉起 `python -m tui_gateway.entry` 后端(`tui_gateway/server.py` 高达 537KB),Python 负责会话/工具/模型/斜杠逻辑。同一个 gateway 还被 Web dashboard 用 PTY 复用。

三个入口最终都落到 `run_agent.py` 的 `AIAgent`。

### 分层/组件图

```
┌────────────────────────────────────────────────────────────────────────────┐
│  入口层   cli.py(722KB)  │  gateway/run.py(921KB)  │  tui_gateway(TS↔Py RPC) │
├────────────────────────────────────────────────────────────────────────────┤
│  Agent 核心(门面 run_agent.py ~5700 LoC,几乎全是转发)                        │
│    prologue  agent/turn_context.py ──► LOOP agent/conversation_loop.py(284KB)│
│                                              └► epilogue agent/turn_finalizer  │
│    旁挂:moa_loop.py · verification_stop.py · turn_retry_state.py             │
│          prompt_caching.py · context_compressor.py(137KB)                    │
├────────────────────────────────────────────────────────────────────────────┤
│  工具层   tools/registry.py(模块级单例,自注册) + toolsets.py(TOOLSETS 字典) │
│    File/Web/Terminal/Browser · delegate_task(154KB) · execute_code(77KB)     │
│    skill_manage · memory · session_search · kanban_* · cronjob · MCP          │
├───────────────┬──────────────────┬───────────────────┬──────────────────────┤
│  技能/记忆     │  学习闭环(自进化) │  调度/多智能体     │  执行后端(6 种)      │
│  skills_hub   │  background_review │  cron/scheduler   │  environments/*.py:   │
│  (4069 LoC)   │  curator.py       │  kanban_db.py     │  local/docker/ssh/    │
│  skill_manager│  skill_provenance │  (>8000 LoC)      │  singularity/modal/   │
│  memory_tool  │  skill_usage      │  delegate/async   │  daytona + file_sync  │
├───────────────┴──────────────────┴───────────────────┴──────────────────────┤
│  Provider 层  providers/ProviderProfile + api_mode + agent/transports/        │
│    anthropic/gemini/bedrock/codex/azure 适配器 · models_dev.py(模型元数据)   │
├────────────────────────────────────────────────────────────────────────────┤
│  平台层   gateway/platforms/base.py(BasePlatformAdapter ~5400 LoC)           │
│    plugins/platforms/<name>/{plugin.yaml,adapter.py} · relay/ · scale_to_zero │
├────────────────────────────────────────────────────────────────────────────┤
│  存储层   hermes_state.SessionDB(SQLite + FTS5) · ~/.hermes/{skills,memories, │
│           cron/jobs.json,teams,checkpoints}                                   │
└────────────────────────────────────────────────────────────────────────────┘
```

关键结构性事实:**分层是"运行时约定"而非"编译期强制"**。`registry ← tools ← model_tools` 的循环导入安全顺序靠人工维护;工具通过 AST 扫描 `tools/*.py` 顶层的 `registry.register(...)` 调用自动发现;平台通过 `plugins/platforms/<name>/adapter.py` 的 `register(ctx)` 动态注册。

### AIAgent 核心回合循环(turn lifecycle)

`AIAgent` 本身是个 ~60 参数构造、几乎每个方法都转发到 `agent/*.py` 的**门面**。真正的驱动是 `agent/conversation_loop.py::run_conversation`(~5000 LoC,单个 try 体约 4000 行),被前奏 `build_turn_context` 和尾声 `finalize_turn` 夹住:

```
用户输入
  │
  ▼  build_turn_context(prologue)
  │    stdio 守卫 · 重置 ~15 个重试计数器 · 新建 IterationBudget(max_iterations)
  │    surrogate 净化 · todo/nudge 计数器水合 · 恢复或构建"缓存系统提示"
  │    建 DB 会话行 · 崩溃前持久化 · preflight 压缩 · pre_llm_call 插件钩子
  │    外部记忆 prefetch
  ▼  外层 while(api_call_count<max_iterations 且 budget.remaining>0,或一次性 grace call)
  │  ┌── 组装 api_messages:把 memory prefetch / 插件上下文 / MoA 综合注入到
  │  │    *当前 user 消息*(绝不改系统提示,保 prefix cache)· 前置字节稳定的系统提示
  │  │    · Anthropic cache_control(system + 末 3 条,4 个断点)· 净化孤儿 tool 结果
  │  │
  │  ├── 内层 while(retry<max_retries,每轮新建 TurnRetryState):一次模型调用
  │  │    (~16 个一次性恢复守卫:各 provider OAuth 刷新 / 429 池 / thinking 签名剥离 /
  │  │     llama.cpp 语法回退 / 1M-beta / 图片缩小 …)
  │  │
  │  ├── 若返回 tool_calls:修复未知名 → 去重/限流 → 并发或串行执行 → guardrail 停机判定
  │  │    → execute_code 退还预算 → 按真实 prompt_tokens 判压缩 → continue
  │  │
  │  └── 若无 tool_calls:content 即最终答案,但先过两道"完成前自检"门:
  │       (1) verify_on_stop:本回合改了真实代码且无新鲜"通过"证据 → 注入合成 user
  │           nudge(finish_reason=verification_required)· continue(上限 2)
  │       (2) pre_verify 插件钩子(上限 3)· 全通过才 break
  ▼  finalize_turn(epilogue)
       budget 耗尽摘要 · 轨迹/会话持久化 · 过度声称页脚 · post_llm_call/on_session_end
       钩子 · 组装结果 dict · **触发后台 memory/skill review(自进化的触发点)**
```

相对"教科书 Agent 循环"(`while not done: 调 LLM; 有 tool 就跑,否则收尾`),Hermes 额外叠了:前奏/尾声分离、两级预算 + grace call、庞大的 provider 专属恢复矩阵、证据门控的完成前自检、MoA 顾问式扇出、回合中 `/steer`、preflight+reactive 压缩、独立预算的子代理委派,以及严格的 prompt-cache 不变量。

### 巨型模块清单

Hermes 的一个显著特征是**超大单文件**——这既是它的能力密度所在,也是其可维护性痛点(`AGENTS.md` 自己就把 `run.py/cli.py/run_agent.py` 标为需要拆解的 "cluster")。

| 模块 | 体量 | 角色 |
|------|------|------|
| `cli.py` | 722KB | CLI 入口 + 大量斜杠逻辑 |
| `gateway/run.py` | ~19,114 LoC / 921KB | `GatewayRunner` 上帝对象(3 大 mixin):多平台复用、`_create_adapter`、重连看护、profile 复用 |
| `tui_gateway/server.py` | 537KB | TUI 后端 JSON-RPC dispatch |
| `agent/auxiliary_client.py` | 300KB | 辅助 LLM 调用(标题/压缩/MoA/curator 共用) |
| `agent/conversation_loop.py` | 284KB | 回合主循环 |
| `tools/terminal_tool.py` | ~132KB | 终端工具 + 6 后端工厂 |
| `agent/context_compressor.py` | ~2800 LoC / 137KB | 头/尾/中压缩 + 结构化摘要 |
| `tools/delegate_tool.py` | ~154KB | 子代理委派(同步批量 + 后台异步) |
| `hermes_cli/kanban_db.py` | >8000 LoC | Kanban 状态机 + `dispatch_once`(在第 6932 行) |
| `tools/code_execution_tool.py` | ~77KB | 零上下文成本管线(PTC / RPC stub) |
| `gateway/platforms/base.py` | ~5400 LoC | `BasePlatformAdapter` ABC,吸收所有跨平台难点 |
| `tools/skills_hub.py` | 4069 LoC | 多源 skill 发布/同步/安装管线 |
| `gateway/slash_commands.py` | 4185 LoC | 网关斜杠命令 mixin |
| `run_agent.py` | ~5700 LoC | `AIAgent` 门面(~60 参数构造 + 大量转发方法) |

### 定义 Hermes 身份的几个子系统

- **MoA(Mixture-of-Agents,`agent/moa_loop.py`)**:`/moa` 把某回合标为 MoA(不是工具)。`aggregate_moa_context` 用线程池(上限 8)把多个参考模型并行扇出到一个"顾问视图"(丢掉 8K 系统提示、把 tool_calls 渲染成文本、为 Anthropic 强制末尾 user 回合),聚合器综合后把结论追加到最后一条 user 消息作私有指导。它与主循环组合而非替换。
- **自进化闭环(核心差异化)**:四个协作机制 + 一个 provenance 开关。`agent/background_review.py` 在每 ~10 回合后 fork 一个子代理,把刚结束的会话蒸馏成 skill 补丁/新建与 memory 写入;`agent/curator.py` 以 7 天节律管理 agent 自建 skill 的 active→stale→archived 生命周期(可选 LLM "umbrella" 合并);枢纽是 `tools/skill_provenance.py` 的 ContextVar——只有在 maintenance fork 内创建的 skill 才被标 `created_by:"agent"` 并纳入 Curator 管理,前台/`/learn` 创建的归用户所有、Curator 永不触碰。
- **Skills 程序性记忆(`tools/skills_*`)**:agentskills.io 兼容的 `SKILL.md`,渐进式披露(仅 name+description 进系统提示),`skill_manage` 是 agent 的写入器,四类 skill(bundled/optional/user/plugin)共存于 `~/.hermes/skills/`,Hub 有 ~10 个注册源 + quarantine→扫描→信任矩阵→安装的安全管线。
- **Memory(`tools/memory_tool.py` + `agent/memory_manager.py`)**:`MEMORY.md`(~2200 字符)/`USER.md`(~1375 字符),§ 分段、原子写、注入扫描,可插外部 provider(`MemoryProvider` ABC,如 Honcho 多轮辩证式用户建模)。
- **6 种终端后端(`tools/environments/`)**:统一 `BaseEnvironment` ABC 下的 `local/docker/ssh/singularity/modal(直连 + Nous 托管)/daytona`,由 `TERMINAL_ENV` 选择;Modal/Daytona 提供无服务器文件系统持久化,SSH/Modal/Daytona 用 `FileSyncManager`(mtime+SHA-256)同步。
- **IM 网关 20+ 平台**:插件平台(Telegram/Discord/Slack/WhatsApp/Matrix/Mattermost/Teams/LINE/IRC/飞书/钉钉/企业微信/Email/SMS/Google Chat/Home Assistant/ntfy/SimpleX/iMessage/Raft)+ 内置适配器(Signal/WhatsApp Cloud/微信/元宝/BlueBubbles/QQBot/MS-Graph/Webhook/API server),一个 `BasePlatformAdapter` 吸收并发/中断/去抖/流式/媒体/TTS-STT 等横切逻辑,新平台可 <1000 LoC 上线(IRC 是纯 stdlib 范例)。
- **Cron(`cron/jobs.py` + `cron/scheduler.py`)**:JSON 文件存储 + 文件锁 60s tick,自然语言/`every X`/5 字段 cron/ISO 调度,隔离会话执行,跨平台投递;`blueprint_catalog.py`/`suggestions.py` 提供同意优先的参数化自动化模板。

---

### 与 coco-rs 的架构范式差异

两者都是 "Claude Code 级" Agent,但**工程范式几乎处于两个极端**:Hermes 是**带插件的 Python 单进程巨石**,coco-rs 是**分层的 Rust Cargo 工作区(~96 个 member crate,按工作区 `Cargo.toml` 实际计数)**。这不是风格偏好,而是深刻影响了正确性保证的位置。

| 维度 | Hermes(Python 巨石 + 插件) | coco-rs(分层 Rust 工作区) |
|------|------------------------------|-----------------------------|
| **依赖纪律** | 运行时约定:靠导入顺序(`registry ← tools ← model_tools`)、AST 自动发现、`register(ctx)` 动态注册维持;分层无编译期强制 | crate 级 DAG,下层不依赖上层;`scripts/check-tui-ui-seam.sh` 等脚本 + Cargo 强制;`coco-memory` 只依赖 `coco-tool-runtime` trait,不碰 `coco-messages` |
| **类型安全** | OpenAI 风格 `dict` 消息 + `role` 字符串;`api_mode == "..."` 字符串分派散落多文件;`getattr(agent,'_x',default)` 哨兵充斥 | 封闭枚举 over 字符串(`ToolName`/`ModelRole`/`ProviderApi`);newtype(`RedactedSecret`/`PositiveTokens`);"闭集不得硬编码字符串"为硬规则 |
| **错误处理** | 到处 `except Exception: pass` 的尽力而为(usage bump、插件钩子、MCP 刷新、验证),失败静默降级 | 三层错误策略:主干 `snafu+coco-error`、边界库 `thiserror`、入口 `anyhow`,由 `just check-error-policy` 强制 |
| **状态封装** | `AIAgent` 上帝对象:~60 参数构造 + 数十个跨 `turn_context/loop/executor/finalizer` 设置的可变 `_下划线`属性,不变量隐式 | `AppState` 树 + `Arc<Mutex/RwLock>` 注册表;回调句柄 trait(`AgentHandle`/`TaskHandle`)打破工具→子系统环 |
| **并发模型** | 线程为主(`ThreadPoolExecutor`、daemon 线程、GIL);回合期间占用一个线程;per-turn 状态用 `contextvars` 隔离(修过 `os.environ` 跨回合污染的 bug) | `tokio` 异步;`CancellationToken` 贯穿各层;安全工具并发、不安全工具排队 |
| **事件系统** | 网关侧有类型化 `StreamEvent`(`stream_events.py`)但整体分散;插件/记忆/检索各有独立回调路径 | 单一 `CoreEvent` 枚举 + 3 个分派层(Protocol/Stream/Tui),发一次消费方按层挑 |
| **配置解析** | `DEFAULT_CONFIG`(`_config_version 32`)深合并 YAML;各适配器散落 `os.getenv`,env>YAML 靠 `not os.getenv()` 临时守卫 | 一次折叠成 `Arc<RuntimeConfig>` 快照;叶子 crate 只读解析后的子配置,禁止 `std::env::var` 特设调用;`Feature` 粗粒度能力门 |
| **多 provider 边界** | `ProviderProfile` 插件 + `api_mode` 字符串选适配器 + `agent/transports/` 注册表;provider 特例(1M-beta、llama.cpp 语法、MiniMax stall)**内联在共享循环里** | 三层边界,provider 关注点(auth/beta 头/cache 断点)**隔离在 `vercel-ai-*` crate**,`services/inference` 保持通用 |
| **可维护性** | 超大单文件(`gateway/run.py` 921KB、`kanban_db.py` >8000 LoC),`AGENTS.md` 自承需拆解 | 模块 <800 LoC 目标,>1600 LoC 就拆;每 crate 有自己的 `CLAUDE.md` |

**对 coco-rs 工程师的解读要点**:

1. **正确性保证的位置根本不同**。Hermes 把大量不变量(prompt-cache 字节稳定、per-turn 属性契约、provenance 分类)靠**注释、约定、运行时守卫**维持——它甚至用下划线前缀的 `_compressed_summary` 元数据键,专门让线缆净化器在严格 OpenAI 网关拒绝前剥掉它。coco-rs 把等价保证下沉到**类型系统与 crate 边界**(如 `ProviderClientFingerprint` 在回合边界做热重载相干性)。Hermes 的做法迭代快、单文件即可读全貌,但重构极易踩到"陈旧内存模块"式的隐蔽失效(其代码注释里点名了 issues #38727/#25322/#14944)。

2. **相同问题的两种解法值得互相借鉴**。二者都极度重视 prompt cache:Hermes 靠"系统提示每会话构建一次 + 日期级时间戳 + 所有易变上下文注入 user 消息";coco-rs 靠 fork 模式的 `FORK_PLACEHOLDER` tool_result 改写 + `CacheBreakDetector`。Hermes 的 **PTC(`execute_code` 写 Python 调工具、只有 stdout 回上下文)**、**MoA 顾问扇出**、**Curator provenance 开关**是 coco-rs 目前完全没有的能力维度;反过来,coco-rs 的**分层错误分级、封闭枚举、单 `CoreEvent`、~96-crate 依赖 DAG** 正是 Hermes 巨石在规模化后最缺的工程护栏。

3. **能力广度 vs 结构纪律的权衡**。Hermes 用"巨石 + 动态插件"换来了 6 种终端后端和 20+ IM 平台的广度(其 `BasePlatformAdapter`/`ProviderProfile`/`SkillSource` 都是 ABC+注册表模式),而 coco-rs 用严格分层换来可编译验证的正确性,但执行后端只有 local+remote、无 IM 网关。这在后续各专题对比中会反复出现:**Hermes 更"宽"且更"自进化",coco-rs 更"稳"且更"可证"**。

---

> ← [返回索引](README.md) · 下一篇：[功能对比矩阵 →](02-feature-comparison.md)
