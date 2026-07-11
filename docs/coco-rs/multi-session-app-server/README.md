# Multi-Session AppServer

Status: active design, reviewed against the production tree on 2026-07-11.

This directory is the single source of truth for coco-rs multi-session
AppServer architecture. It replaces the former
`multi-session-app-server-plan.md` and `concurrent-app-server-plan.md`. Those
documents mixed shipped behavior, rejected identity models, migration logs,
and unimplemented proposals, so neither was reliable as a current design.

## Executive decision

The project should continue the multi-session AppServer work. The product goal
is valuable and the existing crate split is worth keeping, but the end-to-end
goal is not complete.

The server already has useful multi-session infrastructure:

- bounded live-session slots with load, close, replace, and shutdown owners;
- connection and surface routing with at most one interactive owner per session;
- passive subscriptions, durable event envelopes, replay, and slow-consumer
  isolation;
- per-session configuration folds and per-project catalog caching;
- separate server, remote client, transport, application host, and runtime
  crates with the intended dependency direction.

The remaining correctness boundary is more fundamental than an optimization:

- session-scoped requests do not carry an explicit target;
- a connection with multiple interactive surfaces cannot select one;
- initialize inputs, outbound writers, and request correlation do not yet have
  a complete per-connection owner;
- production turn execution can use the last process-installed runtime rather
  than the runtime selected by the request;
- MCP, file history, and reload ownership still contain process-singleton
  paths;
- closing-session resume does not implement the documented wait-and-reopen
  behavior.

Until those items are fixed and covered by end-to-end tests, the SDK process
may store several live sessions but must not be described as safely executing
them concurrently.

## Documents

| Document | Responsibility |
|---|---|
| [review.md](review.md) | Evidence-based verification of the reported issues, including counter-hypotheses and severity |
| [current-architecture.md](current-architecture.md) | What the code does today, including crate dependencies and state ownership |
| [target-architecture.md](target-architecture.md) | Normative architecture and protocol after the breaking refactor |
| [protocol-scope.md](protocol-scope.md) | Exhaustive request classification, target DTOs, connection profile, and callback routing |
| [remediation-plan.md](remediation-plan.md) | Ordered implementation plan, acceptance gates, and test strategy |
| [history.md](history.md) | Concise migration history and rejected approaches |

Stable crate-local details remain owned by each crate's `CLAUDE.md`. Event Hub
wire details remain owned by `docs/coco-rs/event-hub/spec.md`. This directory
defines only the cross-cutting session ownership, routing, and host boundaries.

## Status summary

| Area | Status | Decision |
|---|---|---|
| Crate boundaries | Landed | Keep |
| Registry and lifecycle owner tasks | Landed with a closing-resume gap | Fix gap; keep model |
| Event envelope, sequence, replay, fan-out | Landed | Keep |
| Surface routing and passive observation | Landed | Keep |
| Per-session cwd/config fold | Landed | Keep |
| Project catalog/config cache | Landed | Keep; describe honestly |
| One connection controlling several sessions | Not functional | Add explicit request targets |
| Multiple initialized connections | Not isolated | Add one handler and `ConnectionProfile` per accepted connection |
| Concurrent turn/runtime isolation | Not functional | Resolve runtime from the registry for every request |
| Session-scoped MCP/file history/reload | Partial | Move behind the selected session handle |
| SDK process session slots | Migration residue, still functional | Preserve each feature while relocating ownership; remove only redundant slots |
| Whole-runtime actor | Not implemented and not required | Reject as a v1 prerequisite |
| `ProjectHeavyServices` | Not implemented and poorly named | Reject; add capability-named services only when needed |
| Web/Desktop/IM product adapters | Deferred | Keep outside v1 and outside AppServer core |

## Amendments (2026-07-11 verification pass)

An independent line-level verification of this directory against the
production tree confirmed every finding in [review.md](review.md) and added
the following normative decisions:

- `session/archive` takes a typed `ArchiveTarget` so orphaned sessions stay
  closable without a resume round trip
  ([protocol-scope.md](protocol-scope.md));
- orphan-period callback "fail closed" is defined per request family
  (`NoInteractiveSurface`; approvals become denials, elicitations declines,
  user input cancellations); nothing parks and the running turn continues
  ([protocol-scope.md](protocol-scope.md));
- backpressure is explicitly connection-scoped: channel overflow disconnects
  the whole connection and all its surfaces; recovery is reconnect plus
  replay ([target-architecture.md](target-architecture.md));
- the request-scope classification is one exhaustive `request_scope`
  function in `common/types`, making an unclassified request a compile
  error ([protocol-scope.md](protocol-scope.md));
- `clippy::await_holding_lock` becomes a workspace lint, and session-scoped
  telemetry must carry `session_id`/`turn_id`
  ([target-architecture.md](target-architecture.md));
- the A-D batch is developed as stacked per-package commits with the
  package H suite checked in first as `#[ignore]` skeletons
  ([remediation-plan.md](remediation-plan.md)).

## Completion rule

Multi-session is complete only when tests prove that two independently
targeted sessions can run concurrently with different cwd, configuration,
tools, MCP state, histories, controls, events, and shutdown lifecycles without
cross-session reads or writes. Slot count alone is not completion.
