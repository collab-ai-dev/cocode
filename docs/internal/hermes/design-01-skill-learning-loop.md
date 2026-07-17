# 设计 ①:技能自主学习闭环(Curator 式,战略)

> [← 设计③遥测+Provenance](design-03-skill-telemetry-provenance.md) · [返回索引](README.md) · [设计②IM 网关 →](design-02-im-gateway.md)

> ## ⚠️ 评审修正(权威 — 与下文冲突处以本节为准)
>
> 本设计经对抗式评审(对照真实 seam / 代码已复核)后修正如下:
>
> 1. **`SkillWriteHandle` 归本 crate `coco-skill-learn`(此处为唯一定义)**,镜像 `memory/src/can_use_tool.rs`。设计 ③ 里放在 `coco-skills` 的同名 handle 已作废。
> 2. **Bash 写门要照抄 memory 的多步实现,而非一次调用。** `memory/src/can_use_tool.rs:295-307` 的真实门是:取 `input["command"]` 字符串 → `ShellParser::new().parse(cmd)` → `try_extract_safe_commands()`(遇重定向/子 shell/命令替换即返回 `None`,fail-closed)→ `stages.iter().all(|argv| coco_shell_parser::safety::is_known_safe_command(argv))`。**`is_known_safe_command` 吃的是单段已解析 argv,不是原始 `Value`。** 逐字移植 memory 的 `is_known_safe_bash`,不要用 `is_known_safe_command(Value)` 简写。
> 3. **不要跨 crate 取时钟。** 删除对 `coco_memory::service::dream::DreamService::now_ms()` 的复用——`coco-skill-learn` 的 DAG 不依赖 `coco-memory`,该调用在 `review.rs`/`curator.rs`/`lock.rs` 内不可达。改为在 `coco-skill-learn` 定义本地 `now_ms()`(或复用 `coco-utils` 时间助手),engine 侧与 service 侧共用同一个。
> 4. **用户可见反馈需要新的 SystemMessage 变体。** `SystemMemorySavedMessage` 是 memory 专属变体,**不可复用**。为「Learned skill: X」新增一个 SystemMessage 变体,或复用 `SystemMessage::Informational`。
> 5. **`AttachmentKind::SkillLearnedReminder` 不是一行改动。** 需同步更新 `attachment_kind.rs` 的多处穷举 match:`as_str`(:152)以及分类/覆盖臂(:235/:333/:440/:616)。加法安全但非平凡,实施时按清单逐臂补齐。
> 6. **两条 turn-end 尾路都要挂钩(评审确认的正确点)。** `finalize_turn_post_tools`(有工具调用,`engine_finalize_turn.rs:619` 跑 memory fan-out)与 `handle_no_tool_calls_terminal`(纯文本收尾,`engine_terminal.rs:214`,memory 未覆盖)。skill-review 应抽一个共享 helper,两路都调用,避免纯文本回合漏挂。

---

# DESIGN #1:Skill 自主学习闭环(Skill Autonomous Learning Loop)

> 一句话:把 coco 自己的 memory 闭环(`ExtractService` 每回合 fork+fence+cursor+backoff、`DreamService` 周期性 CAS-lock 巩固)**整套搬到 SKILL 上**,复用同一条 subagent Fork / `CanUseToolHandle` 写栅栏 / `ModelRole` 路由 / `finalize_turn` 挂载点。核心工作量是"改指向"(memory_dir → skills_dir、`ModelRole::Memory` → `ModelRole::Review`、`ForkLabel::ExtractMemories` → `ForkLabel::LearnSkill`),真正的新代码只有:SKILL.md 序列化器、provenance 字段、Curator 老化逻辑、以及一个新 crate 的装配。

---

## 目标 & 非目标

