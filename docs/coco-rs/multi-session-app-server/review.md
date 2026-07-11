# Verified Architecture Review

Resolution status: all confirmed R1-R11 gaps were addressed by the breaking
refactor. The evidence below is retained as the review of the pre-refactor
baseline; the production ownership model is documented in
[current-architecture.md](current-architecture.md).

## Post-refactor delivery audit

Three follow-up delivery findings were also confirmed and resolved on
2026-07-11:

- orphan archive authorization had occurred after handler side effects; it now
  proves orphan authority before dispatch and is protected by a regression test
  in which an owned session's active turn remains live after rejection;
- the earlier integration-test count did not by itself prove all package-H
  scenarios; the suite now contains semantic coverage for every one of the
  eleven requirements, with overall bounded timeouts for all concurrent and
  lifecycle cases;
- the SDK `pending_map` module was unused outside its own tests after callback
  ownership moved to AppServer; the module, tests, and public export were
  removed.

These are closed findings, not amendments to the pre-refactor R1-R11 analysis
below. The evidence mapping is recorded in
[remediation-plan.md](remediation-plan.md#package-h-evidence-matrix).

This review re-derived each reported issue from production call paths and then
looked for code or tests that would refute it. The purpose is to distinguish
real correctness gaps from differences between an aspirational document and a
valid Rust implementation.

## Method

The review followed these paths:

1. canonical `ClientRequest` DTO -> remote/local client helper;
2. JSON-RPC/local adapter -> `AppServerSdkHandler` request context;
3. handler target resolution -> `TurnRunner` and runtime-control handlers;
4. registry slot transition -> host lifecycle wrapper;
5. per-session runtime construction -> project config and integration setup;
6. existing unit/integration tests -> claimed acceptance criteria.

The findings below are about production behavior. A missing actor or a type
that differs from a design sketch is not classified as a bug unless it breaks
an invariant.

## R1: requests cannot target one of several interactive surfaces

**Verdict: confirmed, critical.**

Evidence:

- `TurnStartParams`, `TurnInterrupt`, session status/cost, task controls,
  runtime controls, MCP controls, and several other session operations do not
  carry `session_id` or `surface_id`.
- `RemoteSessionClient` stores both identifiers, but `query()` delegates to
  connection-level `turn_start(params)` and `interrupt()` delegates to
  connection-level `turn_interrupt()` without injecting either identifier.
- `AppServerSdkHandler` derives scope with
  `sole_interactive_session_for_connection`. Routing intentionally returns
  `None` when that connection owns two interactive surfaces for two different
  sessions.
- The fallback after `None` is the process-installed runtime or a sole SDK
  handoff, neither of which identifies the caller's `RemoteSessionClient`.

Counter-hypothesis: the client handle might implicitly bind requests to a
surface through its transport. Refuted: all handles clone the same
`RemoteJsonRpcClient`, and JSON-RPC request context contains a connection key,
not a surface key.

Consequence: the routing data model permits one connection to own several
sessions, but its command protocol cannot select one. The typed handle is
currently an identity display/event-demux facade, not a command capability.

## R2: production turns may run with the wrong SessionRuntime

**Verdict: confirmed, critical.**

Evidence:

- Handler turn setup correctly derives a session id and retrieves that
  session's keyed `SessionHandoffState`.
- The production `StateQueryEngineRunner` ignores that identity when selecting
  the runtime. At execution time it calls
  `SdkServerState::session_runtime_snapshot()`, a single `Option<SessionHandle>`
  replaced by every successful start/resume.
- `run_turn_with_session` obtains cwd, runtime config, tools, model runtime,
  hooks, and other engine inputs from that selected handle, while history and
  `ToolAppState` arrive separately through the keyed handoff.

Counter-hypothesis: the handler's `scoped_runtime` might reach the runner.
Refuted: it is used by runtime-control handlers through
`HandlerContext::resolve_runtime`, but it is not part of the `TurnRunner`
signature and is not passed to `StateQueryEngineRunner`.

Consequence: after session B becomes the installed runtime, a turn targeted at
session A can combine A's history/app state with B's cwd/config/tools. This is
cross-session state mixing, not merely a stale status display.

## R3: session-owned integration state remains process-singleton

**Verdict: confirmed, high.**

The following slots live once on `SdkServerState` and are overwritten or
aborted when a different runtime is installed:

- `SessionRuntimeState`;
- `McpManagerState`;
- `FileHistoryStateSlot`;
- `RuntimeReloadState`.

Examples:

- MCP handlers read `state.mcp_manager_snapshot()` and several tool
  registration paths read `state.session_runtime_snapshot()` instead of the
  routed runtime.
- replacement runtime setup passes the currently installed MCP manager into
  `bootstrap_session_mcp`, allowing sessions with different project MCP
  definitions to share one mutable manager accidentally;
- rewind selects a scoped session id, but reads file-history storage from the
  process singleton;
- installing a new runtime aborts the previous runtime reload subscription.

Counter-hypothesis: these are intentionally process-shared services. Refuted:
their inputs and side effects depend on session cwd, project configuration,
tool registry, file snapshots, or sandbox state. Sharing them requires an
explicit definition-site key and lifetime contract, neither of which these
slots have.

Remediation does not mean deleting their functionality. Immediate deletion
would break turns, controls, MCP operations, rewind, approvals, and reload.
Each capability must first move behind the registry-selected `SessionHandle`;
only the redundant process slot is then removed. MCP and file history already
have partial runtime ownership. Reload still needs an explicit
session-lifetime supervisor before its process slot can be retired.

## R4: resume during Closing does not wait and reopen

**Verdict: confirmed, medium.**

The registry exposes a close completion when `begin_load` observes a
`Closing` slot. The host wrapper converts `AppLoadStart::Closing` directly to
an internal error rather than awaiting completion and retrying the load. This
contradicts the former plan's wait-and-reopen contract.

The target behavior is still reasonable: never return a draining handle;
await close outside locks; retry normal disk load afterward. The behavior must
also be bounded by request cancellation/timeout without cancelling the owner
close task.

## R5: SessionRuntime is not actor-owned

**Verdict: factual, but not itself a defect.**

`SessionRuntime` is an `Arc`-shared resource owner composed from focused
resource groups. `SessionHandle` wraps `Arc<SessionRuntime>`, exposes
`runtime()`, and implements `Deref`. There is no general `SessionCommand`
driver owning all mutable state.

The former document treated a whole-runtime actor as both a locked decision
and a future evolution. That is a documentation contradiction. It is not
evidence that the current lock-based Rust structure is invalid.

Review decision:

- do not require a whole-runtime actor for multi-session correctness;
- remove `Deref` and raw runtime escape APIs after callers have explicit
  capability methods;
- serialize only turn lifecycle and other genuinely coupled state through a
  small `TurnCoordinator` boundary;
- keep independent service handles independent rather than routing every read
  through one mailbox.

This follows Rust's ownership model more directly and avoids a god actor,
mailbox backpressure for ordinary reads, and unnecessary oneshot plumbing.

## R6: ProjectServices differs from the former target

**Verdict: confirmed, but the former target should not be implemented as
written.**

Current `ProjectServices` owns a configuration snapshot and project/plugin
catalog. `ProjectRegistry` provides publication deduplication, freshness
replacement, identity reuse, and idle eviction.

It does not own LSP, retrieval, ignore/context discovery, or project-shared
MCP instances. Concurrent cold loads may both perform I/O, after which one
published `Arc` wins. Therefore "true single-flight" is not an accurate name
for the current algorithm.

The proposed name `ProjectHeavyServices` is rejected. "Heavy" describes an
implementation cost, not a responsibility or invariant. Future shared
capabilities should use functional names such as `ProjectMcpRegistry` or
`ProjectLanguageServices`, and should be introduced only when their keys,
sharing semantics, and teardown behavior are proven.

## R7: crate dependency concerns

**Verdict: refuted as a current issue.**

The desired split is present:

- `coco-app-server-client` depends on transport and canonical types, not on
  `coco-app-server`;
- `coco-agent-host` owns in-process client composition and application runtime
  integration;
- `coco-app-runtime` owns project/workspace/bootstrap contracts;
- engine/core crates are protected from server dependencies by the checked-in
  seam guard.

The boundary should be retained. The remaining defects come from duplicate
runtime ownership above the boundary, not from the crate graph.

## R8: surface and event infrastructure

**Verdict: largely landed.**

Existing tests cover passive plus interactive attachments, second-owner
conflicts, connection surface limits, capability-gated server requests,
replace/archive routing, replay boundaries, slow consumers, owner-task
progress, and multi-slot shutdown.

What is missing is an end-to-end production test that creates two real runtime
handles and runs turns through the public remote client. Routing unit tests do
not refute R1 or R2 because they stop before request target selection and
engine construction.

## R9: accepted connections do not have isolated handler state

**Verdict: confirmed, critical for multi-connection SDK operation.**

Evidence:

- listener connections reuse one `AppServerSdkHandler` backed by one
  `SdkServerState`;
- `InitializeState` is one shared set of `RwLock`s containing SDK agents,
  plan-mode instructions, and hook callbacks; related initialize-derived
  preferences also live on shared `BootstrapState`, so a later connection can
  replace inputs used by another connection's session;
- `ConnectionState` contains one transport and ordered outbound writer slot;
- `McpRegistrationState` indexes status reports only by server name rather
  than by owning session;
- callback waiter/request maps are shared and do not uniformly prove
  connection, surface, session, and request-id ownership on reply.

Counter-hypothesis: AppServer's `ConnectionKey` already provides isolation.
Refuted: routing knows the connection key, but these host fields live outside
the routing entry and are not keyed by it.

Remediation: keep the registry, routing, catalogs, and runtime factory shared,
but create one connection handler with an immutable-after-initialize
`ConnectionProfile`, writer, and JSON-RPC correlation state per accepted
connection. Domain server requests remain owned and validated by AppServer.

## R10: configuration requests do not have one natural process scope

**Verdict: confirmed, high.**

`config/read` derives an effective fold using the handler workspace cwd.
Project and local `config/value/write` also resolve their target path from
cwd. Treating all configuration operations as process-scoped would preserve
the same implicit-current-workspace bug under a different name.

The protocol must distinguish process/user configuration from session-derived
effective reads and project/local writes. The latter require an explicit
session, and writes require an interactive target because they mutate the
selected workspace.

## Overall assessment

The multi-session goal is reasonable and valuable:

- long-lived local IDE/desktop processes need several independent sessions;
- sharing process-level catalogs and provider infrastructure can reduce
  startup cost;
- passive surfaces and reconnect/replay enable observers without duplicating
  engines;
- explicit session ownership improves correctness even for single-session TUI
  and headless modes.

The goal becomes unreasonable if it is defined as maximizing shared mutable
state or forcing all session resources through one actor. Correct isolation
and explicit targeting are the value; a large actor and speculative shared
services are not.
