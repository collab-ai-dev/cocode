# 自进化能力深度剖析（Hermes vs coco-rs）

> [← 功能对比矩阵](02-feature-comparison.md) · [返回索引](README.md) · [IM 接入深剖 →](04-im-integration-deep-dive.md)

---

# 自进化能力深度剖析

自进化(self-evolution)是 Hermes 相对于 coco-rs 差异最大、也最值得研究的一块。核心结论先行:**Hermes 拥有一个真正闭环、且对"能力(capabilities)"生效的自我改进回路;coco-rs 的自动闭环只覆盖"知识(knowledge)",不触碰技能、工具、prompt 或策略。** 换句话说,两边都实现了 `fork 子 agent → 写围栏 → 后台巩固` 这套模式,但 Hermes 把它同时指向了 **记忆(declarative)和技能(procedural)两个 store,并配了 provenance 分权与生命周期老化**;coco-rs 只把它指向了记忆一个 store,技能始终是人工手写的静态 markdown。

---

## 1. Hermes 的闭环学习机制(end-to-end)

Hermes 的"自我改进"不是单个模块,而是 **四个协作机制 + 一个共享 provenance 开关**。它们的分工可以用一句话概括:

> `memory` 记录 **"这个用户是谁"**(persona/preferences),`skills` 记录 **"如何为这个用户完成这类任务"**(procedural),`Curator` 管技能的生老病死,`session_search`/`Honcho` 负责跨会话召回与用户建模。

### 1.1 学习回路数据流(ASCII)

```
                         ┌──────────────────────────────────────────────┐
  user turn ──────────►  │        前台会话 AIAgent.run_conversation        │
                         │  _turns_since_memory++   (agent/turn_context)  │
  final reply ◄───────── │  _iters_since_skill++    (agent/turn_finalizer)│
                         └───────────────────┬──────────────────────────┘
                                             │ 交付回复且未被打断后
             nudge 触发 (计数 >= 10 && delivered)│
                                             ▼
                         ┌──────────────────────────────────────────────┐
                         │ _spawn_background_review  (daemon thread)       │
                         │   forked AIAgent, max_iterations=16             │
                         │   · 继承 parent runtime                          │
                         │   · pin _cached_system_prompt / session_id      │ ← 前缀缓存复用 ~26% 省钱
                         │   · _memory_write_origin = "background_review"   │ ← provenance 开关(linchpin)
                         │   · thread tool 白名单 = {skills[, memory]}      │
                         │   · _bg_review_auto_deny (拒危险命令)            │
                         └───────┬─────────────────────────────┬─────────┘
              _SKILL_REVIEW_PROMPT│               _MEMORY_REVIEW_PROMPT│
                                  ▼                                 ▼
        ┌───────────────────────────────────┐   ┌──────────────────────────────┐
        │ skill_manage  (procedural memory)  │   │ memory  (declarative)         │
        │  create → mark_agent_created()     │   │  MEMORY.md (~2200 chars)      │
        │          仅当 is_background_review()│   │  USER.md   (~1375 chars)      │
        │  patch/edit/write_file → bump_patch│   │  §-delimited, add/replace/rm  │
        │  write_guard: 拒 ext/bundled/pinned │   └──────────┬────────────────────┘
        │  read-before-write guard           │              │ on_memory_write(USER.md 'add')
        └──────────┬─────────────────────────┘              ▼
                   │ created_by="agent"  →  .usage.json  ┌────────────────────────────┐
                   ▼                                     │ Honcho dialectic 用户建模    │
        ┌───────────────────────────────────┐           │  pass0 who-is-this-person → │
        │ Curator  (会话启动触发, 默认 7 天)   │           │  pass1 gap synthesis →      │
        │  should_run_now → run_curator_review│           │  pass2 contradiction recon  │
        │  1. snapshot_skills (tar.gz 备份)   │           └────────────────────────────┘
        │  2. apply_automatic_transitions(纯) │
        │     active→stale→archived           │   所有消息 ──► SessionDB (SQLite FTS5 + trigram)
        │     仅 created_by=="agent"           │              ▲                    │
        │     pinned/cron 豁免, 永不 delete    │  title_generator│                  │ session_search
        │  3. [默认关] consolidate: LLM umbrella│  (aux LLM 命名) │                  ▼ (每轮按需召回)
        │     合并 + absorbed_into=<umbrella>  │           ┌────────────────────────────┐
        └───────────────────────────────────┘           │ 跨会话召回 anchored window   │
                                                         └────────────────────────────┘

   召回注入(每会话一次, 进 cached system prompt, 会话内绝不重载 → 保护前缀缓存):
        MEMORY.md/USER.md block + Honcho block + skills index ──► system prompt
```

