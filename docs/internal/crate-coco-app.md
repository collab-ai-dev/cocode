# Application Crates (`app/`)

Current application architecture. The app layer is split by lifecycle scope,
AppServer protocol direction, execution ownership, and surface presentation.
There is no global `coco-state` crate: backend state is session-owned and each
surface keeps its own projection.

## Dependency Direction

An arrow means "depends on".

```text
coco-cli
  |-> coco-agent-host
  |-> coco-tui
  `-> coco-app-server-client

coco-agent-host
  |-> coco-app-runtime
  |-> coco-query -> coco-session
  |-> coco-app-server
  |-> coco-app-server-client
  `-> coco-tui

coco-app-server-client -> coco-app-server-transport
coco-app-server        -> coco-app-server-transport

coco-app-runtime, coco-session, coco-query, and coco-tui
  -> common/core/services crates
```

`coco-app-server` and `coco-app-server-client` are siblings. The remote client
has no normal dependency on the server implementation. Agent-host owns the
in-process bridge because it is the layer that intentionally composes both.

## Crate Responsibilities

| Crate | Responsibility |
|------|----------------|
| `coco-cli` | `coco` binary, clap schema, subcommand/process policy, listener startup, signals, and interactive TUI loop. |
| `coco-agent-host` | Agent-session composition, `SessionRuntime`, local AppServer client facade, SDK/headless host operations, runtime integrations, and protocol handlers. |
| `coco-app-runtime` | Transport-independent process/project scopes, workspace resolution, project cache, and session-bootstrap contracts. |
| `coco-app-server` | Multi-session lifecycle registry, connection/surface routing, replay, server requests, local adapter, and JSON-RPC adapter. It never constructs an agent runtime. |
| `coco-app-server-client` | Remote JSON-RPC client, request correlation, remote session handles, per-surface demux, and remote connection tasks. |
| `coco-app-server-transport` | JSON-RPC frames and NDJSON/socket/named-pipe framing primitives. No session or domain behavior. |
| `coco-query` | Multi-turn model/tool loop, compaction, budget, command queue, recovery, and turn protocol events. |
| `coco-session` | JSONL transcript persistence, metadata folding, recovery, title generation, and process session registration. |
| `coco-tui` | TEA input/update/render model and terminal presentation. It consumes typed events and does not construct runtimes or call QueryEngine directly. |

## State Ownership

There is deliberately no single global `AppState`.

| State | Owner |
|------|-------|
| Process/project registry and workspace roots | `coco-app-runtime::{ProcessRuntime, ProjectRegistry, SessionWorkspace}` |
| Session aggregate and lifecycle resources | `coco-agent-host::session_runtime::SessionRuntime` |
| Cross-turn permission/tool/plan/task reminder state | `Arc<RwLock<coco_types::ToolAppState>>` inside `SessionRuntime` |
| Model/runtime configuration | `RuntimeConfig`, `QueryEngineConfig`, and session role overrides |
| Transcript and durable session metadata | `coco-session` stores owned by `SessionRuntime` |
| Turn history, usage, command queue, task runtime, MCP, hooks | Dedicated `SessionRuntime` resource containers |
| AppServer lifecycle and surface ownership | `coco-app-server` registry/routing state |
| Terminal read model and UI interaction state | `coco_tui::state::AppState { session, ui, running, clock }` |

The removed `coco-state::AppState` was never instantiated by production code.
Its live responsibilities already belonged to the owners above; parity-only
fields without readers or writers were deleted rather than migrated.

## Boundary Rules

- `coco-cli` maps clap values once into `AgentHostOptions`. Agent-host must not
  depend on clap or process-global CLI types.
- `coco-agent-host` is the application-composition fan-in. A large dependency
  list is expected there; domain rules and reusable wire behavior remain below.
- `coco-app-server-client` must not import `coco_app_server` in production.
  Cross-crate protocol integration tests belong to agent-host, which owns both
  dependencies.
- `coco-app-server-transport` must not depend on server, client, host, session,
  or transcript crates.
- `coco-query` must not depend on CLI, agent-host, AppServer, or TUI.
- `coco-tui` receives `CoreEvent`/protocol DTOs through `coco-types`; it does
  not depend on query or agent-host implementation types.
- In-process AppServer handles live in `coco-agent-host::local_client`, where
  the server implementation and application operations are intentionally joined.

## Surface Flow

```text
TUI input / headless prompt / SDK JSON-RPC
  -> agent-host application handler
  -> AppServer lifecycle and surface routing
  -> SessionHandle / SessionRuntime
  -> QueryEngine turn
  -> session-scoped CoreEvent
  -> AppServer stamped envelope
  -> local TUI/headless projection or remote SDK client
```

The shared unit is an application turn/session use case, not a generic runner
trait. TUI, headless, and SDK retain different input, transport, and output
policies while reusing the same host operations.
