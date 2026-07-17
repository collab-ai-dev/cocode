# 设计 ③:技能遥测 + Provenance(快速收益 / T1.1 前置)

> [← 可落地建议](05-recommendations.md) · [返回索引](README.md) · [设计①学习闭环 →](design-01-skill-learning-loop.md)

> ## ⚠️ 评审修正(权威 — 与下文冲突处以本节为准)
>
> 本设计经对抗式评审(对照真实 seam / 代码已复核)后修正如下:
>
> 1. **`SkillWriteHandle` 不放在 `coco-skills`。** `coco-skills` 是被 `app/query`/`commands`/`tui`/`coco-tools` 广泛消费的纯 loader(其 `Cargo.toml` 仅依赖 file-ignore/types/config/error/file-watch/frontmatter/system-reminder/utils-common)。把 fork 写围栏 + shell 安全 + apply-patch 塞进 loader 会污染依赖树,且与设计 ① 冲突(两份同名 handle)。**决议:写围栏 handle 归 `coco-skill-learn`(设计 ①,镜像 `memory/src/can_use_tool.rs`);本设计 ③ 只交付 (a) `SkillDefinition` 的 provenance 字段 + SKILL.md 序列化、(b) telemetry 统计扩展、(c) 一个 `on_disk_origin()` provenance 谓词供 handle 消费。** 删去 §组件3 中给 `coco-skills` 新增 `coco-tool-runtime`/`coco-shell-parser`/`coco-apply-patch` 依赖的部分。
> 2. **YAML frontmatter 需要「写出」能力。** `coco-frontmatter` 现在只解析、不序列化。`serialize_skill_to_markdown`/`stamp_agent_origin` 需引入 `serde_yaml`(或等价)并保证与 `parse_skill_markdown` 的 key 往返一致(含当前被静默丢弃的 author/license/tags)。
> 3. **`now_ms()` 的 `Option<i64>` 守卫必须保留。** `skills/src/usage.rs:101/120/194` 现有逻辑在系统时钟早于 1970 时**拒绝记录**,以免污染 `score_for` 衰减。新增的 `record_invocation/record_view/record_patch` 必须沿用这个 refuse-on-None 守卫。
> 4. **遥测发射器不必做 3-impl trait。** 作为「快速收益」,沿用现有 `emit_tengu_feature_*` 式的轻量 OTel 发射先例即可,暂不引入三实现的 emitter trait(避免过度设计)。
> 5. **无需新增 Feature 门(正确)。** skills 遵循「configured = enabled」,本设计不加 `Feature` 变体——这与评审一致,保持原样。

---

# DESIGN #3：Skill 遥测 + 溯源（telemetry + provenance）

> 快赢项，也是 DESIGN #1（自动创建 / 自我改进的 skill 学习闭环）的**前置依赖**。本设计只交付「基础设施」：把 skill 的使用数据变得足够丰富、给每个 skill 打上「谁创建的」标记、并复刻 memory 的写围栏保证「用户 skill 永不被 Curator 触碰」。#1 只负责 fork 学习 agent 并调用这里铺好的 API。

## 目标 & 非目标

**目标**

1. （遥测 A）在**现有** `skills/src/usage.rs` 存储上做增量扩展，记录每个 skill 的：成功调用数、失败调用数、view 数、patch 数、last_used / last_patched、最近一次 outcome。填平「`record` 只在成功时触发、失败完全不可见」的空洞。
2. （溯源 B）给 `SkillDefinition` 增加 `origin`（`user` / `agent`）+ `created_by` / `created_at` 三个字段与对应 frontmatter key；**存量 skill 默认 `origin=user`**，Curator 只能管理 `origin=agent` 的 skill。
3. 复刻 `memory/src/can_use_tool.rs` 的写围栏，产出 `SkillWriteHandle: CanUseToolHandle`：只有携带该 handle 的 review-fork 才能在 `skill_dir` 下创建/改写 skill，且**只能改 agent-owned skill**、**read-before-write**。围栏通过 `AgentSpawnRequest` 上的**显式 typed spawn constraint** 传入，绝不使用 ContextVar / thread-local。
4. 镜像 memory 的双通道反馈：`SkillEvent` + `SkillTelemetryEmitter`（OTel-only，不回灌 loop）与持久化 `usage.json`（loop 真正读取的数据面）。

**非目标**

