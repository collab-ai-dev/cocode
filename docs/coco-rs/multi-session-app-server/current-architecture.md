# Current Architecture

Audit baseline: landed post-remediation tree on 2026-07-14.

This document is descriptive; it is not the target design. The v2 remediation
recorded in `remediation-plan.md` is complete. Residual defects that survive in
this description are cross-referenced to the verified follow-up items in
[follow-up-todo.md](follow-up-todo.md) (T-numbers).

## Crate topology

The durable AppServer split is:

```text
coco-types
   ^
   +---------------- coco-app-server-transport
   |                          ^
   |                          |
coco-app-server       coco-app-server-client      coco-app-runtime
   ^                          ^                          ^
   |                          |                          |
   +-------- coco-agent-host--+--------------------------+
```

Useful properties:

- server and remote client share DTOs and transport primitives, not server
  implementation details;
- `coco-app-server` is generic over a cloneable live handle;
- query/core/service crates do not depend on AppServer crates;
- `coco-app-runtime` owns process/project/bootstrap contracts rather than query
  execution.

The former `coco-agent-host -> coco-tui` edge has been removed (Phase G).
The TUI-specific adapters (voice bootstrap, teammate action bridge) moved to
`coco-cli`, the `SystemPushKind`/permission-display formatters were re-homed to
`coco-messages`/`coco-types`, and a seam guard
(`scripts/check-agent-host-seam.sh`) now forbids agent-host from depending on
`coco-tui` or `coco-sdk-server`. Agent-host is protocol-neutral.

## Process startup today

`coco-cli::main` performs sandbox self-dispatch, clap parsing, startup cwd
capture, tracing installation, process-runtime lookup, argument validation,
embedded Hub startup, subcommand dispatch, and surface selection.

CLI mode selection is centralized in `coco-cli::execution_plan`. It constructs
a typed `ExecutionPlan` from parsed CLI arguments and injectable stdin/stdout
terminal capabilities; `main.rs` and `tracing_init.rs` consume that shared plan
for mode and tracing selection. Mode-dependent validation for
`--no-session-persistence` and `--plan-mode-instructions` is part of fallible
pure plan construction. For the default command path:

```text
--non-interactive                         -> headless
prompt present or stdout is not a terminal -> headless
stdin is not a terminal and no prompt      -> headless, stdin is raw prompt
otherwise                                  -> TUI
explicit `sdk` subcommand                  -> SDK
```

The unsupported global `--no-tui` and `--json` flags have been removed from the
clap schema; `ps --json` remains because it has concrete behavior. Placeholder
subcommands that only printed success/not-implemented messages have also been
removed. Confirmed CLI-only flags with no runner consumer are rejected by clap.
The retained top-level clap schema is guarded by an accepted-field consumption
audit test that requires every accepted flag to have an explicit consumer or
execution-plan policy.

## Surface composition today

### TUI

TUI startup is implemented in `app/cli/src/tui/bootstrap.rs`. It:

1. consumes teammate identity environment;
2. folds startup config and builds engine resources;
3. creates the session manager and TUI channels/permission bridge;
4. constructs `SessionRuntimeFactory`;
5. constructs a one-slot `AppServerLocalBridge`;
6. calls local `session/start` or `session/resume`, which builds/registers the
   runtime and attaches the interactive surface through AppServer;
7. creates TUI state and spawns the command driver;
8. routes the migrated control paths through typed local bridge calls;
9. drains AppServer and Event Hub through `ShutdownCoordinator` during exit.

The TUI surface directory contains application operations as well as
presentation policy. The TUI-specific adapters (voice bootstrap, teammate
inbox pump) live in `app/cli/src/tui`; agent-host retains only
protocol-neutral bridges. TUI and headless share the local bridge/factory/
Event-Hub assembly through `agent_host::local_host::build_local_host`, the
local counterpart to the remote `HostBuilder`; the two surfaces differ only in
the `LocalHostInputs` policy they pass in (T4 resolved).

### Headless

Headless execution is implemented in `coco-agent-host::headless`. It:

1. resolves some local slash commands before runtime construction;
2. folds config and constructs model/tool startup state;
3. constructs another `SessionRuntimeFactory` and one-slot local bridge;
4. calls local `session/start` or `session/resume` so AppServer owns runtime
   registration and history hydration;
