# Current Architecture

This document describes the production tree as reviewed on 2026-07-11. It is
descriptive, not aspirational.

## Runtime composition

```text
coco-cli
  | process entry, arguments, listeners, signal/shutdown policy
  |
  +-- ProcessRuntime
  |     `-- ProjectRegistry -> Arc<ProjectServices>
  |
  +-- AppServer<LocalAppSessionHandle>
  |     +-- LiveSessionRegistry
  |     `-- RoutingState
  |
  +-- SessionRuntimeFactory
  |     `-- one SessionHandle per constructed session
  |
  `-- SDK/TUI/headless surface policy

coco-agent-host
  +-- SessionRuntime / SessionHandle
  +-- local typed client
  +-- SDK handlers and AppServer bridge
  +-- QueryEngine turn execution
  `-- MCP/hooks/tasks/persistence application integration
```

SDK mode constructs one shared `AppServer` with configured
`server.max_sessions` (default 32). The stdio connection and optional Unix,
named-pipe, and WebSocket listeners share that registry and routing state.

TUI and headless construct `AppServerLocalBridge`, which deliberately fixes
`max_sessions` to one. They still use AppServer lifecycle code but retain a
single-session product surface.

## Crate boundaries

```text
coco-types <-------------------------+
   ^                                 |
   |                                 |
coco-app-server-transport <------+   |
   ^                             |   |
   |                             |   |
coco-app-server          coco-app-server-client
   ^                             ^
   |                             |
   +--------- coco-agent-host ---+
                  ^
                  |
             coco-app-runtime
                  ^
                  |
               coco-cli