- 不实现学习闭环本体（fork 触发、prompt、self-improve 决策）—— 那是 #1。本设计只提供 `record_patch` / `stamp_agent_origin` / `SkillWriteHandle` 等待 #1 挂载的原语。
- 不实现「struct → SKILL.md 全量序列化器」（`write_skill_markdown`）——该 seam 目前不存在，归 #1；#3 只提供「往已存在文件里盖 origin 章」的 `stamp_agent_origin`。
- 不新增 `Feature` 变体。skills 是 “configured = enabled” 子系统（root CLAUDE.md 明确 skills 非 Feature），遥测/溯源是被动基础设施，恒开。
- 不改动 `score_for` 语义、不改动 `/` autocomplete 的 “recently used” 排序（`usage_count` 保持成功计数不变）。
- 不新增 `SkillSource` 变体（该 enum 被 `coco-tools::skill_advanced` 跨 crate 穷尽匹配，加变体是跨 crate 破坏性改动）——溯源用**正交的 `origin` 字段**表达。

## 现状（基于 seam，已有什么）

| 能力 | 现状 | 文件 |
|---|---|---|
| 使用统计存储 | `SkillUsageStats { usage_count, last_used_at_ms }`，`UsageFile { skills: HashMap<String, SkillUsageStats> }`，落盘 `<config_home>/skill_usage.json`，原子写（`NamedTempFile`+`persist`）+ 60s 进程内 debounce（key = skill_name） | `skills/src/usage.rs` |
| 写入点 | `record(config_home, name)` **仅在成功时**触发：`app/query/src/skill_runtime.rs:476-482`（模型调 Skill 工具，`spawn_blocking`）+ `commands/src/lib.rs:519`（用户敲 `/`，`CommandType::Prompt`） | 同上 |
| 读取点 | `load_all` / `score_for`（7 天半衰期，0.1 下限）——**只**喂 `/` autocomplete 的 “recently used” | `commands/src/lib.rs:403`、`app/cli/tui_runner.rs` |
| 溯源 | **无**。`SkillDefinition` 无 `created_by`/`origin`/`created_at`；frontmatter 的 `author`/`license`/`tags` 被 `parse_skill_markdown` 静默丢弃（`lib.test.rs:1173` 断言）。唯一来源信号是 `SkillSource`（scope，非 authorship）+ `version` | `skills/src/lib.rs:56-214, 982-1159` |
| 写围栏模板 | `memory/src/can_use_tool.rs` 三条策略 + symlink-aware `path_under_root`（lexical normalize + `realpath_deepest_existing`，fail-closed）。trait `CanUseToolHandle` 在 `core/tool-runtime/src/can_use_tool.rs`，step 3.5 于 built-in `check_permissions` 之前分发 | `memory/src/can_use_tool.rs`、`core/tool-runtime/src/can_use_tool.rs` |
| 双环围栏字段 | `AgentSpawnConstraints { max_turns, allowed_write_roots }`（外环）+ `AgentSpawnRequest.can_use_tool`（内环） | `core/tool-runtime/src/agent_handle.rs:44` |
| 遥测双通道模板 | `MemoryEvent` + `MemoryTelemetryEmitter`（Noop/Tracing/Otel，`target: coco_memory::telemetry`，`event_type = tengu_*`）——fire-and-forget，不回灌 loop | `memory/src/telemetry.rs` |
| 错误层级 | coco-skills 为 Tier-2（thiserror `SkillsError`，`generic()` / `Io #[from]`，`ErrorExt`） | `skills/src/error.rs` |

结论：**遥测存储、原子写、写围栏 trait、双环 spawn 约束、双通道遥测模板全部已存在**。#3 = 在既有 `usage.rs` 上加字段 + 加事件、在 skills crate 新增一个 `can_use_tool.rs`（照抄 memory）、给 `SkillDefinition` 加 3 个 serde-default 字段并在 parser 里读它们。零引擎改动。

## 架构总览（crate 归属 + ascii 数据流）

新增/改动全部落在 **`coco-skills`（root 层）**，加一处 `app/query` 写入点接线、一处 `coco-config` 子配置。层 DAG 不变（`coco-skills` 需新增对 `core/tool-runtime` 的依赖以实现 `CanUseToolHandle`；core 在 root 之下，合法，与 `coco-memory` 完全同构）。

