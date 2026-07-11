# coco-agent-host

Agent-session host shared by CLI surfaces. It owns session-runtime construction,
the in-process AppServer client facade, SDK/AppServer request handling,
headless use cases, and runtime integrations.
It does not own the `coco` process entrypoint or the TUI command loop; those
remain in `coco-cli` as surface composition.

This is a Tier-1 application-composition crate under the workspace error
policy: startup/use-case assembly may use `anyhow`, while domain crates below
it expose typed errors and SDK handlers translate failures to protocol results.

## Key Types

| Type | Purpose |
|------|---------|
| `AgentHostOptions` | Clap-independent application inputs mapped once by `coco-cli`. |
| `local_client::LocalServerClient` | In-process typed client over `LocalClientAdapter`; owns local interactive/passive surface handles and receiver demultiplexing without leaking server implementation into the remote client crate. |
| `sdk_server::SdkServer` | NDJSON control server used by the CLI SDK surface. |
| `sdk_server::SdkServer::run_app_server_connection` | SDK-server entrypoint for the AppServer bridge; reuses the server's transport, state, and external notification sources while delegating JSON-RPC ownership to `coco-app-server` |
| `sdk_server::app_server_bridge::AppServerSdkHandler` | Runtime-backed AppServer request handler shared by JSON-RPC SDK and local in-process adapters |
| `sdk_server::AppServerLocalBridge` | Local AppServer client bridge for TUI/headless cut-over: wires AppServer, LocalClientAdapter, LocalServerClient, shared handler, and event forwarding |
| `sdk_server::StdioTransport` | stdin/stdout NDJSON transport |
| `sdk_server::QueryEngineRunner` | Bridges `QueryEngine` to SDK control messages |
| `sdk_server::CliInitializeBootstrap` | Session bootstrap from `initialize` control request |
| `sdk::ModelUsage` + schemas | SDK wire types |
| `model_factory::*` | Builds `Arc<dyn LanguageModelV4>` from provider/model config |
| `output::*` | Non-interactive output formatters (text/json/stream-json) |
| `headless` / `headless_support` | Print-mode orchestration plus goal/slash, transcript, tool-filter, and additional-directory helpers |
| `project_services::ProjectServices` | Project-rooted plugin catalog plus command, skill, hook, MCP, and LSP discovery shared by sessions with the same project root |
| `session_runtime::SessionRuntimeFactory` | Owned construction seam for building `SessionHandle`s from cloneable startup inputs and a target session id. |

## Startup Flow

1. `coco-cli` maps parsed arguments into `AgentHostOptions`.
2. Interactive/print/SDK paths fold config, build `ModelRuntimeRegistry`, and register tools + commands.
3. SDK → `sdk_server` over the AppServer JSON-RPC bridge (NDJSON over stdio,
   `initialize`/`interrupt`/`can_use_tool`/`set_permission_mode`/...)
4. Print mode → local AppServer control bridge +
   local `turn/start` + `output::*` formatter
5. The CLI TUI surface uses the same local AppServer bridge while retaining
   terminal lifecycle and presentation policy in `coco-cli`.

