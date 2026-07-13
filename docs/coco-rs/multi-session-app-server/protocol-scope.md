# Protocol Scope V2

This document is normative. Every request has exactly one scope and all live
mutations carry explicit authority. The v2 protocol intentionally removes
`session/archive` and introduces separate close and delete operations.

## Change control

Wire changes are limited to Workstream 1 and must be required by an explicit
CS-1 through CS-4 gate. CS-1 owns the start/initialize field corrections; CS-3
may add the already-specified stable post-commit/timeout error data. At the end
of Workstream 1, this wire contract and generated SDK/schema artifacts are
frozen for the surface-boundary and internal-cleanup workstreams.

- a process-local construction need never adds a serialized remote field;
- an accepted field has a validation/consumption site or is removed/rejected;
- surface directory moves and agent-host module cleanup cannot change DTOs,
  request scopes, error semantics, or completion meanings;
- a newly discovered convenience or cleanup does not reopen the protocol;
- before Workstream 1 closes, a change requires a failing test tied to its CS
  gate; after it closes, reopening requires evidence that a frozen invariant
  cannot be satisfied by the contract.

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

### Profile versus session policy

`initialize` contains only connection-lifetime capabilities and resources:
client identity/capabilities, callback and hook support, client-hosted MCP
servers, supplied agents, and prompt-suggestion/progress capabilities. It does
not carry model, permission, prompt, structured-output, budget, cwd, or other
per-session execution policy.

`session/start` owns per-session execution policy. The accepted fields are the
requested cwd, model, permission mode, maximum turns, USD budget,
replacement and appended system prompts, JSON schema, and plan-mode
instructions. Each accepted field is validated and consumed during the one
session fold. A field may not be accepted as a documented no-op.

`SessionStartParams.initial_prompt` is removed. A user prompt is a separate
`turn/start` after the session and its interactive authority exist. Duplicate
model/prompt/schema fields are removed from `initialize` rather than resolved
through precedence rules. Agent-definition behavior is a separate contract.

## Start contract

The serialized remote `SessionStartParams` contains neither `session_id` nor
`initial_messages`. The server mints the `SessionId`; a remote caller cannot
guess an existing identity and use start as resume, attach, or mutation.
Legacy/unknown identity and history fields are rejected as invalid params, not
silently ignored.

Start is valid only when the minted registry slot is Missing. Loading, Live,
and Closing are stable conflicts, checked before a runtime is exposed or any
configuration/history mutation can occur. Build, promotion, and interactive
surface attachment are one lifecycle-owner operation. Failure leaves no live
runtime and no routing entry.

Process-local tests or embeddings that genuinely need a chosen fresh identity
or prebuilt history use a non-serialized internal input:

```rust
pub(crate) struct LocalStartSeed {
    pub session_id: SessionId,
    pub initial_messages: Vec<Message>,
}
```

This seam still requires a Missing slot and runs through the same lifecycle
owner. It is not part of JSON-RPC, schemas, generated SDKs, or the public
remote client. Production resume/history hydration uses `session/resume`.

`session/start` and `session/resume` success results both carry the session
identity and require `surface_id`; clients must not infer or recover a missing
surface through a compatibility fallback.

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
| `session/start` | per-session execution policy; no client id/history/user prompt | server mints/builds one new session and atomically attaches an interactive surface |
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

## Replace protocol

`session/replace` consumes the source interactive authority and has one owner
for all destination variants, including an already-live destination.

- pre-commit failure leaves the source attached and returns an error;
- clean success means destination commit and source close both completed;
- post-commit source-close failure cannot roll back routing and returns typed
  `CommittedCloseFailed` data containing the committed destination target;
- caller cancellation does not cancel the owner operation;
- panic/timeout resolves all Loading/Closing state and completion waiters.

## Close protocol

`session/close` is runtime lifecycle only.

Interactive close:

1. validate connection/surface/session/live slot;
2. transition the slot to Closing;
3. run the deterministic close cascade;
4. emit final `SessionResult` and lifecycle effects;
5. remove the slot and detach surfaces;
6. complete the bounded local process-egress handoff (not a remote network ack)
   and retire Hub membership;
7. return success only after no session task can emit another event.

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
- successful start/resume returns a required interactive surface id;
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

The protocol test suite also maintains an accepted-field audit: every field in
every request DTO must have a production validation/consumption site or an
explicit rejection test. Serialization plus code generation is not evidence
that a field is implemented.
