# 功能对比矩阵

> [← Hermes 架构总览](01-hermes-architecture.md) · [返回索引](README.md) · [自进化深剖 →](03-self-evolution-deep-dive.md)

---

# 功能对比矩阵

下表按能力维度逐项对比 Hermes(`/lyz/codespace/3rd/hermes-agent`,Python 单进程 agent + IM gateway)与 coco-rs(`/lyz/codespace/cocode/coco-rs`,Rust 分层 workspace,Claude Code 移植)。图例:✅ 完整实现 · ⚠️ 部分/存根/受限 · ❌ 无 · ❓ 本次调研未确认。「领先方」列基于 findings 证据给出判断。

## A. 核心 Agent 运行时

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| Agent 主循环 | ✅ | ✅ | Hermes 是同步单线程巨型循环 `agent/conversation_loop.py`(~5000 LoC 单 try 体),`AIAgent` 为 ~5700 LoC facade + ~60 参数构造;coco-rs 是 async 的 `QueryEngine`,拆成 ~30 个 `engine_*.rs` 模块。**架构清晰度 coco 领先**,功能覆盖相当。 |
| 迭代预算 / 续跑熔断 | ✅ | ⚠️ | Hermes `IterationBudget` 是**真正的两级**(父 agent 90 / 子 agent 50 各自独立的迭代上限)+ 一次性 `_budget_grace_call` + `execute_code` 退款;coco `BudgetTracker` 是**续跑次数熔断**(最多 3 次续跑、单次 <500 tokens 即停、达 90% 预算停),并非按父/子分级——是否给子 agent 独立迭代上限未获证实。**机制不同,勿等同**。 |
| 中途 steering / 打断 | ✅ | ✅ | Hermes `agent.steer()` 注入到最后 tool 消息 + 线程作用域打断;coco 优先级 `CommandQueue`(Now>Next>Later)+ 三态 `QueryGuard`。coco 的队列 + 世代计数更结构化。 |
| Provider 特定错误恢复矩阵 | ✅ | ✅ | Hermes `TurnRetryState` 把 ~16 个一次性恢复 guard 收敛成一个 dataclass(OAuth 刷新、429 池、格式恢复、1M-beta 等);coco `retry.rs` 双层(退避 + auth 感知刷新)。Hermes 内联了大量 provider 分支(耦合更重),coco 把 provider 关注点隔离在 `vercel-ai-*`。**分层 coco 领先**。 |
| 多 Provider 家族 | ✅ | ✅ | Hermes:`api_mode` + `ProviderProfile` 插件发现 + 适配器(anthropic/gemini/**bedrock**/codex/azure),models.dev 元数据,覆盖面更广。coco:6 家(Anthropic、OpenAI Chat+Responses、Gemini、Google Code Assist、OpenAI-compatible、ByteDance),**显式拒绝 Anthropic 云凭据路由(Bedrock/Vertex)为非目标**。**广度 Hermes 略领先,边界纪律 coco 领先**。 |
| Prompt caching 纪律 | ✅ | ✅ | 两者都把「system prompt 一次构建、逐字节复用、日期级时间戳、volatile 内容注入 user 消息」作为一等不变量。Hermes fork 复用父缓存前缀(实测 ~26% 成本下降);coco `CacheBreakDetector`(16 维哈希、>5% 且 >2000 token 掉落告警)提供可观测性。**大致持平**。 |
| 上下文压缩策略数 | ⚠️ | ✅ | Hermes:head/tail/middle + LLM 结构化摘要 + tool-result 剪枝,单一路径但摘要 prompt 极度打磨(`SUMMARY_PREFIX`/`_SUMMARY_END_MARKER` 编码大量真实 bug 修复)。coco:**6 个独立策略模块**(`compact`(full) / `micro` / `micro_advanced` / **API-native 服务端 context-editing** `api_compact` / `reactive` / `session_memory`,另有 `staged`)+ observer registry + 双熔断器。**策略广度 coco 领先**(尤其 Anthropic 服务端 `clear_tool_uses`/`clear_thinking` 保留缓存);**摘要 prompt 工程 Hermes 领先**。 |
| 压缩失败处理 | ✅ | ✅ | Hermes:auth/网络错误直接中止 + 冷却 600s + 防抖 + 静态回退;coco:`ReactiveCompactState` 3 次失败熔断 + rapid-refill 熔断。均成熟。 |
| MCP | ✅ | ✅ | Hermes:MCP server 注册进 `mcp-<server>` toolset + `notifications/tools/list_changed` 动态刷新;coco:专用 `coco-mcp` + `coco-mcp-types` + `rmcp-client`(stdio/HTTP-SSE、OAuth 持久化)。coco 的 IDE 集成也走 MCP。**crate 化程度 coco 领先**,功能相当。 |

