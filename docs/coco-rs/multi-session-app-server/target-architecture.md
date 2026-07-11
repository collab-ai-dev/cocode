# Target Architecture

This document is normative. Backward compatibility with the current implicit
single-session request behavior is not a goal.

## Goals and value

The target must provide:

1. one process hosting multiple independent root sessions;
2. one immutable `SessionId` as the root conversation identity;
3. explicit targeting and authorization for every session mutation;
4. independent cwd, configuration, tools, MCP, history, permissions, tasks,
   and lifecycle for each session;
5. multiple observing surfaces and at most one interactive owner per session;
6. local-first transports with bounded backpressure and deterministic shutdown;
7. JSONL as canonical transcript storage;
8. no dependency from engine/core crates to AppServer crates.

These goals have product value for IDEs, desktop clients, automation, passive
observers, and long-lived local processes. They also force cleaner state
ownership in single-session modes.

The following are not goals:

- a second root `ThreadId`;
- a multi-session TUI UI in v1;
- public unauthenticated network listeners;
- sharing mutable services merely to reduce object count;
- a whole-runtime actor as an architectural end in itself;
- Web, Desktop, or IM product logic inside `coco-app-server`;
- changing the JSONL persistence model.

## Core invariants

### Identity

- `SessionId` is immutable for the lifetime of a live runtime and transcript.
- `/clear` and replace create a new session; they never retarget an existing
  `SessionHandle` to a new id.
- `SurfaceId` identifies one attachment and is never a root conversation id.
- `ConnectionKey` is server-private and never appears on wire or disk.
- subagents use `AgentId` below one root `SessionId`; they do not create a
  second root identity model.

### Ownership

- `LiveSessionRegistry` is the only process-level map from `SessionId` to a
  live session capability.
- `SessionRuntime` is the only owner of session execution state and
  session-scoped integrations.
- session callback requirements are immutable runtime metadata; mutable
  callback routes belong to AppServer surfaces and connection handlers.
- a handler never reads a process "current session" slot.
- process and project scopes expose immutable snapshots or explicitly keyed
  shared services; they never infer a current session.
- surface state stays in AppServer routing, not in `SessionRuntime`.

### Concurrency

- no `.await` while a registry or routing `std::sync` lock is held;
- registry lock order is registry then routing when both are required;
- one session's slow consumer, turn, MCP process, or shutdown does not block a
  different session;
- coupled turn state has one synchronization owner; unrelated services do not
  share a global mutex or actor mailbox;
- caller cancellation never owns lifecycle progress;
- the no-await-under-lock rule is enforced mechanically by enabling
  `clippy::await_holding_lock` as a workspace lint, not by review alone;
- locks are leaf-level: code holding a guard does not call into methods that
  may acquire other locks.

## Crate responsibilities

### `coco-app-server`

Owns transport-neutral live-session and surface mechanics:

- `LiveSessionRegistry<H>` and lifecycle owner tasks;
- registry/routing atomic commit sections;
- connection and surface indexes;
- interactive ownership and capability validation;
- subscriptions, replay rings, pending server requests, and backpressure;
- `LocalClientAdapter` and `JsonRpcAdapter`;
- graceful process drain primitives.

It treats `H` as an opaque cloneable capability. It does not construct or
inspect `SessionRuntime`.

### `coco-app-server-transport`

Owns frames and byte I/O only:

- JSON-RPC frames and ids;
- NDJSON, Unix socket, named pipe, and WebSocket framing;
- frame limits, read/write errors, and accepted streams.

It owns no coco session or routing state.

### `coco-app-server-client`

Owns remote client behavior:

- connection owner and request correlation;
- typed `RemoteServerClient`;
- interactive and passive session handles;
- per-surface event/lifecycle demultiplexing;
- disconnect invalidation and transport dialing.

It depends on canonical DTOs and transport, never on the server
implementation.

### `coco-app-runtime`

Owns reusable scope and construction contracts:

- `ProcessRuntime` and `ProjectRegistry` lifecycle;
- `ProjectServices` config/catalog snapshots;
- session workspace/project-root resolution;
- bootstrap traits and bundles.

It does not depend on query execution or aggregate application integrations.

### `coco-agent-host`

Owns application use cases:

- `SessionRuntime`, `SessionHandle`, and factory;
- construction from per-session folded configuration;
- turn start/interrupt/steer and runtime controls;
- MCP, hooks, tasks, persistence, file history, sandbox, and reload wiring;
- concrete AppServer request handlers and close cascade;
- in-process typed client used by TUI/headless.

### `coco-cli`

Owns process policy:

- CLI parsing and conversion to `AgentHostOptions`;
- listener enablement and authentication policy;
- signal handling and shutdown deadlines;
- TUI/headless/SDK surface selection;
- top-level construction only.

## Explicit request targeting

Implicit target inference is deleted. The exhaustive request classification,
configuration scopes, connection initialization contract, and server-request
reply validation are normative in [protocol-scope.md](protocol-scope.md).

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTarget {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractiveTarget {
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
}
```

Every operation that mutates or controls live execution or integrations carries
`InteractiveTarget`. Persisted metadata mutations such as rename/tag carry
`SessionTarget` and use the transport authorization boundary. Interactive
examples include:

- `turn/start`, `turn/interrupt`;
- model, thinking, permission, environment, hook, plugin, and MCP controls;
- task and context operations;
- file rewind;
- interactive server-request replies where session ownership matters.

The adapter validates, in one routing lookup, that:

1. the request connection owns `surface_id`;
2. that surface is attached and interactive;
3. it points to `session_id`;
4. the session registry slot is live.

Only after validation does the host receive the opaque live handle. A
`SurfaceId` is not a secret; authorization comes from its association with the
transport connection. If close begins immediately after validation, the
session capability's turn coordinator observes draining state and rejects new
work; a cloned handle never bypasses lifecycle state.

Read-only operations use their natural scope:

- `session/list` is process/catalog scoped;
- `session/read` and `session/turns/list` carry `session_id` and may read
  archived JSONL without an interactive surface;
- `session/subscribe` carries `session_id` and creates a passive surface;
- configuration reads and writes use the required typed scopes in
  `protocol-scope.md`; project/local operations never infer process cwd.

The public client API constructs targets internally:

```rust
impl RemoteSessionClient {
    pub async fn start_turn(&self, input: TurnInput) -> Result<TurnId, ClientError> {
        self.client
            .turn_start(TurnStartParams {
                target: self.target(),
                input,
            })
            .await
    }
}
```

Users cannot accidentally issue an unscoped turn from a session handle.

## Explicit lifecycle protocol

The lifecycle API is intentionally breaking and unambiguous.

### Start

`session/start` always:

1. mints a new `SessionId`;
2. reserves `Loading` under `max_sessions`;
3. builds a fresh runtime using the requested cwd;
4. promotes to `Live`;
5. attaches a new interactive surface to the calling connection.

It never closes or replaces another session implicitly.

### Resume

`session/resume { session_id }` loads or rejoins that identity and attempts to
attach a new interactive surface. If another interactive owner exists, it
returns `InteractiveOwnerConflict`. It does not guess which existing surface
the caller wanted to replace.

If the slot is `Closing`, resume awaits the close completion outside registry
locks and re-enters the normal disk-load path. It never returns the draining
handle.

### Replace

`session/replace` is a real wire operation rather than an implicit side effect
of start/resume.

```rust
pub struct SessionReplaceParams {
    pub source: InteractiveTarget,
    pub destination: SessionReplacement,
}

pub enum SessionReplacement {
    Fresh(SessionStartOptions),
    Resume(SessionId),
}
```

The server constructs or loads the destination before the atomic commit. The
commit validates the source surface, promotes the destination, moves the
source slot to `Closing`, and repoints only the calling surface. Peer surfaces
receive a replacement lifecycle event and detach.

For `Resume`, an already-live destination may be reused only when it has no
interactive owner. A destination with an interactive owner returns a conflict;
Loading is joined; Closing is awaited and reopened. These cases are explicit
registry outcomes, not guesses based on the caller's other surfaces.

The client consumes the source `SessionClient` and creates a new immutable
handle for the same repointed `SurfaceId`. On pre-commit failure it returns the
source handle. After commit, close failure is reported as destination cleanup
status and must not resurrect the old identity.

### Archive

`session/archive` is runtime close, not transcript deletion.

```rust
pub struct SessionArchiveParams {
    pub target: ArchiveTarget,
}

pub enum ArchiveTarget {
    Interactive(InteractiveTarget),
    Orphaned(SessionTarget),
}
```

`Interactive` validates connection/surface ownership like every other
interactive mutation. `Orphaned` exists because a client whose connection
died must be able to close a live session that has no interactive owner
without a resume-then-archive round trip — resume could itself fail with
`ConnectionProfileMismatch` and leave the session unclosable until process
shutdown. `Orphaned` succeeds only when the session currently has no
interactive owner and is authorized by the transport boundary; if an
interactive owner exists it fails with `InteractiveOwnerConflict`. The caller
states which case it claims and the server validates the claim; this is not
an implicit fallback.

In both cases the registry transitions to `Closing`, the host drains
session-owned work, flushes durable state, tears down integrations, removes
routing state, and then removes the slot. JSONL remains resumable.

## Session runtime boundary

The target uses an idiomatic Rust capability handle, not a whole-runtime actor.

```rust
#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<SessionRuntime>,
}