```
                         ┌──────────────────── coco-skills (root) ─────────────────────┐
写入面 (data plane)      │                                                              │
 invoke_skill(Ok)  ─────▶│ usage::record_invocation(Success) ┐                          │
 invoke_skill(Err) ─────▶│ usage::record_invocation(Failure) ├─▶ apply() ─┐            │
 read_skill_body   ─────▶│ usage::record_view                ┘            │            │
 学习 fork(#1) 写盘 ────▶│ usage::record_patch  + serialize::stamp_agent_origin         │
                         │                                    │            ▼            │
                         │        (进程内 file Mutex + 60s debounce + 原子写)           │
                         │                                    └──▶ <config_home>/skill_usage.json
读取面 (control plane)   │                                                              │
 Curator / #1      ◀─────│ usage::load_all + on_disk_origin(path)==Agent → 只选 agent   │
                         │                                                              │
遥测通道 (OTel-only)     │ SkillTelemetryEmitter::emit(SkillEvent::*) ──▶ tracing/OTel  │
                         │                                        (不回灌 agent loop)   │
围栏 (provenance fence)  │ SkillWriteHandle: CanUseToolHandle                           │
                         └───────────────┬──────────────────────────────────────────────┘
                                         │ 作为 typed spawn constraint 传入
                                         ▼
   #1 fork: AgentSpawnRequest{ can_use_tool: Some(skill_write_handle)  (内环)
                               constraints: Some({ allowed_write_roots:[skill_dir] }) (外环)
                               definition: AgentDefinition{ model_role: Some(Review) } }
                                         │
                                         ▼  step 3.5 于 core/tool-runtime/execution
                          SkillWriteHandle::check(tool,input,ctx)
                            Read/Glob/Grep → Allow(记录已读路径)
                            Write/Edit/apply_patch → 路径在 skill_dir 且 目标 origin!=User
                                                     且 (Edit 需 read-before-write) → Allow
                            else → Deny + SkillEvent::SkillWriteDenied
```

## 详细设计

### 组件 1 —— 遥测存储扩展（`skills/src/usage.rs`）

#### 1.1 结构体增量（back-compat 关键）

```rust
/// 单次调用结果，从 invoke_skill 的 Ok / Err 两臂显式传入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutcome {
    Success,
    Failure,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillUsageStats {
    // —— 既有字段，语义与 wire 形状完全不动 ——
    #[serde(default, alias = "usageCount")]
    pub usage_count: i64,          // 成功调用数：仍是 score_for 的唯一输入
    #[serde(default, alias = "lastUsedAt")]
    pub last_used_at_ms: i64,

    // —— 新增字段：全部 #[serde(default)]，旧文件缺键即取 0 ——
    #[serde(default)]
    pub failure_count: i64,        // 失败调用数（此前完全不可见）
    #[serde(default)]
    pub view_count: i64,           // 被预加载/查阅但未执行
    #[serde(default)]
    pub patch_count: i64,          // 被学习 fork 改写的次数
    #[serde(default)]
    pub last_patched_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<SkillOutcome>,
}

impl SkillUsageStats {
    pub fn total_invocations(&self) -> i64 { self.usage_count + self.failure_count }
    /// 成功率，供 Curator 判定「misfiring skill」；无调用时 None。
    pub fn success_rate(&self) -> Option<f64> {
        let t = self.total_invocations();
        (t > 0).then(|| self.usage_count as f64 / t as f64)
    }
}
```

**为什么 `usage_count` 不改语义**：`score_for` / `score_for_at`（`usage.rs:193-211`）读 `usage_count` 驱动 autocomplete 排序。若把失败也算进去会污染 “recently used”。故 `usage_count` 保持 = 成功计数，失败进独立 `failure_count`。`score_for` 一行不改。

**back-compat**：新字段全 `#[serde(default)]` → 存量 `skill_usage.json`（只含 `usage_count`/`last_used_at_ms`）照常反序列化；对 upstream `globalConfig.skillUsage` 仍是「加键」，upstream 读时忽略未知键。别名 `usageCount`/`lastUsedAt` 保留。**逃生口**：若将来需破坏形状，另开 sibling 文件 `skill_telemetry.json`，`skill_usage.json` 维持 wire-compat；本设计选择加字段（seam 明示「extend, don't rebuild」）。

#### 1.2 写入 API（重构 `record` 为统一 mutator）

```rust
enum UsageMut { Invocation(SkillOutcome), View, Patch }

/// 唯一落盘点：进程内 file Mutex 序列化 read-modify-write + (可选)debounce + 原子写。
fn apply(config_home: &Path, skill_name: &str, event: UsageMut) { /* 见 1.3 */ }

pub fn record_invocation(config_home: &Path, name: &str, outcome: SkillOutcome) {
    apply(config_home, name, UsageMut::Invocation(outcome));
}
pub fn record_view(config_home: &Path, name: &str)  { apply(config_home, name, UsageMut::View); }
pub fn record_patch(config_home: &Path, name: &str) { apply(config_home, name, UsageMut::Patch); }

/// 既有两处 caller 保持不动：等价 record_invocation(.., Success)。
pub fn record(config_home: &Path, name: &str) {
    record_invocation(config_home, name, SkillOutcome::Success);
}
```

每种事件的 mutator 语义：

| 事件 | 变更 |
|---|---|
| `Invocation(Success)` | `usage_count += 1`; `last_used_at_ms = now`; `last_status = Some(Success)` |
| `Invocation(Failure)` | `failure_count += 1`; `last_status = Some(Failure)`（**不动** `usage_count`，保 score 稳定） |
| `View` | `view_count += 1` |
| `Patch` | `patch_count += 1`; `last_patched_at_ms = now` |