### 1.2 nudge 机制:不是"提醒",而是"触发器"

关键澄清:Hermes 的 "nudge" **不是塞进对话里的一句 system-reminder,而是触发一个 fork 出来的 review agent 的信号**。

- `agent/agent_init.py` 设两个每-agent 计数器:`_memory_nudge_interval`(默认 10,配置 `memory.nudge_interval`)与 `_skill_nudge_interval`(默认 10,配置 `skills.creation_nudge_interval`)。
- `agent/turn_context.py` 每个用户轮次 `_turns_since_memory++`;`agent/turn_finalizer.py` 统计本轮工具迭代数 `_iters_since_skill++`。
- 当任一计数器越过阈值 **且** 本轮已交付最终回复(未被打断)时,`turn_finalizer` 在 **回复之后** 调用 `agent._spawn_background_review(snapshot, review_memory, review_skills)` —— 刻意放在回复之后,这样 review 永远不与模型对用户的注意力竞争。

### 1.3 后台自我改进 review:技能自动创建 + 自我改写

`agent/background_review.py::_run_review_in_thread` 是自动创建与自我改进的执行体:

- **fork 一个 AIAgent**:`max_iterations=16`、`skip_memory`、`compression_enabled=False`、`_end_session_on_close=False`,继承父 runtime,并 **pin 父的 `_cached_system_prompt` / `session_start` / `session_id`**,让出站请求命中同一 provider 前缀缓存(实测 ~26% 成本削减,PR #17276);若被路由到不同的 aux model,则改为回放紧凑的 `_digest_history`。
- **能力围栏**:安装线程级工具白名单(仅 skills[+memory]),其余工具运行时拒绝;`_bg_review_auto_deny` 审批回调拒绝危险命令。
- **prompt 取向**:`_SKILL_REVIEW_PROMPT` 明确写 "be ACTIVE — most sessions produce at least one skill update",并给出偏好顺序:**(1) patch 当前加载的 skill > (2) patch 已有 umbrella > (3) 加 `references/|templates/|scripts/` 支撑文件 > (4) 才 create 一个 class-level umbrella skill**。这套 "umbrella-building" 哲学是为了避免"一会话一技能"的碎片堆积。

**技能的"自我改进"其本质就是 `skill_manage(action=patch|edit|write_file)`**,由 review fork 执行并 `bump_patch`;新建技能走 `action=create`。写路径受两道守卫:`_background_review_write_guard`(拒绝写 external/bundled/hub/protected/pinned 技能)与 `_background_review_read_before_write_guard`(改之前必须先 `skill_view`)。文件:`tools/skill_manager_tool.py`、`tools/skill_usage.py`。

> 注意:Hermes 还有一条 **前台** 的显式技能创建路径 `/learn`(`agent/learn_prompt.build_learn_prompt`):它让前台 agent 收集来源并按 HARDLINE 标准(description ≤60 chars)写 SKILL.md。因为跑在前台,产出的技能是 **user-owned**,Curator 永不触碰。`agent/learning_graph.py` 把学到的技能与 MEMORY.md/USER.md §-chunk 渲染成"learning made visible"图;`agent/learning_mutations.py` 处理用户发起的编辑;`agent/insights.py` 生成只读用量分析。

### 1.4 provenance 开关:自动学 vs 用户教 的分权(整个设计的枢纽)

`tools/skill_provenance.py` 里的一个 `ContextVar skill_write_origin`(默认 `"foreground"`)是把 **"agent 自己学的"** 与 **"用户让我写的"** 干净分开的枢纽:

- review fork 把它置为 `"background_review"`。
- `skill_manager_tool` 中,`action=create` **仅当 `is_background_review()` 为真** 才调用 `mark_agent_created(name)` → 该技能在 `.usage.json` 里获得 `created_by="agent"`,成为 **Curator-managed**。
- 前台 / `/learn` 创建的技能保持 **user-owned、curator-immune**。

这让 Curator 可以对"agent 沉积物"激进地自动老化归档,却永远不会误伤用户资产。

### 1.5 Curator:确定性老化 + 可选 LLM umbrella 合并

`agent/curator.py`,由会话启动时 `maybe_run_curator`(`cli.py`、`gateway/run.py`)触发:

- `should_run_now` 门控:`curator.enabled`、非 paused、`last_run_at` 距今超过 `interval_hours`(默认 7 天,首跑推迟一个周期)。
- `run_curator_review`:
  1. `curator_backup.snapshot_skills` 打 tar.gz 备份;
  2. `apply_automatic_transitions`(**纯函数**,遍历 `agent_created_report()`):active→stale(超 `stale_after_days`)→archived(超 `archive_after_days`),用过则 reactivate;跳过 pinned + cron-referenced;never-used 有 grace floor;**归档到 `~/.hermes/skills/.archive/` 是最大破坏动作,永不 delete**;
  3. **仅当 `curator.consolidate` 开启(默认关)** 且存在候选时,`_run_llm_review` fork 一个 AIAgent(`max_iterations=9999`、`platform="curator"`、origin=`background_review`)跑 `CURATOR_REVIEW_PROMPT`:按前缀聚类,做 MERGE / CREATE-UMBRELLA / DEMOTE-to-support-file,被吸收的兄弟技能带 `absorbed_into=<umbrella>` 归档(驱动 cron skill-ref 迁移),产出结构化 YAML consolidations/prunings 块。
- 产物:`logs/curator/<ts>/{run.json,REPORT.md}` + `.curator_state`;aux 模型走 `auxiliary.curator.{provider,model}`。

### 1.6 技能用量遥测 sidecar

`tools/skill_usage.py` 独占 `~/.hermes/skills/.usage.json`,每技能记录 `{created_by, use_count, view_count, patch_count, last_used_at/last_viewed_at/last_patched_at, created_at, state(active/stale/archived), pinned, archived_at}`。`bump_use`(加载/引用)、`bump_view`(skill_view)、`bump_patch`(patch/edit/write_file)对 **所有** 技能记账作为可观测性;而 `mark_agent_created`/`set_state`/`set_pinned` 受 `is_curation_eligible`(agent-created-only)门控。这份遥测是 Curator 老化定时器的输入。

### 1.7 agent-curated memory(declarative)

`tools/memory_tool.MemoryStore` 管两份文件:`~/.hermes/memories/MEMORY.md`(target `"memory"`,~2200 chars)与 `USER.md`(target `"user"`,~1375 chars):§-delimited 条目、原子 temp+rename、文件锁、`_scan_memory_content` 注入/外泄守卫、external-drift 守卫、字符上限触发的 `consolidation_failure`(当轮强制合并)。`agent/memory_manager.MemoryManager` 编排内建 provider + 至多一个 external provider(`agent/memory_provider.py` 的 `MemoryProvider` ABC),hooks:`initialize/system_prompt_block/prefetch/sync_turn/on_memory_write/on_session_end`,走后台 `ThreadPoolExecutor`。**记忆块每会话只注入一次进 cached system prompt(`agent/system_prompt.py::format_for_system_prompt`),会话内绝不重载 —— 这是 cache-safety 策略。**

### 1.8 跨会话召回(FTS5 + LLM 命名/综合)

`hermes_state.SessionDB` 用 SQLite FTS5(`messages_fts` + trigram、BM25、损坏自愈)持久化每条消息。`tools/session_search_tool.session_search` 是单形状工具,做 Discovery/Scroll/Read/Browse:`_order_for_recall` 降权 cron 行(防自动化饿死人类会话)、lineage dedup、`get_anchored_view` 窗口/bookends、title-match、跨 profile。`agent/title_generator.py` 用 aux LLM(`call_llm`)在首次交换后异步产出 3-7 词标题供 title-match 召回;agent 自己把召回的 anchored window 综合进回答。

### 1.9 Honcho dialectic 用户建模

`plugins/memory/honcho/HonchoMemoryProvider`(MemoryProvider ABC):`sync_turn` 把 user/assistant 轮次记入 Honcho(分块、多线程);`on_memory_write` 把 USER.md 的 'add' 镜像成 Honcho `create_conclusion`;`_run_dialectic_depth` 跑至多 N 次 `.chat()`/`dialectic_query`,带早退(`_signal_sufficient`):pass0 "who is this person?(偏好/目标/工作风格)"、pass1 self-audit/gap synthesis、pass2 reconciliation/contradiction check → 终综合,每 pass 有独立 reasoning level。结果经 `system_prompt_block`/`prefetch` 注入。**这不是向量库,而是真正的多轮辩证式用户建模。**(已知限制:`on_memory_write` 只镜像 target 'user' 的 'add' 且丢弃 metadata;cron 会话完全跳过 memory provider,即计划运行时的学习被有意丢弃。)

---

## 2. coco-rs 现状:有什么,以及关键地缺什么

coco-rs 在 `coco-memory`(记忆)与 `coco-skills`(技能)两个 crate 上,是 TS 版 Claude Code 设计的高保真移植。

### 2.1 coco-rs 已有(HAVE)

| 能力 | 实现 | 触发/门控 |
|---|---|---|
| **自动抽取**(turn-end 记忆捕获) | `memory/src/service/extract.rs` `ExtractService`:fork 一个 memdir 围栏子 agent(`max_turns=5`,`ModelRole::Memory`),从 cursor 起写/改记忆文件 | 门控顺序:`extraction_enabled` → `Feature::AutoMemory` → coalesce → skip-if-main-agent-wrote → throttle(默认每轮,失败指数退避至 32x);cursor 仅成功时前进;60s drain |
| **auto-dream 巩固**(KAIROS) | `memory/src/service/dream.rs` `DreamService`:4 阶段 Orient/Gather/Consolidate/Prune,合并条目、消解矛盾、剪枝 MEMORY.md 指针,甚至可 `rm` .md | 3 门控(≥24h / ≥5 session / ≤1 次 scan/10min)+ PID+mtime CAS 锁 + 进程内原子;`/dream` = `force()` 绕门控但保留锁、回滚 mtime |
| **KAIROS 日志模式** | `memory/src/kairos/daily_log.rs`:append-only `logs/YYYY/MM/YYYY-MM-DD.md` + rollover watcher | KAIROS 模式下 **禁用** auto-dream |
| **session memory**(9 段摘要) | `memory/src/service/session.rs`:含 `# Learnings` 与 `# Errors & Corrections` 段,供 compaction/resume | token 增长(10k init/5k update)+ 活动(≥3 tool calls 或自然中断) |
| **LLM 排序召回** | `memory/src/recall.rs`:`ModelRole::Memory` side-query 返回 ≤5 文件;`PrefetchState` 去重 + 60KB 预算 | recency fallback **已删** —— 无 ranker 或出错时静默,不产噪声 |
| **4 型分类 + MEMORY.md 索引** | User/Feedback/Project/Reference 型 markdown + 模型策展的 MEMORY.md 索引 | runtime 只读+截断(200 行/25KB),**从不自动重建** |
| **团队记忆同步** | `memory/src/team_sync/`(pull/push/watcher/secret-scan)已实现 | **但未从 app/cli 接线**;`team_memory_enabled` 目前只切换 Combined system-prompt 变体 |
| **技能系统**(静态 markdown) | `skills/src/lib.rs` `SkillManager`:6 源加载、桥接 slash 命令 + Skill 工具(1% context 预算)、Inline vs Fork、`$ARGUMENTS`/`$(shell)` 展开、bundled 抽取(O_EXCL\|O_NOFOLLOW) | paths-glob 激活(部分)、hot-reload watcher(部分) |
| **人在环技能创作** | `skills/src/bundled/skillify.rs`(分析会话→多轮 AskUserQuestion 访谈→草稿→**确认后** 才写 SKILL.md)、`run_skill_generator.rs`(引导创建/精炼) | `skillify` UserType-gated,`run_skill_generator` Feature-gated;**grep 确认零自动调用点** |
| **记忆层审计** | `skills/src/bundled/remember.rs`:读所有记忆层,提出向 CLAUDE.md / CLAUDE.local.md / team memory 的晋升建议 | **只提建议,未经批准不落地** |

值得强调:coco-rs 已经具备了搭建 Hermes 式回路所需的 **几乎全部底层原语**——`SwarmAgentHandle` 的 **Fork 模式**(共享父前缀缓存,`FORK_PLACEHOLDER` 重写 tool_result,`is_in_fork_child` 递归守卫)对应 Hermes 的 review fork;记忆抽取的 **two-ring 写围栏**(`memory/src/can_use_tool.rs` 的 `can_use_tool` 内环 + `allowed_write_roots` 外环)对应 `_memory_write_origin` + write guard;`DreamService` 的后台巩固对应 KAIROS dream。**只是这些原语全部只指向记忆,没有一个指向技能。**

### 2.2 coco-rs 关键地缺失(ABSENT,逐条对照)

| 缺失点 | Hermes 对应 | coco-rs 证据 |
|---|---|---|
| **能力(技能/工具/prompt/策略)的自动闭环** | background review + Curator | 唯一的自动写回路是 **知识**(extract→dream→recall);技能/工具/prompt/策略从不被自动修改 |
| **技能自动创建 + 自我改写** | review fork 的 `skill_manage(create/patch)` | `skillify`/`run-skill-generator` 全是 **手动、人在环** 的 slash 命令;grep 确认 **零自动调用点**;`remember` 只提建议 |
| **provenance 分权开关** | `tools/skill_provenance.py` 的 `ContextVar` + `created_by="agent"` | coco-rs 技能 **无 created_by 概念**,谈不上 agent-created vs user-owned 的区分 |
| **技能生命周期老化(Curator)** | `apply_automatic_transitions` active→stale→archived | coco-rs **无 Curator、无 `.usage.json` 遥测、无 archive/restore、无 stale 定时器** |
| **outcome/遥测驱动的适应** | `.usage.json` 的 use/view/patch 计数喂给 Curator | `MemoryEvent`/OTel 遥测发到 OTel 后 **从不回流** 改变技能或记忆写策略;无 reward/eval 信号 |
| **结构化用户模型** | Honcho dialectic(who → gap → contradiction) | 唯一的用户建模原语是自由格式的 `[user]` 记忆分类型,写成普通记忆文件;**无偏好推断/persona 引擎** |
| **KAIROS 蒸馏路径** | (Hermes 无此坑) | KAIROS 日志 prompt 承诺"nightly 进程把日志蒸馏进 MEMORY.md",但 auto-dream 在 KAIROS 模式被禁用,**该蒸馏路径实际未接线** |

**一句话定性:coco-rs = 检索增强记忆(对知识自动)+ agent 辅助、人工把关的技能创作。它缺的正是 Curator/learning-loop 所暗含的两样东西——自主的、outcome 驱动的能力改进回路,以及结构化的用户模型。**

---

## 3. 差距分析 + 可借鉴点(高层)

### 3.1 差距的本质:同一套模式,只用了一半

coco-rs 与 Hermes 在自进化上的分野不是"缺原语",而是"**没把已有原语指向技能,也没加上分权与老化**"。Hermes 的洞见是把 `fork(共享前缀缓存) → 写围栏 → 后台巩固` 这套模式 **复制两份**:一份给记忆(coco-rs 已有),一份给技能(coco-rs 全缺),并用一个 `ContextVar` provenance 开关把"agent 自学"与"用户教"分权,再用一个纯函数老化器 + 用量 sidecar 让技能可以被安全地激进回收。

| 维度 | Hermes | coco-rs | 差距量级 |
|---|---|---|---|
| 记忆自动闭环(capture→consolidate→recall) | 有 | **有**(extract/dream/recall) | 基本对齐(coco-rs recall 的静默退化更稳健) |
| 技能自动创建 | 有(nudge → review fork) | 无(仅手动 skillify) | **大** |
| 技能自我改写(patch) | 有(review fork mutation) | 无 | **大** |
| 技能生命周期管理 | 有(Curator 老化 + LLM 合并) | 无 | **大** |
| provenance 分权 | 有(created_by=agent) | 无 | **大** |
| 用量遥测驱动策展 | 有(.usage.json → Curator) | 无(遥测不回流) | **中-大** |
| 结构化用户建模 | 有(Honcho dialectic) | 无(仅 `[user]` 自由文本) | **中**(可选,取决于产品定位) |
| 会话内 cache-safety(记忆只注一次) | 有 | 有(且更严格) | 对齐 |

### 3.2 可借鉴点(概览,详细方案见 05 节)

按"改动小、契合 coco-rs 现有分层"排序,几个明确可借鉴的方向:

1. **把 memory 的 fork+围栏+dream 模式复制到 skills**:coco-rs 的 `SwarmAgentHandle` Fork 模式已能共享父前缀缓存,`ExtractService` 的 memdir 写围栏可直接类比出一个"skill 写围栏"。这是把静态 markdown 技能升级为可自动创建/改写的 procedural memory 的最短路径。
2. **引入 provenance 开关**:仿 `tools/skill_provenance.py`,用一个绑定到 fork 子 agent 的写-origin 标记(coco-rs 里可挂在 `ToolUseContext` 或 spawn constraints 上),区分 agent-created vs user-authored 技能——这是让自动老化"安全"的前提。
3. **加技能用量 sidecar + 一个 Curator 等价物**:仿 `.usage.json` + `apply_automatic_transitions`(纯函数老化,永不 delete、只 archive、pinned 豁免、tar.gz 备份),把 coco-rs 现有的 OTel `MemoryEvent` 遥测从"只观测"变成"回流策展"。
4. **nudge 触发器复用 turn-end scheduler**:coco-rs 已有 `MemoryRuntime::finalize_turn` 的 turn-end 调度器,天然是挂 skill-review nudge 的位置,无需新建生命周期。
5. **(可选)结构化用户模型**:若产品需要,`MemoryProvider` ABC 式的可插拔外部 provider(Honcho 那样的多轮辩证建模)可作为 coco-rs `Feature` 门控下的扩展点。

同时要吸取 Hermes 的教训(05 节展开):Hermes 的回路是 **process-local、best-effort**(daemon 线程 + 宽 `except: pass`,进程退出即丢失当轮学习,无持久队列),且 `created_by="agent"` 完全依赖 `is_background_review()` 被正确设置——任何漏绑 ContextVar 的代码路径都会误分类技能。coco-rs 若移植,应利用其已有的 `coco-tasks` 持久化 + `Feature` 门控 + 类型安全枚举把这些脆弱点做扎实,而不是照搬 Python 的 duck-typing 与静默降级。

> 具体的接口设计、crate 归属、迁移步骤与风险缓解,见后续 **第 05 节(详细建议)**。

---

> [← 功能对比矩阵](02-feature-comparison.md) · [返回索引](README.md) · [IM 接入深剖 →](04-im-integration-deep-dive.md)
