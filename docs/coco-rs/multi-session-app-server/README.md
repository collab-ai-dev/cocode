# Multi-Session AppServer

Status: implemented, delivery blockers closed, and release-validated on
2026-07-11.

This directory is the single source of truth for coco-rs multi-session
AppServer architecture. It replaces the former
`multi-session-app-server-plan.md` and `concurrent-app-server-plan.md`. Those
documents mixed shipped behavior, rejected identity models, migration logs,
and unimplemented proposals, so neither was reliable as a current design.

## Executive decision

The breaking multi-session AppServer refactor is implemented across protocol,
server, clients, host lifecycle, and session runtime ownership. The existing
crate split remains intact.

The server already has useful multi-session infrastructure:

- bounded live-session slots with load, close, replace, and shutdown owners;
- connection and surface routing with at most one interactive owner per session;
- passive subscriptions, durable event envelopes, replay, and slow-consumer
  isolation;
- per-session configuration folds and per-project catalog caching;
- separate server, remote client, transport, application host, and runtime
  crates with the intended dependency direction.

Session commands now carry typed targets, accepted connections own immutable
profiles and callback correlation, and every interactive handler receives the
`SessionHandle` selected by AppServer validation. MCP, history, reload,
sandbox, hooks, approvals, active turns, and file rewind are session-owned.
Closing resume waits and retries, replacement is explicit and atomic, and
orphan archive has a dedicated authority type.

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
| Registry and lifecycle owner tasks | Landed | Keep model |
| Event envelope, sequence, replay, fan-out | Landed | Keep |
| Surface routing and passive observation | Landed | Keep |
| Per-session cwd/config fold | Landed | Keep |
| Project catalog/config cache | Landed | Keep; describe honestly |
| One connection controlling several sessions | Landed | Explicit `InteractiveTarget` authority |
| Multiple initialized connections | Landed | One immutable `ConnectionProfile` per connection |
| Concurrent turn/runtime isolation | Landed | Registry-selected `SessionHandle` on every turn/control |
| Session-scoped MCP/file history/reload | Landed | Owned below `SessionHandle` |
| Orphan archive authorization | Landed | Proved before handler side effects |
| Package H production isolation suite | Landed | All 11 required scenarios covered by 16 bounded tests |
| Legacy SDK pending callback map | Removed | AppServer is the sole callback owner |
| SDK process session slots | Reduced to keyed projections | No runtime-selection authority |
| Whole-runtime actor | Not implemented and not required | Reject as a v1 prerequisite |
| `ProjectHeavyServices` | Not implemented and poorly named | Reject; add capability-named services only when needed |
| Web/Desktop/IM product adapters | Deferred | Keep outside v1 and outside AppServer core |

## Landed amendments (2026-07-11)

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

## Final validation (2026-07-11)

The breaking refactor meets the completion rule. Final validation covered the
entire workspace and the production AppServer path:

- `just quick-check` passed, including all seam checks and
  `cargo clippy --workspace --all-features --tests` with zero warnings;
- `cargo nextest run --workspace --no-fail-fast` passed all 13,611 executed
  tests; four tests were skipped by their existing test configuration;
- the host integration suite now passes sixteen production-handler scenarios
  with real runtimes, including concurrent turns, project/local config writes,
  callback authority, orphan resume/lifecycle, reload ownership,
  slow-consumer replay recovery, event identity, and concurrent shutdown;
- focused agent-host, app-server, app-server-client, and types tests passed
  309, 89, 34, and 300 tests respectively;
- `git diff --check` and the removed-architecture symbol audit passed.

The full validation pass exposed one final TUI/local-bridge defect: queue
turns, fast-mode changes, thinking-level changes, and file rewind could build
an interactive target before attaching the local bridge surface. Those paths
now explicitly attach the selected session before dispatch. All 88 TUI runner
tests and the full workspace suite passed after the fix.

## Delivery-blocker closure (2026-07-11)

The final delivery audit closed three follow-up findings:

1. Orphan archive authorization now runs during request-runtime resolution,
   before the archive handler can take, cancel, or drain an active turn, clear
   activity, or emit an archive result. An orphan target for an interactively
   owned session returns `InteractiveOwnerConflict` without mutating the
   runtime; a barrier-backed regression test proves the running turn survives.
2. Package H completion is tracked by its eleven required behaviors, not by
   counting `#[tokio::test]` attributes. The sixteen-test host suite covers the
   full A/B runtime, connection, config, callback, orphan, reload, replay,
   slow-consumer, and shutdown matrix. Every concurrent or lifecycle scenario
   has an overall bounded timeout.
3. The unused SDK `pending_map` module, its self-tests, and its public export
   were deleted. Callback ownership and reply correlation now live only in
   AppServer and validate connection, surface, session, and request id.

The post-fix final gate passed affected all-features clippy, all 13,611
workspace Rust tests (four existing skips), schema and Python code-generation
checks, and 107 Python SDK tests (ten environment-gated skips). See
[remediation-plan.md](remediation-plan.md#implementation-status-2026-07-11)
for the work-package and package-H evidence matrix.
