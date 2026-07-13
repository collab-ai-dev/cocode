# V2 Remediation Plan

Status: in progress on 2026-07-13.

This plan implements [target-architecture.md](target-architecture.md) and the
breaking protocol in [protocol-scope.md](protocol-scope.md). Backward
compatibility is not a constraint.

## Execution principles

- Fix lifecycle correctness before moving files.
- Add characterization/regression tests with each behavior change.
- Do not add a whole-runtime actor.
- Do not create compatibility aliases for removed requests, flags, or modules.
- Move consumers before making internals private.
- One end-to-end invariant per change; touching multiple crates is acceptable
  only when that invariant or generated protocol artifacts require it.
- Use `just quick-check` at phase boundaries.
- Run `just pre-commit` once, after the final change and before the final
  commit, following repository policy.
- No phase is complete based on test counts; each gate below is semantic.
- Do not mix behavioral fixes, file moves, API cleanup, and dead-code removal in
  one change.

## Convergence reset

Effective with the follow-up adversarial review on 2026-07-13, this plan is
executed as three serial workstreams. The existing Phase letters remain as
historical/status labels; they no longer define implementation order. The
workstream order below is normative.

| Workstream | Included work | Entry gate | Exit gate |
|---|---|---|---|
| 1. Correctness stabilization | CS-1 start authority/protocol truth; CS-2 turn history/terminal ordering; CS-3 close/replace/task ownership; CS-4 Hub final local-egress retirement | Current production baseline and one failing adversarial regression for the selected invariant | All four invariants pass their semantic gates; protocol and lifecycle completion meanings are frozen |
| 2. Surface boundary | Phase G DTO moves, `app/cli/src/{tui,headless,sdk}`, removal of agent-host -> TUI | Workstream 1 complete | Dependency seams pass and lifecycle behavior is unchanged |
| 3. Internal cleanup | Phase H capability/module work and Phase I obsolete-code removal | Workstream 2 complete | Public facade, construction, task ownership, module, and documentation gates pass |

There is no parallel execution between workstreams. In particular, no Phase G
file move begins while any CS invariant is open, and no Phase H API/module
cleanup begins while the surface dependency direction is still changing.

### Correctness stabilization

The only blocking invariants are:

1. **CS-1 Start is new-only and honest.** Remote start cannot select or mutate
   an existing identity, and every accepted initialize/start field is consumed
   or rejected.
2. **CS-2 Turn completion is authoritative.** Final history/accounting and all
   owned turn tasks are committed/drained before terminal delivery and next-
   turn admission.
3. **CS-3 Lifecycle completion owns teardown.** Close/replace/shutdown use one
   owner and one deadline, report post-commit failure, and leave no surviving
   registered work.
4. **CS-4 Hub retirement preserves final egress.** Live and retiring Closing
   membership/cursors cover the final bounded local-egress handoff.

CS-1 closes its new-only authority and accepted-field sub-gates separately.
CS-3 is also three separately landed sub-gates, never one batch:

- **CS-3a:** close uses one deadline and joins registered session work;
- **CS-3b:** every replace branch owns source close and reports post-commit
  failure;
- **CS-3c:** process shutdown retains, cancels, aborts, and joins lifecycle
  owner tasks.

For each CS item:

1. add a failing adversarial regression that demonstrates the invariant breach;
2. name the one lifecycle owner and exact completion point in the change;
3. make the smallest behavioral change that closes the regression;
4. run the focused cross-layer suite and the semantic gate;
5. mark the item closed before selecting another CS item.

During this workstream:

- no crate extraction, directory move, broad rename, module split, dead-code
  cleanup, or unrelated CLI/transport work is allowed;
- a process-local need cannot widen a serialized remote DTO;
- green component tests do not override a failing end-to-end ordering test;
- a newly found issue enters this workstream only if it violates CS-1 through
  CS-4. Otherwise it is recorded for Workstream 2, Workstream 3, or a later
  project;
- an unrelated security or data-loss defect is handled as a separately scoped
  hotfix. It may pause this plan, but it does not silently enlarge a CS gate;
- changing `target-architecture.md` or `protocol-scope.md` requires evidence
  that one of the four frozen invariants is internally inconsistent, not merely
  that another cleanup would be useful.

The landed portions of Phase C (execution plan), Phase D (zero-session host),
and Phase F (shared lifecycle entry points) are frozen baseline. They are
reopened only by a regression that violates CS-1 through CS-4. Additional TTY,
WebSocket, Windows named-pipe, output cleanup, and module-size work is not a
correctness blocker.

## Dependency order

```text
CS-1 Phase 0: start authority + protocol field truth
  -> CS-2 Phase B: terminal history/accounting ordering
  -> CS-3 remaining Phase A: close/replace/task ownership
  -> CS-4 remaining Phase E: final local-egress retirement
  -> Workstream 2 / Phase G: direct CLI surface directories + dependency repair
  -> Workstream 3 / Phase H: in-place capability/module hardening
  -> Workstream 3 / Phase I: obsolete-code removal and final documentation
```

