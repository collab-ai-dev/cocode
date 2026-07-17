# Changelog

All notable changes to cocode are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and cocode aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries describe user-visible behavior. Internal refactors appear only when
they change something you can observe.

## Unreleased

The `coco` binary and the `@cocode-cli/cocode-cli` package still report
`0.1.1`. Everything below is on `main` and awaiting a release tag.

### Added

- **Grok subscription login.** `coco login grok` authenticates an existing Grok
  subscription over an RFC 8628 device-code flow, so you can sign in from a
  headless box or over SSH without a loopback browser callback.
- **Mixture of Agents (MoA).** A virtual `moa` provider that fans a turn out to
  up to 8 reference models concurrently, then hands their combined advice to an
  aggregator model that does the real work and owns every tool call. Bind it to
  any model role with `/model moa/<preset>`, run one prompt through it with
  `/moa <prompt>`, or manage presets with `coco moa list|configure|delete`.
  Reference models that fail or time out degrade to an inline note rather than
  failing the turn.
- **Goal runtime with an autonomous supervisor.** `/goal <condition>` sets a
  condition the agent checks before it is allowed to stop; the supervisor then
  drives autonomous continuation turns under explicit turn and no-progress
  budgets. The `get_goal`, `report_goal_turn`, and `create_goal` tools surface
  goal state to the model.
- **xAI provider** (`xai`, `XAI_API_KEY`) and **Groq provider** (`groq`,
  `GROQ_API_KEY`).
- **GPT-5 family models** for the OpenAI provider, with catalog-id to wire-slug
  mapping (`gpt-5-5` → `gpt-5.5`) and Codex slug handling.
- **Voice input (speech-to-text dictation).** `/voice` toggles dictation, `F3`
  is push-to-talk, and `/voice-config` selects backend, language, and model.
  Supports a remote OpenAI-wire backend or on-device Whisper. Gated behind the
  `voice` feature.
- **TypeScript SDK** (`@coco-rs/coco-sdk`) alongside the Python SDK, both
  generated from the JSON schemas in `coco-sdk/schemas/`.
- **`/login` picker.** Bare `/login` lists every OAuth-capable provider and runs
  the flow in-session, without a restart. `coco login --import <path>` adopts an
  existing Codex CLI `auth.json` (read-only; the source file is never modified).
- **Per-slot reasoning effort.** Each model slot in a role carries its own
  `effort`, so a fallback model can think at a different level than the primary.
- **Permission-denial retry.** Recently denied tool calls can be retried from
  the TUI without re-prompting the turn.
- **`coco feedback`** prepares a pre-filled GitHub issue, optionally with logs.
- **Shell output rewriting.** Bash output passes through an embedded post-exec
  filter that trims noise before it reaches the model's context.
- **Recoverable tool-result offload.** Oversized tool results are stored whole
  and presented to the model as head+tail, so a large result costs context
  without losing recoverable content.
- **Memory diagnostics.** Opt-in TUI instrumentation reports RSS, virtual size,
  macOS physical footprint, and jemalloc arena stats.

### Changed

- **jemalloc is now the allocator** in shipped builds (`--features jemalloc`),
  tuned for a short-lived interactive process (1s dirty/muzzy decay, 4-arena
  cap) and purged at end of turn to return freed pages to the OS.
- Model picker restyled; `/provider` opens an add-provider wizard; modal input
  handles IME cursors correctly.
- The baseline permission status is now labeled **"manual mode on"** rather than
  describing the absence of an auto mode.
- Subagents display live cost and compaction hints while running.
- Task notifications render as typed events instead of free-form text.
- The SDK protocol is **JSON-RPC 2.0** over NDJSON.
- Memory prompts moved into Rust builders (previously templated).
- Task and todo semantics aligned with the TypeScript implementation.

### Security

- **A repository can no longer run shell commands through `api_key_helper`.**
  The setting is a command string executed with `sh -c`, and it was read from the
  merged settings — so a `.cocode/settings.json` in a cloned repository could
  supply it and have it run at initialize, with the stored credentials under
  `~/.cocode/auth/` in reach and the permission system nowhere near that path. It
  now reads only user-controlled layers. Narrow in practice: the path required an
  Anthropic main model on the remote/SDK host, not a local TUI session.
- **A repository can no longer disable your permission system.** Settings merge
  as `Plugin < User < Project < Local < Flag < Policy`, and startup read
  `permissions.default_mode` off the merged result — so a `.cocode/settings.json`
  committed to a cloned repository could start your session in
  `bypassPermissions`, auto-approving every tool call with no flag and no
  prompt. The `disable_bypass_mode` killswitch could be switched off the same
  way, overriding a user who had turned it on. Bypass posture now reads only
  sources the person running the agent controls (user, local, flag, policy),
  excluding the project layer — the rule auto-mode settings already followed.
