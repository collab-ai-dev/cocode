# Goal Runtime — Live Integration Plan

> Status: all phases landed (first-cut simplifications tracked below)
>
> Companion to `goal-architecture-redesign.md` (the normative design). That doc
> owns the *what*; this doc owns the *wiring* — the exact seams where the new
> goal-runtime crates plug into the live surfaces, and the order to do it safely.

## What is built (committed, tested)

The entire goal-runtime **domain + host brain** ships as two crates plus a
persistence format, all unit-tested and free of live-surface coupling:

| Layer | Crate / module | Purpose |
|---|---|---|
| Domain reducer | `core/goals` (`coco-goals`) | `decide` state machine, sealed `CompletionAuthorization`, budgets/waits/evidence. Ownerless-active and wakeless-waiting are unrepresentable. 81 tests. |
| Write lease | `app/session::lease` | OS-backed cross-process `SessionWriteLease` folded into `SessionStore`. 6 tests. |
| Transaction boundary | `core/goal-runtime::handle` | `GoalRuntimeHandle` (durable-before-visible) + `GoalStore` seam. |
| Completion | `core/goal-runtime::{coordinator,gate,evidence,verifier}` | Mandatory after-turn coordinator, sealed gate, evidence provenance, async verifier seam. |
| Context | `core/goal-runtime::materializer` | Typed `GoalTurnContext` + `PlanSource` seam. |
| Continuation | `core/goal-runtime::{supervisor,port,admission}` | Sole autonomous owner + `SessionTurnPort` seam + bounded admission. |
| Persistence format | `app/session::storage` | `MetadataEntry::GoalSnapshot`/`GoalCleared` + `latest_goal_snapshot` scan. |

The design's central decisions #1–#10 (first-class aggregate, lease, one logical
turn, sole supervisor, durable context, mandatory coordinator, sealed gate,
runtime evidence, plan-as-memory) are realized in these layers. What remains is
**adapters and cut-over** — connecting the seams to the live session runtime,
query engine, tools, protocol, and TUI, then deleting the Stop-Hook goal.

## Seam inventory (trait → concrete impl to write)

| Seam (in `coco-goal-runtime`) | Concrete impl home | Backed by |
|---|---|---|
| `GoalStore` | `app/agent-host` | `TranscriptIo::append_metadata` + `coco_session::latest_goal_snapshot`, under the write lease |
| `PlanSource` | `app/agent-host` (or `core/context`) | `PlanArtifactService` over the session plan file (`docs/internal/plan-mode-architecture.md`) |
| `CompletionVerifier` | `app/agent-host` / `app/query` | tool-capable review agent via the fork dispatcher (`coco_query::forked_agent`) |
| `SessionTurnPort` | `app/agent-host` | the local AppServer turn slot (`SessionTurnExecutor` / `AppServerLocalBridge::start_turn_and_wait_for_end`) |
| reminder adapter | `core/system-reminder` | render `GoalTurnContext` as an escaped meta fragment (untrusted data separate from instructions) |

## Delivered (all phases landed)

Phases 3-wiring through 10 are implemented, tested, and committed. Highlights:

- **Phase 3-wiring** — `TranscriptGoalStore` + `GoalRuntimeHandle` owned by
  `SessionRuntime`; OS-backed `SessionWriteLease` acquired before mutable-state
  reads with `session_in_use` rejection.
- **Phase 7** — `get_goal` / `report_goal_turn` / `create_goal` tools over the
  `GoalHandle` seam; `ToolName` variants added.
- **Phase 8** — `session/goal/{create,get,edit,setStatus,clear}` RPCs
  (`request_handlers/goal.rs`, routed by `SessionTarget`, `SessionRead` scope,
  `expected_spec_revision` on edit) and the full-snapshot
  `GoalSnapshotChanged` `ServerNotification` **replacing** `ActiveGoalChanged`.
  `coco_types::goal::GoalSnapshotView` is the bounded wire projection;
  `session/goal_view.rs` maps the durable aggregate at the boundary. Turn
  admission binds the queued lease at `turn/start` via `bind_turn`.
- **Phase 9** — TUI `SessionState.goal: GoalSnapshotView`, footer status pill,
  detail-modal resume prompt for stopped goals, `/goal pause` + `/goal resume`,
  and `Ctrl+C` pauses an active goal before cancelling (via
  `LocalServerClient::goal_set_status`).
- **Phase 10** — the Stop-Hook goal implementation is deleted:
  `ActiveGoal`, `ActiveGoalChangedParams`, `ToolAppState.active_goal`,
  `MetadataEntry::Goal` + `GoalMetadata`, `hooks::ManagedHookKind::Goal` (+ the
  `managed_by` field), the engine goal branches + helpers, and
  `restore_goal_from_history`. `ContinueReason::StopHookBlocking` stays for
  ordinary hooks with no goal semantics; continuation is owned by the runtime.

The SDK JSON schemas + Python bindings are regenerated for the new methods and
event.

## Landed beyond the first-cut