## B. 工具、动态加载与执行/沙箱

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| 工具注册表 + toolset | ✅ | ✅ | Hermes:模块级 `ToolRegistry` 单例 + AST 自动发现 + 可组合 `TOOLSETS`(`_HERMES_CORE_TOOLS` 共享核心);coco:`ToolRegistry` + `Tool` trait + 五层过滤管线。均成熟。 |
| 动态工具加载(渐进披露) | ✅ | ✅ | Hermes:`tool_search`/`tool_describe`/`tool_call` 三桥接工具 + 内联**客户端 BM25**,把 MCP/plugin 大工具数组挡在 prompt 外(auto=上下文 10%)。coco:一等 `ToolSearch`(`core/tools/src/tools/tool_search.rs`,`Feature::ToolSearch` **默认开启**,`deferred_tools` 发现 + `DeferredToolsDelta` promotion + `ToolSearchUsageReminder`,设计见 `tool-search-design.md`),**并且有 provider-native 变体**——`Capability::OpenAiNativeToolSearch`、Anthropic `tool_search` beta、`ClientSideToolSearchPromotion`(跨发现保 prompt 缓存前缀),这是 Hermes 纯客户端 BM25 所没有的维度。**大致持平,coco 在 provider-native 集成上反而领先**。 |
| 零上下文成本管道(execute_code / PTC) | ✅ | ❌ | Hermes `execute_code`:agent 写 Python 脚本经 RPC(本地 UDS / 远端文件轮询)回调工具,N 次工具往返压成 1 轮,仅 stdout 回上下文。coco findings **未见**等价能力。**Hermes 独有**。 |
| 终端/执行后端数 | ✅ | ⚠️ | Hermes **6 个后端**(local/docker/ssh/singularity/modal(直连+Nous 托管)/daytona),统一 `BaseEnvironment` ABC,Modal/Daytona 无服务器文件系统持久化 + `FileSyncManager`(mtime+SHA-256)。coco **仅 local + remote**(codex 派生的 exec-server,loopback WebSocket),**无 docker/ssh/serverless/环境注册表**。**Hermes 大幅领先**。 |
| 沙箱隔离 | ✅ | ⚠️ | Hermes:通过后端做容器/VM 隔离(docker cap-drop ALL、singularity overlay、modal microVM)。coco:`exec/sandbox` OS 级三模式(None/ReadOnly/Strict,bubblewrap/sandbox-exec/seccomp),**默认关闭、Strict 仅 Linux/macOS、非容器/VM**。**不可信代码隔离 Hermes 更强**;coco 有更正式的模式抽象但强度弱。 |
| Checkpoints(文件快照回滚) | ✅ | ✅ | Hermes:透明(对 LLM 不可见)每回合 shadow-git 快照,共享 bare store + 对象去重 + 回滚(**仅代码**)。coco:`core/context/src/file_history.rs`「per-turn snapshots + content-addressed(SHA-256)backups」(`MAX_SNAPSHOTS=100`),`FileHistorySnapshotSink` 持久化进 session JSONL 供 resume 重放;`app/tui/src/state/rewind.rs` 是**四相 rewind 状态机**,`RestoreType` 支持 `Both`(代码 file-history + 会话截断)/`CodeOnly`/`ConversationOnly`/`SummarizeFrom`/`SummarizeUpTo`。**coco 在会话回溯维度反而更强**——Hermes shadow-git 只回滚代码,coco 同时回滚代码 + 会话 + Summarize 变体。 |
| Worktree 隔离 | ⚠️ | ✅ | Hermes:kanban 任务 pin `WORKSPACE` 但非 git worktree;coco:`Feature::Worktree` + `EnterWorktree`/`ExitWorktree` 子 agent 隔离。**coco 领先**。 |

