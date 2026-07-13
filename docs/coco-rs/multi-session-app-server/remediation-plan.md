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
- One changed package or coherent package batch per commit.
- Use `just quick-check` at phase boundaries.
- Run `just pre-commit` once, after the final change and before the final
  commit, following repository policy.
- No phase is complete based on test counts; each gate below is semantic.

## Dependency order

```text
close/delete correctness
  -> terminal turn ordering
  -> typed CLI execution plan
  -> zero-session host builder
  -> registry-driven Event Hub
  -> unified surface lifecycle
  -> TUI dependency inversion
  -> agent-runtime extraction
  -> capability/module hardening
  -> obsolete-code removal and final documentation
```

## Phase A: split close from delete and restore task ownership

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
- Previously tested before the latest timeout-test source addition: protocol
  target/scope checks, the full multi-session AppServer suite, remote close
  request serialization, Python SDK close/delete behavior, and
  close-preserves/delete-removes persistence behavior.
- Added and compiled but not yet run in the next batched test pass: forced
  turn-task timeout, forced forwarder-task timeout, and successful-close
  no-late-session-event regressions, plus a byte-for-byte close-preservation
  assertion on the close/delete lifecycle test and an in-flight close
  accounting/order regression. They expect structured `session_close_timeout`
  data where applicable, prove timed-out tasks are aborted/dropped, verify a
  successful close does not leave further same-session outbound events after
  the close response completes, verify `session/close` preserves existing
  transcript bytes exactly, and verify the final close `SessionResult` includes
  the in-flight turn's per-turn `SessionResult` accounting.
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
4. use `server.turn_drain_timeout_secs`, never a hard-coded duration;
5. stop/await background tasks and integrations under the process close
   deadline;
6. commit accounting/history before final `SessionResult`;
7. emit no event after close completion;
8. return close failures rather than logging and reporting success.

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

### Tests and gate

- close preserves an existing JSONL byte-for-byte except allowed close metadata;
- delete removes the transcript only after close;
- delete while Loading/Live/Closing returns `SessionStillLive`;
- forced turn-task timeout aborts and joins the task;
- forced forwarder timeout aborts and joins the task;
- no event or transcript append occurs after close response/completion;
- closing during a turn includes the terminal turn in accounting;
- close uses configured timeout;
- orphan and interactive close use the same cascade;
- storage failure is returned to the client.

Run affected crate tests, schema generation checks, then `just quick-check`.

## Phase B: make turn completion authoritative

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

Primary areas:

- `common/types` turn/result DTOs
- `app/agent-host/src/session_runtime/turn.rs`
- `app/agent-host/src/app_server_host/request_handlers/turn.rs`
- local and remote client demux
- headless runner

Tests and gate:

- exactly one terminal result per turn;
- terminal result follows history/accounting commit;
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

## Phase C: introduce one CLI execution plan

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
consumer were removed from clap and covered by rejection tests. Still open:
final full-workspace gates. The retained top-level clap schema is guarded by an
accepted-field consumption audit test that requires every accepted flag to have
an explicit consumer or execution-plan policy.

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

## Phase D: build a valid process host with zero sessions

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

## Phase E: make Event Hub process/registry owned

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
- Still open in Phase E: run the batched validation pass, add at most the
  missing core regression for SDK stdio Hub egress if validation exposes a gap,
  add explicit close/replace/reconnect-cursor coverage, and verify event
  identity/ack isolation across concurrent sessions.

Changes:

1. replace `RuntimeEventHubConnector::spawn_for_session` with
   `ProcessEventHub::spawn`;
2. resolve Event Hub endpoint as process-host policy;
3. subscribe to AppServer registry lifecycle changes;
4. announce a live-registry snapshot on connect/reconnect;
5. add/update membership protocol support, or reconnect on membership changes
   if the Hub wire has no update frame;
6. request/restore cursors for every live session;
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
- reconnect requests cursors for all currently live sessions;
- events for A/B retain identity and ack independently;
- no placeholder identity appears;
- connector failure does not block unrelated session turns/close.

## Phase F: unify TUI, headless, and SDK lifecycle

Implementation status, 2026-07-13:

- Started the unified lifecycle slice. `AppServerLocalBridge` now exposes
  narrow local lifecycle facades for interactive `session/start`,
  `session/resume`, and in-session resume replacement; the facade sends typed
  client requests, then returns the registered `SessionHandle` plus interactive
  surface.