- **A locally-built `--release` binary no longer reads the OS keychain.** The
  credential store picked its backend from `!cfg!(debug_assertions)`, which is
  also false for anyone's local `cargo build --release`, so development builds
  were treated as signed distribution artifacts and hit the macOS keychain
  (prompting on every rebuild). Provenance is now an explicit build flag set
  only by the release workflow; local builds are file-backed under
  `~/.cocode/auth/`, and `COCO_AUTH_CREDENTIAL_STORE` still forces the choice.
- **A cloned repository's `.mcp.json` no longer auto-connects its MCP
  servers.** Connecting an MCP server spawns whatever process (or contacts
  whatever URL) the config names, and repo-defined servers previously did so at
  session start with no gate of any kind. Project-scope servers now fail closed
  until approved — per server via `/mcp enable`, stored in your per-user
  `~/.cocode.json`, or wholesale via `enable_all_project_mcp_servers` /
  `allowed_mcp_servers`, which are honored only from settings layers the person
  running the agent controls (never from the repository's own settings).
- **Administrators can actually ban an MCP server now.** `denied_mcp_servers`
  existed in the settings schema but nothing read it; it is now enforced at
  activation time, unioned across every settings layer, and matches by exact
  name, exact stdio `command`, or URL prefix — so redefining a banned server
  under a different name does not dodge the ban. Writing `"disabled": true`
  into `managed-mcp.json`, which previously did nothing (the loader skipped the
  entry and kept the user's definition), now also keeps the server off, because
  the merge is single and unconditional and a legacy-`disabled` entry fail-safes
  off.

### Fixed

- **`/mcp disable` now actually disables the server.** Disabling wrote
  `"disabled": true` into the file that defined the server, but the loader
  *skipped* disabled entries instead of recording them — so any
  lower-precedence file defining the same name silently kept the server
  loading, and `/mcp list` could simultaneously report it disabled. Run/don't-
  run state is no longer part of the definition files at all: `/mcp
  enable|disable` writes a per-project toggle in `~/.cocode.json`, keyed by
  server name, which holds no matter which file defines the server. `/mcp
  list` and the session loader now derive from one merged view and one
  activation authority, so they cannot disagree. Config entries still carrying
  the removed `disabled` field are refused fail-safe (kept off, with a
  warning) rather than silently re-enabled; `/mcp enable` migrates them.

- **`coco config set` now writes the setting.** It printed `Would set '<key>' =
  '<value>'` and returned without touching the file, so every scripted
  configuration silently did nothing. Values are parsed as JSON where possible,
  so `true` lands as a boolean rather than a string, with a fallback for bare
  strings like a model id.
- **`/hooks` printed a config example that could not be loaded.** The example
  used Claude Code's nested shape, where a matcher wraps its own `hooks` array;
  this loader takes the handler inline. Following the instructions produced a
  file that failed to parse — and because one bad entry fails the whole settings
  source, every other hook in that file silently stopped firing. The example now
  matches the loader, a test feeds the printed text through it, and the parse
  error names the nested shape instead of only reporting a missing field.
- **`/mcp` read and wrote files the MCP loader never loads.** It listed servers
  from `settings.json` and wrote `add`/`enable`/`disable`/`remove` there, while
  the loader reads only the `mcp.json` family — so servers that were live went
  unlisted, and edits reported success while changing nothing. The command now
  goes through the loader's own file list.
- Usage statistics survive compaction instead of resetting.
- Usage accounting attributes cost to the correct session and role.
- Auto-compaction fallback behavior matches the documented parity target.
- The auto-mode permission classifier runs for gated tools that previously
  bypassed it.
- Plan-mode lifecycle transitions align with the documented state machine.
- Subagent resume continuity restored across session reload.
- Forking a session runs the engine on its own task, fixing a stack overflow.
- Foreground shell completions no longer emit spurious notifications.
- Queued steering input drains after a turn ends rather than being dropped.
- The composer no longer bounces as popups appear and disappear.
- Config hot-reload emits the configured path form for managed drop-in changes.
- Structured output is capped, and stop reasons are attributed correctly.

## 0.1.1 — 2026-06-22

### Added

- Published the `@cocode-cli/cocode-cli` npm package, which installs a small
  JavaScript launcher plus the native `coco` binary for your platform
  (Linux x64, Linux arm64, macOS Apple Silicon).
- Remote exec server (`coco exec-server`), exposing an executor over WebSocket
  or stdio.

### Changed

- Centralized product naming and config paths on `cocode` / `~/.cocode/`.
- Thinking-effort cycling reads model metadata rather than a fixed table.

## 0.1.0 — 2026-06-22

### Added

- Initial public release: the `coco-rs` Rust workspace (CLI, TUI, providers,
  tool runtime, permissions, MCP, memory, plugins, SDK protocol), the npm
  packaging wrapper, and CI.
