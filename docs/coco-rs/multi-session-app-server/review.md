# Adversarial Architecture Review

Audit date: 2026-07-13.

Scope: `coco-rs/app/{agent-host,cli,sdk-server,server,server-client,runtime}`,
the session store, Hub connector, production multi-session tests, and the
previous documents in this directory.

The review treats code and tests as evidence, not as proof that the documented
architecture is correct. Each finding below records the observed path, a
counter-hypothesis, and the resulting decision.

## Method

The audit followed these paths:

1. CLI schema -> mode selection -> tracing mode -> selected runner;
2. process startup -> host construction -> first live session;
3. local/remote request -> target validation -> selected `SessionHandle`;
4. start/resume/replace/close -> registry owner task -> runtime teardown;
5. turn start -> active-turn ownership -> terminal result/event ordering;
6. session close/delete -> `SessionManager` -> JSONL behavior;
7. AppServer event -> Hub connector announce/reconnect/batch path;
8. crate manifests, `lib.rs` exports, module layout, and public capability APIs;
9. existing tests -> claimed completion properties.

## Retained v1 findings

The previous refactor did fix important issues. These results are still
supported by code and production-path tests:

- interactive mutations carry explicit `(session_id, surface_id)` authority;
- AppServer validates connection, surface role, session identity, and live
  registry state before returning a runtime handle;
- accepted remote connections get independent initialize/profile state;
- turn execution receives the validated `SessionHandle` instead of selecting a
  process-global current runtime;
- active turn, MCP, file history, reload state, and callback requirements are
  session-keyed for the tested paths;
- registry loading/closing/replacement owner tasks prevent caller cancellation
  from owning lifecycle progress;
- remote client and server implementation dependencies remain separated.

No evidence supports reverting those decisions.

## V2 findings

### V2-R1: `session/archive` mixes runtime close and durable deletion

Verdict: confirmed, critical; production code now remediated in Phase A.

Original evidence:

- `target-architecture.md` previously said archive was runtime close and JSONL
  remained resumable.
- the former `session_archive::archive_live_session` path called
  `delete_persisted_session_record`;
- that helper called `SessionManager::delete`;
- `SessionManager::delete` explicitly removes the transcript JSONL.

Primary paths:

- removed Phase A path:
  `app/agent-host/src/session_archive.rs` and
  `app/agent-host/src/app_server_host/request_handlers/session/archive.rs`;
- replacement paths:
  `app/agent-host/src/session_close.rs`,
  `app/agent-host/src/app_server_host/session_close.rs`, and
  `app/agent-host/src/app_server_host/request_handlers/session/delete.rs`;
- `app/session/src/lib.rs:542-547`

Counter-hypothesis: "archive" may intentionally mean permanent deletion.

Result: even if that product meaning were intended, one request still combines
two independently important operations and contradicts resume-oriented
protocol text. It also makes orphan cleanup destructive. The API must separate
live close from durable delete.

Decision: remove `session/archive`; add `session/close` and `session/delete`.
Current status: landed. Close preserves the transcript; delete is explicit,
storage-only, and rejects live/loading/closing slots.

### V2-R2: close timeout can leave detached work after close

Verdict: confirmed, critical; partially remediated in Phase A.

Evidence:

- the former archive path removed `ActiveTurnHandles` from
  `SessionTurnCoordinator` before
  draining;
- each task is awaited through `tokio::time::timeout` by value;
- on timeout the `JoinHandle` is dropped without `abort`, detaching the task;
- the warning explicitly allows late events;
- the later registry close saw no active-turn handles because archive already
  took them.

Primary paths:

- removed Phase A path: `app/agent-host/src/session_archive.rs:52-98`
- `app/agent-host/src/session_runtime/session_handle.rs:1075-1093`
- `app/agent-host/src/app_server_host/session_close.rs:120-142`

Counter-hypothesis: cancellation always makes the tasks finish before timeout.

Refuted: the timeout branch is reachable by construction and its own warning
documents the late-task behavior. Cancellation is cooperative and cannot prove
termination.

Decision: registry close is the sole owner of active task handles. It cancels,
awaits, aborts on timeout, and awaits the abort before reporting close complete.
Current status: the destructive archive path is gone and registry close now
drains through `SessionHandle::drain_active_turn`, which aborts and awaits
timed-out turn/forwarder tasks. Close completion now returns structured
`session_close_timeout` data for drain timeouts. Forced turn-task and
forwarder-task timeout regressions have been added and compiled for the next
batched test run. A successful-close no-late-session-event regression has also
been added and compiled; it verifies that close drains the active turn, emits
the final `SessionResult`, and has no further same-session outbound events
after the close response completes.

