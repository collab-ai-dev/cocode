# Remediation Plan

This document is the executable implementation plan. It intentionally breaks
the current protocol and does not preserve target-less request compatibility.
The normative architecture is in [target-architecture.md](target-architecture.md),
and every request scope is defined in [protocol-scope.md](protocol-scope.md).

## Execution contract

- Land only green commits. Characterization cases may be demonstrated as
  failing locally, but the test and its fix land in the same work package.
- Batch related edits. Run targeted tests once at a work-package boundary,
  `just quick-check` at a phase boundary, and `just pre-commit` exactly once
  immediately before the final commit.
- Protocol DTOs, server dispatch, and local/remote clients change atomically.
  There is no compatibility adapter or optional target transition period.
- Move every capability and all consumers before deleting its old field.
- Keep exhaustive matches and typed scope enums; do not add an implicit
  current-session fallback elsewhere.
- Check in the package H scenario suite as `#[ignore]` skeletons at the start
  of the batch so packages A through G are built against concrete target
  assertions; un-ignore each scenario as it becomes real.
- Inside the A-D atomic batch, stack one commit per work package on the
  working branch so review and bisection stay tractable; the batch still
  lands as one green change.

## Dependency order

```text
request scope inventory
  -> canonical target DTOs + connection profile
  -> per-connection handler + AppServer target validation
  -> local/remote clients inject targets
  -> handler receives registry-selected SessionHandle
  -> turn executor uses only that handle
  -> session capabilities move behind that handle
  -> duplicate process fields are deleted
  -> lifecycle edges are fixed
  -> production-path isolation gate
  -> API hardening and optional improvements
```

The connection handler must exist before `InitializeState` or
`ConnectionState` is removed. Registry-selected runtime execution must exist
before `SessionRuntimeState` is removed. Session MCP, file-history, and reload
owners must exist before their process slots are removed.

## Work package A: lock the scope contract

### Changes

1. Add `SessionTarget`, `InteractiveTarget`, and `ArchiveTarget` to
   `common/types/src/client_request.rs`.
2. Change every params DTO according to `protocol-scope.md`, including typed
   config read/write targets.
3. Add the exhaustive `request_scope(ClientRequestMethod) -> RequestScope`
   function (no wildcard arm) classifying every method as connection,
   lifecycle, process/catalog, session-read, or interactive, and make server
   dispatch consume it.
4. Delete optional/string scope fields and target-less request forms.
5. Define connection initialize data as `ConnectionProfile`; initialize is
   accepted exactly once for each connection.

### Primary files

- `common/types/src/client_request.rs`
- `common/types/src/client_request.test.rs`
- `app/server/src/lib.rs`

### Tests and gate

- serialization round trips for both target types and config scope enums;
- exhaustive method-to-scope mapping test;
- malformed or missing targets fail deserialization;
- a newly added request method cannot compile without a scope classification.

Do not merge package A alone. It is one atomic workspace change with packages
B through D because backward compatibility is explicitly out of scope. Defer
the targeted command run until that batch is complete.

## Work package B: validate targets in AppServer

### Changes

1. Add one AppServer operation that accepts `(ConnectionKey,
   InteractiveTarget)` and returns the live opaque handle plus attachment
   snapshot.
2. Validate in one registry/routing critical section:
   connection owns surface, surface is interactive, surface points to session,
   and session is `Live`.
3. Return a typed domain error for missing surface, wrong connection, wrong
   session, passive surface, and loading/closing/missing session.
4. Map domain errors to stable JSON-RPC error data in the adapter.
5. Remove session selection through
   `sole_interactive_session_for_connection` from command dispatch.
6. Validate `ArchiveTarget::Orphaned`: succeed only when the session has no
   interactive owner, otherwise return `InteractiveOwnerConflict`.

### Primary files

- `app/server/src/app_server.rs`
- `app/server/src/app_server.test.rs`
- `app/server/src/json_rpc_adapter.rs`
- `app/server/src/json_rpc_adapter.test.rs`
- `app/server/src/local_client_adapter.rs`
- `app/server/src/local_client_adapter.test.rs`

### Tests and gate

- correct owner/target returns exactly the requested live handle;
- another connection's surface is rejected;
- a passive surface and mismatched session id are rejected;
- loading and closing slots have deterministic errors;
- validation holds no lock across `.await`.

