# Protocol Scope

This document is normative. Every client request has exactly one scope. There
is no active-session fallback and no optional target that changes meaning at
runtime.

## Target types

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

`SessionTarget` selects persisted metadata or a session operation that does not
require live interactive ownership. It does not prove interactive ownership.

`InteractiveTarget` selects a live interactive capability. The server verifies
that the request connection owns the attached interactive surface and that the
surface points to the supplied session.

`session/archive` uses a typed dual-case target because orphaned sessions
must remain closable without an interactive surface:

```rust
pub enum ArchiveTarget {
    Interactive(InteractiveTarget),
    Orphaned(SessionTarget),
}
```

`Orphaned` succeeds only when the session has no interactive owner (otherwise
`InteractiveOwnerConflict`) and is authorized by the transport boundary. The
caller states which case it claims; the server validates the claim. This is
not an implicit fallback.

Do not use `Option<SessionTarget>`, `Option<InteractiveTarget>`, or a missing
field to mean "current session".

## Connection profile

`initialize` configures one transport connection, not the process and not an
already-live session.

```rust
pub struct ConnectionProfile {
    initialize: Arc<InitializeParams>,
}
```

`ConnectionProfile::try_from(InitializeParams)` validates and normalizes every
initialize field once. It does not silently drop `hooks`, `sdk_mcp_servers`,
`json_schema`, prompt overrides, plan-mode instructions, agents, prompt
suggestions, or agent-progress preferences. Rules:

- one profile is owned by one JSON-RPC/local connection handler;
- initialize succeeds exactly once per connection; a second call is a typed
  `AlreadyInitialized` error;
- session start snapshots construction inputs from the calling profile into
  the new runtime;
- resume from disk builds from persisted/current files plus non-persisted SDK
  inputs in the calling profile;
- resume of an already-live orphan does not rebuild it; it retains its
  construction snapshot and validates that the new connection offers every
  callback id/capability required by the session before rebinding routes;
- an incompatible live reattach fails with `ConnectionProfileMismatch`; it
  never silently combines old hook registrations with unrelated new callback
  identifiers;
- callback identifiers and outbound senders never live in process-global
  `AppServerHostState`;
- disconnect removes the callback route. An orphaned session stays live but
  cannot issue interactive callbacks until a new interactive surface attaches;
- hook/approval/elicitation requests route through AppServer to the current
  interactive surface and fail closed when none exists, with the per-family
  outcomes defined below.

The runtime stores only immutable `SessionCallbackRequirements` derived at
construction. AppServer stores the mutable route from those logical callback
ids to the current interactive surface. Connection writers and callback
implementations remain connection-owned.

### Orphaned-session callback semantics

"Fail closed" is a per-family contract, not a generic error. While a session
has no attached interactive surface:

- `approval/*` pending requests fail with `NoInteractiveSurface`; the engine
  treats the outcome as a denial and the turn continues down the normal
  denial path;
- `input/resolveUserInput` fails with `NoInteractiveSurface`; the prompting
  operation is cancelled;
- `elicitation/*` fails with `NoInteractiveSurface`; the engine treats the
  elicitation as declined;
- SDK hook callback requests fail with `NoInteractiveSurface` and follow the
  existing hook-failure policy for the hook's blocking class.

No pending request parks waiting for a future surface, and orphaning alone
neither aborts nor pauses a running turn. These outcomes are protocol
contract and are asserted by the production isolation suite.

The adapter creates a connection-scoped handler rather than sharing one mutable
handler state across accepted connections:

```rust
pub trait JsonRpcConnectionHandlerFactory: Send + Sync {
    type Handler: JsonRpcRequestHandler;

    fn open(&self, connection: ConnectionKey) -> Self::Handler;
}
```

The connection owner holds and drops the returned handler. Equivalent local
connection construction follows the same ownership model. Exact trait naming
may follow existing adapter conventions, but per-connection ownership is not
optional.

The connection state machine is explicit:

```text
Opened -> Initialized(ConnectionProfile) -> Closed
```

`initialize` is the only transition out of `Opened`. Lifecycle and session
requests before initialization fail with `NotInitialized`. Initialize does not
create a hidden startup session; the client next calls `session/start` or
`session/resume`. Connection close is terminal and releases writer/correlation
state plus all attached surfaces.

## Request classification

The tables cover every current `ClientRequest` method and the breaking
`session/replace` addition. Configuration methods are classified separately
because their required target depends on the requested config layer.

### Connection scoped

| Request | Required data | Behavior |
|---|---|---|
| `initialize` | none before initialization | installs this connection's immutable `ConnectionProfile` |
| `control/keepAlive` | none | acknowledges this connection |
| `control/cancelRequest` | connection-issued `request_id` | cancels only a pending request owned by this connection |

### Session creation and attachment

