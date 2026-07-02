# IM 接入能力深度剖析（Hermes vs coco-rs）

> [← 自进化深剖](03-self-evolution-deep-dive.md) · [返回索引](README.md) · [可落地建议 →](05-recommendations.md)

---

# IM 接入能力深度剖析

> 这是本报告的**核心对比维度**之一。Hermes 把"一个 agent、20+ 聊天平台"作为其第二卖点,并为此构建了整个 `gateway/` 子系统;而 coco-rs 在这个维度上是**结构性空白**——它完全没有任何面向消费者聊天平台的接入。本节先深挖 Hermes 的网关架构,再客观评估 coco-rs 现有的外部连通能力(IDE bridge / Event Hub / SDK)能否作为构建 IM 网关的地基,最后给出高层差距结论(具体迁移路线见第 05 节)。

---

## 1. Hermes Gateway 架构

### 1.1 单进程多平台模型(`gateway/run.py`)

Hermes 网关的本质是**一个长驻的 asyncio 进程**,由 `GatewayRunner`(`gateway/run.py`,约 19,000 LoC / 921KB)统一驱动,同时前置 20+ 个聊天平台并把每条入站消息桥接到同一个 Hermes agent runtime。

核心事实:

- `GatewayRunner` 持有 `adapters: Dict[Platform, adapter]`,**所有平台适配器跑在同一个事件循环上**。
- `GatewayRunner` 通过 mixin 组合出巨型编排器:`GatewayAuthorizationMixin`(`authz_mixin.py`)、`GatewayKanbanWatchersMixin`、`GatewaySlashCommandsMixin`(`slash_commands.py`)。findings 明确指出这是一个 God-object(`AGENTS.md` 自己把 `run.py`/`cli.py`/`run_agent.py` 标为需要拆分的"cluster")。
- 入站消息通过 `handle_message()` **快速返回**,真正处理放进后台 task `_process_message_background()`,因此新消息可以打断在途 turn(interrupt/queue/debounce 都在基类里)。
- 断线自愈由 `_platform_reconnect_watcher` 负责:某个适配器致命错误后重建并 `connect(is_reconnect=True)`。

这个"单进程多路复用"模型与 coco-rs 的"每 session 一个 `QueryEngine` 进程"模型形成根本性对立——这是后面评估地基可行性的关键(见 §2.4)。

### 1.2 Platform 抽象层(`gateway/platforms/base.py`)

所有平台都继承 `BasePlatformAdapter`(ABC,约 5,400 LoC)。**基类吸收了几乎所有跨平台硬逻辑**,这是"IRC 适配器 <1000 LoC 而 Telegram/Discord 8000+"的原因。

一个新平台**必须**实现的抽象方法只有 4 个:

| 抽象方法 | 职责 |
|---|---|
| `connect(is_reconnect)` | 建立传输连接 |
| `disconnect()` | 断开 |
| `send(chat_id, content, reply_to, metadata) -> SendResult` | 发送文本 |
| `get_chat_info(chat_id)` | 查询会话元信息 |

基类**已提供默认实现**的可选/富能力表面:

- 媒体:`send_typing / send_image / send_voice / send_video / send_document / send_animation / play_tts`
- 交互式 UX:`send_clarify / send_exec_approval / send_slash_confirm / send_model_picker`(按钮回调 id 约定 `cl:<id>:<idx>`、`appr:<id>:<choice>`、`sc:<choice>:<id>` 是共享的,gateway 侧解析器无需改动)
- 流式:`supports_draft_streaming / send_draft / edit_message`、`_keep_typing`
- 基类自持:`handle_message()`(后台派发 + per-session interrupt + 文本 debounce + `/stop`-`/new`-`/reset`-`/approve`-`/deny` + clarify bypass)、`build_source() -> SessionSource`、`format_message()`、`_send_with_retry`、媒体提取(`extract_media/extract_images/extract_local_files`)、ephemeral TTL

能力用 class-flag 声明:`supports_code_blocks`、`supports_async_delivery`、`splits_long_messages`、`typed_command_prefix`。