- **Per-turn goal-context reminder (§5.5, design #7)** — the session `GoalHandle`
  re-materializes the objective/budget/progress each running goal turn
  (`GoalContextMaterializer`), rendered with prompt-injection-safe separation
  (`session/goal_reminder.rs`) and injected through a mandatory
  `GoalContextGenerator` in the system-reminder pipeline. The worker is
  re-anchored to the goal every turn, surviving compaction.
- **Plan binding (§5.5)** — `SessionPlanSource` over the session plan file feeds
  the materializer a bounded `GoalPlanView` (headings / active steps / digest /
  drift), replacing `NoPlanSource` for live sessions.
- **Periodic completion probe (§12.5)** — the reminder nudges an
  apparently-finished worker to report every N autonomous continuations; no
  terminal authority, so the gate still owns completion.
- **Transcript cell per durable transition (§9.2)** — `finalize_goal_turn`
  returns the transition it enacted (completed / blocked / paused / budget /
  usage) and the engine appends one concise transcript cell, so the conversation
  carries a permanent record of autonomous goal transitions.
- **Runtime evidence-record minting (§10.2 #9)** — the engine mints a runtime-owned
  `GoalEvidenceRecord` (`ev-<tool_use_id>`) for every accepted tool result on a
  goal-owned turn, bound to goal/lease/turn; the goal-context reminder surfaces the
  citable ids, and the completion gate resolves them to prove ownership. A worker
  cites an id but can never mint one — a fabricated id fails ownership closed. The
  store is now **session-scoped** (shared by every per-turn handle and the driver's
  coordinator), so provenance survives across turns.
- **Separate-turn `GoalSupervisor` — Phase A (§10.3):** the concrete
  `SessionTurnPort` (`SessionGoalTurnPort`) runs one promptless, supervisor-owned
  goal turn (engine built with `with_goal_supervisor_owned_finalize`, so it runs
  one logical turn and the supervisor finalizes). A session-owned `GoalDriver`
  (spawned at bootstrap) owns the **cold edges the engine-hook loop cannot**:
  `/goal resume` auto-start, `waiting`-wake (timer-backed for
  deadline/backoff/reset), and restart reconciliation. This closes the confirmed
  liveness gap where resumed/waiting goals sat idle forever.

- **Separate-turn `GoalSupervisor` — Phase B landed (§10.3):** the engine is now a
  pure single-turn executor for goals. `finalize_goal_turn` no longer self-continues
  (returns `Stop`, leaving the goal `active+queued`); `ContinueReason::GoalContinuation`
  is deleted; the `GoalDriver` owns every continuation. The driver holds the
  `TurnCoordinator` slot across each autonomous burst (`SessionBurstScheduler` +
  `start_active_turn`, released by a panic-safe `SlotReleaseGuard`), serialising
  autonomous turns against user turns exactly as the warm loop did — which also
  closed the Phase A slot caveat. After a user goal turn frees the slot the AppServer
  forwarder (`session/events.rs::forward_terminal_event`) nudges
  `goal_driver_edge()`, so continuation resumes race-free.

## Remaining backlog (tracked, not blocking)

- **Live validation:** the cut-over is compile- and unit-test-green but has not been
  exercised against a live model + AppServer; run a real autonomous goal end-to-end
  before relying on it.
- **Task-completion waits:** the wake scheduler services wall-clock waits
  (deadline/backoff/reset) via timers; `waiting(task)` needs a `TaskManager`
  subscription seam.
- **Live surface routing of autonomous turns:** the supervisor turn commits durable
  history but does not yet stream its events to attached surfaces (the driver drains
  the engine event channel); a follow-up routes them through the AppServer outbound
  (`notif_tx` / `forward_turn_events`), which is host-level — so the driver's
  slot/turn ownership ultimately belongs at the AppServer-host layer.
- **Completion verification (§12.3):** an optional stricter semantic contract
  verifier for `contract_checks_and_verifier` (the default
  `candidate_with_evidence` path is already design-faithful via the structural gate).

## Key integration seams (verified file references)

- Turn finalization must cover **both** tails, not just the text-only Stop-hook
  path: `engine_terminal.rs::handle_no_tool_calls_terminal` **and**
  `engine_finalize_turn.rs::finalize_successful_turn_tail` (the tool path, where
  Stop hooks never fire today). The `GoalCompletionCoordinator` runs after every
  goal-owned turn, so both tails invoke it.
- Turn-end token delta rides `TurnEndedParams.usage: Option<TokenUsage>`
  (`event.rs`), computed from `UsageAccounting`; feed input+output into
  `UsageDelta`.
- `TurnId` is minted by the runner and threaded as `cycle_turn_id`
  (`engine_session.rs`); the `SessionTurnPort` adapter mints it and hands it to
  the engine, matching `GoalSupervisor`'s pre-minted-turn-id contract.

## Sequencing and safety

Do the phases in order; each is independently compilable. Land Phase 3-wiring and
Phase 8 behind the existing multi-session E2E harness before Phase 10 deletes the
old path, so the cut-over is atomic per surface (design §1: no transitional Stop
Hook). Run `just pre-commit` before each cut-over commit — these phases modify the
live engine and multi-session runtime, unlike the additive foundation crates.
