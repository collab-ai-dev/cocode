# coco-agent-host

Agent-session host shared by CLI and SDK clients. It owns session-runtime construction,
the in-process AppServer client facade, protocol-neutral AppServer request
handling for remote and local adapters, headless operations, and runtime
integrations. It does not own the `coco` process entrypoint or the TUI command
loop (those are `coco-cli` client composition), nor the SDK JSON-RPC/NDJSON
connection adapter (`coco-sdk-server`).

Tier-1 application-composition crate under the workspace error policy:
startup/application assembly may use `anyhow`; domain crates below it expose
typed errors and adapters translate failures to protocol results.

## Key Types

| Type | Purpose |
|------|---------|
| `AgentHostOptions` | Clap-independent application inputs mapped once by `coco-cli`. |
| `local_client::LocalServerClient` | In-process typed client over `LocalClientAdapter`; one instance owns one AppServer connection, while clones and session observers use in-memory channels. |
| `app_server_host::AppServerHostHandler` | Runtime-backed AppServer request handler shared by remote JSON-RPC and local in-process adapters. |
| `app_server_host::AppServerLocalBridge` | Preferred local entrypoint: owns the local `AppServer`, `LocalServerClient`, shared handler, and outbound forwarder so TUI/headless don't duplicate adapter wiring. |
| `app_server_host::AppServerHostState` | Shared host projection state: turn runner, startup/bootstrap data, session-manager projection, activity, runtime replacement, durable sequence allocation. |
| `app_server_host::SessionTurnExecutor` | Executes a turn against the already-selected `SessionHandle`; shared by remote AppServer, TUI, headless, and harness paths. |
| `app_server_host::CliInitializeBootstrap` | CLI startup data source for AppServer `initialize` responses. |
| `remote_host::HostBuilder` / `PreparedHost` | Remote AppServer host bootstrap for transport adapters; does not own SDK transport. |
| `remote_host::RemoteAppServerBridgeHost` | Narrow remote transport-facing capability handle for opening JSON-RPC/AppServer bindings without exposing raw host state. |
| `session_runtime::SessionRuntimeFactory` | Owned construction seam for building `SessionHandle`s from cloneable startup inputs and a target session id. |
| `session_runtime::SessionExecutionProfile` (`Primary` / `SideChatReadOnly`) + `HookExecutionPolicy` | Construction-time capability decision table read by the runtime installers (predicate methods: `persists_history`, `registers_pid`, `runs_goals`, …). `SideChatReadOnly` disables durable/background ownership and restricts hooks to tool-lifecycle only. |
| `session_controls::*` | Protocol-neutral runtime controls (tasks, status/cost, context usage); adapters map results to their clients. |
| `headless` (`headless/run`, `headless/support`) | Print-mode orchestration plus goal/slash, transcript, tool-filter, and additional-directory helpers. |
| `local_host::build_local_host` | Shared local AppServer host assembly for TUI + headless; local counterpart to `remote_host::HostBuilder`. |
| `coco_app_runtime::ProjectServices` | Project-rooted plugin/command/skill/hook/MCP/LSP discovery shared per project root (lives in `app/runtime`; see Runtime Paths). |

## Multi-Session Ownership

AppServer validates every `(connection, session)` target and its ReadOnly/Full
grant, then hands the handler one opaque `SessionHandle`. Runtime selection is never
repeated in a runner or handler. The runtime owns history, engine/app state,
MCP, reload supervisors, file history, active-turn cancellation, turn ids, and
aggregate turn accounting. `AppServerHostState` owns only process services and
projections: the runner implementation, bootstrap/factory inputs, persistence,
activity timestamps, and durable event sequence allocation. It must not mirror
runtime-owned capabilities — reach them only through the validated
`SessionHandle`.