> 兄弟适配器模式:WhatsApp Baileys 与 whatsapp_cloud 都继承 `WhatsAppBehaviorMixin`(`whatsapp_common.py`),mixin 排在基类前面以让其 `format_message` 生效。

### 1.3 支持平台全清单(~30 个适配器,20+ 独立聊天产品)

**Plugin 平台**(`plugins/platforms/<name>/`,每个含 `plugin.yaml` + `adapter.py`,共 20 个目录):

| # | 平台 | # | 平台 |
|---|---|---|---|
| 1 | Telegram | 11 | DingTalk(钉钉) |
| 2 | Discord | 12 | WeCom(企业微信) |
| 3 | Slack | 13 | Email |
| 4 | WhatsApp(Baileys bridge) | 14 | SMS(Twilio) |
| 5 | Matrix | 15 | Google Chat |
| 6 | Mattermost | 16 | Home Assistant |
| 7 | Microsoft Teams | 17 | ntfy |
| 8 | LINE | 18 | SimpleX Chat |
| 9 | IRC | 19 | iMessage(via Photon) |
| 10 | Feishu/Lark(飞书) | 20 | Raft |

**内建适配器**(`gateway/platforms/*.py`,经 `_create_adapter` 的 if/elif 链实例化):

- Signal
- WhatsApp Cloud(Meta Cloud API)
- Weixin(微信个人/公众号,91KB)
- Yuanbao(腾讯元宝,214KB)
- BlueBubbles(iMessage)
- QQBot
- MS-Graph webhook
- 通用 Webhook
- API server
- `WECOM_CALLBACK`

**特殊**:`LOCAL`(CLI)、`RELAY`(实验性通用中继)。合计约 30 个适配器,横跨 20+ 独立聊天产品——与 findings 中"Hermes 第二卖点"一致。

### 1.4 插件注册 vs 内建注册(动态 `Platform` 枚举)

Hermes 用两条注册路径,并让插件**优先**:

```
plugins/platforms/<name>/
├── plugin.yaml     # name,label,kind:platform,requires_env/optional_env(喂给 setup 向导的富 dict)
└── adapter.py      # register(ctx) -> ctx.register_platform(...)
```

- `ctx.register_platform(...)` 构造一个 `PlatformEntry`(`gateway/platform_registry.py`),携带 `adapter_factory / check_fn / validate_config / is_connected / setup_fn` 以及一批设计良好的钩子:`env_enablement_fn`、`apply_yaml_config_fn`、`cron_deliver_env_var` + `standalone_sender_fn`(进程外 cron 投递,例如 IRC 开一个临时 `-cron` nick、JOIN、PRIVMSG、QUIT)、`allowed_users_env`/`allow_all_env`(接入授权)、`max_message_length`、`pii_safe`、`platform_hint`(注入系统提示的平台指引)。
- **延迟加载**:`register_deferred()` 把 discord.py / lark_oapi / slack_bolt 等重型 SDK 的 import 推迟到首次真正查找,让 `hermes chat` 启动快(findings 明确这是有文档记录的性能动机)。
- **动态枚举**:`Platform` 枚举在 `gateway/config.py` 里对内建平台是封闭的,但 `Platform._missing_()` 会为任意已注册/已捆绑的插件名铸造 identity-stable 的伪成员——于是 `Platform("irc")` 零核心改动即可工作。
- `_create_adapter`(`run.py:7872`)**先查 registry**(插件可覆盖内建),再落到内建 if/elif。

**零核心改动加一个新平台**:实现 4 个抽象方法 + `register(ctx)` + 一个 `plugin.yaml`,即可获得 setup 向导、状态、cron 投递、授权、系统提示 hint 全套一等公民能力。

### 1.5 单进程多路复用与并发隔离(`contextvars`)

- `handle_message` 派发到 `_process_message_background` 后台 task,新消息可打断在途 turn。
- **并发安全的 per-turn 状态从 `os.environ` 迁移到 `contextvars.ContextVar`**(`gateway/session_context.py`:`HERMES_SESSION_PLATFORM/CHAT_ID/THREAD_ID/USER_ID/KEY/ID/MESSAGE_ID` + cron deliver 变量 + async-delivery capability)。原来的进程全局 `os.environ` 会让并发 turn 互相污染,这次迁移修掉了一整类真实 bug;`get_session_env()` 对 CLI/cron 回退到 `os.environ`。
- 可选 `gateway.multiplex_profiles`:一个进程服务多个 bot 身份,namespace 折进 session key。