- `session/start` accepts an optional `session_id` for process-local startup
  policy. This lets headless keep its resolved automation/resume identity
  without bypassing the AppServer lifecycle owner.
- `session/start` also accepts optional `initial_messages` for process-local
  test/embedding callers that already hold typed message history. The
  lifecycle-owned runtime builder hydrates that history before the first turn;
  headless no longer seeds or replaces prior history after startup.
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
- A shared lifecycle conformance regression now runs one start/read/close
  contract against the local typed AppServer surface and the JSON-RPC AppServer
  bridge.
- Still open in Phase F: extend that shared conformance suite to the concrete
  stdio SDK and sidecar transport bindings, and add resume-specific assertions
  to the same matrix.

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

- the same lifecycle conformance suite runs against local typed and JSON-RPC
  AppServer bridge surfaces for start/read/close;
- extend the same suite to local TUI-style, local headless-style, stdio SDK,
  and sidecar connection bindings;
- resume hydration/history/callback requirements are identical;
- no production surface calls `SessionFactory` directly;
- no production surface directly mutates registry/runtime state;
- all surfaces return the same typed close failure outcome;
- all surfaces use shared shutdown ordering.

## Phase G: repair dependency direction

### G1. Move shared DTOs down

Changes:

- move image payload representation out of TUI;
- move `SystemPushKind` domain/message representation to `coco-messages` or
  `coco-types`;
- keep TUI rendering conversion in the TUI layer.

### G2. Create `coco-tui-runner`

Move from CLI/agent-host:

- TUI bootstrap and command driver;
- TUI permission and sandbox bridges;
- voice/TUI application integration;
- teammate TUI command pump;
- editor/theme/keybinding surface orchestration.

### G3. Create `coco-headless`

Move:

- one-shot run options/outcome;
- input/output formatting;
- headless permission and signal policy;
- headless slash presentation behavior.

### G4. Extract `coco-agent-runtime`

Move session runtime, construction, turn coordination, operations, and
integrations without changing behavior in the same commit that redirects
consumers.

Dependency gates:

- `coco-agent-runtime` has no AppServer/TUI/transport dependency;
- `coco-agent-host` has no TUI dependency;
- `coco-cli` depends on surface runners rather than application/core internals;
- the existing server-client independence seam remains green;
- no dependency cycle is introduced.

Run each affected crate suite, dependency seam scripts, then `just quick-check`.

## Phase H: narrow capabilities and reorganize modules

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
10. reduce `lib.rs` to intentional facade exports.

Mechanical gates:

- no public session API returns `Mutex`, `RwLock`, internal registries, or
  mutable manager handles;
- no public `runtime()` or `Deref`;
- no mutable engine config contains `SessionId`;
- no late callback-requirement install;
- public API snapshot reviewed explicitly;
- module-size report has no unexplained file above target;
- rustdoc describes owner/lifetime for every exported capability.

## Phase I: remove obsolete code and align documentation

Changes:

1. delete unused `agent_host::output` and use UTF-8-safe rendering in the real
   headless crate;
2. remove stale startup/runtime replacement helpers and compatibility comments;
3. remove obsolete symbols/files referenced by old plans;
4. update crate `CLAUDE.md` files and root architecture table;
5. update schemas and SDK examples;
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

After all phases are complete and no code changes remain:

1. run focused lifecycle, CLI, Hub, surface conformance, and dependency seam
   suites;
2. run schema/code-generation checks and SDK tests;
3. run `just quick-check`;
4. run `just pre-commit` exactly once;
5. record results by semantic gate, not only test count.

## Definition of done

- close and delete are separate, explicit, tested operations;
- close completion proves no surviving session work or late events;
- terminal accounting includes the drained turn;
- CLI has one execution plan and no accepted no-op flags/commands;
- process host startup creates zero sessions;
- Event Hub membership follows the live registry;
- all surfaces use the same lifecycle and shutdown coordinator;
- agent-runtime and agent-host have correct dependency direction;
- session identity/callback requirements are construction-time invariants;
- public capabilities expose no internal locks/managers;
- module layout communicates ownership;
- retained multi-session authority/isolation tests remain green;
- final documentation describes the landed tree accurately.
