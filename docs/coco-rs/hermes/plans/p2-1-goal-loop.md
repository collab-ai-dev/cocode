# P2-1 — `/goal` Standing Judged Loop + Verify-on-Stop (Design-Level Plan)

Status: not started — **requires its own adversarial design review before
any code** · Size: XL, phased · Owner crates: `coco-query` (loop seam),
`coco-commands` (`/goal`, `/subgoal`), `coco-session` (state), new
`root/goals` module or extension of `tasks` (decide in design review)

This is hermes's largest sustained quality investment (v0.13 → v0.18).
This plan fixes the mechanism inventory and the coco landing points; it
is deliberately NOT an implementation-ready spec.

## Mechanism inventory (hermes-agent @ `a7f65e3bc`)

### A. The Ralph loop (`hermes_cli/goals.py`; release v2026.5.7 #18262/#18275/#21287)

- **Constants** (:47-68): `DEFAULT_MAX_TURNS = 20`,
  `DEFAULT_JUDGE_TIMEOUT = 30.0`, `DEFAULT_JUDGE_MAX_TOKENS = 4096`
  (raised from 200 — reasoning judges truncated JSON; overridable via
  `auxiliary.goal_judge.max_tokens`, :669),
  `_JUDGE_RESPONSE_SNIPPET_CHARS = 4000`,
  `DEFAULT_MAX_CONSECUTIVE_PARSE_FAILURES = 3`.
- **Judge** — `judge_goal` (:836): auxiliary role client
  (`get_text_auxiliary_client("goal_judge")`, :887), sees only a
  4000-char snippet of the last response (:914-932), `temperature=0`
  (:938-948). **Fail-open**: "any error returns
  `('continue', …, False, None)`" (:870, :949-951); non-JSON parses
  fail-open too, but 3 consecutive parse failures auto-pause
  (:693, :1500-1527).
- **Continuation is a plain user message** (prompt-cache-intact —
  no system-prompt mutation, no toolset swap):
  `CONTINUATION_PROMPT_TEMPLATE` (:71-77, "[Continuing toward your
  standing goal]\nGoal: {goal}\n\nContinue working toward this goal.
  Take the next concrete step. …"), injected through the ordinary
  input queue (`cli.py:9288-9292` `self._pending_input.put(prompt)`;
  gateway enqueues via the adapter FIFO "so a user message already in
  flight preempts the continuation naturally",
  `gateway/run.py:12746-12760`).
- **Turn budget**: `GoalState.max_turns = 20` (:395); exhaustion pauses
  with "⏸ Goal paused — {n}/{max} turns used" (:1529-1543).
- **Preemption**: `_maybe_continue_goal_after_turn` (`cli.py:9170`) —
  "if a real user message is already in `_pending_input` we skip
  judging" (:9203-9227); Ctrl+C auto-pauses
  (`mgr.pause(reason="user-interrupted (Ctrl+C)")`, :9235).
- **Persistence**: `state_meta` key `f"goal:{session_id}"`
  (`_meta_key` :489-490); `load_goal`/`save_goal`/`clear_goal`
  (:526/547/560) — `/resume` picks the loop back up.
