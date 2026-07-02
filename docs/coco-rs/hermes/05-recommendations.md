# 吸收 Hermes 优点：可落地建议

> [← IM 接入深剖](04-im-integration-deep-dive.md) · [返回索引](README.md)

---

# 吸收 Hermes 优点：可落地建议

Hermes 与 coco-rs 是两种截然不同的工程取向：Hermes 是一个单进程 Python "生活助理"，其杀手锏是**自主闭环学习（Curator）**与**多平台 IM 网关**；coco-rs 是分层 Rust 工作区，在**编译期类型安全、多 provider 边界、原生滚屏 TUI、compaction 策略**上明显更成熟。下面的路线图只挑选 coco-rs *真正缺失且有产品价值* 的 Hermes 能力，并严格按 coco-rs 分层约定（`Feature` 门控、`ModelRole` 复用、错误分层、库层禁 `anyhow`、typed-over-`Value`、`COCO_*` 前缀）给出落地方案。

指导原则：**优先复用 coco-rs 已有的"半成品座位"**。findings 反复显示 coco-rs 很多子系统的 trait seam / 调度钩子已经就位但空转——`hub/connector` 是一行 re-export 骨架、`WorkflowTool::execute` 是 stub、cron tick 只在 TUI 跑、exec-server 只有 `local`/`remote` 两个 environment。吸收 Hermes 优点在很多情况下等于"把已经画好的座位坐满"，而不是从零起。

---

## Tier 1 — 高价值、可直接开工

### T1.1 技能自主创建 + 自我改进闭环（Curator loop）

**Hermes 做法**：每约 10 轮（`agent/turn_finalizer.py` 的 `_iters_since_skill`/`_turns_since_memory` nudge 计数器）在回复*之后*fork 一个后台 review agent（`agent/background_review.py`），跑 `_SKILL_REVIEW_PROMPT`（"be ACTIVE — 大多数会话都应产出一次技能更新"），按 `patch 已加载技能 > patch umbrella > 加 support file > 新建 umbrella` 的优先级，用 `skill_manage(patch|create)` 把本次会话固化为可复用技能。fork 继承父进程 runtime 并 pin 住父级缓存 system prompt + `session_id` 以命中前缀缓存（实测降本约 26%）。写来源由 ContextVar `skill_write_origin`（`tools/skill_provenance.py`）区分：只有 background_review fork 内创建的技能被标 `created_by:"agent"`，交给 Curator（`agent/curator.py`）按 7 天节奏做确定性老化（active→stale→archived，`.archive/`，从不删除、pinned 豁免）+ 可选 LLM umbrella 合并。

**coco-rs 差距**：闭环只覆盖**知识**（extract→dream→recall）。**能力**（skills/tools/prompts）永远不会被自动改写。`skillify` / `run-skill-generator` 是纯手工 human-in-the-loop 斜杠命令，grep 证实**零自主触发点**（`skills/src/bundled.rs`）。