#### 1.3 并发与写路径（任务显式要求，回答「是否复用 lock.rs」）

- **复用**：原子写（`NamedTempFile::new_in` + `persist`）——沿用现有 `write_atomic`，形状永不半写。
- **新增（关内进程 lost-update 竞态）**：现状只有 debounce map 有 `Mutex`，read-modify-write **无锁**——两个并发 `record()`（不同 skill）交错读/写会丢增量。学习 fork（后台 tokio 任务）与主循环同进程并发写同一文件，必须收口。加一个进程级 `static FILE_LOCK: OnceLock<Mutex<()>>`，在 `apply` 内包住「读 JSON → 改 → 原子写」。
- **debounce 按 (name, kind) 分桶**：现状 debounce key = skill_name，会让「同 60s 内的一次 view 和一次 invocation」互相顶掉。改 key 为 `format!("{name}\u{0}{kind}")`。`Patch` **不 debounce**（学习 fork 罕发，且计数必须精确）；`Invocation`/`View` 维持 60s。
- **不采用 `memory/src/lock.rs`（PID+mtime CAS）**：该机制是为把「一次昂贵的 dream consolidation」在**跨进程**单飞而设，附带 lock 文件 + 死 PID 回收 + mtime = lastConsolidatedAt 语义。计数器自增不需要跨进程互斥；跨进程偶发丢增量对**阈值型**生命周期决策无影响（best-effort，与现有 module doc 一致），且引入 CAS 会把所有会话的遥测写强制串行、平白多一个锁文件。**结论：进程内 Mutex 足矣，跨进程沿用原子重命名的 best-effort。**
- **线程模型不变**：`record_*` 仍为 sync 阻塞 I/O，caller 在 async 里 `spawn_blocking`（两处既有 caller 已如此）。

#### 1.4 读取 API（供 Curator / #1）

`load_all(config_home) -> HashMap<String, SkillUsageStats>` 已存在，直接复用。新增单点查：

```rust
pub fn load_stats(config_home: &Path, name: &str) -> Option<SkillUsageStats>;
```

### 组件 2 —— 溯源字段（`skills/src/lib.rs`）

#### 2.1 新枚举 + `SkillDefinition` 增量

```rust
/// Skill 的 authorship 溯源。与 SkillSource（scope）正交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    /// 人类作者 / bundled / MCP —— Curator 绝不改写。存量默认落到这里。
    #[default]
    User,
    /// 由 skill-learning review-fork 创建或最近改写。
    Agent,
}
```

`SkillDefinition`（`lib.rs:56-156`）追加：

```rust
    /// 溯源：user-authored（对 Curator 免疫）vs agent-created。
    /// #[serde(default)] ⇒ 一切存量 SKILL.md 落 User。
    #[serde(default)]
    pub origin: SkillOrigin,
    /// 创建者标签（fork_label 或 agent_id），仅 origin==Agent 时置。
    /// frontmatter: `created-by`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// RFC3339 创建时间。frontmatter: `created-at`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
```

#### 2.2 parser 读取（`parse_skill_markdown`，`lib.rs:982-1159`）

在现有 `lookup_str` 之后加三处读取，并写进返回的 `SkillDefinition`：

```rust
    let origin = lookup_str(&["origin"])
        .and_then(|s| match s.trim() {
            "agent" => Some(SkillOrigin::Agent),
            "user"  => Some(SkillOrigin::User),
            _ => None,               // 未知值 → fail-safe 到 User
        })
        .unwrap_or_default();        // 缺键 → User（存量免疫）
    let created_by = lookup_str(&["created-by", "created_by"]);
    let created_at = lookup_str(&["created-at", "created_at"]);
```

**迁移 / back-compat**：无需任何文件迁移。缺 `origin:` 键 = `SkillOrigin::User` = 对 Curator 免疫。这是「用户 skill 永远安全」的落地保证。`bundled()`（`bundled.rs:54`）与 `mcp_builders.rs` 显式补 `origin: SkillOrigin::User, created_by: None, created_at: None`（MCP skill 本就被排除在写/溯源逻辑外）。

#### 2.3 盖章原语（`skills/src/serialize.rs`，新文件）

```rust
/// 读回一个已存在的 SKILL.md，把 `origin: agent` + created-by/created-at
/// 写进/覆盖其 YAML frontmatter，原子重写。#1 在 fork 写盘后对
/// AgentSpawnResponse.paths_written 里每个 skill 调它，使 origin 章
/// 权威化（不依赖模型是否自觉写 frontmatter）。
pub fn stamp_agent_origin(path: &Path, created_by: &str) -> crate::Result<()>;

/// 只读 frontmatter 的 origin（前 ~30 行即可），任何解析/IO 失败 → User。
/// 围栏与 Curator 都用它判定「这是不是 agent-owned」。fail-closed 到 User
/// = 拿不准就当用户 skill、拒绝改写。
pub fn on_disk_origin(path: &Path) -> SkillOrigin;
```

