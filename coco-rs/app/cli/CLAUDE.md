# coco-cli

Top-level CLI: clap parser, binary entry, SDK (NDJSON over stdio), subcommand dispatch.
Depends on everything — wires registries, builds model runtime registry, starts TUI or SDK server.

## Key Types

| Type | Purpose |
|------|---------|
| `Cli` (clap `Parser`) | Binary name `coco`; see `lib.rs` for flags |
| `Commands` | Subcommands: Chat, Config, Resume, Sessions, Status, Doctor, Login, Mcp, Plugin, Daemon, Ps/Logs/Attach/Kill, RemoteControl, Sdk, ReleaseNotes, Upgrade, Agents, AutoMode |
| `{Config,Mcp,Plugin}Action` | Subcommand action enums |
| `sdk_server::SdkServer` | NDJSON control server (Commands::Sdk) |
| `sdk_server::SdkServer::run_app_server_connection` | SDK-server entrypoint for the AppServer bridge; reuses the server's transport, state, and external notification sources while delegating JSON-RPC ownership to `coco-app-server` |
| `sdk_server::app_server_bridge::AppServerSdkHandler` | Runtime-backed AppServer request handler shared by JSON-RPC SDK and local in-process adapters |
| `sdk_server::AppServerLocalBridge` | Local AppServer client bridge for TUI/headless cut-over: wires AppServer, LocalClientAdapter, ServerClient, shared handler, and event forwarding |
| `sdk_server::StdioTransport` | stdin/stdout NDJSON transport |
| `sdk_server::QueryEngineRunner` | Bridges `QueryEngine` to SDK control messages |
| `sdk_server::CliInitializeBootstrap` | Session bootstrap from `initialize` control request |
| `sdk::ModelUsage` + schemas | SDK wire types |
| `tui_runner::*` | Launches `coco-tui` after bootstrap |
| `model_factory::*` | Builds `Arc<dyn LanguageModelV4>` from provider/model config |
| `output::*` | Non-interactive output formatters (text/json/stream-json) |
| `project_services::ProjectServices` | Project-rooted plugin catalog plus MCP server discovery shared by sessions with the same project root |

## Startup Flow

1. `Cli::parse()` — clap parses argv (subcommand or default chat)
2. Fast-path subcommands (Config, Doctor, ReleaseNotes, Sessions, Upgrade) bypass QueryEngine
3. Interactive/print/SDK paths: load config, build `ModelRuntimeRegistry`, register tools + commands
4. `Sdk` → `sdk_server` over the AppServer JSON-RPC bridge (NDJSON over stdio,
   `initialize`/`interrupt`/`can_use_tool`/`set_permission_mode`/...)
5. `--non-interactive` (print mode) → local AppServer control bridge +
   local `turn/start` + `output::*` formatter
6. Interactive → local AppServer control bridge + `tui_runner::run`
   (launches `coco-tui`)

