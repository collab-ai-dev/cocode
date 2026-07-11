# Goal Runtime Audit and Redesign

> Status: design proposal
>
> Date: 2026-07-11
>
> Scope: persistent goals, autonomous continuation, cross-turn context, execution-plan
> handoff, system reminders, and completion decisions in `coco-rs`, Hermes Agent,
> and Codex.

## 1. Executive decision

The reported difference in execution strength is real. It is not explained only by
prompt quality or model choice.

The current `coco-rs` happy path does perform autonomous work. A goal is installed as
a session-scoped Stop Prompt Hook. When its judge returns `unmet`, the hook blocks
termination and the same `QueryEngine::run` loop continues. The feature is therefore
not a stub.

However, the implementation violates the central liveness invariant of a persistent
goal:

> If a goal is `active`, the runtime must be able to identify exactly one owner that
> is running work, has queued the next turn, or is waiting on a registered wake-up.

There are concrete paths in `coco-rs` where the goal remains active, the current
engine returns, and no component owns the next continuation. These paths include
judge timeout or invalid output, hook orchestration errors, provider API errors,
query turn or token limits, background-task deferral, interruption, and session
resume. This produces the observable state "goal active, agent idle."

The root cause is an abstraction mismatch. A persistent goal is session-level work,
but `coco-rs` implements it as a query-local hook. A hook is suitable for observing or
vetoing a lifecycle point. It is not a good owner of durable state, budgets, idle
scheduling, wake conditions, recovery, or cross-surface concurrency.

The target decision is:

1. Make a goal a first-class session aggregate.
2. Require one storage-owned cross-process write lease for every writable session
   materialization; multiple sessions may still share a workspace.
3. Make `QueryEngine` execute exactly one logical turn.
4. Give a session-scoped `GoalSupervisor` sole ownership of autonomous continuation.
5. Start each continuation as a new logical turn in the same session; do not fork the
   worker conversation for ordinary goal progress.
6. Keep the durable goal snapshot outside model-visible conversation history and
   outside compaction summaries.
7. Re-inject the authoritative objective, budget, plan reference, and completion
   contract on every autonomous goal turn through one typed goal-context reminder.
8. Run a mandatory `GoalCompletionCoordinator` after every goal-owned turn. Workers
   submit candidates, while only `GoalCompletionGate` may validate evidence and
   persist completion. Natural turn termination is never proof of completion.
9. Bind completion proof through runtime-generated durable evidence records; workers
   may cite evidence but cannot mint or rebind its provenance.
10. Treat the plan file as durable working memory and a requirement index, not as the
   authority for goal status or proof of completion.
11. Remove the existing `ActiveGoal + ManagedHookKind::Goal + GoalStatusPayload`
   compatibility semantics. Backward compatibility is out of scope.

There is no transitional Stop Hook design. Implementation proceeds directly to the
first-class goal runtime and cuts every surface over to it in one authority change.
The existing Stop Hook implementation remains only as the audited baseline until it
is deleted; it receives no new goal state, budget, reminder, wake, or scheduling
responsibilities.

## 2. Audit scope and baseline

This analysis uses the following local revisions:

| Project | Revision | Revision date |
|---|---|---|
| `coco-rs` | `d34d08a3a6b8349185284a7156f23b7c9044603a` | 2026-07-11 |
| Hermes Agent | `4aa499ff9f3fcc0c38ce61da46805a4dcc8f612e` | 2026-07-11 |
| Codex | `6ff670bd030f7f94ce956d8a176c226deb427666` | 2026-07-02 |

The audit follows the complete lifecycle rather than comparing command count or UI
polish:

1. How a goal becomes active.
2. Who decides what happens after a turn.
3. Who owns continuation scheduling.
4. What context the next worker turn receives.
5. Whether a continuation forks, appends, or resumes from a summary.
6. How execution plans survive turns, compaction, resume, and explicit forks.
7. How system reminders re-anchor the worker.
8. How provider, hook, task, usage, and persistence failures change state.
9. How user work, pause, clear, and queued continuation races are resolved.
10. How completion is claimed and what evidence can prove it.

In this document, execution strength means observable runtime properties:

- **Liveness:** an active goal cannot silently stop.
- **Persistence:** progress continues without a user message saying "continue."
- **Recoverability:** wait, failure, compaction, and restart have explicit re-entry
  paths.
- **Convergence:** terminal claims, budgets, and anti-loop policies are defined.
- **Preemption:** committed human turns take priority at idle admission; explicit
  interruption cancels autonomous work, while opt-in mid-turn steering remains
  attached to the running turn.
- **Context fidelity:** the worker retains or reconstructs the information required
  to pursue the original objective.
- **Verifiability:** completion is supported by authoritative evidence.

## 3. Current implementation models

### 3.1 `coco-rs`: goal as a Stop Hook

The principal flow is:

```text
/goal <condition>
  -> ToolAppState.active_goal = Some(...)
  -> register a session-scoped Stop Prompt Hook
  -> run QueryEngine with a kickoff prompt
  -> model reaches a natural stop
  -> execute_stop() calls a separate prompt judge
       unmet      -> blocking error -> StopHookBlocking -> same engine loop continues
       met        -> remove hook + clear active_goal -> engine returns
       impossible -> remove hook + clear active_goal -> engine returns failed
       judge error/timeout/parse error -> generic hook fail-open -> engine returns
```

Primary evidence:

- `coco-rs/app/agent-host/src/goal_command.rs:188-205` registers the goal as
  `HookEventType::Stop` with `HookHandler::Prompt`.
- `coco-rs/app/query/src/engine_stop_hooks.rs:232-261` sets
  `ContinueReason::StopHookBlocking` only for a blocking verdict.
- `coco-rs/app/query/src/engine_stop_hooks.rs:270-281` returns
  `StopHookDecision::Continue` for an aggregate without a terminal verdict and for
  orchestration errors.
- `coco-rs/app/query/src/engine_terminal.rs` interprets that `Continue` as permission
  for the normal turn to finish. Only the separate query token-budget continuation
  policy may run again, and that policy is disabled by default.

Strengths:

- The implementation is small and reuses an existing Stop Hook loop.
- An `unmet` verdict immediately continues work.
- The judge receives a bounded recent transcript instead of only the final assistant
  paragraph.
- Achieved, unmet, and failed status attachments include duration, iteration, and
  output-token statistics.

Weaknesses:

- A goal iteration is hidden inside one logical turn, so user preemption, lifecycle
  events, and accounting do not align with goal progress.
- Hook configuration and trust policy can structurally disable the core goal feature.
- There is no goal-level turn budget, token budget, pause state, blocked state,
  usage-limited state, or registered waiting state.
- Authority is split among `HookRegistry`, `ToolAppState.active_goal`, transcript
  `GoalStatusPayload`, and `MetadataEntry::Goal`.
- Resume reconstructs in-memory state and the hook but does not start work.
- The kickoff prompt states the objective once, but there is no goal-specific durable
  execution-plan handoff or mandatory reminder on every autonomous iteration.

### 3.2 Hermes: after-turn judge and adapter queue

The principal flow is:

```text
/goal <text>
  -> persist GoalState in SessionDB.state_meta[goal:<session_id>]
  -> queue goal text as a normal user turn
  -> complete one normal turn
  -> GoalManager.evaluate_after_turn(last_response)
       done     -> status = done
       continue -> queue a continuation prompt as a new normal user turn
       wait     -> persist pid/session/deadline barrier
       parse failure x3 -> paused
       max goal turns   -> paused
```

Primary evidence is in `hermes_cli/goals.py`:

- Lines 3-25 describe the after-turn judge, normal-user-message continuation,
  fail-open-to-continue behavior, and user preemption.
- `judge_goal()` at approximately lines 836-964 maps auxiliary-client or API failure
  to `continue`.
- `GoalManager.evaluate_after_turn()` at approximately lines 1382-1555 owns turn
  budget, wait, done, parse-failure pause, and continuation decisions.
- CLI, gateway, and TUI gateway call the same manager at their post-turn boundary and
  then enqueue its prompt in surface-specific queues.

Hermes contains two distinct per-turn mechanisms, and the distinction matters for
this design:

- `agent/turn_finalizer.py::finalize_turn()` is the universal deterministic
  chokepoint. Every `run_conversation` turn passes through it unconditionally. It
  persists the session, computes a heuristic completed flag, and emits stuck-turn
  diagnostics. It never calls a model.
- The goal judge is goal-gated, not universal. Surface wiring invokes
  `evaluate_after_turn()` after every turn, but the manager early-returns unless a
  goal is active (`cli.py:9195-9196`).

The mandatory boundary and the model judge are therefore already separable
components inside Hermes itself, not one fused mechanism. Section 6.1 builds on
this observation.

Strengths:

- One continuation is one normal turn, so user messages can naturally win through
  FIFO ordering.
- A default 20-turn cap prevents an unbounded loop.
- Three consecutive unparseable judge responses pause instead of burning the entire
  budget.
- API failure has the correct liveness meaning: keep pursuing the goal.
- Goal state persists in SessionDB and can survive resume and session rotation.
- A completion contract, subgoals, and background-process waits are supported.

Weaknesses:

- CLI, gateway, and TUI gateway contain post-turn integration logic, which risks
  surface drift.
- Every turn pays an additional judge latency and token cost.
- The judge normally sees only the final approximately 4 KB assistant response and
  cannot directly inspect the worktree or external state.
- "Blocked" or "unachievable" can be classified as `DONE`, weakening terminal
  semantics.
- Persistence errors are often swallowed, and there is no goal revision or compare-
  and-swap boundary.
- Waiting is checked lazily; there is no single timer/task wake supervisor for all
  wait kinds.

### 3.3 Codex: persisted aggregate and lifecycle extension

The principal flow is:

```text
external /goal or create_goal
  -> persist ThreadGoal(goal_id, status, budget, usage)
  -> enable goal accounting in GoalRuntimeHandle
  -> thread becomes idle
  -> try_start_turn_if_idle(goal contextual input)
  -> agent works
       update_goal(complete | blocked) -> explicit terminal status
       no update_goal                  -> turn stops -> thread idle -> next turn
       tool finishes                   -> update token/time accounting
       budget reached                  -> inject bounded wrap-up steering
       turn error                      -> blocked
       usage limit                     -> usage_limited
```

Primary evidence:

- `codex-rs/ext/goal/src/extension.rs` connects goal behavior to thread start,
  resume, idle, stop, turn start/stop/abort/error, token usage, and tool finish.
- `codex-rs/ext/goal/src/runtime.rs::continue_if_idle()` reads durable state and calls
  `try_start_turn_if_idle`; the continuation owner is the thread lifecycle, not a UI.
- `codex-rs/ext/goal/src/tool.rs::handle_update()` permits the agent to claim only
  `complete` or `blocked`; pause, resume, budget, and usage statuses remain under
  user or system control.
- `codex-rs/state/src/model/thread_goal.rs` defines durable status, `goal_id`, and
  token/time accounting.
- `codex-rs/ext/goal/templates/goals/continuation.md` re-injects the full objective,
  budget, current-state evidence policy, completion audit, and blocked audit.

Strengths:

- A goal is a first-class thread concept.
- Thread idle lifecycle is the single automatic continuation owner.
- A durable `goal_id` plus expected-id accounting prevents late work from an old goal
  mutating a replacement goal.
- There is no extra judge in the ordinary after-turn hot path.
- Tool-finish accounting can detect a budget boundary before the turn naturally ends.
- Resume, turn error, usage limit, and external mutation have explicit lifecycle
  behavior.
- Recovery from an error stop is a first-class flow: external
  `set_thread_goal(status: Active)` immediately re-arms continuation
  (`ext/goal/src/runtime.rs::apply_external_goal_set()` calls
  `continue_if_idle()`), and the TUI prompts to resume a `Paused`, `Blocked`, or
  `UsageLimited` goal after thread resume
  (`tui/src/app/thread_goal_actions.rs:75-81`). A goal blocked by a provider error
  re-runs after one user confirmation.

Weaknesses:

- Completion is still an agent self-report. The host does not independently verify
  the final state.
- The required three-turn blocker streak is primarily enforced by prompt and tool
  description, not durable host-side evidence.
- Token budget is optional and there is no default goal-turn cap.
- `update_plan` is a turn event and a conversation tool call, not an independent
  durable goal plan. The goal continuation prompt recommends keeping it current but
  does not re-inject the latest plan content.
- The implementation assumes a reliable thread lifecycle and persistent state store.
- The interrupt/abort path accounts usage but never emits the idle lifecycle
  (`core/src/tasks/mod.rs` emits it only from the graceful `on_task_finished`
  path), so an interrupted goal stays `Active` with no continuation owner until an
  unrelated task finishes, a thread resume, or an external goal mutation occurs.
  This is the same active-but-idle defect class proven for `coco-rs` in section 7.
- `BudgetLimited` is modeled as terminal (`is_terminal()`), is excluded from the
  TUI resume prompt, and exits only through a budget edit or an explicit external
  status write. A purely budget-capped goal has weaker resume ergonomics than a
  blocked one.

## 4. Context retention, forks, and summaries

### 4.1 Four different context concepts

The comparison must separate four concepts:

1. **Worker context:** messages visible to the agent that executes tools.
2. **Completion context:** information available to the component deciding whether
   the goal is complete.
3. **Durable control state:** objective, status, budget, versions, and wait condition
   that survive process loss.
4. **Plan memory:** the current execution decomposition and progress markers.

Persisting the transcript does not mean all transcript text remains model-visible.
Keeping worker history does not mean a separate completion judge sees it. Keeping a
plan file does not make the plan a scheduler or completion authority.

### 4.2 Comparison

| Project | Ordinary goal continuation | Worker context before compaction | Worker context after compaction | Completion context |
|---|---|---|---|---|
| `coco-rs` | No fork; the happy path stays inside the same `QueryEngine::run` | Same `MessageHistory`, including prior assistant/tool/result messages | In-memory authority is replaced with compact boundary, summary, recent retained rounds, and re-injected attachments | Separate Prompt judge receives a serialized transcript suffix: 64 KiB first, 32 KiB retry |
| Hermes | No fork; continuation is a normal user-role turn in the same conversation | Same `conversation_history` | Default `compression.in_place=true`: same session id with protected head, middle summary, and recent token-budget tail | Auxiliary judge receives goal, last approximately 4 KB response, optional contract/subgoals, and background snapshot |
| Codex | No fork; `try_start_turn_if_idle` starts a regular turn in the same thread | Same thread active history plus an internal contextual fragment | Same thread with replacement history containing selected user messages, summary, and re-injected initial/world context | No per-turn judge; the worker agent inspects current state and calls `update_goal` |

None of the three implementations forks a full independent agent history for every
goal turn. None summarizes at every turn. Summarization occurs only when context
compaction is triggered.

