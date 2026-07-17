# Swarm / Multi-Agent Coordination: jcode vs coco-rs

Two different lineages, two different topologies. **jcode** runs a persistent
daemon that hosts full live sessions and auto-groups them per git-repo into a
*swarm* with a central coordinator, push scheduling, heartbeats, and a
process-global file-activity bus. **coco-rs** mirrors Claude Code's
single-process, name-scoped *team* model: explicitly created teams, pull-based
self-claiming workers, file-based mailbox IPC, no daemon.

Several jcode wins are genuine for *concurrent multi-agent-in-one-repo*
workloads. Several coco-rs designs are already better engineering for its
stated goals. A difference is judged on merit for coco-rs's goals, not treated
as an automatic deficiency.

All claims below were read at source on both sides. File:line references are
to the repositories as checked out.

---

## jcode approach

**Topology — persistent daemon, repo-auto-grouped swarms.** `jcode serve` /
`jcode connect` runs a long-lived server over a Unix socket. The server owns
every live session's `Arc<Mutex<Agent>>` (`SessionAgents`,
`src/server/comm_control.rs`). Swarm membership is *derived from the repo*:
`swarm_id_for_dir` (`src/server/util.rs:94-102`) returns the **git common dir**
via `git_common_dir_for` (`util.rs:55`) — so every worktree of one repo joins
the same swarm — with a `JCODE_SWARM_ID` env override (`util.rs:95`). This is
the literal mechanism behind the README's "spawn two agents in the same repo
and they will automatically be managed."

**Core state** (`src/server/state.rs`): per-member `SwarmMember`
(session_id, status string, role string, `report_back_to_session_id`,
`latest_completion_report`, fan-out `event_tx`), `swarm_coordinators`,
`swarm_plans` (versioned task DAG with `task_progress`), `swarms_by_id`, and a
`SwarmEvent` ring buffer capped at `MAX_EVENT_HISTORY = 5000`
(`state.rs:289`). `SwarmEventType` (`state.rs:239-277`) covers `FileTouch`,
`Notification`, `PlanUpdate`, `PlanProposal`, `ContextUpdate`, `StatusChange`,
`MemberChange`. The durable subset survives reload via daemon snapshots
(`persist_swarm_state_for`).

**One `swarm` tool, ~40 actions.** Actions cover shared-KV context
(share/read/append), messaging (message/broadcast/dm/channel), plan lifecycle
(propose/approve/reject), spawn/stop/assign_role, status/report/summary, and
task control (assign_task / assign_next / fill_slots / run_plan / cleanup /
start / wake / resume / **retry / reassign / replace / salvage** /
subscribe_channel / await_members). Each routes a typed request over the
socket; the transport filters the response stream to the terminal typed
response matching the request id.

**Push scheduler with dependency + load affinity.**
`resolve_assignment_target_for_task` (`comm_control.rs:216-290`), when no target
is requested, ranks candidate agents by a 5-key sort (`comm_control.rs:246-281`):

```
right_carry.cmp(&left_carry)              // dependency carry-over, desc
  .then(right_meta.cmp(&left_meta))       // metadata carry-over, desc
  .then(left_load.cmp(&right_load))       // assignment load, asc
  .then(left_rank.cmp(&right_rank))       // ready(0) before completed(1)
  .then(left.session_id.cmp(&right))      // tiebreak
```

An agent that did the *blocking* task is preferred (warm context). The
coordinator then *pushes* the assignment, and if the assignee has no live
client it runs the task **in-process** via `spawn_assigned_task_run`
(`comm_control.rs:296-498`) — a full autonomous driver that mutates the plan,
spawns the agent loop, records the completion report, marks the task `done`,
persists, and broadcasts (`comm_control.rs:443-497`).

**Per-task heartbeat (separate timer) + live checkpoints.**
`spawn_assigned_task_run` starts a `tokio::time::interval` heartbeat task
(`comm_control.rs:361-408`) that ticks every `swarm_task_heartbeat_interval()`
(default 10s) calling `touch_swarm_task_progress` — independent of tool cadence,
so a long single tool call does **not** look stale. `touch_swarm_task_progress`
(`src/server/swarm.rs:146-154`) stamps `last_heartbeat_unix_ms`,
`heartbeat_count`, `last_detail`, `checkpoint_summary`, `checkpoint_count`. A
`task_progress_event_sender` intercepts the worker's tool events to update
`last_detail` / `checkpoint_summary` live. `refresh_swarm_task_staleness`
(`swarm.rs:175-205`) flips `running → running_stale` when
`now - last_heartbeat ≥ swarm_task_stale_after()` (default 45s); an arriving
heartbeat auto-revives stale → running. `RunningStale` is a first-class status
(`jcode-swarm-core/src/lib.rs:63`).

