# Agent-Teams Adversarial Review — 2026-06

Adversarial review of the agent-teams (`coco-coordinator` + the `app/cli`
teammate plumbing + the `SendMessage`/`Agent` tool seams) against the TS
reference at `/lyz/codespace/3rd/claude-code/src`. Goal: behavior mirrors TS,
best Rust practice, clear architecture. Backward compatibility disregarded.

Five parallel review passes covered: (1) shutdown/teardown lifecycle, (2)
identity/context threading, (3) coordinator-mode persistence (gap 11), (4)
resume/restore (gap 9), (5) architecture seams + tmux parity. Every finding
below was checked against the cited TS source.

---

## Fixed in this pass (verified)

### F1 — CRITICAL: in-process teammate could not resolve its own identity → shutdown approval failed closed

`respond_to_shutdown` resolves the worker's identity via the 3-tier resolver
(`get_agent_name()/get_team_name()/get_agent_id()`,
`coordinator/src/agent_handle/mod.rs:1020-1026`). For an **in-process**
teammate none of the three tiers were populated during a turn: env vars are
cross-process only, the dynamic context has zero production callers, and the
tier-1 task-local (`run_with_teammate_context`) was **never called in
production** — only in `identity.test.rs`. So an in-process teammate that the
model told to approve its own shutdown hit
`"shutdown_response requires a teammate identity"` and the approval was
dropped.

**TS:** `inProcessRunner.ts` runs the whole teammate turn inside
`runWithTeammateContext(...)`, so `getAgentName()` resolves throughout.

**Fix:** wrap the in-process teammate's `engine.run_query(...)` in
`run_with_teammate_context` (`coordinator/src/runner_loop.rs`). Verified safe:
`SendMessage` is `is_concurrency_safe == false` (default), so it runs **inline
serially** in the streaming executor's `commit_flush` on the same task — the
tokio task-local reaches it. (Safe/read-only tools are `JoinSet`-spawned and
never need identity.)

### F2 — CRITICAL/HIGH: a *rejecting* in-process teammate was terminated anyway

The runner set `handling_shutdown = true` on receipt of a `ShutdownRequest`
and then `break`-ed unconditionally after the next turn — exiting the loop
whether the model approved **or rejected**.

**TS:** `inProcessRunner.ts` "Does NOT auto-approve shutdown — the model should
make that decision." Exit happens only via `SendMessageTool.ts`
`handleShutdownApproval` → `task.abortController.abort()`; a rejection returns
"Continuing to work." and the teammate keeps running.