pub struct SessionRuntime {
    id: SessionId,
    config: Arc<RuntimeConfig>,
    project: Arc<ProjectServices>,
    turn: Mutex<TurnCoordinator>,
    // capability-named resource groups and immutable service handles
}
```

`TurnCoordinator` does not exist in the current tree; it is new work owned by
the turn-selection and lifecycle work packages, not a rename of an existing
type.

Rules:

- `SessionHandle::session_id()` is immutable and lock-free;
- no public `Deref<Target = SessionRuntime>` and no public `runtime()` escape;
- host use cases are methods on `SessionHandle` or focused internal service
  handles;
- the shutdown token is private to runtime/lifecycle code;
- mutable locks guard small invariants, not I/O or whole turns;
- a turn task owns long-running engine work and reports completion back to
  `TurnCoordinator`;
- interrupt snapshots the active cancellation token under a short lock and
  cancels it after releasing the guard;
- status uses watch/snapshot state when subscribers need it.

A dedicated coordinator task is acceptable if steering and queued turns later
need ordered asynchronous message handling. It should own only turn
coordination, not config reads, project catalogs, MCP managers, history
storage, or every runtime method.

## Turn execution

The production executor accepts the already validated session capability:

```rust
pub trait TurnExecutor: Send + Sync {
    fn start(
        &self,
        session: SessionHandle,
        params: TurnStartParams,
    ) -> Result<TurnId, TurnStartError>;
}
```

There is no `StateQueryEngineRunner` that looks up a process-installed runtime.
The request path is:

```text
InteractiveTarget
  -> validate connection/surface/session
  -> LiveSessionRegistry::get(session_id)
  -> SessionHandle::start_turn
  -> QueryEngine built from that same handle
  -> event stamped with that same SessionId