`sdk_server::spawn_sdk_outbound_writer` is the single writer for SDK
notifications, replies, and server requests. The AppServer cut-over bridge
feeds this writer so stream accumulation and wire-order guarantees stay
identical to the removed dispatcher loop. While SDK server-request emission
still uses `SdkServerState::send_server_request`, the AppServer bridge reader
must route matching legacy `Response`/`Error` messages through
`SdkServerState::resolve_server_request` before falling back to AppServer
adapter response handling.
The SDK handler, dispatcher, and approval-bridge tests run through
`SdkServer::run_app_server_connection`; the legacy `SdkServer::run` loop has
been removed, so SDK JSON-RPC ownership lives on the AppServer bridge path.
`AppServerSdkHandler` also implements the local in-process AppServer request
handler trait, so TUI/headless cut-over code can reuse the same exhaustive
`ClientRequest` dispatcher without adding another runtime dispatch table.
Local AppServer cut-over code must pair that handler with
`spawn_app_server_local_outbound_forwarder`, which routes handler-emitted
`CoreEvent`s back through `AppServer::route_envelope` using the current
session id from `SdkServerState`.
`AppServerLocalBridge` is the preferred local entrypoint: it owns the local
`AppServer`, `ServerClient`, shared handler, and outbound forwarder so
TUI/headless code does not duplicate adapter wiring.
For the TUI, `start_passive_event_pump` attaches a separate passive local
surface and continuously forwards bridge-routed `CoreEvent`s into the TUI
event channel. Keep the interactive surface for server-request ownership; use
the passive pump for ordinary event delivery.
Use `install_session_runtime` when TUI/headless have already built a
`SessionRuntime`; it snapshots the existing session id/cwd/model into the
shared handler state instead of issuing a fresh `session/start`, and installs a
`QueryEngineRunner` so local `turn/start` requests have the same engine runner
as the SDK bridge.
`turn/start` lifecycle events must use the same `TurnId` returned by the
synchronous `TurnStartResult`; `AppServerLocalBridge::start_turn_and_wait_for_end`
depends on that correlation and waits for the matching `TurnEnded` on the local
interactive surface. `TurnStartParams` carries optional base64 paste images,
slash metadata attachment text, explicit model selection, and thinking
overrides so TUI-local turn cut-over preserves prompt-command semantics.
Both headless and TUI bootstraps now create this bridge, install their
already-built `SessionRuntime`, and issue a local `keep_alive` request through
`ServerClient`. TUI normal submits and slash/palette prompt turns now start via
local AppServer `turn/start`; a passive completion monitor releases the TUI
`active_turn` slot while `turn/interrupt` handles AppServer-owned cancellation.
Headless `RunChatOutcome` assembly also starts the turn through local AppServer
and reconstructs its structured result from the aggregated session result,
runtime history, and usage snapshot. TUI queued prebuilt-history turns now pass
a serialized full-history override through local AppServer `turn/start`, so
permission retries, queued prompts, and prompt-mode bash follow-up turns no
longer call the engine directly from the TUI runner.
TUI `/reload-plugins` is the first runtime-control path routed through this
local AppServer client (`ServerClient::plugin_reload`) while preserving the TUI
toast and command-palette refresh behavior.
TUI `/context` also routes through the local client
(`ServerClient::context_usage`); the bridge refreshes its snapshot from the
live `SessionRuntime` immediately before dispatch so history-sensitive reports
see current state.
TUI permission-mode changes route through
`ServerClient::set_permission_mode`; the bridge attaches a local interactive
surface and drains forwarded `PermissionModeChanged` events back into the TUI
event channel after dispatch.
TUI teammate current-work interrupt now routes through
`ServerClient::agent_interrupt_current_work`, keeping that runtime-control
request on the same local AppServer path as the SDK handler.

## Runtime Paths

`crate::paths::runtime_paths()` is the app/cli boundary for project-scoped
paths. It reads `global_config::config_home()` plus
`EnvKey::CocoRemoteMemoryDir` and builds `coco_paths::RuntimePaths`.
Production code should derive transcript/session task-output/memory
`ProjectPaths` through `crate::paths::project_paths(cwd)`.

Project-scoped plugin catalogs and MCP server discovery go through
`project_services::ProjectServices`. Session MCP bootstrap asks
`ProjectServices::mcp_servers(config_home, session_cwd)` so project-rooted
config/plugin MCP contributions stay tied to the project key while local MCP
config remains session-cwd scoped.

`config_home` remains the root for user/global artifacts such as logs,
plugins, settings, output styles, models, task lists, and file-history
metadata. `coco-paths` stays pure: no env reads and no dependency on
`coco-config`.

## Flag Highlights