### V2-R3: terminal accounting is captured before active-turn drain

Verdict: confirmed, high.

Evidence:

- `build_session_result` runs before active turn cancellation/drain;
- the former archive handler documented that the in-flight turn was excluded;
- the handler then emits that incomplete aggregate as the terminal
  `SessionResult`;
- the former archive path used a hard-coded five-second drain rather than the host's configured
  turn-drain timeout.

Primary paths:

- removed Phase A paths:
  `app/agent-host/src/session_archive.rs:47-64` and
  `app/agent-host/src/app_server_host/request_handlers/session/archive.rs:18-22,59-72`;
- replacement path:
  `app/agent-host/src/app_server_host/session_close.rs`

Decision: terminal result creation happens after all turn event forwarding has
stopped. One configured close deadline is threaded through the lifecycle owner.
Current status: close emits the final `SessionResult` from the close owner
after the runtime close cascade. An in-flight close accounting/order regression
has been added and compiled for the next batched test run; it forces a turn to
emit per-turn accounting during close and verifies the final close
`SessionResult` includes that accounting.

### V2-R4: declared CLI modes do not select the documented runners

Verdict: confirmed, critical for CLI correctness.

Current status: partially remediated. `coco-cli` now has a shared
`ExecutionPlan`/`IoCapabilities` seam used by both `main.rs` and
`tracing_init.rs`; `--non-interactive` selects headless, `resume` is classified
as interactive, and the unsupported global `--no-tui` and `--json` flags are
rejected by clap. Mode-dependent validation for `--no-session-persistence` and
`--plan-mode-instructions` moved into fallible pure plan construction.
Placeholder subcommands that only printed success/not-implemented messages are
deleted from clap. Headless stdin behavior is now explicit: piped stdin becomes
the raw prompt when no `--prompt` is provided. Confirmed CLI-only flags with no
runner consumer are rejected by clap. The retained top-level clap schema is
guarded by an accepted-field consumption audit test.

Original evidence:

- clap declares `--no-tui`, `--json` (described as SDK mode), and
  `--non-interactive`/`--print`;
- production mode selection ignores all three flags;
- default selection checks only prompt presence or non-terminal stdout;
- stdin is not part of the decision;
- `tracing_init::detect_mode` duplicates the same partial logic;
- several other accepted flags are parsed but never mapped into
  `AgentHostOptions` or consumed by a runner.

Primary paths:

- `app/cli/src/execution_plan.rs`
- `app/cli/src/lib.rs`
- `app/cli/src/main.rs`
- `app/cli/src/tracing_init.rs`

Counter-hypothesis: clap aliases or `AgentHostOptions` perform the conversion.

Refuted: repository-wide production-use searches find no such conversion for
the mode flags. Tests only verify parsing for several scripting flags.

Decision: one pure execution-plan seam validates and selects the mode.
Unsupported flags and placeholder commands are deleted rather than retained as
no-ops.

### V2-R5: SDK startup creates a hidden placeholder session

Verdict: confirmed, high; production placeholder path remediated in Phase D.

Evidence:

- the protocol says initialize does not create a hidden startup session;
- `prepare_remote_host` generates `startup_session_id`, builds a full runtime,
  creates MCP and Event Hub integrations, fires session hooks, and registers the
  runtime before accepting the first client lifecycle request;
- the first start/resume has special logic to replace the detached placeholder.

Original primary paths:

- `app/agent-host/src/remote_host.rs:162-239`
- `app/agent-host/src/app_server_host/session_start_operation.rs:73-119`
- `app/agent-host/src/app_server_host/session_resume_operation.rs:111-132`

Counter-hypothesis: initialize metadata requires a live runtime.

Refuted: initialize already has a bootstrap metadata provider and its unscoped
request context cannot select a live session. The placeholder is not necessary
for request authority.

Decision: SDK prepares process services, AppServer, factory, listeners, and
metadata snapshots only. The first `session/start` or `session/resume` builds
the first runtime.

Current status: landed for the placeholder behavior. `HostBuilder::prepare` no
longer builds a startup runtime, fires session hooks, creates MCP integrations,
or registers a surfaceless session. `RuntimeReplacementContext` no longer
carries `startup_session_id`, and `session/start`/`session/resume` no longer
replace a detached placeholder slot. A regression verifies that prepared remote
hosts have an empty AppServer registry and that `initialize` succeeds without
creating a session. A second regression sets `COCO_SERVER_MAX_SESSIONS=1` and
verifies the first real `session/start` succeeds, preventing the old hidden-slot
failure mode from returning.

