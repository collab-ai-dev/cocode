# Multi-Session AppServer Architecture

Status: architecture audit reopened on 2026-07-13. The explicit-target and
multi-session isolation work remains valuable, but the system is not yet at the
target architecture described in this directory.

This directory is the source of truth for the next breaking refactor of the
coco-rs AppServer host. Backward compatibility with the current CLI flags,
startup sequence, or removed `session/archive` behavior is not a requirement.

## Executive decision

Retain the parts that already make invalid cross-session selection difficult:

- one root `SessionId`;
- explicit `SessionTarget` and `InteractiveTarget` DTOs;
- AppServer-owned registry, surface validation, replay, and callback routing;
- one selected live-session capability per interactive request;
- per-session cwd/config/resource construction;
- a remote client crate that does not depend on the server implementation.

Rework the application host and surface composition because the current tree
still has correctness and ownership defects:

- Phase A has started: Rust DTOs and typed local/remote clients now use
  `session/close` and `session/delete`, and close no longer deletes the JSONL
  transcript. JSON schemas plus Python/TypeScript generated protocol artifacts
  now use the new methods, and AppServer internals no longer use
  archive-oriented names for close routing. Close-owner failures now propagate
  through close completion with structured timeout data. Terminal turn ordering
  remains open;
- CLI mode selection is centralized in `ExecutionPlan` and frozen as landed
  baseline; additional real-terminal coverage is deferred until that surface
  changes;
- SDK startup now creates zero live sessions before the first lifecycle
  request. Broader host-builder cleanup belongs to Workstream 3;
- Event Hub is now process-owned egress for SDK, TUI, and headless startup
  paths, but close/replace/reconnect-cursor regressions are still open;
- TUI/headless startup plus in-session TUI resume/branch/clear now enter the
  AppServer lifecycle, and main TUI shortcut/control paths no longer rebind
  runtimes from the TUI layer. The transitional serialized
  `session/start.{session_id,initial_messages}` fields are unsafe and must be
  replaced by a non-serialized local construction seam;
- `session/start` can currently reach an existing slot before authority is
  established, accepted start/initialize fields are silently ignored, and
  start/resume responses retain optional surface compatibility that v2 does
  not need;
- turn ownership is released before final engine history is committed, so an
  immediate next turn can observe stale history even though terminal events
  have already been sent;
- replace, close, shutdown, and Event Hub retirement still have task-ownership
  and terminal-egress gaps;
- `coco-agent-host` depends on TUI types and exports most of its implementation;
- `SessionHandle` exposes raw locks and service handles through a very broad
  forwarding API;
- several construction-time invariants are installed after construction.

## Convergence policy

This refactor now proceeds through three serial workstreams:

1. correctness stabilization closes only start authority/protocol truth, turn
   history/terminal ordering, close/replace/task ownership, and Hub final local-
   egress retirement;
2. surface boundary moves composition to `app/cli/src/{tui,headless,sdk}` and
   removes agent-host -> TUI without changing lifecycle behavior;
3. internal cleanup narrows capabilities, reorganizes agent-host in place, and
   removes obsolete code without changing protocol behavior.

No workstream overlaps the next. Every correctness change starts with a failing
adversarial regression, names one lifecycle owner and completion point, and
closes one invariant before another is selected. File moves, new crates, broad
renames, dead-code cleanup, and unrelated transport coverage are prohibited
during correctness stabilization.