Do not execute the remaining sections in file order: after Phase 0/CS-1, go to
Phase B/CS-2, then return to the remaining Phase A/CS-3 work, then Phase E/CS-4.
The file order preserves earlier migration history and status notes only.

## Phase 0 / CS-1: close the start-authority and protocol-truth holes

This phase is the new first blocking phase. Moving files while start can mutate
an existing session would preserve a critical authority defect behind cleaner
module names.

### 0.1 Make remote start new-only

Changes:

1. remove serialized `session_id`, `initial_messages`, and `initial_prompt`
   from `SessionStartParams` and generated SDK/schema artifacts;
2. reject those legacy/unknown remote fields as invalid params rather than
   ignoring them;
3. mint the remote identity in the server lifecycle owner;
4. add a new-only registry reservation that accepts only Missing and never
   returns an existing Live handle for start;
5. apply configuration/history only to the newly constructed unpublished
   runtime, then atomically promote and attach;
6. use resume/replace for existing identities;
7. add a non-serialized `LocalStartSeed { session_id, initial_messages }` only
   for process-local tests/embeddings, also requiring Missing.

### 0.2 Make every protocol field honest

Changes:

1. keep connection capabilities/resources in `initialize`;
2. move cwd/model/permission/budget/turn limits/system-prompt/schema/plan-mode
   policy to `session/start` and consume it in the session fold;
3. remove duplicate session policy from initialize rather than inventing
   precedence rules;
4. require `surface_id` in successful start and resume results;
5. add an accepted-field audit requiring a production consumer or explicit
   validation rejection for every DTO field.

Tests and gate:

- a remote start carrying legacy `session_id`/`initial_messages` fails invalid
  params and cannot touch another live/orphan identity;
- an internal chosen-id start against Loading/Live/Closing fails with zero
  config, accounting, history, routing, and hook side effects;
- start failure leaves no registry/routing entry;
- each accepted start/initialize field has a behavioral consumption test;
- start/resume success cannot deserialize without `surface_id`;
- schemas plus Python/TypeScript generated artifacts contain no removed fields.

## Phase A / CS-3: finish close/delete/replace task ownership

CS-3 does not reopen the landed close/delete protocol split or transcript
semantics. Its active scope is limited to CS-3a single-deadline task joining,
CS-3b replace completion/failure ownership, and CS-3c process lifecycle-owner
joining.

Implementation status, 2026-07-13:

- Landed in Rust production code and SDK artifacts: `SessionCloseParams`,
  `SessionCloseTarget`, `SessionDeleteParams`, `session/close`,
  `session/delete`, typed local/remote client methods, removal of the
  `session/archive` dispatcher/handler path, close-time transcript preservation,
  storage-only delete, live-slot delete rejection with `SessionStillLive`
  error data, regenerated JSON schemas, regenerated Python/TypeScript protocol
  types, Python `close_session`/`delete_session`, and TypeScript target-aware
  turn/control requests.
- Landed in AppServer internals: routing and close-completion terminology now
  uses `close_session_surfaces`, `CloseSessionSurfacesOutcome`, and
  `AppCloseCommit` instead of archive-oriented names. The remaining close path
  no longer conflates "close live routing" with "delete persisted storage".
- Landed in close-owner error propagation: close completion now carries
  `Result<(), RegistryError>`, active-turn drain timeouts abort and await the
  affected task, then return structured `session_close_timeout` error data
  (`session_id`, `task`, `timeout_ms`) instead of reporting close success.
- The focused validation run passed: `coco-app-server` ran 92 tests and
  `coco-agent-host` ran 331 unit tests, 26 multi-session integration tests, and
  one WebSocket test. These include the added close/delete preservation,
  timeout abort, no-late-event, and close-during-turn regressions.
- The follow-up audit still found broader structured-concurrency gaps: active
  turn and forwarder waits each receive the full timeout, not one absolute
  deadline; session integrations are not all registered with one task owner;
  and process lifecycle-owner tasks are spawned without retained join handles.
  Phase A remains partial until those semantic gates are implemented.
- TypeScript SDK validation passed in the batched check run:
  `npm run generate:check` and `npm run check` in `coco-sdk/typescript`.
- Catalog refresh notification policy is closed for Phase A: `session/delete`
  does not emit live-session lifecycle or catalog-refresh notifications because
  there is no passive session-catalog subscription protocol. Clients invalidate
  their own `session/list` cache after a successful delete response. A future
  passive catalog feature must add a dedicated subscription/event.

### A1. Protocol DTOs

Changes:

1. replace `SessionArchiveParams`/`ArchiveTarget` with
   `SessionCloseParams`/`SessionCloseTarget`;
2. add `SessionDeleteParams { target: SessionTarget }`;
3. add request methods `session/close` and `session/delete`;
4. remove `session/archive` from DTOs, clients, schemas, generated SDKs, and
   dispatch;
5. add stable `SessionStillLive` and close-timeout error data.

Primary areas:

- `common/types/src/client_request.rs`
- schema/code-generation outputs
- `app/server-client`
- Python SDK generated protocol/tests