5. still applies some live turn configuration directly through
   `SessionHandle` after `session/start` (structured output, live
   permissions, turn runtime config), diverging from the factory-fold
   timing used by TUI/SDK (T5);
6. starts a turn through local AppServer;
7. waits for `TurnEnded` through local AppServer and uses the completion's
   embedded per-turn session result directly;
8. drains the local server and Event Hub through `ShutdownCoordinator`.

The headless module is also the shared home of config/model/permission bootstrap
helpers used by other surfaces, despite its surface-specific name.

### SDK

SDK startup is split between `coco-agent-host::remote_host` and
`coco-sdk-server`:

1. remote host folds startup config/resources and creates bootstrap metadata;
2. it constructs shared AppServer state and listeners;
3. it provides a `RuntimeReplacementContext` through `HostInputs`,
   containing the `SessionRuntimeFactory`, process runtime, cwd, and
   structured-output policy;
4. it does not build or register a runtime during process startup;
5. SDK server opens stdio and optional sidecar connections;
6. the first client start/resume builds the first real runtime through the
   normal AppServer load path;
7. shutdown closes all registry sessions and flushes any installed Event Hub
   connector through `ShutdownCoordinator`.

The hidden placeholder session has been removed. Startup cwd, initialize
bootstrap, startup session manager, and bypass availability now enter
`AppServerHostState` through `HostInputs` rather than late startup
install calls. The production remote `TurnRunner` also enters through
`HostInputs`; local bridge/test overrides can still replace it through
the explicit test/local seam. A focused regression covers the old
`max_sessions = 1` hidden-slot failure mode by verifying that the first real
`session/start` succeeds after zero-session startup. SDK Event Hub egress now
has a process-owned connector seam and can announce an empty live-session set
at zero-session startup. Remote host preparation also starts a registry
membership watcher, and SDK stdio event routing refreshes membership before
forwarding session events to the Hub.

## AppServer state and request routing

`AppServerHostState` currently owns:

- a replaceable `TurnRunner` service;
- keyed session activity projections;
- one process session-manager slot;
- bootstrap initialize metadata/startup cwd/bypass state;
- an optional runtime-replacement context;
- the per-session sequence allocator.

Production remote startup supplies several values through `HostInputs`, but
`HostInputs` derives `Default` and all principal services remain optional.
`AppServerHostState::default`, a fail-closed runner, and public/local
`install_turn_runner`/`install_session_manager` seams still permit a partially
configured host. The old startup `try_write`/panic bootstrap path is gone; the
remaining defect is type-level optionality and replacement after construction,
not lock contention.

Every accepted remote connection gets an `AppServerHostHandler` clone with a
new connection-profile slot. Interactive request targeting is resolved through
AppServer validation and produces a `SessionRequestContext` containing matching
session id and runtime handle.

Request handling is split across several layers:

```text
request_scope in coco-types
  -> AppServer adapter routing
  -> AppServerHostHandler special lifecycle/data cases
  -> request_targeting
  -> request_dispatch exhaustive match
  -> topical request handler
  -> root session operation
  -> registry/surface lifecycle helper
```

The target selection is sound for tested operations, but lifecycle behavior is
distributed and duplicated across these layers.

## Live-session ownership

`LiveSessionRegistry<AppSessionHandle>` remains the process map from
`SessionId` to a live capability. Slots transition through Loading, Live, and
Closing. Load/replace/close/shutdown work runs in spawned owner tasks, so a
request waiter does not own progress.

`AppSessionHandle` stores an immutable registry id snapshot plus a
`SessionHandle`. Registry close compares the snapshot before closing the runtime
to avoid a stale handle closing a replacement runtime.

## Session runtime

`SessionRuntime` owns per-session resources grouped into roughly twenty
resource structs, including:

- tools, model runtimes, and engine config;
- history, transcript store, usage, and `ToolAppState`;
- cwd/project/config/reloader state;
- hooks, commands, skills, agents, permissions, sandbox, tasks, memory, MCP,
  LSP, and file history;
- command/attachment queues and shutdown state;
- `SessionTurnCoordinator`.

`SessionTurnCoordinator` owns turn ids, one active turn record, and aggregate
accounting using short `std::sync::Mutex` critical sections plus an atomic turn
counter. A whole-runtime actor is not used.