**落地方案（crate 归属）**：
- **复用现成的 Fork 基础设施**。coco-rs 的 subagent Fork spawn mode（`core/subagent/src/fork.rs` 的 `build_fork_context` / `FORK_PLACEHOLDER` / `CacheSafeParams`）**已经在做 Hermes 那套 pin 父缓存 system prompt + 重写 tool_result 的前缀缓存共享**——这正是 background review 需要的。
- **在 `root/memory` 挂新服务**。`MemoryRuntime` 已有 turn-end scheduler（`memory/src/runtime.rs::finalize_turn`，由 `app/query::engine_finalize_turn` 每轮驱动）并已有 `ExtractService`/`DreamService` 两个"fork 子 agent 做后台蒸馏"的范式。新增 `SkillReviewService`（对齐 `extract.rs` 结构）+ `CuratorService`（对齐 `dream.rs` 的 gate + PID/mtime CAS 锁 `memory/src/lock.rs`）。nudge 计数器直接加在 `MemoryRuntime`，回复投递后再触发（不与主模型抢注意力）。
- **provenance 改进 Hermes 而非照抄**。coco-rs 惯例是显式传参而非进程全局；把 Hermes 的 ContextVar 换成在 fork 的 `AgentSpawnConstraints`/`ToolUseContext` 里带一个 `SkillWriteOrigin::BackgroundReview` 字段，技能写工具据此打 `created_by`。同时复用 coco-memory 的**两环写栅栏**（`can_use_tool` 回调内环 + `allowed_write_roots` 外环，`memory/src/can_use_tool.rs`）把 review fork 锁死在 skills 目录。
- **确定性老化是纯逻辑 → 放 `root/skills`**（`coco-skills` 是纯库），LLM 合并的 fork 编排放 `root/memory`（有 AgentHandle）。
- **门控**：新增 `Feature::SkillLearning`（Experimental），复用 `ModelRole::Subagent`（技能创作偏 coding，不新增 role）。错误用 snafu + `coco-error`（memory/skills 均为主干 root 模块）。
- **比 Hermes 更稳的一点**：Hermes 的 review 是 daemon 线程 + 宽泛 `except: pass`，进程退出即丢失当轮学习。coco-rs 应把待处理 review 持久化（一个轻量 pending-queue 文件 + `CancellationToken`），崩溃可恢复。

**工作量 / 风险**：**L** / 中高（自主写盘、前缀缓存正确性、技能膨胀——需像 Hermes 那样在 prompt 里强推 umbrella 级技能并尽早上 Curator 合并）。

**依赖 / 前置**：T1.2（provenance + telemetry，Curator 的输入）；Fork mode（已具备）；skill 编程式写路径（当前只有 `skillify` 交互式，需抽出可被 fork 调用的技能创作工具）。

### T1.2 技能使用遥测 + provenance sidecar（T1.1 的前置，亦可独立发布）

**Hermes 做法**：`tools/skill_usage.py` 维护 `~/.hermes/skills/.usage.json`，每技能记录 `{created_by, use_count/view_count/patch_count, last_used_at/…, state(active/stale/archived), pinned}`；`bump_use/view/patch` 在技能被加载/查看/改写时触发，是 Curator 老化的唯一数据源。

**coco-rs 差距**：技能零遥测。`MemoryEvent`/OTel 有 telemetry 但从不回喂策略（findings 明确"never fed back"）。skills 有 bundled/user/project/plugin/managed/MCP 六来源，但无 `created_by:agent` 这种"agent 自造 vs 用户所有"的可区分标记。

**落地方案**：在 `coco-skills` 增加 sidecar store（`SkillProvenance` typed struct，非 `Value`）。`SkillManager`（`skills/src/lib.rs`）已把技能桥接进斜杠命令与 Skill 工具目录——在这两个调用点打 bump 即可；可选同时 emit 一个 `CoreEvent`（三层分发中的 Protocol/Tui 层）。

**工作量 / 风险**：**S–M** / 低。

**依赖 / 前置**：无，纯增量。

### T1.3 跨会话 session-search 召回（FTS + 工具化）

**Hermes 做法**：`hermes_state.SessionDB` 用 SQLite FTS5（+trigram、BM25、损坏自愈）持久化每条消息；`tools/session_search_tool.session_search` 提供 Discovery/Scroll/Read/Browse + anchored-window 召回 + cron 行降权 + lineage 去重 + 跨 profile；`agent/title_generator.py` 用辅助 LLM 异步生成 3–7 词标题供 title-match 召回。

**coco-rs 差距（增量而非从零）**：`coco-memory` 的 recall 只覆盖 memory 文件，不覆盖会话历史。但**基础设施已大半就位**——`coco-session` 已把每条消息持久化为 **JSONL 事实源**，`coco-retrieval` 已有可复用的 BM25 引擎（现面向代码、`RetrievalEvent` 隔离流），且 file-history + 四相 rewind + Local Session Hub 已**部分覆盖**"翻阅/恢复历史"的诉求。真正缺的只是「**消息级 FTS 索引** + 一个**模型可调用的会话检索工具**」这两块，故本项是增量填缝而非新建子系统，必要性也相应有限。

