# coco-goals

Pure domain layer for the first-class session goal runtime
(`docs/coco-rs/goal-architecture-redesign.md`). **No** Tokio, model-client,
filesystem, protocol, or UI dependency — only `coco-types` (shared ids),
`coco-utils-string` (bounded text), serde, strum, thiserror. The host
(`GoalRuntimeHandle` in the session runtime) does all I/O; this crate only
`decide`s.

## Entry point

`decide(snapshot: Option<&GoalSnapshot>, command: GoalCommand) -> Result<GoalDecision, GoalTransitionError>`

Pure reducer: no I/O, no locks, no clock, no id minting. Every
non-deterministic input (ids, timestamps) arrives through the command, so the
host can commit the returned `GoalSnapshot` durably **before** publishing live
state and events (§10.1). `GoalDecision` carries the next snapshot (`None` after
`Clear`), a `Vec<GoalEffect>` for the host to execute, and a
`GoalTransitionEvent` for one transcript cell.

## Type-level guarantees

The two liveness defects proven for the legacy Stop-Hook goal (§7) are
**unrepresentable**:

- `GoalLifecycle::Active { lease: GoalLease }` — active always owns a queued or
  running lease. No ownerless active.
- `GoalLifecycle::Waiting { wake: GoalWake }` — waiting always carries a durable
  `WakeId`. No wakeless waiting. (Watcher *liveness* is a supervisor concern,
  reconciled level-triggered; the DTO only proves the obligation.)

Completion authority is **sealed**: `completed` is reachable only with a
`CompletionAuthorization`, which has no public constructor and is minted solely
by `authorize_completion` (private `seal` fn in `completion.rs`). It is
non-serializable, so it cannot be forged by round-tripping JSON. The reducer
re-checks it against the live snapshot (goal id / spec revision / running lease).

## Module map

| Module | Owns |
|--------|------|
| `id` | Branded newtypes: `GoalId`, `GoalLeaseId`, `WakeId`, `EffectId`, `EvidenceId`, `PlanArtifactId`, `VerificationAttemptId`, `ContentDigest`, and monotonic `SpecRevision`/`StateVersion`/`PlanRevision`, `Timestamp`. Reducer never mints these. |
| `text` | `BoundedText` — UTF-8-safe capped strings for durable model-visible fields. |
| `budget` | `GoalBudget` (NonZero limits), `GoalUsage`, `UsageDelta`, `GoalCounters`, `GoalTurnTrigger`. Autonomous cap vs token budget (§11.1). |
| `status` | `GoalLifecycle`, `GoalLease`, `GoalWake`, `GoalStatus`, `PauseReason`, `UsageLimitReason`, `BudgetKind`. |
| `disposition` | `GoalTurnDisposition`, `ProgressSignal`, `WaitCondition`, `WaitResolution`, `ModeGate`, `BlockerEvidence`, `RequirementCoverage`. Report vs runtime-derived signals kept separate (§12.2). |
| `completion` | `CompletionPolicy`, contract compilation (`ContractItem`/`DeterministicCheck`/`SemanticCriterion`), the sealed `CompletionAuthorization` + `authorize_completion`/`precheck_candidate`, `ProbeVerdict`. |
| `evidence` | `GoalEvidenceRecord` (runtime-owned provenance), `EvidenceRef`, `EvidenceSource`, `DurableResultRef`. |
| `plan` | `GoalPlanRef` (bounded reference, not the body), `PlanCheckpoint`, drift detection. |
| `snapshot` | `GoalSnapshot` — the durable versioned aggregate (§13.1). |
| `command` | `GoalCommand` + sub-structs. In-process values (not persisted). |
| `decision` | `GoalDecision`, `GoalEffect`, `GoalReminderKind`, `GoalTransitionEvent`. |
| `reducer` | `decide` and per-command handlers. |
| `error` | `GoalTransitionError` (thiserror). |

## Invariants enforced by the reducer

- Create/resume/wake/budget-resume that yields `active` commits a queued lease in
  the same decision (§9.1 invariant 9) and emits `ScheduleTurn`.
- A `wait` transition always carries a `GoalWake` and emits `RegisterWake`.
- Autonomous `StartTurn` beyond `max_autonomous_turns` → `budget_limited(turns)`;
  token overflow on continue → `budget_limited(tokens)`.
- Three signal-free continues → `paused(no_progress)` (safety net; the
  coordinator runs the boundary audit first).
- `Edit` compares `expected_spec_revision`; a budget-raise edit on a
  `budget_limited` goal atomically resumes it.
- `completed` requires a matching sealed authorization; a stale-spec
  authorization is rejected.

## Conventions

- Tests use companion `*.test.rs` files; shared builders live in `test_support.rs`
  (`#[cfg(test)]`). `allow-unwrap-in-tests`/`allow-expect-in-tests` are on.
- Commands are **not** serde types (they carry the non-serializable
  authorization capability); only `GoalSnapshot` and its parts persist.