Hermes has one important legacy exception. With `compression.in_place=false`, it
ends the parent session, creates a continuation child, and migrates the goal. This is
fork-like at the persistence and session-id layer, but the child receives compressed
head/summary/tail context, not a full clone of the parent history. The current default
is in-place compression.

### 4.3 `coco-rs` context behavior

On the normal `unmet` path, `StopHookBlocking` does not create a new session or copy
history. Prior assistant text, tool calls, and tool results are already in the same
`MessageHistory`, so local continuity is strong. The trade-off is that goal iteration,
query budget, and user-preemption boundaries are obscured.

Compaction calls `replace_history_after_compact()` and makes the compacted messages
authoritative for subsequent model calls. `coco-compact::build_post_compact_messages()`
orders the replacement as boundary, summary, recent messages, attachments, and hook
results. The append-only transcript may still retain raw history for audit, but text
that has been summarized is no longer directly visible to the worker.

The summarizer may use a side fork or fallback model call. That is a summarization
implementation detail; it does not move the primary goal worker to a fork.

The goal judge has less capability than the worker. It is a `HookHandler::Prompt`, not
a tool-using `HookHandler::Agent`. `hook_llm.rs` limits the stop transcript to a recent
64 KiB suffix and retries at 32 KiB after a prompt-too-long error. Its prompt requires
a decision based on transcript evidence only. It therefore:

- sees recent tool results and possibly a compaction summary;
- cannot recover omitted details that the summary failed to preserve;
- cannot read a file, run a test, or query an external service;
- cannot distinguish a claimed test result from the current worktree result.

Resume scans `GoalStatusPayload` messages to reconstruct the condition rather than
loading one canonical durable goal record. Goal recovery therefore depends on message
recovery and attachment preservation.

### 4.4 Hermes context behavior

Hermes queues its continuation prompt as a normal user message in the same
conversation. Before compaction, the next worker turn receives prior messages rather
than starting statelessly.

`ContextCompressor.compress()` protects the head and a recent token-budget tail,
summarizes the middle, and rebuilds the live message list. In the default in-place
mode the session id remains stable. Pre-compaction database rows are soft-archived and
remain available for search or recovery, while the model sees only the compacted live
rows.

The completion judge sees much less than the worker: goal text, truncated final
assistant response, optional completion contract and subgoals, current background
processes, and time. Work completed in an earlier turn must be repeated as concrete
evidence in the final response or represented in the contract. The judge cannot
independently validate that evidence.

### 4.5 Codex context behavior

Codex starts a regular turn through the same `Session::try_start_turn_if_idle()`. It
does not call `fork_thread()`. Pending human work, active tasks, and Plan mode are
checked before automatic idle work is accepted.

Local compaction uses `replace_compacted_history()` to replace active model context
with bounded user messages, a summary, and re-injected initial/world-state context.
Thread identity remains unchanged. The goal row is stored independently, so a summary
cannot erase the objective. Every continuation re-injects the full objective and
budget from the durable row.

There is no separate completion judge. The worker can use tools to inspect files,
tests, PR state, or CI state and then call `update_goal`. This gives it access to
authoritative current state, but the host still trusts the worker's claim.

## 5. Execution plans and system reminders across turns

### 5.1 A plan file, a checklist tool, and a goal are different abstractions

The three projects use the word "plan" for different mechanisms:

- A **goal** defines the durable outcome and owns autonomous lifecycle.
- A **plan file** is a human-editable execution artifact containing approach,
  requirements, decisions, and verification steps.
- A **checklist tool** (`TodoWrite`, Hermes `todo`, or Codex `update_plan`) is a compact
  progress projection, often optimized for UI display.
- A **system reminder** makes selected state model-visible at a turn boundary. It does
  not make that state durable and must not own scheduling.

Merging these into one state object would create a new authority problem. The goal
must remain stable while the plan changes. A checklist may be stale without changing
goal status. A reminder may fail without deleting the underlying state.

### 5.2 What `coco-rs` already provides

`coco-rs` has the most complete plan-file lifecycle of the three projects, but the
current goal implementation does not use it as cross-turn goal memory.

The behavior is documented in `docs/coco-rs/plan-mode-architecture.md` and implemented
across `coco-context`, `coco-system-reminder`, query, session, and TUI layers:

- The main plan file is `<config_home>/plans/{session_slug}.md`.
- A sub-agent uses `{session_slug}-agent-{agent_id}.md`.
- The session slug is stable and resolved lazily.
- An explicit session fork copies the plan to a new slug so the child cannot clobber
  the parent plan.
- Resume first reuses transcript metadata, then recovers missing files from snapshots,
  tool inputs, or plan-file-reference attachments.
- Plan mode reminders include the path and whether the plan exists.
- A full or sparse plan-mode reminder is emitted on a five-human-turn cadence.
- Re-entry and exit reminders use one-shot latches.
- `TodoWrite` state is restored by scanning the latest paired tool call in the
  transcript, and todo reminders can re-display the current list after inactivity.

These mechanisms solve persistence and plan-mode guidance, not goal continuation:

- The `/goal` kickoff prompt does not establish or reference a plan file.
- Plan-mode reminders are gated on Plan mode; autonomous goal work normally runs in
  the execution mode.
- The five-turn cadence is insufficient as the only anchor for an autonomous goal.
- `TodoWrite` is an in-memory projection reconstructed from history, not durable goal
  control state.
- Compaction can preserve reminders, but no goal-specific policy guarantees that the
  latest plan and original objective are reintroduced together.

This is a missed integration opportunity, not a reason to replace the plan subsystem.

### 5.3 Hermes plan handoff

Hermes does not use a canonical per-goal Markdown plan file in its normal goal loop.
It uses an in-memory `TodoStore` attached to the agent:

- the `todo` tool reads or replaces/merges a structured checklist;
- every tool call returns the full list;
- a newly created agent hydrates the store from the latest correctly paired todo tool
  result in conversation history;
- after context compression, only pending and in-progress items are rendered into a
  bounded message beginning with "Your active task list was preserved across context
  compression";
- item count, item length, and hydration payload are bounded.

This is a useful pattern: active plan state is explicitly re-injected after the one
operation most likely to erase it. However, it is not a general per-turn system
reminder, and it remains transcript-derived. Ordinary goal continuation separately
repeats the goal and optional completion contract in each queued prompt. The goal and
todo list are not one atomic durable record.

### 5.4 Codex plan handoff

Codex `update_plan` is a TODO/checklist tool, not Plan mode and not a plan file.
`core/src/tools/handlers/plan.rs` parses `UpdatePlanArgs` and emits
`EventMsg::PlanUpdate`. AppServer translates that event to
`turn/plan/updated`, while TUI renders it as a plan history cell.

The model also sees the `update_plan` function call and its `Plan updated` result in
ordinary conversation history. That usually carries the checklist into the next turn
until compaction. The latest plan is not stored as a first-class field in the durable
goal row, and the goal continuation template does not render it. The continuation
template only advises the worker to use `update_plan` when work is meaningfully
multi-step.

Consequently, Codex has strong durable goal re-anchoring but weaker independent plan
handoff. A summary may preserve the plan semantically, but this is not a typed
guarantee.

### 5.5 Required target behavior for `coco-rs`

The target architecture should reuse the existing plan file but define its role
precisely.

The current plan subsystem is path-based: it derives a Markdown path from the
session slug and reads or writes that file directly. It does not yet provide an
artifact id, monotonic revision, atomic content-plus-digest observation, or an
external-edit subscription. Those capabilities are therefore an explicit extension
of the plan subsystem, not behavior that the goal runtime may assume already exists.

A small session-owned `PlanArtifactService` should be the only adapter that maps a
`PlanArtifactId` to the current plan path. It should:

- mint or recover the stable id for the session's current plan artifact;
- read bounded content and compute its digest from the same byte snapshot;
- maintain a monotonic observed revision when a new digest is accepted;
- expose fork-copy and resume-recovery through the existing plan lifecycle;
- report external edits as level-triggered digest changes rather than relying only
  on a lossy file-watch edge.

Goal code consumes this service and never invents a second path registry. The service
does not make the plan a goal-status authority.

#### Goal-to-plan reference

The durable goal snapshot may contain a bounded reference, not the plan body:

```rust
pub struct GoalPlanRef {
    pub artifact_id: PlanArtifactId,
    pub revision: PlanRevision,
    pub content_digest: Option<ContentDigest>,
    pub observed_at: SystemTime,
}
```

The exact public type belongs in the appropriate crate documentation when
implemented. The design invariants are:

- `artifact_id` is a session-owned opaque identifier. `PlanArtifactService` resolves
  it to the session-owned path; neither the model nor protocol clients can persist
  an arbitrary filesystem path as the goal's plan.
- The goal store never duplicates the Markdown body.
- The digest detects change and stale excerpts; it is not a security boundary.
- Plan revision changes do not implicitly change goal spec revision or objective.
- A missing or unreadable resolved plan produces an explicit context warning, not
  silent omission and not automatic goal completion.
- An explicit session fork copies the plan to a child slug. Ordinary goal turns never
  fork or copy it.

#### Plan binding at creation and after Plan mode

Two binding paths produce the same invariants:

1. **Plan-first goal.** When `/goal` is created while the session already has a
   plan artifact, creation offers binding the current artifact at its observed
   revision/digest, showing the plan path, digest, and headline so the user
   decides; a stale plan from earlier unrelated work can be declined or replaced.
   The objective remains the user text; the plan never becomes the objective. If
   the user wants the plan to define success, goal creation may draft a
   completion contract from the plan's requirement checklist, and that draft
   becomes authoritative only after explicit user approval at the observed plan
   revision. Later plan edits change plan revision only; the approved contract
   stays fixed until a `SpecRevision` edit.
2. **Mid-goal Plan mode.** Exiting Plan mode re-binds the current artifact by
   recording the new revision/digest and emits the one-shot
   `goal_plan_activated` reminder; the first autonomous turn must reconcile the
   plan against the unchanged objective (section 9.2).

Plan-goal association is structural, not semantic. The session owns exactly one
current plan artifact per slug, and `GoalPlanRef` binds by artifact id; there is
no runtime LLM judgment of whether a plan file "relates to" the goal. This
matches the existing non-goal plan lifecycle, which also binds by session slug
and recovers by reference, never by content similarity. Semantic consistency is
enforced at three existing points instead: digest-drift detection with the
`goal_plan_changed` reminder, the worker's reconcile instruction on the first
turn after Plan mode, and the gate's refusal to complete while the plan holds
unresolved requirements. A dedicated plan-relevance model judge is rejected for
the same reasons as the per-turn completion judge (section 6.1): it would add
per-edit latency and a new fallible authority to a question that identity
binding and drift detection already answer deterministically.

#### Cross-turn materialization

Before every autonomous goal turn, a `GoalContextMaterializer` should read:

1. the latest durable `GoalSnapshot`;
2. the referenced plan file, if present;
3. the current todo/task projection, if useful;
4. the latest bounded progress and blocker checkpoint;
5. the remaining goal budget.

It should produce one bounded typed context fragment. For a large plan it should
include the path, digest/revision, headings, active or unchecked steps, and explicit
verification commands, then instruct the worker to read the file before changing it.
It must not copy an unbounded plan body into every prompt.

At the end of a goal turn, the runtime records a bounded `PlanCheckpoint` containing
the observed revision/digest and short progress summary. The file remains the plan
content authority; the checkpoint only supports drift detection and the next prompt.

#### Reminder cooperation

The existing `coco-system-reminder` machinery should render and inject the fragment,
but it must not become the scheduler or durable store.

The target injection policy is:

- `goal_context`: mandatory on every autonomous goal turn;
- `goal_objective_changed`: one-shot steering when the user edits the objective;
- `goal_plan_changed`: one-shot notice when the file digest changed outside the
  current worker turn;
- `goal_wait_resolved`: one-shot attachment containing task/deadline completion;
- `goal_report_missing`: one-shot steering after an `Unreported` turn, restating
  that `report_goal_turn` is mandatory and which disposition fits the last turn;
- `goal_completion_probe`: one-shot steering after a `LikelyComplete` probe
  verdict, instructing the worker to submit a completion candidate with evidence
  or state what remains;
- ordinary plan-mode and todo reminders: remain supplementary and keep their current
  cadence.

`goal_context` also describes the `report_goal_turn` disposition protocol and makes
clear that a report is not terminal authority. The reminder improves model compliance;
the coordinator's mandatory invocation and synthesized `Unreported` are the hard
fallback when compliance fails.

The goal context is inserted as a meta/contextual user fragment. The objective and
plan are user-authored data, so XML tags or a `system-reminder` wrapper must not grant
them system authority. Static runtime instructions and untrusted data must be rendered
in separate fields.

The mandatory goal reminder bypasses optional reminder toggles because it is part of
the goal execution contract. If context materialization fails, the supervisor moves
the goal to `paused(context_unavailable)` rather than starting an unanchored turn.

#### Prompt-cache behavior

The reminder should be appended at the turn boundary rather than mutating old
messages. Stable instructions stay in the cached prompt prefix. Dynamic objective,
budget, plan digest, and active steps stay in a bounded suffix. Compaction may replace
conversation history, but it cannot replace `GoalSnapshot` or the plan file.

### 5.6 Plans do not prove completion

A plan file helps enumerate requirements, but checking every box is not sufficient
proof. The plan is agent-editable and can omit or accidentally narrow a user
requirement.

Completion policy must therefore use this precedence:

1. Original objective and explicit completion contract.
2. User-authored referenced specifications and acceptance criteria.
3. Authoritative current-state evidence.
4. Plan file as a requirement index and audit aid.
5. Conversation summary or assistant claims only as navigation hints.

If a plan still contains an explicit unresolved requirement, a verifier should reject
completion until the plan is reconciled or the requirement is shown to be obsolete.
The absence of unresolved plan items cannot by itself approve completion.

## 6. How the three projects decide completion

"Hard completion mechanism" has four separate meanings. Collapsing them caused an
important gap in the first version of this design:

1. **Orchestration trigger:** does runtime execute completion handling at a mandatory
   lifecycle boundary?
2. **Candidate discovery:** who notices that the objective may be complete?
3. **Terminal authorization:** who is allowed to persist `completed`?
4. **Semantic verification:** what proves that the candidate is actually correct?

| Project | Hard orchestration trigger | Candidate discovery | Terminal authorization | Semantic verification |
|---|---|---|---|---|
| `coco-rs` current | Stop lifecycle always enters Hook orchestration on the normal path | Separate Prompt judge | Goal-specific terminal handler clears Hook/live state | Bounded transcript only; no current-state tools |
| Hermes | Surfaces call `GoalManager.evaluate_after_turn()` after every turn, but it early-returns unless a goal is active; the unconditional per-turn chokepoint is the deterministic `finalize_turn()` | Auxiliary judge returns done/continue/wait | `GoalManager` persists `GoalState.status=done` | Model judgment over goal, final response, contract/subgoals, and background snapshot |
| Codex | Thread lifecycle guarantees that active state continues after idle and only goal APIs can mutate terminal state | Worker decides to call `update_goal(complete)` | Tool handler plus durable goal store accepts the controlled status transition | No independent verifier; host trusts the worker claim |

