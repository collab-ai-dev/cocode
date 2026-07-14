# Multi-Session AppServer Architecture

Status: **v2 remediation landed 2026-07-14.** All three workstreams
(correctness stabilization, surface boundary, internal cleanup) are complete
and verified by a full green `just pre-commit`;
[remediation-plan.md](remediation-plan.md) is now the migration record. A
post-completion adversarial re-review (2026-07-14) cross-validated the landed
tree against the completion rule below: 10 of 13 items are fully demonstrated,
3 carry bounded residuals. The verified residuals and the prioritized
follow-up work live in [follow-up-todo.md](follow-up-todo.md).

This directory remains the source of truth for the coco-rs AppServer host
architecture. Backward compatibility with the pre-v2 CLI flags, startup
sequence, or removed `session/archive` behavior is not a requirement.

## Landed architecture (summary)

The target described in [target-architecture.md](target-architecture.md) is in
production:

- one process hosts zero or more independent root sessions; process startup
  creates no session;
- remote `session/start` mints its identity, requires a Missing slot, and
  every accepted protocol field is consumed or rejected
  (`deny_unknown_fields`);
- turn completion is authoritative: history/accounting commit precedes
  terminal delivery and next-turn admission;
- close and delete are separate, tested operations; close drains owned work
  under one absolute deadline and emits nothing after completion;
- Event Hub egress is process-owned, derived from registry membership
  covering Live plus retiring Closing sessions;
- `coco-agent-host` depends on neither `coco-tui` nor `coco-sdk-server`
  (seam-guarded in `pre-commit`); surface composition lives directly in
  `app/cli/src/{tui,headless,sdk}`; `coco-sdk-server` is transport-only;
- `SessionRuntime` is private; session identity and callback requirements are
  construction-time invariants; the module tree is grouped by ownership
  (`session`/`integrations`/`host`/`client`/`lifecycle`) with companion-file
  tests throughout.

The dependency graph is a strict DAG:

```text
L0 coco-app-server-transport (wire leaf)
L1 coco-app-server | coco-app-server-client (client never sees server impl)
L2 coco-app-runtime, coco-query, coco-session, coco-tui (no upward edges)
L3 coco-agent-host (host + private session aggregate)
L4 coco-sdk-server (SDK transport adapter)
L5 coco-cli (composition root: tui + headless + sdk)
```

## Follow-up status (tracked in follow-up-todo.md)

The 2026-07-14 review's follow-up items are **all 15 resolved** (T1–T15),
verified by a full green `just pre-commit`:

- lock leak closed (`live_permission_rules()` returns a narrow capability);
- local host assembly unified behind `agent_host::local_host::build_local_host`,
  consumed by both TUI and headless;
- deterministic coordinator regression pins the R13 admission gate;
- the two session-owned detached MCP tasks migrated to the close-joined
  supervisor;
- `plan_mode_instructions` moved off `initialize` to `session/start` /
  `session/resume` (schemas + SDK regenerated);
- mechanical debt cleared: dead Hub match arm, stale `tui_runner` docs, four
  inline test modules, seam-guard coverage, naming/placement nits, test-only
  compat-seam framing;
- T9 verified already correct (reconnect cursors cover retiring Closing
  sessions via the existing announce/ack; membership updates on every lifecycle
  transition) and locked in with a regression; the "dedicated lifecycle-revision
  stream" is an unneeded micro-optimization.

## Documents

| Document | Responsibility |
|---|---|
| [review.md](review.md) | Evidence, counter-hypotheses, severity, and verified test gaps from the 2026-07-13 adversarial review |
| [current-architecture.md](current-architecture.md) | Descriptive account of the landed production tree |
| [target-architecture.md](target-architecture.md) | Normative v2 ownership, crate, startup, lifecycle, and shutdown architecture |
| [protocol-scope.md](protocol-scope.md) | Normative breaking protocol scopes and lifecycle semantics |
| [remediation-plan.md](remediation-plan.md) | Migration record: ordered implementation history with per-phase status |
| [follow-up-todo.md](follow-up-todo.md) | Post-completion verified residuals and prioritized follow-up TODO (2026-07-14 review) |
| [history.md](history.md) | Migration history, retained v1 work, and superseded decisions |