**Recovery handoff verbs.** `Reassign` / `Replace` / `Salvage`
(`comm_control.rs:1644-1767`): blocks when the source task is `running`
(returns an error with `retry_after_secs: Some(1)`, "Wait, wake, or stop that
agent before handing the task off", `comm_control.rs:1674-1684`); `Replace`
validates the status is `queued|failed|stopped|crashed|running_stale`
(`1686-1701`); **`Salvage`** reads the prior assignee's
`get_tool_call_summaries(12)` and appends `progress.checkpoint_summary` +
`progress.last_detail` into `format_salvage_message`
(`comm_control.rs:1703-1733`), then routes through `handle_comm_assign_task` to
the *new* target so model-authored progress carries over.

**Idempotent mutations.** Every mutating handler wraps in
`begin_swarm_mutation_or_replay` / `finish_swarm_mutation_request` keyed by
`swarm_mutation_request_key(session, action, args)`
(`comm_control.rs:748-983`) — a retried spawn/assign/stop replays the prior
persisted response instead of double-spawning.

**Crash-recoverable await.** `await_members` (`src/server/comm_await.rs`)
persists `PersistedAwaitMembersState`; on reconnect it replays the final
response **if the statuses still satisfy the predicate**
(`comm_await.rs:272-291`), otherwise re-subscribes to the `swarm_event_tx`
broadcast and re-arms the deadline timer (`spawn_or_resume_await_members:153`).

**File-shift conflict detection (the headline feature).** File tools publish
`BusEvent::FileTouch { session_id, path, op, intent, summary, detail }`
(`src/bus.rs:101-113`, `BusEvent::FileTouch` at `bus.rs:329`) onto a
process-global `Bus` singleton (`OnceLock`, `bus.rs:417`); producers are
`tool/{edit,write,read,apply_patch}.rs`. The server's bus consumer
(`server.rs:1505-1735`) records each touch with a 30-min expiry, finds
same-swarm peers, and **only on modification** (`touch.op.is_modification()`,
`bus.rs:96-98`) computes prior peer touches via `latest_peer_touches`, then
sends **bidirectional** alerts: the current agent learns who previously touched
the file (`server.rs:1624-1677`), and prior agents learn that someone "just
edited this file you previously worked with" (`server.rs:1679-1733`).
`file_activity_scope_label` (`src/server/file_activity.rs:39-55`) parses line
ranges from the summary strings and labels the alert **"overlapping lines"** /
**"same file, non-overlapping lines"** / **"same file"**. The agent-supplied
`intent` is threaded into both alerts (`server.rs:1630-1646`, "— intent:
<why>"). Each alert goes out as both a `ServerEvent::Notification {
NotificationType::FileConflict }` **and** a queued soft interrupt. Plain reads
are tracked but never proactively alert (`server.rs:1577-1580`).

**Delivery = soft interrupts injected mid-turn.** All inter-agent traffic (DM,
channel, broadcast, plan update, file conflict, task assignment) is delivered
via `queue_soft_interrupt_for_session` into a per-session
`SoftInterruptQueue = Arc<Mutex<Vec<SoftInterruptMessage>>>`
(`jcode-agent-runtime/src/lib.rs:3-20`). `queue_soft_interrupt`
(`src/agent/interrupts.rs:120-152`) uses a separate lock so it works *while the
agent is processing*. Messages carry an `urgent` flag; `has_urgent_interrupt`
(`interrupts.rs:186-192`) lets the turn loop skip remaining tools. The turn
streaming loop drains at multiple **safe points** — between tools only when
urgent (skips the rest of the batch), and after all tools when non-urgent — so
a peer's message interleaves into an in-flight turn without starting a new one.
Completed/idle agents do *not* auto-resume on notification; only explicit
assign/wake/start re-activates them.

**Shared context KV + topic channels.** `handle_comm_context`
(`src/server/client_comm_context.rs:43-155`) maintains
`shared_context: HashMap<swarm, HashMap<key, SharedContext>>` with set/append +
`SwarmEventType::ContextUpdate` + `NotificationType::SharedContext` fanout.
`handle_comm_list_channels` / `handle_comm_channel_members`
(`src/server/client_comm_channels.rs:21-74`) plus `subscribe_session_to_channel`
and `ChannelIndex.by_swarm_channel` give real pub-sub topic channels.

**Note on the design doc.** `SWARM_ARCHITECTURE.md` is marked
`Status: Proposed`. The coordination *engine* (assignment, heartbeat, await,
file-conflict, soft-interrupt, channels, shared-context) is fully built and
verified above; the Worktree-Manager-as-integrator role and the live
plan-info / swarm-info graph widgets the doc describes are thinner or absent in
the code. Treat that doc as a roadmap. Note also that `channel.rs` (Telegram /
Discord) and `bus.rs` (process-local UI broadcast) are *not* the swarm comms
layer — the real coordination lives in `src/server/comm_*` and
`tool/communicate/`.

---

## coco-rs approach

**Topology — single-process, name-scoped teams (Claude Code port).** No swarm
daemon, no per-repo auto-grouping. Teams are created explicitly by the model
via `TeamCreate` and persisted under `~/.claude/teams/{team}/config.json`
(`coordinator/src/team_file.rs:34-41`). The subsystem lives in the L5-root
`coco-coordinator` crate; shared shapes live in `coco_types::agent_ipc` to
avoid a cycle with `coco-state`.

**Two execution lanes.** (1) **Background `LocalAgent`** —
`AgentTool(run_in_background=true)` fans out a subagent in the same process,
registered in `TaskManager`, streaming output, resumable via per-agent JSONL
transcript. (2) **`InProcessTeammate`** — long-lived named teammate with
mailbox + team identity. `RemoteAgent` (CCR) is an explicit non-goal —
`AgentTool` returns an unsupported error.

**Runner loop is mailbox-poll-driven, pull-based** (`runner_loop.rs`). Per
teammate: build system prompt → claim a task → loop { `run_query` (through the
`AgentExecutionEngine` trait so `app/query` provides the engine, no cycle) →
tail-of-turn compaction → optional plan-approval gate → transition to idle →
`wait_for_next_prompt_or_shutdown` }. The wait function
(`runner_loop.rs:981-1083`) polls the mailbox every **500 ms**
(`POLL_INTERVAL_MS`) with a fixed priority: abort → shutdown request →
team-lead message → peer FIFO → unclaimed task-list task.

**Task assignment is pull-based self-claiming, NOT coordinator scheduling.**
`claim_first_available_task` (`runner_loop.rs:1138-1196`) lists the team task
list (a real DAG: `blocked_by`, `claim_task`, `owner`, `TaskClaimOutcome`),
filters to Pending + unowned + not-blocked-by-unresolved
(`runner_loop.rs:1156-1165`), and claims the **first** match. There is no
load-balancing, no dependency-affinity ranking, no coordinator push — every
idle worker races for the next free task. The leader influences work only by
writing into mailboxes and the shared task list.

**During a turn, only control messages are polled.** While `run_query` is in
flight, the runner's inner `tokio::select!` (`runner_loop.rs:430-435`) ticks a
`POLL_INTERVAL_MS` interval that calls **only** `drain_control_messages`
(mode-set / permission-update) — it does **not** read peer or leader *content*.
Content is read only at idle in `wait_for_next_prompt_or_shutdown`.

**Mailbox = file-based JSON inbox with fs2 advisory lock.**
`~/.claude/teams/{team}/inboxes/{agent}.json`, a JSON array of
`TeammateMessage{from,text,timestamp,read,color,summary}`. Writes serialize
through `with_inbox_lock` (fs2 exclusive lock on a sidecar `.lock`, retry with
exponential backoff + jitter, aligned to TS `proper-lockfile`), doing
read-append-write inside the lock to avoid TOCTOU loss.

**Rich typed protocol over the same inbox.** `ProtocolMessage`
(`coordinator/src/mailbox/protocol.rs:425-577`) is a `#[serde(tag = "type")]`
enum with **13 structured variants**: `IdleNotification`,
`PermissionRequest/Response`, `SandboxPermissionRequest/Response`,
`PlanApprovalRequest/Response`, `ShutdownRequest/Approved/Rejected`,
`TaskAssignment`, `TeamPermissionUpdate`, `ModeSetRequest`.
`is_structured_protocol_message` / `parse_protocol_message` discriminate. This
is materially richer per-message *typing* than jcode's free-text alerts.

**Messaging surface = `SendMessage` + `TeamCreate`/`TeamDelete`.**
`SendMessageTool` (`core/tools/src/tools/agent/send_message_tool.rs`) targets a
teammate name or `*` broadcast, with two TS-faithful behaviors absent in
jcode's tool:
- **Auto-resume** — if the target task is in a terminal state, it transparently
  resumes the *stopped* agent from its persisted JSONL transcript via
  `ctx.agent.resume_agent` instead of routing to the mailbox
  (`send_message_tool.rs:155-203`).
- **Pending-message queue** — if the target is a Running BgAgent, it pushes
  onto a per-task FIFO that surfaces as an `agent_pending_messages`
  system-reminder on the target's **next turn** (`send_message_tool.rs:213-231`).

Both gated by `Feature::AgentTeams`.

**Coordinator mode + handoff classifier (security).** Pure logic in
`core/subagent`: `coordinator_system_prompt`, `worker_tool_pool`,
`render_task_notification` (worker-terminate `<task-notification>` XML pushed to
the leader mailbox). A 2-stage LLM handoff safety classifier
(`coordinator/src/agent_handle/handoff.rs:16-67`) runs after every subagent
completion (`should_classify` gate at `handoff.rs:21`), builds a transcript
summary, and on a non-safe verdict can **block** the worker's output with a
`render_block_message` payload (`handoff.rs:62-66`). Team-memory writes are
secret-guarded (`check_team_mem_secret`, `core/tools/src/lib.rs:302`, called
from `edit.rs:324` / `write.rs:233`).

**Terminal backends** (`coordinator/src/pane/`): tmux / iTerm2 / in-process
pane backends with per-teammate and per-agent-type color caches — for *headed*
teammate panes, a capability jcode invests in differently (jcode launches
visible workers in new OS terminal windows).

**Persistence / recovery.** Team roster persists in `config.json`;
background-spawn resume works via on-disk JSONL transcript + meta. There is
**no** swarm-event ring buffer, **no** crash-recoverable await, **no**
idempotent-mutation replay layer, and **no** daemon snapshot — recovery is
per-team-file + per-agent-transcript only.

**Gate.** `Feature::AgentTeams` (`Stage::Experimental`), plus
`COCO_COORDINATOR_MODE` / `COCO_FORK_SUBAGENT` env.

---

## Head-to-head comparison

| Dimension | jcode | coco-rs |
|---|---|---|
| Topology | Persistent daemon, repo-auto-grouped swarm | Single-process, explicit named team |
| Membership | `git common dir` auto-join (`util.rs:94-102`) | Model calls `TeamCreate` |
| Scheduling | Coordinator **push** + 5-key affinity sort (`comm_control.rs:246-281`) | Worker **pull**, first-fit claim (`runner_loop.rs:1156`) |
| Message delivery | Soft-interrupt **mid-turn** at safe points (`interrupts.rs:120-192`) | Mailbox at idle; running BgAgent gets **next-turn** FIFO (`send_message_tool.rs:213-231`) |
| File-shift conflict | Bidirectional alerts + line-range overlap + intent (`server.rs:1505-1735`) | **None** |
| Liveness | 10s heartbeat timer + `RunningStale` + checkpoints (`comm_control.rs:361-408`, `swarm.rs:146-205`) | Progress counters only; **no heartbeat, no stale** |
| Recovery handoff | retry / reassign / replace / **salvage** with preserved progress (`comm_control.rs:1644-1733`) | resume *same* agent only |
| Crash recovery | Idempotent-replay mutations + persisted await (`comm_control.rs:748-983`, `comm_await.rs:272-291`) | Team-file + transcript only |
| Inter-agent typing | Free-text + `Other(String)` status enums (`jcode-swarm-core/lib.rs:11-115`) | 13-variant typed `ProtocolMessage` (`protocol.rs:425-577`) |
| Topic channels | join/leave/post pub-sub (`client_comm_channels.rs`) | Direct + `*` broadcast only; `subscriptions` field **dead** |
| Shared context KV | set/append/read (`client_comm_context.rs:43-155`) | None |
| Permission sync across agents | Per-session config only | Live mode + rule sync + worker→leader bridge |
| Handoff security | None (output flows back unfiltered) | 2-stage LLM classifier blocks output (`handoff.rs:16-67`) |
| Headed workers | New OS terminal windows | Multiplexed tmux / iTerm2 panes |

**Where jcode genuinely wins for concurrent-in-one-repo work:**

1. **File-shift conflict detection** — the single most useful primitive for
   parallel coding, and coco-rs has *nothing*. When agent A edits a file under
   agent B's feet, jcode tells B (and A) mid-turn with "overlapping lines" vs
   "same file" granularity plus the editor's *intent*. The mechanism is a
   process-global `Bus` every file tool publishes to, correlated against
   repo-derived swarm membership. coco-rs's teammates are isolated mailbox
   pollers with no shared file-activity channel.

2. **Mid-turn redirection** — a coco-rs worker 5 tool-calls deep into a wrong
   approach cannot be steered until it goes idle; jcode can interrupt it now.

3. **Dependency-affinity push scheduling** — jcode keeps the follow-up task on
   whoever did the blocking task (context locality) and balances load; coco-rs
   can pile sequential dependent tasks onto whichever cold worker polls first.

4. **Liveness telemetry** — jcode detects and salvages wedged workers
   (`RunningStale`); coco-rs cannot tell a wedged worker from a slow one.

5. **Crash robustness under churn** — idempotent mutations + persisted await
   are demanded by the daemon model; coco-rs re-does an interrupted
   assign/await on restart.

**Resource implications.** jcode's persistent daemon + per-task heartbeat tasks
+ 5000-entry event ring + global FileTouch bus cost steady-state memory/CPU
even when idle (the README's "~10MB per added session" reflects holding full
sessions resident). coco-rs's poll-based teammates are cheaper at rest (a 500 ms
mailbox stat) but the polling adds fixed latency to every cross-agent signal and
steady filesystem syscalls; jcode's push path has lower signal latency at the
cost of the always-on server.