> 注：`write_skill_markdown(dir, &SkillDefinition) -> crate::Result<PathBuf>`（全量 struct→SKILL.md）归 #1；#3 只交付「盖章」与「读 origin」这两个最小原语。

### 组件 3 —— 写围栏（`skills/src/can_use_tool.rs`，新文件，照抄 memory）

```rust
pub fn create_skill_write_handle(skill_dir: PathBuf) -> CanUseToolHandleRef;
pub fn create_skill_write_handle_with_telemetry(
    skill_dir: PathBuf, telemetry: Arc<dyn SkillTelemetryEmitter>,
) -> CanUseToolHandleRef;

struct SkillWriteHandle {
    skill_dir: PathBuf,
    /// read-before-write：本 fork 内已 Read 过的规范化路径集合。
    read_paths: std::sync::Mutex<HashSet<PathBuf>>,
    telemetry: Arc<dyn SkillTelemetryEmitter>,
}
```

`CanUseToolHandle::check(tool_name, input, ctx)` 策略（比 memory 多一层「provenance 免疫」+「read-before-write」）：

| tool | 决策 |
|---|---|
| `Read` | 记录规范化路径进 `read_paths` → `Allow` |
| `Glob` / `Grep` | `Allow` |
| `Bash` | `coco_shell_parser::safety::is_known_safe_command`（整条 pipeline 逐段、fail-closed 于重定向/子 shell）→ `Allow`，否则 `Deny` |
| `Edit` | 路径经 `ctx.cwd` 解析后须 `.md` 且在 `skill_dir` 下（symlink-aware `path_under_root`）；目标**须已存在**且 `on_disk_origin != User`（否则 = 用户 skill，`Deny` 免疫）；且路径**须在 `read_paths` 内**（read-before-write，否则 `Deny`）→ `Allow` |
| `Write` | 同上 containment；若目标已存在则 `on_disk_origin != User`（用户 skill 免疫 `Deny`），若为**新文件**则 `Allow`（创建，随后由 `stamp_agent_origin` 盖章）|
| `apply_patch` | 每个 effect path 均 `.md`、在 `skill_dir` 下、且（存在的）`origin != User` → `Allow`，否则 `Deny` |
| 其他 | `Deny` |

每次 `Deny` → `telemetry.emit(SkillEvent::SkillWriteDenied { tool_name, reason })`（镜像 memory 的 `ExtractionToolDenied`）。

**containment 复用**：memory 的 `path_under_root` + `lexical_normalize` + `realpath_deepest_existing`（symlink-aware、fail-closed 于 ELOOP/悬垂/非对称）是战功卓著的实现。**建议抽到 `coco_utils_absolute_path`** 一个 `contains_symlink_aware(root, candidate) -> bool` 供 memory 与 skills 共用（避免复制，遵「no single-use helpers」）；若不想扩大改面，则在 `skills/src/path.rs` 拷贝一份，并在两处留 TODO 指向抽取任务。

**双环防御（显式 typed constraint，非 ContextVar）**：#1 的 fork 请求同时设置

```rust
AgentSpawnRequest {
    can_use_tool: Some(create_skill_write_handle_with_telemetry(skill_dir, tel)), // 内环（权威）
    require_can_use_tool: true,
    constraints: Some(AgentSpawnConstraints {
        max_turns: Some(N),
        allowed_write_roots: vec![skill_dir],                                     // 外环
    }),
    definition: Some(Arc::new(AgentDefinition {                                   // ModelRole 路由
        agent_type: AgentTypeId::Custom("skill-curator".into()),
        model_role: Some(ModelRole::Review),
        ..Default::default()
    })),
    fork_label: Some(ForkLabel::/*#1 新增*/ LearnSkill),
    skip_transcript: true,
    ..Default::default()
}
```

围栏在 `core/tool-runtime/src/execution` step 3.5、于工具内建 `check_permissions` 之前分发（已有机制）。**溯源保证**：因为写只能落在 `skill_dir`（两环）、且 Edit/覆盖只对 `origin=Agent` 放行、创建后立即被 `stamp_agent_origin` 盖 `origin=agent` 章——任何 `origin=agent` 的 skill 文件**只可能**来自被围栏的 fork，用户 skill 全程免疫。这就是把 Hermes 报告里「用 typed spawn constraint 而非 thread-local」落地的方式：所有权限都是 `AgentSpawnRequest` 上的字段，无环境上下文。

