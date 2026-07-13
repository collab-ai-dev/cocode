# Protocol Scope V2

This document is normative. Every request has exactly one scope and all live
mutations carry explicit authority. The v2 protocol intentionally removes
`session/archive` and introduces separate close and delete operations.

## Identity and authority types

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionCloseTarget {
    Interactive { target: InteractiveTarget },
    Orphaned { target: SessionTarget },
}
```

`SessionTarget` selects durable/read state. It is not interactive authority.

`InteractiveTarget` proves authority only after AppServer validates that the
request connection owns the interactive surface, the surface points to the
session, and the registry slot is Live.

`SessionCloseTarget::Orphaned` is explicit authority for cleaning up a live
session with no interactive owner. It fails with
`InteractiveOwnerConflict` when an owner exists. There is no fallback from an
invalid interactive target to orphan authority.

Never use an absent/optional target to mean current session.

## Connection profile

The connection state machine is:

```text
Opened -> Initialized(ConnectionProfile) -> Closed
```

Rules:

- every accepted connection owns a fresh handler/profile slot;
- initialize is accepted exactly once;
- every non-initialize request before initialization fails with
  `NotInitialized`, except transport-level close;
- initialize creates no session and does not reserve a registry slot;
- the profile is immutable after validation;
- start snapshots all construction inputs from the calling profile;
- resume of durable state combines persisted/current files with the calling
  profile's non-persisted inputs;
- live-orphan rebind validates immutable callback requirements before routing
  changes;
- connection writers and JSON-RPC correlation are connection-owned;
- callback domain ownership is AppServer-owned and includes connection,
  surface, session, turn, and request id where applicable;
- disconnect removes callback routes and surfaces but does not cancel an
  otherwise valid running turn.

## Request scopes

### Connection scoped

| Request | Required data | Behavior |
|---|---|---|
| `initialize` | initialize params | freezes one `ConnectionProfile`; creates no session |
| `control/keepAlive` | none | acknowledges this connection |
| `control/cancelRequest` | connection-owned request id | cancels only that connection's pending request |

### Session lifecycle

| Request | Required data | Behavior |
|---|---|---|
| `session/start` | start options, optional process-local `initial_messages` | mints/builds one session, hydrates any supplied initial history, and attaches an interactive surface |
| `session/resume` | `SessionTarget` | loads/rebinds one identity and attaches an interactive surface |
| `session/replace` | source `InteractiveTarget` plus typed destination (`fresh`, `resume`, or `clear`) | atomically replaces one surface/session identity |
| `session/subscribe` | `SessionTarget` plus replay cursor | attaches a passive surface |
| `session/close` | `SessionCloseTarget` | closes runtime, preserves transcript |
| `session/delete` | `SessionTarget` | deletes durable state only when no live/loading/closing slot exists |

There is no `session/archive` in v2.

### Process/catalog scoped

| Request | Required data | Behavior |
|---|---|---|
| `session/list` | query/pagination | lists durable plus live sessions |

### Durable or non-interactive session scoped

| Request | Required data |
|---|---|
| `session/read` | `SessionTarget` |
| `session/turns/list` | `SessionTarget` |
| `session/rename` | `SessionTarget` plus name |
| `session/toggleTag` | `SessionTarget` plus tag |
| `session/cost` | `SessionTarget` |
| `session/status` | `SessionTarget` |
| `task/list` | `SessionTarget` |
| `task/detail` | `SessionTarget` plus task id |
| `context/usage` | `SessionTarget` |
| `mcp/status` | `SessionTarget` |

When the session is live, AppServer may provide a live overlay. Otherwise the
operation reads durable state where meaningful. A read never infers a process
current session.

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

Every params DTO in this table contains `InteractiveTarget`. Server-request
replies also contain request id and are accepted only when all authority fields
match the pending request.

## Configuration scope

Configuration keeps explicit typed scope:

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

- process read returns process/user/policy/flag/env inputs, not a fake effective
  session fold;
- session read resolves project/local layers from the selected session;
- user write targets user settings;
- project/local writes derive paths from the selected interactive session;
- stringly typed or optional scope fields are forbidden.

## Turn completion

`turn/start` returns a `TurnId`. `TurnEnded` is the authoritative per-turn
terminal result and contains the data required by non-interactive surfaces:

```rust
pub struct TurnEndedParams {
    pub identity: TurnIdentity,
    pub outcome: TurnOutcome,
    pub result: Option<String>,
    pub structured_output: Option<Value>,
    pub usage: TokenUsage,
    pub model_usage: HashMap<String, SessionModelUsage>,
    pub duration_ms: i64,
    pub duration_api_ms: i64,
    pub permission_denials: Vec<PermissionDenialInfo>,
    pub errors: Vec<ErrorPayload>,
}
```

Exact field factoring may reuse existing result DTOs, but the contract is:

- the event is emitted exactly once;
- it follows engine completion, event-forwarder drain, history commit, and
  accounting commit;
- no surface polls an internal projection or invents a fallback result;
- session close waits for the terminal turn event before emitting final
  `SessionResult`.

## Close protocol

`session/close` is runtime lifecycle only.

Interactive close:

1. validate connection/surface/session/live slot;
2. transition the slot to Closing;
3. run the deterministic close cascade;
4. emit final `SessionResult` and lifecycle effects;
5. remove the slot and detach surfaces;
6. return success only after no session task can emit another event.

Orphan close:

1. prove under registry/routing lock that no interactive owner exists;
2. transition to Closing atomically with that proof;
3. run the same close cascade.

Close never calls transcript delete.

## Delete protocol

`session/delete` operates only on durable storage:

1. verify authorization for the `SessionTarget`;
2. verify registry state is Missing, not Loading/Live/Closing;
3. delete transcript and explicitly documented auxiliary artifacts;
4. return storage errors to the client;
5. do not emit a session-catalog refresh notification in this phase.

There is no process-wide session-catalog subscription protocol today:
`session/list` is an explicit request/response read, while live close already
emits AppServer lifecycle effects for attached surfaces. A client that caches
`session/list` invalidates that cache after its own successful `session/delete`
response. If a future surface needs passive catalog updates, add a dedicated
catalog subscription/notification instead of overloading live session lifecycle
events.

Delete does not create, resume, close, or attach a session.

## Server-initiated requests

```text
LiveSession
  -> AppServer.route_server_request(session, turn, capability, payload)
  -> current interactive SurfaceId
  -> owning connection writer
  -> reply validated against connection + surface + session + turn + request id
```

While orphaned:

- approvals fail with `NoInteractiveSurface` and are treated as denial;
- user input fails and the prompting operation is cancelled;
- elicitation fails and is treated as declined;
- SDK hook callbacks follow their existing blocking-class failure policy;
- no request parks waiting for a future surface;
- orphaning alone does not cancel the running turn.

## Client API

- connection clients expose initialize, lifecycle, process, and catalog
  operations;
- interactive clients inject `InteractiveTarget` automatically;
- passive clients expose subscription/read operations only;
- close consumes an interactive client;
- replace consumes the source interactive client;
- orphan close and delete are connection-level operations with explicit target;
- low-level unscoped mutation helpers are not public;
- local and remote clients expose equivalent authority boundaries.

## Exhaustiveness

`ClientRequestMethod -> RequestScope` remains one exhaustive no-wildcard match
in `coco-types`. Host protocol dispatch is also one exhaustive no-wildcard
match after AppServer has resolved the scope/authority.

Adding a request requires:

1. a scope classification;
2. a params DTO whose target makes the scope unambiguous;
3. local and remote typed client support where applicable;
4. behavioral authority tests;
5. an update to this document.