### 1.6 DM pairing + 分层授权

**DM 配对流程**(`gateway/pairing.py` `PairingStore`)——这是 Hermes 相对普通 bot allowlist 的差异化点:

- 陌生私聊者拿到一个 8 字符一次性配对码(32 字符无歧义字母表,`secrets.choice`),**只存 salted SHA-256 哈希**;1h TTL、每平台最多 3 个 pending、10 分钟 1 次限流、5 次失败审批后 lockout、常数时间比较、`~/.hermes/platforms/pairing` 下 0600 原子文件。owner 通过 CLI 审批进 approved 列表。WhatsApp 标识符做归一化/别名。

**分层授权**(`GatewayAuthorizationMixin._is_user_authorized(source)`,判定顺序):

```
HomeAssistant/Webhook 自动放行(自带传输鉴权)
  → relay 上游信任(delivered_via_upstream_relay / authorization_is_upstream)
  → chat 级群组 allowlist(TELEGRAM_GROUP_ALLOWED_CHATS / QQ)
  → <PLATFORM>_ALLOWED_USERS / _ALLOW_ALL_USERS(插件走 PlatformEntry.allowed_users_env/allow_all_env)
  → role_authorized
  → pairing approved
  → 全局 GATEWAY_ALLOW_ALL_USERS
  → 默认拒绝
```

`dm_policy`(open/pairing/disabled)控制未授权 DM 的处理策略。findings 也诚实指出:授权面很广(allow-all / 群组 / role / pairing / 上游信任多条放行路径),是个较大的可信面。

### 1.7 会话映射与跨平台连续性

**单一确定性键函数** `build_session_key(source, group_sessions_per_user=True, thread_sessions_per_user=False, profile)`:

```
agent:<profile-ns>:<platform>:<chat_type>[:chat_id][:thread][:user]
```

规则:DM 按 chat_id/user_id 隔离;群按参与者隔离(默认);thread 默认共享(除非 `thread_sessions_per_user`);WhatsApp JID/LID 归一化以防按用户拆分 session。

- 持久化在 SQLite(`hermes_state.SessionDB`,FTS5);`sessions.json` 是路由索引(key → session_id),硬崩后启动做 stale 清理(`_prune_stale_sessions_locked`)。
- **跨平台连续性**靠 `gateway/mirror.py`:向目标 session transcript 追加一条 delivery-mirror(cron 带外投递用 `role=user`,交互式 `send_message` 用 `role=assistant`),让接收端 agent 看到上下文。findings 指出这是 best-effort 且在 SQLite 边界有损(mirror 元数据被丢),需要 role hack 规避严格交替 provider。

`SessionSource`(`gateway/session.py`)捕获 platform/chat_id/chat_type/user_id/thread_id/scope_id(guild)/profile/message_id + 备用 ID(Signal UUID、Feishu union_id)。

### 1.8 投递路由与韧性(`gateway/delivery.py`)

- `DeliveryRouter/DeliveryTarget` 解析目标串:`origin`(回源)、`local`(存 md 文件)、`telegram`(home channel)、`telegram:chat_id[:thread_id]`(显式)。
- `DeadTargetRegistry`(`dead_targets.py`)短路已确认不可达的会话(群被删/bot 被拉黑),后续成功时自愈。
- **反机器人互刷**:silence-narration 过滤丢弃幻觉出的 `(silent)`/emoji token,打断 bot-to-bot 循环。
- 超长 cron 输出:总是审计存盘;分块适配器(`splits_long_messages`)拿全量,非分块的在 `MAX_PLATFORM_OUTPUT=4000` 截断加 footer。
- `channel_directory.py` 缓存每平台可达会话(5 分钟刷新)+ 用户友好名别名,用于 `send_message` 名称解析。