## C. 多智能体、委派与编排

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| 子 agent 委派 | ✅ | ✅ | Hermes:`delegate_task`(单 `goal`/批量 `tasks[]`)、leaf vs orchestrator、同步(ThreadPoolExecutor)/`background=true` 异步经 `completion_queue` 回灌、子集 toolset(只窄化不放宽)。coco:`AgentTool` + **Fresh/Fork/Resume 三种 spawn 模式**、五层工具过滤、`SUBAGENT_DEPTH_LIMIT=5`。**均强**;coco Fork 模式共享父 prompt 缓存(`FORK_PLACEHOLDER`)是真实成本优化。 |
| 持久看板 / Kanban 工作队列 | ✅ | ✅ | Hermes:SQLite board + gateway 内 ~60s dispatcher tick(回收陈旧 claim、`recompute_ready`、原子 claim、`hermes -p <profile>` 子进程),board 硬边界 / tenant 软命名空间,失败上限自动 block。coco:`coco-tasks::task_list` 磁盘存储 + fs2 文件锁 + `.highwatermark` + 原子 `claim_task` + `blocked_by` 依赖解析,team 共享。**均成熟,设计各有侧重**。 |
| 长期具名 teammate / swarm | ⚠️ | ✅ | Hermes 以 kanban 子进程调度实现多 agent,但无「长期具名 teammate + 终端 pane」概念。coco:`coco-coordinator`(v2,`Feature::AgentTeams` Experimental)+ tmux/iTerm2/in-process 后端 + 文件权限 mailbox + coordinator-mode prompt 切换 + handoff 安全分类器。**coco 领先(但仅 Experimental)**。 |
| 动态 Workflow 编排(脚本化) | ❌ | ⚠️ | Hermes 无 JS/脚本工作流引擎(有 MoA、blueprints 作为编排替代)。coco:`core/workflow` 前端解析器(tree-sitter 解析 JS、拒 TS)已在,但 **`WorkflowTool::execute` 是诚实存根返回 "not available"**,rquickjs 执行引擎/determinism shim/journal 仅为计划(`Feature::Workflow` UnderDevelopment)。**两者都不可用;coco 有半成品脚手架**。 |
| Mixture-of-Agents(MoA) | ✅ | ❌ | Hermes:`/moa` 每回合并行 fan-out 参考模型(ThreadPoolExecutor,cap 8)+ aggregator 综合,注入为私有指导;有 acting-aggregator facade + 每回合签名缓存。coco findings **未见**。**Hermes 独有**。 |

