# Hermes Agent vs coco-rs — 分析报告

> 分析对象:`/lyz/codespace/3rd/hermes-agent`(Nous Research 出品的自进化 AI Agent,Python)
> 对照对象:`/lyz/codespace/cocode/coco-rs`(多 provider LLM SDK + CLI,Rust,Claude Code 移植)
> 生成日期:2026-07-02 · 聚焦 Hermes 两大宣传能力:**自进化(self-evolution)** 与 **IM 接入**

---

## 这份报告是什么

Hermes 自我定位为"唯一内建学习回路的 Agent":它从经验中创造技能、在使用中改进技能、周期性提醒自己固化知识、检索自己的历史对话、跨会话建立对用户的模型;同时它"活在你所在之处"——一个网关进程同时接入 Telegram / Discord / Slack / WhatsApp / Signal / 微信 / 飞书 等 20+ 聊天平台,并能跑在 $5 VPS、GPU 集群或无服务器(Modal/Daytona)之上。

本报告深入对比了两个代码库的**架构范式**与**逐能力功能**,重点剖析 Hermes 的两大宣传能力在 coco-rs 中的对应现状,并给出 coco-rs 可落地吸收的优先级路线图。

## 阅读顺序

| # | 文档 | 内容 |
|---|------|------|
| 01 | [Hermes 架构总览](01-hermes-architecture.md) | Hermes 是什么、进程模型、Agent 回合生命周期、巨型模块地图,以及与 coco-rs 分层 Rust 工作区的**范式差异** |
| 02 | [功能对比矩阵](02-feature-comparison.md) | 逐能力(运行时/工具/多智能体/技能学习/记忆/外部连接/工程)✅⚠️❌ 对照 + 领先方判断 |
| 03 | [自进化能力深度剖析](03-self-evolution-deep-dive.md) | **重点**:Hermes 闭环学习(Curator / background review / nudge / provenance / session-search / Honcho)端到端,vs coco-rs 现状与差距 |
| 04 | [IM 接入能力深度剖析](04-im-integration-deep-dive.md) | **重点**:Hermes 单网关多平台架构、Platform 抽象、配对授权、跨平台会话连续性,vs coco-rs 的 Bridge/Event Hub 地基评估 |
| 05 | [吸收 Hermes 优点:可落地建议](05-recommendations.md) | 分级(Tier 1/2/3)可落地路线图,每项含 coco-rs crate 归属、工作量/风险、前置依赖 |

### 实现设计文档(code-grounded,可直接开工)

在 05 的路线图之上,以下三篇是对**最优先三项能力**的详细实现设计——每篇都基于对真实 `.rs` seam 的抽取(`memory` runtime、`subagent` fork、`skills/usage.rs`、`engine_finalize_turn`、`CommandQueue`/`CoreEvent`),给出新类型/新 fn 的 Rust 签名、挂载点、crate 归属、配置/Feature 门、错误分级、分阶段计划与测试策略,并各带一节**评审修正**(对抗式 critic 已对照代码复核)。建议按依赖顺序阅读:③ → ① → ②。

| # | 设计 | 定位 |
|---|------|------|
| ③ | [技能遥测 + Provenance](design-03-skill-telemetry-provenance.md) | **快速收益 / ① 的前置**:扩展已有 `skills/src/usage.rs`,给 `SkillDefinition` 加 agent/user provenance |
| ① | [技能自主学习闭环(Curator 式)](design-01-skill-learning-loop.md) | **战略**:镜像 coco 自己的 memory 闭环(ExtractService fork+fence+finalize / DreamService 整合)到技能,复用 subagent Fork 前缀缓存 |
| ② | [IM 网关](design-02-im-gateway.md) | **战略**:新建 GatewayRunner daemon,入站走 `QueueOrigin::Im`、出站走 `CoreEvent` egress,复用 headless query 路径;先 Telegram 后 Slack |
| ④ | [Journey 学习时间线 + Journal 事件源(English)](design-04-journey.md) | **①③ 落地后的可观测面 + 执行开发计划**(2026-07-16,基于已实现的 `coco-skill-learn` 现状,纯英文,**已对抗式评审**——30 项引用审计 + 12 项集成缝核查,17 项修正已并入正文,见其 §11 review log):`/journey` TUI 时间线(学到的技能+记忆)、append-only journal 补齐时间维度、delete/edit 往返;含 12 个 work package 的执行计划与 learn 闭环 6 项优化(notices、用户主动 `/learn`、signal/cursor/退避、config 化) |

### Release-log 吸收分析(2026-07-10,English)

上面五篇聚焦自进化与 IM;下面这条线是另一次独立分析——通读 Hermes 全部 21 个
release(v0.2.0→v0.18.2)提取优化点,并逐项对照 coco-rs 现状(31 项事实带
file:line 取证)后的吸收清单与开发计划:

| 文档 | 内容 |
|------|------|
| [hermes-opt.md](hermes-opt.md) | 全量 release-log 扫描结论:已具备清单(防重复立项)、P0/P1/P2 吸收分级、反面教训、落地顺序 |
| [plans/](plans/README.md) | 按 PR 切分的成套开发计划(10 篇),每篇含 Hermes 源码引用证据(仓库相对路径,锚定 commit `a7f65e3bc`)与 coco-rs 落点 |

---

