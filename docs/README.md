# cocode documentation

cocode is a fast, multi-provider AI coding agent for your terminal. If you are
new here, start with [Getting started](getting-started.md).

Documentation is English-only. The project README is also available in
[简体中文](../README.zh-CN.md).

## Start here

| Guide | What's in it |
| --- | --- |
| [Getting started](getting-started.md) | Install, pick a model, first real task |
| [Configuration](configuration.md) | Config files, merge order, feature gates |
| [Troubleshooting](troubleshooting.md) | Common failures and their real causes |

## Models and providers

| Guide | What's in it |
| --- | --- |
| [Providers and authentication](providers-and-auth.md) | The provider catalog, API keys, subscription login, adding your own endpoint |
| [Models and MoA](models-and-moa.md) | The eight model roles, fallback chains, reasoning effort, and Mixture of Agents |

## Using cocode

| Guide | What's in it |
| --- | --- |
| [CLI reference](cli-reference.md) | Every flag and subcommand, and how TUI/headless/SDK mode is chosen |
| [Slash commands](slash-commands.md) | Everything you can type in a session |
| [Tools](tools.md) | What the model is able to call, and how availability is gated |
| [Permissions](permissions.md) | Modes, allow/deny rules, and bypass |
| [Sandbox](sandbox.md) | Optional OS-level isolation for shell commands |
| [Memory](memory.md) | `CLAUDE.md` discovery, rules, and imports |

## Extending

| Guide | What's in it |
| --- | --- |
| [MCP](mcp.md) | Model Context Protocol servers and their tools |
| [Extending](extending.md) | Skills, plugins, and hooks |
| [Subagents and teams](subagents-and-teams.md) | Built-in and custom agents; experimental agent teams |
| [SDK](sdk.md) | Driving cocode from TypeScript or Python |

## Project

- [Changelog](../CHANGELOG.md) — what changed in each release
- [CLAUDE.md](../CLAUDE.md) — conventions and architecture for contributors
- [`docs/internal/`](internal/) — internal design notes and historical plans.
  **Not user documentation**, and parts are out of date; the code and these
  guides win over anything in there.

## A note on accuracy

Several tables in these guides — the provider catalog, feature gates, CLI flags,
tool names, and model roles — are generated from the source and checked in CI, so
they cannot silently drift from the code. Blocks marked
`<!-- BEGIN GENERATED: ... -->` are written by `just docs-gen`; edit the
generator, not the table.
