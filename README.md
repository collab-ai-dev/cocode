<div align="center">

# cocode

**A fast, multi-provider AI coding agent for your terminal.**

[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)
[![npm](https://img.shields.io/npm/v/@cocode-cli/cocode-cli?style=flat-square)](https://www.npmjs.com/package/@cocode-cli/cocode-cli)
[![Rust](https://img.shields.io/badge/rust-1.93.1-orange?style=flat-square&logo=rust)](coco-rs/rust-toolchain.toml)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-lightgrey?style=flat-square)](#install)

**English** · [简体中文](README.zh-CN.md)

[Quick start](#quick-start) · [Performance](#performance) · [Providers](#providers-and-authentication) · [Mixture of Agents](#mixture-of-agents) · [Docs](docs/) · [Changelog](CHANGELOG.md)

</div>

---

cocode is a terminal-first coding agent written in Rust. It reads and edits your
code, runs shell commands, searches the web, talks to MCP servers, spawns
subagents, and remembers what matters across sessions.

It ships as **one native binary** with no Node runtime, no Electron, and no
sidecar process. It is **not tied to one model vendor**: point it at Anthropic,
OpenAI, Google, xAI, DeepSeek, Groq, Z.ai, Volcengine, or anything that speaks
the OpenAI-compatible API — with an API key, or with a subscription you already
pay for.

```bash
npm install -g @cocode-cli/cocode-cli
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

## Why cocode

- **One native binary.** Rust, single process per session, statically linked on
  Linux. Nothing to install alongside it — not even `ripgrep`.
- **Bring your own model.** 12 built-in provider instances across 7 provider
  APIs, plus a generic OpenAI-compatible path for everything else. Model
  references are always explicit: `<provider>/<model_id>`.
- **API key *or* subscription.** Log in to a ChatGPT, Gemini Code Assist, or
  Grok subscription over OAuth, or just export an API key. Both work; pick per
  provider.
- **Eight model roles, not one model.** Route planning, exploration, review,
  memory, and subagents to different models — each with its own fallback chain,
  retry policy, and reasoning effort.
- **Mixture of Agents.** Fan a turn out to several models in parallel, then let
  an aggregator model do the work with their advice in hand. Bind it to any
  role.
- **Permissions you can actually reason about.** Explicit modes, scoped
  allow/deny rules, and an optional OS-level sandbox.
- **Extensible.** MCP servers, skills, plugins, hooks, custom subagents, and
  TypeScript + Python SDKs over a JSON-RPC protocol.

## Performance

cocode is a single native process. There is no Node runtime to boot, no
JavaScript to parse, and no server to attach to before the first frame renders.

<!-- BEGIN MEASURED -->
| Metric — one idle session | cocode |
| --- | --- |
| Memory, physical footprint | **37 MB** |
| Memory, resident set size (RSS) | **60 MB** |

Measured on an Apple M3 (macOS 15.7.3, 24 GB) against commit `88d1477`, using a
release build with jemalloc — the same flags the published binaries use. Six PTY
launches of the TUI in a clean project directory with default settings; the
median is reported. Physical footprint is what macOS Activity Monitor calls
"Memory"; RSS additionally counts shared pages.

Reproduce it yourself — the harness is in the repo, not a screenshot:

```bash
python3 scripts/bench-startup.py ./coco-rs/target/release/coco 6
```
<!-- END MEASURED -->

What makes it lean, concretely:

- **No managed runtime.** The npm package ships a small JavaScript launcher that
  `exec`s the native binary; the agent itself is Rust. Nothing about the agent
  runs on Node.
- **Tuned allocator.** Shipped builds link jemalloc configured for a short-lived
  interactive process (1s dirty/muzzy decay, 4-arena cap) and purge the arenas at
  the end of every turn, returning freed pages to the OS instead of sitting on
  them.
- **Native terminal scrollback.** The TUI paints into your terminal's own
  scrollback with cell-level diffing, rather than owning an alternate screen and
  re-rendering a scroll buffer. Your terminal's native selection and scrolling
  keep working.
- **Optimized release builds.** Thin LTO, one codegen unit, symbols stripped;
  statically linked against musl on Linux.

## Install

**npm** (recommended):

```bash
npm install -g @cocode-cli/cocode-cli
cocode-cli --version
```

The package installs a JavaScript launcher plus the native `coco` binary for
your platform.

| Platform | Supported |
| --- | --- |
| Linux x86_64 | ✅ `x86_64-unknown-linux-musl` |
| Linux aarch64 | ✅ `aarch64-unknown-linux-musl` |
| macOS Apple Silicon | ✅ `aarch64-apple-darwin` |
| macOS Intel | ❌ not published |
| Windows | ❌ not published |

**From source** (Rust 1.93.1, pinned by `rust-toolchain.toml`):

```bash
git clone https://github.com/collab-ai-dev/cocode.git
cd cocode/coco-rs
just coco                      # build and launch the TUI
```

> **A note on names.** The binary is `coco`. The npm launcher is `cocode-cli`.
> `--help` prints `cocode`. They are the same program.

## Quick start

cocode has no default model — you must choose one. The shortest path:

```bash
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

To make it permanent, write `~/.cocode/settings.json`:

```jsonc
{
  "models": {
    // Required. Every other role falls back to this one.
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

Then:

```bash
cocode-cli                                  # interactive TUI
cocode-cli -p "Summarize this repository"   # one-shot, non-interactive
cocode-cli -C /path/to/project              # run against another directory
cocode-cli --continue                       # resume the last conversation
```

`-p` implies non-interactive mode; so does a non-TTY stdin or stdout, which is
what makes cocode scriptable in CI.

See [getting started](docs/getting-started.md) for a guided tour.

## Providers and authentication

Every model is addressed as `<provider>/<model_id>`. Providers come from a
built-in catalog, and you can add your own in
[`~/.cocode/providers.json`](docs/providers-and-auth.md).

Built-in providers:

| Provider | Auth | Notes |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | Claude models |
| `openai` | `OPENAI_API_KEY` | GPT-5 family, Responses API |
| `openai-chatgpt` | **subscription login** | Your ChatGPT plan |
| `google` | `GOOGLE_API_KEY` | Gemini |
| `gemini-code-assist` | **subscription login** | Gemini Code Assist |
| `xai` | `XAI_API_KEY` | Grok models |
| `grok` | **subscription login** | Your Grok plan |
| `deepseek-openai` | `DEEPSEEK_API_KEY` | DeepSeek, OpenAI-compatible |
| `deepseek-anthropic` | `DEEPSEEK_API_KEY` | DeepSeek, Anthropic-compatible |
| `groq` | `GROQ_API_KEY` | |
| `zai` | `ZAI_API_KEY` | |
| `volcengine` | `ARK_API_KEY` | |

Anything else that speaks the OpenAI-compatible API works too — add it with the
`/provider` wizard or by hand.

**Subscription login** uses OAuth and stores credentials outside your shell
history:

```bash
coco login openai     # ChatGPT subscription
coco login gemini     # Gemini Code Assist
coco login grok       # Grok subscription (device code — works over SSH)
```

`coco login grok` uses a device-code flow, so it works on a headless box with no
browser. Inside a session, bare `/login` opens a picker and completes the flow
without a restart.

**API keys** are read from the environment variable each provider declares
(`env_key`), which always wins over an `api_key` written into `providers.json`.
Keep keys in your environment or a secret manager.

Full details: [providers and authentication](docs/providers-and-auth.md).

## Models and roles

cocode routes different work to different models. Only `main` is required;
everything else falls back to it.

| Role | Used for |
| --- | --- |
| `main` | The primary conversation and coding agent |
| `plan` | Plan mode |
| `fast` | Cheap helper calls, like title generation |
| `explore` | Read-only codebase exploration |
| `review` | Review-oriented subagent work |
| `subagent` | Generic spawned subagents |
| `memory` | Memory extraction and recall |
| `hook_agent` | Agents invoked by hooks |

Each role takes a fallback chain, a retry/recovery policy, and per-slot
reasoning effort — so a fallback can think harder than the primary:

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

Full details: [models and MoA](docs/models-and-moa.md).

## Mixture of Agents

MoA is a virtual provider. Bind a role to `moa/<preset>` and every call on that
role becomes: **fan out to N reference models in parallel → hand their combined
advice to an aggregator model → the aggregator does the real work and owns every
tool call.**

```bash
coco moa configure default \
  --aggregator anthropic/claude-sonnet-4-6 \
  --reference openai/gpt-5-5 \
  --reference deepseek-openai/deepseek-v4-pro \
  --default

coco moa list
```

Then use it anywhere a model is selected:

```
/model moa/default          # bind the main role to the preset
/model plan moa/default     # or just plan mode
/moa <prompt>               # or run one prompt through it, changing nothing
```

Reference models are advisors: they never execute tools, and a reference that
fails or times out degrades to an inline note instead of failing your turn. You
can point references at up to 8 models, and choose whether they re-run every
loop iteration (`per_iteration`) or once per user turn (`user_turn`).

If you never configure a preset, cocode synthesizes a `default` one from your
existing roles — `main` aggregates, `review` and `fast` advise.

Full details: [Mixture of Agents](docs/models-and-moa.md#mixture-of-agents).

## What else is in the box

- **Plan mode** — the agent researches and proposes before it is allowed to
  edit, and can swap to a different model for the duration.
- **Goals** — `/goal <condition>` sets a condition the agent checks before it is
  allowed to stop, with an autonomous supervisor and explicit turn budgets.
- **Subagents** — built-in `Explore`, `Plan`, and `general-purpose` agents, plus
  your own as markdown files in `.cocode/agents/`.
- **Workflows** — deterministic multi-agent orchestration scripts in JavaScript,
  run on an embedded QuickJS engine.
- **MCP** — stdio and HTTP/SSE servers, with OAuth for the ones that need it.
- **Skills, plugins, hooks** — bundled/project/user skills, `PLUGIN.toml`
  plugins with marketplaces, and Pre/PostToolUse hooks.
- **Memory** — `CLAUDE.md` / `AGENTS.md` discovery up the directory tree, plus
  `.cocode/rules/`.
- **SDKs** — TypeScript and Python clients over a JSON-RPC 2.0 protocol.
- **Sandbox** — optional OS-level enforcement (Seatbelt / bubblewrap) around
  shell commands.
- **Voice input** — speech-to-text dictation, remote or on-device Whisper.

Some of these are experimental or off by default. The
[configuration guide](docs/configuration.md#feature-gates) lists every feature
gate with its real default.

## Documentation

| Guide | What's in it |
| --- | --- |
| [Getting started](docs/getting-started.md) | Install, first run, first real task |
| [Configuration](docs/configuration.md) | Config files, settings, feature gates |
| [Providers and auth](docs/providers-and-auth.md) | Every provider, API keys, OAuth login |
| [Models and MoA](docs/models-and-moa.md) | Roles, fallbacks, effort, Mixture of Agents |
| [CLI reference](docs/cli-reference.md) | Every flag and subcommand |
| [Slash commands](docs/slash-commands.md) | Every in-session command |
| [Tools](docs/tools.md) | What the model can call |
| [Permissions](docs/permissions.md) | Modes, rules, bypass |
| [Sandbox](docs/sandbox.md) | OS-level command isolation |
| [MCP](docs/mcp.md) | Model Context Protocol servers |
| [Memory](docs/memory.md) | CLAUDE.md, rules, session memory |
| [Extending](docs/extending.md) | Skills, plugins, hooks |
| [Subagents and teams](docs/subagents-and-teams.md) | Built-in and custom agents |
| [SDK](docs/sdk.md) | TypeScript and Python clients |
| [Troubleshooting](docs/troubleshooting.md) | When something goes wrong |

## Repository layout

| Path | Contents |
| --- | --- |
| `coco-rs/` | The Rust workspace: CLI, TUI, providers, tools, services |
| `coco-cli/` | npm packaging wrapper for the native binary |
| `coco-sdk/` | Protocol schemas and the TypeScript / Python SDKs |
| `docs/` | User documentation |
| `docs/internal/` | Internal design notes — historical, may be stale |

## Development

```bash
cd coco-rs
just quick-check    # fmt + lints + type check — use this while iterating
just pre-commit     # the full gate, including the test suite — run once, before committing
```

`just pre-commit` compiles every test binary in the workspace and is much slower
than `quick-check`. Read [`CLAUDE.md`](CLAUDE.md) before making changes; each
crate has its own `CLAUDE.md` with local invariants.

## License

[Apache-2.0](LICENSE).