| Request | Required data | Behavior |
|---|---|---|
| `session/start` | start options | mints a session, snapshots the connection profile, attaches a new interactive surface |
| `session/resume` | `SessionTarget` | loads/rejoins the session and attaches an interactive surface using this connection profile |
| `session/replace` | source `InteractiveTarget` plus typed fresh/resume destination | atomically repoints the source surface; never inferred from start/resume |
| `session/subscribe` | `SessionTarget` plus replay cursor | attaches a passive surface |
| `session/archive` | `ArchiveTarget` (interactive, or orphaned when no interactive owner exists) | closes the selected live runtime; JSONL remains |

### Process/catalog scoped

| Request | Required data | Behavior |
|---|---|---|
| `session/list` | query/pagination only | lists persisted plus live sessions |

No live runtime is selected for these operations.

### Persisted or non-interactive session scoped

| Request | Required data | Behavior |
|---|---|---|
| `session/read` | `SessionTarget` | reads live overlay plus persisted transcript |
| `session/turns/list` | `SessionTarget` | reads turn summaries/history |
| `session/rename` | `SessionTarget` plus name | updates persisted metadata and live projection if present |
| `session/toggleTag` | `SessionTarget` plus tag | updates persisted metadata and live projection if present |
| `session/cost` | `SessionTarget` | reads targeted accounting |
| `session/status` | `SessionTarget` | reads targeted live/persisted status |
| `task/list` | `SessionTarget` | reads tasks owned by the selected session |
| `task/detail` | `SessionTarget` plus task id | reads a task owned by the selected session |
| `context/usage` | `SessionTarget` | reads history/app-state usage from the selected session |
| `mcp/status` | `SessionTarget` | reads the selected session's MCP manager and registration reports |

Local transport authentication is the v1 authorization boundary for these
read/catalog operations. A future multi-user server may add an authorization
capability without changing session identity.

### Interactive session scoped

| Request family | Requests |
|---|---|
| Turn control | `turn/start`, `turn/interrupt` |
| Client-resolved interaction | `approval/resolve`, `input/resolveUserInput`, `elicitation/resolve` |
| Runtime model/config | `control/setModel`, `control/setModelRole`, `control/setPermissionMode`, `control/setThinking`, `control/setAgentColor`, `config/applyFlags` |
| Permission/task mutation | `control/applyPermissionUpdate`, `control/resetSessionPermissionRules`, `control/stopTask`, `control/backgroundAllTasks`, `agent/interruptCurrentWork` |
| Workspace mutation | `control/rewindFiles`, `control/updateEnv` |
| MCP mutation | `mcp/setServers`, `mcp/reconnect`, `mcp/toggle` |
| Runtime reload | `plugin/reload`, `hook/reload` |

Every params DTO in this table carries `InteractiveTarget`. Resolve operations
also carry their `request_id`; AppServer validates that the request id, target,
surface, and connection match the pending server request.

## Configuration requests

Configuration scope is encoded as a required enum because one method spans
different owners today.

```rust
pub enum ConfigReadTarget {
    Process,
    Session(SessionTarget),
}

pub enum ConfigWriteTarget {
    User,
    Project(InteractiveTarget),
    Local(InteractiveTarget),
}
```

| Request | Required data |
|---|---|
| `config/read` | `ConfigReadTarget` |
| `config/value/write` | `ConfigWriteTarget`, key, and value |

- process read returns process/user/policy/flag/env inputs and does not pretend
  to be an effective session fold;
- session read resolves project/local roots from the selected session cwd and
  returns its effective layered configuration;
- user write targets the user settings file;
- project/local writes resolve their paths from the selected interactive
  session and never from process cwd;
- stringly typed `scope: Option<String>` is removed.

## Server-initiated requests

Approval, user-input, elicitation, SDK hook callback, and MCP route requests use
the existing AppServer pending-request routing model:

```text
SessionHandle
  -> AppServer.route_server_request(session_id, capability, turn_id, payload)
  -> current interactive SurfaceId
  -> owning connection writer
  -> reply validated against connection + surface + session + request id
```

Do not keep a second process-global pending map for the same request family.
Connection-level JSON-RPC id correlation remains in the connection owner; the
domain request ownership remains in AppServer.

## Client API rules

- `RemoteSessionClient` and `LocalSessionClient` inject
  `InteractiveTarget` into mutations automatically;
- persisted/non-interactive methods inject `SessionTarget`;
- passive handles expose read/subscription methods but no interactive
  mutations;
- connection clients expose only connection/process/catalog operations;
- low-level unscoped turn/runtime/MCP methods are not public;
- replace and interactive archive consume the interactive handle; orphaned
  archive is a connection-level operation that validates no interactive owner
  exists;
- no compatibility overload preserves target-less requests.

## Exhaustiveness gate

`ClientRequest` dispatch remains an exhaustive match. A request may be added
only when this document classifies its scope and its params type makes that
scope explicit. The mapping is one exhaustive function in `common/types`:

```rust
pub fn request_scope(method: ClientRequestMethod) -> RequestScope
```

with no wildcard arm, used by server dispatch as the source of truth. Adding
a request variant without classifying it fails compilation; no separate CI
assertion is needed beyond this function existing and dispatch using it.
