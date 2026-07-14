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
- transport or UI types in agent-host session modules;
- eliminating every adjacent agent-host/CLI/transport cleanup in this refactor;
- preserving unsupported flags or placeholder commands.

## Delivery constraints

Architecture convergence is part of the target:

1. prove the four lifecycle invariants before moving files or changing crate
   boundaries;
2. change surface dependency direction only after lifecycle contracts are
   frozen, without changing their behavior;
3. narrow APIs and reorganize modules only after the dependency graph is
   stable, without changing protocol schemas;
4. keep the crate graph and no-new-runtime-crate decision in this document
   frozen unless a correctness invariant is shown to be impossible;
5. treat unrelated cleanup and additional transport/platform coverage as later
   work, not as a reason to expand an active gate.

Each lifecycle operation has one named owner and one externally meaningful
completion point. A component-level event, registry promotion, cancellation
request, or task spawn is not completion unless the operation contract says so.
The exact workstream gates are normative in `remediation-plan.md`.

## Dependency architecture

The target reuses the existing crates and repairs their ownership boundaries:

```text
common/core/services       coco-app-server/client/transport
        ^                               ^
        |                               |
coco-app-runtime <----- coco-agent-host-+
 process/bootstrap       process host + private session aggregate
                               ^
                  +------------+-------------+
                  |                          |
        app/cli/src/{tui,headless,sdk}   coco-sdk-server
          executable composition        reusable SDK transport adapter
```

### `coco-agent-host`

Owns the application-specific AppServer host and its private session aggregate:

- `HostBuilder` and fully initialized `PreparedHost`;
- `AppServer<AppSessionHandle>` composition;
- connection handler factory and protocol dispatch;
- start/resume/replace/close/delete orchestration;
- `SessionRuntime`, focused session capabilities, and turn coordination;
- history, usage, hooks, MCP, LSP, sandbox, tasks, memory, and reload ownership;
- local typed host client used by local surfaces;
- process session catalog and sequence allocation;
- registry-driven Event Hub membership/egress;
- process shutdown coordination.

It depends on AppServer, application runtime, query/core/services, and common
DTO crates. It does not depend on TUI or SDK transport crates. This refactor
does not introduce `coco-agent-runtime`: moving the same implementation into a
new crate before a second consumer exists would add a dependency boundary
without reducing ownership complexity. Future extraction requires a clean
facade plus demonstrated reuse or measurable compile/dependency benefit; it
must move, never copy, the implementation.

### `coco-cli`

The executable crate owns clap, `ExecutionPlan`, sandbox/tracing pre-dispatch,
and three direct surface directories. There is no intermediate `surfaces/`
directory and no new runner crate:

```text
app/cli/src/
  main.rs
  tui/
    mod.rs
    bootstrap.rs
    driver.rs
    ...
  headless/
    mod.rs
    input.rs
    runner.rs
    output.rs
    signal.rs
  sdk/
    mod.rs
    runner.rs
```

`tui/` owns:

- terminal lifecycle and TEA application loop;
- TUI channels and presentation-only state hydration;
- TUI permission/sandbox dialogs and rendering adapters;
- editor, keybinding, theme, voice, and teammate UI policy;
- mapping TUI commands to typed local host-client operations.

`headless/` owns:

- structured input/output formats;
- prompt/stdin handling;
- typed local host-client lifecycle;
- turn completion/result rendering;
- non-interactive permission and process-signal policy.

It does not receive `SessionHandle`, construct a runtime, or host shared
config/model/session-factory helpers. Those remain in agent-host under neutral
bootstrap/session names.

`sdk/` owns only executable SDK-mode composition:

- conversion from `ExecutionPlan::Sdk` to startup inputs;
- `PreparedHost` construction/invocation;
- sidecar/listener configuration chosen by the CLI;
- process signal and shutdown policy;
- calling the SDK transport adapter.

The current `coco-sdk-server::startup::run_sdk_mode` orchestration moves here.
Transport implementations do not.

### `coco-sdk-server`

Continues to own SDK transport only:

- stdio/sidecar connection acceptance;
- frame I/O and writer ordering;
- JSON-RPC correlation and rendering;
- callback replies bound to the connection.

It remains a separate reusable crate for SDK clients, IDE integrations,
sidecars, and transport tests. It receives host/connection capabilities; it
does not select CLI mode, build a session, or own process policy. Renaming or
folding this working transport boundary into the binary is out of scope.

## Module organization

Modules are organized by owner and behavior, not by filename prefixes or one
file per processing step.

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
  session/
    mod.rs
    builder.rs
    handle.rs
    identity.rs
    turn.rs
    teardown.rs
    task_scope.rs
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
- do not split `host`, `protocol`, or `lifecycle` into one file per trivial
  processing step;
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
    pub execution: SessionExecutionPolicy,
    pub callback_requirements: SessionCallbackRequirements,
    pub interaction: InteractionPolicy,
    pub file_history: FileHistoryPolicy,
}
```

`SessionExecutionPolicy` is the validated one-owner representation of start
model, permission, turn/budget limits, system-prompt replacement/append, JSON
schema, and plan-mode instructions. `ConnectionProfile` contains only
connection capabilities/resources. No field is accepted by the protocol and
then discarded during this conversion.

There is no startup snapshot runtime, no mutable runtime identity, and no
post-construction callback-requirement install.

## Session capability design

`SessionRuntime` is private to `coco-agent-host::session`. The registry stores a
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
  -> Finishing { lifecycle owner still holds admission }
  -> committed history/accounting + joined tasks + terminal delivery
  -> Idle
```