`sdk_server::spawn_sdk_outbound_writer` is the single writer for SDK
notifications, replies, and server requests. Session events carry a mandatory
`SessionId`, are stamped once, and route through the shared AppServer before
the stdio connection renders its NDJSON notification view. This feeds the
retention ring and sidecar subscribers while preserving per-session stream
accumulation; rendered stdio params include `session_id`, `surface_id`,
`turn_id`, `agent_id`, and `session_seq`. The only non-session event,
`plugins/changed`, uses a dedicated unstamped process-event variant.
`SdkTransport` has frame-level
`recv_frame` / `send_frame` methods for AppServer traffic; stdio decodes and
encodes `coco-app-server-transport::JsonRpcFrame` directly.
`SdkServerState::send_server_request` enqueues outbound server requests as
frames and resolves matching client `Success`/`Error` reply frames back to the
SDK hook/MCP callers through
`SdkServerState::resolve_server_request_frame` before falling back to AppServer
adapter response handling.
The installed SDK `TurnRunner` sits behind `TurnRunnerState`; builder setup,
runtime-bridge replacement, turn dispatch, and tests use `SdkServerState`
install/snapshot methods instead of raw runner locks.
The installed SDK `SessionHandle` sits behind `SessionRuntimeState`; SDK
startup, AppServer replacement, runtime controls, approval/MCP bridges, and
tests use `SdkServerState` install/snapshot methods instead of raw runtime
locks.
The pending server-request waiter map and issued-id counter sit behind the
SDK handler `ServerRequestState`; callers continue through `SdkServerState`
methods so request cleanup and frame matching stay centralized.
The SDK transport handle and ordered outbound writer queue sit behind
`ConnectionState`; approval, hook, MCP, and bridge code use
`SdkServerState` accessors instead of raw transport/outbound slots.
The SDK handler request context, result type, and exhaustive
`ClientRequest` dispatcher live in `sdk_server::handlers::dispatch`; topical
handler modules continue to import the re-exported `HandlerContext` /
`HandlerResult` from `sdk_server::handlers`.
The optional SDK `McpConnectionManager` sits behind `McpManagerState`; startup,
bridge bootstrap, SDK-hosted MCP registration, and MCP handlers use
`SdkServerState` install/snapshot methods instead of reading the raw slot.
The SDK production runtime replacement context sits behind
`RuntimeReplacementState`; SDK startup installs it and AppServer start/resume
interception reads it through `SdkServerState` methods.
The SDK runtime reload subscriber sits behind `RuntimeReloadState`; runtime
install aborts and replaces the sandbox reload task through `SdkServerState`
methods instead of a raw task slot.
MCP tool-registration reports sit behind `McpRegistrationState`; `mcp/status`
requests ask `SdkServerState` for the status projection instead of reading the
report map directly.
SDK file-history state plus config home sit behind `FileHistoryStateSlot`, so
rewind handlers and runtime install paths use `SdkServerState` snapshots and
install methods instead of raw slots.
Pre-runtime initialize bootstrap data, startup cwd, the SDK agent-progress
opt-in flag, and startup-authorized bypass capability sit behind
`BootstrapState`.
The SDK handler, dispatcher, and approval-bridge tests run through
`SdkServer::run_app_server_connection`; the legacy `SdkServer::run` loop has
been removed, so SDK JSON-RPC ownership lives on the AppServer bridge path.
`AppServerSdkHandler` also implements the local in-process AppServer request
handler trait, so TUI/headless cut-over code can reuse the same exhaustive
`ClientRequest` dispatcher without adding another runtime dispatch table.
Local AppServer cut-over code must pair that handler with
`spawn_app_server_local_outbound_forwarder`, which routes handler-emitted
`CoreEvent`s back through `AppServer::route_envelope`. Every event message has
an explicit routed session id; there is no active-session fallback. Both local
and SDK stamping seams derive `agent_id` from the protocol payload when present.
`AppServerLocalBridge` is the preferred local entrypoint: it owns the local
`AppServer`, `LocalServerClient`, shared handler, and outbound forwarder so
TUI/headless code does not duplicate adapter wiring.
Session lifecycle interception, runtime-backed start/resume construction,
replace/close cascades, and scoped-state commit ordering live in
`sdk_server/session_lifecycle.rs`; idle deadline supervision lives in
`sdk_server/idle_session_supervisor.rs`. `app_server_bridge.rs` is the adapter
composition/transport owner and must not absorb those policies again.
Startup runtime construction now passes `SessionRuntimeFactory` into the
local AppServer bridge load helpers (or the equivalent SDK stdio bridge helper),
so `SessionRuntimeFactory` to `LocalAppSessionHandle` conversion happens inside
the bridge-owned `spawn_load` path instead of in TUI/headless/main startup
callers.
Its AppServer registry stores `LocalAppSessionHandle` snapshots rather than
empty `()` handles. Installed runtime snapshots carry the current application-host
`SessionHandle`, whose session id is an immutable snapshot. Close cascade logic
checks the registry snapshot before touching runtime-backed state, so stale
registry handles from a replacement swap do not tear down the new live runtime.
Local `session/resume` uses `AppServer::spawn_replace`
when the previous live session has an interactive local surface, and uses
`AppServer::spawn_replace_detached` plus a fresh requester surface when no
replace caller surface exists. Re-installing a runtime-backed handle for an
already-live local session refreshes the registry handle without changing
surface routing.
AppServer close for local/SDK bridge handles now performs the bridge-owned
cascade before archiving surfaces: if the closing session still matches the
SDK active-session state, it cancels the state-owned active turn, waits
boundedly for the turn runner and forwarder to drain, clears the slot, then
asks the matching `SessionHandle` to fire runtime SessionEnd hooks and cancel
the runtime shutdown signal. The registry snapshot guard skips fused-runtime
shutdown when the runtime handle no longer matches the registry snapshot.
The optional idle-session supervisor is event driven: AppServer activity, SDK
turn state, and `CommandQueue` revisions wake it, and it sleeps to the earliest
per-session deadline. Attached surfaces, active turns, and non-empty cross-turn
queues are never idle; queue enqueue/dequeue timestamps reset the deadline.
TUI orchestration remains in `coco-cli` and is split by UI ownership rather
than hidden behind a generic runner. The host exposes application use cases;
the CLI driver retains terminal lifecycle and command-loop policy.
SDK JSON-RPC mode also uses `LocalAppSessionHandle` in the AppServer registry.
`session/start`, `session/resume`, and `session/archive` dispatched through
`SdkServer::run_app_server_connection` apply the same lifecycle registration
and close path as local requests after the existing SDK handler succeeds.
Successful JSON-RPC start/resume also attaches an interactive surface to the
request connection and returns that `SurfaceId` on the result DTO; remote
clients may still fall back to lifecycle activation when reading older streams.
When `RuntimeConfig.server.unix_socket_path` is set (settings
`server.unix_socket_path` or `COCO_SERVER_UNIX_SOCKET_PATH`), SDK mode also
binds an NDJSON Unix-domain socket sidecar on the same `AppServer` and shared
handler before entering stdio dispatch. Bind failures are startup failures; the
listener is stopped with a bounded shutdown when stdio dispatch exits.
When `RuntimeConfig.server.websocket_bind` is set (settings
`server.websocket_bind` or `COCO_SERVER_WEBSOCKET_BIND`), SDK mode also binds
an opt-in WebSocket sidecar on the same bridge. No TCP/WebSocket listener is
opened by default.
On Windows, when `RuntimeConfig.server.named_pipe_name` is set (settings
`server.named_pipe_name` or `COCO_SERVER_NAMED_PIPE`), SDK mode also binds an
opt-in NDJSON named-pipe sidecar on the same bridge. No named pipe is opened by
default.
JSON-RPC `session/subscribe` is handled directly by the AppServer bridge: it
attaches a passive surface to the request connection, returns the passive
`SurfaceId` plus replayed envelopes, and rejects missing/stale cursors with
the snapshot-required marker.
Local TUI/headless AppServer bridges and SDK stdio AppServer startup now size
event retention and outbound queues from `RuntimeConfig.server`
(`server.event_retention_per_session` and `server.outbound_queue_frames`,
with matching `COCO_SERVER_*` env overrides). SDK stdio AppServer startup also
uses `server.max_sessions` / `COCO_SERVER_MAX_SESSIONS` for the process-level
multi-session slot limit; the TUI/headless local bridge remains capped at one
active session for v1. SDK stdio and config-aware local bridge startup pass
`server.max_surfaces_per_connection` /
`COCO_SERVER_MAX_SURFACES_PER_CONNECTION` and
`server.max_passive_surfaces_per_session` /
`COCO_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION` into AppServer routing limits.
AppServer-routed `session/list`, `session/read`, and `session/turns/list` layer
live AppServer state over the persisted `SessionManager` response, so a started
session is visible before its transcript has been written. Persisted data
remains canonical when available. `coco-app-server` owns the request
composition over `AppSessionDataSource` / `AppSessionDataHandle`, while
`sdk_server::session_data` supplies the concrete `SessionManager` callbacks and
live-handle snapshots. The persisted list/read/turn loaders in
`sdk_server::session_data` are shared by that AppServer local view and the
remaining legacy SDK handlers, so the live-overlay path has one owner while
the JSONL `SessionManager` boundary remains in `coco-agent-host`.
The local AppServer bridge exposes `shutdown_registered_sessions`, which
drains all registered AppServer slots through the same concrete close cascade
used by `session/archive`: scoped SDK active-turn state is cancelled and
boundedly drained using `RuntimeConfig.server.turn_drain_timeout_secs`
(`server.turn_drain_timeout_secs`, env
`COCO_SERVER_TURN_DRAIN_TIMEOUT_SECS`; default 10) before AppServer close
completions resolve. The TUI driver invokes that drain on exit before its
final best-effort metadata append, headless invokes it before hub flush, and
SDK stdio mode invokes it after sidecar listener shutdown before memory/hub
flush. SDK sidecar listener joins and whole-process AppServer shutdown waits
are bounded by
`RuntimeConfig.server.shutdown_timeout_secs` (`server.shutdown_timeout_secs`,
env `COCO_SERVER_SHUTDOWN_TIMEOUT_SECS`; default 30). Headless, TUI, and SDK
stdio convert local AppServer shutdown drain and Event Hub connector flush
failures or timeouts into a nonzero process result after their ordinary cleanup
has run. Both bounded waits also observe OS interrupt signals and return a
non-clean shutdown result instead of continuing to wait for the timeout.
`SdkServerState` keeps persisted-session storage behind install/snapshot
methods backed by `sdk_server::session_store`, so the remaining `coco-session`
boundary is localized outside the broad handler state.
`session/turns/list` derives turn spans from transcript message order until
persisted transcript entries carry durable turn ids.
When constructed with a `HubConnectorSender`, the local outbound forwarder
clones each AppServer-stamped `SessionEnvelope` to the Hub connector queue
after routing it locally. Startup-owned Hub worker construction/configuration
is now wired for TUI/headless through `RuntimeConfig.event_hub.url`
(`event_hub_url`, `COCO_EVENT_HUB_URL`, or `--event-hub-url`), and both paths
flush the worker during normal shutdown. `--serve-hub` / `--hub-port` are
accepted by all builds; the default build emits the documented missing-feature
diagnostic, while the `serve-hub` Cargo feature starts an embedded
SQLite-backed `coco-hub-server` and fills `event_hub_url` with its local
WebSocket endpoint. SDK/NDJSON egress uses the same stamp-and-route contract
inside the SDK writer: the stamped envelope goes to AppServer and the Hub
connector before the stdio surface renders its NDJSON view.
For the TUI, `start_passive_event_pump` attaches a separate passive local
surface and continuously forwards bridge-routed `CoreEvent`s into the TUI
event channel. Keep the interactive surface for server-request ownership; use
the passive pump for ordinary event delivery.
Use `install_session_runtime` when TUI/headless have already built a
`SessionRuntime`; it snapshots the existing session id/cwd/model into the
shared handler state instead of issuing a fresh `session/start`, installs the
runtime's `SessionManager` so local `session/list`, `session/read`, and
`session/turns/list` see persisted transcripts, and installs a `QueryEngineRunner` so local
`turn/start` requests have the same engine runner as the SDK bridge.
TUI, headless, and SDK bootstraps now construct their initial runtime through
`SessionRuntimeFactory`; the factory owns the cloneable build inputs and can
build explicit-id handles. TUI/headless startup reserve the fresh/resume target
id before runtime construction and SDK startup reserves a fresh startup id;
all three load that initial runtime through an AppServer `spawn_load` owner
task. Resume/fork therefore no longer creates a throwaway startup identity
first. Production SDK `session/start` now builds the client-started runtime
through the same factory inside the AppServer load/replace owner task, closes
the explicitly identified startup placeholder slot (including a slot observed
only by passive surfaces), then swaps `SdkServerState.session_runtime` and
installs scoped SDK state maps for the constructed handle without writing the
process-global active identity; the legacy handler now rejects `session/start` when a
runtime is already installed without the AppServer replacement context.
The shared `session/resume` handler only hydrates an installed runtime that is
already on the requested id; different-id runtime-backed resume must use the
AppServer replacement path.
The local bridge has runtime-backed `spawn_replace` /
`spawn_replace_detached` helpers that return the constructed runtime handle to
callers. The TUI driver now has a swappable current-session owner: each command
loop iteration reads the current `SessionHandle`, and `/resume` / `/branch`
construct a fresh runtime through `SessionRuntimeFactory`, seed the loaded
transcript state, commit the AppServer replacement, and install the returned
handle into both the TUI owner and local bridge. SDK `session/resume` uses the
same factory-backed replacement ordering in production: the AppServer bridge
loads the persisted session, builds the target runtime inside the AppServer
load/replace owner task, replays resume hydration plus SDK late binds, commits
the AppServer slot/surface switch, and only then swaps
`SdkServerState.session_runtime` and installs scoped SDK state maps without
writing process-global active identity. The state-owned SDK turn handoff carries the
resumed transcript history and runtime app state so the next SDK `turn/start`
continues from the loaded chain; construction failure leaves the prior
SDK/AppServer live slot untouched. Runtime-backed SDK control
paths now update/read model, permission mode, thinking level, and cwd from the
installed runtime first; turn id counters, aggregate archive accounting, and
active-turn handles/cancellation sit behind `TurnState`. Legacy cwd/model
metadata, session-scoped plan-mode instruction snapshots, SDK turn handoff
history, and live app state sit behind `ScopedSessionState`; callers still
enter through `SdkServerState` methods. The SDK singleton active identity is
deleted. Approval, user-input, and elicitation waiter maps sit behind
`PendingClientRequestState`, with turn resolve/cancel handlers entering
through `SdkServerState` methods instead of touching maps directly.
Direct legacy start/resume install the same scoped SDK state maps; unscoped handlers
resolve a sole scoped session when no AppServer surface or installed runtime
identifies the session. AppServer-routed requests also
carry an optional connection-scoped session id from the sole attached
interactive surface; runtime controls, rewind, normal and shortcut turn setup,
and other simple readers prefer that scope, then the installed runtime's scoped
state, before falling back to a sole scoped state.
Runtime-backed SDK session/start and session/resume, scoped archive, and
AppServer close cleanup operate by routed session id instead of requiring the
SDK active identity. The REPL bridge control handler also falls back to the
installed runtime's current session id for bridge-origin
controls. Per-turn `SessionResult` accounting for scoped turns folds while the
routed session's scoped state is still live, so it also no longer requires the
process-global active identity. SDK/AppServer fallback event stamping, unscoped runtime-backed
turn cleanup, and live session-data overlay now prefer runtime/scoped state;
process-level bridge fallbacks share
`SdkServerState::runtime_or_active_session_id()`. AppServer runtime replacement
and no-runtime-replacement `session/start` / `session/resume` install scoped
SDK state and rely on AppServer registry/surface ownership instead of claiming
a process-global identity. TUI, headless, and SDK runtime construction uses
`SessionRuntimeFactory` with a `SessionRuntimeBootstrapSource`. Production
factories use the per-session fold source: each target cwd rebuilds
`RuntimeConfig` plus the derived model, system prompt, startup permission state,
command registry, skill manager, project services, and agent search paths as
one bundle, then stores the session-owned `RuntimeReloader` on the constructed
runtime. TUI config-change hooks, sandbox reload, sandbox violation forwarding,
sandbox approval bridging, model-runtime reload, TUI settings reload, and TUI
skill-override writes use a runtime-reload subscription owner that reattaches
to the session-owned publisher after startup, `/resume`, `/branch`, or
`/clear` replacement. SDK sandbox reload and SDK sandbox approval bridging are
installed through the shared SDK runtime-state installer, so AppServer-backed
SDK `session/start` / `session/resume` replacement aborts the old runtime's
reload subscriber and attaches the new runtime's session-owned publisher.
Compatibility tests may still use
`SessionRuntimeBootstrapSource::startup_snapshot(...)`. Factory builds can
also receive an explicit target cwd, and TUI startup resume, TUI `/resume` /
`/branch` / `/clear`, and SDK runtime replacement start/resume use that cwd for
runtime construction and late binds. SDK `initialize` now prefers the installed
runtime for command, agent, and output-style metadata, falling back to the
bootstrap snapshot only before a runtime exists; live fast-mode state is now
read from the installed runtime's engine config, while account/auth remains
bootstrap-owned until those sources grow runtime accessors. SDK-supplied
agents, initialize hook callbacks, and plan-mode instructions sit behind
`InitializeState` and are replayed into session start/resume replacement paths
through `SdkServerState` methods. SDK MCP manager
construction now happens after the startup runtime is loaded and uses that
runtime's MCP config; TUI/headless MCP bootstrap already builds or reuses
managers from the session runtime. TUI, headless, and SDK event-hub connectors
now spawn after their startup runtime loads and use that runtime's event-hub
config plus session id. `SessionExecutionResources` now owns the shared tool
registry plus model runtime registry instead of leaving them as flat
`SessionRuntime` fields. `SessionHookResources` now owns the hook registry,
hook LLM handle, hook event buffers, and FileChanged watcher instead of leaving
hook orchestration handles as flat `SessionRuntime` fields.
`SessionPersistenceResources` now owns the session manager, project storage
paths, main transcript store, and persistence flag instead of leaving session
storage as flat `SessionRuntime` fields.
`SessionProjectResources` now owns the process runtime plus project-services
snapshot instead of leaving project/process services as flat `SessionRuntime`
fields.
`SessionConfigResources` now owns the config home, per-session folded runtime
config, and runtime reloader instead of leaving config/reload handles as flat
`SessionRuntime` fields.
`SessionCatalogResources` now owns the slash-command registry plus session
skill manager instead of leaving command and skill catalogs as flat
`SessionRuntime` fields.
`SessionTurnResources` now owns the schedule store, side-query handle, usage
accounting, mailbox, and optional permission bridge instead of leaving per-turn
engine plumbing as flat `SessionRuntime` fields.
`SessionLifecycleResources` now owns the session shutdown token plus
PID-registry guard, and `SessionCommandResources` now owns the cross-turn
command queue plus attachment channel, and `SessionTitleResources` now owns
auto-title enablement plus the fast-model spec, and
`SessionWorkspaceResources` now owns original cwd, project root, and live cwd
and `SessionEngineConfigResources` now owns per-session engine config, the
synchronous orchestration mirror, and model-role overrides, and
`SessionEngineStateResources` now owns shared mutable engine state, file
history/read state, app state, loop sentinel state, pending peer messages,
auto-mode/denial state, transcript dedup, clear rewind snapshots, terminal-goal
metadata flag, and tool-result replacement state, and
`SessionIntegrationResources` now owns late-bound MCP/LSP handles, the live
MCP manager slot, and the MCP reconnect key instead of leaving
lifecycle/producer/title/workspace/config/engine-state/integration plumbing as
flat `SessionRuntime` fields. Engine wiring and reload paths read or mutate
those slots through `SessionRuntime` accessors rather than reaching into the
owner directly.
`SessionRuntime` is now the resource-owner struct itself instead of a wrapper
around a separate `SessionRuntimeResources` field.
`control/updateEnv` now applies to the installed runtime's session-owned shell
env store, which Bash/PowerShell providers snapshot before future shell spawns;
the no-runtime SDK fallback still acknowledges the request without storing an
unused singleton map. SDK `context/usage` reads the installed runtime's main
context directly, so it no longer depends on SDK handoff state when a runtime is
present. The TUI skill watcher now uses the swappable current-session owner when
handling debounced file changes, so skill ConfigChange hooks, catalog reload,
and slash-command refresh target
the post-resume / post-branch runtime. The TUI cron tick driver also reads the
swappable owner on each tick, so scheduled prompts enqueue into the current
runtime's command queue after startup resume, `/resume`, `/branch`, or
`/clear`. The TUI ConfigChange watcher and permission notification bridge now
resolve the same swappable owner before firing hooks, updating permission prompt
state, or generating permission-risk explanations, so these paths no longer
hold startup-only runtime handles. Post-login OpenAI model refresh, leader
inbox polling, hook-agent scoped registry construction, resume UI hydration,
file-history diff, and prompt-mode bash response helpers also keep
`SessionHandle` at their boundary instead of accepting raw `SessionRuntime`
references. The cron tick, skill watcher, late-bind installer, and unified MCP
bootstrap also use the handle internally instead of cloning the raw runtime for
ordinary runtime access. SDK turn execution and headless print-mode setup also
use `SessionHandle` directly for runtime services instead of cloning the raw
runtime from the handle. The TUI runner now also uses `SessionHandle` directly
for event-hub startup, reload subscriptions, command waits, resume/clear
hydration, goal state, rewind, plugin/agent/permission payloads, and
model/thinking updates, so it has no remaining `SessionHandle::runtime()` escape
calls. SDK startup MCP/event-hub setup, structured-output enablement,
setup/start hooks, and file-history handoff now also call through the startup
`SessionHandle`, and the current-session config-change watcher fires hooks
through the swappable handle directly. SDK turn/runtime/session handlers now use
installed `SessionHandle`s directly for memory shortcuts, goal state, manual
compact, model/permission/color updates, tag toggling, and resume hydration;
remaining production `.runtime()` calls are local AppServer registry snapshot
extraction points (`LocalAppSessionHandle`) or tests.
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
Both headless and TUI bootstraps now create this bridge before initial runtime
construction, load that runtime through the local `spawn_load` owner task,
install it into the shared handler state, and issue a local `keep_alive`
request through `LocalServerClient`. TUI normal submits and slash/palette prompt
turns now start via local AppServer `turn/start`; a passive completion monitor
releases the TUI `active_turn` slot while `turn/interrupt` handles
AppServer-owned cancellation.
Headless `RunChatOutcome` assembly also starts the turn through local AppServer
and reconstructs its structured result from the aggregated session result,
runtime history, and usage snapshot. TUI queued prebuilt-history turns now pass
a serialized full-history override through local AppServer `turn/start`, so
permission retries, queued prompts, and prompt-mode bash follow-up turns no
longer call the engine directly from the TUI runner.
Headless embedding/test callers must use `run_chat_with_options` with an
explicit `RunChatOptions::cwd` unless `AgentHostOptions::cwd` is set; only `main.rs` reads
process cwd at startup and passes that snapshot into headless execution. SDK
mode also installs the same startup cwd into `SdkServerState`, so pre-session
`session/start`, `config/read`, and `config/value/write` requests do not fall
back to a relative process cwd.
TUI `/reload-plugins` routes through this local AppServer client
(`LocalServerClient::plugin_reload`) while preserving the TUI toast and
command-palette refresh behavior. TUI `/hooks reload` uses the same local
client path (`LocalServerClient::hook_reload`) instead of directly mutating the
runtime from the TUI runner.
TUI `/context` also routes through the local client
(`LocalServerClient::context_usage`); the bridge refreshes its snapshot from the
live `SessionRuntime` immediately before dispatch so history-sensitive reports
see current state.
TUI `/cost` and `/status` route through local AppServer `session/cost` and
`session/status`, so live usage/status observability uses the same handler
boundary instead of direct TUI runtime reads.
TUI `/compact` routes the existing compact sentinel through local AppServer
`turn/start`, reusing the same handler-owned compaction shortcut as SDK mode
and keeping interrupt/completion ownership on the AppServer turn path.
TUI `/dream` and `/summary` do the same with their memory sentinels, so manual
memory shortcuts no longer call `MemoryRuntime` directly from the TUI runner.
TUI `/btw` also sends its sentinel through local AppServer `turn/start`, so the
side-question fork and degraded no-dispatcher response live at the handler
boundary instead of in the TUI runner.
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
`LocalServerClient::set_permission_mode`; the bridge attaches a local interactive
surface and drains forwarded `PermissionModeChanged` events back into the TUI
event channel after dispatch.
TUI fast-mode toggles route through `LocalServerClient::config_apply_flags` with
the `fast_mode` setting; the SDK handler mutates the installed runtime's
engine config and emits `FastModeChanged` from the AppServer path.
TUI Ctrl+T thinking-level changes route through `LocalServerClient::set_thinking`;
the SDK handler updates the installed runtime's engine config and emits
`ModelRoleChanged` from the AppServer path.
TUI `/model` picker role/provider/model overrides route through
`LocalServerClient::set_model_role`; the SDK handler applies the live
`SessionRuntime` role override and emits `ModelRoleChanged`, while the TUI
keeps only the picker confirmation/history message.
TUI `/permissions` editor, `/permissions allow|deny`, approval always-allow,
and `/add-dir` updates route through `LocalServerClient::apply_permission_update`;
the SDK handler applies the live permission base and persists writable
destinations, while the TUI refreshes the editor overlay from disk afterward
for editor edits. `/permissions reset` routes through
`LocalServerClient::reset_session_permission_rules`, clearing only session-scoped
live allow/deny rules.
TUI `/color` changes route through `LocalServerClient::set_agent_color`; the SDK
handler updates the installed runtime's live app-state color.
TUI teammate current-work interrupt now routes through
`LocalServerClient::agent_interrupt_current_work`, keeping that runtime-control
request on the same local AppServer path as the SDK handler.
TUI teammate/subagent cancellation routes through local AppServer
`LocalServerClient::stop_task`; the SDK handler uses the installed `TaskRuntime`
when present and only falls back to active-turn cancellation for legacy
SDK-only sessions with no installed `SessionRuntime`.
TUI Ctrl+B background-all foreground tasks routes through
`LocalServerClient::background_all_tasks`, which dispatches the AppServer
`control/backgroundAllTasks` request and returns the ids that transitioned.
The `/tasks cancel <id>` slash command uses the same local
`LocalServerClient::stop_task` path; `/tasks list` and `/tasks detail <id>` use the
same local AppServer seam through `LocalServerClient::task_list` and
`LocalServerClient::task_detail`.
TUI explicit `/rewind` keeps conversation truncation local to the TUI runner,
but its file-restore half routes through `LocalServerClient::rewind_files`; the
bridge installs the runtime's `FileHistoryState` and config home into
`SdkServerState` when it adopts a `SessionRuntime`.
In-session TUI `/resume <id>` and `/branch` route through local
`LocalServerClient::session_resume`; the TUI keeps only target resolution/fork
creation, coordinator-mode reconciliation, and UI reset/history hydration.
TUI `/clear` now builds a fresh empty runtime through `SessionRuntimeFactory`,
commits it through local AppServer replacement with a `Clear` close reason,
copies only the live permission base plus the hidden pre-clear rewind prefix,
and swaps the TUI current-session owner/local bridge to the returned handle.
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

`crate::paths::runtime_paths()` is the application-host boundary for project-scoped
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