---

## Where coco-rs already matches or wins

1. **Typed protocol envelopes vs free-text alerts.** coco-rs's `ProtocolMessage`
   (`protocol.rs:425-577`) is a closed 13-variant tagged union (permission,
   sandbox-permission, plan-approval, 3-way shutdown, mode-set,
   team-permission-update, task-assignment, idle). jcode's inter-agent traffic
   is largely free-text strings with string-extensible status enums
   (`SwarmLifecycleStatus` / `SwarmRole` carry `Other(String)`,
   `jcode-swarm-core/lib.rs:11-115`). For correctness and evolvability,
   coco-rs's closed typed unions are the better engineering — consistent with
   the project's "typed structs over Value / no hardcoded strings for closed
   sets" rule.

2. **Permission propagation across teammates.** coco-rs threads the full
   permission model into every teammate query: live permission mode + live rule
   set (`runner_loop.rs:415-418`), team-allowed-path rules loaded into the
   worker's rule store (`load_team_allowed_path_rules:1215-1230`),
   `ModeSetRequest` / `TeamPermissionUpdate` control messages that mutate a
   running teammate's permission state mid-loop via `drain_control_messages`
   (polled even *during* a turn at `runner_loop.rs:433`), and a worker→leader
   permission-request bridge with auto-resume of the blocked tool. jcode's
   swarm has no equivalent fine-grained, mode-aware, rule-syncing permission
   plumbing — it relies on each session's own permission config.

