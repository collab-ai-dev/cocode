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
4. start DTO -> new/existing registry slot -> mutation -> surface attachment;
5. initialize/start accepted fields -> validation/production consumers;
6. start/resume/replace/close -> registry owner task -> runtime teardown;
7. turn start -> engine history commit -> active-turn ownership -> terminal
   result/event ordering;
8. session close/delete -> `SessionManager` -> JSONL behavior;
9. AppServer event -> Hub connector announce/reconnect/batch path;
10. crate manifests, `lib.rs` exports, module layout, and public capability APIs;
11. existing tests -> claimed completion properties.

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
forwarder-task timeout plus successful-close no-late-session-event regressions
now run and pass in the focused agent-host suite. R15 records the remaining
broader task-supervisor and single-deadline gap.

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
after the runtime close cascade. The in-flight close accounting/order
regression runs and passes; it verifies the final close `SessionResult` includes
the in-flight turn's accounting. R13 is a separate normal turn-completion
history race and is not disproved by this close regression.

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
Shared lifecycle conformance now spans local typed, direct JSON-RPC, concrete
Unix NDJSON sidecar, and SDK stdio transport paths for the core
start/read/close plus durable resume/read/close contract. Remaining coverage
gaps are narrower: WebSocket and Windows named-pipe sidecars are not in the
matrix, and exact production TUI/headless startup adapter smoke tests should
only be added when those adapters move again.

The serialized `session_id`/`initial_messages` workaround is not retained in
the target. R12 shows that it weakens start authority. Local seeded construction
must be a non-serialized internal seam, while production restoration uses
resume.

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

Decision: keep `coco_tui_ui::paste::ImageData { bytes, mime }` TUI-internal and
convert it at the boundary to the existing base64
`coco-types::QueuedCommandEditImage`. Do not rename that wire type during Phase
G; the rename is unrelated protocol/schema cleanup. Move `SystemPushKind`'s
message meaning to `coco-messages` as `SystemMessageDraft` (or use
`SystemMessage` directly), while `UserCommand` stays TUI-only. Permission
display input is already shared; its formatting moves to the
application/permission layer rather than creating another DTO.

TUI-only bridges and the driver move to `app/cli/src/tui`. There is no
`surfaces/` directory and no `coco-tui-runner` crate.

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
- `SessionHandle` is 1,539 lines with roughly 186 public methods;
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

- production remote startup now supplies several required values through
  `HostInputs`, but `HostInputs`/state still permit optional/defaulted services
  and local/test seams replace runner/session-manager state after construction;
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
headless rendering belongs in `app/cli/src/headless` and uses UTF-8-safe
utilities.

### V2-R12: remote start can mutate an existing session before authority

Verdict: confirmed, critical.

Evidence:

- serialized `SessionStartParams` exposes optional `session_id` and typed
  `initial_messages`;
- generic `spawn_load` returns the existing Live handle or joins an existing
  Loading operation for that id;
- start then installs runtime state and applies model/permission/accounting
  mutation before interactive surface attachment validates ownership;
- the eventual owner conflict therefore does not undo the mutation. A live
  orphan can also be attached through start instead of resume's callback-
  compatibility checks.

Primary paths:

- `common/types/src/client_request.rs:542-570`
- `app/server/src/app_server.rs:774-825`
- `app/agent-host/src/app_server_host/session_start_operation.rs:94-116`
- `app/agent-host/src/session_start.rs:73-87`

Counter-hypothesis: only trusted process-local callers set `session_id`.

Refuted: the field is serialized, included in schemas/SDK DTOs, and handled by
the same remote request path. Trust is not encoded in the type boundary.

Decision: remote start mints an id and requires Missing. It has no serialized
id/history. A non-serialized `LocalStartSeed` supports narrow test/embedding
needs and still requires Missing. Existing identities use resume/replace.

### V2-R13: terminal delivery precedes final history commit

Verdict: confirmed, critical conversation-integrity race.

Evidence:

- query engine emits `TurnEnded` and per-turn `SessionResult` before returning;
- the AppServer event forwarder marks the turn Finishing and clears the active
  coordinator slot before it forwards `TurnEnded`;
- `SessionTurnExecutor` commits `result.final_history` only after the engine
  returns and its inner forwarder is joined;
- a client that immediately sends the next turn after seeing `TurnEnded` can
  therefore be admitted against stale history.

Primary paths:

- `app/query/src/engine_session.rs:331-367`
- `app/agent-host/src/app_server_host/request_handlers/session/events.rs:126-157`
- `app/agent-host/src/app_server_host/session_turn_executor.rs:336-355`

Decision: one turn lifecycle owner keeps admission closed, drains forwarding,
commits history/accounting, joins owned tasks, delivers the terminal event, and
only then returns the coordinator to Idle. Event forwarding does not mutate the
turn lifecycle.

### V2-R14: protocol accepts fields it does not implement

Verdict: confirmed, high.

Evidence:

- start declares `max_turns`, `max_budget_usd`, system-prompt variants, and
  `initial_prompt`, but preparation currently consumes only id/cwd/model/
  permission/history plus connection-derived fields;
- initialize also declares JSON schema and system-prompt variants, duplicating
  session policy without a defined precedence or complete consumer;
- `SessionStartResult.surface_id` and `SessionResumeResult.surface_id` are
  optional, preserving fallback behavior despite successful lifecycle calls
  requiring an interactive attachment.

Primary paths:

- `common/types/src/client_request.rs:330-374,542-570`
- `app/agent-host/src/session_start.rs:40-87`
- `common/types/src/server_request.rs:669-690`

Decision: initialize keeps connection capabilities/resources; start owns all
per-session execution policy. Remove `initial_prompt` and send prompts through
`turn/start`. Every accepted field is validated and consumed or rejected, and
start/resume return required surface ids. Backward compatibility is not kept.

### V2-R15: close/shutdown do not own all spawned work under one deadline

Verdict: confirmed, high.

Evidence:

- active turn and forwarder each receive the full drain timeout sequentially,
  so the operation may take twice its declared budget;
- session close explicitly drains active turn/reload/hook paths, but a
  cancellation token is not proof that every session-owned integration task
  has been joined;
- AppServer lifecycle owners are spawned without retained process-level join
  handles;
- query hook-forwarder timeout cancels and drops the moved join handle without
  aborting and awaiting it.

Primary paths:

- `app/agent-host/src/session_runtime/session_handle.rs:1114-1142`
- `app/server/src/app_server.rs:774-825,828-1060`
- `app/query/src/engine_session.rs:240-258`

Decision: add a narrow session/lifecycle task supervisor, not a runtime actor.
Use one absolute deadline; on expiry cancel, abort all, and join. Process
shutdown retains and drains lifecycle-owner handles before reporting success.

### V2-R16: replace can report success before source close is owned

Verdict: confirmed, high.

Evidence:

- the already-live destination branch commits routing and launches source close
  with a bare `tokio::spawn`, retaining neither owner guard nor completion;
- panic/failure can strand Closing state, and the caller returns success
  immediately;
- regular replacement waits on a registry completion whose promotion can occur
  before source close failure is surfaced.

Primary paths:

- `app/agent-host/src/app_server_host/session_replace_operation.rs:215-266,279-310`
- `app/server/src/app_server.rs` replace owner/completion implementation

Decision: every replace branch has one AppServer-owned owner. Pre-commit
failure preserves the source. Post-commit source-close failure returns a typed
committed-but-close-failed outcome carrying the destination; it cannot roll
back, but it cannot be reported as clean success.

### V2-R17: Hub retires membership before final close egress is durable

Verdict: confirmed, high reconnect/cursor risk.

Evidence:

- membership watches general session activity and derives only
  `list_live_sessions()`;
- that list filters out Closing slots;
- final `SessionResult` is emitted inside the Closing owner after the slot has
  already disappeared from announced membership;
- a reconnect between those operations can negotiate no resume cursor for the
  closing session.

Primary paths:

- `app/agent-host/src/event_hub.rs:122-195`
- `app/server/src/app_server.rs:663-686`
- `app/agent-host/src/app_server_host/session_close.rs:150-180`

Decision: publish a dedicated lifecycle revision and retain announced/retiring
membership for Closing sessions until the final local egress handoff completes.
Reconnect snapshots and cursor requests include that retiring set.

### V2-R18: prescribed crate extraction is larger than the ownership problem

Verdict: confirmed architecture overreach.

The prior plan proposed `coco-agent-runtime`, `coco-tui-runner`, and
`coco-headless`. Agent runtime currently has one real application owner, so a
new crate would mostly relocate the broad `SessionHandle` API before it is
narrowed. The two surface crates would wrap executable-only composition already
owned by `coco-cli`.

Decision: refactor agent-host in place. Use direct
`app/cli/src/{tui,headless,sdk}` directories, with no `surfaces/` layer. Move
only `run_sdk_mode`/CLI startup policy from `coco-sdk-server`; retain that crate
for reusable transports, ordered writing, sidecars, and JSON-RPC connection
adaptation. Reconsider runtime extraction only with a real second consumer or
measured dependency benefit.

## Test-gap verification

The production multi-session scenarios remain useful, but they do not cover all
v2 findings above. During this audit, `just test-crate coco-app-server` passed
92 tests and `just test-crate coco-agent-host` passed 331 unit tests, 26
multi-session integration tests, and one WebSocket test. This confirms the
current regressions run; it does not prove the missing cross-owner orderings.

- CLI tests cover the pure execution-plan matrix, but not full terminal
  integration behavior under a real TTY;
- SDK zero-session startup now has a focused regression;
- focused Hub tests cover empty/A/B startup membership, but not close/replace/
  reconnect final-event cursors;
- no test starts a second turn immediately on terminal receipt and verifies
  committed tool/assistant history;
- no hostile connection starts with another session's id and asserts zero
  mutation;
- no audit proves every accepted initialize/start field has a consumer;
- replace panic/source-close failure and full task-supervisor drain are not
  covered;
- no seam check forbids `coco-agent-host -> coco-tui`;
- no API gate prevents public raw session locks.

Therefore the previous test counts cannot support the claim that the overall
architecture was complete.

## Convergence diagnosis

The refactor lost monotonic progress because it combined correctness,
surface/dependency migration, and internal cleanup while their contracts were
still changing. Several locally reasonable changes demonstrate the pattern:

- a local lifecycle-unification need widened serialized start and created R12;
- terminal DTO delivery improved, but its local completion point preceded the
  final history commit in R13;
- replace promoted the destination, but request completion did not own source
  close in R16;
- dynamic Hub live membership replaced the static placeholder, but omitted the
  Closing/final-egress interval in R17;
- speculative crate boundaries were designed before the host facade was narrow
  enough to support them in R18.

The control failure is therefore not the number of findings. It is allowing a
new finding or adjacent cleanup to change the active target before the previous
end-to-end invariant was closed. Green component suites and local completion
signals made partial progress look final.

The review now applies these triage rules:

1. only violations of CS-1 through CS-4 block correctness stabilization;
2. each blocking finding gets one failing adversarial regression, one named
   owner, and one completion definition before implementation;
3. dependency/file moves wait for all four correctness gates;
4. API/module/dead-code cleanup waits for the dependency seam;
5. findings outside the active workstream are recorded but do not expand its
   exit criteria.

## Overall assessment

The v1 registry, authority, and selected-runtime work should remain. The next
refactor should not introduce a whole-runtime actor, speculative shared
services, or a new runtime crate. Execution is deliberately serial:

1. close CS-1 start, CS-2 turn, CS-3 teardown, and CS-4 Hub retirement, one at
   a time;
2. freeze lifecycle/protocol behavior and complete Phase G surface dependency
   repair;
3. freeze dependency direction and complete Phase H/I internal cleanup.

The execution-plan, zero-session startup, and shared lifecycle entry work stays
as frozen baseline. Additional platform coverage and unrelated cleanup are not
allowed to delay the first workstream.
