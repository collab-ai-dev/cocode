# coco-goal-runtime

Host orchestration layer over the pure [`coco-goals`] domain — turns reducer
decisions into durable, observable session state. Tier-3 main-trunk
(thiserror + `coco-error` `ErrorExt`). Depends only on `coco-goals`,
`coco-types`, `coco-error`, `tokio` (sync). Live wiring lives in the
consuming crates — see the last section.

## Key types

| Type | Purpose |
|------|---------|
| `GoalRuntimeHandle` | Session-local transaction boundary (§10.2). `apply(cmd)` runs `decide` under a tokio mutex, commits durably via `GoalStore` **before** advancing the live projection, and returns `AppliedGoalDecision { snapshot, effects, event }`. Sole writer of the live goal projection. |
| `GoalStore` | Narrow persistence seam: `persist` / `clear` / `load` (append-only; highest `state_version` wins). All under the session write lease. `InMemoryGoalStore` is the test/ephemeral double. |
| `AppliedGoalDecision` | The committed result the caller executes: typed effects + transition event. |
| `GoalRuntimeError` | `Transition` (reducer rejected) / `Store` (persist failed). `is_conflict()` marks stale-identity/version failures the caller resolves by refreshing. |
| `GoalCompletionCoordinator` | Mandatory, deterministic, non-LLM after-turn boundary (§6, §12). `coordinate(snapshot, GoalTurnResult) -> CoordinatorOutcome`: normalizes the disposition, gates completion candidates, runs the boundary audit before a no-progress/blocked stop, returns the `TurnFinishOutcome`. Cannot fail open. |
| `GoalCompletionGate` | The only path to a `completed` transition — thin wrapper over the domain's sealed `authorize_completion`; requires a running lease. |
| `EvidenceStore` | Records/resolves runtime-owned `GoalEvidenceRecord`s; unknown ids fail closed. `InMemoryEvidenceStore` double. |
| `CompletionVerifier` | Async policy-execution seam (deterministic checks / evidence review / semantic verifier). Never owns the transition. `AlwaysVerified/Rejected/Unavailable` doubles; the tool-capable impl lives in the session runtime. |
| `GoalContextMaterializer` | Builds the bounded typed `GoalTurnContext` from durable state + the current plan (§5.5). Untrusted fields stay data, not authority. `PlanSource` seam (concrete `PlanArtifactService` over the plan file lives in the session runtime); a missing plan → `ContextUnavailable`. |
| `GoalSupervisor` | Sole owner of autonomous continuation (§10.2). Level-triggered `advance()`: materialize → record `running` (before the port, closing the persist-then-schedule window) → start via `SessionTurnPort` → await the once-resolving completion → coordinate → `FinishTurn`. Idempotent; finalizes only if still running the same turn (handles concurrent pause/clear). |
| `SessionTurnPort` | Explicit session scheduling seam; `GoalTurnHandle.completion` resolves exactly once to an exhaustive `GoalTurnOutcome` (the port synthesizes an error if the runner exits without a turn end). The AppServer-backed impl lives in the session runtime. |
| `AutonomousAdmission` | Process-wide bounded concurrency for autonomous continuations; user-started turns are not routed through it. |

## Invariants

- **Durable before visible** (§10.1): a failed `persist` leaves the projection
  unchanged, so a crash never exposes state without its durable record.
- The handle never schedules turns or registers wakes itself — it returns
  `GoalEffect`s for the supervisor/session-runtime to execute.

## Live-wiring seams (implemented by)

Concrete impls live in the consuming crates:

- `GoalStore` → `TranscriptGoalStore`; `PlanSource` → `SessionPlanSource`;
  `SessionTurnPort` + supervisor driver → `SessionGoalTurnPort` — all under
  `app/agent-host/src/session/` (`goal_store.rs` / `goal_plan.rs` / `goal_driver.rs`).
- Reminder adapter → `goal_reminder::render_goal_context` (agent-host), feeding
  the `goal_context` generator in `coco-system-reminder`.
- Goal tools (`GetGoalTool` / `ReportGoalTurnTool` / `CreateGoalTool`) →
  `core/tools` (`goal_tools.rs`, default-registered), reaching the runtime via
  `coco-tool-runtime::GoalHandle` (impl: `goal_tool_handle.rs`).
- `CompletionVerifier` — live wiring passes the `AlwaysVerified` double (the
  coordinator's deterministic gates carry the audit); a tool-capable /
  model-backed verifier is not yet implemented.