### 组件 4 —— 遥测双通道（`skills/src/telemetry.rs`，新文件，镜像 memory）

```rust
pub enum SkillEvent {
    SkillInvoked { name: String, source: &'static str, context: &'static str,
                   outcome: SkillOutcome, duration_ms: i64, prompt_chars: i64 },
    SkillViewed  { name: String },
    SkillWriteDenied { tool_name: String, reason: &'static str },
    SkillCreated { name: String, created_by: String },
    SkillPatched { name: String },
    UsageWriteFailed,
}

pub trait SkillTelemetryEmitter: Send + Sync { fn emit(&self, event: SkillEvent); }

#[derive(Debug, Default)] pub struct NoopEmitter;      // 测试默认
#[derive(Debug, Default)] pub struct TracingEmitter;   // 生产：target=coco_skills::telemetry, event_type=tengu_skill_*
// OtelEmitter(Arc<coco_otel::OtelManager>) 可选，与 memory::OtelEmitter 同构
```

**两通道分工**（与 memory 完全一致）：
- **持久化 `usage.json`** = loop 真正**读取**的数据面（`record_*` 写、`load_all` 读）。
- **`SkillEvent` 遥测** = OTel dashboard，fire-and-forget，**不回灌** agent loop。
- 复用现有 `emit_tengu_feature_sad`（`lib.rs:180`，`target: coco_skills::telemetry`）的字段风格，`event_type = tengu_skill_*` 保持与 upstream 命名可对齐。
- （可选，属 #1）若要向用户/模型回显「Learned skill: X」，另建 memory 式 `NoticeInbox` 走 `FinalizeTurnReport` → `<system-reminder>`；#3 不做。

### 挂载点汇总（谁调谁）

| 调用 | 位置 | 改法 |
|---|---|---|
| 成功/失败调用计数 | `app/query/src/skill_runtime.rs:471-483`（`invoke_skill` 的 Ok/Err 两臂） | 把「仅 `result.is_ok()` 时 `record`」改为**两臂都**在 `spawn_blocking` 里 `record_invocation(outcome)`；`skill.source`/`context`/`agent_id`/`prompt_chars`/`duration` 均在作用域内，同时 `emit(SkillEvent::SkillInvoked)` |
| 用户 `/` 调用计数 | `commands/src/lib.rs:519` | `record` 保持不变（= `Success`），或改 `record_invocation(.., Success)` |
| view 计数 | `app/query/src/skill_runtime.rs:486`（`read_skill_body`，子 agent 预加载 skill 正文但未执行） | 命中后 `spawn_blocking` 调 `record_view` |
| patch 计数 + 盖章 | #1 学习 fork 的 finalize：遍历 `AgentSpawnResponse.paths_written` | 每个写出的 skill 调 `stamp_agent_origin` + `record_patch`（原语由 #3 提供） |
| Curator 选盘 | #1 | `load_all` + `on_disk_origin(path) == SkillOrigin::Agent` 过滤 |

## 配置 & Feature 门 & ModelRole

- **Feature 门**：**不新增**。skills 是 “configured = enabled” 子系统，遥测/溯源恒开（best-effort）。#1 的**写**闭环若需 coarse 开关，复用既有 `Feature::RunSkillGenerator`（`features.rs:148`，已 gate `/run-skill-generator`）或在子系统入口（`Option<Arc<SkillRuntime>>` 是否存在）门控——那是 #1 的决定，#3 不引入。
- **子配置（新增 `SkillsConfig`）**：遵 `MemoryConfig` 单点折叠模式，在 `common/config/src/sections.rs` 定义、`runtime.rs::build_runtime_config_with` 里 `skills: SkillsConfig::resolve(merged, &env)` 挂进 `RuntimeConfig`：
  ```rust
  pub struct SkillsConfig {
      pub telemetry_enabled: bool,      // default true；关掉则 record_* 变 no-op
      pub learning_enabled: bool,       // 预留给 #1；default false
      pub learning_dir: Option<PathBuf>,// 预留：agent-owned skill 落盘根
  }
  ```
  子开关全部住 `SkillsConfig`，**不**升成 Feature 变体（与 memory 的 `extraction_enabled` 同规）。
- **env**：路径经 `coco_config::global_config::config_home()`（既有）。如需开关 env，走 `EnvKey`（`common/config/src/env.rs`）加 `CocoSkillTelemetryDisable`（`COCO_SKILL_TELEMETRY_DISABLE`）+ `as_str()` 臂；**禁止** leaf crate 直接 `std::env::var`。既有 skill env 先例 `COCO_DISABLE_POLICY_SKILLS`。
- **ModelRole**：#3 本身不 fork，不加 role。#1 的 review-fork 经 `AgentDefinition.model_role = Some(ModelRole::Review)`（`ModelRole::Review` 已存在，专为 review 型子 agent）路由，绝不硬编码 model_id、绝不加 per-request override。