**落地方案**：
- 新增会话消息索引：**优先复用 `coco-retrieval` 的 BM25**（喂一个 session-message 语料），或在 `coco-session` 内加 rusqlite FTS5 store。
- 在 `core/tools` 加 `SessionSearch` 工具，后端经一个注入式 handle trait（对齐 `ScheduleStore`/`MailboxHandle` 的 callback-handle 范式，放 `core/tool-runtime`），实现体在 app 层注入——保持 tools 不直接依赖子系统。
- **标题生成已有**：findings 指出 `coco-session` 已做 title generation，直接复用做 title-match，无需新建。

**工作量 / 风险**：**M** / 低中（索引一致性、增量更新）。

**依赖 / 前置**：`coco-session`（已具备）；retrieval BM25（已具备）或引入 rusqlite FTS5。

### T1.4 两个即插即用增强（快速收尾）

- **~~MCP/插件工具的渐进式披露~~（coco 已具备，非缺口）**。经代码核实，coco-rs **已有一等 `ToolSearch`**（`core/tools/src/tools/tool_search.rs`，`Feature::ToolSearch` 默认开启，`deferred_tools` 发现 + `DeferredToolsDelta` promotion，见 `tool-search-design.md`），甚至含 **provider-native 变体**（`Capability::OpenAiNativeToolSearch`、Anthropic `tool_search` beta、`ClientSideToolSearchPromotion`），已**优于** Hermes 的纯客户端 BM25。因此无需从 Hermes 移植；若要精进，只需核对「MCP/plugin 大工具数组是否已纳入延迟分类覆盖面」——属 **S / 低** 的验证性收尾，而非新建。
- **evidence-based verify-on-stop（防过度声称）**。Hermes 在"无 tool_call 的收尾分支"前置一个门（`agent/verification_stop.py`）：本轮改了*真实代码*（过滤文档路径）且工作区缺"通过"证据时，注入一条合成 nudge、`finish_reason=verification_required`、不外显尝试性答案并继续，surface-aware（聊天平台静默）、有界（≤2 次）。落地：在 `app/query::engine_finalize_turn` 加一个可配置门，配合 coco-rs 已有的文件变更追踪。**S / 低**。这是 coco-rs 当前缺失、且改动局部的高性价比项。

---

## Tier 2 — 高价值、投入更大

### T2.1 IM 网关：把 Event Hub 从骨架建成真网关

**Hermes 做法**：单 asyncio 进程（`gateway/run.py`）同时前置 20+ 平台，核心是 `BasePlatformAdapter` ABC（`gateway/platforms/base.py`，~5400 LoC）吸收了并发/中断/去抖/流式/媒体/TTS-STT/typing/重试等横切逻辑，故 IRC 适配器 <1000 LoC；插件化平台注册（`register(ctx)` + `PlatformEntry` + `Platform._missing_()` 动态枚举，零核心改动即可加平台）；确定性 `build_session_key` + SQLite 会话store 实现跨平台连续性；DM 配对（`gateway/pairing.py`，哈希 8 位码、TTL、限流、锁定、constant-time）+ 分层授权；`DeliveryRouter` + 死目标自愈 + 静默叙述反循环。

**coco-rs 差距**：**完全没有任何消费级 IM 集成**（无 Telegram/Slack/Discord/…）。`hub/connector` 是一行 re-export 骨架，spec 里的"WS 出流→SQLite→实时 Web UI→多实例聚合"未建；`hub/server` 只是只读本地会话查看器；SDK 仅 stdio。唯一触达 IM 的方式是用户自配一个通用 MCP。这是相对 Hermes 最尖锐的差距。