The landed execution-plan, zero-session startup, and shared lifecycle entry
points are frozen baseline. They are reopened only by a regression against one
of the four correctness invariants. Newly discovered adjacent cleanup is
recorded for a later workstream; it does not automatically expand the current
completion gate. The normative ordering and entry/exit gates are in
[remediation-plan.md](remediation-plan.md#convergence-reset).

## Documents

| Document | Responsibility |
|---|---|
| [review.md](review.md) | Evidence, counter-hypotheses, severity, and verified test gaps from the 2026-07-13 adversarial review |
| [current-architecture.md](current-architecture.md) | Descriptive account of the current production tree, including known defects |
| [target-architecture.md](target-architecture.md) | Normative v2 ownership, crate, startup, lifecycle, and shutdown architecture |
| [protocol-scope.md](protocol-scope.md) | Normative breaking protocol scopes and lifecycle semantics |
| [remediation-plan.md](remediation-plan.md) | Ordered implementation plan with package-level acceptance gates |
| [history.md](history.md) | Migration history, retained v1 work, and superseded decisions |

Crate-local implementation guidance remains in each crate's `CLAUDE.md`.
Event Hub wire encoding remains in `docs/coco-rs/event-hub/spec.md`; this
directory owns how AppServer lifecycle and live-session membership feed that
wire protocol.

## Current status

| Area | Status | v2 decision |
|---|---|---|
| Explicit request targeting | Landed | Retain |
| Registry and surface authority | Landed | Retain |
| Start identity and mutation authority | Defective | Remote start mints a new id and operates only on Missing; local seeded start is non-serialized |
| Accepted protocol-field consumption | Defective | Every accepted field is validated and consumed or rejected; remove duplicates/no-ops |
| Per-connection profile/callback isolation | Landed | Retain |
| Concurrent A/B runtime isolation | Landed for tested paths | Retain and extend tests |
| Runtime close versus transcript deletion | Partially landed | Finish single-deadline and task-supervisor close gates |
| Turn drain, history commit, and terminal ordering | Defective | One lifecycle owner; commit before Idle and terminal delivery |
| CLI mode resolution | Landed/frozen | Retain one typed `ExecutionPlan`; defer extra TTY coverage |
| SDK zero-session startup | Landed/frozen | Retain; host-builder cleanup waits for Workstream 3 |
| SDK transport boundary | Adequate | Retain `coco-sdk-server`; move only CLI startup policy to `app/cli/src/sdk` |
| Event Hub live membership | Partially landed | Lifecycle revisions plus retiring membership through final local-egress handoff |
| TUI/headless/SDK lifecycle symmetry | Partial | All surfaces use the same client lifecycle operations |
| Surface directory boundaries | Defective | Use `app/cli/src/{tui,headless,sdk}` directly; no `surfaces/` or runner crates |
| Host/TUI dependency direction | Defective | Move TUI-only composition to `app/cli/src/tui` |
| Agent-host module/API boundary | Defective | Refactor in place into private cohesive modules and narrow facades; do not duplicate a runtime crate |
| Session capability boundary | Partial | No raw locks/managers; construction-time identity and callback requirements |
| Whole-runtime actor | Rejected | Keep fine-grained synchronization and a small turn coordinator |
| Project service sharing | Adequate | Add only capability-named services with explicit keys |
| Delivery cadence | Reset | Correctness -> surface boundary -> internal cleanup; no overlap |

## Completion rule

The v2 refactor is complete only when all of the following are demonstrated.
`remediation-plan.md` assigns them to independently recorded workstreams;
later cleanup does not delay or silently reopen an already proven correctness
gate:

1. close never deletes the transcript and delete is explicit;
2. no turn, forwarder, hook, or integration task survives a completed close;
3. terminal accounting is generated after turn drain and cannot be followed by
   late session events;
4. one typed execution plan selects exactly one CLI mode;
5. SDK startup creates zero live sessions before the first lifecycle request;
6. TUI, headless, and SDK use the same start/resume/replace/close operations;
7. remote start cannot name or mutate an existing session, and every accepted
   protocol field is validated and consumed;
8. turn history/accounting is committed before the coordinator returns to Idle
   or emits the terminal result;
9. Event Hub membership and reconnect cursors cover Live and retiring Closing
   sessions through final local-egress handoff;
10. `coco-agent-host` has no dependency on `coco-tui`;
11. session identity and callback requirements are valid at construction;
12. public session capabilities expose operations and snapshots, not raw locks;
13. two real sessions continue to pass authority, runtime, integration, replay,
    and shutdown isolation tests.

Passing the existing workspace suite is necessary but not sufficient: the
suite now covers basic close-preserves/delete-removes behavior, CLI execution
plan behavior, SDK zero-session startup, the `max_sessions = 1` first real
`session/start` path, and focused Event Hub startup plus A/B membership
snapshots. The focused `coco-app-server` suite (92 tests) and
`coco-agent-host` suites (331 unit, 26 multi-session integration, and one
WebSocket test) passed during this audit. They still do not exercise legacy-id
start rejection, immediate-next-turn history visibility, replace close-owner
failure, complete background-task joining, or Hub final-event cursor behavior.