### 1.9 流式投递与 per-platform 格式化

- `GatewayStreamConsumer`(`stream_consumer.py`)把 agent 的同步回调桥接到异步平台投递,**两种模式按 chat 协商**(`adapter.supports_draft_streaming()`):原生 draft streaming(Telegram `sendMessageDraft` 动画草稿帧,`send_draft`,单调 draft_id,最终答案另发)vs 就地 `edit_message` 渐进编辑;无 edit 能力的平台退化为一段一条消息。
- `GatewayEventDispatcher`(`stream_dispatch.py`)把类型化 `StreamEvent`(MessageChunk/Commentary/ToolCallChunk/LongToolHint/GatewayNotice)路由到适配器 render 钩子(`render_message_event`、`format_tool_event` 可返回 `None` 吃掉 tool chrome)。工具进度模式 off/new/all/verbose。
- **per-platform 格式化**:`format_message()` + `markdown_dialect`(IRC 剥离 markdown、Telegram MarkdownV2、Slack mrkdwn、UTF-16 长度计数)。findings 诚实点出:没有统一的类型化消息/格式化模型,一致性依赖每个适配器各自实现。

### 1.10 媒体、语音转写(STT)与语音模式(TTS)

- 入站媒体经 `cache_image_from_bytes/cache_audio_from_bytes/cache_document_from_bytes` 落地;`MessageType.VOICE` 由 STT 工具(OpenAI Whisper)从缓存文件转写(`gateway.platforms.<p>.stt_enabled`)。
- 出站语音:`send_voice()` 发原生语音气泡(Telegram)/文件附件(Discord);`prepare_tts_text` 剥 markdown + 截断;`play_tts()`/`send_voice()` 自动 TTS。
- 语音模式:`/voice on|off|tts|all|voice_only`(`slash_commands.py`)写 `_voice_mode`,切换 `_auto_tts_enabled_chats/_auto_tts_disabled_chats`;门控 `_should_auto_tts_for_chat`:chat opt-in 或(`voice.auto_tts` 默认且未 opt-out)时触发。**VOICE 输入时回复音频在文本发送前生成**。

### 1.11 Relay adapter(Gateway↔Gateway 连接器,实验性)

`RelayAdapter`(`gateway/relay/adapter.py`)是**一个 `BasePlatformAdapter` 前置多个平台**:

- 握手时 connector 交出一个 frozen `CapabilityDescriptor`(`relay/descriptor.py`:platform/label/max_message_length/supports_draft_streaming/edit/threads/markdown_dialect/len_unit(chars|utf16)/emoji/platform_hint/pii_safe;`contract_version` 门控,加法式版本演进)。
- `RelayTransport` Protocol(`transport.py`)定义 connect/handshake/send_outbound/get_chat_info/send_interrupt/go_idle/send_follow_up;生产实现 `WebSocketRelayTransport`(`ws_transport.py`)**主动拨出**到 connector(托管网关无入站端口)。
- 鉴权(`relay/auth.py`)是 HMAC-SHA256,**逐字节镜像** connector 的 TS 实现:WS-upgrade bearer token + per-delivery 签名(replay window)+ 多密钥轮转。
- connector 在投递前完成 owner author-binding,故 relay 事件是预授权的;source 携带**底层平台**(用于 keying/egress),而非 RELAY。`GATEWAY_RELAY_PLATFORMS/BOT_IDS` 支持一条 socket 上多平台。

### 1.12 Scale-to-zero / serverless(`gateway/scale_to_zero.py`)

在 relay 原语之上的纯谓词决策层:

- `scale_to_zero_enabled`(`HERMES_SCALE_TO_ZERO` env)、`parse_idle_timeout_seconds`(默认 5min)、`messaging_is_relay_only_or_absent`(直连 socket 平台会解除武装)、`should_arm`(flag ∧ relay-only ∧ wakeUrl)、`is_idle`(无在途 agent turn ∧ 无活跃后台工作 ∧ 超时内无入站)。
- idle 时驱动 `transport.go_dormant()/go_idle()`——socket 关闭但 supervisor + RAM 保留,Fly 用 `autostop:suspend` 挂起、`autostart-on-wakeUrl` 复活。**从不 `disconnect()`/drain**,后台工作永不丢失。