### V2-R6: Event Hub membership is not registry-owned process state

Verdict: confirmed architecture defect; event loss not proven.

Evidence:

- `RuntimeEventHubConnector::spawn_for_session` creates one immutable
  `AnnounceFrame`;
- live membership is not derived from the AppServer registry;
- later AppServer start/resume/replace/close operations do not update a
  process-owned announce live set;
- reconnect reuses the same announce and Hub resume cursors are returned only
  for announced live sessions.

Primary paths:

- `app/agent-host/src/event_hub.rs:43-59,108-120`
- `app/agent-host/src/remote_host.rs`
- `hub/connector/src/worker.rs:491-492,553-572`

Counter-hypothesis: Hub accepts batches for sessions not listed in announce.

Not refuted: current connector code can still send those batches, so this
review does not claim proven event loss. What is proven is incorrect live
membership and incomplete reconnect cursor negotiation.

Decision: Event Hub is process-host egress. Its announce is generated from the
AppServer live registry and is refreshed on lifecycle changes/reconnect.

Current status: partially remediated. The connector owner is now
`ProcessEventHub`, and process hosts start it with an explicit live-session
snapshot instead of requiring a placeholder runtime. The connector worker
announces on startup, so an empty host can announce `live_sessions: []`.
SDK remote, TUI, and headless startup paths attach process-owned Event Hub
egress to the AppServer outbound path and run a membership watcher over
AppServer activity revisions. Local sidecar and SDK stdio writers also refresh
membership immediately before routing a session event to the Hub, so the event
cannot intentionally outrun the live-set announce after a session transition.
Registry-derived dynamic membership is still not fully proven: close, replace,
reconnect-cursor, and identity/ack isolation regressions remain open. SDK
remote startup and A/B start membership are covered by a focused remote-host
regression.

### V2-R7: the three surfaces do not share one session lifecycle

Verdict: confirmed, high drift risk.

Evidence:

- TUI startup previously constructed a runtime for the resume id and directly
  hydrated and bound it when the ids matched;
- headless previously constructed a runtime with an id override and directly
  seeded resume state;
- SDK uses the AppServer resume lifecycle operation;
- headless previously waited for session aggregation with a short polling loop
  and fabricated a fallback result if the projection had not updated. That
  specific polling/fallback path has been removed in Phase B startup work.

Primary paths:

- `app/cli/src/tui_runner/bootstrap.rs:168-240,309-339`
- `app/cli/src/tui_runner/session_switching.rs:278-365`
- `app/agent-host/src/headless.rs:867-955,1181-1207`

Counter-hypothesis: all paths eventually install the same runtime shape.

Result: shared construction shape does not guarantee shared lifecycle ordering,
callback binding, close semantics, or result delivery. The earlier refactor
removed engine drift but not surface orchestration drift.

Decision: every surface opens a typed client connection and uses the same
start/resume/replace/close operations. Surface code never directly registers or
hydrates a runtime.

Current status: partially remediated. TUI startup now uses local
`session/start` for fresh sessions and local `session/resume` when the binary
resolved a resume/fork plan; it no longer creates a placeholder fresh runtime
before startup resume. Production headless resume now carries an explicit
`resume_target` and enters through local `session/resume`; fresh headless uses
local `session/start` with an explicit session id. Runtime integration policy
is supplied through `RuntimeReplacementContext`, so lifecycle-owned runtime
construction installs the TUI/headless-specific integrations instead of surface
startup doing it after registration. In-session TUI `/resume` and `/branch`
now switch through a local typed `session/replace` resume facade, and `/clear`
now switches through a typed `session/replace` clear destination. Main TUI shortcut,
observability, and driver control paths now activate an already-live
interactive session through a `SessionId` facade instead of registering
`SessionHandle`s from the TUI layer. Prompt-mode bash response turns now return
to the main driver and start through that same bridge instead of a short-lived
binding bridge. Test/embedding headless callers that provide in-memory prior
messages now send them through typed `session/start.initial_messages`; the
AppServer-owned runtime builder hydrates history before the first turn instead
of the headless surface mutating history after startup. AppServer drain and
Event Hub membership-watcher stop/flush now use a shared
`ShutdownCoordinator` across headless, TUI, and SDK remote-host shutdown.
Remaining gap: shared lifecycle conformance coverage does not yet span all
connection styles.

### V2-R8: `coco-agent-host` is not protocol-neutral

Verdict: confirmed architecture defect.

Evidence:

- `coco-agent-host` directly depends on `coco-tui`;
- production host modules use `coco_tui::ImageData`, `SystemPushKind`,
  `UserCommand`, `App`, and TUI permission rendering;
- the target dependency graph omits this edge.

Primary paths:

- `app/agent-host/Cargo.toml:20-68`
- `app/agent-host/src/app_server_host/session_turn_executor.rs:43-71`
- `app/agent-host/src/session_messages.rs:88-128`
- `app/agent-host/src/{tui_permission_bridge,voice_bootstrap,teammate_inbox_pump}.rs`

Decision: generic image/message DTOs move to lower crates. TUI bridges and the
TUI driver move to a surface composition crate above agent-host.

### V2-R9: module and public API organization do not express ownership

Verdict: confirmed maintainability defect.

Evidence:

- `agent-host/src/lib.rs` exports 69 implementation modules;
- the source root contains 71 non-test non-`lib.rs` Rust files, including 18
  top-level `session_*` files;
- `app_server_host` contains another 16 `session_*` modules with step-oriented
  names;
- many modules use `session_*`
  prefixes as a substitute for a module hierarchy;
- `app_server_host` has the opposite problem: one lifecycle request is spread
  across many small step-named files;
- `SessionHandle` is about 1,489 lines with roughly 186 public methods;
- it returns raw `Arc<Mutex<_>>`, `Arc<RwLock<_>>`, registries, managers, and
  mutable service handles;
- several modules exceed the repository's 800-line target.

Counter-hypothesis: explicit forwarding preserves an opaque runtime.

Result: it hides the `SessionRuntime` type but still exports its mutable
implementation capabilities. This is not a data race by itself, but it defeats
the claimed focused-capability boundary and makes surface coupling easy.

Decision: organize by lifecycle/operation/integration ownership, default
modules to private, and expose small operation/snapshot capabilities rather
than locks.

### V2-R10: construction-time invariants remain temporally optional

Verdict: confirmed architecture defect.

Evidence:

- `AppServerHostState::default` is followed by a sequence of `install_*`
  mutations before the host is valid;
- startup-only installation uses `try_write` plus `panic` on lock contention;
- `SessionCallbackRequirements` is a late `OnceLock`; a second set error is
  ignored and reads before installation return default requirements;
- session identity exists both as an immutable handle snapshot and inside
  mutable `QueryEngineConfig`; an arbitrary update closure is constrained by a
  runtime `assert_eq!`.

Primary paths:

- `app/agent-host/src/app_server_host/state.rs:13-30,59-130`
- `app/agent-host/src/app_server_host/bootstrap_state.rs:13-38`
- `app/agent-host/src/session_runtime/session_handle.rs:1384-1414`
- `app/agent-host/src/session_runtime/state.rs:1085-1109`

Decision: builders produce fully valid immutable host/session inputs. Identity
and callback requirements are constructor fields. Mutable engine settings do
not contain session identity.

### V2-R11: obsolete public output code and accepted no-op commands remain

Verdict: confirmed cleanup defect.

Evidence:

- `agent-host::output` has no production callers in the workspace;
- legacy CLI output-format flags are rejected rather than wired to it;
- it performs `&text[..500]`, which can panic on a non-ASCII UTF-8 boundary and
  violates repository string-slicing policy;
- multiple CLI subcommands only print placeholder messages and return success.

Decision: delete dead output code and unsupported commands/flags. Implemented
headless rendering belongs in the headless surface crate and uses UTF-8-safe
utilities.

## Test-gap verification

The sixteen production multi-session scenarios remain useful, but they do not
cover all v2 findings above. Phase A has since added and compiled targeted
regressions for close/delete byte preservation, close timeout cleanup, no-late
session events after successful close, and close-during-turn accounting; those
regressions still need to be run in the next batched test pass.

- CLI tests cover the pure execution-plan matrix, but not full terminal
  integration behavior under a real TTY;
- SDK zero-session startup now has a focused regression;
- the production multi-session suite has no Event Hub scenario;
- no seam check forbids `coco-agent-host -> coco-tui`;
- no API gate prevents public raw session locks.

Therefore the previous test counts cannot support the claim that the overall
architecture was complete.

## Overall assessment

The v1 registry, authority, and selected-runtime work should remain. The next
refactor should not introduce a whole-runtime actor or speculative shared
services. The shortest path to a clear architecture is:

1. make close/delete and task ownership correct;
2. make mode selection and startup explicit;
3. make all surfaces use one lifecycle;
4. repair dependency direction;
5. narrow capabilities and reorganize modules;
6. delete obsolete code and rewrite current-state documentation.