Hermes therefore has hard automatic **completion discovery orchestration**, although
the verdict itself is still a model judgment. Codex has a hard **terminal API gate and
continuation invariant**, but no independent automatic completion discovery. Its
prompt/tool contract strongly instructs the worker to claim completion, which is not
equivalent to a runtime guarantee that it will do so.

Primary references are Hermes `hermes_cli/goals.py::GoalManager.evaluate_after_turn()`
and Codex `ext/goal/src/tool.rs::handle_update()`,
`ext/goal/src/runtime.rs::continue_if_idle()`, and
`ext/goal/templates/goals/continuation.md`.

There is no universally correct algorithm for completion of an unconstrained natural-
language goal. The final runtime therefore combines the hard parts of both references:

1. A mandatory, non-LLM `GoalCompletionCoordinator` runs after every goal-owned turn,
   preserving Hermes's hard after-turn orchestration.
2. The worker may submit a structured completion candidate, preserving Codex's
   explicit terminal protocol, but cannot persist `completed` directly.
3. Deterministic contracts can independently create a system completion candidate.
4. No-progress, budget, blocked-candidate, and system-generated pause boundaries
   trigger a final independent completion audit so a forgotten worker report is not
   silently lost. Explicit human pause/interrupt bypasses verification and takes
   effect immediately.
5. Only `GoalCompletionGate` validates evidence and authorizes the durable terminal
   transition.

This removes the per-turn model judge without removing the per-turn hard completion
boundary.

### 6.1 Removing the per-turn model judge: evaluation

Removing the judge is a deliberate trade, not a free win. The decision holds
because the judge's costs are per-turn and its failure modes corrupt terminal
state, while the cost of removing it is bounded discovery latency.

| Property | Per-turn judge (Hermes) | This design |
|---|---|---|
| Completion discovery latency | Same turn | Same turn with a worker report or contract hit; otherwise the probe creates a bounded re-check opportunity, while guaranteed stopping remains the nearest hard boundary |
| Evidence quality | Final ~4 KB assistant prose; no tools | Durable evidence records, deterministic checks, candidate-time evidence review, and an optional stricter contract verifier |
| False completion risk | Judge can mark DONE from prose; no blocked verdict exists, so an impasse can terminate as done | Gate validates identity, coverage, and evidence ownership; prose can never complete |
| Failure mode | Fail-open on parse/API error erases the boundary it was meant to provide | Coordinator is deterministic and cannot fail open; candidate-time verification can be unavailable without erasing the coordinator boundary |
| Marginal cost | One model call of latency and tokens on every goal turn | Zero model calls on ordinary progress turns |

The bounded-latency claim must be stated honestly:

- If the worker reports a completion candidate or a deterministic contract fires,
  discovery is same-turn, equal to Hermes.
- If the worker under-reports and no contract exists, discovery waits for the
  nearest boundary: three signal-free turns, the turn cap, or the token budget.
  The worst case is a worker that keeps generating real progress signals after the
  objective is met; it runs until the turn cap before the boundary audit fires.
- Under the default `candidate_with_evidence` policy the boundary audit cannot
  semantically prove completion of a free-form objective. Its job at the boundary
  is to prevent a wrong pause when gate-checkable coverage and evidence already
  exist, and otherwise to surface unverified-completion detail alongside the
  original stop transition. The backstop is only as strong as the persisted
  policy: deterministic contracts and configured semantic verification give it teeth, and
  users who need unattended completion detection should configure one of them
  (section 12.3).

Mitigations that keep discovery latency low without a per-turn model call:

- the mandatory `goal_context` reminder describes the `report_goal_turn` protocol
  on every autonomous turn;
- an `Unreported` turn triggers the one-shot `goal_report_missing` steering
  reminder on the next turn (section 5.5);
- deterministic contract signals are evaluated on every coordinator pass, so a
  contract-covered goal completes in the same turn its checks go green.

A per-turn model probe was rejected: it would reintroduce per-turn latency and
cost for exactly the goals least able to validate the probe's verdict, and its
prose-level evidence is the weakest input the gate could receive. What is adopted
instead is a bounded periodic completion probe (section 12.5) for goals without
deterministic check coverage: every N autonomous turns (default five), one model
assessment over structured durable state nudges an apparently finished worker to
report. The probe has no terminal authority and its failure is a safe no-op. It
creates a completion-discovery opportunity at the probe interval without claiming a
hard latency bound that the default free-form evidence policy cannot always prove.

## 7. Defect proof for the current `coco-rs` implementation

### 7.1 Proven: judge failure can create active-but-idle

The static path is:

```text
Prompt judge timeout / API error / invalid JSON
  -> HookEvaluationResult::Cancelled | NonBlockingError
  -> hook result is neither blocking nor a goal terminal verdict
  -> AggregatedHookResult contains no goal verdict
  -> run_stop_hooks() returns StopHookDecision::Continue
  -> QueryEngine terminal path returns normally
  -> active_goal and managed goal hook remain installed
  -> no scheduler starts another turn
```

This is a semantic inversion. Generic hook fail-open means "do not let an optional
hook break the agent." For a goal completion judge, the same value means "allow the
agent to stop pursuing an unfinished goal."

### 7.2 Proven: provider API error terminates without a goal transition

The provider API-error terminal branch records a skipped stop-hook decision and
returns. It does not clear, pause, block, or reschedule the active goal. The next user
message may incidentally make the hook run again, but no autonomous owner exists.

### 7.3 Proven: background-task deferral has no wake owner

When live background tasks exist, the stop-hook path temporarily removes the managed
goal hook so the judge does not declare completion while work is still running. It
then restores the hook and returns `Continue`. Existing tests assert the combination:

- goal remains active;
- managed hook is restored;
- engine may return.

No registered task-terminal wake schedules the next turn. The state is "waiting" in
behavior but still `active` in the model, with no wake handle.

### 7.4 Proven: query budgets do not transition goal state

The outer query loop can stop at `max_turns` or token budget and return directly.
These are query-level limits, not goal-level transitions. The active goal can survive
without becoming paused or budget-limited and without queuing a new logical turn.

### 7.5 Proven: resume restores state but does not restore liveness

Resume scans goal status attachments, rebuilds `ActiveGoal`, and reinstalls the Stop
Hook. It does not automatically start `QueryEngine`. Restoring the predicate without
restoring the continuation owner leaves an active goal idle until unrelated user work
arrives.

### 7.6 Proven by invariant, not subjective UX

The problem can be expressed mechanically:

```text
active(goal) => exactly_one(running_turn, queued_continuation, registered_wake)
```

Each path above reaches `active(goal)` while all three right-hand states are false.
This proves a runtime defect independently of model quality or user preference.

## 8. Comparative matrix

| Dimension | `coco-rs` | Hermes | Codex |
|---|---|---|---|
| Core abstraction | Special Stop Hook | Per-session GoalManager | Thread goal aggregate plus lifecycle extension |
| Continuation owner | Current `QueryEngine` | Surface post-turn glue | Thread idle lifecycle |
| Continuation boundary | Re-entry in one logical turn | New normal turn | New normal turn |
| Ordinary turn context | Same history | Same conversation | Same thread |
| Per-turn full fork | No | No | No |
| Compaction | Summary plus recent messages/attachments | Head plus middle summary plus recent tail | Selected user messages plus summary and initial/world context |
| Durable goal outside history | No single authority | Yes, SessionDB metadata | Yes, persistent goal row |
| Plan handoff | Durable plan file exists, but goal does not integrate it | In-memory todo restored from transcript and re-injected after compaction | `update_plan` event/tool call; no independent durable goal plan |
| Goal reminder | Kickoff plus Stop Hook judge reason | Objective/contract repeated in continuation prompt | Full objective/budget/audit repeated every continuation |
| Completion | Prompt judge | After-turn judge | Agent `update_goal` claim |
| Judge failure | Allows engine to finish | Continue | Not in ordinary hot path |
| Default runaway guard | No goal-level guard | 20 goal turns | No default goal-turn guard |
| Waiting | Detects live task but lacks wake owner | Process/session/deadline barrier | Lifecycle-driven, but no equivalent general wait contract |
| Error status | Active may remain orphaned | Usually continue; parse errors pause | Blocked/usage-limited/budget-limited |
| User preemption | Constrained by one engine loop | FIFO priority | Atomic idle-start checks; abort path does not re-arm continuation |
| Resume | Reinstall hook, no automatic turn | Restore manager; surface turn drives it | Active auto-continues; Paused/Blocked/UsageLimited prompt to resume; external `set(Active)` re-arms immediately |
| Durable identity | Condition text | Session key | `goal_id` and expected id |
| Authority count | Four competing locations | DB plus live object | Persistent row |

Hermes feels stronger primarily because every turn ends with an explicit decision and
the next normal turn is queued. Codex has the best target architecture because its
thread-level lifecycle owns continuation. The useful lesson is not its SQLite choice
or extension framework; it is first-class state plus a single idle owner.

## 9. Final technical decisions by concern

This section is normative. It turns the comparison into concrete `coco-rs` behavior.

### 9.1 Goal state, persistence, resume, and explicit forks

#### Reference comparison

| Question | Hermes | Codex | Final `coco-rs` decision |
|---|---|---|---|
| Durable owner | JSON in `SessionDB.state_meta[goal:<session_id>]` plus a live `GoalManager` | First-class `ThreadGoal` row in the state database plus a runtime handle | One versioned `GoalSnapshot` in session metadata plus a read-only live projection |
| Identity | Session-keyed; no goal revision | Stable `goal_id`; expected id protects accounting | `GoalId`, `SpecRevision`, `StateVersion`, `GoalLeaseId`, and idempotent effect ids |
| Write ordering | Live object and best-effort DB writes can diverge | State-store mutation precedes lifecycle projection | Append durable snapshot first, then publish live state and events |
| Resume | Load manager; a later surface turn drives progress | Restore accounting, then thread idle lifecycle continues active work | Materialize session, restore snapshot and wakes, then supervisor schedules at the first eligible idle edge |
| Active replacement | Re-setting mutates the session goal | API distinguishes existing unfinished goal and TUI confirms replacement | Reject replacement without the current goal id/spec revision and explicit confirmation |
| Branch/fork | Conversation branch copies history; compression rotation migrates the active goal | Goal is tied to its thread; automatic continuation stays on that thread | Ordinary continuation never forks; an explicit user branch copies the plan but does not clone an active goal by default |

The persisted snapshot conceptually contains:

- schema version, `GoalId`, session id, `SpecRevision`, and `StateVersion`;
- bounded immutable original objective, durable attachment references, and explicit
  user-approved amendments;
- optional completion contract and referenced specifications;
- status and typed status reason;
- current work state: queued lease, running lease/turn, or registered wait identity;
- plan artifact reference and last observed plan revision/digest;
- autonomous turn budget, optional token budget, and committed usage;
- current wait condition and registered wake identity;
- bounded progress, completion-rejection, and blocker checkpoints;
- created and updated timestamps.

Exact public field definitions remain owned by future crate documentation. The
cross-cutting invariants are fixed here:

1. There is at most one current goal per session.
2. Objective, contract, budget, and plan-binding edits compare `SpecRevision`.
3. Runtime transitions serialize under the session goal lock and validate
   `GoalLeaseId`; pause, clear, and interrupt are not rejected merely because usage
   advanced `StateVersion`.
4. A snapshot append and its `StateVersion` are the commit point and event-order key.
5. Protocol/TUI state is never reconstructed from rendered transcript messages.
6. Goal state is not copied into compaction summaries as an authority.
7. A new goal receives a new id even if the objective text is identical.
8. Clear appends an audit event and removes the current snapshot projection.
9. Creating, resuming, waking, or continuing an active goal commits a queued lease in
   the same snapshot transaction; it never commits ownerless `active` first and queues
   work later.

Large pasted specifications, images, and other rich objective inputs are materialized
as session-owned artifacts before the goal commit, following the useful Codex
`goal_files` pattern. The snapshot stores bounded text and opaque artifact ids, never
ephemeral TUI paste handles or arbitrary model-supplied paths.

The existing append-only session JSONL remains sufficient, but the single-writer
assumption must be enforced by session storage rather than inferred from one process's
`SessionRuntime` registry. Two coco processes may share a workspace, but they must not
materialize the same session for mutation at the same time. This is a session-wide
rule, not a goal-specific workaround: it also protects transcript appends, metadata,
resume recovery, usage snapshots, plan binding, and every future session mutation.

#### Session write lease

Following the existing focused-store-trait pattern, `coco-session` should add a lease
capability and include it in the combined `SessionStore` boundary, conceptually:

```rust
pub trait SessionLeaseStore: Send + Sync {
    fn require_write_lease(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionWriteLease, SessionLeaseError>;
}

pub trait SessionStore:
    TranscriptIo + AgentTranscriptStore + UsageSnapshotStore + SessionLeaseStore
{}
```

The exact trait placement remains crate-doc owned, but the semantics are fixed:

1. `SessionCatalog::resolve` may first locate the storage namespace without reading
   mutable session state. Runtime construction then acquires the lease before reading
   state that resume may repair or mutate and reloads the latest transcript under the
   lease. Loading state first and locking afterward is an invalid
   time-of-check/time-of-use sequence.
2. `SessionRuntime` owns the RAII lease for its full writable lifetime. AppServer
   surfaces attached to that runtime share the lease; they do not acquire competing
   leases.
3. Every mutating session-store operation requires either the matching
   `SessionWriteLease` or a writable store handle that encapsulates it. The lock is a
   capability enforced by the API, not an advisory caller convention.
4. Read-only listing, search, transcript inspection, and status discovery do not need
   the lease. An attempted writable resume of an already-owned session fails with a
   typed `session_in_use` error and owner diagnostics when available.
5. The lease is released only after turn admission stops, live turns drain or are
   persisted as resumable state, pending writes flush, and the runtime closes.
6. Explicit forks use a new session id and therefore a different lease. A branch
   never shares the parent's session lease.

For the file-backed `TranscriptStore`, use an OS-backed exclusive advisory lock on a
stable lock file such as `<project>/.session-locks/<session_id>.lock`. The lock file is
not removed during ordinary session deletion or close, avoiding inode-replacement
races. The kernel lock is authoritative; bounded contents such as pid,
process-instance id, and acquisition time are diagnostics only. Acquisition should be
non-blocking by default so TUI, SDK, and headless callers can surface the owner instead
of hanging. Process
death releases the OS lock automatically. A process-local registry keyed by the
normalized lock path supplements the OS lock so two runtimes in one process cannot
depend on platform-specific same-process lock semantics. `InMemoryStore` provides an
equivalent process-local exclusive lease.