```

The diagram omits the many engine/service dependencies of
`coco-agent-host`. Important rules:

- server and client share canonical DTOs through `coco-types` and frame/I/O
  primitives through `coco-app-server-transport`;
- the remote client does not depend on the server implementation;
- AppServer is generic over its live handle and does not depend on
  `SessionRuntime`;
- `coco-agent-host` is the composition layer that knows both AppServer and the
  concrete session runtime;
- query/core/service crates do not depend on AppServer crates.

These rules are clear, useful, and should not be reversed.

## Process, project, and session scopes

### ProcessRuntime

`ProcessRuntime` currently owns:

- the process-lifetime `ProjectRegistryManager`;
- the one-shot application of the project-service idle TTL (the eviction
  itself runs as a periodic background sweep owned by the registry manager).

It is intentionally small. The old design listed model registries, auth,
transports, session storage, and catalogs as fields it ought to own, but those
are not present today. A process scope should grow only when it becomes the
actual lifecycle owner of a resource; it should not become a manifest of
everything that happens to live for a long time.

### ProjectRegistry and ProjectServices

`ProjectRegistry` is keyed by `(config_home, project_root)`. It:

- reuses a published `Arc<ProjectServices>` for the same fresh key;
- separates different project roots;
- replaces stale catalog/config entries for later sessions;
- keeps entries alive while sessions hold an `Arc`;
- evicts unattached entries after an idle grace period.

`ProjectServices` currently owns:

- `ProjectConfigSnapshot`;
- `ProjectCatalogSnapshot`, including project/plugin-derived commands, skills,
  hooks, output styles, agents, and MCP definitions.

It does not own live LSP, retrieval, context discovery, ignore, or MCP process
instances. The current cold-load algorithm may perform duplicate I/O for two
same-key racers but publishes and returns one winning `Arc`. This is
publication deduplication, not strict single-flight execution.

### SessionRuntime

`SessionRuntime` is one resource owner per constructed root session. It owns
focused resource groups for:

- execution, tools, model runtimes, and engine configuration;
- history, transcript persistence, usage, and cross-turn `ToolAppState`;
- session cwd/project anchors and the folded `RuntimeConfig`;
- hooks, commands, skills, agents, permissions, sandbox, tasks, memory, MCP,
  LSP, file history, and shutdown state.

The object is shared behind `Arc`. Independent fields use `Mutex`, `RwLock`,
atomics, watch channels, cancellation tokens, or immutable `Arc` snapshots as
appropriate. It is not driven by one actor mailbox.

`SessionHandle` adds an immutable session-id snapshot but currently exposes the
underlying runtime through `runtime()` and `Deref`. Consequently it is a
convenient shared pointer, not a strict capability boundary.

## State ownership

There is no useful process-global `AppState`.

| State | Current owner | Scope |
|---|---|---|
| TUI rendering/input state | `coco_tui::state::AppState` | one TUI surface |
| tool/permission/reminder/task-panel state | `ToolAppState` inside `SessionRuntime` | one root session |
| transcript and engine resources | `SessionRuntime` | one root session |
| live slot lifecycle | `LiveSessionRegistry` | process registry, keyed by `SessionId` |
| connection/surface/replay routing | `RoutingState` | AppServer process |
| SDK handoff/accounting/active-turn maps | `SdkServerState` | keyed by `SessionId` |
| installed runtime/MCP/file-history/reload slots | `SdkServerState` | process singleton, problematic |
| initialize metadata and SDK construction inputs | `InitializeState` in shared `SdkServerState` | process singleton, but semantically per connection |
| transport writer and outbound queue | `ConnectionState` in shared `SdkServerState` | process singleton, but semantically per connection |
| pending callback/request maps | `PendingClientRequestState` and `ServerRequestState` | shared, with incomplete connection/surface correlation |
| MCP registration reports | `McpRegistrationState` keyed only by server name | process singleton, but semantically per session |

The singleton and incompletely keyed rows are ownership defects. A keyed
handoff does not make execution session-safe when engine services are selected
from process-singleton slots. Likewise, accepting several transports is not
connection-safe when `initialize` can overwrite construction inputs or a
server request can select a shared writer without its originating connection.

## AppServer registry and lifecycle

`LiveSessionRegistry<H>` stores:

```text
Loading(completion)
Live(H)
Closing(H, completion)
```

Load, close, replace, and shutdown work runs in spawned owner tasks. Completion
signals are observations of that work, so caller cancellation does not wedge a
slot. Loading, live, and closing entries count against `max_sessions`; replace
temporarily reserves one additional destination slot.

The combined AppServer commit path holds registry then routing locks, performs
no await while locked, validates before mutation, and atomically updates live
slots plus surface routing. This is an appropriate use of synchronous locks:
critical sections are short and contain no asynchronous work.

Current gap: when host loading observes `Closing`, the close completion is
discarded and an internal error is returned. Reopen-after-close is therefore
not implemented above the registry.

## Connections and surfaces

A connection is a transport relationship identified by private
`ConnectionKey`. A surface is a public attachment identified by `SurfaceId`
and points to one `SessionId`.

AppServer supports:

- multiple surfaces per connection;
- surfaces for different sessions on one connection;
- at most one interactive owner per session;
- bounded passive observers;
- per-surface notification preferences and capabilities;
- replace/archive lifecycle effects;
- per-connection outbound channels carrying per-surface-addressed event and
  server-request deliveries.

This routing model is internally coherent. The defect is at the request DTO
boundary: session commands do not identify which interactive surface issued
them.

The listener composition has a second boundary defect. Accepted connections
share the same `AppServerSdkHandler` and `SdkServerState`; `InitializeState`
therefore represents the latest initialize call rather than one immutable
connection profile. `ConnectionState` also contains a single transport/writer
slot. The target must create one connection handler and callback-correlation
owner for every accepted connection while retaining the shared AppServer
registry and routing state.

## Request dispatch today

For a session-scoped request without an explicit id, the handler tries:

1. the sole interactive session attached to the request connection;
2. the process-installed `SessionRuntime`;
3. a sole keyed SDK handoff.

This fallback chain preserves old single-session behavior but is ambiguous by
construction. When one connection owns two interactive surfaces, step 1
returns no result. When two connections own distinct sessions, step 1 can
select the correct handoff, but the production runner still reads the runtime
from step 2.

Runtime-control handlers that call `HandlerContext::resolve_runtime` are
better: they prefer the registry-resolved `scoped_runtime`. That improvement
is incomplete because turn execution, shortcuts, MCP, rewind, approvals, and
some session handlers still read process slots directly.

## Event flow

```text
QueryEngine CoreEvent
  -> OutboundMessage(session_id, event)
  -> SessionEnvelope::stamp
  -> per-session SessionSeqAllocator
  -> AppServer routing + retention ring
  +-> attached surfaces
  `-> optional Event Hub connector
```

Durable event classes receive a per-session sequence and enter the replay
ring. Ephemeral stream/TUI deltas stay live-only. Slow consumers are isolated
by bounded connection channels and disconnected rather than blocking engine
producers. Outbound channels are per connection, so overflow disconnects the
whole connection and detaches all of its surfaces; recovery is reconnect plus
replay.

This design is substantially landed. Event correctness still depends on turn
execution selecting the right runtime and stamping with the selected session
id; routing cannot repair an event produced from mixed session state.

## Test coverage today

Existing tests prove the individual registry, lifecycle, routing, replay,
surface, client-demux, project-cache, and shutdown mechanisms.

They do not prove the product invariant. There is no production-path test that
uses public client handles to:

1. create two real runtimes;
2. retain both interactive surfaces;
3. run turns concurrently through the same or different connections;
4. assert each engine used its own cwd/config/tools/history/MCP state;
5. assert controls and events stayed isolated.

The absence of this test explains why correct routing components coexist with
an incorrect end-to-end runtime selection path.