3. **Security: handoff classifier + team-memory secret guard.** coco-rs runs a
   2-stage LLM handoff safety classifier after every subagent completion
   (`handoff.rs:16-67`) that can block output, and a `check_team_mem_secret`
   block-don't-redact guard on team-memory writes
   (`core/tools/src/lib.rs:302`). jcode's swarm has no analog — a sub-agent's
   output flows back to the coordinator unfiltered. For multi-agent trust
   boundaries, coco-rs is more defensive.

4. **Transcript-backed resume of stopped agents.** coco-rs's `SendMessage`
   auto-resume (`send_message_tool.rs:155-203`) transparently restarts a
   *terminated* background agent from its persisted JSONL transcript when the
   model messages it — the model just keeps using `SendMessage`. jcode keeps
   sessions alive in the daemon (so "resume" is "the session never died"), but
   coco-rs recovers a stopped agent's *conversation* from disk without a
   resident process — the cheaper, more portable model for a non-daemon CLI.

5. **Headed teammate panes (tmux / iTerm2) are first-class.** coco-rs invests in
   pane backends with stable per-teammate and per-agent-type color caches
   (`coordinator/src/pane/`). jcode spawns visible workers by launching new OS
   terminal windows — coarser than coco-rs's in-terminal multiplexed panes.