- **`/subgoal`** (release v2026.5.16 #25449): layers extra success
  criteria into the judge prompt and a
  `CONTINUATION_PROMPT_WITH_SUBGOALS_TEMPLATE` (:99) without
  restarting the loop (`GoalManager.add_subgoal…`, :1218-1252).
- **Wait barriers** (release v2026.7.1 #50503): `/goal wait <pid>`
  parks the loop on OS-process exit
  (`cli_commands_mixin.py:2064-2093`); barriers `wait_on(pid)` /
  `wait_on_session` / `wait_for_seconds` (:1265-1330), lazily
  auto-cleared; the judge itself may return a `wait` verdict.

### B. Verify-on-stop (release v2026.7.1 #50501/#52285/#53552/#55449)

- **Stop-seam nudge** — `agent/verification_stop.py`
  `build_verify_on_stop_nudge` (:245-310): returns `None` when no
  verifiable paths, `attempts >= 2`, or ledger status == `"passed"`;
  otherwise a synthetic follow-up "[System: You edited code in this
  turn, but the workspace does not have fresh passing verification
  evidence yet. …]" listing ≤ 8 changed paths (:16) and the detected
  `verifyCommands` (first 3). Loop wiring
  `agent/conversation_loop.py:5131-5176`: the attempted final answer
  stays in history flagged synthetic
  (`finish_reason = "verification_required"`), a synthetic user nudge
  follows, and the loop continues silently.
- **Doc-only skip** (:24-72): extension frozenset
  (`.md .rst .txt .csv …`) + filename set (`license changelog …`);
  "a SKILL.md or README edit must never demand a verification script".
- **Gate** — `verify_on_stop_enabled` (:135-170): env >
  config `agent.verify_on_stop` > surface-aware `"auto"` (ON for
  cli/tui/desktop/api, OFF for messaging surfaces).
- **Canonical-check detection** — `agent/coding_context.py`
  `detect_project_facts` (:747-783): `scripts/run_tests.sh`;
  package.json scripts matched against
  `_VERIFY_TARGETS = ("test","tests","lint","typecheck","check",
  "build","fmt","format")` (:149) with lockfile-detected package
  manager; pytest via `pytest.ini`/`[tool.pytest`; Makefile targets by
  regex; capped `_MAX_VERIFY_COMMANDS = 8` (:150).
- **Evidence ledger** — `agent/verification_evidence.py`: SQLite (WAL)
  with `verification_events` (command, status, exit_code, …, :84-97)
  and `verification_state` (PK `(session_id, root)`, `last_edit_at`,
  `changed_paths_json`, :102-109). Producers:
  `classify_verification_command` (:383 — terminal commands matched
  against detected verifyCommands, `passed` iff exit 0, :421) fed from
  the terminal tool (`tools/terminal_tool.py:2746-2754`);
  `mark_workspace_edited` (:495) fed from file tools
  (`tools/file_tools.py:1615/1633`, changed-paths cap 200). Consumer
  `verification_status` (:549): `not_applicable` / `unverified` /
  **`stale`** (edit newer than last event, :608-609) / passed / failed.
- **`pre_verify` hook** — `agent/verify_hooks.py`:
  `DEFAULT_MAX_VERIFY_NUDGES = 3` (:21), optional guidance text
  (:26-32); fired at the stop seam
  (`conversation_loop.py:5183-5224`) when files were edited and the
  hook exists.

## coco-rs landing sketch

### What coco already has (build on, don't duplicate)

- Turn-boundary queued-input drain with origin tagging
  (`CommandQueue`, `QueueOrigin`, drained in
  `engine_finalize_turn.rs:509`) — the continuation-injection seam,
  matching hermes's queue semantics exactly (their CLI also injects at
  the turn boundary).
- `ContinueReason` state machine — where "goal not satisfied →
  continue" plugs in.
- Stop hooks (`coco-hooks`) — the `pre_verify` analog exists as a hook
  event family; verify-on-stop composes with it rather than replacing
  it.
- Durable `task_list` + session persistence — `GoalState` rides the
  session store like hermes's `state_meta`.
- Aux model roles (`ModelRole`) — the judge routes to `Fast` (or a new
  `Judge` role if the design review prefers; note the repo rule: add a
  `ModelRole` variant rather than a raw string).

### Phase A — goal loop core

1. `GoalState { goal: String, subgoals: Vec<String>, turns_used: i64,
   max_turns: i64 /* default 20 */, status: GoalStatus, wait: Option<WaitBarrier> }`
   persisted per session (session store, key parity with hermes's
   `goal:<session_id>`).