## Work package C: isolate accepted connections

### Changes

1. Replace the shared handler instance with a
   `JsonRpcConnectionHandlerFactory`; `open(connection)` creates one handler.
2. Move profile, transport writer, ordered outbound queue, and JSON-RPC id
   correlation into that connection handler.
3. Snapshot the connection profile into start/resume runtime construction.
4. On disconnect, remove that connection's callback route and surfaces without
   changing sibling connection state.
5. Route approval, user-input, elicitation, hook, and MCP domain requests
   through AppServer's pending request owner. Replies must match connection,
   surface, session, and request id. When no interactive surface is attached,
   pending requests fail with `NoInteractiveSurface` per the orphaned-session
   callback semantics in `protocol-scope.md`; nothing parks.
6. Move MCP registration reports below the targeted session rather than keying
   process state only by server name.
7. Derive immutable `SessionCallbackRequirements` during runtime construction.
   Live orphan resume rebinds only when the new profile satisfies them;
   otherwise return `ConnectionProfileMismatch`.

### Primary files

- `app/server/src/json_rpc_adapter.rs`
- `app/server/src/json_rpc_adapter.test.rs`
- `app/agent-host/src/sdk_server/app_server_bridge.rs`
- `app/agent-host/src/sdk_server/app_server_bridge.test.rs`
- `app/agent-host/src/sdk_server/handlers/initialize_state.rs`
- `app/agent-host/src/sdk_server/handlers/bootstrap_state.rs`
- `app/agent-host/src/sdk_server/handlers/connection_state.rs`
- `app/agent-host/src/sdk_server/handlers/server_request_state.rs`
- `app/agent-host/src/sdk_server/handlers/pending_client_request_state.rs`
- `app/agent-host/src/sdk_server/handlers/mcp_registration_state.rs`

### Tests and gate

- two connections initialize with different agents/hooks/preferences and each
  constructed session observes only its own profile;
- concurrent server requests use the correct writer and accept only the
  matching reply;
- a compatible orphan resume rebinds callbacks, while a profile with missing
  or different callback identifiers is rejected without changing the runtime;
- callbacks issued while a session is orphaned fail with
  `NoInteractiveSurface` and the running turn continues per contract;
- disconnecting A does not invalidate B;
- same-named MCP servers in A and B have distinct status reports;
- `InitializeState`, connection-scoped `BootstrapState` fields,
  `ConnectionState`, and process-wide `McpRegistrationState` no longer exist
  on shared host state; process startup cwd/policy remains separately owned.

## Work package D: make clients carry authority

### Changes

1. Make `RemoteSessionClient` and `LocalSessionClient` store their immutable
   `SessionTarget`/`InteractiveTarget` and inject it into every method.
2. Passive handles expose reads and subscriptions, never mutations.
3. Connection clients expose only initialize, lifecycle, process, and catalog
   operations.
4. Remove public low-level unscoped turn, runtime-control, MCP, rewind, and
   callback-reply helpers.
5. Make replace/archive consume the interactive handle where the operation
   invalidates or repoints it.

### Primary files

- `app/server-client/src/lib.rs`
- `app/server-client/src/lib.test.rs`
- `app/agent-host/src/local_client.rs`
- `app/agent-host/src/local_client.test.rs`

### Tests and phase gate

- one remote connection holds A and B and serializes distinct targets;
- local and remote handle APIs have matching authority boundaries;
- passive mutation is unrepresentable through the public typed API;
- connection-wide helpers cannot issue session mutations.

Run once after packages A through D are complete:

```bash
just test-crate coco-types
just test-crate coco-app-server
just test-crate coco-app-server-client
just test-crate coco-agent-host
just quick-check
```

## Work package E: select one canonical runtime

### Changes

1. Split broad handler context into process/connection context and a required
   `SessionRequestContext` containing the validated target and `SessionHandle`.
2. Delete the fallback chain from sole surface to installed runtime to sole
   scoped handoff.
3. Replace `StateQueryEngineRunner` with a `TurnExecutor` that receives the
   selected `SessionHandle` on every call.
4. Build history, `ToolAppState`, cwd, folded config, model runtime, tools,
   event identity, and active-turn coordination from that same handle.