### 目标
1. **触发**:在每个"已交付(非中断)"的主 agent 回合尾部,做一次廉价的 skill-review 判定(counter/throttle,与 memory 的 `extraction_throttle` 同构),只在有"可学习片段"时才 fork。
2. **Review Fork**:spawn 一个受约束的 Fork 子 agent,工具白名单仅限"读 + skill-write",跑一个把本次会话蒸馏成技能变更的 review prompt,偏好序遵循 Hermes:`UPDATE 已加载 skill` > `UPDATE umbrella skill` > `ADD 支撑文件` > `CREATE 新 umbrella`。所有写入带 `created_by=agent` provenance。
3. **Consolidation(Curator,DreamService 类比)**:周期性(阈值 + CAS lock)pass,按使用遥测(design #3)把 **仅 agent 创建** 的 skill 老化(active→stale→archived),并可选 LLM-merge 进 umbrella。**归档而非删除** + 备份。
4. **安全**:两环写栅栏 + fork 内危险命令自动拒绝 + provenance 强制 + Feature 门。

### 非目标
- 不改 model 驱动的 `/skillify` / `/run-skill-generator` 交互式创建路径(它们继续存在,与本闭环互补)。
- 不触碰 `created_by=user` / `Bundled` / `Managed` / `Plugin` / `Mcp` 来源的 skill(Curator 与 Review 的 UPDATE 只作用于 agent 创建物)。
- 不实现"全 system-prompt 缓存平价"的 Path A(ForkDispatcher);本设计采用 memory 同款 Path B(`spawn_agent` + `fork_context_messages`,消息前缀缓存共享),把 Path A 列为开放取舍(见 §风险)。
- headless/SDK 模式的 turn-end fan-out 不在本期保证(与 memory 一致,仅交互式 TUI 有 drain pump)。

---

## 现状(基于 seam,已有什么)

| 需要的能力 | seam 里已存在的东西 | 复用方式 |
|---|---|---|
| 每回合 turn-end 钩子 | `QueryEngine::finalize_turn_post_tools` → `build_memory_finalize_ctx_and_run`(`app/query/src/engine_finalize_turn.rs:595` / `:1383`) | **对称新增** `build_skill_review_ctx_and_run`,紧挨 memory block(`:619`) |
| fork+fence+cursor+backoff 服务模板 | `ExtractService`(`memory/src/service/extract.rs`)`maybe_extract`/`run`/`InProgressGuard`/`effective_throttle` | **整体 copy-adapt** 成 `SkillReviewService` |
| 周期性巩固 + CAS lock 模板 | `DreamService::consolidate_with_gates`(`memory/src/service/dream.rs:354`)+ `lock::try_acquire`(`memory/src/lock.rs:108`) | **整体 copy-adapt** 成 `SkillCuratorService` + 独立 lock 文件 |
| 写栅栏(内环) | `CanUseToolHandle` trait(`core/tool-runtime/src/can_use_tool.rs:167`)+ `AutoMemHandle`(`memory/src/can_use_tool.rs`) | **新 impl** `SkillWriteHandle` 镜像 `AutoMemHandle` |
| 写栅栏(外环) | `AgentSpawnConstraints { max_turns, allowed_write_roots }`(`core/tool-runtime/src/agent_handle.rs:44`) | 直接填 `allowed_write_roots: vec![skills_dir]` |
| fork 请求描述符 | `AgentSpawnRequest`(`agent_handle.rs:145`)、`AgentHandle::spawn_agent` | 完全按 `ExtractService::run` 构造 |
| ModelRole 路由 | 合成 `AgentDefinition{ model_role }`(extract.rs:665)+ `resolve_subagent_selection` | 合成 def 里塞 `ModelRole::Review` |
| fork 遥测标签 | `ForkLabel`(`common/types/src/fork_label.rs`) | 新增 `LearnSkill` / `CurateSkills` 变体 |
| fork 写入清单 | `AgentSpawnResponse.paths_written`(`agent_handle.rs:403`) | 读它得知写了哪些 SKILL.md |
| 遥测通道(OTel-only) | `MemoryTelemetryEmitter` + `MemoryEvent`(`memory/src/telemetry.rs`) | 镜像 `SkillLearnTelemetryEmitter` + `SkillLearnEvent` |
| 反馈通道(进对话) | `NoticeInbox` → `FinalizeTurnReport.notices` → 引擎投影 `SystemMemorySavedMessage` / `<system-reminder>`(`memory/src/notice.rs`) | 镜像 `SkillLearnInbox` + `AttachmentKind::SkillLearnedReminder` |
| 晚绑定 AgentHandle | `install_agent`(共享 `RwLock<AgentHandleRef>`)+ session bootstrap(`app/cli/src/session_runtime.rs:927-1008`) | 同款装配 |
| skill 使用遥测 | `SkillUsageStats{ usage_count, last_used_at_ms }` + `score_for`(`skills/src/usage.rs`) | Curator 老化直接读它(design #3 扩展 outcome) |
| skill 目录/目录构建 | `build_session_skill_manager`(`skills/src/lib.rs:1271`)、`SkillChangeDetector` 热重载(`skills/src/watcher.rs`) | 写盘后**自动热重载,零额外代码** |
| Builder 挂载模式 | `QueryEngine::with_memory_runtime` / `memory_runtime()`(`engine_builder.rs:579`) | 镜像 `with_skill_review_runtime` |

**关键缺口(必须新建)**:
- `SkillDefinition → SKILL.md` **序列化器**(seam 明确:今天没有任何代码把结构体写回 frontmatter+body;`extraction.rs` 只是把 bundled 参考文件解到 temp,不是生成 skill)。
- `SkillDefinition` 上的 **provenance 字段**(`created_by`/`created_at`/`generated`)+ `parse_skill_markdown` 读取(今天 `author` 等未知 frontmatter 键被静默丢弃,见 `lib.test.rs:1173`)。→ **属 design #3,是本设计 Phase 0 前置**。
- Curator 的 **老化状态机**(今天 `score_for` 只喂 `/` 自动补全 UI,无任何代码据 usage 触发 skill 精炼)。

**必须纠正 task 中的一处误解**:**不存在 `FORK_PLACEHOLDER`**。`core/subagent/src/fork.rs` 明确:coco 逐字透传父回合真实的 `tool_result` body(早期 TS 设计把它们抹成 placeholder 反而击穿了缓存)。前缀缓存共享靠两份逐字节相同的输入(rendered system prompt + 带真实结果的父历史),**不是**占位符。本设计据此走 Path B。

---

## 架构总览(crate 归属 + ascii 数据流)

新建根层 crate **`coco-skill-learn`**(`skill-learn/`),**完全镜像 `coco-memory` 的层级约束**:只依赖 `coco-tool-runtime`(traits:`AgentHandle`/`CanUseToolHandle`/`SideQuery`)、`coco-config`、`coco-types`、以及 `coco-skills`(序列化器 + provenance + `SkillManager` + usage 遥测)。**不依赖** `coco-messages` / `coco-inference`(保持 DAG 干净,与 memory 同规矩,见 `memory/CLAUDE.md:169`)。

```
                          coco-config (SkillLearnConfig, EnvKey, Feature::SkillLearning)
                                  │ 折叠一次 build_runtime_config_with
                                  ▼
 app/cli/session_runtime.rs ── build SkillReviewRuntime (builder) ─┐
     · Feature::SkillLearning 门 + config.enabled 激活             │ install_agent(swarm handle 晚绑)
     · with_skill_review_runtime(rt) 装到每回合 engine            │ install_side_query(可选,用于 skill-recall)
                                  │                                ▼
 app/query/engine_finalize_turn.rs::finalize_turn_post_tools      Arc<RwLock<AgentHandleRef>>
   ├─(已存在)memory block  ── build_memory_finalize_ctx_and_run
   └─(新增,对称)         ── build_skill_review_ctx_and_run ──► SkillReviewRuntime::finalize_turn(ctx)
                                                                     │  gate: bare_mode || is_subagent → skipped
                                                                     │  gate: !turn_delivered → skipped
                                                                     ▼
                                                          TurnEndScheduler(单-pending 合并)
                                                          ┌──────────────┴───────────────┐
                                                          ▼                               ▼
                                     SkillReviewService::maybe_review        SkillCuratorService::maybe_consolidate
                                     (ExtractService 类比)                    (DreamService 类比)
                                       throttle+backoff+cursor                  time/scan/session gate + CAS lock
                                          │ run()                                   │ consolidate_with_gates()
                                          ▼                                         ▼
                                 AgentHandle::spawn_agent(AgentSpawnRequest)  ← 同一条 Path B fork 路径
                                   · definition = AgentDefinition{ ModelRole::Review }
                                   · fork_context_messages = 父历史切片(消息前缀缓存共享)
                                   · constraints{ max_turns, allowed_write_roots:[skills_dir] } ← 外环
                                   · can_use_tool = SkillWriteHandle(skills_dir)              ← 内环(step 3.5)
                                   · fork_label = LearnSkill / CurateSkills
                                   · skip_transcript = true
                                          │
                                          ▼  fork 用(受栅栏的)Write/Edit 写盘
                            <skills_user_dir>/<name>/SKILL.md  (frontmatter created_by=agent)
                                          │
                                          ├─► SkillChangeDetector 300ms debounce 热重载(零额外代码)
                                          └─► response.paths_written → SkillReviewReport.notices
                                          ▼
        引擎投影:SkillLearnNotice → SystemMessage(用户可见) + <system-reminder>(模型可见,AttachmentKind::SkillLearnedReminder)
        并行:SkillLearnEvent → SkillLearnTelemetryEmitter(OTel/tracing,不回流 agent loop)
```

**双环栅栏 + 三重 provenance**(纵深防御,与 memory 同思路):
```
外环: AgentSpawnConstraints.allowed_write_roots = [skills_dir]   (文件变更工具落盘前拒)
内环: SkillWriteHandle::check @ tool-runtime step 3.5           (回调权威,Deny 短路成 tool_result)
prov: prompt 指令 + Curator 预过滤 created_by==Agent + 序列化器强制写 created_by=agent frontmatter
```

---

## 详细设计

### 组件 0 —— provenance & 遥测(design #3 前置,本设计所需最小面)

**位置:`coco-skills`(Tier-2,返回 `crate::Result<T>=Result<T,SkillsError>`)。**

在 `SkillDefinition`(`skills/src/lib.rs:56`)新增一个 provenance 子结构(用 `Option`/`#[serde(default)]` 保证老 SKILL.md 仍可解析,遵守"typed-over-Value"):

```rust
// skills/src/lib.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuthor {
    #[default]
    User,       // 人写的 / 未标注(默认,保护存量)
    Agent,      // 本闭环 fork 生成
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillProvenance {
    #[serde(default, skip_serializing_if = "is_default_author")]
    pub created_by: SkillAuthor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<i64>,
    /// 生成它的 fork session/turn,用于回溯与审计
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_session_id: Option<String>,
}
// SkillDefinition 增字段:
//   #[serde(default)] pub provenance: SkillProvenance,
```

`parse_skill_markdown`(`lib.rs:982`)新增显式读键 `created_by` / `created_at` / `generated`(今天未知键被丢弃),映射进 `provenance`;`bundled()`(`bundled.rs:46`)与 mcp builder 里默认 `SkillAuthor::User`。

**序列化器(design #1 唯一硬缺口,新文件 `skills/src/serialize.rs`)**:

```rust
// skills/src/serialize.rs  (Tier-2)
/// 把 SkillDefinition 渲染成 YAML frontmatter + body,键名与 parse_skill_markdown
/// 读取的键**逐字一致**以保证 round-trip。
pub fn serialize_skill_to_markdown(def: &SkillDefinition) -> crate::Result<String>;

/// 原子写(NamedTempFile+persist,复用 usage.rs 的落盘手法)到
/// <dir>/<name>/SKILL.md;返回写入的绝对路径。**不做**目录级校验——
/// 目录约束由 fork 栅栏保证;此函数仅供 fork 外的程序化创建/Curator 备份用。
pub fn write_skill_file(dir: &Path, def: &SkillDefinition) -> crate::Result<PathBuf>;
```

> 注意:Review fork 实际写盘走的是 fork 内的 `Write`/`Edit` 工具(受栅栏),序列化器主要供 **Curator 归档/备份** 以及未来"程序化创建(不经 model turn)"用。fork 写出的文件 frontmatter 由 review prompt 指令保证。

**遥测扩展(design #3)**:`SkillUsageStats`(`skills/src/usage.rs:70`)保持 wire 兼容(`usageCount`/`lastUsedAt` alias 不动),追加:

```rust
#[serde(default)] pub success_count: i64,
#[serde(default)] pub failure_count: i64,
#[serde(default, skip_serializing_if = "Option::is_none")] pub last_status: Option<String>,
```

新 `pub fn record_invocation(config_home: &Path, name: &str, outcome: InvocationOutcome)`,从 `QuerySkillRuntime::invoke_skill`(`app/query/src/skill_runtime.rs:150-484`)的 **Ok 和 Err 双臂** 调用(今天只在 Ok 调 `record`,失败不可见)。Curator 老化只需 `usage_count`+`last_used_at_ms`+`score_for`,已有;`success/failure` 供未来 self-improve 打分。

*copy-adaptable*:落盘手法、debounce、`score_for`。*new*:序列化器、provenance 字段、双臂 record。

---

### 组件 1 —— 触发器:`SkillReviewRuntime::finalize_turn` + 引擎挂载点

**位置:`coco-skill-learn`(root 层)。** 组合根 `SkillReviewRuntime`,一 session 一个,`Arc` 持有于 session runtime,与 `MemoryRuntime`(`memory/src/runtime.rs:413`)同形。

```rust
// skill-learn/src/runtime.rs
pub struct SkillReviewRuntime {
    pub config: SkillLearnConfig,          // coco_config 的薄镜像
    skills_dir: PathBuf,                    // 可写用户 skill 目录(config_home/skills)
    pub review: Arc<SkillReviewService>,
    pub curator: Arc<SkillCuratorService>,
    skill_manager: Arc<coco_skills::SkillManager>,  // 读取 loaded/created_by=agent skill
    agent_slot: Arc<RwLock<AgentHandleRef>>,        // 晚绑,3 服务共享
    side_query: OnceLock<SideQueryHandle>,          // 可选:skill-recall(镜像 MemoryRuntime::recall)
    notices: SkillLearnInbox,
    telemetry: Arc<dyn SkillLearnTelemetryEmitter>,
    turn_end_scheduler: Arc<TurnEndScheduler>,      // copy-adapt memory 的 runtime.rs:1359
}

pub struct FinalizeSkillReviewContext {
    pub bare_mode: bool,
    pub is_subagent: bool,       // = engine.config.agent_id.is_some()
    pub turn_delivered: bool,    // 非中断/非纯 reactive-compact 回合(见下)
    pub now_ms: i64,
    pub review_input: SkillReviewInput,   // 懒闭包 + signal
    pub transcript_dir: Option<PathBuf>,  // Curator 会话枚举用
}

pub struct SkillReviewReport {
    pub skipped: bool,
    pub review: Option<ReviewOutcome>,
    pub curator: Option<CuratorOutcome>,
    pub notices: Vec<SkillLearnNotice>,   // 引擎投影进 history
}

impl SkillReviewRuntime {
    pub async fn finalize_turn(&self, ctx: FinalizeSkillReviewContext) -> SkillReviewReport {
        if ctx.bare_mode || ctx.is_subagent || !ctx.turn_delivered {
            return SkillReviewReport::skipped();     // 与 memory 同款早退
        }
        self.turn_end_scheduler.schedule(ctx, self.worker_deps()).await;
        SkillReviewReport { skipped: false, review: None, curator: None,
                            notices: self.notices.drain() }
    }
    pub fn install_agent(&self, h: AgentHandleRef);           // 镜像 runtime.rs:695
    pub fn install_side_query(&self, h: SideQueryHandle) -> Result<(), SideQueryHandle>;
}
```

**引擎挂载点(对称新增)** —— `app/query/src/engine_finalize_turn.rs`,紧挨 memory block(`:619` 之后):

```rust
// app/query/src/engine_finalize_turn.rs
pub(crate) async fn build_skill_review_ctx_and_run(
    &self,
    history: &MessageHistory,
    bare_mode: bool,
    turn_delivered: bool,
    runtime: &Arc<coco_skill_learn::SkillReviewRuntime>,
) -> coco_skill_learn::runtime::SkillReviewReport {
    // 复用 memory 已算好的量:tool_calls_last_turn、last_msg_id
    let last_cursor = runtime.review.last_cursor().await;
    let messages_for_fork = history.to_vec();          // 懒闭包快照(memory 同款)
    let review_input = coco_skill_learn::service::review::SkillReviewInput {
        fork_messages: Box::new(move || arc_messages_since(&messages_for_fork, last_cursor.as_deref())),
        signal: compute_review_signal(history.as_slice()),   // 见下:可学习片段判定
        last_message_id: history.last().and_then(|m| m.uuid()).map(|u| u.to_string()),
    };
    let ctx = coco_skill_learn::runtime::FinalizeSkillReviewContext {
        bare_mode,
        is_subagent: self.config.agent_id.is_some(),
        turn_delivered,
        now_ms: coco_memory::service::dream::DreamService::now_ms(),  // 复用统一时钟
        review_input,
        transcript_dir: self.transcript_dir_for_curator(),
    };
    runtime.finalize_turn(ctx).await
}
```

在 `finalize_turn_post_tools` 内,memory block 之后加对称块(把 `report.notices` 投影成 `AttachmentKind::SkillLearnedReminder` 的 `<system-reminder>` + 一条用户可见 `SystemMessage`,复用 `crate::history_sync::history_push_and_emit`):

```rust
if let Some(rt) = self.skill_review_runtime.clone() {
    // turn_delivered:该回合真正交付了 tool 结果、且非中断。finalize_turn_post_tools
    // 本身只在工具执行完成路径运行 ⇒ 到这里即视为已交付;再排除 reactive-compact 重试。
    let turn_delivered = !matches!(continuation, TurnContinuation::Continuing if self.is_collapse_active());
    let report = self.build_skill_review_ctx_and_run(history, bare_mode_active, turn_delivered, &rt).await;
    for notice in report.notices {
        let reminder = format_skill_learn_reminder(&notice);
        let msg = coco_messages::wrapping::create_system_reminder_message_with_kind(
            coco_types::AttachmentKind::SkillLearnedReminder, &reminder);
        crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
    }
}
```

**"可学习片段" signal(`compute_review_signal`)** —— 触发不是每回合无脑 fork,而是像 memory `has_memory_writes` 一样先做廉价判定,只在满足下列之一时才让 throttle 继续:
- 本回合工具调用数 ≥ `review_min_tool_calls`(多步工作流 ⇒ 有可沉淀的流程);或
- 本回合 **invoke 过某个已加载 skill**(⇒ self-improve 机会,附上被调 skill 名);或
- 一个 `task_list` 项在本回合被标记完成(durable 计划完成 ⇒ 值得固化)。

signal 为空则 `ReviewOutcome::Skipped(NoSignal)`,零 fork 成本。

**关于 text-only 结束回合**:`handle_no_tool_calls_terminal`(`engine_terminal.rs`)**不**调 `finalize_turn_post_tools`,故与 memory 一样,本闭环只在工具执行回合触发。这对 skill(通常来自多步工具流)是合理的;把"纯文本回合也触发"列为开放问题(§风险),若要覆盖需把 fan-out 抽成 `run_skill_review_finalize` 共享 helper,在 `engine_terminal.rs:263~267` 之间再调一次。

*copy-adaptable*:`FinalizeTurnContext/Report` I/O 契约、懒闭包快照、`TurnEndScheduler`、早退门。*new*:`compute_review_signal`、`turn_delivered` 判定、投影到 `SkillLearnedReminder`。

---

### 组件 2 —— `SkillReviewService`(`ExtractService` 类比)+ 状态机

**位置:`skill-learn/src/service/review.rs`。整体 copy-adapt `memory/src/service/extract.rs`。**

```rust
// skill-learn/src/service/review.rs
pub type LazyForkMessages = Box<dyn FnOnce() -> Vec<Arc<Message>> + Send>;

pub struct SkillReviewInput {
    pub fork_messages: LazyForkMessages,
    pub signal: ReviewSignal,               // 空则不 fork
    pub last_message_id: Option<String>,
}

pub enum ReviewOutcome {
    Skipped(SkipReason),
    Completed { files_written: i32, duration_ms: i64 },
    Failed { reason: String },
}
pub enum SkipReason { Disabled, NoSignal, InProgress, Throttled, BackoffActive }

pub struct SkillReviewService {
    config: SkillLearnConfig,
    skills_dir: PathBuf,
    skill_manager: Arc<coco_skills::SkillManager>,
    agent: Arc<RwLock<AgentHandleRef>>,     // 与 runtime 共享 slot
    state: Mutex<ReviewState>,              // last_cursor / turns_since_last / consecutive_failures / pending_trailing
    in_progress: Arc<AtomicBool>,           // InProgressGuard RAII —— 逐字 copy extract.rs
    telemetry: Arc<dyn SkillLearnTelemetryEmitter>,
    notices: SkillLearnInbox,
    session_id: ArcSwap<String>,
    active_shell_tool: ActiveShellTool,
}

impl SkillReviewService {
    pub async fn maybe_review(&self, input: SkillReviewInput) -> ReviewOutcome;
    async fn run(&self, fork_context: Vec<Arc<Message>>, signal: ReviewSignal) -> ReviewOutcome;
    pub async fn last_cursor(&self) -> Option<String>;
    pub async fn reset(&self);              // /clear 时清 cursor+throttle
    fn effective_throttle(&self) -> i32 {   // 逐字同 extract.rs:
        self.config.review_throttle.max(1)
            << (self.consecutive_failures().min(MAX_BACKOFF_SHIFT))   // MAX_BACKOFF_SHIFT=5,封顶 32x
    }
}
```

`maybe_review` 门链(与 `extract.rs:331` 同结构):`review_enabled`(否→`Disabled`)→ `signal.is_empty()`(否→`NoSignal`)→ early `in_progress` 探测(占用则 stash `pending_trailing` + emit `ReviewCoalesced`,返回 `InProgress`)→ `turns_since_last += 1`;`< effective_throttle()`→`Throttled|BackoffActive`→ 否则重置计数并 `try_claim()` in_progress slot → `run()`。**cursor 只在 `Completed` 前进**,`Failed` 累加 `consecutive_failures` 驱动指数退避;主 fork 后 loop 排空 `pending_trailing`。

*copy-adaptable*:整套门链/backoff/coalescing/`InProgressGuard`/cursor 语义,几乎逐行。*new*:`ReviewSignal` 的存在(memory 用 `has_memory_writes` 做反向 direct-write 跳过;这里用 signal 做正向"有料才跑")。

---

### 组件 3 —— Review Fork spawn 调用 + ModelRole + ForkLabel

**`run()` 完全按 `ExtractService::run`(`extract.rs:631`)构造 `AgentSpawnRequest`**,只改指向:

```rust
// skill-learn/src/service/review.rs::run
let review_def = Arc::new(coco_types::AgentDefinition {
    agent_type: coco_types::AgentTypeId::Custom("skill-review".into()),
    name: "skill-review".into(),
    model_role: Some(coco_types::ModelRole::Review),   // ★ 用已存在的 Review 角色(purpose-built)
    ..Default::default()
});
let request = AgentSpawnRequest {
    prompt: build_review_prompt(&loaded_skills_manifest, &signal, skill_write_tools),
    description: Some("skill review".into()),
    session_id: (**self.session_id.load()).clone(),
    subagent_type: Some("general-purpose".into()),
    definition: Some(review_def),
    fork_context_messages: fork_context,               // ★ 消息前缀缓存共享(真实 tool_result,非 placeholder)
    constraints: Some(AgentSpawnConstraints {
        max_turns: Some(self.config.review_max_turns),  // 类比 extract 的 5
        allowed_write_roots: vec![self.skills_dir.clone()],  // ★ 外环栅栏
    }),
    skip_transcript: true,                              // 不污染用户 JSONL
    can_use_tool: Some(create_skill_write_handle_with_telemetry(
        self.skills_dir.clone(), self.telemetry.clone())),   // ★ 内环栅栏
    require_can_use_tool: false,
    fork_label: Some(coco_types::ForkLabel::LearnSkill),     // ★ 新变体
    active_shell_tool: self.active_shell_tool,
    ..Default::default()
};
let agent = self.agent.read().unwrap_or_else(PoisonError::into_inner).clone();  // 持锁 clone 再 drop
let resp = agent.spawn_agent(request).await;
```

成功后:`files_written = resp.paths_written.iter().filter(is_skill_file).count()`,push 一条 `SkillLearnNotice`,emit `SkillLearnEvent::ReviewCompleted{...}`。

**ModelRole 决策(回答 task 的"Memory or a new role?")**:用 **已存在的 `ModelRole::Review`**(`common/types/src/provider.rs`,8 变体里现成),它就是"给 review 子 agent 用"的角色,operator 可通过 `settings.models.review` 单独调。**不新增角色、不硬编码 model_id、不加 per-request override**(seam 铁律)。

**ForkLabel**:`common/types/src/fork_label.rs` 新增 `LearnSkill`(review)与 `CurateSkills`(consolidation)两变体 + `as_str()` 臂 + `fork_label.test.rs` round-trip 表。白名单排除 `Agent` 工具 ⇒ 天然非递归(叠加 `is_in_fork_child` 守卫),`child_query_depth` 默认 0 绕过 `SUBAGENT_DEPTH_LIMIT`。

**缓存取舍**:Path B 只共享消息前缀(system prompt 各自渲染),这是 memory 现成、可直接复用的路径。若要 **全 system-prompt 缓存平价**,须走 Path A(`ForkDispatcher::dispatch` + `CacheSafeParams`),但那要求 **pin 父 exact (provider, model_id)** ⇒ 无法路由 `ModelRole::Review`(角色→不同 model→缓存 miss),且 Path A 当前未把 `allowed_write_roots` 接进 `ForkContextOverrides`(外环缺失,只剩 `can_use_tool` 内环)。见 §风险。

*copy-adaptable*:整个 request 构造 + 合成 def + 持锁 clone handle。*new*:两个 ForkLabel 变体、`ModelRole::Memory`→`Review`、指向 skills_dir。

---

### 组件 4 —— `SkillWriteHandle` 写栅栏(`AutoMemHandle` 类比)

**位置:`skill-learn/src/can_use_tool.rs`。镜像 `memory/src/can_use_tool.rs` 的 `AutoMemHandle`。**

```rust
// skill-learn/src/can_use_tool.rs
pub fn create_skill_write_handle(skills_dir: PathBuf) -> CanUseToolHandleRef;
pub fn create_skill_write_handle_with_telemetry(
    skills_dir: PathBuf, telemetry: Arc<dyn SkillLearnTelemetryEmitter>) -> CanUseToolHandleRef;

struct SkillWriteHandle { skills_dir: PathBuf, telemetry: Arc<dyn SkillLearnTelemetryEmitter>,
                          allow_rm_md_bash: bool /* Curator=true, Review=false */ }

#[async_trait]
impl CanUseToolHandle for SkillWriteHandle {
    async fn check(&self, tool_name: &str, input: &Value, ctx: &CanUseToolCallContext)
        -> CanUseToolDecision
    {
        match tool_name {
            TOOL_READ | TOOL_GLOB | TOOL_GREP => allow("skill_write: read unrestricted"),
            TOOL_BASH => {                       // 危险命令自动拒(安全项)
                if coco_shell_parser::safety::is_known_safe_command(input) // 全 shell 解析,fail-closed
                   || (self.allow_rm_md_bash && is_rm_of_md_under(&self.skills_dir, input)) {
                    allow("skill_write: safe bash")
                } else { self.deny("bash not read-only") }
            }
            TOOL_WRITE | TOOL_EDIT | "apply_patch" => {
                // 相对路径先按 ctx.cwd 解析,再 symlink-aware path_under_root(fail-closed)
                if all_effect_paths_are_skill_files_under(&self.skills_dir, input, &ctx.cwd) {
                    allow("skill_write: skill file")
                } else { self.deny("write outside skills_dir") }
            }
            _ => self.deny("tool not in skill-write whitelist"),
        }
    }
}
```

每次 `Deny` emit `SkillLearnEvent::ReviewToolDenied{ tool_name }`(镜像 `ExtractionToolDenied`)。路径判定必须复用 `coco_utils_string`(不手切 `&str`)、`coco_utils_absolute_path`(cwd 解析),symlink-aware + fail-closed(悬挂/ELOOP/不对称→Deny),逐字沿用 memory 的 `path_under_root`。

*copy-adaptable*:整个 handle 结构、Bash 安全判定、path containment。*new*:skill 文件扩展判定(`SKILL.md` 及 skill 目录内 `.md`)、Deny 事件类型。

---

### 组件 5 —— Review prompt 契约(Hermes 偏好序)

**位置:`skill-learn/src/prompt.rs`。** `build_review_prompt` 预注入:(a) 当前 **已加载/本会话被调** 的 skill 清单(名字 + `created_by` + `skill_root`),(b) `ReviewSignal`(触发原因 + 相关 skill 名)。合同要点:

1. **偏好序(严格从高到低,只做一项)**:
   - `UPDATE 已加载 skill`:若本会话调用的某 agent 创建 skill 存在需修正/补强之处 → Edit 它;
   - `UPDATE umbrella skill`:若属于某已存在 agent 创建 umbrella 主题 → 编辑该 umbrella;
   - `ADD 支撑文件`:向已存在 skill 目录加参考文件(不动 SKILL.md 主体);
   - `CREATE 新 umbrella`:以上都不适用且确有可复用流程 → 新建 `<skills_dir>/<name>/SKILL.md`。
2. **provenance 强制**:任何 CREATE/UPDATE 必须在 frontmatter 写/保留 `created_by: agent`、`created_at`/`modified_at`(ms)、`origin_session_id`。**禁止编辑 `created_by: user` / bundled / managed 文件**(prompt 明说 + 栅栏兜底)。
3. **保守准则**:无高置信可复用价值就 **什么都不写**(宁缺毋滥,类比 memory 的克制)。禁止把机密/一次性上下文写进 skill。
4. **工具**:只有 Read/Glob/Grep + `Write`/`Edit`(工具名从 `tool_overrides` 取,与 memory 的 `FileMutationPromptTools` 同法)。

Curator prompt(`build_curator_prompt`)见组件 7。

*new*(全新,但 prompt 组织手法沿用 memory 的 `build_extract_prompt`/`build_dream_prompt`)。

---

### 组件 6 —— 已并入组件 0(序列化器)

见 §组件 0 `serialize.rs`。fork 写盘走受栅栏的 `Write`/`Edit`;序列化器主要供 Curator 备份/归档与未来非-model 程序化创建。

---

### 组件 7 —— `SkillCuratorService` 巩固(`DreamService` 类比)

**位置:`skill-learn/src/service/curator.rs` + `skill-learn/src/lock.rs`。整体 copy-adapt `memory/src/service/dream.rs` + `memory/src/lock.rs`。**

```rust
// skill-learn/src/service/curator.rs
pub enum CuratorOutcome { Skipped(SkipReason), Completed { archived: i32, merged: i32, duration_ms: i64 }, Failed { reason: String } }

impl SkillCuratorService {
    pub async fn maybe_consolidate<F: FnOnce() -> Vec<String> + Send>(
        &self, transcript_dir: &Path, enumerate_sessions: F, now_ms: i64) -> CuratorOutcome
    { self.consolidate_with_gates(transcript_dir, enumerate_sessions, now_ms, /*force*/ false).await }

    async fn consolidate_with_gates<F>(&self, transcript_dir: &Path, enumerate_sessions: F,
                                       now_ms: i64, force: bool) -> CuratorOutcome
    where F: FnOnce() -> Vec<String> + Send { /* 门序完全同 dream.rs:354 */ }
}
```

门序(逐字对齐 `dream.rs:354`,只改配置名与 lock 路径):
1. `curator_enabled`(否→`Disabled`);
2. 进程内 `try_claim_consolidating()` 原子(RAII,取消安全);
3. **time gate**:`lock::last_consolidated_at`(= curator lock 文件 mtime)vs `curator_min_hours`;
4. **scan throttle**:`SCAN_THROTTLE=10min`,一旦付出扫描即打戳(即使后续 gate 失败也保留,防自旋);
5. 懒 `enumerate_sessions()` → **session gate** `curator_min_sessions`;
6. **PID+mtime CAS lock**(`lock::try_acquire`,复用 memory `lock.rs` 手法,lock 文件 `<skills_dir>/.skill-curator-lock`;同进程可回收、1h 死 PID 回收、`LockGuard` RAII 在 drop 回滚 mtime,`commit()` 成功后保留 mtime 作 `lastConsolidatedAt`;`force`/`/curate` 手动跑 **回滚 mtime** 以不扰动自动节奏)。

**Curator fork(4 阶段,`ForkLabel::CurateSkills`,`allow_rm_md_bash: true`,`create_skill_write_handle` 归档变体)**:
- **Orient**:枚举 `<skills_dir>`,**预过滤 `created_by == Agent`**(读遥测 `load_all` + 解析 provenance)——只有 agent 创建物入候选(provenance 强制,第一环);
- **Assess(老化)**:按 `score_for(stats)`(`skills/src/usage.rs`:`usage_count * max(0.5^(days/7), 0.1)`)+ `last_used_at_ms`:
  - 超过 `stale_after_days` 未用 → 标记 `stale`(frontmatter 加 `lifecycle: stale`,仍可用但降权);
  - 超过 `archive_after_days` 且 score 低于阈 → **归档**:`Write` 一份到 `<skills_dir>/.archive/<name>-<ts>/SKILL.md`(备份),再 `rm` 原 `.md`(Bash 走 `allow_rm_md_bash` 白名单,仅限 skills_dir 内 `.md`)。**归档而非删除**;
- **Merge(可选)**:同主题多个 agent skill → LLM 合并进一个 umbrella,子文件归档;
- **Prune**:清理 `.archive` 中超 `archive_retention_days` 的旧备份(仍是 `rm .md` 白名单)。

**provenance 三环**:预过滤(候选只含 agent)+ prompt 指令(禁碰 user/bundled/managed)+ `SkillWriteHandle` 目录栅栏。任一失守其余兜底。

Curator 与 Review 在同一个 `TurnEndScheduler::run_worker` 里 fan-out(先 review,再 curator),与 memory 的 extract→dream 顺序同构。手动 `/curate`(Feature 门后)映射到 `SkillCuratorService::force`(绕过 time/session gate 但回滚 mtime)。

*copy-adaptable*:整套门序、CAS lock、RAII、force 回滚 mtime、4 阶段 prompt 骨架。*new*:老化打分(用现成 `score_for`)、`.archive` 归档路径、`lifecycle` frontmatter、predicate `created_by==Agent`。

---

### 组件 8 —— 双通道反馈 + 遥测(memory 双通道类比)

```rust
// skill-learn/src/telemetry.rs  —— OTel-only,不回流 loop
pub trait SkillLearnTelemetryEmitter: Send + Sync { fn emit(&self, e: SkillLearnEvent); }
pub enum SkillLearnEvent {
    ReviewCoalesced, ReviewCompleted { files_written: i32, input_tokens: i64, output_tokens: i64, duration_ms: i64 },
    ReviewError { duration_ms: i64 }, ReviewToolDenied { tool_name: String },
    CuratorFired { candidates: i32 }, CuratorSkipped { reason: String },
    CuratorCompleted { archived: i32, merged: i32, duration_ms: i64 }, CuratorFailed { phase: String, error_class: String },
}
// impls: NoopEmitter / TracingEmitter / OtelEmitter(Arc<OtelManager>) —— 逐字镜像 memory/telemetry.rs:235

// skill-learn/src/notice.rs  —— 反馈进对话
pub enum SkillLearnVerb { Created, Updated, Archived }
pub struct SkillLearnNotice { pub written_paths: Vec<String>, pub verb: SkillLearnVerb }
pub struct SkillLearnInbox { inner: Arc<Mutex<Vec<SkillLearnNotice>>> }  // push()/drain() 同 NoticeInbox
```

引擎侧把 `report.notices` 投影成:(a) 用户可见 `SystemMessage`("Learned skill: X" / "Archived N stale skills"),(b) 模型可见 `<system-reminder>`(`AttachmentKind::SkillLearnedReminder`,ambient,不主动叙述)。遥测走 OTel/tracing,**不**回流 agent loop(与 memory 严格二分)。

*copy-adaptable*:两通道结构、emitter 三 impl、drain-into-report。*new*:事件/verb 名。

---

### 组件 9 —— session bootstrap 装配

`app/cli/src/session_runtime.rs`(镜像 `:927-1008` memory 装配):

```rust
if runtime_config.features.enabled(coco_types::Feature::SkillLearning)
   && runtime_config.skill_learn.enabled {
    let skill_review = coco_skill_learn::SkillReviewRuntimeBuilder::new()
        .config(runtime_config.skill_learn.clone())
        .skills_dir(config_home.join("skills"))
        .skill_manager(skill_manager.clone())
        .telemetry(otel_emitter.clone())
        .build();
    let skill_review = Arc::new(skill_review);
    // 晚绑 swarm handle(TaskRuntime attach 之后):
    skill_review.install_agent(swarm_agent_handle.clone());
    // engine 装配:每回合 engine 都拿到它
    // (SessionRuntime::wire_engine 里 engine.with_skill_review_runtime(skill_review.clone()))
}
```

`app/query/src/engine_builder.rs` 镜像 `with_memory_runtime`:

```rust
pub fn with_skill_review_runtime(mut self, rt: Arc<coco_skill_learn::SkillReviewRuntime>) -> Self;
// 字段:pub(crate) skill_review_runtime: Option<Arc<coco_skill_learn::SkillReviewRuntime>>,  (new_with_turn_abort 初始化 None)
```

写盘后 `SkillChangeDetector`(User/Project scope,300ms debounce)**自动热重载**新/改的 SKILL.md ⇒ 无需额外重载代码。

*copy-adaptable*:builder + `Arc` + `install_agent` 晚绑 + `with_*` 装配 + 激活门。*new*:仅字段名。

---

## 配置 & Feature 门 & ModelRole

### Feature 门
新增 **`Feature::SkillLearning`**(`common/types/src/features.rs`:enum 变体 + `FEATURES` const 行,两者缺一 `Feature::info()` 会 `unreachable!`)。理由:自主编写/归档 skill 是一项**粗粒度新能力**,与 `AutoMemory`(它就是 Feature)对称——`AutoMemory` gate memory 子系统,`SkillLearning` gate 本子系统。

```rust
FeatureSpec { id: Feature::SkillLearning, key: "skill_learning",
              stage: Stage::UnderDevelopment, default_enabled: false }
```

门法:**在子系统入口** 判定(`with_skill_review_runtime` 仅在 Feature enabled 时装),而非 tool registry(seam 规矩)。子 agent 继承父 `Arc<Features>`,`finalize_turn` 对 `is_subagent` 直接 skip ⇒ 永不 widen。

### 配置(`SkillLearnConfig`,单一 merge 点)
`common/config/src/sections.rs` 定义 `SkillLearnConfig` + `impl { fn resolve(merged: &Settings, env: &EnvSnapshot) -> Self }`;`RuntimeConfig` 加 `pub skill_learn: SkillLearnConfig` 字段;`build_runtime_config_with`(`runtime.rs:311`)加一行 `skill_learn: SkillLearnConfig::resolve(merged, &env)`。`coco-skill-learn` 里放薄镜像 `config.rs`(字段对齐,单一真源在 coco_config,与 memory `config.rs:12` 同法)。所有子开关**留在 config,绝不升成 Feature 变体**:

| key | 默认 | 说明 |
|---|---|---|
| `enabled` | false | 子系统总开关(Feature 之下的细开关) |
| `review_throttle` | 5 | 每 N 个合格回合最多 fork 一次(类比 `extraction_throttle`) |
| `review_min_tool_calls` | 3 | signal 阈值 |
| `review_max_turns` | 5 | review fork turn 上限(类比 extract 的 5) |
| `curator_enabled` | true | Curator 总开关(类比 `dream_enabled`) |
| `curator_min_hours` | 24 | time gate |
| `curator_min_sessions` | 3 | session gate |
| `stale_after_days` | 30 | 老化→stale |
| `archive_after_days` | 90 | 老化→archived |
| `archive_retention_days` | 180 | `.archive` 备份保留 |
| `skills_write_dir` | None | 覆盖默认 `config_home/skills` |
| `guidelines` / `extra_guidelines` | None | 注入 review/curator prompt 的额外准则 |

### EnvKey(`common/config/src/env.rs`)
新增 `COCO_SKILL_LEARN_*`(`COCO_` 前缀强制,enum 变体 + `as_str()` 臂,E0004 逼你补):`CocoSkillLearnDisable`、`CocoSkillLearnReviewThrottle`、`CocoSkillLearnCuratorDisable`。经 `EnvSnapshot` 读,**禁止 leaf crate ad-hoc `std::env::var`**。开关 Feature 走自动 `COCO_FEATURE_skill_learning=1/0`(无需 EnvKey)。

### ModelRole
`ModelRole::Review`(现成,`common/types/src/provider.rs`),经合成 `AgentDefinition.model_role` 路由,`ModelRoles::get(ModelRole::Review)` 解析成 `(provider, api, model_id)`。**不新增角色、不 bare string、不 `title_model:String`**。

---

## 错误处理分级

| 层 | 位置 | 约定 |
|---|---|---|
| Tier-3 主干 | `coco-skill-learn`(root)、`app/query` 挂载点 | snafu + coco-error;但**turn-end 服务沿用 memory 的"outcome enum"约定**:`SkillReviewRuntime::finalize_turn` 返回 `SkillReviewReport`,`maybe_review`/`maybe_consolidate` 返回 `ReviewOutcome`/`CuratorOutcome`,**不返回 `Result`**。跨层/需分类的错误才用 `coco_error::BoxedError`。 |
| Tier-2 库 | `coco-skills`(序列化器/provenance/usage) | thiserror,`crate::Result<T>=Result<T,SkillsError>`;新序列化 fn 返回它,**不 anyhow**(`just check-error-policy` 强制)。`SkillsError::generic()` / `Io #[from]`。 |
| lock / spawn 边界 | `skill-learn/src/lock.rs`、`AgentHandle::spawn_agent` | lock 返回 `LockOutcome`(同 memory);`spawn_agent` 返回 `Result<_, String>`(既有约定),`run()` 把它折进 `ReviewOutcome::Failed`。 |
| Tier-1 | `app/cli` 装配 | anyhow 可用。 |

非测试代码零 `.unwrap()`;poisoned lock 用 `PoisonError::into_inner` 恢复(memory 同法)。若为新 StatusCode 需要,归属应新申请一个类目(seam 记 EventHub=14 已占;SkillLearning 另申),但本期 outcome-enum 路径基本不触发 wire ErrorCode。

---

## 分阶段实施计划(里程碑)

**Phase 0 — 遥测 + provenance(design #3,本设计前置)**
- `coco-skills`:`SkillProvenance`/`SkillAuthor` 字段 + `parse_skill_markdown` 读 `created_by`/`created_at`/`generated` + `bundled()`/mcp 默认 `User`;`serialize_skill_to_markdown`/`write_skill_file`(round-trip 测试)。
- `SkillUsageStats` 加 success/failure/last_status(保 alias);`record_invocation` 双臂调用挂 `QuerySkillRuntime::invoke_skill` Ok/Err。
- 门槛:round-trip(parse→serialize→parse 不丢字段)、老 SKILL.md 仍可解析。

**Phase 1 — review-fork 写路径(核心闭环)**
- 新 crate `coco-skill-learn`:`config`/`telemetry`/`notice`/`can_use_tool`(`SkillWriteHandle`)/`prompt`(`build_review_prompt`)/`service/review.rs`(copy-adapt `ExtractService`)/`runtime.rs`(`SkillReviewRuntime` + `TurnEndScheduler`,先只跑 review)。
- `common/types`:`ForkLabel::LearnSkill`、`AttachmentKind::SkillLearnedReminder`、`Feature::SkillLearning`。
- `coco-config`:`SkillLearnConfig` + resolve + EnvKey。
- `app/query`:`with_skill_review_runtime` + `build_skill_review_ctx_and_run` + 投影;`app/cli`:装配 + `install_agent`。
- 门槛:一次多步会话后,fork 在栅栏内写出 `created_by=agent` 的新 SKILL.md,`SkillChangeDetector` 热重载可见,越界写被 Deny 并 emit `ReviewToolDenied`。

**Phase 2 — consolidation(Curator)**
- `service/curator.rs`(copy-adapt `DreamService`)+ `lock.rs`(copy-adapt `lock.rs`)+ `build_curator_prompt`;`ForkLabel::CurateSkills`;`SkillCuratorService` 接入 `TurnEndScheduler`(review→curator)。
- 老化(读 `score_for`)、`.archive` 归档+备份、`created_by==Agent` 预过滤、force 回滚 mtime;可选 `/curate` 手动命令(Feature 门)。
- 门槛:阈值到达时 CAS lock 单飞,stale/archive 只作用于 agent skill,归档可回滚(备份存在),user/bundled skill 不受影响。

**Phase 3(可选)— self-improve 打分 + skill-recall**
- 用 design #3 的 success/failure 让 Curator 优先精炼"高频但低成功率"的 agent skill;`install_side_query` 接 `SideQuery` 做 turn 前 skill-recall(镜像 `MemoryRuntime::recall`,两步 structured-output→forced-tool,`ModelRole::Review`/`Subagent`)。

---

## 测试策略

- **companion `.test.rs`** 强制(`#[path="x.test.rs"] mod tests;`,禁 inline)。
- **Phase 0**:`serialize.test.rs` round-trip(含 CJK/多字节,走 `coco_utils_string`);`provenance` 缺省解析;`usage.test.rs` 双臂 record + `score_for_at` 注入时钟。
- **review.test.rs**(copy-adapt `extract.test.rs`):门链每分支(`Disabled`/`NoSignal`/`InProgress`+coalesce/`Throttled`/`BackoffActive`)、`effective_throttle` 退避封顶 32x、cursor 仅成功前进、`pending_trailing` 排空、`InProgressGuard` 取消不 wedge。用 `NoOp*` AgentHandle test double。
- **can_use_tool.test.rs**(copy-adapt memory):Read/Glob/Grep allow;越界 Write/Edit/apply_patch/相对路径/symlink 逃逸 fail-closed Deny;Bash 非只读 Deny;Curator 变体 `rm .md` 仅限 skills_dir。
- **curator.test.rs**(copy-adapt `dream.test.rs`):time/scan/session gate、CAS lock Held/Acquired/死 PID 回收、force 回滚 mtime、`created_by==Agent` 预过滤、归档产生备份且原文件消失、user skill 不动。
- **engine 集成**:mock runtime 断言 `build_skill_review_ctx_and_run` 在 `is_subagent`/`bare_mode`/`!turn_delivered`/`NoSignal` 下 skip;notices 投影成 `SkillLearnedReminder` + `SystemMessage`。
- **provenance 端到端**:fork 写出的文件 `created_by=agent`;Curator 拒改 `created_by=user` 文件(prompt + 栅栏双验证)。

---

## 风险 & 开放问题

1. **缓存取舍(Path A vs B)**:本设计走 Path B(memory 现成,消息前缀共享 + `ModelRole::Review` 路由 + 双环栅栏)。Path A(`ForkDispatcher`)给全 system-prompt 缓存平价,但**强制 pin 父 model ⇒ 无法用 Review 角色**,且外环 `allowed_write_roots` 未接进 `fork_dispatcher.rs`。**开放**:是否值得为 review fork 额外接一条 Path A + wire `iso.allowed_write_roots`?建议先 Path B,量到成本再优化。切忌给缓存共享 fork 设 `effort`(seam 记 effort='low' 曾把命中率 92.7%→61%)。
2. **text-only 结束回合不触发**:与 memory 同限。多步 skill 通常有工具流,影响有限;若要覆盖需抽 `run_skill_review_finalize` 共享 helper 双挂 `engine_terminal.rs`。**开放**。
3. **headless/SDK 无 drain pump**:turn-end fan-out 仅交互式 TUI 生效(cron 同限)。durable 的 Curator 阈值仍持久,下次交互会话补跑。**接受**(与 memory 一致)。
4. **skill 名路径派生 + rename 碰撞**:`name` 来自目录 basename;agent 生成新 umbrella 需选稳定唯一名(建议 `<topic-slug>` + 冲突检测),否则 first-wins dedup 会静默丢弃。**新逻辑需处理**。
5. **provenance 可被人手改**:`created_by` 是明文 frontmatter,用户可手动改写。栅栏保证"agent 只能写 skills_dir",但不能证明"某文件一定是 agent 写的"。审计靠 `origin_session_id` + `paths_written` 记录。**接受**(纵深防御而非密码学证明)。
6. **Curator LLM-merge 的破坏性**:合并 umbrella 可能吞掉有用细节。缓解:merge 前必备份到 `.archive`、只并 agent skill、`archive_retention_days` 保底可恢复。**开放**:是否需要 merge 的 dry-run/人工确认门?
7. **`SkillSource` 变体的跨 crate 影响**:若把 provenance 做成 `SkillSource` 变体(而非 `SkillDefinition` 字段),会波及 `coco-tools/skill_advanced.rs` 的穷举 match。本设计选**字段**方案避开,`SkillSource` 不动。
8. **与 `/skillify` 交互式创建的去重**:两条路径可能对同一主题各建一个 skill。**开放**:Curator 是否应把 model 驱动创建(`created_by=user`?)与 agent 创建做跨来源合并?本期 Curator 只碰 agent 物,交互式创建不受影响,留待观测。

---

> [← 设计③遥测+Provenance](design-03-skill-telemetry-provenance.md) · [返回索引](README.md) · [设计②IM 网关 →](design-02-im-gateway.md)