**落地方案（新 crate + 复用 QueryEngine）**：
- **新建 `hub/gateway` crate**（与 `hub/connector`/`hub/server` 同层）。定义 `PlatformAdapter` trait（Rust 版 BasePlatformAdapter）：`connect/disconnect/send/get_chat_info` 为必需，媒体/流式/交互按需默认实现。**coco-rs 天然优势**：它是 async/tokio，每个会话一个 task，不需要 Hermes 那套"os.environ→contextvars"迁移来修跨轮污染——并发隔离免费获得。
- **驱动复用 `app/query::QueryEngine` + CommandQueue**：入站消息→按 `build_session_key` 映射到 `coco-session` id→以新 `QueueOrigin::Gateway` 入队（对齐已有 `QueueOrigin::Cron`）唤醒该会话 driver。
- **出流复用 CoreEvent**：`QueryEngine` 的 `mpsc::Sender<CoreEvent>` 就是设计好的 egress 点（今天 connector 空转）；`CoreEvent::Stream` → 平台 edit-in-place（如 Telegram `editMessageText`），仿 `GatewayStreamConsumer`。
- **授权**：分层 allowlist（config）+ 移植 DM 配对（哈希码/TTL/限流），密钥走 `utils/keyring-store`。
- **平台**：先做 **Telegram（HTTP long-poll，最简）** + **Slack** 两个适配器，各自子模块或插件。
- **CLI**：新增 `coco gateway` 长驻子命令（`app/cli`），持有 `HashMap<Platform, Box<dyn PlatformAdapter>>`。
- **门控**：`Feature::Gateway`（Experimental）。**错误分层**：网关编排跨层且面向用户 → snafu + `coco-error`；平台 SDK 包装的叶子 crate → thiserror（库层禁 `anyhow`）。

**工作量 / 风险**：**XL** / 高（面广、长驻进程、鉴权、每平台怪癖、限流）。建议按"先 1 平台端到端跑通 → 抽象稳定 → 再加第 2 个"推进。

**依赖 / 前置**：CoreEvent egress（部分就位）；`coco-session` 会话keying；`QueueOrigin` 扩展；平台 token 配置；keyring-store。

### T2.2 cron 结果投递到外部渠道 + headless tick

**Hermes 做法**：cron 在 gateway 守护进程内跑，`run_job` 结果经 `_deliver_result`/`_resolve_delivery_targets` 路由到 origin chat 或 home 平台（telegram/discord/slack…）；blueprints/suggestions 提供 consent-first 参数化模板。

**coco-rs 差距**：cron **已端到端实现**（`utils/coco-cron` + 六个调度工具 + `app/cli/src/cron_tick.rs` 的 1s tick），但**只在 TUI 交互态运行**（headless/SDK 无 queue-drain pump），且只把 prompt 打进本地会话——**无外部渠道投递**；`RemoteTrigger` 是 default-off stub。

**落地方案**：
- **投递层**：给 `CronTask` 加可选 delivery target；cron 触发的一轮完成后，经一个 `DeliveryRouter`（放 `hub/gateway`）投递。**无网关时的降级路径**：coco-rs *已有*一个 SSRF-guarded 的出站 webhook hook 类型（`hooks/src/lib.rs`）——先做 webhook-only 投递即可零新依赖上线。
- **headless/守护 tick**：把 cron tick pump 挪进 T2.1 的 `coco gateway` 进程（或加 `coco daemon` 模式），让 durable cron 任务无需交互会话即可触发——正是 Hermes "gateway 守护进程跑 cron"的形态。findings 也点名 cron 的跨进程 lease lock / file-watcher / jitter 目前 deferred，可一并补齐。
- blueprints/suggestions 优先级低，先提及。

**工作量 / 风险**：webhook 投递 **M**；网关集成 + headless tick **L** / 中。

**依赖 / 前置**：外部平台投递依赖 T2.1；webhook-only 版本仅依赖现有 hooks，可先行。

---

## Tier 3 — 战略 / 可选

### T3.1 serverless/远程终端后端（Modal/Daytona 类比）经 exec-server trait