## 核心结论(TL;DR)

### Hermes 真正领先的两处,恰是它的宣传点

1. **自进化(闭环学习)——coco-rs 结构性缺失。** Hermes 每 ~10 回合自动 fork 一个受限子 agent(`agent/background_review.py`),把刚结束的会话蒸馏成**技能补丁 / 新技能 + 记忆写入**;再由 **Curator**(`agent/curator.py`,默认 7 天周期)对 agent 自建技能做确定性老化(active→stale→archived)与可选 LLM"umbrella 合并"。一个写来源 ContextVar(`skill_provenance.py`)把"agent 自学的"(受生命周期管理)与"用户让写的"(永久)干净分权。**coco-rs 的自动闭环只作用于知识(memory:extract→dream→recall),从不触碰能力(技能/工具/prompt/策略)**——这是两系统最大分野。详见 [03](03-self-evolution-deep-dive.md)。

2. **IM 接入——coco-rs 零消费级集成。** Hermes `gateway/` 单进程接入 **20+ 聊天平台**,统一 `BasePlatformAdapter` + 插件注册表 + relay + DM 配对授权 + 跨平台会话连续性 + 语音 STT/TTS + scale-to-zero。coco-rs **全工作区 grep 无任何 IM 集成**,唯一外部触达是开发者向的 IDE bridge / Event Hub / SDK 传输(SSE/WS/NDJSON)。其 Event Hub 可作为地基,但存在**"每会话一进程" vs "单进程多路复用"的根本模型冲突**、`hub/connector` 仍是一行 re-export 空骨架、cron 仅在 TUI 交互态有 drain pump 等硬限制。详见 [04](04-im-integration-deep-dive.md)。

此外,Hermes 在 **6 个终端后端**(local/docker/ssh/singularity/modal/daytona)、`execute_code` **零上下文成本管道**、**MoA** 顾问扇出、**Skills Hub 生态分发**、**结构化用户建模(Honcho)**、训练向轨迹工具上也明显更全。

### coco-rs 领先的核心:工程分层纪律 + 运行时质量

同样的能力,coco-rs 用清晰的 ~96-crate 分层 DAG + callback-handle trait seam 实现,provider 关注点严格隔离在 `vercel-ai-*`。经**直接代码核实**,以下几项 coco-rs 反而优于或 Hermes 未见:

- **checkpoint / rewind**:coco 的四相 rewind(`app/tui/src/state/rewind.rs`)同时回溯**代码 + 会话 + Summarize** 变体,Hermes shadow-git 只回滚代码。
- **provider-native ToolSearch**:coco 的动态工具加载有 `OpenAiNativeToolSearch` / Anthropic `tool_search` beta 等 provider 原生变体,Hermes 仅客户端 BM25。
- **Plan Mode + `VerifyPlanExecution` 计划态机**、**LSP**、**代码检索(BM25/vector/RepoMap)**、**git worktree 隔离**、**配置热重载**、**`apply-patch` 模糊补丁**、**`secret-redact`/SSRF 守卫**、**codex 级原生 scrollback 无闪烁 TUI paint engine**。

> ⚠️ **勘误说明**:本报告初稿曾因"以调研沉默代替代码核验",把 coco 的 **checkpoints**(误标 ❓)与 **动态工具加载/ToolSearch**(误标 ⚠️)判为落后于 Hermes;经直接 grep coco 代码库,二者均为 coco 一等能力(甚至更强),已在 [02](02-feature-comparison.md) 全部**更正为 ✅**。凡涉及 coco"缺失"的判定,均以代码 grep 为准。

## 最值得吸收的三件事(详见 [05](05-recommendations.md))

| 优先级 | 建议 | coco-rs 归属 | 量级 | 为什么 |
|--------|------|-------------|------|--------|
| **战略下注 #1** | **技能自主学习闭环(Curator 式)**:把现有 memory 的 fork+围栏+整合模式扩展到技能/prompt | `memory` + `skills` + `subagent` | L | Hermes 头号差异;coco 已有 Fork 前缀缓存共享 + 每轮 finalize 钩子,成本被显著摊薄 |
| **战略下注 #2** | **IM 网关**(先 Telegram/Slack 单平台端到端),建在 Event Hub / `CommandQueue` egress 之上 | 新 `hub/gateway` + `app/cli` | XL | Hermes 头号差异;coco 唯一"从无到有"的战略能力 |
| **快速收益** | 技能遥测 + provenance(`.usage.json` 等价 + agent/user 分权),为 #1 铺路 | `skills` | S–M | 独立、低风险、是学习闭环的前置 |

---

## 方法学

本报告由一个 16-agent 编排工作流生成:6 个 reader 深读 Hermes 子系统(自进化 / 技能 / IM 网关 / Agent 核心 / 工具委派终端后端 / 上下文-cron-TUI-provider)→ 4 个 reader 深读 coco-rs 对应子系统 → 5 个 synthesizer 撰写各章 → 1 个对抗式 critic 校验事实与完整性。critic 发现的**两处会翻转核心结论的事实错误**(checkpoints、ToolSearch)已由维护者**逐一 grep coco 代码库核实并更正**;其余对 coco"缺失项"(`execute_code`/PTC、MoA、语音 STT/TTS、消费级 IM、6 后端中的 docker/ssh/serverless)的判定亦经核验属实。