### A2. One close owner

Changes:

1. delete handler-side active-turn draining and transcript deletion;
2. move all close sequencing into the registry close owner callback;
3. retain `ActiveTurnHandles` until both tasks finish or are aborted/awaited;
4. compute one absolute deadline from `server.turn_drain_timeout_secs`; every
   sequential wait receives only its remaining duration;
5. register session background tasks/integrations in one narrow supervisor and
   stop/await them under that deadline;
6. commit accounting/history before final `SessionResult`;
7. emit no event after close completion;
8. return close failures rather than logging and reporting success;
9. retain process-level lifecycle owner handles so shutdown can cancel, abort,
   and join them.

Primary areas:

- `app/agent-host/src/session_close.rs`
- `app/agent-host/src/app_server_host/request_handlers/session/delete.rs`
- `app/agent-host/src/app_server_host/session_close.rs`
- `app/agent-host/src/session_runtime/{turn,session_handle}.rs`
- `app/server/src/{registry,app_server}.rs`

### A3. Durable delete

Changes:

1. implement delete as storage-only behavior;
2. reject Loading/Live/Closing sessions;
3. document and test which auxiliary artifacts are deleted;
4. propagate storage errors;
5. do not emit catalog refresh notifications until a dedicated passive
   catalog subscription protocol exists.

### A4. One replace owner

Changes:

1. remove the bare spawned source-close branch for an already-live
   destination;
2. run every replace variant in an AppServer-owned task with an owner guard and
   retained completion;
3. complete the request only after destination commit and source close;
4. return pre-commit failure with the source intact;
5. return typed `CommittedCloseFailed` data with the committed destination for
   post-commit close failure; never log it as success;
6. make panic cleanup resolve Loading/Closing slots and waiters.

### Tests and gate

- close preserves an existing JSONL byte-for-byte except allowed close metadata;
- delete removes the transcript only after close;
- delete while Loading/Live/Closing returns `SessionStillLive`;
- forced turn-task timeout aborts and joins the task;
- forced forwarder timeout aborts and joins the task;
- no event or transcript append occurs after close response/completion;
- closing during a turn includes the terminal turn in accounting;
- close uses configured timeout;
- turn plus forwarder drain cannot consume twice the configured timeout;
- every registered session/lifecycle task has terminated when close succeeds;
- orphan and interactive close use the same cascade;
- storage failure is returned to the client;
- replace source-close timeout/failure is returned, including after commit;
- cancellation or panic in every replace branch leaves no wedged slot or
  unresolved completion.

Run affected crate tests, schema generation checks, then `just quick-check`.

## Phase B / CS-2: make turn completion authoritative

CS-2 does not add another result DTO or redesign the query engine. Its active
scope is the one-owner Finishing sequence: drain engine/forwarder, commit final
history/accounting, deliver terminal output, then admit the next turn.

Implementation status, 2026-07-13:

- Started: AppServer turn forwarding now attaches the engine's per-turn
  `SessionResult` to the terminal `TurnEnded` before delivering that terminal
  event to surfaces. Local AppServer `start_turn_and_wait_for_end` returns that
  embedded result along with `TurnStartResult` and `TurnEndedParams`. Headless
  uses the completion result directly; the short polling loop over
  `current_session_result` and the fabricated fallback success result have been
  removed. `SessionTurnCoordinator` now uses an explicit internal
  `Idle`/`Running`/`Finishing` lifecycle enum, and AppServer marks a turn
  `Finishing` when terminal delivery is pending. Terminal delivery now clears
  the slot through a Finishing-only completion method; a generic active-turn
  clear path no longer exists. Close/drain takeover now distinguishes
  `Running` from `Finishing`: `Running` is cancelled before drain, while
  `Finishing` is awaited without issuing a new cancellation. AppServer shortcut
  producers now follow the same terminal contract: synchronous shortcuts that
  return a `turn_id` emit `turn/started` and terminal `turn/ended`, terminal
  `TurnEnded` carries per-turn `session_result`, and shortcut results are
  folded into runtime-owned session accounting. The AppServer turn forwarder
  accepts either an engine-style standalone per-turn `SessionResult` or an
  embedded `TurnEnded.session_result` as the accounting source, and avoids
  double-counting when both are present. Manual `/compact` now returns its
  `CompactOutcome` through the runtime seam and emits a typed per-turn result
  before the terminal event. Local AppServer
  `start_turn_and_wait_for_end` now rejects a terminal event that lacks
  `session_result` instead of rebuilding a result from a live runtime snapshot;
  a focused local-bridge regression covers that fail-fast behavior. The Python
  typed structured-output helper now reads `structured_output` from
  `TurnEnded.session_result` as well as final standalone `session/result`, so
  SDK structured output no longer depends on per-turn standalone
  `session/result` events. Query-engine direct no-tool success exits now delay
  `TurnEnded(Completed)` until the outer lifecycle seam has built
  `SessionResultParams`, then emit `TurnEnded.session_result` and the final
  standalone `session/result` from the same metadata. AppServer forwarding
  de-duplicates accounting when both embedded and standalone per-turn results
  are present. Query-engine direct tool-call success exits now use the same
  pending-terminal seam, so multi-round/tool-call completions also emit
  `TurnEnded.session_result` matching the final `session/result`. Remaining
  direct query terminal producers outside the AppServer forwarder path now use
  the same outer lifecycle seam: failure, max-turns, token-budget, USD-budget,
  blocking-limit, image-size, and structured-output retry-cap terminals return
  typed `TurnEndedParams`; `engine_session` attaches the final
  `SessionResultParams` and remains the only query-engine production sender of
  terminal turn events.