`SessionHandle` keeps `Arc<SessionRuntime>` private and exposes a forwarding
API split across responsibility submodules (`session_handle/{capabilities,
history,controls,engine,late_bind,tasks,mcp,hooks}`). No public method returns a
raw `Mutex`/`RwLock`: the lock accessors are `pub(crate)` or replaced by narrow
snapshot ops, `live_permission_rules()` returns the append-only
`LivePermissionRulesHandle` capability (T1 resolved), and the dead
`mcp_manager()` raw-lock accessor was removed. Session identity lives only on the immutable handle /
`SessionEngineConfigResources` — it was removed from the mutable
`QueryEngineConfig` entirely, so a config edit cannot rotate it. Callback
requirements are a mandatory construction input (no `OnceLock` late install).

## Start, resume, and replace

Remote start/resume use the AppServer lifecycle owner tasks and construct the
runtime from the calling connection profile.

TUI and production headless startup now enter through the local AppServer
lifecycle facade: fresh startup uses `session/start`, while startup resume uses
`session/resume`. The lifecycle-owned runtime construction receives
surface-specific integration policy through `RuntimeReplacementContext`.
In-session TUI `/resume` and `/branch` now replace the live interactive surface
through the local typed `session/replace` resume facade instead of directly
registering or hydrating the destination runtime from the TUI layer. `/clear`
now uses a typed `session/replace` clear destination; the handler owns snapshot
capture, destination runtime construction, clear SessionStart hooks, and
`ExitReason::Clear` source shutdown. Main TUI shortcut/control paths now
activate an already-live AppServer session by `SessionId` before sending typed
requests; they no longer pass `SessionHandle` back into a bridge bind/register
entrypoint.
Prompt-mode bash still executes the shell command off the driver loop, but any
model response turn is handed back to the main driver and starts through the
same local bridge. Test/embedding headless callers that already hold typed
messages pass them as `session/start.initial_messages`; the AppServer-owned
start builder hydrates that initial history before the first turn.

The start DTO is authority-safe. Serialized `SessionStartParams` carries no
`session_id`, `initial_messages`, or `initial_prompt`;
`#[serde(deny_unknown_fields)]` rejects legacy fields as invalid params. Remote
start mints its identity and loads through a new-only reservation that accepts
only a fresh `Started` slot. The local seed is realized as `#[serde(skip)]`
fields settable only by in-process construction. `max_turns`,
`max_budget_usd`, system-prompt variants, and `json_schema` are consumed by
the session fold; initialize carries only connection capabilities (residual:
`plan_mode_instructions` still rides on initialize, T8); start/resume results
require `surface_id`.

Replace ownership is uniform. Every replace branch runs in an AppServer-owned
task with a retained join handle (`spawn_tracked` + `owner_tasks`;
`abort_and_join_owner_tasks` on shutdown), and a post-commit source-close
failure surfaces as a structured `committed_close_failed` error rather than
silent success.

The shared lifecycle conformance coverage verifies start/read/close and
durable resume/read/close against the local typed surface, the JSON-RPC
AppServer bridge, and the concrete Unix NDJSON sidecar binding on Unix. SDK
stdio is covered in `coco-sdk-server` through `SdkServer::run_app_server_connection`
and `InMemoryTransport`, keeping SDK transport tests in the crate that owns the
SDK wire boundary. WebSocket and Windows named-pipe sidecar bindings are not
part of that matrix yet. The process-level project-service owner now has an
explicit background-task shutdown policy; CLI process exit and SDK remote-host
shutdown both call it instead of relying on static drop or process termination.

## Close and delete behavior

The Rust protocol and typed clients now expose separate lifecycle and durable
storage operations:

1. `session/close`:
   - validates interactive or orphan close authority through AppServer;
   - runs through the AppServer registry close owner callback;
   - persists the sequence watermark;
   - asks `SessionHandle` to drain any still-registered active turn;
   - stops reload supervision;
   - fires SessionEnd hooks;
   - cancels runtime shutdown;
   - emits the aggregate `SessionResult`;
   - preserves the persisted transcript.
2. `session/delete`:
   - takes a `SessionTarget`;
   - rejects any Loading/Live/Closing registry slot with `SessionStillLive`;
   - calls `SessionManager::delete` only after the live-slot check passes.