## D. 技能、学习闭环与记忆(核心差异区)

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| Skills 系统(过程记忆) | ✅ | ✅ | 两者都是 agentskills.io 兼容 SKILL.md + 渐进披露 + slash/工具双入口 + Inline/Fork。Hermes 单根 `~/.hermes/skills/` 四类共存(bundled/optional/user/plugin);coco 六来源 + 源优先级 + 1% 上下文预算。功能相当。 |
| **Skills Hub / 生态分发** | ✅ | ❌ | Hermes:`tools/skills_hub.py`(4069 LoC)聚合 ~10 个 registry(skills.sh/GitHub taps/ClawHub/Claude Marketplace/LobeHub/browse.sh)+ quarantine→scan→policy-gate→install 流水线 + PR 发布 + 信任矩阵。coco **无 Hub/市场**。**Hermes 大幅领先**。 |
| **技能自动创建** | ✅ | ❌ | Hermes:每 ~10 回合 `_spawn_background_review` fork 一个受限 agent,`_SKILL_REVIEW_PROMPT` 明确「be ACTIVE — 多数会话都应产出一次技能更新」,`skill_manage(create)` 自动建技能。coco:**仅手动** `/skillify`、`/run-skill-generator`(UserType/Feature 门控、AskUserQuestion 访谈、用户确认后才写),grep 证实**零自主调用点**。**Hermes 决定性领先**。 |
| **技能自我改进(学习闭环)** | ✅ | ❌ | Hermes:background review 用 `skill_manage(patch/edit)` 变异技能 + **Curator**(默认 7 天周期)确定性老化 active→stale→archived + 可选 LLM「umbrella 合并」。coco:**自动闭环只作用于知识(memory),从不改技能/工具/prompt/策略**;`remember` 技能仅提议、不自动应用。**Hermes 决定性领先——这是两系统最大分野**。 |
| Provenance(agent-created vs user-owned) | ✅ | ⚠️ | Hermes:`skill_provenance.py` ContextVar 写来源开关,只有 background_review fork 内 create 才标 `created_by:agent` 并归 Curator 管理,用户技能永久免疫。coco:靠内存写围栏(memdir fence)隔离自动写,但技能无此二分(因无自动建技能)。**Hermes 领先**。 |
| 持久跨会话记忆 | ✅ | ✅ | Hermes:`MEMORY.md`(~2200 字)/`USER.md`(~1375 字)§ 分段 + 外部 provider ABC。coco:`coco-memory` 四类 taxonomy(User/Feedback/Project/Reference)markdown 文件 + 模型策展 `MEMORY.md` 索引。均成熟;coco 层次纪律更清(不依赖 `coco-messages`/`coco-inference`)。 |
| 自动抽取(turn-end 捕获) | ✅ | ✅ | Hermes:review fork 写记忆;coco:`ExtractService` 每合格回合 fork memdir 围栏子 agent(游标推进仅成功时、失败退避至 32x)。均有强围栏(coco 双环 write fence + 只读 Bash 白名单 + .md-only)。 |
| 记忆整合(dream/curator) | ✅ | ✅ | Hermes:Curator + review 承担整合;coco:`DreamService`(24h/5-session/10min 三门 + PID+mtime CAS 锁 + 4 阶段 Orient/Gather/Consolidate/Prune,可 `rm` .md)+ KAIROS 日志模式。均成熟(**注:coco KAIROS 承诺的「夜间蒸馏日志→MEMORY.md」实际未接线**)。 |
| 跨会话全文检索(session-search) | ✅ | ⚠️ | Hermes:`hermes_state.SessionDB` FTS5(BM25+trigram+自愈)+ `session_search` 工具(Discovery/Scroll/Read/Browse + 锚定窗口 + cron 行降权 + 跨 profile)+ 辅助 LLM 标题生成。coco:有 LLM 排序 recall(从记忆文件,≤5 个 + 60KB 预算)+ 本地只读 session hub 查看器,但**无消息级 FTS 搜索工具**。**Hermes 领先**。 |
| 结构化用户建模 | ✅ | ❌ | Hermes:Honcho 辩证用户模型(多轮 `.chat()`:who-is-this-person→gap 综合→矛盾调和 + 早停 + 每 pass 推理档位),经 MemoryProvider ABC 注入。coco:**仅自由格式 `[user]` 记忆类型**当普通文件写,无偏好推断/persona 引擎。**Hermes 领先**。 |
| 会话内反思(Learnings/Errors) | ✅ | ✅ | Hermes:session 摘要含 Learnings/Errors&Corrections(但为会话内);coco:`SessionMemoryService` 9 段摘要含 `# Learnings` 与 `# Errors & Corrections`,供压缩/`--resume` 恢复。均为会话内上下文恢复,**非**跨会话行为学习。 |
| 结果/telemetry 驱动的行为适配 | ⚠️ | ❌ | Hermes:Curator 基于 `.usage.json`(use/view/patch_count、last_used_at)做**生命周期**决策(老化/归档),但技能内容变异仍靠 LLM 判断;无 reward/eval 信号。coco:`MemoryEvent` 仅发往 OTel,**从不回灌**改技能或写策略。**Hermes 部分领先**(有 usage-driven 生命周期)。 |

