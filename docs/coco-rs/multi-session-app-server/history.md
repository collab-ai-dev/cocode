# Migration History

This is a concise record of how the current architecture arrived here. It is
not normative; `target-architecture.md` owns future decisions.

## Superseded designs

### Concurrent AppServer plan

The earliest plan copied a Codex-style `ThreadId` plus `SessionId` hierarchy
and proposed a new thread manager. That model was rejected for coco-rs:

- `SessionId` already is the durable root conversation and JSONL identity;
- subagents are represented below the root by `AgentId` and coordinator state;
- a second interchangeable root id adds conversions and ambiguous ownership
  without solving a coco-rs requirement;
- `/clear`, fork, and resume can be expressed with immutable SessionIds and
  explicit lifecycle operations.

The old `concurrent-app-server-plan.md` is removed rather than archived as an
active plan because its identity and crate model conflict with the shipped
architecture.

### Monolithic multi-session plan

The later `multi-session-app-server-plan.md` corrected the identity model and
guided substantial implementation, but grew into a mixture of:

- requirements and reference-product research;
- speculative type definitions;
- adversarial findings and refutations;
- five landing-status passes;
- future actor and project-service proposals;
- acceptance tests for both v1 and deferred adapters;
- a decision log that did not distinguish landed from proposed decisions.

As code changed, its "Current State" and crate sections contradicted later
landing notes. It is replaced by the evidence/design/plan split in this
directory.

## Work retained

The following implementation waves remain part of the architecture:

1. `SessionId` stays the only root conversation identity.
2. `SessionEnvelope` provides session/agent/turn attribution and durable
   per-session sequence allocation.
3. AppServer owns bounded replay and per-surface fan-out.
4. `LiveSessionRegistry` uses Loading/Live/Closing slots and spawned owner
   tasks so caller cancellation cannot wedge lifecycle progress.
5. replace commits validate registry plus routing state under fixed no-await
   lock order.
6. interactive and passive surfaces are distinct; only one interactive owner
   exists per session.
7. `coco-app-server-client` is remote-only and does not depend on the server.
8. in-process typed client composition lives in `coco-agent-host`.
9. `coco-state` was removed; live backend, tool state, and TUI projection have
   distinct owners.
10. `background-review` was renamed `coco-maintenance`.
11. `coco-app-runtime` owns process/project/workspace/bootstrap contracts;
    fused application session composition lives in `coco-agent-host`.
12. TUI/headless use an AppServer bridge capped at one session; SDK uses a
    configured multi-slot AppServer.

## Work reclassified

The following former "decisions" are now classified differently:

| Former idea | New classification |
|---|---|
| whole `SessionRuntime` actor and universal `SessionCommand` mailbox | Rejected as a v1 prerequisite; optional turn-coordinator evolution only |
| `SessionHandle` as mailbox/watch-only value | Replaced by an opaque `Arc` capability without `Deref` |
| `ProjectHeavyServices` | Rejected name and aggregate; capability-named services only |
| strict ProjectServices single-flight | Optional optimization; current publication dedup is valid |
| implicit replace through start/resume | Rejected; add explicit `session/replace` |
| current-session fallback for request dispatch | Rejected; every mutation has an explicit target |
| process-installed SDK runtime/MCP/file-history/reload | Relocate each capability without behavior loss, then remove only the duplicate process slot |
| external Web/Desktop/IM adapters | Deferred until core isolation passes |

## Review baseline

The replacement documents were written after re-reading the production DTO,
client, adapter, handler, runner, registry, runtime, project-service, and test
paths on 2026-07-11. The most important correction is that multi-slot storage
and surface routing do not by themselves prove multi-session execution.

## Breaking refactor landed (2026-07-11)

- added exhaustive request scopes and required typed targets;
- isolated initialize state, writers, and callback correlation per connection;
- made AppServer validation the only interactive runtime selector;
- moved active turns, MCP, file history, reload, hooks, sandbox, and approvals
  behind `SessionHandle`;
- added explicit replacement, closing-resume retry, atomic orphan close, and
  multi-runtime shutdown;
- removed singleton runtime/capability state and the implicit sole-session
  fallback APIs;
- hardened `SessionHandle` into an opaque focused capability and enabled
  `clippy::await_holding_lock` workspace-wide.

## Release validation completed (2026-07-11)

The final workspace validation exposed four related local AppServer call paths
that installed a runtime but could construct an `InteractiveTarget` before the
local bridge had attached its interactive surface: queued history turns,
fast-mode changes, thinking-level changes, and explicit file rewind. Each path
now explicitly attaches the selected session before dispatch.

After that correction, all seam checks and workspace clippy passed, all 88 TUI
runner tests passed, and nextest passed all 13,606 executed workspace tests
(four tests remained skipped by their existing configuration). The focused
agent-host, app-server, app-server-client, and types suites also passed in full.

## Ownership hardening follow-up (2026-07-11)

A final production-path audit removed the remaining duplicate SDK session
owners. `LocalAppSessionHandle` now always contains a live `SessionHandle`;
start/resume without a runtime factory fail closed. `SessionTurnExecutor` is
the only turn executor across SDK, TUI, headless, and live harnesses. Turn ids,
active tasks, cancellation, and aggregate accounting moved from process-keyed
SDK maps into each runtime's `SessionTurnCoordinator`.

The audit also made callback reply correlation include the originating surface,
applied `session/start` model and permission inputs to the constructed runtime,
and expanded the release-blocking host suite from six shallow handle scenarios
to sixteen production-handler scenarios backed by real runtimes.

The follow-up validation passed affected all-features clippy, all 13,611
workspace tests (four existing skips), schema/Python generation checks, and
107 Python SDK tests (ten environment-gated skips).

The release suite was then tightened against every package-H sentence rather
than its test count: the same A/B flows now cover controls, live reads,
turn-list projection, file rewind, initialized-connection config/history,
real SDK-hosted MCP handshakes with isolated tool catalogs, the complete
callback authority matrix, replacement cleanup, turn identity, and explicit
per-scenario deadlines.

## Delivery-blocker closure (2026-07-11)

The last audit found that `ArchiveTarget::Orphaned` selected a registry runtime
before proving the session was actually orphaned. Because the handler cancelled
and drained the active turn before the lifecycle close performed its own
authorization, an owned session could suffer destructive side effects and only
then return `InteractiveOwnerConflict`. Authorization now occurs in request
resolution before handler dispatch, and a real-runtime barrier test proves a
rejected orphan archive leaves the active turn and registry entry intact.

The audit also rejected test-count equivalence as completion evidence. The
host suite now maps all eleven package-H requirements to concrete assertions,
including both project and local config writes, real SDK-hosted MCP isolation,
the full callback authority tuple, compatible/incompatible orphan resume,
targeted reload close/replacement, orphan turn continuation, lossless
slow-consumer recovery, and non-serial shutdown. All concurrent and lifecycle
tests have overall deadlines.

Finally, the obsolete SDK `pending_map` module and its self-only tests were
deleted after confirming that production callback ownership had moved to
AppServer. The final post-fix gate passed all 13,611 workspace Rust tests,
affected all-features clippy, schema/Python code-generation checks, and 107
Python SDK tests.