### 1.13 消息流 ASCII 图

```
┌──────────────┐  平台 SDK 推送
│  聊天平台     │  (Telegram/Discord/Slack/微信/飞书/IRC/…)
│  (~30 个)     │
└──────┬───────┘
       │ inbound
       ▼
┌─────────────────────────────────────────────────────────────┐
│ BasePlatformAdapter（子类解析原生事件）                        │
│   parse → build_source() → MessageEvent → handle_message()    │
└──────┬────────────────────────────────────────────────────────┘
       │  topic-recovery 改写
       ▼
┌─────────────────────────────────────────────────────────────┐
│ GatewayRunner（单进程，adapters: Dict[Platform, adapter]）    │
│  1. build_session_key(source)  → agent:<ns>:<plat>:<type>:…   │
│  2. authz: _is_user_authorized(source) / pairing / dm_policy  │
│  3. active-session guard（interrupt / queue / clarify-bypass）│
│  4. spawn _process_message_background()  ← 快速返回            │
│  5. 设置 contextvars（session_context.py，隔离并发 turn）      │
└──────┬────────────────────────────────────────────────────────┘
       │ _message_handler（agent bridge）
       ▼
┌─────────────────────────────────────────────────────────────┐
│ AIAgent runtime（run_conversation）                           │
│   同一个 agent runtime 服务所有平台的所有 session             │
└──────┬────────────────────────────────────────────────────────┘
       │ 同步回调流
       ▼
┌─────────────────────────────────────────────────────────────┐
│ GatewayStreamConsumer / GatewayEventDispatcher               │
│   draft streaming ↔ edit-in-place ↔ 分段；typed StreamEvent   │
│   format_message + markdown_dialect（per-platform）           │
└──────┬────────────────────────────────────────────────────────┘
       │ send()/send_voice()/media
       ▼
┌─────────────────────────────────────────────────────────────┐
│ DeliveryRouter → DeliveryTarget.parse(origin|local|plat:chat) │
│   DeadTargetRegistry（自愈）+ silence-narration 反刷          │
│   超长 → 审计存盘 + 能力感知截断                              │
└──────┬────────────────────────────────────────────────────────┘
       ▼
┌──────────────┐   回投；mirror.py 把带外投递写入目标 session
│  聊天平台     │   transcript → 跨平台会话连续性
└──────────────┘

带外触发:cron / send_message tool → DeliveryRouter.deliver()
                                    → adapter.send() 或 standalone_sender_fn(进程外)
```

### 1.14 Per-platform slash-command 访问控制

findings 覆盖到的部分:

- Slash 逻辑集中在 `GatewaySlashCommandsMixin`(`gateway/slash_commands.py`,约 4,185 LoC)。
- 基类 `handle_message()` 直接处理 `/stop`-`/new`-`/reset`、`/approve`-`/deny`、clarify bypass;`/voice on|off|tts|all|voice_only` 在 mixin 里切换 per-chat TTS 状态。
- 命令前缀由 capability flag `typed_command_prefix` 声明,格式差异靠 `format_message`/`markdown_dialect` 吸收。
- **访问控制复用统一授权栈**:命令与普通消息走同一条 `_is_user_authorized` 判定链,per-platform 允许名单由 `PlatformEntry.allowed_users_env/allow_all_env` 注入,orchestrator-only 命令再叠加 role 门控。

> 诚实说明:findings 未提供"每条 slash 命令在每个平台上独立 ACL 矩阵"这样的细粒度证据。可确认的是命令 ACL = 平台授权栈 + role 门控 + capability flag 的组合,而非每命令一张独立表。

---

## 2. coco-rs 现状:没有 IM 接入,只有开发者面向的连通

### 2.1 结论先行:零消费者 IM 集成

对全工作区 grep `telegram|discord|slack|whatsapp|signal|feishu|lark|wechat|mattermost|matrix` 的结果是**零集成代码**。所有命中都是误报:

- `"Slack"`/`"Lark"` 仅出现在 bundled skill 的**提示词文本**里(`skills/src/bundled/schedule.rs`、`skillify.rs` 引导用户去 `claude.ai/settings/connectors` 配置 Slack 作为通用 MCP connector)和 skill-parser 测试夹具(`"lark-base"` 作为示例 skill 名);
- `"signal"` 指 OS 信号(SIGQUIT / `tokio::signal`);
- `"gateway"` 专指 AI/LLM 上游代理(LiteLLM、Cloudflare AI Gateway,见 `services/inference/src/logging.rs`);
- `"messaging"` 指 swarm agent 间的 mailbox(`coordinator/src/mailbox`,`SendMessage` 工具)。

**唯一能通往 IM 平台的路径是用户自配的通用 MCP server**(例如第三方 Slack MCP connector)——这正是 bundled skill 提示词引用 Slack 的方式。没有任何内建的、一等公民的 IM bridge/gateway。

### 2.2 coco-rs 实际拥有的外部连通面

| 能力 | crate/位置 | 状态 | 性质 |
|---|---|---|---|
| SDK/server 模式(JSON-RPC NDJSON over stdio) | `app/cli/src/sdk_server/` | **已实现**(仅 `StdioTransport`) | 开发者/程序化控制面 |
| IDE/REPL bridge + 权限中继 | `bridge/`(`coco-bridge`) | ControlRequestHandler 已接入 SDK server;`BridgeServer` 是 channel-only 骨架,无真实 socket listener | 开发者(IDE 走 MCP,非直连 socket) |
| Event Hub(设计) | `hub/protocol` + `hub/connector` + `hub/server` | `connector` 是**空骨架**(`lib.rs` 仅一行 re-export);无 SQLite/WS ingest | 设计文档为主 |
| Event Hub(实际交付) | `hub/server`(`coco-hub-server`) | **已实现的 "Local Session Hub"**:Axum+Askama+HTMX 只读投影本地 `coco-session` JSONL | 本地 session 浏览器查看器 |
| Remote session / 上游代理 | `docs/coco-rs/crate-coco-remote.md` | **仅计划**,`remote/` crate 不存在;CLI `RemoteControl`/`Sync` 是 println stub | N/A(Anthropic CCR 是明确非目标) |

关键细节:

- **SDK stdio JSON-RPC 才是真正的外部控制面**:`coco sdk` 起一个 JSON-RPC 控制循环(`sdk_server/transport.rs`、`dispatcher.rs`),处理 `initialize/interrupt/can_use_tool/set_permission_mode/set_model`。尽管 CLAUDE.md 宣称 SSE/WS/NDJSON,实际只实现了 `StdioTransport`。
- **IDE 集成实际走 MCP**(IDE 作为 MCP server 运行),而非 `coco-bridge` 里的直连 WS 中继;`bridge/CLAUDE.md` 对此有明确说明。cloud-session API(updateBridgeSessionTitle/fetchSession/archiveSession)是**刻意不实现**的(claude.ai/CCR 后端是非目标)。
- **Local Session Hub 甚至没有嵌进主 `coco` 二进制**(没有 `--serve-hub` flag 接线),它是独立的 `coco-hub-server` 二进制,只读渲染磁盘上的 session transcript,依赖仅 axum+askama(无 rusqlite/tungstenite/reqwest)。它是"浏览器里看 session"的查看器,**不是远程控制通道**。

### 2.3 developer-facing vs end-user-chat-facing 的本质区别

| 维度 | Hermes IM Gateway | coco-rs 现有连通面 |
|---|---|---|
| 面向对象 | **终端用户**(在自己的聊天软件里对话) | **开发者/运维**(SDK 客户端、IDE、本地浏览器) |
| 入站触发 | 陌生人私聊 → pairing → 授权对话 | 需先启动进程并连上 stdio/MCP |
| 身份/授权 | pairing 码 + 多层 allowlist + role | 无用户身份概念;信任本地进程 |
| 会话映射 | `(platform, chat, user, thread)` 确定性键 + 跨平台 mirror | `coco-session` 按 **project** 键的 JSONL,无平台/用户维度 |
| 投递 | 主动 push 到聊天平台(带 dead-target 自愈) | 无 push;只有 SDK 响应 / 本地只读查看 / 出站 webhook hook |
| 并发多会话 | 单进程前置多平台多用户并发 | 单 session 一个 `QueryEngine`,无多路复用编排器 |