- Still open: tighten any remaining coordinator transitions around the explicit
  state machine, remove remaining duplicate aggregation paths, and add the full
  Phase B terminal-ordering tests.
- Adversarial recheck: the current event forwarder clears Finishing before it
  forwards `TurnEnded`, while `SessionTurnExecutor` commits final engine history
  only after the engine returns. Embedding a result in `TurnEnded` therefore did
  not finish Phase B; an immediate next turn can still observe stale history.

Changes:

1. enrich/factor `TurnEnded` so it carries the complete per-turn result required
   by headless/SDK;
2. define coordinator states Idle/Running/Finishing explicitly;
3. commit history/accounting before terminal delivery;
4. make local/remote interactive clients expose
   `start_turn_and_wait_for_end` using turn-id correlation;
5. remove `current_session_result` polling and fallback fabrication;
6. require every AppServer `turn/start` path that returns a `turn_id` to emit
   exactly one terminal event carrying per-turn `session_result`;
7. remove duplicate aggregation paths that can publish terminal state early.
8. make the turn lifecycle owner the only component that can enter/leave
   Finishing; event forwarding cannot clear the coordinator;
9. drain the engine/forwarder, commit final history/accounting, deliver terminal
   output, and only then return to Idle.

Primary areas:

- `common/types` turn/result DTOs
- `app/agent-host/src/session_runtime/turn.rs`
- `app/agent-host/src/app_server_host/request_handlers/turn.rs`
- local and remote client demux
- headless runner

Tests and gate:

- exactly one terminal result per turn;
- terminal result follows history/accounting commit;
- a next turn sent immediately on terminal receipt sees the completed
  assistant/tool history;
- a fast next turn cannot clear the previous turn's handles;
- headless does not sleep/poll for result availability;
- local AppServer completion fails fast if a terminal event lacks
  `session_result`;
- AppServer shortcut turns emit terminal lifecycle events and fold their
  per-turn result into final close accounting;
- Python SDK typed structured output reads terminal embedded
  `session_result.structured_output`;
- query-engine direct no-tool success emits `TurnEnded.session_result`
  matching the final `session/result`;
- query-engine direct tool-call success emits `TurnEnded.session_result`
  matching the final `session/result`;
- query-engine direct failure and budget terminals emit
  `TurnEnded.session_result` matching the final `session/result`;
- interruption/failure/budget exhaustion preserve typed outcomes;
- close during Finishing waits for terminal delivery.

## Phase C: introduce one CLI execution plan (frozen baseline)

Status: started. `coco-cli` now has a pure `ExecutionPlan` built from
`Cli` plus injectable `IoCapabilities`; `main.rs` and `tracing_init.rs` consume
that shared plan for mode selection. The plan makes `--non-interactive`
select headless, treats `resume` as interactive TUI, records stdin/stdout TTY
state, and removes the unsupported global `--no-tui` and `--json` flags from
the clap schema. Mode-dependent validation for `--no-session-persistence` and
`--plan-mode-instructions` now happens inside fallible pure plan construction.
Placeholder subcommands that only printed success/not-implemented messages
(`daemon`, `logs`, `attach`, `kill`, `remote-control`/`rc`/`bridge`, `sync`,
`upgrade`, and `usage`) are removed from clap and covered by rejection tests.
Headless stdin behavior is explicit: when no `--prompt` is provided and stdin
is not a terminal, stdin is read as the raw prompt; non-terminal stdout only
selects headless presentation. Confirmed CLI-only flags with no runner
consumer were removed from clap and covered by rejection tests. The final
full-workspace run belongs to the final release gate; no Phase C implementation
remains active. The retained top-level clap schema is guarded by an accepted-
field consumption audit test that requires every accepted flag to have an
explicit consumer or execution-plan policy.

Changes:

1. add `ExecutionPlan` and injectable `IoCapabilities`;
2. move all mode/flag validation into pure plan construction;
3. derive tracing mode from the plan;
4. remove duplicate TTY detection from `main` and tracing setup;
5. define stdin/stdout behavior for headless explicitly;
6. remove unsupported/no-op flags including `--no-tui` and `--json` unless
   their behavior is implemented in this phase;
7. make `--non-interactive` select headless;
8. remove placeholder subcommands that only print success/not-implemented;
9. ensure every remaining flag is consumed by its plan/runner.

Primary areas:

- `app/cli/src/lib.rs`
- `app/cli/src/main.rs`
- `app/cli/src/tracing_init.rs`
- CLI behavioral tests

