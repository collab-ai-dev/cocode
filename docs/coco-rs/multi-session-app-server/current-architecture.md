# Current Architecture

This document describes the production tree after the breaking refactor on
2026-07-11. It is descriptive, not aspirational.

## Runtime composition

```text
coco-cli
  | process entry, arguments, listeners, signal/shutdown policy
  |
  +-- ProcessRuntime
  |     `-- ProjectRegistry -> Arc<ProjectServices>
  |
  +-- AppServer<AppSessionHandle>
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
  +-- AppServer host handlers and SDK/local bridges
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

`SessionHandle` adds an immutable session-id snapshot and focused capability
methods. Its `Arc<SessionRuntime>` is private: there is no `Deref` or public
`runtime()` escape hatch.

## State ownership

There is no useful process-global `AppState`.

| State | Current owner | Scope |
|---|---|---|
| TUI rendering/input state | `coco_tui::state::AppState` | one TUI surface |
| tool/permission/reminder/task-panel state | `ToolAppState` inside `SessionRuntime` | one root session |
| transcript and engine resources | `SessionRuntime` | one root session |
| live slot lifecycle | `LiveSessionRegistry` | process registry, keyed by `SessionId` |
| connection/surface/replay routing | `RoutingState` | AppServer process |
| turn ids, aggregate accounting, active-turn tasks/cancellation | `SessionTurnCoordinator` inside `SessionRuntime` | one root session |
| AppServer bootstrap, factory, activity and durable-sequence projections | `AppServerHostState` | process services, keyed projections only |
| immutable initialize inputs | per-connection `ConnectionProfile` | one accepted connection |
| transport writer and outbound queues | connection runner / adapter | one accepted connection |
| pending callback correlation | AppServer + connection adapter, keyed by request/session/surface/connection | one originating request |
| MCP manager and registration reports | `SessionRuntime` integration resources | one root session |

`AppServerHostState` retains process services and cheap activity/durable-sequence
projections, but it owns no selectable runtime, history, turn counter,
accounting, active turn, MCP manager, file history, reload slot, connection
writer, or pending callback map.

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

When resume observes `Closing`, the host waits for its completion with a
bounded timeout and retries the load. Orphan archive proves that no interactive
owner exists while resolving the request runtime, before the archive handler
can cancel a turn or mutate activity. The subsequent orphan close revalidates
routing and moves the registry slot to `Closing` under the canonical
registry-then-routing lock order.

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

Interactive request DTOs carry both `session_id` and `surface_id`. AppServer
validates session, surface, role, and connection ownership before returning a
live handle. Persisted-only requests carry `SessionTarget`; archive uses the
typed interactive-or-orphan `ArchiveTarget`.

Every accepted transport calls the JSON handler factory and receives a fresh
handler with an empty initialize cell. Exactly one `initialize` freezes its
`ConnectionProfile`; local in-process clients use an explicitly preinitialized
handler. Writers and synchronous reply correlation remain connection-owned.

## Request dispatch today

`request_scope` exhaustively classifies every `ClientRequest`. The handler
adapter resolves explicit targets before dispatch and constructs a
`SessionRequestContext` whose id and `SessionHandle` came from the same
AppServer validation. Interactive handlers cannot fall back to a sole surface,
process runtime, or sole handoff.

`SessionTurnExecutor` receives that selected handle on every call. Shortcuts,
MCP, rewind, approvals, sandbox changes, hooks, config, reload, active-turn
interrupt, and shutdown use focused capabilities on the same handle.

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

Event production takes identity from the selected handle and active turn.
Server-initiated requests carry the originating session id; typed replies are
accepted only from their owning connection, surface, session, and request id.

## Test coverage today

Package tests cover target classification, connection isolation, registry
lifecycle, replacement, closing retry, orphan archive, callback correlation,
slow-consumer disconnect, replay, client authority injection, session-owned
capabilities, and multi-session shutdown. The release-blocking host integration
suite uses public clients and production handlers for multi-session authority,
cross-connection rejection, orphan lifecycle, and event/replay identity.

The integration suite contains sixteen production-handler scenarios using real
`SessionRuntime`s. It covers concurrent A/B turns and targeted interruption,
cwd plus project/local config isolation, independent initialized profiles,
per-session tool catalogs and real SDK-hosted MCP handshakes, independent
history/read/turn-list/rewind/control state, connection/surface/session/request
callback correlation, compatible and incompatible orphan resume, orphan
fail-closed behavior, close/replacement reload-supervisor lifetime, replay
session/turn identity, slow-consumer recovery, orphan archive, and concurrent
multi-runtime shutdown. Each package-H scenario has an explicit deadline;
focused AppServer tests additionally cover lifecycle races and surface rebind.

The final 2026-07-11 release-validation snapshot was:

- all seam checks and workspace clippy passed with every feature and test
  target enabled;
- all 13,611 executed workspace tests passed under nextest, with four tests
  skipped by existing configuration;
- all 88 TUI runner tests passed;
- focused totals were 309 agent-host, 89 app-server, 34 app-server-client, and
  300 types tests, all passing;
- all schema and Python protocol generation checks passed, and the Python SDK
  suite passed 107 tests with ten environment-gated skips;
- the sixteen production-handler integration tests passed after every
  concurrent and lifecycle scenario received an overall bounded timeout.