最接近"离终端触达用户"的能力是 hook 的 `webhook`/`http` 类型(`hooks/src/lib.rs`,生命周期事件上 SSRF-guarded 的出站 HTTP POST)——**仅出站,不是入站 IM 通道**。

### 2.4 评估:Event Hub / connector 架构能否作为 IM 网关地基?

**部分骨架可复用,但缺失的正是 IM 网关最难的那一半。**

✅ 已有的、可复用的"骨头":

1. **出站事件总线**:`QueryEngine` 的 `mpsc::Sender<CoreEvent>` 是设计好的 egress 点,`CoreEvent` 单枚举三层分发(Protocol/Stream/Tui),消费者按层取用——这正是 `hub/connector` 当初要做事件转发的接口。一个 IM 投递层可以订阅 `CoreEvent::Stream`(内容增量)+ `CoreEvent::Protocol`。
2. **入站命令注入范式已存在**:`app/cli/src/cron_tick.rs` 已经演示了"把 prompt 入队到 `CommandQueue`(`QueueOrigin::Cron`)唤醒 idle agent driver"。IM 入站可镜像成 `QueueOrigin::Im` 之类。`CommandQueue` 的优先级 + `QueryGuard` 三态机 + generation counter 已经处理了 steering/打断并发。
3. **传输枚举雏形**:`bridge` crate 已有 `BridgeTransport {WebSocket, Sse, Ndjson}` 与 channel-based `BridgeServer` 骨架,可作为网络 listener 的起点。
4. **只读 session 投影**:`hub/server` 的 `EventStore` trait + `LocalSessionJsonStore` 展示了如何把 transcript 映射成 hub 形态。

❌ 缺失的、也是最难的一半:

1. **connector 根本没建**:`hub/connector/src/lib.rs` 仅 `pub use coco_hub_protocol as protocol;`,没有 WS client、ring buffer、tungstenite 依赖。设计里的 WS egress + SQLite ingest + 多实例聚合**完全不存在**。
2. **进程模型不匹配(最根本)**:coco-rs 是"每 session 一个 `QueryEngine`",而 IM 网关需要 Hermes `GatewayRunner` 那样的**单进程多路复用编排器**——同时前置多平台、多用户、多并发 session。coco-rs 没有这一层,也没有 `contextvars` 式的 per-turn 并发隔离等价物。
3. **cron 已暴露的致命限制会同样卡住 IM**:cron 只在 **TUI 交互模式**下工作——headless(`coco -p`)和 SDK 模式**没有 queue-drain pump**。IM 网关必须是一个持久 daemon 且带 drain pump,而 coco-rs 目前**没有这样的一等公民 daemon 模式**。
4. **会话键与跨平台连续性从零开始**:`coco-session` 是单 project 的 JSONL,按 project 键;没有 `build_session_key((platform,chat,user,thread))`、没有 `mirror.py` 等价物、没有 SQLite FTS5 路由索引。
5. **无用户身份/授权/pairing 原语**:coco-rs 的权限系统面向本地工具审批,不面向"多聊天用户身份";pairing、per-platform allowlist、role-based grant 全部缺失。
6. **无平台适配器抽象**:没有 `BasePlatformAdapter` 等价 trait,没有 registry,没有格式化/媒体/流式协商层。

**判定**:coco-rs 的 `CoreEvent` 三层事件总线 + `CommandQueue`(带 `QueueOrigin`)+ SDK 控制协议,构成一个**干净但很薄的接入 seam**——足以作为"入站入队 / 出站订阅"的对接点。但 Event Hub/connector 本身作为地基价值有限(connector 是空的,Local Session Hub 是只读查看器)。真正要做 IM 网关,需要在 coco-rs 之上**新建一个 Hermes `GatewayRunner` 量级的多路复用 daemon 层 + 平台适配器抽象 + 会话键/授权子系统**,复用现有的是事件总线和命令队列这两条 seam,而不是 hub 代码。