## E. 外部连接、交互与调度

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| **IM / 消息平台** | ✅ | ❌ | Hermes:`gateway/` 单进程接入 **20+ 平台**(Telegram/Discord/Slack/WhatsApp/Signal/Feishu/微信/QQ/Matrix/Teams/…),统一 `BasePlatformAdapter`(5400 LoC)+ 插件注册表 + 动态 `Platform` 枚举 + relay + scale-to-zero。coco:**全工作区 grep 零 IM 集成代码**,唯一路径是用户自配通用 MCP server。**Hermes 压倒性领先——最尖锐的对比点**。 |
| DM 配对 / 分层授权 | ✅ | ⚠️ | Hermes:`pairing.py`(8 位哈希码、恒定时比较、限速+锁定、0600 原子写)+ 多层 authz(群白名单/角色/pairing/upstream-trust/全局)。coco:`permissions` crate(2 阶段 auto-mode/yolo XML 分类器 + bypass killswitch),但面向工具权限而非 IM 用户。**场景不同,Hermes 面向 IM 领先**。 |
| 语音(STT/TTS / voice mode) | ✅ | ❌ | Hermes:入站语音经 Whisper STT 转写,`/voice` 切换自动 TTS,原生语音气泡/附件送出。coco findings **未见**语音能力。**Hermes 独有**。 |
| Cron / 定时任务 | ✅ | ✅ | Hermes:JSON 文件 job store + 60s flock tick + NL 调度 + 跨平台投递 + blueprints/suggestions + `no_agent` 纯脚本 job。coco:`utils/coco-cron`(5 字段子集)+ `ScheduleStore` + 1s tick → `CommandQueue`(`QueueOrigin::Cron`),`MAX_CRON_JOBS=50`。**但 coco cron 仅 TUI 交互态生效**(headless/SDK 无 drain pump),RemoteTrigger 为存根。**Hermes 领先**(投递到 IM、blueprints、成熟度更高)。 |
| TUI | ✅ | ✅ | Hermes:双进程(TS Ink/React 前端 + Python JSON-RPC 后端,contextvar 绑定 transport),同一 gateway 复用于 CLI/web/desktop。coco:纯 Rust TEA(`coco-tui` + `coco-tui-ui`),**codex 级原生 scrollback 无闪烁 paint engine**(BSU/ESU 同步更新 + cell-diff,seam 由脚本强制)。**paint-engine 精细度 coco 领先**;复用性 Hermes 领先。 |
| Desktop / Web 应用 | ✅ | ⚠️ | Hermes:web dashboard + desktop app,经 PTY 复用 gateway。coco:仅 **Local Session Hub**(独立 `coco-hub-server`、只读浏览器 transcript 查看器、**未嵌入主 `coco` 二进制、无 `--serve-hub`**);Event Hub 的 WS 出口/SQLite 摄取设计**未建**(`hub/connector` 是一行 re-export 骨架)。**Hermes 领先**。 |
| IDE bridge / SDK | ✅ | ⚠️ | Hermes:IDE 经 MCP 集成 + gateway。coco:`coco sdk`(JSON-RPC NDJSON,**仅 StdioTransport**,SSE/WS 未实现)+ `coco-bridge`(权限中继原语,但 `BridgeServer` 是 channel 骨架,云会话 API 故意不实现);IDE 亦走 MCP。功能相近,coco transport 面偏窄。 |
| 配置 / profiles | ✅ | ✅ | Hermes:`config.yaml` + `.env` 分离,`DEFAULT_CONFIG` 深合并(`_config_version` 32),`HERMES_HOME` profile 隔离。coco:分层 `RuntimeConfig` 单点合并(settings.json→env→overrides)+ `SettingsWatcher` 热重载。**热重载 coco 领先**;profile 隔离 Hermes 更显式。 |

## F. 其他工程维度

| 维度 | Hermes | coco-rs | 说明 / 领先方 |
|------|:------:|:-------:|------|
| 研究 / 轨迹工具(训练向) | ✅ | ❌ | Hermes:`trajectory_compressor.py`(ShareGPT 轨迹按 token 预算压缩,供训练)+ `batch_runner.py`(多进程并行生成 + checkpoint/resume + toolset 分布采样)。coco 为产品向 agent,**无训练轨迹工具**。**Hermes 独有(定位差异)**。 |
| LSP 集成 | ❓ | ✅ | coco:`coco-lsp`(按名+种类查询而非位置,rust-analyzer/gopls/pyright/tsserver)。Hermes findings **未提及**专用 LSP。**coco 领先**。 |
| 代码检索(BM25/vector/RepoMap) | ⚠️ | ✅ | Hermes:有 session FTS5,但无代码检索 facade。coco:`coco-retrieval`(BM25 + vector + AST + RepoMap PageRank,`RetrievalFacade`,隔离 `RetrievalEvent` 流)。**coco 领先**。 |
| 权限 / 安全模型 | ✅ | ✅ | Hermes:`skills_guard`(regex+结构化 verdict × 信任矩阵)+ 独立 opt-in AST 审计(诚实标注「非安全边界」)+ IM pairing。coco:`permissions`(评估器 + 2 阶段分类器 + DenialTracker + killswitch)+ hooks SSRF guard + secret-redact。**侧重不同**:Hermes 强在技能安装/IM 授权,coco 强在工具执行分类。 |
| 计划态机 / 计划执行校验 | ❓ | ✅ | coco:`VerifyPlanExecution` 工具(`core/tools/src/tools/verify_plan_execution.rs`)+ Plan Mode 完整生命周期 + Pewter-ledger Phase-4 变体(`null`/`trim`/`cut`/`cap`)+ Interview 阶段(见 `plan-mode-architecture.md`)。Hermes findings **未见**等价的显式计划态机(有 blueprints 作近似)。**coco 领先**。 |
| 模糊补丁应用 | ⚠️ | ✅ | coco:独立 `exec/apply-patch` crate(统一 diff/patch + 模糊匹配应用)。Hermes 走 `tools/patch_parser.py`,但无独立模糊补丁子系统。**coco 工程化领先**。 |
| 密钥脱敏 / SSRF 守卫 | ⚠️ | ✅ | coco:`utils/secret-redact`(OpenAI/Anthropic/GitHub/Slack/AWS/bearer token 集中脱敏)+ hooks SSRF guard。Hermes 有 `agent/redact.py`/`tools/url_safety.py` 等分散实现。**coco 的 crate 化脱敏更系统**。 |