**Hermes 做法**：六后端统一在 `BaseEnvironment` ABC 后（local/docker/ssh/singularity/modal/daytona）；Modal/Daytona 提供跨会话文件系统持久化；`FileSyncManager`（mtime+SHA-256）双向同步凭据/技能。

**coco-rs 差距**：`exec/exec-server` 只有 `local` + `remote`（loopback WebSocket，codex 派生），`EnvironmentManager` 恰好两个 environment；无 docker/ssh/serverless；environment registry 是 v1 显式排除项。这是相对 Hermes 的最大执行层差距。

**落地方案**：exec-server 的 trait seam **已备好**（`ExecBackend`/`ExecutorFileSystem`/`HttpClient` + `EnvironmentManager`，`exec/exec-server/src/environment.rs`）——加后端只需实现 trait。依次补：`docker` 后端、`ssh` 后端、serverless（Modal/Daytona）后端；解冻 environment registry；`FileSyncManager` 类比落成一个 `utils/*` crate（thiserror）。

**工作量 / 风险**：docker/ssh **L**；带持久化的 serverless **XL** / 中。

**依赖 / 前置**：exec-server trait seam（已具备）；解冻 environment registry。

### T3.2 结构化用户建模（Honcho dialectic 类比）

**Hermes 做法**：`plugins/memory/honcho` 经 `MemoryProvider` ABC 做多轮 dialectic 用户建模（who-is-this-person→gap synthesis→矛盾调和，带早停），结果注入 system prompt。

**coco-rs 差距**：只有自由文本 `[user]` memory 类型（普通 memory 文件），无偏好推断引擎/persona 模型/provider ABC（taxonomy 是闭合枚举）。

**落地方案**：(i) 给 `coco-memory` 加 `MemoryProvider` 风格 trait 以支持外部 provider；(ii) 一个轻量 user-model side-query（复用 `ModelRole::Memory`）定期把 `[user]` memory 蒸馏成结构化 persona 块、每会话注入一次。不必照搬 Honcho 的 OAuth/外部服务。

**工作量 / 风险**：**M–L** / 中（质量、隐私）。

**依赖 / 前置**：coco-memory recall/注入（已具备）。

### T3.3 execute_code 零上下文成本管道（PTC）

**Hermes 做法**：`execute_code` 生成 `hermes_tools.py` RPC stub，agent 写脚本经 UDS（本地）/文件轮询（远端）回调工具，多步工具链塌缩为一轮推理，**只有 stdout 回上下文**，中间结果永不进 context——真正的 token 效率创新。

**coco-rs 差距**：无 execute_code 工具。

**落地方案**：在 `core/tools` 加 `ExecuteCode` 工具，脚本经 `exec/sandbox` 执行，用 UDS RPC 回调 `ToolRegistry`。**关键协同**：coco-rs 的 Dynamic Workflow 运行时计划正是嵌入 `rquickjs`（QuickJS）+ 确定性 shim + host fns（`workflow-runtime-plan.md`）。建议 execute_code **建在该 rquickjs 宿主之上**（JS 脚本），一石二鸟地把休眠的 workflow 运行时激活。

**工作量 / 风险**：**L** / 中（sandbox、RPC）。

**依赖 / 前置**：workflow-runtime（rquickjs，目前 stub）或另择脚本宿主；`exec/sandbox`（已具备）。

### T3.4 Kanban 调度器：多 agent 工作队列自动分发

**Hermes 做法**：SQLite Kanban + gateway 内 dispatcher tick（回收 stale claim、promote ready、原子 claim、spawn `hermes -p <profile>` 子进程）、per-profile 并发上限、失败上限自动 block。

**coco-rs 差距**：`coco-tasks` 的 `task_list`（`tasks/src/task_list.rs`）已是 disk-backed Kanban（fs2 锁、原子 claim、`blocked_by` 依赖、team-shared）——**架构已很强**。缺的是**自主 dispatcher**：没有 tick 去 promote/claim/spawn，coordinator 的 spawn 目前是工具驱动的手动过程。