---

## 3. 差距分析 + 可借鉴点(高层)

### 3.1 差距总览

| 能力 | Hermes | coco-rs | 差距级别 |
|---|---|---|---|
| 消费者 IM 平台接入 | ~30 适配器 / 20+ 产品 | **0** | 结构性空白 |
| 平台适配器抽象 | `BasePlatformAdapter`(4 抽象方法即可) | 无等价 trait | 缺失 |
| 插件化平台注册 | `register(ctx)` + 动态 `Platform` 枚举,零核心改动 | 无 | 缺失 |
| 单进程多平台多路复用 daemon | `GatewayRunner` + `contextvars` 隔离 | 每 session 一进程,无 drain pump daemon | 缺失(最根本) |
| 用户身份 / DM pairing / 分层授权 | 完整(hashed 码 + 多层 allowlist + role) | 无(仅本地工具权限) | 缺失 |
| 跨平台会话键 + 连续性 | `build_session_key` + `mirror.py` + FTS5 路由 | project 键 JSONL | 缺失 |
| 流式投递协商 + per-platform 格式化 | draft/edit/分段 + markdown_dialect | 无(仅 TUI 渲染 / SDK) | 缺失 |
| 语音 STT/TTS + 媒体 | 完整 | 无 | 缺失 |
| 投递韧性(dead-target 自愈 / 反刷) | 有 | 无 | 缺失 |
| serverless / scale-to-zero | 有(relay + Fly suspend) | 无 | 缺失 |
| 可复用的事件/命令 seam | `CoreEvent` + `CommandQueue` | **已有且干净** | coco-rs 相对优势 |

### 3.2 高层可借鉴点(细节见第 05 节)

1. **抄 `BasePlatformAdapter` 的抽象边界**:4 个抽象方法 + 富默认能力表面,是把跨平台硬逻辑收敛到基类的正确切法;在 Rust 里可映射为一个 `PlatformAdapter` trait + 默认方法。
2. **抄插件注册 + 能力描述符**:`PlatformEntry` 的钩子集(`env_enablement_fn`/`apply_yaml_config_fn`/`standalone_sender_fn`/`allowed_users_env`)+ relay 的 frozen `CapabilityDescriptor`(加法式版本演进)非常契合 coco-rs 已有的 registry/plugin 传统。
3. **复用 `CommandQueue` + `QueueOrigin` 作为入站 seam**:IM 入站入队即可复用现成的 steering/打断/优先级机制——但**前提是先补一个持久 daemon + queue-drain pump**(cron 的同一短板必须先解决)。
4. **复用 `CoreEvent` 三层总线作为出站 seam**:订阅 `Stream`/`Protocol` 层驱动投递,对接 `hub/connector` 当初为事件 egress 预留的位置。
5. **移植会话键与连续性子系统**:`build_session_key` 的确定性键 + `mirror.py` 的跨平台 transcript 镜像,是多平台连续性的关键,需要在 `coco-session` 之上新增 `(platform,chat,user,thread)` 维度。
6. **借鉴安全姿态**:DM pairing(hashed 码 + 常数时间比较 + 限流 + lockout)、relay HMAC 逐字节对齐、silence-narration 反 bot 互刷、SSRF-guarded 出站——这些是把 agent 暴露到公网聊天的必备护栏。

> ⚠️ 采纳前必须正视 Hermes 自身的教训(findings 已诚实标注):`run.py` 19K LoC / `base.py` 5.4K LoC 的 God-object、双重平台注册真相源(枚举成员 + `_missing_()` + if/elif 三处)、env 变量满天飞的跨进程协调、relay 的 EXPERIMENTAL 状态与 schema churn。coco-rs 若移植,应以 trait + registry 单一真相源、`RuntimeConfig` 而非散落 env、以及严格的模块尺寸纪律来规避这些坑。

---

> [← 自进化深剖](03-self-evolution-deep-dive.md) · [返回索引](README.md) · [可落地建议 →](05-recommendations.md)