```

History, `ToolAppState`, cwd, config, tools, event identity, and active-turn
state therefore derive from one capability, making mixed-session assembly
unrepresentable in the ordinary API.

## SDK host state

The target has three explicit owners:

- `HostProcessState`: immutable process policy, runtime factory, transcript
  catalog/store, durable sequence allocation, shared AppServer, and shutdown;
- `ConnectionHandler`: one per accepted transport, containing its
  `ConnectionKey`, initialize state as `ConnectionProfile`, outbound writer,
  JSON-RPC correlation, and disconnect lifetime;
- `SessionRuntime`: one per live root session, containing all mutable
  execution and integration capabilities selected through the registry.

`JsonRpcConnectionHandlerFactory` may be shared, but `open(connection)` must
return a new handler. A connection handler snapshots its profile into each
start/resume construction request; it never installs live session resources
back into process state.

The current `SdkServerState` must be decomposed field by field. This is the
required destination map, not merely a suggestion to remove four slots:

| Current field | Required target owner |
|---|---|
| `TurnRunnerState` | process-stateless `TurnExecutor` service; every call receives a selected `SessionHandle` |
| `TurnState` | `SessionRuntime` / `TurnCoordinator` |
| `session_activity` | AppServer lifecycle projection keyed by `SessionId` |
| `ScopedSessionState` | live data on `SessionRuntime`; persisted projection in the transcript catalog |
| `PendingClientRequestState` | AppServer pending domain request keyed by connection, surface, session, and request id |
| `ServerRequestState` | per-connection JSON-RPC correlation only; domain ownership stays in AppServer |
| `ConnectionState` | per-connection `ConnectionHandler` |
| `SessionStore` | `HostProcessState` transcript catalog/store |
| `FileHistoryStateSlot` | targeted `SessionRuntime` file-history capability |
| `McpManagerState` | targeted `SessionRuntime` MCP capability |
| `BootstrapState` | immutable process policy plus per-connection `ConnectionProfile`; no mixed aggregate |
| `SessionRuntimeState` | delete after all consumers use the registry-selected `SessionHandle` |
| `RuntimeReloadState` | session-owned `SessionReloadSupervisor` |
| `RuntimeReplacementState` | `HostProcessState` runtime factory and explicit AppServer lifecycle calls |
| `InitializeState` | per-connection `ConnectionProfile` |
| `McpRegistrationState` | targeted session MCP capability, keyed below `SessionId` |
| `session_seq` | process AppServer sequence allocator keyed by `SessionId` |

The installed-runtime, MCP-manager, file-history, and runtime-reload fields are
duplicate process-level owners of session capabilities. They are removed only
after every consumer has moved to its replacement owner. Literal field
deletion before migration would remove working SDK features.

| Session slot to retire | Function that must remain | Replacement owner |
|---|---|---|
| installed `SessionRuntime` | turn execution, runtime controls, approval/sandbox hooks, session operations | registry-selected `SessionHandle` in `SessionRequestContext` |
| installed MCP manager | status, set/reconnect/toggle, tool registration, elicitation routing | MCP capability owned by the targeted `SessionRuntime` |
| installed file history | rewind preview/apply and snapshot lookup | file-history capability owned by the targeted `SessionRuntime` |
| installed runtime reload task | sandbox/config reload subscription | session-owned `SessionReloadSupervisor` stopped by session close |

Migration rules:

- move consumers first and remove the redundant slot last;
- SDK initialization may retain MCP definitions and other construction inputs,
  but never a live current-session manager;
- connection-specific server-request correlation remains on the connection or
  AppServer routing owner and records its session/surface target;
- live orphan resume validates `SessionCallbackRequirements` against the new
  profile before AppServer rebinds callback routes;
- initialize data is accepted exactly once per connection and is never stored
  in a process-wide last-writer-wins slot;
- TUI may keep its product-level selected `SessionHandle`; that is not an SDK
  process-global runtime and is unaffected;
- process startup no longer requires a mutable "current runtime" placeholder.

After migration, session runtime, MCP, file history, reload supervision,
history, and active turn are all resolved from `SessionHandle`.
Connection-specific outbound writers and pending JSON-RPC correlation stay on
the connection owner, not in shared host state.

Removal gate:

1. no production session request calls `session_runtime_snapshot()`;
2. all MCP handlers and callbacks resolve the targeted session capability;
3. rewind reads history/config paths from the targeted session;
4. starting session B does not abort session A's reload supervisor;
5. approval, elicitation, and sandbox callbacks retain session/surface
   correlation;
6. dual-session tests cover all four migrated capabilities;
7. two initialized connections cannot overwrite profiles, writers, callback
   routes, or MCP status owned by one another.

## Project scope

Keep the current `ProjectServices` name for the project-root-scoped aggregate.
Its fields must be named by responsibility.

Current responsibilities remain:

- `ProjectConfigSnapshot`;
- `ProjectCatalogSnapshot`.

Do not introduce `ProjectHeavyServices`. Cost is not a capability, and the
proposed members do not automatically share the same key or lifecycle.

If profiling and semantics justify sharing, add focused owners independently:

- `ProjectLanguageServices` for project-rooted LSP processes;
- `ProjectContextIndex` for explicitly keyed context discovery/index data;
- `ProjectIgnoreCache` only if ignore inputs can be represented in the cache
  key;
- `ProjectMcpRegistry` for explicitly project-scoped MCP instances.

Each addition must define:

1. cache key and definition-site identity;
2. whether session cwd/local settings affect the result;
3. initialization and failure retry semantics;
4. configuration refresh behavior;
5. teardown on project eviction;
6. security/isolation consequences.

Strict single-flight loading is optional. The current optimistic publication
model is valid when duplicate scans are cheap. If loading becomes expensive,
use a per-key initialization cell or loading entry so same-key callers wait
without holding the global project map lock across I/O. Never hold the global
`RwLock` while scanning disk or starting services.

## MCP isolation

Default MCP runtime scope is per session. This gives each session configuration,
tool registry, lifecycle, cwd, elicitation route, and failure isolation.

Project-shared MCP is an explicit later feature, not an automatic optimization.
If implemented, `ProjectMcpRegistry` keys instances by project root plus the
resolved definition identity and configuration fingerprint. Same server names
from different projects never imply sharing. Session-local and project-local
credentials or approvals must remain distinguishable.

SDK MCP controls resolve the targeted `SessionHandle` first. They never read a
process "current MCP manager".

## State projections

- `ToolAppState` remains the session's cross-turn engine/tool state.
- TUI `AppState` remains a surface-local event projection.
- AppServer stores routing and lifecycle metadata, not UI or tool state.
- passive clients reconstruct views from snapshot plus sequenced events.

No new generic `AppState` aggregate is introduced.

## Events and replay

The current `SessionEnvelope` model remains:

- one authoritative root `session_id`;
- optional `agent_id` and `turn_id` attribution;
- durable per-session sequence only for replayable content;
- bounded per-session ring;
- snapshot plus `seq > cursor` replay;
- one stamp-and-route path for local surfaces and Event Hub.

The event source must receive `SessionId` from the selected `SessionHandle`,
not from process state. AppServer validates payload identity where payloads
also carry it.

The backpressure unit is the connection. Outbound channels are per connection
and deliveries are addressed per surface. When a bounded connection channel
overflows, the whole connection is disconnected and all of its surfaces
detach; recovery is reconnect plus per-surface replay from the last cursor.
Per-surface eviction on overflow is rejected for v1: fullness of a shared
channel cannot be attributed to one surface, and the transport is one pipe,
so evicting a single surface would not relieve the consumer. A per-surface
lossy delivery policy for passive observers may be added later without
changing this contract.

## Observability

Every session-scoped span, log line, and metric carries the `session_id`
field, plus `turn_id` where a turn is in scope, following the field
conventions in `common/otel/CLAUDE.md`. Without these fields, concurrent
sessions in one process cannot be attributed during incident analysis.
Adding a session-scoped code path without them is a review defect.

## Shutdown

Process shutdown order:

1. stop accepting connections and new session starts;
2. begin close for every loading/live/closing slot;
3. drain sessions concurrently under the process deadline;
4. flush event-hub egress;
5. stop project service managers;
6. return a non-clean result if the deadline forced abort.

One session's timeout does not prevent clean sessions from flushing. Owner
tasks remain responsible for slot completion even if the initiating request
disconnects.
