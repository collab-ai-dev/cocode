# Current Architecture

Audit baseline: production tree on 2026-07-13.

This document is descriptive. It includes defects and transitional behavior;
it is not the target design.

## Crate topology

The durable AppServer split is:

```text
coco-types
   ^
   +---------------- coco-app-server-transport
   |                          ^
   |                          |
coco-app-server       coco-app-server-client
   ^                          ^
   |                          |
   +-------- coco-agent-host--+
                 ^
                 |
          coco-app-runtime
```

Useful properties:

- server and remote client share DTOs and transport primitives, not server
  implementation details;
- `coco-app-server` is generic over a cloneable live handle;
- query/core/service crates do not depend on AppServer crates;
- `coco-app-runtime` owns process/project/bootstrap contracts rather than query
  execution.

The omitted but important current edge is:

```text
coco-agent-host -> coco-tui
```

That edge exists because agent-host contains TUI permission, voice, teammate,
image, and message adapters. It contradicts the intended protocol-neutral host
boundary.

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

TUI startup is implemented in `coco-cli/src/tui_runner/bootstrap.rs`. It:

1. consumes teammate identity environment;
2. folds startup config and builds engine resources;
3. creates the session manager and TUI channels/permission bridge;
4. constructs `SessionRuntimeFactory`;
5. constructs a one-slot `AppServerLocalBridge`;
6. builds and registers a runtime;
7. installs integrations, reload subscriptions, watchers, and hooks;
8. binds an interactive local surface;
9. directly hydrates startup resume state when the runtime already has the
   target id;
10. creates TUI state and spawns the command driver;
11. drains AppServer and Event Hub through `ShutdownCoordinator` during exit.

The TUI runner contains application operations as well as presentation policy.
Its module tree is about 7,300 non-test lines.

### Headless

Headless execution is implemented in `coco-agent-host::headless`. It:

1. resolves some local slash commands before runtime construction;
2. folds config and constructs model/tool startup state;
3. constructs another `SessionRuntimeFactory` and one-slot local bridge;
4. builds/registers a runtime and installs integrations;
5. directly seeds resume transcript state;
6. applies live turn configuration directly through `SessionHandle`;
7. starts a turn through local AppServer;
8. waits for `TurnEnded` through local AppServer and uses the completion's
   embedded per-turn session result directly;
9. drains the local server and Event Hub through `ShutdownCoordinator`.

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

It is constructed with `Default` and made usable through later `install_*`
calls. Some startup installs use `try_write` and panic if the lock is not
immediately available.

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

`SessionHandle` keeps `Arc<SessionRuntime>` private, but exposes a very broad
forwarding API. Several methods return raw locks or internal manager/registry
handles. Session identity is duplicated between the immutable handle and
mutable `QueryEngineConfig`; mutation is guarded by a runtime assertion.
Callback requirements are installed after construction through `OnceLock` and
default to empty when read before installation.

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

The resulting runtimes are similar, but the ordering and ownership are not one
fully shared lifecycle until shared conformance coverage proves the same
boundary across all connection styles and the remaining process-service stop
policy is explicit.

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

Remaining defects: terminal turn/accounting ordering is still not proven by an
authoritative `TurnEnded` contract. Phase A now has compiled regressions for
timeout abort/join, successful-close no-late outbound events, byte-for-byte
close preservation, and close-during-turn accounting, but those regressions
still need to be run in the next batched test pass. Generated Python/TypeScript
clients still need lifecycle conformance coverage beyond codegen and unit
tests.

## Event and Hub flow

AppServer session events are stamped with session/turn/agent identity and
routed through bounded per-connection channels and per-session replay rings.
That model remains appropriate.

`ProcessEventHub` is now the connector owner. It can be created by the process
host with an explicit live-session snapshot, including an empty snapshot during
SDK zero-session startup, and the connector worker announces immediately on
startup. SDK remote, TUI, and headless startup paths attach the process-owned
egress to AppServer event routing and run a membership watcher that reads
`AppServer::list_live_sessions()` after AppServer activity revisions. Local
sidecar and SDK stdio writers also refresh membership immediately before
forwarding a session event to the Hub. The remaining gaps are validation and
protocol edge coverage: close, replace, reconnect cursor behavior, and event
identity/ack isolation still need dedicated regressions. SDK remote startup
and A/B start membership are covered by focused regressions.

## Module and dependency shape

Current `coco-agent-host` characteristics:

- 69 public modules from `lib.rs`;
- 71 non-test, non-`lib.rs` Rust files at the source root;
- 18 top-level `session_*` files plus another 16 `app_server_host/session_*`
  files;
- both flat `session_*` files and fragmented `app_server_host` lifecycle step
  files;
- direct dependencies on most application domains plus `coco-tui`;
- large modules including `session_handle.rs`, `headless.rs`, `local_client.rs`,
  and session runtime state;
- a public output module with no production callers.

Current `coco-cli` characteristics:

- clap schema and tracing/process helpers;
- a large TUI application driver;
- mode-specific startup and shutdown policy;
- many direct dependencies on application/core crates;
- placeholder or no-op flags/subcommands.

## Test coverage and gaps

Existing production tests cover explicit authority, two-session runtime and
integration isolation, callbacks, replay, reload ownership, slow consumers,
orphan authority, and concurrent registry shutdown.

They do not currently prove:

- close preserves JSONL;
- delete is a separate explicit operation;
- timed-out close leaves no detached work or late events;
- terminal accounting includes the drained turn;
- CLI flags select the documented modes;
- SDK startup begins with an empty registry;
- Event Hub announces all current live sessions;
- agent-host has no TUI dependency;
- session public APIs do not expose raw locks.
