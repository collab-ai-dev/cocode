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
- declared CLI modes and actual mode selection disagree;
- SDK startup now creates zero live sessions before the first lifecycle
  request, but the broader host-builder cleanup remains open;
- Event Hub is now process-owned egress for SDK, TUI, and headless startup
  paths, but close/replace/reconnect-cursor regressions are still open;
- TUI/headless startup plus in-session TUI resume/branch/clear now enter the
  AppServer lifecycle, headless in-memory initial history is carried by
  `session/start.initial_messages`, and main TUI shortcut/control paths no
  longer rebind runtimes from the TUI layer. AppServer drain and Event Hub
  shutdown now share a coordinator across TUI, headless, and SDK host paths;
- `coco-agent-host` depends on TUI types and exports most of its implementation;
- `SessionHandle` exposes raw locks and service handles through a very broad
  forwarding API;
- several construction-time invariants are installed after construction.

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
| Per-connection profile/callback isolation | Landed | Retain |
| Concurrent A/B runtime isolation | Landed for tested paths | Retain and extend tests |
| Runtime close versus transcript deletion | Partially landed | Finish close-owner gates and batched SDK validation |
| Turn drain and terminal result ordering | Partially landed | One lifecycle owner; abort timed-out tasks; result after drain |
| CLI mode resolution | Partially landed | One typed `ExecutionPlan`; delete unsupported flags |
| SDK startup | Partially landed | Finish host-builder cleanup; keep zero-session startup |
| Event Hub live membership | Partially landed | Registry-derived dynamic membership |
| TUI/headless/SDK lifecycle symmetry | Partial; startup, TUI resume/branch/clear, headless initial history, main TUI controls, and AppServer/Event Hub shutdown migrated | All surfaces use the same client lifecycle operations |
| Host/TUI dependency direction | Defective | Move TUI composition to `coco-tui-runner` |
| Agent-host module/API boundary | Defective | Private cohesive modules and narrow facades |
| Session capability boundary | Partial | No raw locks/managers; construction-time identity and callback requirements |
| Whole-runtime actor | Rejected | Keep fine-grained synchronization and a small turn coordinator |
| Project service sharing | Adequate | Add only capability-named services with explicit keys |

## Completion rule

The v2 refactor is complete only when all of the following are demonstrated:

1. close never deletes the transcript and delete is explicit;
2. no turn, forwarder, hook, or integration task survives a completed close;
3. terminal accounting is generated after turn drain and cannot be followed by
   late session events;
4. one typed execution plan selects exactly one CLI mode;
5. SDK startup creates zero live sessions before the first lifecycle request;
6. TUI, headless, and SDK use the same start/resume/replace/close operations;
7. Event Hub membership and reconnect cursors cover every live session;
8. `coco-agent-host` has no dependency on `coco-tui`;
9. session identity and callback requirements are valid at construction;
10. public session capabilities expose operations and snapshots, not raw locks;
11. two real sessions continue to pass authority, runtime, integration, replay,
    and shutdown isolation tests.

Passing the existing workspace suite is necessary but not sufficient: the
suite now covers basic close-preserves/delete-removes behavior, CLI execution
plan behavior, SDK zero-session startup, the `max_sessions = 1` first real
`session/start` path, and focused Event Hub startup plus A/B membership
snapshots. Phase A has compiled targeted regressions for byte-for-byte close
preservation and close timeout cleanup. Close/replace Event Hub membership and
reconnect-cursor behavior still need explicit batched validation.