**落地方案**：加一个 dispatcher tick（放 `coordinator` 或新模块，跑在 T2.1 的守护进程或 `app/cli`），仿 `cron_tick` 周期扫描 task_list store，promote ready、原子 claim、经现有 `SwarmAgentHandle`（`coordinator/src/agent_handle/spawn.rs`）spawn teammate；加 per-agent 并发上限 + 失败上限自动 block。纯组合 coco-tasks + coco-coordinator + 一个 tick pump。

**工作量 / 风险**：**M–L** / 中。

**依赖 / 前置**：coco-tasks / coco-coordinator（均已具备）；tick pump（网关/CLI）。

### T3.5 trajectory / research 工具（strictly optional）

**Hermes 做法**：`trajectory_compressor.py` + `batch_runner.py` 为训练 tool-calling 模型批量生成/压缩 ShareGPT 轨迹。

**coco-rs 差距**：无——coco-rs 是产品 SDK，非训练管线。

**落地方案**：仅当规划 RL/eval 时才做。可作独立 bin 复用 `QueryEngine` + 一个 toolset 采样器。默认**不建议**投入。

**工作量 / 风险**：**L** / 低（隔离）。

---

## 小结：快速收益 vs 战略下注

| 项目 | 层级 | crate 归属 | 工作量 | 风险 | 定位 |
|------|------|-----------|--------|------|------|
| T1.2 技能遥测 + provenance | T1 | `skills` | S–M | 低 | **快速收益**（且是 T1.1 前置）|
| T1.4b verify-on-stop | T1 | `app/query` | S | 低 | **快速收益** |
| T1.3 session-search 召回 | T1 | `core/tools` + `session`/`retrieval` | M | 低中 | **快速收益** |
| ~~T1.4a 渐进式工具披露~~ | — | `core/tools` | — | — | **coco 已具备（ToolSearch），非缺口** |
| T2.2 cron webhook 投递 | T2 | `hooks` 复用 | M | 中 | **快速收益**（webhook-only 版）|
| T1.1 技能自主学习闭环（Curator） | T1 | `memory` + `skills` + `subagent` | L | 中高 | **战略下注**（Hermes 头号差异）|
| T2.1 IM 网关（Telegram/Slack 起步） | T2 | 新 `hub/gateway` + `app/cli` | XL | 高 | **战略下注**（Hermes 头号差异）|
| T3.1 serverless 终端后端 | T3 | `exec/exec-server` + `utils` | L–XL | 中 | 战略下注 |
| T3.3 execute_code PTC | T3 | `core/tools` + workflow-runtime | L | 中 | 战略下注（顺带激活 workflow 运行时）|
| T3.4 Kanban 调度器 | T3 | `coordinator` + `tasks` | M–L | 中 | 战略下注 |
| T3.2 用户建模 | T3 | `memory` | M–L | 中 | 战略下注 |
| T3.5 trajectory 工具 | T3 | 独立 bin | L | 低 | 可选，产品线暂不建议 |

**推荐执行序**：先清 Tier 1 快速项（T1.2 → T1.4b verify-on-stop → T1.3），它们独立、低风险、且 T1.2 为 T1.1 铺路（T1.4a 渐进式工具披露经核实 coco 已具备，已从待办中移除）；随后投 **T1.1 技能学习闭环**（coco-rs 已有 Fork 前缀缓存共享 + 每轮 finalize 调度钩子，落地成本被显著摊薄，是最能拉平与 Hermes 差距的单点）。Tier 2 的 **IM 网关（T2.1）** 是最大工程投入，但也是 coco-rs 唯一"从无到有"的战略能力，建议在 T2.2 webhook 投递验证 delivery 抽象后再启动，并以单平台端到端为里程碑。Tier 3 视产品方向按需取用——其中 T3.1/T3.3/T3.4 都能"坐满 coco-rs 已画好的空座位"（exec-server trait、rquickjs 宿主、task_list Kanban），性价比高于从零新建。

---

> [← IM 接入深剖](04-im-integration-deep-dive.md) · [返回索引](README.md)