The storage backend must advertise whether its lock is reliable for the configured
filesystem. If an OS or remote filesystem cannot provide the required exclusive-lock
semantics, writable materialization fails closed with `session_lock_unsupported`;
silently degrading to an unlocked writer is prohibited. A future remote session-store
backend may implement the same lease contract with a service-side lease instead of a
local file lock.

A new SQLite dependency is not justified only to imitate Codex. If concurrent writers
to one session are ever required, `coco-session` must replace this exclusive lease
contract with a real CAS/transaction protocol. Goal code must never add a private
database or private lock that bypasses the session store.

#### Resume matrix

| Persisted state | Resume behavior |
|---|---|
| Session write lease held by another process | Reject writable resume as `session_in_use`; offer read-only inspection or opening a fork with a new session id |
| `active` and execution mode | Restore plan/context, publish snapshot, then schedule at idle after pending human work |
| `active` while Plan or Review mode is selected | Normalize to `waiting(mode_gate)` and register a mode-change wake |
| `waiting(deadline)` | Re-register the timer; wake immediately when the persisted deadline is already due |
| `waiting(task)` | Reconcile task ids with `TaskManager`; re-register terminal subscriptions |
| `waiting(permission)` | Reconnect to the pending approval if recoverable; otherwise pause with `approval_recovery_failed` |
| `waiting(provider_backoff)` | Re-register the backoff timer with the persisted attempt count; wake immediately when the deadline already passed |
| `waiting(user_acceptance)` | Re-present the acceptance prompt with the validated candidate's evidence summary |
| `paused` or `blocked` | Do not auto-run; publish state and offer the explicit resume action defined in section 11.3 |
| `usage_limited` | Re-register the reset wake when a provider reset deadline is persisted; otherwise offer manual resume |
| `budget_limited` | Do not auto-run; resume requires a budget edit or an explicit completion decision |
| `completed` | Publish terminal state only |
| Missing plan artifact | Pause with `context_unavailable`; offer open/recover/replace actions |
| Corrupt or invalid latest snapshot | Load the last complete valid state version only when append-tail recovery proves it is the predecessor; otherwise fail closed as `paused(recovery_error)` |

An explicit conversation branch must not silently create two autonomous workers for
the same objective. The child receives its own copied plan artifact so edits cannot
clobber the parent, but starts without an active goal. TUI may offer "Clone goal as
paused" as an explicit operation; cloning creates a new `GoalId` and zeroed usage.

### 9.2 Plan mode and execution-plan lifecycle

#### Reference comparison

- Hermes has no equivalent first-class Plan collaboration mode in the normal goal
  loop. Its structured todo list survives history hydration and is re-injected after
  compaction, but it does not gate autonomous execution.
- Codex has the clearest safety boundary: `try_start_turn_if_idle()` rejects automatic
  work in Plan mode, and goal accounting clears the current goal binding for a Plan
  turn. Its TUI currently gives the Plan indicator precedence over the goal indicator.
- `coco-rs` already has the strongest plan-file mechanics: stable session paths,
  explicit-fork copying, resume recovery, full/sparse reminders, re-entry reminders,
  and plan-file editor integration.

The Codex behavior is explicit in `codex-rs/core/src/session/inject.rs` and
`codex-rs/ext/goal/src/extension.rs:199-224`. The existing `coco-rs` lifecycle is
owned by `docs/coco-rs/plan-mode-architecture.md`.

The final policy combines the Codex execution gate with the existing `coco-rs` plan
artifact:

1. Goal lifecycle and Plan mode are orthogonal state machines.
2. Entering Plan mode never clears, completes, or replaces the goal.
3. A goal created while Plan mode is active is persisted as
   `waiting(mode_gate=plan)`; no automatic turn starts.
4. Entering Plan mode during a goal-owned turn cancels that autonomous turn at the
   safe turn boundary and persists `waiting(mode_gate=plan)` before another turn can
   start.
5. Plan-mode turns do not consume autonomous goal turns, goal tokens, blocker streaks,
   or completion-verifier attempts.
6. `report_goal_turn` and autonomous wait/block tools are not exposed in Plan mode.
   User control-plane actions such as edit, pause, clear, and budget change remain
   available.
7. Exiting Plan mode resolves the current plan artifact, records its digest/revision,
   transitions the goal back to `active`, and schedules it at the next eligible idle
   edge.
8. Review mode uses the same automatic-work gate. It must not accidentally become an
   execution turn for the goal.

The plan file is mutable execution memory. It may refine approach, sequencing, risks,
and verification commands, but it cannot silently narrow the original objective or
completion contract. A user edit to the objective increments `SpecRevision`; an agent
edit to the plan increments plan revision only.

The first autonomous turn after Plan mode must receive:

- the unchanged authoritative objective;
- the current plan artifact id, path display, digest, and bounded active sections;
- a one-shot `goal_plan_activated` reminder;
- the instruction to reconcile the plan against the objective before executing it.

Plan mode and goal indicators remain simultaneously visible in the TUI. Hiding the
goal while Plan mode is active, as Codex currently does, obscures that autonomous work
will resume after mode exit. A compact rendering such as `PLAN | GOAL waiting` is
preferred over indicator replacement.

### 9.3 Completion and blocked decisions

#### Reference comparison

| Property | Hermes | Codex | Final `coco-rs` decision |
|---|---|---|---|
| Hard turn-finalization mechanism | `GoalManager.evaluate_after_turn()` after every turn | Active goal is continued by thread lifecycle; no automatic completion probe | `GoalCompletionCoordinator` runs after every goal-owned turn |
| Completion candidate | Judge returns `done` | Worker calls `update_goal(complete)` | Worker report, deterministic contract, or boundary audit |
| Terminal authority | GoalManager persists `done` | Controlled tool handler persists `complete` | Only `GoalCompletionGate` persists `completed` |
| Current-state tools | Judge has none | Worker has normal tools | Worker cites runtime-owned evidence; deterministic checks, candidate-time evidence review, and optional contract verification inspect current state |
| Judge/model outage | Continue | Not in the hot path | Coordinator is non-LLM; verifier outage pauses a worker candidate, while a boundary audit falls back to its original stop transition |
| Plan as proof | Contract may be quoted in response | `update_plan` is advisory | Plan is an audit index, never sufficient proof |
| Blocked behavior | Blocked/unachievable may become `DONE` | Worker self-reports after a prompt-defined repeated blocker | Separate typed blocked claim; never conflate blocked with completed |

Natural model termination means only "this logical turn ended." If the goal remains
active, the supervisor starts another turn. Every turn still passes through a hard
completion coordinator; there is no per-turn **model judge**.

Only a turn carrying a valid `GoalLease` may submit `GoalTurnDisposition`. This
prevents an unrelated human turn or stale sub-agent from proposing completion. The
report is a candidate input, not a status mutation. A human can still pause, clear,
edit, or explicitly request a goal-bound continuation through the control plane.

A worker completion candidate contains:

- goal id, expected spec revision, and running lease id;
- a bounded requirement-by-requirement result;
- evidence references to commands, test results, files, artifacts, or external state;
- the observed plan revision/digest;
- an assertion that no required work remains.

The coordinator decision order is:

1. Read the submitted disposition or synthesize `Unreported` when the worker omitted
   it.
2. Evaluate deterministic contract signals that changed during the turn.
3. At no-progress, budget, blocked-candidate, or system-generated pause boundaries,
   create a boundary-audit candidate even when the worker did not report completion.
   Explicit human pause/interrupt never waits for this audit.
4. Send all candidates to `GoalCompletionGate`.
5. The gate rejects stale identity, spec revision, status, plan observation, or lease;
   validates evidence ownership; runs contract checks and any required verifier; then
   persists `completed`, rejects a worker candidate back to `active`, or allows a
   rejected boundary audit to commit its original blocked/limited/system-paused
   transition. When required verification is unavailable, a worker candidate pauses
   as `verification_unavailable`, while a boundary audit commits its original
   transition with an audit-skipped annotation.

The completion contract is authoritative only when it is directly supplied or
approved by the user. An automatically drafted plan or model-generated checklist
cannot silently redefine success.

Do not copy Codex's prompt-only "same blocker for three turns" rule as a host
invariant. It wastes turns for obvious external impasses and is not actually enforced
by durable state. A blocked claim instead carries a typed dependency, attempted
actions, supporting evidence, and the user or external change required to proceed.
The runtime validates structure and identity, persists `blocked`, and stops scheduling.
Temporary asynchronous conditions use `waiting`, not `blocked`.

### 9.4 TUI and control-plane interaction

#### Reference comparison

- Current `coco-rs` exposes a goal badge, a status modal, transcript status cells, and
  `/goal clear`, but its UI is derived from split live/transcript state and has too few
  actionable statuses.
- Hermes surfaces goal continuation, pause, and completion mostly as transient status
  updates and CLI commands. It is informative but not a canonical state control
  plane.
- Codex is the strongest reference: AppServer owns `thread/goal/set|get|clear`, emits
  full `thread/goal/updated` snapshots, confirms unfinished-goal replacement, provides
  edit/pause/resume/clear actions, prompts after resume, and renders status plus usage
  in the footer.

Relevant source boundaries are `coco-rs/app/cli/src/tui_runner/goal_commands.rs`,
`coco-rs/app/tui/src/status_bar/builtin.rs`, Hermes
`ui-tui/src/app/createGatewayEventHandler.ts:469-482`, and Codex
`tui/src/app/thread_goal_actions.rs`, `tui/src/chatwidget/goal_menu.rs`,
`tui/src/chatwidget/goal_status.rs`, and
`app-server-protocol/src/protocol/v2/thread.rs:734-828`.

The final `coco-rs` TUI follows the Codex control-plane shape, with explicit plan and
wait visibility.

#### Commands and actions

| Interaction | Behavior |
|---|---|
| `/goal` | Open the current goal detail view; show usage help when absent |
| `/goal <objective>` | Create a goal; confirm before replacing any unfinished goal |
| `/goal edit` | Edit objective/contract through a dedicated editor; use expected spec revision |
| `/goal pause` | Atomically cancel queued/running autonomous work and persist paused |
| `/goal resume` | Validate context/plan, transition active, and schedule at idle |
| `/goal clear` | Confirm when work is running; cancel lease and append clear audit event |
| `/goal plan` | Open the resolved plan file through the existing editor handoff |
| `/goal budget` | Show or edit autonomous-turn and token budgets |

No command handler calls `QueryEngine` directly. Create/resume commits an active
snapshot with a queued lease and returns; `GoalSupervisor` observes that committed
work state and starts the turn through `SessionTurnPort`.

The detail view displays:

- objective and completion contract;
- selected completion policy;
- status, typed reason, spec revision, and state version;
- lifecycle owner: running turn, queued lease, or registered wake;
- plan file display path, revision/digest, and active steps;
- autonomous turns used/remaining, tokens used/budget, and elapsed active time;
- current wait condition or blocker evidence;
- last bounded progress checkpoint;
- last completion rejection and missing evidence;
- context-appropriate actions.

#### Status bar and transcript behavior

- The footer consumes the full current snapshot and shows status plus compact usage.
- Plan/Review mode and goal state compose; one never hides the other.
- `waiting` shows the wait kind, such as task, deadline, permission, or mode gate.
- Each durable status transition creates one concise transcript cell.
- Ordinary continuation turns do not add repetitive "goal continuing" transcript
  spam; running/queued progress belongs in the footer and detail view.
- Completion produces one terminal cell with verified evidence summary and final
  usage.

#### Resume, interruption, and stale UI operations

- Resuming an active goal does not ask for confirmation; it becomes visible and
  continues automatically once the session is eligible.
- Resuming a paused, blocked, usage-limited, or budget-limited goal opens an action
  prompt explaining what must change.
- `Ctrl+C` during a goal-owned turn atomically pauses the goal before cancelling the
  turn, preventing an immediate idle restart.
- A new ordinary human-turn request does not pause the goal and wins the next idle
  admission. Explicit mid-turn steering is a separate action and stays bound to the
  current goal lease.
- Objective/contract/budget edits include expected spec revision. Pause, clear, and
  interrupt target the current goal id and apply to the latest state, so frequent
  usage updates do not make safety controls spuriously stale. A real stale-id/spec
  error refreshes the snapshot instead of overwriting newer intent.
- Events for another session are ignored by the current view but retained by the
  session registry.
- Session switchers show a compact goal-status badge, and the global footer shows the
  count of active background goals. Switching the visible session does not silently
  pause an explicitly autonomous goal.
- Ephemeral sessions reject goal creation with a clear "persist the session first"
  action, matching the useful Codex behavior.

### 9.5 Additional concerns that are required for a complete design

The four primary topics are not sufficient by themselves. The following decisions
are also required:

| Concern | Failure if omitted | Final decision |
|---|---|---|
| Session write ownership | Two processes resume one session and append conflicting state versions | `SessionStore::require_write_lease` grants one writable runtime per session id across processes |
| Single scheduling owner | Duplicate or missing turns | Only `GoalSupervisor` starts autonomous work |
| Context and compaction | Objective or plan drift after summary | Re-materialize durable goal and plan context every autonomous turn |
| Human preemption | Goal competes with user work | A committed human-turn request wins idle admission; explicit mid-turn steering remains part of the running goal lease |
| Budgets and cost | Runaway spend | Default 20 autonomous turns; optional explicit token budget; total input+output accounting |
| Background tasks and deadlines | Active-but-idle waits | Durable typed wait plus registered task/timer wake |
| Permission prompts | Autonomous work hangs or bypasses authority | Persist `waiting(permission)`; never weaken normal tool permissions |
| Provider/rate-limit errors | Silent loop or silent stop | Typed retry policy and explicit limited/blocked/paused transitions |
| Idempotency | Duplicate accounting, completion, or starts | Goal id, spec revision, state version, lease id, and effect id on lifecycle writes |
| Multi-session isolation | Work starts in the wrong session | Every command, event, lease, and turn port call carries explicit session id |
| Cross-session fairness | One goal monopolizes provider/tool capacity | A small process-wide autonomous-admission semaphore uses FIFO/round-robin fairness among sessions in that process |
| Shared workspace concurrency | Two independent sessions or processes may intentionally use one checkout | Workspace sharing is allowed and is not a correctness lock; sessions remain isolated by session id, and current-state evidence is revalidated before completion |
| Sub-agents | Child completes or mutates parent goal incorrectly | Parent supervisor owns the goal; children return evidence/progress only unless explicitly delegated a scoped child contract |
| Explicit conversation fork | Two autonomous workers mutate the same workspace | Copy plan, do not auto-clone active goal; explicit paused clone only |
| Prompt injection | Objective/plan gains system authority | Escape and label user-authored data; keep static instructions separate |
| Rich objective attachments | Paste/image inputs disappear on resume | Materialize session-owned artifacts before goal commit and persist opaque references |
| Evidence provenance | A worker cites stale or borrowed tool output as proof | Runtime-generated `GoalEvidenceRecord`s bind durable results to goal, lease, turn, and tool/artifact identity |
| Model/tool configuration | Goal tools disappear mid-run | Recompute tool visibility per turn; pause if required goal tools are unavailable |
| Time accounting | Wall-clock jumps corrupt budgets | Use monotonic time while live and persist committed duration deltas |
| Empty-turn livelock | Model repeatedly stops without tools or progress | Track typed progress signals; three consecutive signal-free goal turns pause as `no_progress` |
| Process crash boundaries | Persisted active state has lost side effects | Durable-before-visible commits and restart reconciliation |
| Application shutdown | Goal is abandoned or shutdown races a new turn | Stop admission, revoke/flush live leases, persist resumable queued state, then close sessions |
| Observability privacy | Objective/code leaks into metrics | Metrics contain ids, enums, counts, and durations only |
| Resource cleanup | Stale task/timer subscriptions | Lease-scoped cancellation and drop guards on every terminal transition |
| Testing | Happy-path confidence hides liveness races | Reducer properties, integration races, fault injection, and multi-surface E2E |