Accepted connections own immutable initialize profiles, bounded outbound
writers, and connection-owned hook/MCP callback registration. Human replies
are accepted only when connection, session, and request id match pending
AppServer state. Any Full connection may reply and the first valid response
wins. Live resume never requires a peer to reproduce another connection's
callback profile. Runtime callback definitions may be prepared before publish,
but connection ownership, client hook invocation, and client-MCP setup begin
only after live Full attachment. Do not add sole-session inference, writer leases, optional
live-runtime handles, or process-keyed capability maps. See
`docs/internal/multi-session-app-server/README.md` and `protocol-scope.md`.

Handler rules:

- `AppServerHostHandler` implements both the remote and local request-handler
  traits — one exhaustive `ClientRequest` dispatcher
  (`host/app_server_host/{request_context,request_dispatch,request_handlers}`),
  never a second dispatch table.
- Every routed event carries an explicit session id; no active-session
  fallback. Local handler-emitted `CoreEvent`s return through
  `AppServer::route_envelope` via the bridge's outbound forwarder.
- TUI: the bridge creates one local AppServer connection. Its command handles,
  event pump, turn waiters, and sidechat observers are bounded broadcast
  cursors over that connection; they neither attach transport-level observer
  connections nor compete for one destructive event queue.
- SDK transport/correlation/outbound writing stay in `coco-sdk-server`; slot
  lifecycle/replace/close-owner semantics are owned by `coco-app-server`.

## Startup Flow

1. `coco-cli` maps parsed arguments into `AgentHostOptions`.
2. All paths fold config, build the model runtime registry, and register tools
   and commands.
3. SDK mode → `coco-sdk-server` over the AppServer JSON-RPC bridge (NDJSON
   stdio, plus opt-in sidecar listeners from `RuntimeConfig.server`).
4. Print mode → local AppServer bridge + local `turn/start`; `coco-cli`'s
   headless formatter renders output.
5. TUI → the same local bridge; terminal lifecycle and presentation policy stay
   in `coco-cli`.

Headless embedding/test callers use `run_chat_with_options` with an explicit
`RunChatOptions::cwd` unless `AgentHostOptions::cwd` is set; only the binary
reads process cwd. SDK mode installs the same startup cwd into
`AppServerHostState` so pre-session requests never see a relative process cwd.

## Runtime Loading & Replacement

- TUI, headless, and SDK all build runtimes through `SessionRuntimeFactory`
  and load them through an AppServer `spawn_load` owner task — never install a
  runtime via a bare `tokio::spawn` or a direct registry write. Target ids are
  reserved before construction; no throwaway startup identities.
- Start/resume without a configured runtime factory fail closed with the
  stable `runtime_factory_required` error
  (`app_server_host/runtime_replacement_gate.rs`).
- Replacement routes through `session_replace_operation.rs`: AppServer
  `spawn_replace` (fresh factory build) or `spawn_replace_to_live` (repoint
  onto an already-live orphan). The registry swaps only after construction and
  hydration succeed; construction failure leaves the prior slot untouched.
- Runtime-reload subscribers reattach to the session-owned publisher after
  every replacement; consumers resolve the swappable current-session owner
  instead of holding startup-only runtime handles.

## Turn Lifecycle Invariants

1. `SessionTurnExecutor` owns exactly one terminal `TurnEnded` on every exit
   path (including `prevent_continuation` and error/cancel); `turn.rs` never
   synthesizes a second. `forward_turn_events` clears the active-turn slot once
   on the first terminal, drops duplicates with a warn, and synthesizes a
   failed terminal if the runner channel closes with none (waiters never hang).
2. `SessionTurnCoordinator` has a `closed` tombstone: the close cascade
   (`SessionHandle::close_turn_coordinator` in `close_runtime`) rejects turn
   admission after close and cancels a turn admitted in the drain→close race.
3. In-session shortcuts (`/cost`, `/summary`, …) reserve the slot atomically via
   `reserve_shortcut_turn` (`TurnLifecycleState::Reserved` + RAII guard), not a
   check-then-act probe, so a shortcut and a real `turn/start` cannot both be
   admitted.
4. The active turn id is read via `SessionHandle::active_turn_id`;
   server-request bridges tag pending requests with it, and terminal
   forwarding calls `AppServer::cancel_turn_server_requests` so an interrupted
   turn's pending requests are reclaimed.

