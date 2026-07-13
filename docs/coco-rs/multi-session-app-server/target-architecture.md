# Target Architecture V2

This document is normative. Backward compatibility with current target-less
CLI behavior, no-op flags, hidden SDK startup sessions, or
`session/archive` semantics is explicitly out of scope.

## Goals

The target provides:

1. one process hosting zero or more independent root sessions;
2. one immutable `SessionId` per root conversation/runtime;
3. explicit connection/surface/session authority for every live mutation;
4. independent cwd, config, tools, MCP, history, permissions, tasks, hooks,
   reload, accounting, and shutdown for every session;
5. one shared lifecycle used by TUI, headless, SDK, and future surfaces;
6. process startup that does not create a session;
7. deterministic close with no late tasks/events;
8. separate close and durable delete operations;
9. process-level Event Hub egress derived from live registry membership;
10. clear dependency direction from domain runtime to host to surfaces;
11. narrow Rust capabilities that do not expose internal locks.

Non-goals:

- a second root `ThreadId`;
- a whole-runtime actor;
- speculative project-shared mutable services;
- a multi-session TUI user interface;
- public unauthenticated network listeners;
- transport or UI types in session runtime crates;
- preserving unsupported flags or placeholder commands.

## Dependency architecture

The target crates are:

```text
common/core/services
        ^
        |
coco-app-runtime
  process/project/bootstrap contracts
        ^
        |
coco-agent-runtime                 coco-app-server
  session aggregate                    registry/routing/replay
        ^                                  ^
        +----------- coco-agent-host ------+
                      process host,
                      protocol handlers,
                      local host client
                          ^
             +------------+-------------+
             |            |             |
     coco-tui-runner  coco-headless  coco-sdk-server
             ^            ^             ^
             +------------+-------------+
                          |
                       coco-cli
```

### `coco-agent-runtime` (new)

Owns application session behavior without AppServer, transport, or TUI types:

- `SessionRuntime`, construction, and private resource ownership;
- `LiveSession`/focused capabilities;
- turn coordination and engine assembly;
- history, persistence, usage, file history, and session config;
- hooks, MCP, LSP, sandbox, tasks, memory, skills, commands, and reload;
- session close preparation and integration teardown.

It may depend on `coco-app-runtime`, query/core/services, and common DTOs. It
must not depend on `coco-app-server`, `coco-app-server-client`, `coco-tui`, or
transport crates.

### `coco-agent-host`

Owns the application-specific AppServer host:

- `HostBuilder` and fully initialized `PreparedHost`;
- `AppServer<AppSessionHandle>` composition;
- connection handler factory and protocol dispatch;
- start/resume/replace/close/delete orchestration;
- local typed host client used by local surfaces;
- process session catalog and sequence allocation;
- registry-driven Event Hub membership/egress;
- process shutdown coordination.

It depends on `coco-agent-runtime` and AppServer crates. It does not depend on
TUI crates.

### `coco-tui-runner` (new)

Owns the TUI surface composition that currently lives in CLI and agent-host:

- terminal lifecycle and TEA application loop;
- TUI channels and presentation-only state hydration;
- TUI permission/sandbox dialogs and rendering adapters;
- editor, keybinding, theme, voice, and teammate UI policy;
- mapping TUI commands to typed local host-client operations.

It depends on `coco-tui` and `coco-agent-host`. It does not receive raw
`SessionRuntime` or session locks.

### `coco-headless`

Owns one-shot/scripting surface policy:

- structured input/output formats;
- prompt/stdin handling;
- typed local host-client lifecycle;
- turn completion/result rendering;
- non-interactive permission policy.

It does not host config/bootstrap helpers for other surfaces.

### `coco-sdk-server`

Continues to own SDK transport only:

- stdio/sidecar connection acceptance;
- frame I/O and writer ordering;
- JSON-RPC correlation and rendering;
- callback replies bound to the connection.

It receives a `PreparedHost`; it does not construct a session.

### `coco-cli`

Owns only:

- clap schema;
- pure conversion to `ExecutionPlan`;
- sandbox pre-dispatch;
- startup cwd and tracing installation from the plan;
- embedded listener policy;
- invocation of the selected surface/command.

## Module organization

Modules are organized by owner and behavior, not by filename prefixes or one
file per processing step.

Suggested `coco-agent-runtime` layout:

```text
src/
  lib.rs
  session/
    mod.rs
    builder.rs
    handle.rs
    identity.rs
    turn.rs
    lifecycle.rs
    history.rs
    controls.rs
  integrations/
    mod.rs
    hooks.rs
    mcp.rs
    lsp.rs
    sandbox.rs
    tasks.rs
    memory.rs
  engine/
    mod.rs
    build.rs
    config.rs
```

Suggested `coco-agent-host` layout:

```text
src/
  lib.rs
  host/
    mod.rs
    builder.rs
    state.rs
    shutdown.rs
    event_hub.rs
  protocol/
    mod.rs
    handler.rs
    dispatch.rs
    context.rs
    error.rs
    connection.rs
  lifecycle/
    mod.rs
    start.rs
    resume.rs
    replace.rs
    close.rs
    delete.rs
  client/
    mod.rs
    local.rs
```