## 结论摘要

**Hermes 领先的核心在于「自主学习闭环 + 面向消费者的连接层」。** 它是唯一具备真正闭环自我进化的一方:每 ~10 回合自动 fork 后台 review,把刚结束的会话蒸馏为技能补丁/新技能与记忆写入,再由 Curator 按周期做确定性老化与 LLM「umbrella 合并」——技能自动创建、技能自我改进、Skills Hub 生态、结构化用户建模(Honcho 辩证)在 coco-rs 中**全部缺席**。同样压倒性的是 IM gateway:20+ 聊天平台、统一适配器抽象、DM 配对授权、语音 STT/TTS,coco-rs 则是**零 IM 集成**。在执行后端(6 个 vs local+remote)、`execute_code` 零上下文管道、MoA、训练轨迹工具上 Hermes 也明显更全。

**coco-rs 领先的核心在于「工程分层纪律 + 运行时质量」。** 同样的能力,coco-rs 用清晰的 crate 分层与 callback-handle trait seam 实现,provider 关注点严格隔离在 `vercel-ai-*`,压缩服务对 provider 完全无感;上下文压缩更丰富(6 个策略模块含 Anthropic 服务端 context-editing,保留缓存);TUI 是 codex 级原生 scrollback 无闪烁 paint engine。更关键的是几项经代码核实、Hermes 反而不及或更分散的能力:**checkpoint/rewind**(coco 四相 rewind 同时回溯代码+会话+Summarize,Hermes shadow-git 只回代码)、**provider-native ToolSearch**(`OpenAiNativeToolSearch`/Anthropic beta,Hermes 仅客户端 BM25)、**Plan Mode + `VerifyPlanExecution` 计划态机**,以及 LSP、代码检索(BM25/vector/RepoMap)、git worktree 隔离、配置热重载、`apply-patch` 模糊补丁、`secret-redact`/SSRF 守卫。它的子 agent 委派与看板任务系统也已「架构完整、与 TS 高度对齐」。

**两者在若干维度大致持平但路径不同。** Agent 主循环、多 provider、prompt caching、MCP、跨会话记忆的自动抽取/整合上,双方都成熟;差别更多在实现风格——Hermes 是重度打磨但耦合的巨型 Python 模块(`conversation_loop.py` 284KB、`run.py` 19k LoC),把 provider 恢复逻辑内联进共享循环;coco-rs 把同样的关注点拆散到模块与 crate,可维护性更好但部分高级特性仍为「已铺设但未激活」(marble_origami ledger 零生产调用、prompt-role layout 尚非全量路径)。

**关于不确定项的核实说明(重要)。** 初稿曾因「findings 沉默」把 coco 的**动态工具加载**标为 ⚠️ 存疑、把 **checkpoints** 标为 ❓ 未确认——经直接 grep coco 代码库,二者均为 coco 一等能力,已**上修为 ✅ 并在上表更正**:ToolSearch 见 `core/tools/src/tools/tool_search.rs` + `tool-search-design.md` + provider-native `Capability`;checkpoint/rewind 见 `core/context/src/file_history.rs` + `app/tui/src/state/rewind.rs`。这两处原本都朝「高估 Hermes 领先」偏移,更正后天平回正。仍存的不确定项:Hermes 是否有专用 **LSP** 集成 findings 未提及(标 ❓)。原则:凡涉及 coco「缺失」的判定,均以代码 grep 为准而非仅凭 findings 沉默。

**选型启示。** 若目标是「能自己越用越强、能从微信/Telegram 等 IM 直接触达、支持语音、可在 docker/ssh/serverless 多后端跑不可信代码」的个人助理型 agent,Hermes 的能力面无可替代;若目标是「多 provider 可移植、上下文管理精细、TUI/检索/LSP 工程质量高、代码库整洁可长期演进」的编码 agent,coco-rs 是更稳的地基——而它最值得从 Hermes 借鉴的,正是那条把「知识记忆闭环」扩展到「能力(技能/prompt/策略)闭环」的 Curator 式自主学习回路。

---

> [← Hermes 架构总览](01-hermes-architecture.md) · [返回索引](README.md) · [自进化深剖 →](03-self-evolution-deep-dive.md)