## 错误处理分级

- `coco-skills` 为 **Tier-2（thiserror `SkillsError`）**。新公共 fn：
  - `stamp_agent_origin` / `write_skill_markdown` → `crate::Result<()>`（`Result<_, SkillsError>`），`SkillsError::Io #[from]` 覆盖 IO，其余 `SkillsError::generic()`。**禁止 anyhow**（`just check-error-policy` 强制）。
  - `record_*` / `record_view` / `record_patch` → 返回 `()`，best-effort，失败经 `tracing` + `SkillEvent::UsageWriteFailed`，不上抛（保持现有 `record` 语义）。
  - `on_disk_origin` → 直接返回 `SkillOrigin`（fail-closed 到 `User`），不返回 `Result`。
- `SkillWriteHandle::check` → 返回 `CanUseToolDecision`（trait 约定，非 `Result`）；任何路径解析/frontmatter 读失败一律 `Deny`（fail-closed）。
- 遥测 `emit` 为 fire-and-forget，无 `Result`。
- **跨 crate 注意**：`coco-skills` 新增依赖 `coco-tool-runtime`（拿 `CanUseToolHandle`/`CanUseToolCallContext`/`DecisionReason`）与 `coco-shell-parser`（`is_known_safe_command`）、`coco-frontmatter`（已依赖，用于读 origin）、`coco-apply-patch`（apply_patch 路径检查）——与 `coco-memory` 的依赖集一致，DAG 合法。

## 分阶段实施计划（里程碑）

**M1 —— 遥测存储（纯 `usage.rs`，零外部接线）**
- 扩 `SkillUsageStats` + `SkillOutcome`；`record` 重构为 `apply`+`record_invocation/view/patch`；加进程级 file `Mutex`、(name,kind) debounce、`load_stats`。
- 扩 `usage.test.rs`：新字段 back-compat 反序列化、失败不动 `usage_count`、并发写不丢增量、patch 不 debounce、`score_for` 行为不变。
- 门槛：`just quick-check`。

**M2 —— 接线两处/三处写入点（`app/query` + `commands`）**
- `invoke_skill` 两臂改 `record_invocation(outcome)` + `SkillEvent::SkillInvoked`；`read_skill_body` 加 `record_view`。
- `SkillTelemetryEmitter`/`SkillEvent`/`telemetry.rs` 落地（Noop/Tracing）。

**M3 —— 溯源字段 + 盖章原语（`skills/src/lib.rs` + `serialize.rs`）**
- `SkillOrigin` + 3 字段 + parser 读取 + `bundled()`/`mcp_builders` 补默认。
- `stamp_agent_origin` / `on_disk_origin`。
- `lib.test.rs`：缺键→User、`origin: agent` 解析、bundled 恒 User、盖章 round-trip。

**M4 —— 写围栏（`skills/src/can_use_tool.rs`）**
- `SkillWriteHandle` + `create_skill_write_handle*`；containment（抽取或拷贝 `path_under_root`）；provenance-免疫 + read-before-write。
- `can_use_tool.test.rs`：越界 Deny、symlink 逃逸 Deny、用户 skill Edit Deny、新文件 Write Allow、未读先写 Deny、Bash 只读放行。

**M5 —— 子配置（`coco-config`）**
- `SkillsConfig` + `resolve` + `RuntimeConfig` 字段 + `EnvKey`（可选）；`sections.test.rs`。
- 最终门槛：`just pre-commit`（一次性）。

> M1–M5 全部 land 后，#1 只需：加 `ForkLabel::LearnSkill`、fork 时把 `create_skill_write_handle` + `allowed_write_roots` + `ModelRole::Review` 挂上 `AgentSpawnRequest`、finalize 里 `stamp_agent_origin`+`record_patch`、按 `load_all`+`on_disk_origin` 选盘——零新基础设施。

## 测试策略

- **companion `.test.rs` 强制**（`#[path="x.test.rs"] mod tests;`，禁 inline）。
- `usage.test.rs`（扩）：注入时钟（`score_for_at` 已有）；`reset_debounce_for_tests` 已有，补 `reset_file_lock_for_tests`；断言旧文件（仅两字段）反序列化后新字段为 0；失败调用后 `usage_count` 不变、`failure_count`=1；两线程并发 `record_patch` 同 skill → `patch_count`=2（验进程锁）。
- `can_use_tool.test.rs`（新）：照抄 memory 的 symlink/lexical/dangling 用例表；新增「Edit 用户 skill → Deny」「Write 新 skill → Allow」「read-before-write：先 Edit 未读 → Deny，先 Read 再 Edit → Allow」。
- `serialize.test.rs`（新）：`stamp_agent_origin` 幂等、保留其余 frontmatter、`on_disk_origin` fail-closed（坏 YAML→User）。
- `lib.test.rs`（扩）：把现有「`author` 被丢弃」断言旁补「`origin`/`created-by` 被读取」；bundled skill `origin==User`。
- 用 `pretty_assertions::assert_eq` 比整对象。UI 无关，无需 insta。