**Fix:** removed the `handling_shutdown` flag + unconditional break entirely.
A `ShutdownRequest` is now delivered as an ordinary turn; the model's
`shutdown_response` decides:
- **approve** → `respond_to_shutdown` calls a new
  `identity::signal_self_stop()`, which flips the teammate's task-local
  `self_stop_signal` (the runner's own `config.cancelled` Arc); the loop exits
  on its next `config.cancelled` check — the in-process analog of TS aborting
  the `abortController`.
- **reject** → flag stays clear, teammate continues (TS parity).

This reuses the existing cancellation seam and does **not** depend on the
child-engine→runner wiring (the signal travels through the task-local Arc the
runner itself installs). New regression test
`in_process_teammate_rejecting_shutdown_keeps_working` proves reject → continue,
and the updated approve test proves approve → clean exit.

### F3 — HIGH: silent permission-request drop in the leader poller

`leader_inbox_poller.rs` dispatched the worker's `PermissionRequest` to the
approval UI via `setter(serde_json::to_value(&parsed).unwrap_or_default())` and
then marked the message read. On a serialization failure that produced
`Value::Null`, the worker's request was silently consumed (marked read) and
never surfaced — the worker then blocked until its bounded wait timed out.

**Fix:** serialize first; on failure `continue` and leave the message **unread**
so the next tick retries. Mark-read only after a successful dispatch.

### F4 — MEDIUM: rejecting a shutdown without a reason was silently accepted

`dispatch_shutdown_response` accepted `approve == false` with no `reason`,
writing a `ShutdownRejected` with the placeholder
`"shutdown rejected by teammate"`.

**TS:** `SendMessageTool.ts:705-714` returns a validation error
`"reason is required when rejecting a shutdown request"`.

**Fix:** mirror the TS validation in `dispatch_shutdown_response`
(`core/tools/src/tools/agent/send_message_tool.rs`).

---

## Cross-validation + fixes (R/A findings)

Each R/A finding was independently re-verified against the live code + TS
source by a separate review pass. The verdicts corrected the first-pass report
in several places (R3, R5, R7 below). **Verdict** = is the defect real;
**Status** = `FIXED` this pass / `DEFER` with reason.

### Verified GENUINE → FIXED

| # | Verdict | What was actually wrong | Fix (file) |
|---|---------|-------------------------|------------|
| R1 | GENUINE | `leader_inbox_poller` installed ONLY in `tui_runner`; team creation/spawn is gated on `Feature::AgentTeams`, not entrypoint, so a headless/SDK leader leaks `team.json` membership + orphaned tasks (even for in-process teammates). TS handles this on the print path too (`cli/print.ts:2497-2613`). | Shared `leader_inbox_poller::install_leader(runtime, bridge)`; wired into SDK (`main.rs`), headless (`headless.rs`), and TUI (dedup). |
| R2 | GENUINE | Coordinator `Mode` metadata written only at the TUI exit checkpoint; headless/SDK never persisted it, so `--resume` silently dropped the role. | `coordinator_mode_resume::persist_session_mode`; called at SDK + headless end-of-run. (R2b crash-safety = first-turn write, deferred.) |
| R4 | GENUINE | Teammate applying a mode never wrote it back to `team.json`; a cross-process teammate's Shift+Tab self-cycle left the leader's roster stale. TS: `useInboxPoller.ts:594` + `syncTeammateMode`. | `team_file::set_member_mode_in_team_file`; called from the `SetPermissionMode` handler when `is_teammate()`. |
| R5 | PARTIAL→FIXED | Code claim true (`teardown_teammate` ignored the message `backend_type`); but a session hosts ONE pane backend today (`agent_handle_factory` detects once), so the wrong-backend kill is currently *unreachable*. | Defensive guard: skip `kill_pane` + warn on `backend_type` mismatch (future mixed-backend safety). |
| R6 | GENUINE | `unassign_teammate_tasks` return discarded; peers/leader never learned the teammate is gone or which tasks reopened. TS pushes `teammate_terminated` with the task summary. | Capture the reopened list, write a coordinator-framed message to the leader's inbox (the poller re-injects it as a turn). |
| R7 | PARTIAL→FIXED | FALSE for in-process (the runner wrapper already marks `Completed`). REAL for **tmux/iTerm2** teammates whose running-task lingers `Running` forever (exactly TS `useInboxPoller.ts:749-765`). The report misattributed the backend. | `teardown_teammate` calls `complete_teammate_task(Completed)` for the pane case (idempotent on terminal rows). |
| R8 | GENUINE | Roster picker hardcoded `mode: Default`; the existing test even *codified* the bug. Could not show divergent per-member modes. | Per-member `mode` seeded fresh from `team.json` (`read_team_file`), cycled in place; renderer shows each row's mode; the bug-codifying test was corrected + a per-member-independence test added. (Cycle-all = feature add, deferred.) |
| R9 | GENUINE | In-process teammate permission prompts hardcoded `worker_badge: None`; the leader couldn't tell which teammate was asking. The misleading "TS only badges cross-process" comment was wrong. | `leader_permission::enrich_in_process_worker_badge` (reads the task-local identity); called by both the TUI and SDK bridges. |
| A5 | GENUINE | `enable_pane_border_status` used `set-option -g` (server-global, never reverted), mutating the user's unrelated tmux windows. | Scope to `-w -t <target>` with a `current_window_target()` fallback. TS: `TmuxBackend.ts:233-252`. |
| A6a | GENUINE | External first-teammate path created a session (with an initial pane) then *unconditionally* split — orphaning the initial pane as a stray empty shell. | Reuse the session's initial pane for the first teammate; split only for subsequent. |
| A6b | GENUINE (race) | `mark_messages_as_read{,_by_index,_by_predicate}` did an UNLOCKED read-modify-write; a peer `write_to_mailbox` interleaving between read and write-back was silently lost (the exact TOCTOU `write_to_mailbox` was hardened against). | Wrap all three in `with_inbox_lock` + in-lock re-read. |

### Verified FALSE-POSITIVE / cosmetic → DEFER (with reason)

| # | Verdict | Why not fixed |
|---|---------|---------------|
| R3 | FALSE-POSITIVE (as a regression) | `reconnect.rs` was **dead code** (zero production callers; tier-2 dynamic context has no writer). coco-rs re-establishes teammate identity on resume via inherited `COCO_*` env (tier-3), which survives `--resume` for spawned teammates. Deleting it is correct. The only residual gap — a manual `coco --resume <id>` in a bare shell — is pre-existing, narrow, and out of the supported topology. A point fix (one `TranscriptMetadata` field + one `set_dynamic_team_context` call) is noted but not a "design pass". |
| A1 | cosmetic | `AgentHandle` carries 4 teammate-lifecycle methods, but all have trait-level default `Err` impls — only `SwarmAgentHandle` overrides them. A `TeammateLifecycle` sub-trait is pure cosmetics; the default-impl pattern already gives the 5 test doubles a zero-cost opt-out. |
| A2 | FALSE-POSITIVE | Not duplication: `TeammateControlState` is defined once; the cross-process pump deliberately reuses the live `SetPermissionMode` + shared rules `Arc` instead of a parallel struct. Two consumers of one protocol. |
| A3 | real but DEFER | `tui_runner.rs` ≈ 5567 LoC IS over the cap, but a safe mechanical split touches the most-contended CLI file and belongs in its own no-logic-change PR, not bundled with behavior fixes. |
| A4 | real but DEFER | Env-mutation test isolation is genuine, but a DI seam for `COCO_TEAMS_DIR` cascades through the entire mailbox/team-file/identity API surface — not worth it standalone. |
| A6c | cosmetic | `coco_types::TeamContext` + the `Option<&TeamContext>` params on `get_team_name`/`is_team_lead` are vestigial (always `None` in prod). Trivial cleanup; removal adds cross-file churn for zero behavior change. |
| A7a | real but DEFER (product call) | The roster toggle is dead code: the keybinding resolver returns `AppToggleTodos` for Ctrl+T before the legacy `OpenTeamRoster` fallback, so the roster picker has no reachable trigger and isn't rebindable. Fixing it is a product/keybinding decision (which key; keep vs. fold into the expanded-tasks Teammates view) — flagged for an explicit decision rather than guessing a default binding. |
| A7b | cosmetic | In-process `bypass_permissions_available` is hardcoded `false`; cross-process computes it — but the cross-process spawn passes no bypass flag, so both converge to `false`. A code smell, not a behavior gap; documents the "teammates never bypass" stance. |

### Second-pass disposition of the deferred items

Re-analyzed each deferred item on its merits ("does it need a *fix*, a *test*,
or neither"):

| # | Decision | Action |
|---|----------|--------|
| **A7a** | **FIX** — a genuine feature was unreachable | Added a rebindable `app:toggleTeamRoster` action (default `ctrl+shift+t`, gated to teammates-present), wired its dispatch, and deleted the dead hardcoded fallback. Making it **rebindable** dissolves the "which key" product question (the default is a fresh coco-rs choice — TS has no roster shortcut). 3 tests: dispatch (inert / opens), parse round-trip, and an end-to-end bridge test proving `ctrl+shift+t` → `OpenTeamRoster`. |
| **A6c** | **FIX** — verified dead code | Dropped the always-`None` `Option<&TeamContext>` param from `get_team_name`, deleted the zero-caller `is_team_lead`, and removed the orphaned `coco_types::{TeamContext, TeammateEntry}` structs + their re-exports. |
| **R3** | **No fix** (false-positive) — but **add a test** | Added `test_resolve_teammate_identity_from_env_vars` locking the tier-3 (env) resume path that makes the `reconnect.rs` deletion safe. That path was previously untested. |
| **A7b** | **No behavior change** (current `false` is safe) | Made the invariant explicit with comments at both spawn sites — a future grant of teammate bypass must be a deliberate edit. A unit test would only assert a hardcoded literal (low value, heavy setup). |
| **A1** | **No fix** | The default-`Err`-impl pattern is idiomatic Rust and already lets the 5 test doubles opt out at zero cost; a `TeammateLifecycle` split is pure churn. |
| **A2** | **No fix** | Confirmed not duplication — two consumers of one protocol. |
| **A4** | **No fix** | The tests are isolated correctly under nextest (process-per-test); a DI seam would cascade through the entire mailbox API for a bare-`cargo test` benefit the project doesn't rely on. |
| **R8 cycle-all** | **FIX** (done) | Implemented the TS-parity feature: **Shift+Left/Right** in the roster cycles ALL teammates in tandem, mirroring `cycleAllTeammateModes` (diverge → normalise all to `Default`; uniform → advance all). Also aligned the interaction model to TS — cycling (single + all) now applies **immediately** (Enter just closes), persisting via a new atomic batch setter `roster_store::set_member_modes` + `AgentHandle::set_teammate_modes` (one `team.json` write, per-teammate mailbox). 4 tests (normalize / advance / empty / key-wiring) + the single-cycle test updated to the new return contract. |
| **R2b** (crash-safety) | **Optional, not done** | R2a already closed the cross-entrypoint gap (headless/SDK now persist at exit). R2b only adds TUI-crash-before-exit robustness — a rare edge; a first-turn write hook in the 5.5k-LoC `tui_runner` is disproportionate. |

---

## Why the existing tests did not catch these

A consistent pattern explains every miss — **the tests stop at the in-memory
boundary or test a helper in isolation, while the bug lives in the
wiring/IO/derivation step that no test exercises:**

- **R1 / R2 — untested bootstrap glue.** Which runner path installs the poller,
  and whether headless/SDK persist the mode, lives in `tui_runner` / `main.rs` /
  `headless.rs` bootstrap code with no unit coverage. `leader_inbox_poller`
  tests cover only idle-notification *formatting*; `save_mode` tests prove the
  *function* works, never *when/whether it is called*. No test runs a
  headless/SDK coordinator session end-to-end.
- **R4 / R8(seed) — tests stop at the in-memory effect.** The mode-set tests
  assert the live permission store flips; none re-read `team.json` after apply.
  The roster test literally `assert_eq!(r.mode, Default)` — it *encoded* the
  hardcoded-default bug as expected behavior.
- **R5 / R6 / R7 — `teardown_teammate` is entirely untested.** No test calls it
  or asserts on post-teardown mailbox/queue/task-status. `unassign_teammate_tasks`
  has a unit test for the re-pend side effect but nothing checks that callers
  surface its return.
- **R9 — derivation untested.** Badge tests cover the *renderer* given a
  pre-set badge; nothing exercises the *derivation* (`permission_controller`
  request construction) or runs a request inside a `run_with_teammate_context`
  scope. The bug sat in the untested derivation step, so the passing renderer
  tests gave false confidence.
- **A6b — no concurrency test.** The write path's lock is covered; the mark-read
  paths have no concurrent-writer test, so the missing lock was invisible.
- **A5 / A6a — tmux arg vectors unasserted.** `tmux.test.rs` can't exec tmux and
  never asserts the emitted `set-option` / `new-session` arg vectors (which ARE
  constructible/assertable), so `-g` vs `-w` and split-vs-reuse went unchecked.

### Tests added/updated this pass

- `in_process_teammate_rejecting_shutdown_keeps_working` (F2 regression).
- `test_send_message_shutdown_response_reject_without_reason_rejected` (F4).
- `team_roster_cycle_mode_wraps_interactive_modes` rewritten to seed from the
  member's live mode and assert **per-member independence** (R8).
- `show.rs` roster test corrected off the bug-codifying `Default` assertion.

### Test debt still open (recommended next)

- A `teardown_teammate` integration test (covers R5/R6/R7): seed a team +
  tasks, call teardown, assert kill/rollback/unassign + the terminate
  notification + pane-task `Completed`.
- A headless/SDK coordinator-session E2E asserting the `Mode` entry lands
  (R2) and `team.json` membership is cleaned on shutdown (R1).
- A concurrent write-vs-mark-read test for the mailbox lock (A6b).
- A `team.json`-seeded roster test asserting the picker reflects a stored
  non-default mode (R8) — needs a coordinator-side fixture since `TeamFile`
  construction crosses the `app/tui` module-privacy boundary.

## Notes

- F1+F2 close the in-process shutdown story (approve resolves identity + exits;
  reject keeps working). R5–R9 + A5/A6 close the teardown-fidelity and
  pane/mailbox-hygiene gaps. R1/R2/R4 make coordinator mode + teardown durable
  outside the TUI.
- Deferred items are either false-positives (R3, A2), pure cosmetics
  (A1, A6c, A7b), large mechanical refactors that warrant their own PR
  (A3, A4), or a product decision (A7a, R8 cycle-all, R2b crash-safety).