`turn/start` lifecycle events use the `TurnId` returned by the synchronous
`TurnStartResult`; `AppServerLocalBridge::start_turn_and_wait_for_end` depends
on that correlation. `TurnStartParams` carries optional paste images, slash
metadata attachment text, explicit model selection, and thinking overrides.

## Close Cascade & Shutdown

AppServer close performs the bridge-owned cascade before removing attachments:
cancel the state-owned active turn, boundedly drain the turn runner and
forwarder, clear the slot, then ask the matching
`SessionHandle::close_runtime` to fire SessionEnd hooks, tombstone the turn
coordinator, and cancel the runtime shutdown signal. The cascade closes only
the slot it owns, so it never tears down a replacement's new live runtime —
slot/owner semantics live in `coco-app-server`. The local bridge's
`shutdown_registered_sessions` drains all registered slots through this path
(TUI on exit, headless before hub flush, SDK after sidecar shutdown). Drain
waits observe OS interrupts (`drain_with_timeout_or_signal`); drain/flush
failures become a nonzero process result. The optional idle-session supervisor
is event-driven: attached connections, active turns, and non-empty cross-turn
queues are never idle.

## Server Config (`RuntimeConfig.server`)

| Key (`server.*`) | Env (`COCO_SERVER_*`) | Default | Effect |
|------------------|----------------------|---------|--------|
| `unix_socket_path` | `UNIX_SOCKET_PATH` | off | SDK NDJSON Unix-socket sidecar; bind failure = startup failure |
| `websocket_bind` | `WEBSOCKET_BIND` | off | SDK WebSocket sidecar |
| `named_pipe_name` | `NAMED_PIPE` | off | Windows NDJSON named-pipe sidecar |
| `max_sessions` | `MAX_SESSIONS` | 32 | SDK process session-slot limit (local TUI/headless bridge allows primary + sidechat) |
| `max_attached_sessions_per_connection` | `MAX_ATTACHED_SESSIONS_PER_CONNECTION` | 8 | Live-attachment cap per connection |
| `max_connections_per_session` | `MAX_CONNECTIONS_PER_SESSION` | 16 | Attached-connection cap per session (resource policy only; never elects an owner) |
| `server_request_timeout_secs` | `REQUEST_TIMEOUT_SECS` | 900 | Pending server→client request expiry (approvals, user input, elicitation, hook/MCP callbacks) |
| `event_retention_per_session` | `EVENT_RETENTION_PER_SESSION` | 1024 | Event retention ring size |
| `outbound_queue_frames` | `OUTBOUND_QUEUE_FRAMES` | 1024 | Outbound queue size |
| `turn_drain_timeout_secs` | `TURN_DRAIN_TIMEOUT_SECS` | 10 | Close-cascade active-turn drain bound |
| `shutdown_timeout_secs` | `SHUTDOWN_TIMEOUT_SECS` | 30 | Sidecar joins + process AppServer shutdown bound |
| `project_services_idle_ttl_secs` | `PROJECT_SERVICES_IDLE_TTL_SECS` | 3600 | `ProjectRegistry` idle sweep TTL |
| `idle_session_timeout_secs` | `IDLE_SESSION_TIMEOUT_SECS` | off | Idle-session auto-close supervisor |

## TUI Slash-Command Routing

TUI slash commands and runtime controls route through `LocalServerClient`
methods (the local AppServer seam shared with SDK handlers); the TUI keeps only
parsing, confirmation/toast rendering, and session event policy. Representative
mappings:

| Command / control | `LocalServerClient` method |
|-------------------|---------------------------|
| `/reload-plugins`, `/hooks reload` | `plugin_reload`, `hook_reload` |
| `/context` | `context_usage` |
| `/cost`, `/status` | `session_cost`, `session_status` |
| `/compact`, `/dream`, `/summary` | `turn_start` (handler-intercepted sentinel; same for SDK) |
| `/btw` | local bridge child-session open/close; unavailable remotely |
| `/model` picker | `set_model_role` |
| Ctrl+T thinking / fast-mode toggle | `set_thinking` / `config_apply_flags` |
| permission mode / `/permissions`, `/add-dir` | `set_permission_mode` / `apply_permission_update` (`reset` → `reset_session_permission_rules`) |
| `/color` | `set_agent_color` |
| `/tasks cancel` / Ctrl+B / list / detail | `stop_task` / `background_all_tasks` / `task_list` / `task_detail` |
| `/rewind` (file-restore half) | `rewind_files` (conversation truncation stays TUI-local) |
| `/resume <id>`, `/branch` | `session_resume` (TUI keeps target resolution + UI hydration) |
| `/clear` | factory build + AppServer replacement commit with `Clear` close reason |
| `/rename`, `/tag` | `session_rename`, `session_toggle_tag` |

SDK slash sentinels (`/cost`, `/status`, `/dream`, `/summary`, `/compact`,
`/goal`, `/rename`) are intercepted in `turn/start` and reuse the same handlers
before a normal runner task is spawned. Raw remote `/btw` input is rejected.

## Session Module Ownership (`src/session/`)

Host modules own the operation; remote/local adapters keep parsing, rendering,
and session event policy.

| Module | Owns |
|--------|------|
| `session_controls` | Model/thinking/fast-mode/permission controls, task ops, status/cost, context usage, file-history diff/rewind |
| `session_labels` | Rename/tag mutations + rename-name resolution |
| `session_mcp` | MCP status, dynamic registration, reconnect, toggle, hook wrapping |
| `session_memory` | Dream/summary memory refresh operations |
| `session_dialogs` | Agents/permissions/workflow/skills dialog payload assembly |
| `session_agents` | Agent-file create staging + template generation |
| `session_queue` | Human command-queue entry construction and enqueue |
| `goal_command` | Goal command resolution, status transcript append, active-goal metadata persistence — enter here; no `SessionHandle` goal forwarders |
| `session_compaction` | Manual compact turn execution |
| `session_messages` | Meta/slash metadata message construction, slash history append |

`SessionRuntime` is composed of `Session*Resources` owner structs (execution,
hooks, persistence, config, turn, lifecycle, workspace, engine-state,
integration, …); reach fields via `SessionRuntime` accessors, never through the
owner structs directly.

## Runtime Paths & Project Services

- Session workspace resolution (`SessionWorkspace`, `resolve_project_root`,
  `project_paths`, `runtime_paths`, `settings_roots_for_cwd`, `git_root_for`)
  is owned by `coco-app-runtime` and re-exported by `crate::paths`. Derive
  transcript/task-output/memory `ProjectPaths` via
  `crate::paths::project_paths(cwd)`.
- `crate::paths` itself owns output-style dirs (user → project walk → managed)
  and standard agent search paths (with linked-worktree canonical fallback).
- Project-scoped discovery goes through `coco_app_runtime::ProjectServices`
  (`build_command_registry`, `build_skill_manager`, `register_plugin_hooks`,
  `mcp_servers`, `lsp_servers`, `agent_search_paths`); session bootstrap and
  reload ask these instead of re-discovering.
- `ProjectRegistry` caches `Arc<ProjectServices>` per `(config_home,
  project_root)`; `ProcessRuntime::global()` owns the background idle sweep
  (evicts only entries with no external strong refs). CLI exit and SDK
  remote-host shutdown call `ProcessRuntime::shutdown_background_tasks()`.
- `config_home` remains the root for user/global artifacts (logs, plugins,
  settings, output styles, models, task lists, file-history metadata).

## Stop Hooks (agent-host half)

Forked background work (promptSuggestion, extractMemories, autoDream) dispatches
via `coco_query::forked_agent::ForkDispatcher`, whose production impl this
crate installs at session bootstrap (`integrations/fork_dispatcher.rs`,
`fork_dispatcher::install`). The dispatcher threads the parent's
`CacheSafeParams` so the fork request prefix matches byte-for-byte.
Dispatch ordering/gating lives in `coco-query`; per-fork `canUseTool` policies
live in `coco-memory`.