## 风险 & 开放问题

1. **「view」的语义边界**：本设计把 view 定义为 `read_skill_body` 的子 agent 预加载（「被查阅未执行」）。若要覆盖 skill_discovery 的推荐面（`reminder_source.rs`，每轮可能触发）会过噪。**开放**：是否也在 `SkillsSource::invoked()`（`reminder_source.rs:110`，当前返回空、seam 标 “future work”）接一个 `InvokedSkillsTracker` 以获得每会话调用视图——建议归 #1/后续。
2. **`usage.json` wire-compat 边界**：加字段对读安全；但若 upstream 会严格校验形状则需 sibling 文件。**决定**：先走加字段（seam 授权 “extend”），sibling 文件留作破坏性变更逃生口。
3. **`path_under_root` DRY**：抽到 `coco_utils_absolute_path` 是正解但扩大改面；拷贝到 `skills/src/path.rs` 更快但有两份 symlink 安全代码。**建议**：M4 先拷贝并留 TODO，抽取单开一个 refactor PR（两份实现字节一致，抽取零风险）。
4. **origin 盖章的信任模型**：模型在 fork 内可能自己写 `origin: user` 试图「伪装成用户 skill 免疫」。缓解：`stamp_agent_origin` 在 fork 写盘后由 #1 **无条件覆盖**为 `agent`（权威盖章不信模型 frontmatter）；且围栏对「已存在且 `origin=User`」的目标一律 Deny，模型无法把已有用户 skill 改成 agent。残余风险：模型创建**新**文件时写 `origin: user`——被 `stamp_agent_origin` 覆盖修正。可接受。
5. **跨进程遥测丢增量**：多会话共享 `config_home` 时并发写可能丢个别自增。对阈值型 Curator 决策无实质影响（已论证不采用 lock.rs）。若未来要精确跨进程计数，再评估 CAS 或每会话分片 + 归并。
6. **`SkillsConfig` 的最小面**：M5 只落 `telemetry_enabled`；`learning_enabled`/`learning_dir` 是给 #1 的预留位——需与 #1 owner 对齐字段名，避免二次改 schema。

**为什么必须先于 #1 落地**：#1 的 Curator 要按 use/failure/patch 计数做「改进/退役」决策——无遥测即无数据（且失败此前不可见）；#1 自动创建 skill，无 `origin` 门 + 写围栏就无法把「自己的 skill」与「用户 skill」区分开，会有清洗用户资产的风险；围栏 + `allowed_write_roots` + `stamp_agent_origin` 正是 #1 fork 依赖的溯源**执行面**。三者是 #1 的安全与数据前提，故 #3 是硬前置。

---

关键文件（绝对路径）：
- `/lyz/codespace/cocode/coco-rs/skills/src/usage.rs`（M1 扩 `SkillUsageStats` + `record_*` + 并发）
- `/lyz/codespace/cocode/coco-rs/skills/src/lib.rs`（M3 `SkillOrigin` + 3 字段 + `parse_skill_markdown:982`）
- `/lyz/codespace/cocode/coco-rs/skills/src/bundled.rs`（M3 `bundled():54` 补默认 origin）
- `/lyz/codespace/cocode/coco-rs/skills/src/can_use_tool.rs`（M4 新增，照抄 `/lyz/codespace/cocode/coco-rs/memory/src/can_use_tool.rs`）
- `/lyz/codespace/cocode/coco-rs/skills/src/telemetry.rs`、`/lyz/codespace/cocode/coco-rs/skills/src/serialize.rs`（新增）
- `/lyz/codespace/cocode/coco-rs/app/query/src/skill_runtime.rs`（M2 接线 `:471-483` Ok/Err 两臂 + `:486` view）
- `/lyz/codespace/cocode/coco-rs/commands/src/lib.rs`（`:519` 用户 slash 记录点）
- `/lyz/codespace/cocode/coco-rs/core/tool-runtime/src/agent_handle.rs`（`AgentSpawnConstraints:44` 外环，#1 挂载）
- `/lyz/codespace/cocode/coco-rs/common/config/src/sections.rs` + `runtime.rs`（M5 `SkillsConfig`）

---

> [← 可落地建议](05-recommendations.md) · [返回索引](README.md) · [设计①学习闭环 →](design-01-skill-learning-loop.md)