6. **Layering discipline.** coco-rs keeps the swarm split clean: pure logic in
   `core/subagent`, orchestration in `coco-coordinator`, the execution engine
   injected via the `AgentExecutionEngine` trait to avoid a cycle with
   `app/query`, shared shapes in `coco_types::agent_ipc`. jcode's swarm is woven
   through a monolithic `src/server/*` with 10–18-argument handler functions
   (`#[expect(clippy::too_many_arguments)]` throughout `comm_control.rs`).
   coco-rs is more maintainable.

7. **jcode's `SWARM_ARCHITECTURE.md` over-claims relative to source.** It is
   explicitly `Status: Proposed`. The Worktree-Manager-as-integrator role and
   the live graph widgets are described richly but are thin/absent in the
   coordination code. The *engine* claims in the top-level README
   (auto-management, file-shift, DM/broadcast, agent-spawned swarms) are real;
   the architecture doc's widget/integration claims are roadmap. Anyone
   comparing should treat that doc as aspirational.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions whose adversarial verdict was **confirmed** or **nuanced** are
included. Nuanced corrections are folded in. Strong verifier missed-findings are
folded in as additional recommendations (M05-S7, M05-S8).

### M05-S1 — Cross-agent file-shift (edit-under-feet) conflict notifications [CONFIRMED]