Tests and gate:

- table-driven matrix for subcommand, prompt, `--non-interactive`, and all
  stdin/stdout TTY combinations;
- exactly one plan for every accepted invocation;
- incompatible flag combinations fail before host construction;
- tracing mode equals execution mode;
- repository check reports no accepted but unconsumed CLI field;
- removed placeholder commands fail clap parsing.

## Phase D: build a valid process host with zero sessions (frozen baseline)

Implementation status, 2026-07-13:

- Landed the zero-session SDK startup slice. `HostBuilder::prepare` now builds
  process/bootstrap state, AppServer routing, and a runtime factory without
  constructing a startup `SessionHandle`.
- `RuntimeReplacementContext` no longer carries `startup_session_id`.
- `session/start` and `session/resume` now use the normal AppServer load path
  for the first real session instead of replacing a detached placeholder.
- SDK remote startup no longer fires session-start/setup hooks, creates MCP
  integrations, or installs a session manager from a hidden runtime before a
  client lifecycle request.
- `HostInputs` now installs startup cwd, initialize bootstrap,
  startup session manager, and startup bypass availability at
  `AppServerHostState` construction time. `BootstrapState` now stores immutable
  startup inputs directly instead of `RwLock<Option<_>>`, and the startup
  `try_write`/panic paths for bootstrap/session-store installation have been
  removed.
- `RuntimeReplacementContext` is now also supplied through `HostInputs`. The
  runtime-replacement state is immutable after host
  construction instead of being a `RwLock<Option<_>>` populated by a late
  startup install.
- The production remote `TurnRunner` is now supplied through `HostInputs`. The
  remaining `install_turn_runner` path is retained
  for local bridge runtime rebinding and focused tests, not SDK startup
  construction.
- Added a focused regression:
  `host_builder_starts_without_placeholder_session`, which verifies the
  prepared registry is empty and `initialize` succeeds without creating a
  session. Added `constructor_installs_startup_inputs_without_late_mutation`,
  which verifies constructor-provided cwd/bootstrap/session-manager/bypass
  inputs are available without late startup mutation. Added
  `max_sessions_one_allows_first_real_session_without_placeholder`, which sets
  `COCO_SERVER_MAX_SESSIONS=1` and verifies the first real `session/start`
  succeeds because startup did not consume a hidden live-session slot.
- The constructor seam now uses the target terminology: `HostInputs` for
  construction-time process host inputs and `PreparedHost` for the prepared
  remote process host returned by `HostBuilder::prepare`.

Changes:

1. introduce `HostInputs`, `HostBuilder`, and `PreparedHost`;
2. make required startup cwd/config/catalog/session-store/runner inputs
   constructor fields;
3. remove `AppServerHostState::default` plus startup `install_*` sequencing;
4. replace immutable `RwLock<Option<_>>` startup slots with immutable values;
5. remove startup `try_write`/panic paths;
6. remove `startup_session_id` from `RuntimeReplacementContext`;
7. delete SDK placeholder runtime construction and special first-start/first-
   resume replacement logic;
8. keep initialize metadata available from process catalog snapshots;
9. make `SessionFactory` the only runtime construction entry for lifecycle
   owner tasks.

Primary areas:

- `app/agent-host/src/remote_host.rs`
- `app/agent-host/src/app_server_host/{state,bootstrap_state,runtime_replacement}.rs`
- start/resume/replace operations
- `app/sdk-server/src/startup.rs`

Tests and gate:

- preparing SDK host leaves registry empty;
- initialize succeeds without a live runtime;
- initialize has no session hooks/MCP/session storage side effects;
- first start and first resume use normal lifecycle code;
- `max_sessions = 1` permits the first real session without placeholder rules;
- no `startup_session_id` symbol remains in production code.

## Phase E / CS-4: finish Event Hub retirement semantics

CS-4 does not redesign connector transport or process ownership. Its active
scope is limited to a lifecycle revision, Live/retiring-Closing membership,
bounded final local-egress handoff, and the corresponding reconnect-cursor
regressions.

Implementation status, 2026-07-13:

- Landed the main process-owned Event Hub production slice. Agent-host now exposes
  `ProcessEventHub::spawn(runtime_config, cwd, live_sessions)` instead of the
  old `RuntimeEventHubConnector::spawn_for_session` constructor. SDK remote,
  TUI, and headless startup paths start the connector with an explicit
  live-session snapshot, preserving zero-session startup without requiring a
  placeholder runtime.
- The hub connector worker now announces as soon as it starts, even before a
  batch is pending. This makes an empty host visible to the Hub as
  `live_sessions: []` instead of waiting for the first session event.
- Process hosts now run an Event Hub membership watcher over the AppServer
  activity revision stream. On each wakeup it reads
  `AppServer::list_live_sessions()`, compares the session-id snapshot, and
  re-announces membership through a bounded reconnect when the live set
  changes. Local sidecar and SDK stdio writers also sync membership immediately
  before routing a session event to the Hub, so the first event after a
  lifecycle transition does not intentionally outrun its live-set announce.