5. Move active turn state behind the runtime. A keyed transitional map is
   acceptable only inside this package and must never select a runtime.

### Primary files

- `app/agent-host/src/sdk_server/handlers/dispatch.rs`
- `app/agent-host/src/sdk_server/dispatcher.rs`
- `app/agent-host/src/sdk_server/sdk_runner.rs`
- `app/agent-host/src/sdk_server/sdk_runner.test.rs`
- `app/agent-host/src/sdk_server/handlers/mod.rs`
- `app/agent-host/src/session_runtime.rs`

### Tests and gate

- A's turn cannot combine A history with B cwd/config/tools;
- A and B can both have active turns and interrupt independently;
- event `(session_id, turn_id)` comes from the selected runtime;
- the production turn path contains no `session_runtime_snapshot()` or
  implicit runtime resolution. Other capability consumers are removed in F.

Run package tests once, then `just quick-check` once.

## Work package F: relocate session capabilities

Migrate capabilities in the following order. For each row, change all
consumers, add A/B isolation coverage, and only then remove the old field.

| Capability | Consumers to migrate | Removal gate |
|---|---|---|
| Runtime | turns, controls, shortcuts, approvals, sandbox, lifecycle helpers | no request path reads `SessionRuntimeState` |
| MCP | status, set/reconnect/toggle, tool registration, auth, elicitation | all operations use targeted session; same-named definitions remain isolated |
| File history | rewind preview/apply, snapshot/config-home lookup | A cannot read or restore B snapshots |
| Reload | sandbox/config/model reload subscriptions and shutdown | B startup/replacement does not abort A supervisor |

Reload needs a session-lifetime owner such as
`SessionReloadSupervisor`, which replaces only its own task, observes session
shutdown, and is stopped and awaited by the close cascade.

### Primary files

- `app/agent-host/src/sdk_server/handlers/mcp.rs`
- `app/agent-host/src/sdk_server/handlers/rewind.rs`
- `app/agent-host/src/sdk_server/handlers/session_runtime_state.rs`
- `app/agent-host/src/sdk_server/handlers/mcp_manager_state.rs`
- `app/agent-host/src/sdk_server/handlers/file_history_state.rs`
- `app/agent-host/src/sdk_server/handlers/runtime_reload_state.rs`
- `app/agent-host/src/session_runtime/session_handle.rs`
- `app/agent-host/src/session_runtime/resources.rs`
- `app/agent-host/src/session_runtime/reload.rs`
- `app/agent-host/src/session_runtime/state/file_history.rs`

### Phase gate

- `SessionRuntimeState`, `McpManagerState`, `FileHistoryStateSlot`, and
  `RuntimeReloadState` are absent from shared process state;
- every removed field's feature remains available through a targeted session;
- starting, replacing, or closing B cannot overwrite or abort A resources;
- no duplicate process pending-request map competes with AppServer ownership.

Run affected crate tests once, then `just quick-check` once.

## Work package G: finish lifecycle semantics

### Replace

Add the explicit `session/replace` protocol. Start always creates a new
identity; resume rejoins a requested identity; neither implicitly replaces a
different session. Reuse the existing two-phase registry/routing commit.

### Resume while closing

When load observes `Closing`, clone the completion, release locks, await it
with request timeout/cancellation, and retry normal load. Cancelling the
waiter must not cancel the owner close task. Competing resumes converge on the
same loading completion.

### Orphan archive

`session/archive` accepts `ArchiveTarget`. `Interactive` validates like any
interactive mutation; `Orphaned` closes a live session that has no
interactive owner under the transport authorization boundary, so a
disconnected client never needs resume-then-archive to release a runtime.

### Close cascade

1. reject new turns;
2. cancel/drain the active turn under the configured timeout;
3. stop session background tasks;
4. run bounded end hooks;
5. flush transcript and sequence watermark;
6. tear down MCP, reload, and file resources;
7. archive routing and remove the registry slot.

### Primary files

- `app/server/src/registry.rs`
- `app/server/src/registry.test.rs`
- `app/server/src/app_server.rs`
- `app/agent-host/src/sdk_server/session_lifecycle.rs`
- `app/agent-host/src/sdk_server/app_server_bridge.rs`