`ProgressSignal` is a closed runtime enum produced by accepted tool observations,
workspace changes, plan changes, evidence records, task delegation, or registered
waits. Assistant prose alone is not a signal. Three consecutive goal turns with no
signal transition to `paused(no_progress)` and surface a replan/resume action; they do
not invoke an ordinary per-turn model judge. The no-progress boundary does run the
configured completion audit before pausing. Progress and report compliance are
tracked as separate counters: an unreported turn that still produced accepted
signals is not a no-progress turn (section 12.2).

## 10. Target architecture

### 10.1 Design principles

1. Goal state is a session aggregate, not a hook.
2. Scheduling, state transition, context rendering, and completion verification are
   separate responsibilities.
3. Durable state is committed before live projections and notifications change.
4. One OS-backed session write lease protects every writable materialization of a
   session id across processes.
5. Every autonomous turn has an explicit session id, goal id, spec revision, and lease id.
6. A committed human turn wins idle admission; explicit mid-turn steering has separate
   lease-bound semantics.
7. Goal context is reconstructed from durable state; it is not trusted to compaction.
8. Plan content remains a separate durable artifact.
9. Status transitions use exhaustive Rust enums and reducer tests.
10. The design reuses existing session, task, reminder, usage, and transcript
   infrastructure. It does not introduce a general actor framework or generic
   extension platform for one feature.

### 10.2 Component boundaries

```text
 SessionStore -- required SessionWriteLease --> SessionRuntime
                                                   ^
 user / SDK / TUI -> GoalCommandService -----------|
                                                   |
                                           GoalRuntimeHandle
                  durable commit | live projection
                               v
 Session metadata <---- GoalSnapshot + GoalEvent ----> protocol/TUI
                               |
                      pure GoalState reducer
                               |
     turn/task/idle events -> GoalSupervisor -> SessionTurnPort
                                  |                    |
                                  |                    v
                                  |              QueryEngine
                                  |                    |
                                  |                    v
 GoalRuntimeHandle <------ GoalCompletionGate <- GoalCompletionCoordinator
                                  |
                        GoalContextMaterializer
                         |        |         |
                  GoalSnapshot  PlanArtifactService  task/todo state
                                  |
                       typed goal reminder
```

#### `coco-goals`: pure domain crate

A small pure crate should own:

- status and command enums;
- immutable snapshot value objects;
- transition validation;
- budget accounting rules;
- wait-condition semantics;
- invariant checks;
- no Tokio, model client, filesystem, protocol, or UI dependency.

The core API should resemble a reducer:

```rust
pub fn decide(
    snapshot: Option<&GoalSnapshot>,
    command: GoalCommand,
) -> Result<GoalDecision, GoalTransitionError>;
```

`GoalDecision` contains the new snapshot and typed effects. The host executes effects
and commits the snapshot; the domain crate never performs I/O.

#### `GoalRuntimeHandle`: session-local transaction boundary

Owned by `SessionRuntime`, this component should:

- borrow the runtime's matching `SessionWriteLease`; it cannot exist in writable form
  without that lease;
- serialize mutations with a Tokio mutex;
- validate goal id, spec revision, state transition, lease id, and effect id as
  appropriate for each command;
- append the new snapshot/event to session persistence;
- update the live projection only after durable success;
- emit protocol events after commit;
- expose read-only snapshots to tools, TUI, and context materialization.

This is deliberately not a generic repository trait hierarchy. The existing session
metadata abstraction is sufficient.

#### `GoalSupervisor`: sole continuation owner

The supervisor consumes:

- session idle/resume;
- turn started/stopped/aborted/error;
- task terminal notifications;
- deadline wake events;
- goal mutation events;
- user queue state;
- usage and budget signals.

It performs an atomic claim using `(goal_id, lease_id)` under the goal transition lock
and starts work through `SessionTurnPort`. No TUI, SDK handler, hook, or command queue
may independently run an autonomous goal turn.

Queued and running ownership is represented by a durable lease record. This goal
lease is distinct from the session write lease: the session lease excludes a second
writer process, while the goal lease identifies one autonomous work attempt inside
the owning runtime. Starting a turn transitions `queued(lease_id)` to
`running(lease_id, turn_id)` under the current state version. An unfinished turn
commits the next queued lease as part of its stop transition. Resume treats a
persisted running lease as stale, reconciles any known turn result, and otherwise
replaces it with a new queued lease. This closes the persist-then-schedule crash
window rather than merely detecting it later.

The supervisor is level-triggered, not dependent on receiving every lifecycle edge.
Whenever session activity, a turn task, or a wake watcher changes, it reconciles the
durable snapshot against the AppServer turn slot and registered wake table. Missing or
duplicated notifications therefore cause an idempotent reconciliation rather than an
ownerless state.

#### `SessionTurnPort`: explicit session scheduling seam

The port starts a turn for an explicit `session_id` with typed contextual input. It
returns an owned handle rather than only a `turn_id`:

```rust
pub struct GoalTurnHandle {
    pub turn_id: TurnId,
    pub cancel: TurnCancelHandle,
    pub completion: GoalTurnCompletion,
}
```

`GoalTurnCompletion` resolves exactly once in memory to an exhaustive
`GoalTurnOutcome`, including completed, interrupted, provider/tool error, runner
panic, and event-channel closure. The port wrapper synthesizes an error outcome when
the underlying runner exits without `TurnEnded`; the supervisor must not infer turn
completion only from an optional protocol event. A first version may adapt the
existing local AppServer bridge or `QueryEngineRunner`. The multi-session target
resolves a `SessionHandle` through AppServer.

Before starting, the port passes through one small process-wide
`AutonomousAdmission` service. It provides bounded cross-session concurrency and
round-robin fairness for sessions in that process. It does not serialize by
workspace: distinct sessions and processes may intentionally share a checkout. It
does not own goal state or continuation policy; it only admits an already-durable
queued lease.

Do not repurpose `CommandQueue` as the idle scheduler. It is a mid-turn steering queue
and can contribute priority/origin information, but it does not own thread-idle turn
creation.

#### `GoalContextMaterializer`: bounded model context

This component combines durable goal state with the current plan and wait/task
results. It returns a typed value, not a pre-authorized string:

```rust
pub struct GoalTurnContext {
    pub goal_id: GoalId,
    pub spec_revision: SpecRevision,
    pub state_version: StateVersion,
    pub lease_id: GoalLeaseId,
    pub objective: String,
    pub budget: GoalBudgetView,
    pub plan: Option<GoalPlanView>,
    pub progress: Option<ProgressCheckpoint>,
    pub wait_resolution: Option<WaitResolution>,
    pub completion_contract: Option<CompletionContract>,
}
```

The reminder adapter escapes untrusted fields and renders stable instructions
separately.

#### `GoalEvidenceRecord`: runtime-owned provenance

Completion evidence cannot be ownership-checked from the current transcript shape
alone. Tool output, artifacts, tests, and external observations used as proof must
therefore receive a bounded durable envelope when the runtime accepts them:

```rust
pub struct GoalEvidenceRecord {
    pub evidence_id: EvidenceId,
    pub goal_id: GoalId,
    pub lease_id: GoalLeaseId,
    pub turn_id: TurnId,
    pub source: EvidenceSource,
    pub result_ref: DurableResultRef,
    pub content_digest: Option<ContentDigest>,
    pub observed_at: SystemTime,
}
```

The runtime creates these records from accepted tool completion, artifact writes,
deterministic checks, or registered external-state observations. The model may cite an
`EvidenceId` but cannot mint or rebind one. A report-time wrapper around an old tool
result is insufficient: provenance is captured when the result is produced. Existing
large tool output remains in its current durable storage; the evidence record is the
bounded ownership and integrity index. Provenance does not freeze mutable workspace or
external state: the gate re-runs approved checks or compares a fresh digest/observation
before completion, so another session's later workspace edit can invalidate otherwise
well-owned evidence.

#### `GoalCompletionCoordinator` and `GoalCompletionGate`

`GoalCompletionCoordinator` is invoked by `GoalSupervisor` for every goal-owned turn
result. It is deterministic orchestration, not an LLM judge. It normalizes the worker
report, synthesizes `Unreported`, evaluates changed contract signals, creates mandatory
boundary-audit candidates, and routes the result.

Lifecycle delivery is at least once. The durable coordinator decision is idempotent by
key `(goal_id, lease_id, turn_id, finalization_effect_id)`: duplicate stop/error
delivery replays a committed decision rather than emitting another transition.
External verifier execution cannot be guaranteed exactly once across a crash after
the call but before persistence. Each attempt therefore has a durable
`VerificationAttemptId`; completed results are persisted and reused, while recovery
may safely retry an attempt whose result was never committed. Correctness depends on
the idempotent durable transition, not on exactly-once model invocation.

`GoalCompletionGate` is the only component allowed to request the reducer's completed
transition. It validates identity, lease, plan observation, requirement coverage, and
evidence ownership before running the persisted completion policy. The normal public
`GoalCommand` set does not contain a directly constructible completed transition;
the runtime accepts only a sealed `CompletionAuthorization` produced by the gate's
domain validation path. Exact module privacy belongs in the crate design, but the
authority must be enforced by the Rust API rather than only by a comment. The
coordinator guarantees that completion handling occurs; the gate guarantees that a
candidate is not equivalent to completion.

### 10.3 `QueryEngine` boundary

In the target architecture, `QueryEngine::run` executes one logical turn:

- it does not read durable goal state to run an outer autonomous loop;
- a goal Stop Hook does not veto terminal behavior;
- it emits ordinary turn lifecycle, usage, and tool outcomes;
- the supervisor decides whether another turn is needed after the turn returns.

`ContinueReason::StopHookBlocking` may remain for ordinary hooks, but it no longer has
goal semantics. Goal execution cannot be disabled by generic hook policy.

## 11. Goal state machine

The Rust model should make ownerless active and a waiting state without a durable wake
identity unrepresentable. A Rust value cannot prove that a volatile watcher task is
alive, so the live `waiting => watcher registered or being reconciled` invariant is
enforced by `GoalSupervisor`, not claimed by the DTO alone. The following is an
illustrative internal shape, not the final public DTO definition:

```rust
pub enum GoalLifecycle {
    Active { lease: GoalLease },
    Waiting { wake: GoalWake },
    Paused { reason: PauseReason },
    Blocked { evidence: BlockerEvidence },
    UsageLimited { reason: UsageLimitReason },
    BudgetLimited { kind: BudgetKind, usage: GoalUsage },
    Completed { evidence: CompletionEvidenceSummary },
}

pub enum GoalLease {
    Queued { lease_id: GoalLeaseId, attempt: u32 },
    Running { lease_id: GoalLeaseId, turn_id: TurnId },
}
```

Use newtypes for goal, lease, spec revision, state version, and artifact identity;
closed enums for all reasons; and non-zero integer types for configured positive limits. The pure reducer
returns typed transition errors using the workspace error conventions. It does not
use string status matching, wildcard fallbacks, async locks, or I/O.

Recommended closed status set:

| Status | Automatic work | Entry cause | Exit |
|---|---|---|---|
| `active` | Running or queued | Create, resume, wake | Complete, block, pause, wait, limit |
| `waiting` | No turn; durable wake identity plus registered/recoverable watcher | Live task, deadline, permission, Plan/Review mode, provider backoff, user acceptance, external condition | Wake to active; user pause/clear |
| `paused` | No | User interrupt, no progress, unavailable context/verifier/scheduler | User resume |
| `blocked` | No | Typed, evidenced impasse or terminal execution error | User resume/edit |
| `usage_limited` | No | Provider/account quota | Reset wake or user resume |
| `budget_limited` | No | Autonomous-turn or token budget | Increase the exhausted budget and resume, or complete |
| `completed` | No | Gate-authorized completion candidate | New goal replaces it |

```text
none --create--> active --accepted completion--> completed
  \--create while mode-gated---------------------> waiting
                   |  \
                   |   +--repeated impasse/error--> blocked --------resume--------> active
                   +--interrupt/no-progress-------> paused  --------resume--------> active
                   +--task/deadline/mode/approval-> waiting --------wake----------> active
                   +--transient provider failure--> waiting(provider_backoff) --wake--> active
                   +--usage limit-----------------> usage_limited --reset wake/resume--> active
                   +--turn/token budget-----------> budget_limited --budget edit/resume--> active
```

Clearing removes the current snapshot but appends an audit event. A recoverable
current status named `cleared` is unnecessary.

### 11.1 Budgets

Recommended defaults:

- autonomous continuation turns: 20;
- token budget: none unless explicitly requested;
- completion probe interval: five autonomous turns, only for goals without
  completion-relevant check coverage;
- completion verifier attempts: one per gate evaluation;
- transient scheduler retries: bounded exponential backoff, at most three attempts,
  then `paused(scheduler_unavailable)`.

Query `max_turns` and goal continuation turns are different. The first limits the
agent/tool loop inside one logical turn. The second limits autonomous turns across the
goal lifecycle.

Token accounting should use total input plus output deltas from session
`UsageAccounting`, not an output-token-only display delta.

`GoalAccounting` records idempotent deltas keyed by `(goal_id, lease_id, effect_id)`
at tool-finish and turn-stop boundaries. This preserves mid-turn budget enforcement
without appending a snapshot for every streamed token. UI usage notifications may be
coalesced, but the final committed totals and budget transition are durable.

Only a turn started with a valid goal lease consumes goal turn/token/time budgets.
Ordinary human turns remain unbound even while a goal exists, so an unrelated user
question cannot consume or complete the goal. Human steering injected into an already
goal-bound turn inherits that turn's lease. This is more precise than Codex's current
non-Plan active-goal accounting and makes the budget specifically an autonomous-work
budget.

### 11.2 Background work and waits

When a goal-owned turn ends with live `TaskManager` work:

1. Allocate a wake id and prepare a watcher that is not yet allowed to mutate goal
   state.
2. Persist `waiting` with task ids, condition, and that wake id; discard the prepared
   watcher if the commit fails.
