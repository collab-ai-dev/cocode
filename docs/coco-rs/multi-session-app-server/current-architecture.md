# Current Architecture

Audit baseline: production tree on 2026-07-13.

This document is descriptive. It includes defects and transitional behavior;
it is not the target design. Listing a defect here does not automatically make
it a blocker for the active workstream; admission and ordering follow the
convergence gates in `remediation-plan.md`.

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

TUI startup is implemented in `coco-cli/src/tui_runner/bootstrap.rs`. It:

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

The TUI runner contains application operations as well as presentation policy.
Agent-host still supplies several TUI-specific permission, voice, teammate,
image, and command adapters, so dependency direction remains inverted.

### Headless

Headless execution is implemented in `coco-agent-host::headless`. It:

1. resolves some local slash commands before runtime construction;
2. folds config and constructs model/tool startup state;
3. constructs another `SessionRuntimeFactory` and one-slot local bridge;
4. calls local `session/start` or `session/resume` so AppServer owns runtime
   registration and history hydration;
5. still applies some live turn configuration directly through
   `SessionHandle`;
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
raw `Mutex`/`RwLock` (the lock accessors are `pub(crate)` or replaced by narrow
snapshot ops). Session identity lives only on the immutable handle /
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

That transitional start DTO is not authority-safe. Because serialized start
also accepts `session_id` and the generic load path returns an existing Live
handle, a second connection can reach and mutate runtime state before surface
attachment rejects the ownership conflict. The start mapping also ignores
declared `max_turns`, `max_budget_usd`, system-prompt variants, and
`initial_prompt`. Initialize duplicates several session-policy fields, while
start/resume response surface ids are optional. These are current defects, not
v2 compatibility requirements.

Replace ownership is also inconsistent. The already-live destination path
commits routing and launches old-session close with a bare `tokio::spawn`, then
returns success without a retained owner/completion. Other replacement paths
use `spawn_replace`, but destination promotion can resolve the waiter before a
source-close failure is known. Post-commit close failure therefore has no
uniform typed result today.

The resulting runtimes are similar, but the ordering and ownership remain
transitional until production-surface startup adapters are covered end to end.
The shared lifecycle conformance coverage now verifies start/read/close and
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

Remaining defects: the event forwarder clears turn admission before
`SessionTurnExecutor` commits the engine's final history, so an immediate next
turn can read stale history. Active turn and forwarder drains each receive the
full configured timeout rather than sharing one deadline, and close has no
single registry of all session-owned background tasks. Generated
Python/TypeScript clients still need lifecycle conformance coverage beyond
codegen and unit tests.

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

There is also a semantic gap before those tests: the watcher subscribes to a
general activity revision and derives membership from
`list_live_sessions()`, which excludes Closing slots. The close owner emits its
final `SessionResult` while the slot is Closing. Membership can therefore be
re-announced without that identity before final egress is enqueued, and a
reconnect cursor negotiation may omit the final event.

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
- top-level module grouping under `session`/`integrations`/`host`/`protocol`/
  `lifecycle`/`client` (Phase H #7) is not yet applied — modules remain flat at
  the source root.

Current `coco-cli` characteristics:

- clap schema and tracing/process helpers;
- a large TUI application driver;
- mode-specific startup and shutdown policy;
- many direct dependencies on application/core crates;
- direct `main.rs` dependency on `coco-sdk-server::run_sdk_mode` startup
  orchestration.

## Test coverage and gaps

Existing production tests cover explicit authority, two-session runtime and
integration isolation, callbacks, replay, reload ownership, slow consumers,
orphan authority, and concurrent registry shutdown.

The focused `coco-app-server` suite (92 tests) and `coco-agent-host` suites (331
unit, 26 multi-session integration, and one WebSocket test) passed during this
audit. They do not currently prove:

- remote start cannot name/mutate another live or orphaned session;
- every accepted initialize/start field is consumed or rejected;
- an immediate next turn sees the just-completed engine history;
- replace waits for and reports source close failure on every branch;
- every session/background/lifecycle owner task is joined under one deadline;
- Hub close/replace/reconnect includes final-event cursor state;
- agent-host has no TUI dependency;
- session public APIs do not expose raw locks.