2. After each terminal turn (the seam where
   `handle_no_tool_calls_terminal` returns `end_turn`): if a goal is
   active and **no human-origin item is queued** (preemption check via
   `QueueOrigin::Human` peek), run the judge — aux role, temperature 0,
   4096 max_tokens, last-response snippet cut UTF-8-safely at ~4000
   chars, 30 s timeout, **fail-open to continue** with a
   3-consecutive-parse-failure auto-pause.
3. Verdict `continue` → enqueue the continuation template as a
   plain user-role message with a new `QueueOrigin::Goal`; budget
   decrement; exhaustion → pause + user notice. Verdict `done` → clear
   state + notice. Ctrl+C/interrupt → auto-pause.
4. Commands: `/goal <text>`, `/goal pause|resume|clear|status`,
   `/subgoal <text>` (v1 can fold into `/goal sub <text>`).
5. Marker text avoids `[SYSTEM:` (anti-lesson 4); the continuation is
   visible in the transcript (it IS a user-visible loop, unlike
   reminders).

### Phase B — verify-on-stop + evidence ledger

1. Project-facts probe (coco flavor): detect `justfile` recipes,
   `Cargo.toml` (`cargo nextest run` / `just pre-commit`-style
   composites), `package.json` scripts × `_VERIFY_TARGETS` set,
   `Makefile` targets, `scripts/run_tests.sh`; cap 8. Cache per
   workspace root.
2. Evidence ledger as JSON/JSONL in the session/project state dir
   (coco convention; SQLite not required at this write rate):
   events (command, exit, ts) + state (last_edit_at, changed_paths ≤
   200). Producers: Bash outcome path (classify against detected
   commands, passed iff exit 0) + Write/Edit/ApplyPatch success path
   (`mark_workspace_edited`). Staleness = edit newer than last passing
   event.
3. Stop-seam nudge: on a terminal answer with code edits this turn and
   ledger not fresh-passed → inject the synthetic verification nudge
   (≤ 2 attempts), doc-only extension/filename skip, then continue the
   loop silently. Gate: `verify.on_stop` config enum
   `{ Off, Auto, On }`, `Auto` = ON for TUI/headless, OFF for
   SDK-server sessions (surface-aware default, hermes parity).
4. Compose with Stop hooks: hook decision runs first (existing
   ordering rules from the skill-learning work: review/verify logic
   runs after the stop-hook decision, skipped on `BlockedContinueLoop`).

### Phase C — wait barriers

`/goal wait <pid>` (process-exit barrier via the existing task/process
watch infrastructure), `wait_for_seconds`, `wait_on_session`; judge may
return `wait`. Lowest priority — ship A/B first.

## Design-review questions (blockers before code)

1. Judge role: reuse `Fast` vs new `ModelRole::Judge` (enum change
   ripples through config docs).
2. Where `GoalState` lives: session store vs `tasks` integration
   (goal ≈ a durable task with a judge — avoid two overlapping
   concepts; the review must pick one).
3. Continuation visibility: hermes shows it; coco could mark it meta.
   Recommend visible (user must see why the agent keeps going).
4. Interaction with the existing headless `--max-turns`-style budgets
   and with workflow runs (goals + workflows both re-enter the loop —
   define precedence, likely "goal loop never fires inside a workflow
   turn").
5. Evidence-ledger scope: per-session vs per-workspace (hermes keys
   state by `(session_id, root)` — probably right for coco too).
6. SDK surface: goal events over the AppServer protocol
   (multi-session plan D-decisions may constrain envelope shape).

## Non-goals

- Hermes's kanban/board integration (`goal_mode` cards), messaging
  surfaces, and the `darwinian-evolver`-style meta-loops.
- Auto-generated verification scripts in v1 (hermes's tempfile
  fallback) — detected canonical commands only; the fallback invites
  the model to write ad-hoc scripts with side effects.
- Any LLM in the evidence ledger — classification is exit-code +
  command matching, deterministic.