- Added focused coverage for startup and A/B start membership behavior:
  `worker_announces_on_start_with_empty_backlog`,
  `worker_reannounces_after_membership_update`,
  `process_announce_accepts_empty_live_session_snapshot`, and
  `host_builder_updates_event_hub_membership_from_registry`, which verifies
  empty startup, first start, and second start membership snapshots.
- Adversarial recheck: the watcher uses general activity revisions and
  `list_live_sessions()`, which drops Closing slots before their final
  `SessionResult` is safely represented in egress/reconnect cursor state.
  Phase E needs a lifecycle revision and announced/retiring membership, not
  only more tests around the current snapshot.

Changes:

1. replace `RuntimeEventHubConnector::spawn_for_session` with
   `ProcessEventHub::spawn`;
2. resolve Event Hub endpoint as process-host policy;
3. publish and subscribe to dedicated AppServer lifecycle revisions;
4. announce Live plus retiring Closing membership on connect/reconnect;
5. add/update membership protocol support, or reconnect on membership changes
   if the Hub wire has no update frame;
6. request/restore cursors for every announced or retiring session;
7. keep event envelope identity authoritative and validate it against routing;
8. flush once through shared `ShutdownCoordinator`.

Primary areas:

- `app/agent-host/src/event_hub.rs`
- `hub/connector`
- `hub/protocol` if a membership update frame is added
- AppServer lifecycle observer API

Tests and gate:

- empty host announces zero sessions;
- A/B start announces both identities;
- replace removes source/adds destination;
- close removes the session;
- close keeps the session announced until final local egress handoff completes;
- reconnect requests cursors for all currently live sessions;
- events for A/B retain identity and ack independently;
- no placeholder identity appears;
- connector failure does not block unrelated session turns/close.

## Phase F: unify TUI, headless, and SDK lifecycle (frozen baseline)

Implementation status, 2026-07-13:

- Started the unified lifecycle slice. `AppServerLocalBridge` now exposes
  narrow local lifecycle facades for interactive `session/start`,
  `session/resume`, and in-session resume replacement; the facade sends typed
  client requests, then returns the registered `SessionHandle` plus interactive
  surface.
- The current shared facade temporarily accepts `session_id` and
  `initial_messages` in serialized `session/start`. The lifecycle-owned builder
  does hydrate them, but the follow-up authority audit rejects this API shape:
  Phase 0 replaces it with server-minted remote start plus a non-serialized
  local seed. Production resolved existing identities use resume.
- `RuntimeReplacementContext` now carries `SessionIntegrationOptions`, so TUI,
  headless, and SDK runtime construction install their integration policy inside
  the lifecycle-owned runtime builder instead of in surface startup code.
- TUI startup no longer creates a placeholder fresh runtime before applying a
  resume plan. Fresh startup uses local `session/start`; startup resume/fork
  uses local `session/resume` directly.
- Production headless resume now carries a `resume_target` and uses local
  `session/resume`; fresh headless uses local `session/start`.
- In-session TUI `/resume` and `/branch` now replace the interactive surface
  through the local typed `session/replace` resume facade instead of directly
  registering, hydrating, or swapping runtimes from `coco-cli`.
- `session/replace` now reserves its destination through the AppServer replace
  owner for fresh/non-live resume destinations. Replacement therefore does not
  require an extra live-session capacity slot and works correctly with
  `max_sessions = 1`.
- TUI `/clear` now enters `session/replace` with a typed clear destination.
  AppServer owns source snapshot capture, destination runtime construction,
  clear SessionStart hooks, and `ExitReason::Clear` shutdown of the source.
  The old local clear-specific runtime replacement helper has been removed.
- Main TUI shortcut, observability, command-queue, interrupt/cancel, and
  driver control paths now call `activate_existing_interactive_session`, which
  only attaches an already-live AppServer session and optionally starts the
  passive event pump. These paths no longer pass `SessionHandle` into a bridge
  bind/register method from `coco-cli`.
- Prompt-mode bash still executes the shell command in the background, but it
  now sends response-turn history back to the main driver. The response turn
  starts through the existing local AppServer bridge instead of a short-lived
  bridge that re-registered the runtime.
- `ShutdownCoordinator` now owns the shared AppServer drain plus Event Hub
  membership-watcher stop/flush sequence. Headless uses it directly; the TUI
  driver uses the same coordinator while keeping its final metadata checkpoint
  between AppServer drain and Event Hub flush; SDK remote-host shutdown uses
  it after sidecar/listener shutdown.
- `ProcessRuntime` now exposes an explicit background-task shutdown policy for
  process-owned project services. CLI process exit covers all early-return
  modes through a guard, and SDK remote-host shutdown calls the same policy
  after AppServer/Event Hub drain.
- Shared lifecycle conformance now covers start/read/close and durable
  resume/read/close across the local typed AppServer surface, the direct
  JSON-RPC AppServer bridge, and the concrete Unix NDJSON sidecar binding on
  Unix. SDK stdio coverage lives in `coco-sdk-server`, where the transport
  boundary belongs, and drives the same lifecycle contract through
  `SdkServer::run_app_server_connection` plus `InMemoryTransport`.