The lifecycle owner, not the event forwarder, performs the transition. The
terminal event is emitted only after:

1. engine task completion;
2. event forwarder drain;
3. history/accounting commit.

A new turn cannot start until terminal delivery has completed and the owner has
returned the coordinator to Idle. Event forwarding may report data but cannot
clear active handles or make the session admit another turn.

## Lifecycle semantics

### Start

Remote `session/start` contains per-session execution policy but no client
chosen `SessionId`, `initial_messages`, or initial prompt. The server mints one
identity, proves its slot is Missing, builds one runtime from the calling
profile, promotes it to Live, and attaches an interactive surface as one owner
operation. Loading, Live, or Closing conflicts have zero runtime/config/history
side effects. It never loads, rebinds, or replaces another session.

A non-serialized `LocalStartSeed` may supply a chosen fresh id and typed history
for process-local tests/embeddings. It still requires a Missing slot and uses
the same owner. Production history restoration uses resume; user input starts
with `turn/start`.

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

Every replace branch, including an already-live destination, runs under one
AppServer-owned owner task with a retained completion handle. Success is
reported only after destination commit and source close complete. A failure
after commit returns a typed `CommittedCloseFailed` lifecycle outcome carrying
the committed destination identity; it is never logged and converted to
success. Panics cannot strand a slot in Closing.

The clear destination is a first-class `session/replace` variant. The handler
captures the source runtime snapshot, constructs a fresh destination identity,
applies clear-preserved state, emits clear SessionStart hooks, and closes the
source with `ExitReason::Clear`. Surface code never builds or registers the
clear replacement runtime directly.

### Close

`session/close` closes a live runtime and preserves durable transcript data.
It accepts explicit interactive or orphan authority as defined in
`protocol-scope.md`.

One owner performs this sequence under one absolute `Instant` deadline:

1. mark the slot Closing and reject new turns;
2. cancel the active turn;
3. await turn and forwarder using the remaining time from that deadline;
4. abort and await either task that exceeds the deadline;
5. stop/await session background tasks and reload supervisors;
6. run bounded SessionEnd hooks;
7. flush history, usage, and sequence watermark;
8. close MCP/LSP/sandbox/file resources;
9. cancel the session shutdown token;
10. build and route the final `SessionResult` through process egress;
11. complete the local egress handoff and membership retirement;
12. detach surfaces and remove the registry slot.

Session-owned work is registered in a narrow lifecycle task supervisor
(`JoinSet`, `TaskTracker`, or equivalent). On timeout the owner cancels,
aborts, and joins every registered task before returning a structured non-clean
close outcome. Sequential waits do not each receive the full timeout. No
session event may be emitted after close completion. This is a bounded local
queue/replay handoff, not a wait for a remote Hub network acknowledgement;
connector outage is process-egress health and does not extend session close or
block unrelated sessions.

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

`ProcessEventHub` subscribes to a dedicated AppServer lifecycle revision, not
general session activity, and maintains announced plus retiring membership. On
connect/reconnect it announces a registry snapshot containing Live sessions
and Closing sessions whose final local egress handoff is incomplete. Lifecycle
changes either update membership through the Hub protocol or force a bounded
reconnect if the protocol has no update frame.

Rules:

- no placeholder session;
- every Live or retiring Closing session is represented in membership;
- resume cursors are requested for every represented session;
- close retires membership only after the final event is handed to egress;
- event envelopes still carry their own authoritative session id;
- one session's config cannot silently retarget process egress; Event Hub URL
  is resolved as explicit process-host policy;
- Hub tests include two sessions, replace, close, disconnect, and reconnect.

## Shutdown

`ShutdownCoordinator` owns process teardown for all surfaces:

1. stop accepting connections and lifecycle starts;
2. begin close for every registry slot;
3. drain sessions concurrently under the process deadline;
4. cancel, abort, and join all remaining session and lifecycle owner tasks;
5. flush Event Hub;
6. stop project-service managers and watchers;
7. return a structured non-clean outcome when any phase fails or times out.

TUI, headless, and SDK call the same coordinator. Presentation may differ, but
ordering and failure semantics do not.

## Mechanical architecture gates

The target adds repository checks for:

- no `coco-agent-host` dependency on `coco-tui` or `coco-sdk-server`;
- no AppServer dependency below the host layer;
- no public session method returning `Mutex`, `RwLock`, or internal managers;
- no production computed string byte slicing;
- no accepted CLI flag without an execution-plan consumer and behavioral test;
- no accepted protocol field without a validation/consumption site or explicit
  rejection test;
- serialized `SessionStartParams` has no session id or initial history;
- legacy start identity/history fields are rejected, not ignored;
- start/resume success surface ids are required;
- exhaustive request scope and dispatch matches;
- `clippy::await_holding_lock = deny` remains enabled;
- session/turn telemetry fields remain required.