Session: `--prompt`, `--output-format`, `--input-format`, `--json-schema`, `--max-turns`, `--max-budget-usd`
Resume: `--continue`, `--resume`, `--fork-session`, `--session-id`, `--name`
Auth/Perms: `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-mode`, `--permission-prompt-tool`
Tools: `--allowed-tools`, `--disallowed-tools`, `--add-dir`
Config: `--settings`, `--setting-sources`, `--system-prompt`, `--append-system-prompt(-file)`, `--mcp-config`, `--strict-mcp-config`
Model: `--models.main`, `--fallback-model`, `--betas`, `--agent`, `--thinking`, `--thinking-budget`, `--max-thinking-tokens`, `--effort`
Worktree/bg: `--worktree`, `--bg`
SDK: `--replay-user-messages`, `--include-hook-events`, `--include-partial-messages`

## Stop Hooks Dispatch Order

Post-turn hooks fire from `coco_query::engine_finalize_turn` in this order:

1. **bareMode gate** — `--bare` mode skips all post-turn forks (no
   prompt suggestion, no memory extraction, no auto-dream). Used
   by SDK / scripted `-p` invocations that don't want background
   work after each turn.
2. **promptSuggestion** — fires unconditionally (subject to its
   own 9-step guard sequence in
   `coco_query::prompt_suggestion::try_generate_suggestion`).
3. **extractMemories** — fires when `MemoryConfig.extraction_enabled`
   AND `agent_id.is_none()` (subagents don't extract).
4. **autoDream** — fires when `MemoryConfig.dream_enabled` AND
   `agent_id.is_none()`. The 3-gate scheduler (`memory/src/service/dream.rs`)
   then internally checks 24h elapsed + 5 distinct sessions + PID
   lock before paying for the consolidation.

Each fork dispatches via `coco_query::forked_agent::ForkDispatcher`
(installed by `fork_dispatcher::install` at session bootstrap).
The dispatcher threads the parent's `CacheSafeParams` so the child's API
request prefix matches byte-for-byte. Per-fork `canUseTool` policies live in
`coco-memory::can_use_tool` (auto-mem + session-mem); promptSuggestion
+ side_question + agent_summary use `deny_all_handle`.

## Allocator (jemalloc)

jemalloc is the global allocator for release/distribution builds, installed via
`#[global_allocator]` in `src/main.rs` — opt-in behind the `jemalloc` Cargo
feature, never on Windows (no jemalloc-sys MSVC build). Tuning
(`dirty_decay_ms` / `muzzy_decay_ms` / `narenas`) is **baked into libjemalloc at
build time** via `JEMALLOC_SYS_WITH_MALLOC_CONF` in `.cargo/config.toml`
(`--with-malloc-conf`), which is why it applies on **both Linux and macOS** —
the exported `malloc_conf` symbol form would be ignored on macOS's `_rjem_`-
prefixed build. Per-knob meanings and defaults are documented inline in `.cargo/config.toml`.
Three ways to set the tuning, by when it binds:

- **Build-time baseline** (current). Edit the `JEMALLOC_SYS_WITH_MALLOC_CONF`
  string and rebuild. Setting that env at launch does nothing — it is consumed
  only by the jemalloc-sys build script.
- **Startup override, no rebuild.** jemalloc reads its own env at init (a later
  conf source, so it overrides the baked baseline — including `narenas`): set
  `MALLOC_CONF` on Linux, `_RJEM_MALLOC_CONF` on macOS (the `_rjem_`-prefixed
  build). Caveat: init runs before `main` (on the first allocation), so it must
  be present in the environment **before exec** — `coco` cannot set it for
  itself, and settings.json (parsed post-init) cannot drive it.
- **Live, in-process.** The `jemalloc` feature wires `tikv-jemalloc-{ctl,sys}`
  through the isolated `coco-utils-jemalloc` wrapper. Runtime support is
  intentionally limited to end-of-turn `arena.*.purge` (forced page
  reclamation, driven from `coco-tui`; see that crate + `utils/jemalloc`) plus
  `stats.*` reads for the memory perf log. Runtime decay tuning is not
  supported: do not expose `dirty_decay_ms`, `muzzy_decay_ms`, or `narenas`
  mutation through settings, slash commands, or live APIs.
