<div align="center">

# cocode

**快速、多模型供应商的终端 AI 编程助手。**

[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)
[![npm](https://img.shields.io/npm/v/@cocode-cli/cocode-cli?style=flat-square)](https://www.npmjs.com/package/@cocode-cli/cocode-cli)
[![Rust](https://img.shields.io/badge/rust-1.93.1-orange?style=flat-square&logo=rust)](coco-rs/rust-toolchain.toml)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-lightgrey?style=flat-square)](#安装)

[English](README.md) · **简体中文**

[快速开始](#快速开始) · [性能](#性能) · [模型供应商](#模型供应商与认证) · [Mixture of Agents](#mixture-of-agents混合智能体) · [文档](docs/) · [更新日志](CHANGELOG.md)

</div>

---

cocode 是一个用 Rust 编写的终端优先编程助手。它能读写代码、执行 shell 命令、搜索
网页、对接 MCP 服务、派生子智能体，并在多次会话之间记住重要的上下文。

它以**单个原生二进制**分发：没有 Node 运行时，没有 Electron，也没有额外的常驻进程。
它同样**不绑定任何单一模型厂商**：Anthropic、OpenAI、Google、xAI、DeepSeek、Groq、
智谱（Z.ai）、火山引擎，以及任何兼容 OpenAI 接口的服务都可以接入 —— 既可以用
API Key，也可以直接用你已经付费的订阅账号。

```bash
npm install -g @cocode-cli/cocode-cli
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

> 文档目前仅提供英文版：[docs/](docs/)。本页是 README 的中文版本。

## 为什么选择 cocode

- **单个原生二进制。** Rust 编写，每个会话一个进程，Linux 下静态链接。无需另外安装
  任何东西 —— 连 `ripgrep` 都不需要。
- **模型自由。** 内置 12 个模型供应商实例、覆盖 7 种供应商 API，另有通用的
  OpenAI 兼容通道接入其余一切。模型引用始终是显式的：`<provider>/<model_id>`。
- **API Key 与订阅登录都支持。** 可以通过 OAuth 登录 ChatGPT、Gemini Code Assist
  或 Grok 订阅，也可以只导出一个 API Key。两条路都通，且可按供应商分别选择。
- **八个模型角色，而不是一个模型。** 规划、探索、评审、记忆、子智能体可以分别路由到
  不同的模型，每个角色都有独立的降级链、重试策略和推理强度。
- **Mixture of Agents。** 把一次请求并发扇出给多个模型，再让聚合模型带着它们的建议
  去真正干活。可绑定到任意角色。
- **权限体系清晰可控。** 明确的权限模式、带作用域的允许/拒绝规则，以及可选的操作系统
  级沙箱。
- **可扩展。** MCP 服务、Skills、插件、Hooks、自定义子智能体，以及基于 JSON-RPC 协议
  的 TypeScript 与 Python SDK。

## 性能

cocode 是单个原生进程。启动时没有 Node 运行时要加载，没有 JavaScript 要解析，
也不需要先连上某个服务端才能渲染出第一帧。

<!-- BEGIN MEASURED -->
| 指标 —— 单个空闲会话 | cocode |
| --- | --- |
| 内存占用（physical footprint） | **37 MB** |
| 内存占用（RSS） | **60 MB** |

测试环境为 Apple M3（macOS 15.7.3，24 GB），对应提交 `88d1477`，使用带 jemalloc 的
release 构建 —— 与正式发布二进制完全相同的编译参数。在干净的项目目录、默认配置下，
通过 PTY 启动 TUI 共 6 次，取中位数。physical footprint 就是 macOS 活动监视器里
显示的“内存”；RSS 还会额外计入共享页。

这些数字可以自己复现 —— 测试脚本就在仓库里，而不是一张截图：

```bash
python3 scripts/bench-startup.py ./coco-rs/target/release/coco 6
```
<!-- END MEASURED -->

它之所以轻量，具体原因是：

- **没有托管运行时。** npm 包里只有一个用于启动的 JavaScript 壳，它直接 `exec`
  原生二进制；智能体本身是 Rust，不跑在 Node 上。
- **调优过的内存分配器。** 发行版链接 jemalloc，并按“短生命周期交互进程”调参
  （dirty/muzzy 衰减 1 秒、arena 上限 4 个），且在每轮对话结束时主动 purge，
  把释放的页真正还给操作系统，而不是一直占着。
- **原生终端回滚缓冲。** TUI 直接绘制到终端自带的 scrollback，并做单元格级 diff，
  而不是占用 alternate screen 再自己维护一套滚动缓冲。因此终端原生的选中与滚动
  依然可用。
- **优化过的发行构建。** Thin LTO、单 codegen unit、剥离符号；Linux 下静态链接 musl。

## 安装

**npm**（推荐）：

```bash
npm install -g @cocode-cli/cocode-cli
cocode-cli --version
```

该 npm 包会安装一个 JavaScript 启动器，以及对应平台的原生 `coco` 二进制。

| 平台 | 是否支持 |
| --- | --- |
| Linux x86_64 | ✅ `x86_64-unknown-linux-musl` |
| Linux aarch64 | ✅ `aarch64-unknown-linux-musl` |
| macOS Apple Silicon | ✅ `aarch64-apple-darwin` |
| macOS Intel | ❌ 未发布 |
| Windows | ❌ 未发布 |

**从源码构建**（Rust 1.93.1，由 `rust-toolchain.toml` 固定）：

```bash
git clone https://github.com/collab-ai-dev/cocode.git
cd cocode/coco-rs
just coco                      # 构建并启动 TUI
```

> **关于名字。** 二进制叫 `coco`，npm 启动器叫 `cocode-cli`，`--help` 里显示的是
> `cocode`。它们是同一个程序。

## 快速开始

cocode 没有默认模型，必须自己选一个。最短路径：

```bash
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

想固化下来，就写入 `~/.cocode/settings.json`：

```jsonc
{
  "models": {
    // 必填。其余所有角色都会回退到它。
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

然后：

```bash
cocode-cli                                  # 交互式 TUI
cocode-cli -p "总结这个仓库"                 # 一次性、非交互
cocode-cli -C /path/to/project              # 指定工作目录
cocode-cli --continue                       # 继续上一次会话
```

`-p` 会自动进入非交互模式；stdin 或 stdout 不是 TTY 时同样如此 —— 这正是 cocode
可以直接用在 CI 脚本里的原因。

更详细的上手过程见 [getting started](docs/getting-started.md)（英文）。

## 模型供应商与认证

每个模型都用 `<provider>/<model_id>` 显式引用。供应商来自内置目录，你也可以在
[`~/.cocode/providers.json`](docs/providers-and-auth.md) 里添加自己的。

内置供应商：

| 供应商 | 认证方式 | 说明 |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | Claude 系列 |
| `openai` | `OPENAI_API_KEY` | GPT-5 系列，Responses API |
| `openai-chatgpt` | **订阅登录** | 你的 ChatGPT 套餐 |
| `google` | `GOOGLE_API_KEY` | Gemini |
| `gemini-code-assist` | **订阅登录** | Gemini Code Assist |
| `xai` | `XAI_API_KEY` | Grok 系列 |
| `grok` | **订阅登录** | 你的 Grok 套餐 |
| `deepseek-openai` | `DEEPSEEK_API_KEY` | DeepSeek，OpenAI 兼容 |
| `deepseek-anthropic` | `DEEPSEEK_API_KEY` | DeepSeek，Anthropic 兼容 |
| `groq` | `GROQ_API_KEY` | |
| `zai` | `ZAI_API_KEY` | |
| `volcengine` | `ARK_API_KEY` | |

任何兼容 OpenAI 接口的服务同样可用 —— 用 `/provider` 向导添加，或手写配置。

**订阅登录**走 OAuth，凭据不会留在 shell 历史里：

```bash
coco login openai     # ChatGPT 订阅
coco login gemini     # Gemini Code Assist
coco login grok       # Grok 订阅（设备码，SSH 下可用）
```

`coco login grok` 使用设备码流程，因此在没有浏览器的远程机器上也能完成登录。
在会话内直接输入 `/login` 会打开选择器，无需重启即可完成认证。

**API Key** 从每个供应商声明的环境变量（`env_key`）读取；它的优先级高于写在
`providers.json` 里的 `api_key`。请把密钥放在环境变量或密钥管理器中。

完整说明：[providers and authentication](docs/providers-and-auth.md)（英文）。

## 模型角色

cocode 会把不同类型的工作路由到不同模型。只有 `main` 是必填的，其余角色都会回退到它。

| 角色 | 用途 |
| --- | --- |
| `main` | 主对话与主力编程智能体 |
| `plan` | 规划模式 |
| `fast` | 廉价的辅助调用，例如生成标题 |
| `explore` | 只读的代码库探索 |
| `review` | 偏评审类的子智能体工作 |
| `subagent` | 通用子智能体 |
| `memory` | 记忆抽取与召回 |
| `hook_agent` | 由 Hook 触发的智能体 |

每个角色都可以配置降级链、重试/恢复策略，以及**按槽位**设置的推理强度 —— 也就是说
降级模型可以比主模型想得更久：

```jsonc
{
  "models": {
    "main": {
      "primary": { "provider": "anthropic", "model_id": "claude-sonnet-4-6" },
      "fallbacks": [
        { "provider": "deepseek-openai", "model_id": "deepseek-v4-pro", "effort": "high" }
      ]
    },
    "fast": "groq/llama-3.3-70b-versatile"
  }
}
```

完整说明：[models and MoA](docs/models-and-moa.md)（英文）。

## Mixture of Agents（混合智能体）

MoA 是一个**虚拟供应商**。把某个角色绑定到 `moa/<preset>` 之后，该角色上的每次模型
调用都会变成：**并发扇出给 N 个参考模型 → 把它们的综合建议交给聚合模型 → 由聚合模型
真正执行本轮任务并负责所有工具调用。**

```bash
coco moa configure default \
  --aggregator anthropic/claude-sonnet-4-6 \
  --reference openai/gpt-5-5 \
  --reference deepseek-openai/deepseek-v4-pro \
  --default

coco moa list
```

随后就可以在任何能选模型的地方使用它：

```
/model moa/default          # 把 main 角色绑定到该预设
/model plan moa/default     # 或只绑定规划模式
/moa <prompt>               # 或只让单条 prompt 走一次 MoA，不改动任何绑定
```

参考模型只是顾问：它们不执行任何工具；某个参考模型失败或超时，只会在建议里降级成
一条内联提示，而不会让你这一轮失败。参考模型最多 8 个，并且可以选择每轮循环都重新
询问（`per_iteration`），还是每个用户回合只问一次（`user_turn`）。

如果你从未配置过预设，cocode 会用你已有的角色合成一个 `default`：`main` 做聚合，
`review` 和 `fast` 做参考。

完整说明：[Mixture of Agents](docs/models-and-moa.md#mixture-of-agents)（英文）。

## 还有什么

- **规划模式** —— 先调研并给出方案，经你确认后才允许改动，且可在规划期间切换到
  另一个模型。
- **Goals** —— `/goal <条件>` 设定一个“停止前必须满足的条件”，配合自主监督器与
  明确的回合预算。
- **子智能体** —— 内置 `Explore`、`Plan`、`general-purpose`，也可以在
  `.cocode/agents/` 下用 markdown 定义自己的。
- **Workflows** —— 用 JavaScript 编写的确定性多智能体编排脚本，运行在内嵌的
  QuickJS 引擎上。
- **MCP** —— 支持 stdio 与 HTTP/SSE 服务，并为需要的服务提供 OAuth。
- **Skills、插件、Hooks** —— 内置/项目/用户级 Skills，带市场的 `PLUGIN.toml`
  插件，以及 Pre/PostToolUse Hooks。
- **记忆** —— 沿目录树自动发现 `CLAUDE.md` / `AGENTS.md`，以及 `.cocode/rules/`。
- **SDK** —— 基于 JSON-RPC 2.0 协议的 TypeScript 与 Python 客户端。
- **沙箱** —— 可选的操作系统级隔离（Seatbelt / bubblewrap）。
- **语音输入** —— 语音转文字听写，支持远程或本地 Whisper。

其中部分能力是实验性的或默认关闭。每个特性开关的真实默认值见
[configuration 指南](docs/configuration.md#feature-gates)。

## 文档

全部文档为英文：

| 指南 | 内容 |
| --- | --- |
| [Getting started](docs/getting-started.md) | 安装、首次运行、第一个真实任务 |
| [Configuration](docs/configuration.md) | 配置文件、设置、特性开关 |
| [Providers and auth](docs/providers-and-auth.md) | 全部供应商、API Key、OAuth 登录 |
| [Models and MoA](docs/models-and-moa.md) | 角色、降级、推理强度、混合智能体 |
| [CLI reference](docs/cli-reference.md) | 全部命令行参数与子命令 |
| [Slash commands](docs/slash-commands.md) | 全部会话内命令 |
| [Tools](docs/tools.md) | 模型可调用的工具 |
| [Permissions](docs/permissions.md) | 权限模式、规则、绕过 |
| [Sandbox](docs/sandbox.md) | 命令级操作系统隔离 |
| [MCP](docs/mcp.md) | Model Context Protocol 服务 |
| [Memory](docs/memory.md) | CLAUDE.md、规则、会话记忆 |
| [Extending](docs/extending.md) | Skills、插件、Hooks |
| [Subagents and teams](docs/subagents-and-teams.md) | 内置与自定义智能体 |
| [SDK](docs/sdk.md) | TypeScript 与 Python 客户端 |
| [Troubleshooting](docs/troubleshooting.md) | 出问题时怎么排查 |

## 仓库结构

| 路径 | 内容 |
| --- | --- |
| `coco-rs/` | Rust workspace：CLI、TUI、供应商、工具、服务 |
| `coco-cli/` | 原生二进制的 npm 打包壳 |
| `coco-sdk/` | 协议 schema 与 TypeScript / Python SDK |
| `docs/` | 用户文档 |
| `docs/internal/` | 内部设计笔记 —— 属于历史资料，可能已过时 |

## 开发

```bash
cd coco-rs
just quick-check    # fmt + lint + 类型检查 —— 迭代时用这个
just pre-commit     # 完整关卡（含测试套件）—— 提交前跑一次即可
```

`just pre-commit` 会编译 workspace 里所有测试二进制，比 `quick-check` 慢得多。
改代码前请先读 [`CLAUDE.md`](CLAUDE.md)；每个 crate 还有自己的 `CLAUDE.md`，
记录了该 crate 的局部约束。

## 许可证

[Apache-2.0](LICENSE)。