Rules:

- modules are private by default;
- `lib.rs` re-exports only intentional facade types;
- keep modules near the 800-line repository target;
- do not create tiny pass-through files unless they own an invariant;
- tests remain companion `*.test.rs` files;
- protocol dispatch is exhaustive in one host match after AppServer scope
  resolution; lifecycle requests are not matched again in a second fallback
  dispatcher.

## Process and session construction

### Typed execution plan

CLI parsing produces exactly one plan before tracing or application startup:

```rust
pub enum ExecutionPlan {
    Tui(TuiPlan),
    Headless(HeadlessPlan),
    Sdk(SdkPlan),
    Command(CommandPlan),
}

pub struct IoCapabilities {
    pub stdin_is_terminal: bool,
    pub stdout_is_terminal: bool,
    pub stderr_is_terminal: bool,
}
```

The conversion is pure and table-tested. It rejects incompatible flags and
contains all output/input mode decisions. Tracing mode is derived from the
plan; it does not re-run mode detection.

Unsupported flags and subcommands are removed. A command may be present only
when it has a production handler and behavioral test.

### Host construction

`HostBuilder` takes all required process-lifetime dependencies and returns a
fully valid `PreparedHost`:

```rust
pub struct HostInputs {
    pub startup_cwd: AbsolutePathBuf,
    pub process_runtime: Arc<ProcessRuntime>,
    pub host_config: Arc<HostConfig>,
    pub initialize_catalog: Arc<InitializeCatalog>,
}

pub struct PreparedHost {
    app_server: Arc<AppServer<AppSessionHandle>>,
    session_factory: SessionFactory,
    connection_factory: ConnectionHandlerFactory,
    shutdown: ShutdownCoordinator,
    event_hub: Option<ProcessEventHub>,
}
```

Required fields are constructor inputs, not later `install_*` calls. Immutable
startup data uses immutable fields/`Arc`, not `RwLock<Option<_>>`.

Building `PreparedHost` creates zero live sessions. TUI/headless open a local
connection and call lifecycle methods. SDK opens remote connections and waits
for client lifecycle methods.

### Session construction

`SessionFactory` performs exactly one per-session fold for the target cwd. It
receives explicit construction policy rather than ambiguous booleans:

```rust
pub enum FileHistoryPolicy {
    Enabled,
    Disabled,
}

pub enum InteractionPolicy {
    LocalInteractive,
    RemoteInteractive,
    NonInteractive,
}
```

Session identity and callback requirements are construction inputs:

```rust
pub struct SessionBuildRequest {
    pub session_id: SessionId,
    pub cwd: AbsolutePathBuf,
    pub connection_profile: Arc<ConnectionProfile>,
    pub callback_requirements: SessionCallbackRequirements,
    pub interaction: InteractionPolicy,
    pub file_history: FileHistoryPolicy,
}
```

There is no startup snapshot runtime, no mutable runtime identity, and no
post-construction callback-requirement install.

## Session capability design

`SessionRuntime` is private to `coco-agent-runtime`. The registry stores a
cloneable live capability with immutable identity:

```rust
#[derive(Clone)]
pub struct LiveSession {
    identity: Arc<SessionIdentity>,
    inner: Arc<SessionRuntime>,
}
```

Public operations are high level:

```rust
impl LiveSession {
    pub fn id(&self) -> &SessionId;
    pub fn snapshot(&self) -> impl Future<Output = SessionSnapshot>;
    pub fn start_turn(&self, request: TurnRequest) -> Result<TurnTicket, TurnError>;
    pub fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), TurnError>;
    pub fn apply_control(&self, control: SessionControl) -> ...;
    pub fn begin_close(&self, reason: CloseReason) -> ...;
}
```

Rules:

- no public `runtime()`, `Deref`, raw `Arc<Mutex<_>>`, or `Arc<RwLock<_>>`;
- no public manager/registry handles;
- reads return immutable snapshots or narrow owned capabilities;
- mutation methods validate invariants inside the owning module;
- `QueryEngineConfig` does not contain `SessionId`;
- turn tasks receive immutable `SessionIdentity` separately;
- closure-based arbitrary engine-config mutation is private;
- callback requirements cannot be absent or replaced after construction.

Fine-grained locks remain appropriate. `SessionTurnCoordinator` owns only
coupled turn state: reservation, active task handles, cancellation, terminal
ordering, and accounting. It is not a god actor.

## Unified surface lifecycle

All surfaces use the same lifecycle boundary:

```text
PreparedHost
  -> open connection
  -> initialize ConnectionProfile
  -> session/start | session/resume
  -> typed interactive SessionClient
  -> turn/start
  -> await matching terminal turn result
  -> optional session/replace
  -> session/close
```

Surface code does not:

- call `SessionFactory` directly;
- register a runtime directly in AppServer;
- hydrate resume state directly;
- install connection callbacks directly;
- mutate session locks directly;
- infer the current session from process state.

TUI may retain one product-selected `SessionClient`. That selection is UI
state, not a process runtime singleton.

## Turn completion and accounting

`turn/start` reserves the turn and returns `TurnId`. The client can await a
typed terminal `TurnEnded` carrying the complete per-turn result needed by
headless/SDK surfaces. No surface polls host projections or fabricates a
fallback result.

The turn coordinator owns:

```text
Idle
  -> Running { turn_id, cancel, turn_task, forwarder_task }
  -> Finishing
  -> Idle + committed accounting + terminal event
```

The terminal event is emitted only after:

1. engine task completion;
2. event forwarder drain;
3. history/accounting commit.

A new turn cannot clear or replace handles belonging to a previous turn.

## Lifecycle semantics

### Start

`session/start` builds one runtime from the calling profile and requested cwd,
promotes it to Live, and attaches an interactive surface. A process-local
caller may provide an explicit `SessionId` when startup policy already resolved
the identity; otherwise the lifecycle owner mints one. Process-local
test/embedding callers may also provide typed `initial_messages`; startup
hydrates that history inside the lifecycle-owned runtime builder before the
first turn. It never replaces another session.

### Resume

`session/resume` targets one durable identity. It loads/hydrates inside the
AppServer lifecycle owner task. A live orphan may be rebound only when callback
requirements match. A Closing session is awaited outside locks and then loaded
normally. Surface code never performs hydration.

### Replace

`session/replace` constructs/hydrates the destination before an atomic
registry/routing commit. The source client is consumed. Pre-commit failure
returns the source client; post-commit close failure never resurrects the old
identity.

The clear destination is a first-class `session/replace` variant. The handler
captures the source runtime snapshot, constructs a fresh destination identity,
applies clear-preserved state, emits clear SessionStart hooks, and closes the
source with `ExitReason::Clear`. Surface code never builds or registers the
clear replacement runtime directly.

### Close

`session/close` closes a live runtime and preserves durable transcript data.
It accepts explicit interactive or orphan authority as defined in
`protocol-scope.md`.

One owner performs this sequence:

1. mark the slot Closing and reject new turns;
2. cancel the active turn;
3. await turn and forwarder under one configured deadline;
4. abort and await either task that exceeds the deadline;
5. stop/await session background tasks and reload supervisors;
6. run bounded SessionEnd hooks;
7. flush history, usage, and sequence watermark;
8. close MCP/LSP/sandbox/file resources;
9. cancel the session shutdown token;
10. build and route the final `SessionResult`;
11. detach surfaces and remove the registry slot.

No session event may be emitted after close completion.

### Delete

`session/delete` removes durable transcript/session artifacts. It:

- requires explicit `SessionTarget` and transport authorization;
- fails with `SessionStillLive` if any Loading, Live, or Closing slot exists;
- performs no runtime lifecycle work;
- reports storage failures rather than logging and returning success;
- does not emit a live-session lifecycle or catalog-refresh notification.

`session/list` is an explicit read. Clients that cache its result invalidate on
their own successful delete response. A future passive session-catalog observer
must use a dedicated catalog subscription/event rather than `SessionEnded`,
which is reserved for live runtime lifecycle.

Close and delete are never combined implicitly.

## Event Hub

Event Hub egress is process-host state, not session-owned state.

`ProcessEventHub` subscribes to AppServer lifecycle changes and maintains the
announced live-session set. On connect/reconnect it announces a registry
snapshot. Lifecycle changes either update membership through the Hub protocol
or force a bounded reconnect if the protocol has no update frame.

Rules:

- no placeholder session;
- every live session is represented in membership;
- resume cursors are requested for every live session;
- event envelopes still carry their own authoritative session id;
- one session's config cannot silently retarget process egress; Event Hub URL
  is resolved as explicit process-host policy;
- Hub tests include two sessions, replace, close, disconnect, and reconnect.

## Shutdown

`ShutdownCoordinator` owns process teardown for all surfaces:

1. stop accepting connections and lifecycle starts;
2. begin close for every registry slot;
3. drain sessions concurrently under the process deadline;
4. abort remaining session owner tasks and await aborts;
5. flush Event Hub;
6. stop project-service managers and watchers;
7. return a structured non-clean outcome when any phase fails or times out.

TUI, headless, and SDK call the same coordinator. Presentation may differ, but
ordering and failure semantics do not.

## Mechanical architecture gates

The target adds repository checks for:

- no `coco-agent-host` or `coco-agent-runtime` dependency on `coco-tui`;
- no AppServer dependency below the host layer;
- no public session method returning `Mutex`, `RwLock`, or internal managers;
- no production computed string byte slicing;
- no accepted CLI flag without an execution-plan consumer and behavioral test;
- exhaustive request scope and dispatch matches;
- `clippy::await_holding_lock = deny` remains enabled;
- session/turn telemetry fields remain required.