### Gate

- closing resume waits and reopens without returning a draining handle;
- load/close/replace cancellation never owns lifecycle progress;
- replace consumes the old client and returns it only on pre-commit failure;
- an orphaned session is archivable without reattaching, and
  `ArchiveTarget::Orphaned` on a session with an interactive owner fails with
  `InteractiveOwnerConflict`;
- close A leaves B live and process shutdown drains both.

Run affected crate tests once, then `just quick-check` once.

## Work package H: production isolation suite

Add `app/agent-host/tests/multi_session_app_server.rs` as the release-blocking
production-path suite. Use public local or remote clients and production
handlers; direct registry construction is insufficient.

Required scenarios:

1. one connection starts A and B and independently turns, controls, reads,
   rewinds, and interrupts each handle;
2. two initialized connections run real turns concurrently with different
   profiles, cwd, config, tools, MCP definitions, histories, and writers;
3. cross-connection surface use, passive mutation, and mismatched
   `(surface_id, session_id)` fail with stable typed errors;
4. project/local config writes resolve from the targeted session cwd and
   cannot modify the sibling project;
5. callback replies cannot cross connection, surface, session, or request id;
6. orphan resume rejects incompatible callback requirements and safely rebinds
   a compatible connection;
7. reload subscriptions coexist and close/replacement reaps only the selected
   runtime;
8. events and replay retain correct session/turn identity;
9. process shutdown drains A and B without serially blocking unrelated work;
10. a disconnected connection leaves its session orphaned: its callbacks fail
    closed per the protocol contract, the running turn continues, and the
    session is archived via `ArchiveTarget::Orphaned`;
11. a slow consumer's connection is disconnected as one unit and both
    sessions recover by reconnect plus replay without event loss or
    cross-session leakage.

Use deterministic barriers rather than sleeps for concurrency assertions.
Give each scenario a bounded timeout so a deadlock reports a local failure.

Run once after the suite is complete:

```bash
just test-crate coco-agent-host
just quick-check
```

## Work package I: hardening after correctness

Only after package H is green:

- replace `SessionHandle::Deref` and public `runtime()` with focused capability
  methods; this improves the boundary but must not block runtime selection;
- move `TurnCoordinator` to a task only if ordered steering/queue semantics
  require it;
- add strict per-key project load single-flight only if measurements show
  duplicate initialization is material;
- add capability-named project services only with an explicit key and
  lifecycle. Do not add `ProjectHeavyServices`;
- enable `clippy::await_holding_lock` as a workspace lint so the
  no-await-under-lock invariant is mechanical rather than review-enforced
  (may land earlier at any green point — `coco-app-server` is already
  compliant);
- audit session-scoped spans, logs, and metrics for the `session_id` and
  `turn_id` fields required by the Observability section of
  `target-architecture.md`;
- build Web/Desktop/IM adapters outside AppServer core after local/SDK
  isolation is complete.

The final target still forbids public runtime escape APIs. This ordering merely
separates architectural hardening from the critical correctness repair.

## Final validation

Run `just quick-check` once for the final changed phase. After it is green and
no code changes remain, run the final gate:

```bash
just pre-commit
```

Run `just pre-commit` once, immediately before the commit. Do not rerun it
after a green result unless the code changes again.

## Definition of done

- every `ClientRequest` has one required scope from `protocol-scope.md`;
- no session mutation has an absent or optional target;
- every accepted connection has an isolated profile, writer, correlation
  state, and disconnect lifetime;
- every live capability is reached through the registry-selected
  `SessionHandle` or an explicitly keyed process/catalog owner;
- no production handler reads a current-runtime/current-MCP singleton;
- the four session process slots and connection-mis-scoped fields are removed
  only after feature parity tests pass;
- config reads/writes use their typed process/user/session/project/local scope;
- lifecycle races have deterministic typed outcomes and no waiter owns
  progress;
- orphaned sessions are archivable, and orphan-period callbacks fail closed
  with the per-family outcomes in `protocol-scope.md`;
- production A/B tests prove runtime, integration, callback, event, and
  shutdown isolation;
- no global lock is held across I/O or `.await`;
- `just pre-commit` is green once on the final tree;
- current architecture documentation is updated to describe the landed code.