**Why.** jcode's `BusEvent::FileTouch` → server cross-reference of same-swarm
peers → bidirectional alerts with line-range overlap classification
(`bus.rs:101-113`, `server.rs:1505-1735`, `file_activity.rs:39-55`) is the most
useful swarm primitive for parallel coding. coco-rs has **no** file-touch
channel and no conflict detection: greps for `file_touch` / `FileConflict` /
`overlapping` across `coordinator/` and `core/tools/agent/` return only a
*prompt string* (`agent_tool.rs` telling subagents to "work on non-overlapping
tasks") — soft prompt-time deconfliction, not real-time tracking. Teammates are
isolated mailbox pollers (`runner_loop.rs:1001-1065`).

**Concrete change.** In `coco-tool-runtime` / `coco-tools`, have
Edit/Write/Read/ApplyPatch emit a `FileTouch{agent_id, path, op, line_range,
intent}` onto an opt-in `mpsc::Sender<CoreEvent>` aggregate variant (per the
project's "single aggregate variant through an opt-in sink" event rule). In
`coco-coordinator`, a per-team `FileTouchRegistry` correlates touches across
active teammates; on a modification overlapping a peer's prior touch, write a
structured `FileConflict` `ProtocolMessage` to both inboxes (extend the
`protocol.rs` enum). Port the line-range overlap classifier from
`file_activity.rs:39-55`. Scope by `team_name` (coco-rs has no repo-swarm),
correct because teammates already share a cwd/worktree.

**Folded-in corrections (verifier):**
- jcode's design works *because* all agents share a process-global `Bus`
  singleton (`bus.rs:417`). coco-rs teammates may run **cross-process**
  (tmux / iTerm2 panes via file-mailbox IPC), where a process-global mpsc does
  **not** reach peers. So scope the registry to (a) **in-process teammates
  only**, or (b) persist touches to a **team-shared file**
  (`~/.claude/teams/{team}/file-activity.jsonl`, like the existing mailbox) so
  cross-process panes can poll it. The in-process opt-in-sink alone covers only
  in-process teammates.
- The line-range classifier depends on a `"lines N-M"` summary string in the
  same format jcode's tools emit (`edit.rs` "edited lines 45-60"). coco-rs's
  Edit/Read tools do **not** currently emit such a structured summary onto any
  channel, so that summary-format contract must be **added at the same time** —
  not "ported verbatim."
- Without M05-S2 the alert lands on the peer's **next turn** (still useful).

**Impact: high. Effort: high. Risk:** the touch sink must thread through
`ToolUseContext` for every file tool; must only alert on *modification* vs read
(like jcode, `server.rs:1577-1580`) to avoid noise.

**Non-goals:** respected.

---

### M05-S2 — Deliver urgent teammate messages at the earliest safe turn boundary [NUANCED]

**Why.** jcode interleaves messages mid-turn: `queue_soft_interrupt`
(`interrupts.rs:120-152`, separate lock so callable while processing),
`has_urgent_interrupt` (`interrupts.rs:186-192`), drained at safe points in the
turn loop (urgent skips remaining tools). coco-rs only delivers to a *running*
teammate via the `pending_messages` FIFO surfacing on the **next** turn
(`send_message_tool.rs:213-231`); the runner reads peer/leader content only at
idle (`runner_loop.rs:732-739` → `wait_for_next_prompt_or_shutdown`), and during
a turn its `tokio::select!` (`runner_loop.rs:430-435`) polls **only**
`drain_control_messages`, not content. A busy worker cannot be redirected
mid-turn.

**The original premise is wrong — do NOT "reuse the main-loop CommandQueue
mid-turn."** coco-rs's `CommandQueue` is **not** a mid-turn injector. An earlier
mid-turn `Now`-drain was **deliberately deleted** (`app/query/engine.rs:2569`)
because it inserted a User message between an assistant `tool_use` and its
matching `tool_result`, breaking pairing on providers that enforce it (Anthropic
400). `app/query/CLAUDE.md:92-95` states plainly: "No mid-turn `Now` drain …
coco-rs intentionally does not — it would break tool_use/tool_result pairing on
non-streaming providers. All priorities are honored at the turn-boundary
drain." Moreover the teammate path does **not** even wire a `CommandQueue`
(`with_command_queue` has **zero** hits in `coordinator/` — the queue is
`SessionRuntime`-scoped, main-loop only). coco-rs has **no** mid-turn injection
anywhere, by deliberate multi-provider safety design.

**Re-scoped concrete change (verifier).** Achievable, non-goal-respecting win:
1. Deliver urgent peer/leader messages at the **earliest safe turn boundary**
   rather than only on idle. Extend the in-turn `tokio::select!`
   (`runner_loop.rs:430-435`) to also peek the mailbox for an **urgent-flagged**
   `ProtocolMessage`; on seeing one, **cancel `current_turn_cancel`** (the
   runner already holds it — `runner_loop.rs:387, 419, 553`) so the turn
   *finalizes cleanly* and the next iteration immediately picks up the message,
   instead of waiting for natural completion + idle poll. This is bounded,
   pairing-safe (the turn completes before injection), and reuses existing
   cancellation.
2. Optionally honor jcode's urgent-skips-remaining-tools **only on streaming
   providers**, where coco-rs already runs tools via `StreamingHandle` and could
   safely stop the batch.

Drop the "reuse main-loop CommandQueue mid-turn" framing entirely — it
misrepresents a documented deliberate choice.

**Impact: high. Effort: high. Risk:** must respect tool_use/tool_result pairing
(the turn-cancel approach sidesteps this by finalizing first); add an
`urgent`-flagged peer/leader message channel (no such flag exists on
`TeammateMessage` today).

**Non-goals:** the *original* "reuse CommandQueue mid-turn" framing conflicts
with the documented no-mid-turn-injection choice; the **re-scoped** turn-cancel
approach respects it.

---

### M05-S3 — Per-teammate liveness heartbeat + stale detection [CONFIRMED]

**Why.** jcode spawns a per-running-task `tokio::time::interval` heartbeat
(`comm_control.rs:361-408`, default 10s) calling `touch_swarm_task_progress`
(`swarm.rs:146`, stamps `last_heartbeat_unix_ms` + `heartbeat_count`);
`refresh_swarm_task_staleness` (`swarm.rs:175-205`) flips
`running → running_stale` after default 45s; `RunningStale` is a first-class
status (`jcode-swarm-core/lib.rs:63`). coco-rs's `SubAgentStatus`
(`common/types/src/agent_ipc.rs:151-156`) is Pending/Running/Completed/Failed/
Interrupted — **no `RunningStale`**; `TaskProgress` (`common/types/src/task.rs`)
has tokens/tool-count/turn-count/`last_tool_name`/`recent_activities`/`summary`
— **no `last_heartbeat`, no `checkpoint_summary`**. A wedged worker is
indistinguishable from a slow one.

**Concrete change.** In `coco-coordinator`, spawn an **independent interval task
per in-process teammate** that stamps `last_heartbeat_at` while the `run_query`
future is alive (drop it on completion). Add `RunningStale` to
`coco_types::SubAgentStatus` and a `last_heartbeat_at` to
`SubAgentState`/`TaskProgress`; a lightweight coordinator-side sweeper marks
teammates whose heartbeat age exceeds a `stale_after` threshold. Surface
heartbeat-age + last-tool in the existing CoordinatorPanel/SubagentPanel. This
makes the reassign/salvage path (M05-S4) actionable.

**Folded-in correction (verifier).** The original rec proposed keying the
heartbeat off **tool boundaries** (`runner_loop.rs:468-491`). That would
**false-positive** on exactly the long-build case the risk note worries about
(no tool boundary for 60s → looks stale). Prefer jcode's design: a **separate
interval timer** decoupled from tool cadence, ticking while the run future is
alive — so a long single tool does not trip stale detection.

**Impact: medium. Effort: medium. Risk:** low — additive telemetry. Threshold
choice matters; key off the independent timer, not tool cadence.

**Non-goals:** respected.

---

### M05-S4 — Task reassign / salvage verbs that move a task to a different worker with preserved progress [CONFIRMED]

**Why.** jcode's `Reassign` / `Replace` / `Salvage` (`comm_control.rs:1644-1767`)
blocks a *running* task (returns error + `retry_after_secs`, `1674-1684`),
validates replaceable status (`1686-1701`), and `Salvage` forwards the prior
assignee's last 12 tool-call summaries + `checkpoint_summary` + `last_detail`
into the new assignee's prompt (`1703-1733`) so progress carries over. coco-rs
has **resume** (continue the *same* agent's transcript via `SpawnMode::Resume`,
`agent_handle/resume.rs:18-40`) but **no** verb to *move* a task to a
*different* worker with preserved progress. `SwarmAgentHandle` exposes
spawn/send_message/resume only. The task list has `claim_task`/`owner`/
`blocked_by` but unclaim + re-prompt-with-salvaged-context is not implemented.

**Concrete change.** Add a coordinator-facing reassign/salvage path in
`coco-coordinator`. On salvage: read the stuck agent's recent messages via the
**existing** `coco_subagent::build_transcript_summary`
(`core/subagent/src/handoff.rs:182`, already strips tool-result bodies and is
bounded — the reuse target exists), unclaim its task via `TaskListStore`, and
spawn/assign a fresh worker with a "salvage prior progress from {name}"
preamble + that summary. Surface it as a new team action or coordinator command.
Gate on M05-S3 stale detection.

**Folded-in corrections (verifier):**
- coco-rs has **no** `checkpoint_summary`/`last_detail` on `TaskProgress`, so
  salvage context can only carry `build_transcript_summary` output +
  `recent_activities`/`last_tool_name` — it **cannot** match jcode's
  model-authored checkpoint unless M05-S3's progress fields are extended too
  (add `checkpoint_summary` there; see also M05-S8).
- The running-block must use coco-rs's actual liveness signal: today that is the
  teammate's `current_work_cancel` being set
  (`set_teammate_current_work_cancel`, `runner_loop.rs:943-952`) + `is_idle` on
  the `TeammateTask`. Gate salvage on those, and prefer salvaging
  `RunningStale`.

**Impact: medium. Effort: medium. Risk:** must block reassign while the source
worker is mid-turn (jcode rejects with retry_after) to avoid two agents on one
task.

**Non-goals:** respected.

---

### M05-S5 — Dependency-affinity claim-ordering hint (NOT a push scheduler) [NUANCED]

**Why.** jcode's `resolve_assignment_target_for_task` (`comm_control.rs:246-281`)
ranks workers by dependency carry-over → metadata affinity → load → ready-rank,
then *pushes*. coco-rs workers self-claim the **first** unblocked task
(`runner_loop.rs:1156`) — no ranking, no load, no affinity (grep for
`affinit`/`load_balanc`/`dependency_carryover`/`preferred_owner` in
`coordinator/` + `tasks/` returns nothing).

**This is a fundamental architecture divergence, not a missing feature.** jcode
uses a coordinator-**push** model (a central `resolve_assignment_target_for_task`
picks and pushes via assign_task + soft-interrupt); coco-rs uses a **pull**
model (idle workers self-claim from a shared `TaskListStore`,
`tasks/task_list.rs::claim_task`) — which mirrors TS. Porting the central
scheduler conflicts with that documented pull / TS-faithful non-goal.

**Concrete change — advisory claim-ordering hint ONLY (verifier):**
1. Add `TaskRecord.preferred_owner: Option<String>`, set at task-completion time
   to the completer of a task in *this* task's `blocked_by` (dependency
   carry-over).
2. In `claim_first_available_task` (`runner_loop.rs:1156`), among eligible
   tasks **prefer** those where `preferred_owner == claimant` before falling
   back to first-fit. Keep it advisory — no central assigner.
3. The **load-balancing arm is NOT cleanly portable to pull** (no worker sees
   global load at claim time without a shared in-flight counter). Implement only
   a cheap **soft per-worker in-flight cap** if load skew is observed; treat
   affinity strictly as a tiebreaker. Do **not** port
   `metadata_carryover`/`ready-rank`/`load-sort` wholesale — that requires the
   central plan-DAG view coco-rs deliberately lacks.

**Impact: medium. Effort: medium. Risk:** the wholesale 5-key sort conflicts
with the pull non-goal; only the **tiebreaker subset** is in-scope. Affinity can
mis-route in mixed-skill teams — keep it a tiebreaker only.

**Non-goals:** the push-scheduler interpretation conflicts; the **claim-ordering
hint** respects the pull model.

---

### M05-S6 — Bounded team event journal for coordinator visibility + restart catch-up [NUANCED]

**Why.** jcode keeps a 5000-entry `SwarmEvent` ring (`state.rs:239-289`,
`MAX_EVENT_HISTORY=5000`) covering FileTouch / Notification / PlanUpdate /
PlanProposal / ContextUpdate / StatusChange / MemberChange; `record_swarm_event`
appends + broadcasts via `swarm_event_tx`; the await machinery subscribes
(`comm_await.rs`); the ring survives reloads. coco-rs has **no** swarm-scoped
event journal — state is reconstructed from `config.json` roster + per-agent
transcripts + point-in-time `TaskManager` progress only; `roster_store.rs` has
no journal (only roster CRUD). There is no "who joined / changed status / was
assigned" feed for a reconnecting leader or overview panel.

**Folded-in correction (verifier).** The original evidence over-claimed that the
ring "feeds catch-up." It does **not**: jcode's catch-up brief
(`src/catchup.rs:38-54` `build_brief`) is reconstructed from **per-session
state** (`render_messages`, `collect_touched_files`, `collect_tool_counts`,
`collect_activity_steps`) — **not** from the `SwarmEvent` ring. The ring feeds
the member-await machinery + real-time event subscription/UI. So a coco-rs
catch-up-after-restart capability would be a **new** feature (inspired by
jcode's per-session brief *combined with* a new team event journal), not a
direct port of "the ring feeds catch-up."

**Concrete change.** Add a bounded append-only `TeamEventLog` to
`coco-coordinator` (in-memory `VecDeque` + optional
`~/.claude/teams/{team}/events.jsonl`, capped). Emit on transitions the runner
already computes (roster `set_member_active`; idle/working transitions and
task-notification in `runner_loop.rs`). Consume it for (a) a coordinator "team
activity" view in the existing CoordinatorPanel, and (b) a coco-rs analog of a
returning-leader catch-up brief. Keep it **isolated from `CoreEvent`** per the
event-system non-bridging rule (unless a single aggregate variant is needed).

**Impact: medium. Effort: medium. Risk:** low — additive. Must cap size and
avoid logging message bodies/secrets (reuse `coco_secret_redact`).

**Non-goals:** respected (kept isolated from `CoreEvent`; not a wire enum).

---

### M05-S7 — Inter-agent topic channels (group-chat pub-sub) + wire the dead `subscriptions` field [VERIFIER FINDING]

**Why.** jcode has join/leave + post-to-channel + member inspection:
`handle_comm_list_channels` / `handle_comm_channel_members`
(`client_comm_channels.rs:21-74`), `ChannelIndex.by_swarm_channel`,
`subscribe_session_to_channel`. coco-rs has only direct send + `to:"*"`
broadcast (`send_message_tool.rs`, `roster_store.rs::broadcast_recipients`); its
`TeammateContext.subscriptions` field (`coordinator/src/types.rs:243`) is
**dead** — always `Vec::new()` (`roster_store.rs:151, 216`), never consumed for
routing.

**Concrete change.** Either (a) remove the dead `subscriptions` field for
hygiene, or (b) wire it: let a teammate subscribe to named topics, route
`SendMessage(to: "#topic")` to all subscribers via the mailbox, and surface
member lists. Channel state can live per-team in `roster_store` or a sidecar
file. This is additive and respects the typed-protocol direction (add a
`ChannelPost` `ProtocolMessage` variant rather than free text).

**Impact: low-medium. Effort: medium. Risk:** low. Channel fan-out over the
file-mailbox adds write amplification proportional to subscriber count — bound
it.

**Non-goals:** respected.

---

### M05-S8 — Add `checkpoint_summary` / `last_detail` to `TaskProgress` for higher-fidelity salvage handoff [VERIFIER FINDING]

**Why.** jcode's `task_progress` carries `checkpoint_summary` + `last_detail` +
`checkpoint_count` + `heartbeat_count` (`swarm.rs:127-154`), persisted and
forwarded on salvage (`comm_control.rs:1723-1731`). coco-rs's `TaskProgress`
(`common/types/src/task.rs`) has only a `summary` field (written by the periodic
AgentSummary timer) — no model-authored checkpoint or last-detail. So even with
M05-S4, salvage can forward a transcript summary but not a model-authored
checkpoint.

**Concrete change.** Add `checkpoint_summary: Option<String>` (and optionally
`last_detail`) to `coco_types::TaskProgress`, written at tool-boundaries or from
the AgentSummary path; forward it in the M05-S4 salvage preamble. Materially
improves handoff fidelity and dovetails with M05-S3 (same progress struct) and
M05-S4 (the consumer).

**Impact: low-medium. Effort: low. Risk:** low — additive field. Truncate like
jcode (`truncate_detail`, 120 chars) and run it through `coco_secret_redact`.

**Non-goals:** respected.

> Not promoted to a standalone recommendation: jcode's **shared-context KV
> store** (`client_comm_context.rs:43-155`, set/append/read with
> `ContextUpdate` fanout). coco-rs teammates coordinate via mailbox messages +
> the durable task list, and the task list already serves much of the
> shared-state role. A KV store is a plausible future addition but is lower
> value than M05-S1–S8 for a coding-agent team; it is left out of the ranked
> set deliberately.

---

## Rejected after adversarial review

No suggestion in the analyst set received a **refuted** verdict — all six
(M05-S1 … M05-S6) were upheld as **confirmed** (S1, S3, S4) or **nuanced**
(S2, S5, S6), and the nuanced corrections are folded into the recommendations
above. For completeness, the specific *framings* that were dropped during
review:

- **M05-S2's original premise — "reuse the main-loop `CommandQueue` mid-turn
  machinery" — was rejected.** It misrepresents a documented deliberate choice:
  coco-rs's mid-turn `Now`-drain was *removed* to preserve tool_use/tool_result
  pairing on non-streaming providers (`app/query/engine.rs:2569`,
  `app/query/CLAUDE.md:92-95`), and the teammate path does not even wire a
  `CommandQueue` (zero `with_command_queue` hits in `coordinator/`). The
  recommendation was re-scoped to a pairing-safe turn-boundary cancel.

- **M05-S5's wholesale 5-key push scheduler was rejected** as conflicting with
  coco-rs's deliberate pull / TS-faithful self-claim model
  (`runner_loop.rs:1156`, `tasks/task_list.rs::claim_task`). Only the advisory
  `preferred_owner` claim-ordering tiebreaker (+ an optional soft in-flight cap)
  is in-scope; `metadata_carryover` / `ready-rank` / global `load-sort` require
  the central plan-DAG view coco-rs intentionally lacks.

- **M05-S6's "the 5000-ring feeds catch-up" linkage was rejected as
  imprecise.** jcode's catch-up brief is per-session-state-derived
  (`catchup.rs:38-54`), not ring-derived; the ring feeds member-await + the
  real-time subscription. The underlying optimization (a team event journal) is
  valid and kept.