Turn/close ordering is authoritative: the event forwarder holds the terminal
`TurnEnded` until `commit_engine_turn_history` completes, so an immediate next
turn observes committed history (named regression still to add, T6). Close
drains the turn task then the forwarder under one absolute
`timeout_at(deadline, …)` budget, and `join_session_tasks(deadline)` joins the
session task supervisor. Residuals: supervisor adoption is one site deep —
most session-owned spawns still use raw `tokio::spawn` and need per-site
triage (T7); generated Python/TypeScript clients still need lifecycle
conformance coverage beyond codegen and unit tests.

## Event and Hub flow

AppServer session events are stamped with session/turn/agent identity and
routed through bounded per-connection channels and per-session replay rings.
That model remains appropriate.

`ProcessEventHub` is now the connector owner. It can be created by the process
host with an explicit live-session snapshot, including an empty snapshot during
SDK zero-session startup, and the connector worker announces immediately on
startup. SDK remote, TUI, and headless startup paths attach the process-owned
egress to AppServer event routing and run a membership watcher that reads
`AppServer::announced_session_ids()` (Live plus retiring Closing slots) after
AppServer activity revisions. Local
sidecar and SDK stdio writers also refresh membership immediately before
forwarding a session event to the Hub. The remaining gaps are validation and
protocol edge coverage: close, replace, reconnect cursor behavior, and event
identity/ack isolation still need dedicated regressions. SDK remote startup
and A/B start membership are covered by focused regressions.

The R17 membership hole is closed: a Closing session stays announced until the
completed close cascade removes its slot, so re-announce cannot drop the
identity before final egress is enqueued. Two protocol-level residuals remain
(T9): the watcher still wakes on the general activity revision and diffs
snapshots rather than subscribing to a dedicated lifecycle revision, and
reconnect cursor requests are not yet scoped to the retiring set.

## Module and dependency shape

Current `coco-agent-host` characteristics:

- most `lib.rs` modules narrowed to `pub(crate)`; `SessionRuntime` is
  crate-private; the facade exposes intentional capabilities only;
- the previously oversized modules are split into directory modules, each file
  well under the 800-line target: `session_handle/` (9 files), `state/` (10),
  `local_client/` (8), `headless/` (7);
- protocol-neutral: no dependency on `coco-tui` or `coco-sdk-server` (seam-guarded);
- the dead `output` module (no production callers) and the superseded
  `session_replacement` module were removed;
- top-level modules are grouped under `session`/`integrations`/`host`/
  `client`/`lifecycle` (Phase H #7; the plan's `protocol` group had no
  agent-host members), with `event_hub`, `headless`, `options`, and `paths`
  at the root and companion `*.test.rs` files alongside every implementation.

Current `coco-cli` characteristics:

- clap schema, `ExecutionPlan`, and tracing/process helpers;
- three surface directories: `app/cli/src/{tui,headless,sdk}` — the TUI
  application driver, the headless one-shot runner, and the SDK process
  entrypoint `run_sdk_mode` (moved here from `coco-sdk-server` in Phase G4);
- many direct dependencies on application/core crates (composition root).

## Test coverage and gaps

Existing production tests cover explicit authority, two-session runtime and
integration isolation, callbacks, replay, reload ownership, slow consumers,
orphan authority, and concurrent registry shutdown.

The full workspace `pre-commit` suite (fmt, seam guards, error policy,
full-workspace clippy, complete nextest run, SDK Python suite) passed on
2026-07-14. Now proven since the 2026-07-13 audit: remote start cannot
name/mutate another live or orphaned session
(`session_start_rejects_existing_live_id_without_mutation`); legacy/unknown
start fields are rejected (`deny_unknown_fields` + consumption tests);
agent-host has no TUI or SDK-transport dependency (seam script in
`pre-commit`); close preserves/delete removes, timeout abort+join, and
in-flight-turn close accounting.

Still not proven by tests (tracked in follow-up-todo.md):

- an immediate next turn sees the just-completed engine history (T6);
- every session-owned background task is joined under the close deadline
  (T7 — supervisor exists; per-site adoption incomplete);
- Hub close/replace/reconnect includes final-event cursor state for the
  retiring set (T9).

The public-lock-accessor residual is closed: `live_permission_rules()` now
returns the narrow `LivePermissionRulesHandle` (T1) and the dead `mcp_manager()`
raw-lock accessor was removed; no `SessionHandle` method returns a raw lock.