- Deferred outside correctness stabilization: extend the concrete sidecar
  matrix beyond Unix NDJSON where relevant, and add production-surface smoke
  coverage only when those adapters are changed. This is not a Phase F or
  Workstream 1 blocker.

Changes:

1. expose local connection initialization/start/resume/replace/close from
   `PreparedHost`;
2. make TUI startup resume call typed `session/resume`;
3. make headless fresh/resume use the same typed lifecycle;
4. remove surface calls to runtime registration, hydration, callback install,
   and direct registry helpers;
5. route surface controls through typed clients;
6. move common shutdown ordering into `ShutdownCoordinator`;
7. keep presentation-specific cleanup outside the coordinator but before/after
   it according to an explicit contract;
8. replace ambiguous `is_non_interactive` with typed interaction/file-history
   policies.

Tests and gate:

- the same lifecycle conformance suite runs against local typed, JSON-RPC
  AppServer bridge, and concrete Unix NDJSON sidecar surfaces for
  start/read/close and durable resume/read/close;
- SDK stdio runs the same lifecycle contract from the SDK transport crate
  without reversing the `agent-host` -> `sdk-server` dependency direction;
- extend sidecar coverage beyond Unix NDJSON only when that transport is
  changed or when platform-specific behavior needs validation;
- resume hydration/history/callback requirements are identical across the
  covered surfaces;
- no production surface calls `SessionFactory` directly;
- no production surface directly mutates registry/runtime state;
- remote surface start cannot select an existing id; local seeded start cannot
  target a non-Missing slot;
- all surfaces return the same typed close failure outcome;
- all surfaces use shared shutdown ordering.

## Phase G / Workstream 2: repair dependency direction

Entry gate: CS-1 through CS-4 are closed and their protocol/lifecycle contracts
are frozen. Phase G changes dependency direction and composition location only;
it must not change request DTO semantics, lifecycle ordering, or completion
outcomes. If a Phase G change exposes a correctness defect, stop Phase G and
return that defect to the applicable CS gate.

### G1. Move shared DTOs down

Changes:

- keep `coco_tui_ui::paste::ImageData { bytes, mime }` inside TUI; it represents
  clipboard/UI input, not a protocol DTO;
- retain existing `coco_types::QueuedCommandEditImage { media_type,
  data_base64 }` for turn-start, queues, and agent-host operations;
- convert `ImageData -> QueuedCommandEditImage` at the TUI boundary;
- defer any `InputImage` rename to a separate protocol cleanup because it is
  not required for dependency inversion and may change schema/codegen names;
- move the message meaning of TUI `SystemPushKind` to `coco-messages` as
  `SystemMessageDraft` with an `into_message` conversion, or use the existing
  `SystemMessage` directly when no draft state is needed;
- keep `UserCommand` in TUI and remove agent-host's dependency through typed
  host-client operations/adapters;
- retain the already-shared `PermissionDisplayInput`; move its input formatter
  from TUI to the owning permission/application layer rather than adding a
  duplicate DTO.

Do not move presentation state, key events, `App`, clipboard bytes, or TUI
commands into `coco-types`/`coco-messages`.

### G2. Move TUI composition to `app/cli/src/tui`

Rename current `app/cli/src/tui_runner` to `app/cli/src/tui` and move from
agent-host only the TUI-specific adapters:

- TUI bootstrap and command driver;
- TUI permission and sandbox bridges;
- voice/TUI application integration;
- teammate action-to-`UserCommand` conversion (mailbox/application logic stays
  in agent-host behind typed operations);
- editor/theme/keybinding surface orchestration.

Target layout:

```text
app/cli/src/tui/
  mod.rs
  bootstrap.rs
  driver.rs
  ...
```

### G3. Move headless policy to `app/cli/src/headless`

Move only executable one-shot policy:

- one-shot run options/outcome;
- input/output formatting;
- headless permission and signal policy;
- headless slash presentation behavior.

Do not move `coco-agent-host::headless` wholesale. First rename/move shared
config, model, system-prompt, permission, bootstrap, and session-factory logic
to neutral private agent-host modules. Replace direct `SessionHandle` use with
the typed host facade; then move the remaining surface runner.

Target layout:

```text
app/cli/src/headless/
  mod.rs
  input.rs
  runner.rs
  output.rs
  signal.rs
```

### G4. Move SDK process policy to `app/cli/src/sdk`

Move `coco-sdk-server::startup::run_sdk_mode` and CLI sidecar-config selection
to:

```text
app/cli/src/sdk/
  mod.rs
  runner.rs
```

Retain `coco-sdk-server` as a separate crate. It continues to own
`SdkTransport`, stdio/in-memory transports, Unix/WebSocket/named-pipe sidecar
implementations, JSON-RPC/AppServer connection adaptation, ordered outbound
writing, and transport conformance tests. It does not own CLI mode selection,
process signals, host construction, or global shutdown policy.