3. Activate the watcher only after commit, then immediately re-read its current
   task/deadline predicate. This closes the race where completion occurs between
   subscription creation and the durable `waiting` transition.
4. Release the running lease only through that same transition.
5. Do not run a completion judge and do not spend a continuation turn.
6. Transition back to active with a queued lease when the condition is satisfied.
7. Start a turn at the next idle edge with a typed wait-resolution attachment.

Deadlines require a durable timer owner or restart-time deadline scan. A textual
"wait" reason without a wake handle is invalid. `waiting { wake_id }` proves the
durable obligation to register a wake, not that a volatile watcher is currently
alive; supervisor reconciliation must recreate missing watchers and re-check their
level-triggered predicate after restart, watcher failure, or task-registry change.

### 11.3 Resume and re-run semantics

Every non-terminal stopped status has exactly one resume path. Resume is a reducer
command (`GoalCommand::Resume`) with exhaustive from-status matching, not a status
overwrite; the reducer rejects resume from statuses that require a different
action.

| From status | Resume trigger | Behavior |
|---|---|---|
| `paused` | User resume | Validate context/plan, commit `active` plus queued lease, schedule at idle |
| `blocked` | User resume or objective/contract edit | Same as paused; the next turn's materialized context includes the bounded blocker evidence and stop cause so the worker does not blindly repeat the failing action |
| `usage_limited` | Registered reset wake, or user resume | The wake commits `active` plus queued lease automatically when the persisted reset deadline passes |
| `budget_limited` | Budget edit (or explicit completion decision) | Raising the exhausted turn/token budget commits `active` plus queued lease; resume without a sufficient budget change is rejected with a typed error |
| `waiting` | Registered wake owns re-entry | An explicit user resume forces the wake early; it does not bypass wake validation |
| `completed` | Not resumable | A new goal receives a new `GoalId` |

Resume rules:

1. Resume targets the current goal id and applies to the latest state; like pause
   and clear, it is not rejected merely because usage advanced `StateVersion`. A
   stale-goal-id resume is rejected with a snapshot refresh.
2. Resume commits `active` and a queued lease in one snapshot transaction
   (section 9.1 invariant 9). "Resumed but idle" is therefore unrepresentable: a
   successful resume always has a scheduling owner. This mirrors the Codex
   behavior where external `set(status: Active)` immediately calls
   `continue_if_idle()` instead of waiting for an unrelated idle edge.
3. Resume re-materializes goal context and the plan artifact first; failure
   produces `paused(context_unavailable)` with recovery actions, never a silent
   no-op and never an unanchored turn.
4. Resume resets the no-progress streak, the unreported streak, and transient
   scheduler/provider retry counters. It does not reset turn, token, or time
   usage; budgets span the goal lifetime. Exhausting the default autonomous-turn
   cap enters `budget_limited(kind=turns)`, not generic `paused`, so a plain resume
   cannot create an immediately exhausted queued lease. The user must raise the turn
   budget through the same atomic budget-edit-and-resume path used for token limits.
5. A goal stopped by a provider error is re-runnable by design: transient
   failures wait under `waiting(provider_backoff)` and retry automatically, and
   non-retryable failures persist `blocked(execution_error)` and re-run after one
   explicit user resume. Both re-entry paths converge on the same queued-lease
   transition.

## 12. Completion protocol

### 12.1 Goal tools

Goal tools are conditionally visible, recomputed per turn by the standard tool
filter pipeline:

- `get_goal`: read objective, status, budget, usage, plan reference, and versions;
- `report_goal_turn`: submit progress, waiting, completion-candidate, or
  blocked-candidate disposition for the current lease;
- `create_goal`: available only when the user or system explicitly requested a goal.

Visibility rules:

- `report_goal_turn` and `get_goal` are registered only for a turn holding a
  valid goal lease, and are injected eagerly into that turn's tool list, never
  deferred behind lazy tool discovery: a mandatory protocol tool must be visible
  without a search. Ordinary human turns do not see `report_goal_turn` even
  while a goal exists, because they are not goal-owned and cannot submit
  dispositions (section 9.3).
- `create_goal` is registered only on explicit user or system request; it is not
  part of the ambient tool list.
- No goal tool is visible in Plan or Review mode (section 9.2).

The agent cannot mutate pause, resume, usage-limit, or budget-limit status. Those are
user/system control-plane operations. It also cannot write `completed` or `blocked`
directly.

Every goal-owned turn enters `GoalCompletionCoordinator` even when the worker never
calls `report_goal_turn`. While a goal remains active, finalization either commits the
next queued lease, a validated wait/block state, or a gate-authorized completion.

### 12.2 Turn disposition and completion candidates

The report uses a closed enum. The exact public DTO remains crate-doc owned:

```rust
pub enum GoalTurnDisposition {
    Progress {
        summary: ProgressSummary,
        next_step: NextStep,
        evidence: Vec<EvidenceRef>,
    },
    Waiting {
        condition: WaitCondition,
    },
    CompletionCandidate {
        coverage: RequirementCoverage,
        evidence: Vec<EvidenceRef>,
    },
    BlockedCandidate {
        evidence: BlockerEvidence,
    },
    Unreported,
}
```

`Unreported` is runtime-synthesized; it is not accepted from the model, and it can
never authorize completion. It feeds two separate counters that must not be
conflated:

- `unreported_streak` counts consecutive goal turns without a `report_goal_turn`
  call. It triggers the one-shot `goal_report_missing` steering reminder
  (section 5.5) and is a compliance metric; it never pauses the goal by itself.
- `no_progress_streak` counts consecutive goal turns without any accepted
  `ProgressSignal` (section 9.5). Only this counter drives the no-progress
  boundary.

A turn with real tool activity but no report resets `no_progress_streak` and
increments `unreported_streak`. A turn with a `Progress` report but no accepted
signal still increments `no_progress_streak`, because prose is not a signal. This
makes the after-turn protocol hard even though model tool use cannot be
guaranteed.

Evidence references contain a runtime-issued `EvidenceId` plus a short summary. The
referenced `GoalEvidenceRecord` binds command/test/artifact/external-state identity to
the current session, goal, lease, and turn when the source result is accepted. Large
output stays in existing persisted tool output or transcript storage. The worker can
cite a record but cannot create one or wrap an old result at report time to acquire
fresh provenance.

A report is recorded when the tool call succeeds and is evaluated once, at turn
finalization, by the coordinator and gate in the same finalization pass. A worker
completion candidate therefore triggers validation immediately at the turn
boundary, not on a later schedule. Validation is layered by cost: structural
checks always run (identity, lease, spec revision, plan observation, and evidence
ownership, where every reference must resolve to a durable record produced under
this goal's lease); approved deterministic checks run when present; and for a
goal with no deterministic coverage the gate spends one evidence-grounded review
before completing (section 12.3). An unverified report is never trusted bare:
even without checks or review, fabricated or borrowed evidence fails ownership
resolution and a stale plan observation fails the digest comparison.

Completion candidates have three sources:

1. worker `CompletionCandidate` report;
2. deterministic contract checks whose relevant evidence changed during the turn;
3. a mandatory boundary audit before no-progress, budget, blocked-candidate, or a
   system-generated pause would stop autonomous execution, and after an unanswered
   `LikelyComplete` probe nudge (section 12.5). Explicit human pause and interrupt
   bypass the audit.

The boundary audit derives requirement coverage and evidence from durable tool,
artifact, plan, and external-state records. It does not reinterpret the final
assistant prose as proof. If the configured policy cannot establish completion, the
original pause/block/budget transition proceeds with explicit missing-evidence detail.

### 12.3 Candidate-time evidence review and optional contract verifier

The completion policy is selected at goal creation and persisted:

| Policy | Decision rule |
|---|---|
| `candidate_with_evidence` | Default for free-form goals; the gate validates candidate identity, coverage, and evidence ownership, then spends one evidence-grounded review per candidate when the contract provides no deterministic coverage |
| `contract_checks` | Relevant deterministic checks run automatically; all user-approved checks must pass |
| `contract_checks_and_verifier` | Deterministic checks run first, then one tool-capable semantic verifier examines current state |
| `user_acceptance` | The gate validates the candidate first; the goal then transitions to `waiting(user_acceptance)` and completes only on an explicit user accept |

The runtime never silently derives a stricter or weaker contract from an agent-written
plan. TUI displays the selected policy and allows a user to approve or edit a drafted
contract before it becomes authoritative.

Contract authoring is optional. The default `/goal <objective>` flow requires no
contract review: worker self-report plus gate validation is the primary
completion path, and the completion probe (section 12.5) and boundary audits
bound its failure modes. A contract is compiled only when the user supplies
explicit conditions or asks for strict verification.

Compiled checks carry an asymmetric authority by default:

- As **necessary conditions**, checks always apply: while any approved check
  fails, the gate vetoes every completion candidate, cheaply and precisely.
- As **sufficient conditions**, checks complete the goal on their own only under
  the explicit `contract_checks` policy. This opt-in exists because a compiled
  check set can silently under-specify the user's intent; a user who cannot
  precisely validate the compiled checks should leave sufficiency with the
  worker report, the verifier, or their own acceptance rather than with the
  check set.

A contract authored in natural language is compiled once, at approval time; it is
not interpreted from raw text at judgment time. The draft splits the user text
into typed items:

```rust
pub enum ContractItem {
    // command, file-content, artifact, or external-state predicate
    // with an expected result; executing it never involves a model
    Check(DeterministicCheck),
    // bounded natural-language requirement; satisfiable only by the
    // tool-capable verifier or an explicit user acceptance
    Criterion(SemanticCriterion),
}
```

- A model may draft the compilation from the objective, conditions, or plan, and
  the TUI labels every item as deterministic or semantic. The user approves the
  compiled form, and the compiled form is what the runtime executes afterward.
- Goal creation rejects a policy that cannot judge the approved contract: a
  contract containing `Criterion` items requires `contract_checks_and_verifier`
  or `user_acceptance`. A checks-only policy with semantic criteria would
  silently reduce them to unenforced text.
- The gate evaluates items independently. Every `Check` must pass
  deterministically; `Criterion` items route to the policy's judge. Rejection
  detail carries a typed per-item result, so the next reminder names exactly
  which item failed and why.

Diffuse criteria such as "the code matches the design document" must not be
approved as one monolithic criterion. Compilation decomposes them:

1. Enumerate the referenced document's normative claims (types, invariants,
   state transitions, protocol shapes) into bounded items, each anchored to a
   document section. A model drafts the claim list; the user approves it.
2. Compile every claim with an executable form into a `Check`: assertion
   scripts, grep/AST guards, dependency or schema comparisons. Existing
   precedent in this workspace: `scripts/check-tui-ui-seam.sh` and
   `just check-error-policy` are document invariants compiled into
   deterministic guards.
3. The residue stays as scoped `Criterion` items. The verifier evaluates them
   claim by claim with read-only tools, recording a per-claim verdict and
   file-level evidence references; it never returns one global impression over
   unbounded inputs.
4. A referenced document binds by digest, like the plan artifact. A document
   edit during the goal invalidates prior claim verdicts and requires
   re-approval or re-audit, never silent reuse.

The per-claim audit report becomes the `CompletionEvidenceSummary` on success
and the typed rejection detail on failure. Two consequences follow. A contract
compiled entirely to `Check` items completes with zero model involvement in
discovery, judgment, and authorization: the coordinator's deterministic
contract pass creates the candidate even when the worker never reports, so the
worker report is an accelerator, not a dependency. A contract that retains
`Criterion` items necessarily spends one judge (verifier or user) per gate
evaluation; decomposing aggressively toward `Check` items is therefore also a
latency and cost optimization, because checks give the coordinator cheap
per-turn changed-evidence signals that criteria cannot.

Acceptance criteria enter the goal at creation, or during execution through
`/goal edit` under an expected `SpecRevision`. They are never solicited at
completion time: a completion candidate must not pause the goal to ask the user
to define success post hoc, because criteria written after seeing the result
invite moving goalposts and break deterministic checks, boundary audits, and
unattended completion. Under `user_acceptance` the pre-stated objective and
contract remain the criteria; only the accept/reject decision is post hoc. A
gate-validated candidate parks as `waiting(user_acceptance)` with the pending
decision registered as its wake; accept persists `completed` through the gate,
and reject returns the goal to `active` with bounded rejection reasons and a
queued lease so the next turn addresses the gaps.

The verifier machinery runs in two scopes:

- **Evidence review (default backstop).** For a candidate on a goal whose
  contract provides no deterministic completion coverage, the gate spends one
  bounded, read-only-tool review that answers, per claimed requirement, whether
  the cited evidence checked against current state actually supports the claim.
  Candidates are rare, so this costs one model call per candidate, not per
  turn. Without it, an uncheckable goal would complete on structural validation
  alone, fully trusting the worker's semantic claims.
- **Contract verification (opt-in policy).** Under
  `contract_checks_and_verifier`, the verifier additionally judges the approved
  `Criterion` items claim by claim under the compilation rules above.

Both scopes return `verified`, `rejected`, or `unavailable` and feed the gate;
neither owns the transition. In both scopes, the verifier runs only after a
candidate or mandatory boundary audit:

- prefer deterministic checks first;
- use a tool-capable review agent for semantic current-state checks;
- return only `verified`, `rejected`, or `unavailable`;
- `verified`: persist `completed` and stop scheduling;
- `rejected`: persist `active` with bounded reasons and schedule another turn;
- `unavailable`: persist `paused(verification_unavailable)`;
- do not spend a verifier call after an ordinary turn that has progress and no
  completion/boundary candidate.

The verifier receives the objective, completion contract, plan reference, bounded
evidence references, and read-only current-state tools. It does not need an unbounded
transcript replay.

Backstop strength is policy-dependent and must be presented that way. Under the
default `candidate_with_evidence` policy, a boundary audit can authorize completion
only when gate-checkable coverage and owned evidence already exist; it cannot
semantically prove a free-form objective. When it cannot, the original boundary
transition proceeds and the detail view shows the unverified-completion detail and
missing evidence. Goals that must complete unattended should persist
`contract_checks` or `contract_checks_and_verifier`.

### 12.4 Completion transaction

```text
goal turn finalizes
  -> GoalCompletionCoordinator always runs
  -> load report or synthesize Unreported
  -> evaluate changed deterministic contract signals
  -> determine proposed non-completion transition
  -> add boundary-audit candidate before a system stop boundary
  -> no candidate
       progress/unreported -> persist checkpoint -> queue next lease
       waiting             -> validate wake -> persist waiting
       blocked candidate   -> validate blocker -> persist blocked or reject to active
  -> candidate exists
       GoalCompletionGate validates goal/spec/lease/plan/evidence
       deterministic checks
       evidence review or contract verifier per effective policy
         verified    -> persist completed -> emit snapshot -> stop scheduling
         verified under user_acceptance   -> waiting(user_acceptance)
                                             accept -> persist completed
                                             reject -> persist active + reasons + queued lease
         rejected worker candidate -> persist active + reasons + queued lease
         rejected stop-boundary audit -> commit original blocked/limited/system-paused transition
         rejected probe audit      -> persist active + queued lease + probe cooldown
         unavailable at worker candidate -> persist paused(verification_unavailable)
         unavailable at stop-boundary audit -> commit original transition + audit-skipped note
         unavailable at probe audit -> persist active + queued lease + probe cooldown
```

