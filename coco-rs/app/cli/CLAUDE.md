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
| `project_services::ProjectServices` | Project-rooted plugin catalog plus command, skill, hook, MCP, and LSP discovery shared by sessions with the same project root |

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
Its AppServer registry stores `LocalAppSessionHandle` snapshots rather than
empty `()` handles. Installed runtime snapshots carry the current app/cli
`SessionHandle`, whose session id is an immutable snapshot; legacy retarget
paths must call `SessionHandle::snapshot_current()` before re-installing. Close
cascade logic remains retarget-safe until Phase B removes fused runtime
in-place retargeting. Local `session/resume` uses `AppServer::spawn_replace`
when the previous live session has an interactive local surface, and falls back
to close-then-register when no replace caller surface exists. Re-installing a
runtime-backed handle for an already-live local session refreshes the registry
handle without changing surface routing.
When constructed with a `HubConnectorSender`, the local outbound forwarder
clones each AppServer-stamped `SessionEnvelope` to the Hub connector queue
after routing it locally. Startup-owned Hub worker construction/configuration
is now wired for TUI/headless through `RuntimeConfig.event_hub.url`
(`event_hub_url`, `COCO_EVENT_HUB_URL`, or `--event-hub-url`), and both paths
flush the worker during normal shutdown. `--serve-hub` / `--hub-port` are
accepted by all builds; the default build emits the documented missing-feature
diagnostic, while the `serve-hub` Cargo feature starts an embedded
SQLite-backed `coco-hub-server` and fills `event_hub_url` with its local
WebSocket endpoint. SDK/NDJSON egress is wired separately because that path
does not use the local AppServer forwarder: the SDK single-writer serializer
clones each SDK-visible protocol notification into the same Hub connector
queue when configured, preserving NDJSON output behavior.
For the TUI, `start_passive_event_pump` attaches a separate passive local
surface and continuously forwards bridge-routed `CoreEvent`s into the TUI
event channel. Keep the interactive surface for server-request ownership; use
the passive pump for ordinary event delivery.
Use `install_session_runtime` when TUI/headless have already built a
`SessionRuntime`; it snapshots the existing session id/cwd/model into the
shared handler state instead of issuing a fresh `session/start`, installs the
runtime's `SessionManager` so local `session/list` / `session/read` see
persisted transcripts, and installs a `QueryEngineRunner` so local
`turn/start` requests have the same engine runner as the SDK bridge.
Local `session/start` and `session/resume` register the session in the local
`AppServer` registry; resume first closes any previous local live slot for the
single-session bridge so the resumed session can load without leaking registry
state. Local `session/archive` drives the AppServer close path so attached
surfaces receive `SessionEnded` lifecycle notifications.
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
Headless embedding/test callers must use `run_chat_with_options` with an
explicit `RunChatOptions::cwd` unless `Cli::cwd` is set; only `main.rs` reads
process cwd at startup and passes that snapshot into headless execution. SDK
mode also installs the same startup cwd into `SdkServerState`, so pre-session
`session/start`, `config/read`, and `config/value/write` requests do not fall
back to a relative process cwd.
TUI `/reload-plugins` routes through this local AppServer client
(`ServerClient::plugin_reload`) while preserving the TUI toast and
command-palette refresh behavior. TUI `/hooks reload` uses the same local
client path (`ServerClient::hook_reload`) instead of directly mutating the
runtime from the TUI runner.
TUI `/context` also routes through the local client
(`ServerClient::context_usage`); the bridge refreshes its snapshot from the
live `SessionRuntime` immediately before dispatch so history-sensitive reports
see current state.
TUI `/cost` and `/status` route through local AppServer `session/cost` and
`session/status`, so live usage/status observability uses the same handler
boundary instead of direct TUI runtime reads.
SDK `/cost`, `/status`, `/dream`, `/summary`, `/btw`, `/compact`, and
`/goal` slash sentinels are intercepted in `turn/start` before a normal runner
task is spawned. Cost/status reuse the same AppServer `session/cost` /
`session/status` handlers to append meta output; dream/summary call the
installed `MemoryRuntime` from the handler boundary and silently no-op when
auto-memory is unavailable; `/btw` uses the installed fork dispatcher or emits
the same transcript-only degraded response when no dispatcher is installed;
`/compact` runs a handler-owned manual compaction task against the installed
`SessionRuntime`; `/goal status` and `/goal clear` complete at the handler
boundary while `/goal <condition>` installs the managed Stop hook there before
falling through to the normal runner with the kickoff prompt.
TUI permission-mode changes route through
`ServerClient::set_permission_mode`; the bridge attaches a local interactive
surface and drains forwarded `PermissionModeChanged` events back into the TUI
event channel after dispatch.
TUI fast-mode toggles route through `ServerClient::config_apply_flags` with
the `fast_mode` setting; the SDK handler mutates the installed runtime's
engine config and emits `FastModeChanged` from the AppServer path.
TUI Ctrl+T thinking-level changes route through `ServerClient::set_thinking`;
the SDK handler updates the installed runtime's engine config and emits
`ModelRoleChanged` from the AppServer path.
TUI `/model` picker role/provider/model overrides route through
`ServerClient::set_model_role`; the SDK handler applies the live
`SessionRuntime` role override and emits `ModelRoleChanged`, while the TUI
keeps only the picker confirmation/history message.
TUI `/permissions` editor, `/permissions allow|deny`, approval always-allow,
and `/add-dir` updates route through `ServerClient::apply_permission_update`;
the SDK handler applies the live permission base and persists writable
destinations, while the TUI refreshes the editor overlay from disk afterward
for editor edits. `/permissions reset` routes through
`ServerClient::reset_session_permission_rules`, clearing only session-scoped
live allow/deny rules.
TUI `/color` changes route through `ServerClient::set_agent_color`; the SDK
handler updates the installed runtime's live app-state color.
TUI teammate current-work interrupt now routes through
`ServerClient::agent_interrupt_current_work`, keeping that runtime-control
request on the same local AppServer path as the SDK handler.
TUI teammate/subagent cancellation routes through local AppServer
`ServerClient::stop_task`; the SDK handler uses the installed `TaskRuntime`
when present and only falls back to active-turn cancellation for legacy
SDK-only sessions with no installed `SessionRuntime`.
TUI Ctrl+B background-all foreground tasks routes through
`ServerClient::background_all_tasks`, which dispatches the AppServer
`control/backgroundAllTasks` request and returns the ids that transitioned.
The `/tasks cancel <id>` slash command uses the same local
`ServerClient::stop_task` path; `/tasks list` and `/tasks detail <id>` use the
same local AppServer seam through `ServerClient::task_list` and
`ServerClient::task_detail`.
TUI explicit `/rewind` keeps conversation truncation local to the TUI runner,
but its file-restore half routes through `ServerClient::rewind_files`; the
bridge installs the runtime's `FileHistoryState` and config home into
`SdkServerState` when it adopts a `SessionRuntime`.
In-session TUI `/resume <id>` and `/branch` route through local
`ServerClient::session_resume`; the TUI keeps only target resolution/fork
creation, coordinator-mode reconciliation, and UI reset/history hydration.
TUI `/clear` still uses the runtime clear/retarget compatibility helper for
id rotation, then closes the old local AppServer live slot and re-installs the
post-clear runtime snapshot so local interactive/passive surfaces follow the
new session id.
TUI `/rename`, `/tag`, `/branch` fork-title persistence, and post-plan
auto-title persistence route session metadata writes through local AppServer
`session/rename` and `session/toggleTag`; auto-rename still resolves the name
locally before sending the metadata write request. SDK `/rename` slash
sentinels are intercepted in `turn/start`; explicit names and locally resolved
auto-rename candidates are persisted through the same AppServer
`session/rename` handler instead of direct runner writes.
The REPL bridge `ControlRequestHandler` keeps the explicit bypass guard for
permission-mode changes, and routes initialize, interrupt, set-model,
MCP-status, context-usage, and rewind-file controls through the same SDK
`dispatch_client_request` handler table instead of carrying bridge-only stubs.