There is no `app/cli/src/surfaces/` directory.

### G5. Freeze the runtime crate boundary

Do not create or extract `coco-agent-runtime` during Phase G. The existing
private runtime remains in `coco-agent-host`; narrowing and organization wait
for Phase H. Reconsider a crate extraction only after this refactor, and only
when a real second consumer or measured compile/dependency benefit exists;
extraction must move one implementation, never duplicate agent-host.

Dependency gates:

- `coco-agent-host` has no TUI dependency;
- `coco-agent-host` has no `coco-sdk-server` dependency;
- `coco-sdk-server` has no `coco-cli` dependency and retains independent
  transport tests;
- `app/cli/src/{tui,headless,sdk}` use typed host/transport facades and do not
  receive raw session locks;
- the existing server-client independence seam remains green;
- no dependency cycle is introduced.

Run each affected crate suite, dependency seam scripts, then `just quick-check`.

## Phase H / Workstream 3: narrow capabilities and reorganize modules

This phase reorganizes `coco-agent-host` in place after the semantic phases are
green. It must not become a behavior rewrite plus crate move in one change.
Its entry gate is the completed Phase G dependency seam. Public behavior and
protocol schemas are frozen during this phase.

Changes:

1. move `SessionId` out of mutable `QueryEngineConfig`;
2. make callback requirements mandatory session-construction input;
3. replace public arbitrary config-mutation closures with typed controls;
4. replace public raw locks/managers with snapshots and narrow operations;
5. split `SessionHandle` by responsibility while keeping runtime private;
6. make modules private by default;
7. group files under `session`, `integrations`, `host`, `protocol`,
   `lifecycle`, and `client` modules;
8. merge tiny pass-through modules that do not own an invariant;
9. split modules above the repository size target by cohesive behavior;
10. reduce `lib.rs` to intentional facade exports;
11. keep `SessionRuntime` private to `coco-agent-host::session`;
12. organize and privatize the task supervisor already landed by CS-2/CS-3;
    do not introduce new task semantics or turn the runtime into an actor.

Mechanical gates:

- no public session API returns `Mutex`, `RwLock`, internal registries, or
  mutable manager handles;
- no public `runtime()` or `Deref`;
- no mutable engine config contains `SessionId`;
- no late callback-requirement install;
- retained CS-2/CS-3 task-owner regressions remain green;
- public API snapshot reviewed explicitly;
- module-size report has no unexplained file above target;
- rustdoc describes owner/lifetime for every exported capability.

## Phase I / Workstream 3: remove obsolete code and align documentation

Changes:

1. delete unused `agent_host::output` and use UTF-8-safe rendering in
   `app/cli/src/headless`;
2. remove stale startup/runtime replacement helpers and compatibility comments;
3. remove obsolete symbols/files referenced by old plans;
4. update crate `CLAUDE.md` files and root architecture table;
5. verify generated schemas remain unchanged and update SDK examples/docs only
   to reflect already-landed protocol behavior;
6. convert this plan to a completed migration record only after every semantic
   gate passes;
7. rewrite `current-architecture.md` from the landed tree.

Tests and gate:

- no removed architecture symbols remain;
- no computed raw string byte slicing in changed production paths;
- docs contain no contradictory current/target claims;
- `git diff --check` passes;
- `just quick-check` passes.

## Final release gate

Close and record each workstream independently. Completing Workstream 1 does
not wait for directory/module cleanup; starting Workstream 2 does require the
recorded Workstream 1 result. A closed workstream is reopened only by a failing
regression against its explicit gate.

After all three workstreams are complete and no code changes remain:

1. run focused lifecycle, CLI, Hub, surface conformance, and dependency seam
   suites;
2. run schema/code-generation checks and SDK tests;
3. run `just quick-check`;
4. run `just pre-commit` exactly once;
5. record results by semantic gate, not only test count.

## Definition of done

Workstream 1, correctness stabilization:

- close and delete are separate, explicit, tested operations;
- close completion proves no surviving session work or late events;
- terminal history/accounting is committed before terminal delivery and next
  turn admission;
- remote start cannot select or mutate an existing identity;
- every accepted protocol field is consumed or rejected;
- CLI has one execution plan and no accepted no-op flags/commands;
- process host startup creates zero sessions;
- Event Hub membership covers Live and retiring Closing sessions through final
  local-egress handoff;
- all surfaces use the same lifecycle and shutdown coordinator;
- retained multi-session authority/isolation tests remain green.

Workstream 2, surface boundary:

- surface composition lives directly in `app/cli/src/{tui,headless,sdk}`;
- `coco-sdk-server` remains the transport adapter crate, with CLI startup policy
  outside it;
- agent-host has no TUI dependency;
- Workstream 1 lifecycle/authority regression results are unchanged.

Workstream 3, internal cleanup:

- agent-host runtime remains private in place;
- session identity/callback requirements are construction-time invariants;
- public capabilities expose no internal locks/managers;
- module layout communicates ownership;
- final documentation describes the landed tree accurately.