### 12.5 Completion probe: bounded discovery aid

Goals without deterministic check coverage keep worker self-report as the primary
discovery path. Its known failure mode is indefinite spinning: a worker that
keeps producing real progress signals while never claiming completion. The
completion probe bounds this without reintroducing a per-turn judge.

Trigger and cadence:

- runs at most once every N autonomous turns (default five, configurable);
- only for goals whose contract contains no completion-relevant `Check` items,
  because checks already give the coordinator cheap per-turn discovery signals;
- suppressed while waiting or paused, and for a cooldown after any candidate,
  boundary audit, or prior probe (no back-to-back probing).

Input and output. The probe reads structured durable state: the objective,
recent progress checkpoints, evidence records, and plan active steps. It does
not judge raw transcript prose. It returns a closed verdict:

```rust
pub enum ProbeVerdict {
    LikelyComplete { rationale: BoundedText },
    OnTrack,
    Circling { rationale: BoundedText },
}
```

Effects are steering only; the probe has no terminal authority:

- `LikelyComplete`: inject the one-shot `goal_completion_probe` reminder telling
  the worker to submit a completion candidate with evidence or state what
  remains. If the following turn again ends without a candidate and without new
  requirement work, the coordinator raises a probe-escalated audit through the
  ordinary gate. A rejected, unverifiable, or unavailable probe audit has no
  original pause/block transition to commit: it persists `active` with the next
  queued lease and a probe cooldown. Hard turn/no-progress/token boundaries still
  apply independently.
- `Circling`: inject a one-shot replan nudge. Pausing still requires the
  deterministic no-progress counter; a model impression alone never pauses the
  goal.
- `OnTrack`, probe failure, timeout, or invalid output: no effect. The probe is
  not load-bearing, so fail-open here is safe by construction, unlike the Hermes
  judge whose fail-open erased the completion boundary itself.

The probe creates a bounded completion-discovery opportunity roughly every N turns at
the cost of one bounded model call per interval. It does not guarantee completion by
that interval for a free-form goal whose durable evidence cannot establish coverage;
the turn, token, and no-progress boundaries remain the hard convergence guarantees.

## 13. Persistence and protocol

### 13.1 Single source of truth

Reuse append-only session JSONL metadata. A new database is not required solely for
goals. Persist a complete versioned `GoalSnapshot`; under the session write lease, the
highest valid state version is the authority. `StateVersion` orders writes inside that
exclusive session writer; it is not a substitute for the cross-process session lease.

Writable session materialization fails before goal recovery when
`require_write_lease(session_id)` reports another owner. After acquiring the lease,
resume reloads the transcript and latest snapshot so no pre-lock read becomes the
runtime's authority.

Remove these authorities:

- `ToolAppState.active_goal` as an independent mutable source;
- `ManagedHookKind::Goal`;
- set/clear sentinel encoding through fake `met` values;
- condition-text matching to identify a goal hook;
- terminal-goal metadata flags that a later turn must clear.

`SessionRuntime` may retain a read-only live projection for low-latency tools and UI.
Only `GoalRuntimeHandle` updates it, after durable commit.

### 13.2 Events

AppServer is the control-plane boundary. The protocol uses explicit session targets
and explicit mutations rather than a generic partial object merge:

- `session/goal/create { session_id, objective, attachments, budgets }`;
- `session/goal/get { session_id }`;
- `session/goal/edit { session_id, goal_id, expected_spec_revision, objective?, contract?, budgets?, plan_binding? }`;
- `session/goal/set_status { session_id, goal_id, action }` where
  `action` is a closed user/system action enum such as pause or resume;
- `session/goal/clear { session_id, goal_id }`.

Agent `report_goal_turn` requests enter the coordinator through the same runtime
boundary but cannot mutate status directly. This prevents clients or stale model
turns from forging completion through a generic `set` request.

Durable notifications:

- `GoalUpdated { snapshot, cause, turn_id? }`;
- `GoalCleared { goal_id }`.

Optional ephemeral observability events:

- `GoalContinuationQueued`;
- `GoalContinuationStarted`;
- `GoalContinuationRejected`;
- `GoalContextMaterialized`.

TUI, headless JSON, SDK notification, and status display consume the same snapshot
projection. Notifications carry the complete bounded snapshot, not a patch, so a
reconnecting client repairs missed events with one update. They do not infer status
from transcript text. Every mutation response returns either the committed snapshot
or a typed stale-spec/invalid-transition/persistence error.

### 13.3 Compaction and resume

Goal and plan state are independent from compacted message history:

```text
require session write lease
  -> reload transcript and latest GoalSnapshot under the lease
  -> resolve and recover referenced session plan file
  -> create GoalRuntimeHandle
  -> emit current snapshot
  -> if active, publish session idle
  -> GoalSupervisor claim
  -> GoalContextMaterializer
  -> start continuation
```

Because backward compatibility is explicitly excluded, old `GoalStatusPayload` and
`MetadataEntry::Goal` records may be ignored rather than heuristically restored.

## 14. Concurrency and error policy

### 14.1 Race rules

| Race | Required rule |
|---|---|
| Two processes resume the same session | `SessionStore::require_write_lease` admits one writable runtime; the loser receives `session_in_use` before recovery or mutation |
| Committed human turn vs queued goal continuation | AppServer admission linearizes the two requests; a human turn committed before the autonomous claim wins, and the goal waits for the next idle edge |
| Pause/clear vs queued continuation | Under the goal transition lock, revoke lease and persist status; stale start is rejected |
| Objective edit vs running turn | Spec revision increments and the running lease is revoked; next turn uses the new objective |
| Plan edit vs queued turn | Re-materialize on claim or reject stale plan digest |
| Two idle notifications | `(goal_id, lease_id)` starts exactly one turn |
| Old usage event vs replacement goal | Goal id/lease id/effect id mismatch rejects accounting |
| Crash after persist before schedule | Resume/idle replays scheduling from durable active state |
| Event emission failure | Durable state remains; reconnect sends current snapshot |

Two different session ids may be materialized by different processes in the same
workspace. The goal runtime does not turn the workspace into a global mutex. Such
sessions may observe each other's file changes, so completion evidence and plan
digests are revalidated against current state; ordinary filesystem and version-control
conflict behavior remains visible to the workers and users.

Human-turn requests and mid-turn steering are distinct operations:

- `QueueHumanTurn` requests a new ordinary turn. It remains outside goal accounting
  and participates in the AppServer idle-admission arbitration above.
- `SteerCurrentTurn` uses the existing `CommandQueue` behavior. When accepted during a
  goal-owned turn it is injected at the safe internal boundary, inherits that turn's
  goal lease, and consumes its accounting. The UI/API must label this as steering,
  not promise that it will run first as a separate turn.

The linearization point is the AppServer per-session turn-admission critical section,
not the goal mutex alone. A human request that arrives after a goal turn has already
started does not retroactively win that admission; it either explicitly steers the
running turn or waits as the next human turn. This is the precise meaning of human
priority and avoids an impossible "wins every race" promise across two independent
queues.

For a running goal turn, pause/clear/Plan-mode entry first revokes the durable lease
under the transition lock, then requests turn cancellation. Already-started external
side effects cannot be rolled back, but all late usage, completion, progress, and
follow-up scheduling events from the revoked lease are rejected.

### 14.2 Error mapping

| Runtime outcome | Goal transition |
|---|---|
| Writable resume while session lease is held | No goal transition; reject runtime materialization with typed `session_in_use` and optional owner diagnostics |
| Storage backend cannot guarantee the session lease | No goal transition; fail closed with `session_lock_unsupported` |
| Explicit user interrupt | `paused(user_interrupted)` |
| New human-turn request arrives | Do not pause; queue the unbound human turn, which wins the next eligible admission before the goal resumes |
| Explicit mid-turn steering arrives | Inject at a safe internal boundary under the running goal lease; do not represent it as an unbound human turn |
| Provider usage limit | `usage_limited`; register a reset wake when the provider reports a reset time |
| Retryable provider error exhausted in-turn | `waiting(provider_backoff)` with a deadline wake; at most three consecutive backoff waits, then `blocked(execution_error)` |
| Non-retryable provider/tool-loop error | `blocked(execution_error)`; resumable per section 11.3 |
| Transient start failure below retry cap | Remain scheduled; bounded retry |
| Transient start failure at cap | `paused(scheduler_unavailable)` |
| Query max-turn | Goal remains active; supervisor starts a new turn unless goal turn cap is reached |
| Goal turn cap | Boundary audit, then `completed` or `budget_limited(kind=turns)` |
| Goal token budget | Boundary audit, then `completed` or `budget_limited` |
| Three signal-free turns | Boundary audit, then `completed` or `paused(no_progress)` |
| Context or plan materialization failure | `paused(context_unavailable)` |
| Verifier unavailable at a worker candidate | `paused(verification_unavailable)` |
| Verifier unavailable at a boundary audit | Commit the original boundary transition with an audit-skipped annotation |
| Completion probe failure or invalid output | No transition; the probe is skipped (steering aid, not load-bearing) |
| Persistence failure | Mutation fails; old state remains; no success event |

Every terminal/error variant must have an exhaustive transition. A wildcard branch
that logs and leaves the goal active is prohibited.

## 15. Direct final cutover

There is no Stop Hook hardening milestone and no supported hybrid runtime. The
implementation sequence may be incremental in source control, but a materialized
session must never have two goal authorities or two continuation owners.

The cutover rules are:

1. Freeze the existing goal implementation except for test fixtures needed to prove
   the audited defects.
2. Build `coco-goals`, `GoalRuntimeHandle`, `GoalSupervisor`, context materialization,
   protocol, and TUI integration against the final contracts.
3. Use construction-only development gates if needed, but never dual-write old and new
   goal state and never run both schedulers in one session. Such gates are not a
   shipped compatibility mode.
4. Switch TUI, SDK, headless, resume, and AppServer routing to the new runtime in one
   authority cutover.
5. Delete managed goal Hook registration, sentinel recovery, `ActiveGoal` authority,
   and terminal-goal flags in the same cutover change.
6. Accept protocol, metadata, configuration, and transcript incompatibility. Old goal
   records are ignored; users must create a new goal after upgrading.

This approach is intentionally narrower than cloning the Codex extension framework.
It reuses `SessionRuntime`, AppServer, `UsageAccounting`, `TaskManager`, session JSONL,
`coco-system-reminder`, the plan-file lifecycle, CoreEvent, and TUI projections. Only
the goal domain, supervisor, adapters, and protocol are new.

## 16. Reference-derived decision record

| Concern | Hermes lesson | Codex lesson | Chosen design |
|---|---|---|---|
| Durable state | Session-scoped persistence survives resume and compression rotation | First-class typed goal row and identity prevent projection drift | Versioned session snapshot with goal/spec/state/lease identities under one storage-owned session write lease |
| Continuation | New normal turns improve preemption and observability | Thread idle lifecycle is the correct single owner | Session-scoped supervisor starts same-session logical turns |
| Completion | Mandatory after-turn orchestration catches omitted claims, but the model judge sees weak evidence | Controlled terminal tool and active-goal continuation provide a hard gate, but discovery is worker-driven | Mandatory non-LLM coordinator plus multi-source candidates, runtime-owned evidence records, and exclusive evidence gate |
| Runaway control | Default 20 turns is an effective guard | Token/time accounting and mid-turn budget steering are strong | Default 20 autonomous turns plus optional token budget and total-token accounting |
| Error stop and resume | API failure fails open to continue, which preserves liveness but is unbounded | Turn error becomes blocked; external `set(Active)` immediately re-arms continuation and the TUI prompts to resume Paused/Blocked/UsageLimited goals, but abort leaves an idle Active goal and BudgetLimited is terminal | Transient errors wait under a bounded backoff wake; non-retryable errors block but resume with one action committing a queued lease atomically; interrupt pauses explicitly |
| Completion ergonomics | Every-turn judge needs no user setup but pays per turn and can mis-terminate | `update_goal` self-report is friction-free but can spin indefinitely when the worker never claims | Self-report stays primary with no mandatory contract review; checks veto cheaply; a bounded N-turn probe nudges without authority; hard boundaries stop |
| Plan handoff | Re-inject active todo state after compaction | Re-inject authoritative objective every continuation; reject Plan-mode auto-start | Reuse durable coco plan file and materialize it with the goal reminder every turn |
| Plan mode | No direct reference | Do not schedule or account goal work in Plan mode | Persist `waiting(mode_gate)` and wake on execution-mode entry |
| Waiting | Typed process/deadline barriers are useful, but need a wake owner | Lifecycle owner avoids surface-specific loops | Durable typed waits owned by supervisor task/timer subscriptions |
| TUI | Transient status makes progress visible | Full snapshot protocol, footer state, edit/pause/resume/clear, replacement confirmation | Codex-style control plane plus plan/wake details and simultaneous Plan+Goal indicators |
| Persistence backend | Session database is sufficient for session state | SQLite gives strong typed queries but is not the essential lesson | Reuse append-only session metadata, but enforce one cross-process writable owner through `SessionStore::require_write_lease`; do not add a database only for goals |
| Workspace sharing | Multiple adapters may act on shared external state | Threads remain isolated even when their tools target the same checkout | Permit different sessions/processes to share a workspace; isolate session state and revalidate evidence instead of imposing a workspace mutex |
| Extensibility | Surface glue demonstrates the cost of duplicated owners | Extension isolation is clean but broad | Add goal-specific ports, not a general extension framework |

## 17. Delivery phases

### Phase 0: turn defects into failing tests

Add red tests for:

1. a second process/store instance cannot acquire the write lease for the same
   session while the first writable runtime is alive;
2. an unfinished logical turn automatically schedules the next goal turn;
3. a runner that exits, panics, or closes its event channel without `TurnEnded`
   still produces one supervisor outcome;
4. provider errors always retry or enter an explicit status;
5. query limits cannot leave an orphan active goal;
6. background task completion, including completion during wait commit, wakes the
   goal exactly once at the durable transition boundary;
7. resume of an active goal starts continuation;
8. Plan mode prevents goal scheduling and accounting, then wakes on exit;
9. user interrupt does not immediately bounce into another turn;
10. clear racing a queued continuation rejects stale work;
11. a committed human turn wins idle admission, while explicit steering remains
    bound to the running goal lease;
12. compaction followed by continuation re-injects objective and plan reference;
13. external plan edit is observed before the next goal turn;
14. completion cannot be inferred solely from completed plan checkboxes;
15. evidence from another goal, lease, or turn is rejected;
16. exhausting the autonomous-turn budget requires an atomic budget raise before
    resume;