Crate-local implementation guidance remains in each crate's `CLAUDE.md`.
Event Hub wire encoding remains in `docs/coco-rs/event-hub/spec.md`; this
directory owns how AppServer lifecycle and live-session membership feed that
wire protocol.

## Current status

| Area | Status | Notes |
|---|---|---|
| Explicit request targeting | Landed | |
| Registry and surface authority | Landed | |
| Start identity and mutation authority | Landed | Remote start mints; serialized id/history removed; local seed is `#[serde(skip)]` |
| Accepted protocol-field consumption | Landed with one residual | `plan_mode_instructions` still on `initialize` (T8) |
| Per-connection profile/callback isolation | Landed | Callback requirements are construction inputs |
| Concurrent A/B runtime isolation | Landed | 26 multi-session integration tests |
| Runtime close versus transcript deletion | Landed | Separate `session/close` / `session/delete`; single absolute deadline |
| Turn drain, history commit, terminal ordering | Landed | Deterministic next-turn regression still to add (T6) |
| CLI mode resolution | Landed | One typed `ExecutionPlan` |
| SDK zero-session startup | Landed | `HostBuilder`/`PreparedHost` |
| SDK transport boundary | Landed | `run_sdk_mode` moved to `app/cli/src/sdk`; crate is wire-only |
| Event Hub live membership | Landed with residuals | Live + retiring Closing announced; lifecycle-revision stream and retiring cursors unbuilt (T9) |
| TUI/headless/SDK lifecycle symmetry | Landed with residuals | Same typed lifecycle everywhere; local assembly triplicated (T4/T5) |
| Surface directory boundaries | Landed | `app/cli/src/{tui,headless,sdk}` |
| Host/TUI dependency direction | Landed | Seam-guarded |
| Agent-host module/API boundary | Landed with one leak | `live_permission_rules()` lock leak (T1) |
| Session capability boundary | Landed with one leak | Same item |
| Whole-runtime actor | Rejected | Fine-grained sync + small turn coordinator retained |
| Delivery cadence | Complete | Three serial workstreams closed in order |

## Completion rule

The v2 refactor is complete only when all of the following are demonstrated.
Verified state as of 2026-07-14 — ✅ demonstrated, ⚠️ bounded residual (see
[follow-up-todo.md](follow-up-todo.md)):

1. ✅ close never deletes the transcript and delete is explicit;
2. ⚠️ no turn, forwarder, hook, or integration task survives a completed close
   — supervisor + close-deadline join landed; per-site spawn triage open (T7);
3. ✅ terminal accounting is generated after turn drain and cannot be followed
   by late session events;
4. ✅ one typed execution plan selects exactly one CLI mode;
5. ✅ SDK startup creates zero live sessions before the first lifecycle
   request;
6. ✅ TUI, headless, and SDK use the same start/resume/replace/close
   operations;
7. ✅ remote start cannot name or mutate an existing session, and accepted
   protocol fields are validated and consumed (`plan_mode_instructions` moved
   to `session/start`/`session/resume`, T8);
8. ✅ turn history/accounting is committed before the coordinator returns to
   Idle or emits the terminal result (deterministic coordinator regression
   added, T6);
9. ✅ Event Hub membership and reconnect cursors cover Live and retiring
   Closing sessions through final local-egress handoff — the announce carries
   `announced_session_ids()` (Live + retiring Closing) and reconnect cursors
   come from the announce ack; regression added (T9);
10. ✅ `coco-agent-host` has no dependency on `coco-tui`;
11. ✅ session identity and callback requirements are valid at construction;
12. ✅ public session capabilities expose operations and snapshots, not raw
    locks — `live_permission_rules()` now returns a narrow capability (T1);
13. ✅ two real sessions continue to pass authority, runtime, integration,
    replay, and shutdown isolation tests.