## Runtime Paths

`crate::paths::runtime_paths()` is the app/cli boundary for project-scoped
paths. It reads `global_config::config_home()` plus
`EnvKey::CocoRemoteMemoryDir` and builds `coco_paths::RuntimePaths`.
Production code should derive transcript/session task-output/memory
`ProjectPaths` through `crate::paths::project_paths(cwd)`.

Project-scoped plugin catalogs and MCP server discovery go through
`project_services::ProjectServices`. Session command bootstrap/reload asks
`ProjectServices::build_command_registry(...)` so project plugin slash
commands are registered from the project-service catalog. Session skill
bootstrap/reload asks
`ProjectServices::build_skill_manager(config_home, session_cwd, gates)` so
project plugin skills and builtin plugin skills are folded into the same
manager behind the project-service boundary. Session hook bootstrap/reload
calls `ProjectServices::register_plugin_hooks()` for project plugin hooks
after settings hooks are layered. Agent search paths also come from
`ProjectServices::agent_search_paths(config_home, session_cwd)`, and plugin
reload refreshes the runtime's agent search paths before the agent catalog is
rebuilt. Session MCP bootstrap asks
`ProjectServices::mcp_servers(config_home, session_cwd)` so project-rooted
config/plugin MCP contributions stay tied to the project key while local MCP
config remains session-cwd scoped. LSP startup/reload likewise asks
`ProjectServices::lsp_servers()` for plugin-contributed server config before
prewarming the live manager.
`ProjectRegistry` caches `Arc<ProjectServices>` per `(config_home,
project_root)`. `ProcessRuntime::global()` is the process-level owner for the
`ProjectRegistryManager` background idle sweep, and startup threads the same
`Arc<ProcessRuntime>` into TUI, SDK, headless, session runtime, and LSP reload
paths. The sweep evicts only entries with no external strong references, so
live sessions keep their shared project services attached.

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
Hub: `--event-hub-url`, `--serve-hub`, `--hub-port`
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