17. TUI receives the same snapshot as SDK and headless surfaces.

### Phase 1: domain and durable state

- Add `SessionLeaseStore::require_write_lease` semantics and the file-backed
  `TranscriptStore` OS-lock implementation; acquire before writable resume/load and
  thread the lease capability through every session mutation.
- Add canonical goal DTOs and ids in `coco-types`.
- Add pure `coco-goals` reducer and exhaustive transition tests.
- Add versioned goal snapshot metadata to `coco-session`.
- Add `GoalEvidenceRecord` metadata and sealed completion-authorization types.
- Extend the existing plan subsystem with `PlanArtifactService`, stable artifact
  identity, atomic digest observation, and observed revisions.
- Install `GoalRuntimeHandle` in `SessionRuntime`.
- Add `GoalPlanRef` through `PlanArtifactService`.
- Build protocol and TUI projection support for the new snapshot contract behind the
  same construction-only development gate; shipped surfaces remain on the old
  authority until Phase 4.

### Phase 2: minimal vertical goal runtime

- Implement session-scoped `GoalSupervisor`.
- Add explicit-session `SessionTurnPort` returning an owned `GoalTurnHandle` whose
  completion covers normal stop, abort, error, panic, and channel closure.
- Connect session resume/idle, level-triggered reconciliation, and turn lifecycle.
- Implement the mandatory coordinator and exclusive gate with the minimal persisted
  completion policy needed for vertical tests; no caller can write completed directly.
- Register lease-bound `get_goal` and `report_goal_turn`.
- Add AppServer-linearized human-turn priority, explicit steering semantics, durable
  goal-lease claims, and duplicate-idle suppression.
- Add default goal-turn cap and total-token accounting.
- Add deadline-wake prepare/commit/activate/recheck infrastructure.
- Add typed provider-error mapping with bounded `waiting(provider_backoff)` wakes.
- Inject a minimal authoritative objective/budget/completion-contract context so no
  autonomous turn starts unanchored.

Exit criterion: the active-goal invariant passes model/property tests, and TUI,
headless, and SDK each pass one multi-turn end-to-end test behind a construction-only
development gate. The old Stop Hook remains the only shipped authority until Phase 4;
the two runtimes never own one materialized session simultaneously.

### Phase 3: evidence, plans, waits, and advanced completion

- Implement `GoalContextMaterializer`.
- Add mandatory `goal_context` reminder and one-shot delta reminders.
- Materialize bounded plan headings, active steps, and verification commands.
- Register runtime-generated evidence producers for tool results, artifacts,
  deterministic checks, and external-state observations.
- Add `create_goal` on explicit user/system request.
- Add task-terminal and deadline wake paths.
- Add budget-limit mid-turn steering.
- Add candidate-time evidence review, optional contract verifier, compiled checks,
  user acceptance, and the bounded completion probe.
- Add resume actions for paused/blocked/usage-limited and the usage-limit reset
  wake.

### Phase 4: delete old implementation

- Atomically cut all command/resume/surface routing to `GoalRuntimeHandle`.
- Remove managed goal Stop Hook registration and matching.
- Remove old sentinel scan and restore logic.
- Remove `ToolAppState.active_goal` authority.
- Remove terminal-goal metadata flags.
- Update crate docs, configuration docs, and protocol schemas.

Do not keep a compatibility shim, dual-write bridge, or runtime fallback.

## 18. Verification matrix

### 18.1 Domain tests

- exhaustive status/command transitions;
- invalid resume, replace, wait, and complete are rejected;
- stale goal id, spec revision, lease, and effect ids are rejected;
- turn/token/time accounting is not duplicated;
- budget boundaries fire exactly once;
- `budget_limited(kind=turns)` rejects plain resume and accepts an atomic sufficient
  budget raise plus resume;
- clear/recreate generates a new identity;
- `active => running || queued`;
- `waiting => durable wake identity and condition`;
- only sealed gate authorization can construct the completed transition;
- plan revision cannot alter objective/status implicitly.

### 18.2 Runtime integration tests

1. A first turn stops without completion; a second turn starts automatically.
2. A worker completion candidate can only complete through `GoalCompletionGate`.
3. Default goal-turn cap enters `budget_limited(kind=turns)`.
4. Token budget enters `budget_limited` and injects only one wrap-up.
5. Provider usage limit enters `usage_limited`.
6. Terminal execution error enters `blocked`.
7. Background work does not burn continuation turns and wakes on completion.
8. A committed human-turn request races continuation and wins the next admission;
   explicit mid-turn steering remains bound to the running goal lease.
9. Clear after queueing prevents stale continuation start.
10. Restart/resume automatically continues an active goal.
11. Compaction cannot change the goal snapshot.
12. The next post-compaction turn receives full objective and current plan reference.
13. A changed plan digest causes re-materialization, not stale plan reuse.
14. Persistence failure emits no success event.
15. Two sessions have isolated goals, plans, accounting, and schedulers.
16. Plan/review mode does not start or account a goal turn.
17. Exiting Plan mode activates the current plan revision and wakes the goal.
18. TUI shows Plan and Goal state simultaneously.
19. TUI, headless, and SDK observe the same snapshot/status.
20. A rejected completion candidate returns evidence gaps in the next reminder.
21. An explicit branch copies the plan but does not clone an active goal.
22. Three signal-free goal turns pause with `no_progress` rather than loop.
23. Two background session goals receive fair admission.
24. Two different sessions may run in the same workspace, keep independent goal
    state, and revalidate evidence against current shared-workspace state.
25. A turn without `report_goal_turn` becomes `Unreported` and continues safely.
26. A deterministic contract can complete without a worker claim.
27. No-progress, budget, blocked-candidate, and system-pause boundaries run a
    completion audit first; explicit human pause does not wait.
28. Only `GoalCompletionGate` can emit the completed transition.
29. Duplicate turn-finalization events commit one idempotent coordinator decision;
    verifier delivery is allowed to retry only when no result was committed for its
    durable attempt id.
30. `Unreported` and assistant prose alone can never produce completed state.
31. Resume from `blocked(execution_error)` commits a queued lease in the same
    transaction and automatically re-runs a turn.
32. `usage_limited` with a persisted reset deadline wakes and continues without
    user action.
33. A turn with accepted progress signals but no `report_goal_turn` increments
    only the unreported streak and triggers the one-shot report reminder; it does
    not advance the no-progress streak.
34. A boundary audit whose required verifier is unavailable commits the original
    boundary transition with an audit-skipped annotation.
35. Retryable provider errors enter `waiting(provider_backoff)` and wake; the
    failure after the third consecutive backoff becomes `blocked(execution_error)`.
36. Resume validation failure (missing plan artifact) produces
    `paused(context_unavailable)`, not a silent no-op.
37. Under `user_acceptance`, a gate-validated candidate parks as
    `waiting(user_acceptance)`; reject returns `active` with bounded reasons and
    a queued lease, and accept persists `completed` only through the gate.
38. A completion candidate can never trigger a prompt asking the user to define
    acceptance criteria post hoc; criteria change only via `/goal edit` with an
    expected spec revision.
39. Goal creation rejects a contract containing semantic criteria when the
    selected policy has neither a verifier nor user acceptance.
40. A contract compiled entirely to deterministic checks completes with no model
    call in discovery, judgment, or authorization, even when the worker never
    reports.
41. A document-alignment criterion yields per-claim verdicts with evidence
    references, and a changed document digest invalidates prior verdicts.
42. The completion probe fires at the configured interval only for goals without
    completion-relevant check coverage, and never back-to-back.
43. A `LikelyComplete` probe verdict injects the one-shot reminder; a subsequent
    candidate-less turn raises a probe-escalated audit through the gate.
44. Probe failure, timeout, or invalid output changes no goal state, and a probe
    verdict can never directly complete or pause a goal.
45. A failing approved check vetoes a worker completion candidate under every
    policy; passing checks alone complete the goal only under `contract_checks`.
46. `report_goal_turn` and `get_goal` are visible only on turns holding a valid
    goal lease and are injected eagerly, not via deferred tool discovery.
47. A candidate on a goal without deterministic coverage spends exactly one
    evidence review; review rejection returns `active` with per-requirement
    gaps, and no review runs on ordinary progress turns.
48. A second writable materialization of the same session id fails with
    `session_in_use`; releasing the lease or terminating the owner process allows a
    fresh process to acquire the OS lock and reload the latest state.
49. Session reads and listing remain available while another process holds the write
    lease, but no mutating store API is callable without the matching lease capability.
50. A runner panic or event-channel close without `TurnEnded` resolves its owned
    `GoalTurnHandle` to an error outcome and cannot leave a durable running lease.
51. A task or deadline satisfied between watcher preparation and waiting-state commit
    is observed by the post-commit predicate recheck and queues one continuation.
52. A missing or failed volatile wake watcher is recreated by level-triggered
    supervisor reconciliation from the durable wake identity.
53. Evidence created under another goal id, lease id, turn id, or session cannot pass
    ownership validation; the model cannot mint a valid `GoalEvidenceRecord`.
54. A rejected or unavailable probe-escalated audit returns to `active` with one
    queued lease and cooldown; it never tries to commit a nonexistent original pause
    transition.
55. `PlanArtifactService` observes content and digest atomically, advances revision
    only on accepted digest change, and resolves only session-owned artifact ids.
56. A file/remote storage backend that cannot guarantee exclusive session locking
    rejects writable materialization with `session_lock_unsupported` rather than
    silently continuing unlocked.

### 18.3 Fault injection

- session write-lease contention from a second process;
- owner process exit while holding the session lock, followed by recovery acquisition;
- event channel full or closed;
- turn runner panic or return without a terminal event;
- transcript append failure;
- AppServer turn start rejected or busy;
- plan file missing, unreadable, oversized, or changed during materialization;
- shared-workspace file change between evidence creation and completion validation;
- task watcher disconnect;
- task/deadline satisfaction between watcher preparation and waiting-state commit;
- deadline timer recovery after restart;
- verifier timeout or invalid output;
- crash after verifier response but before result persistence;
- duplicate idle notification;
- late usage event from a previous goal;
- cancellation between persist and schedule.

### 18.4 Observability

Metrics must exclude objective and plan content. Record:

- session write-lease acquisition, contention, owner process kind, and hold duration
  without persisting user content;
- goal created/resumed/completed/blocked/paused/cleared;
- continuation queued/started/rejected/retried;
- active and waiting duration;
- goal turns/tokens/time;
- plan materialization success/failure and digest change;
- budget, usage, verifier, context, and scheduler stop reasons;
- stale continuation rejection;
- synthesized turn outcomes for panic/channel-close paths and wake reconciliation;
- persistence failure and invariant violation.

Invariant violations require an error log and counter. Silent self-healing is not
sufficient observability.

## 19. Acceptance criteria

The refactor is complete only when all conditions hold:

- At most one writable `SessionRuntime` owns a session id across processes; a second
  writable resume fails before recovery or mutation, while read-only inspection
  remains available.
- Every active goal identifies a running turn or queued lease; every waiting goal has
  a durable wake identity whose volatile watcher is registered or recoverable by
  level-triggered supervisor reconciliation.
- Every `QueryEngine` terminal/error variant, runner panic, and missing-terminal-event
  path has a tested supervisor transition.
- Work advances across multiple logical turns without user input.
- Ordinary continuation never forks the worker session.
- Plan/Review mode never runs or accounts autonomous goal work.
- Leaving Plan mode re-materializes the current plan and resumes eligible work.
- Context compaction summarizes conversation only; goal and plan references survive
  independently.
- Every autonomous turn receives the authoritative objective, budget, current plan
  reference, and completion policy.
- User interrupt produces an explicit paused state.
- Every non-terminal stopped status resumes through one validated path that
  commits a queued lease in the same transaction.
- Exhausted turn/token budgets cannot resume without a sufficient atomic budget edit;
  lifetime usage is never silently reset.
- Transient provider failures wait with a registered wake or become an explicit
  resumable status; they never silently end autonomous work.
- Background tasks and deadlines wake the supervisor.
- Process restart automatically resumes active work.
- Goal turn and token budgets are explainable and exactly once.
- No hook or optional-reminder policy can disable the core goal runtime.
- There is one durable current-goal authority.
- The plan file is durable working memory, not a competing status authority.
- Completion is a gated candidate backed by current-state evidence; deterministic
  contracts may create the candidate without a worker claim.
- Completion evidence is runtime-owned and durably bound to session, goal, lease,
  turn, and source identity before the worker cites it.
- Every goal-owned turn reaches `GoalCompletionCoordinator`, including turns where
  the worker omits its report.
- No tool, surface, verifier, or worker can persist completed state except through
  `GoalCompletionGate`.
- No surface-specific autonomous loop remains.
- Different sessions may share one workspace without sharing session state or write
  leases; completion revalidates current shared-workspace evidence.
- Old goal Hook and sentinel recovery code is deleted.
- No transitional Stop Hook goal path or dual authority remains.
- Relevant tests, seam guards, error-policy checks, formatting, and Clippy pass.

## 20. Final architecture decisions

1. A goal is a session aggregate, not a Hook.
2. A continuation is a new logical turn in the same session, not a query-local retry
   and not a full worker fork.
3. `GoalSupervisor` is the only automatic continuation owner.
4. `SessionStore::require_write_lease` permits one writable materialization of a
   session id across processes; the session lease belongs to storage, not to goals.
5. A durable snapshot is the single goal source of truth; runtime, UI, events, and
   transcript messages are projections.
6. Conversation compaction may summarize prior turns, but it cannot summarize away
   goal control state or the plan reference.
7. The existing plan file is reused as mutable execution memory through
   `PlanArtifactService`. It is referenced and re-materialized, not copied into the
   goal row.
8. One mandatory typed goal reminder re-injects objective, budget, plan state, wait
   results, and completion policy on every autonomous turn.
9. Every goal turn passes through `GoalCompletionCoordinator`; worker reports,
   deterministic contracts, and boundary audits produce candidates, and only
   `GoalCompletionGate` may persist completion.
10. Runtime-owned evidence records bind proof to session, goal, lease, turn, and
    source before the worker cites it.
11. The default autonomous-turn budget is 20; token budget is explicit-only, and
    exhausted budgets require an atomic raise before resume.
12. A committed human turn wins the next idle admission; explicit mid-turn steering
    remains bound to the current goal lease. Stale autonomous work is rejected by
    goal id and lease id.
13. Plan/Review mode gates autonomous work and goal accounting without hiding or
    clearing the goal.
14. TUI is a snapshot-driven control plane with explicit plan, wait, budget, and
    evidence visibility.
15. Every turn end commits a queued lease, durable recoverable wait, or non-active status.
    Active-but-idle is invalid.
16. Different sessions and processes may share a workspace; workspace sharing never
    permits two writable owners of the same session id.
17. Cut directly to the final runtime. Do not add any new goal behavior to the managed
    Stop Hook and do not ship dual authorities.
