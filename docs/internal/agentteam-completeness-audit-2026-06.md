# Agent-Teams Completeness Audit — 2026-06

> **Status (2026-06-04, re-verified): gap 6 + gap 7-pane now implemented;
> several "remaining" gaps were stale.** A HEAD re-verification (5 adversarial
> agents, one per remaining gap + a baseline pass) corrected the table below —
> see **§ Re-verification (2026-06-04)**. Net picture:
> - **In-process teams: complete & usable.**
> - **Cross-process teams: bootable + driven + reports back + now shuts down
>   cleanly.** This pass closed the one real deliverability blocker (shutdown
>   leaked processes/panes on exit).
>
> **Done:** gap 1 (inbox→turn pump — keystone), gap 2 (clap launch break),
> gap 3 (plan-approval codec), gap 4a/4b (teammate→leader content), gap 5
> (leader team-awareness reminder — `team_context` + `agent_pending_messages`
> fed; `teammate_mailbox` deliberately `None` — coco-rs delivers a teammate's
> mailbox as injected TURNS via the pump/runner, so a parallel reminder would
> double-deliver; now documented at the source), gap 7 (team-dir cleanup
> on exit), **gap 6 (shutdown round-trip — Phase 1)**, **gap 7-pane
> (orphan-pane kill on exit — Phase 1)**, **gap 11 (coordinator-mode resume
> survival — Phase 2)**, **gap 12 (worker badge — Phase 3)**.
>
> **Remaining (re-sequenced):** gap 8 **DONE (Phase 4 + 4-UI + tail)** —
> producer + bidirectional consumer (mode via `SetPermissionMode`, rules via
> the shared live-rules `Arc`) + `team_allowed_paths` boot seeding + the TUI
> roster picker all landed; gap 9 **DELETED (Phase 5)**; gap 10 (DEFERRED —
> blocked on sandbox-bootstrap, out of agentteam scope); and the two-process
> tmux E2E (validation debt for gaps 1 / 4b / 6) — the last open item.

### What Phase 4 (gap 8) landed — the functional core

The cross-process consumer turned out **much lighter** than the audit's
"thread `TeammateControlState` into the pump" framing: the cross-process
teammate is a normal `coco` session, so a `ModeSetRequest` is applied by
**reusing the session's existing `UserCommand::SetPermissionMode` seam**
rather than building a parallel control state.

- **Producer:** `TeamRosterStore::set_member_mode` (team.json + live roster
  write-back, TS `setMemberMode`) + `SwarmAgentHandle::set_teammate_mode`
  (write-back **+** `ModeSetRequest` to the teammate's mailbox) +
  `AgentHandle::set_teammate_mode` trait method (default-`Err`).
- **In-process consumer:** already wired — `runner_loop::drain_control_messages`
  applies `ModeSet` + `TeamPermissionUpdate` to the live `TeammateControlState`.
- **Cross-process consumer:** `teammate_inbox_pump::drain_control_tick` drains a
  leader `ModeSetRequest` each tick and injects `UserCommand::SetPermissionMode`
  (fire-and-forget; no turn, no handshake). Pure `control_message_to_command`
  helper is unit-tested.
- **Tests:** `control_mode_set_maps_to_set_permission_mode` +
  `control_other_message_maps_to_none`.

**TUI roster picker (Phase 4-UI) — landed.** A `ModalState::TeamRoster`
interactive modal, opened with **Ctrl+T** when a team is active (gated on
`session.subagents` containing a teammate). It lists the running teammates
(read synchronously from `session.subagents`, no new event stream / no
coordinator coupling), and the leader cycles the focused teammate's mode
(←/→ over the four interactive modes) and applies it on Enter via
`UserCommand::SetTeammateMode` → `AgentHandle::set_teammate_mode`. Full TEA
wiring: state + `ModalState` variant, `show::team_roster` constructor,
`picker_styled::team_roster_lines` (reuses `render_select_list`),
`surface_content` dispatch, `KeybindingContext::TeamRoster` +
`map_team_roster_key`, `nav` / `confirm` / `team_roster_cycle_mode`
handlers, en + zh-CN i18n keys. Tests: `team_roster_lists_only_running_teammates`,
`team_roster_cycle_mode_wraps_interactive_modes`.

**gap-8 tail — DONE.** The cross-process teammate now seeds its engine
config's `live_permission_rules` `Arc` at boot from
`load_team_allowed_path_rules(team)` (now `pub`), and the pump's
`drain_control_tick` extends that same `Arc` on a leader `TeamPermissionUpdate`
(via `into_permission_rules`). Both mirror the in-process `TeammateControlState`
exactly — the main `QueryEngineConfig.live_permission_rules` seam made this a
small wire, not the "thread a parallel control state" lift the audit feared.
The `Arc` is built at the teammate boot branch in `tui_runner`, installed via
`update_engine_config`, and shared into `teammate_inbox_pump::spawn`.

## Re-verification (2026-06-04) — corrections to the original table

A HEAD re-check found the original 5-gap "remaining" table partly stale:

| Gap | Original claim | Verified at HEAD | Action this pass |
|---|---|---|---|
| **6 shutdown** | large, all owed | Accurate. Request-delivery wired; the whole approval leg (worker producer → leader consumer → kill_pane + remove member + unassign tasks) was missing; all helpers existed unwired. | **Implemented** (round-trip + prompt). |
| **7 pane-kill** | (folded into cleanup) | Dir/worktree/task cleanup wired at `tui_runner:715`; `kill_orphaned_teammate_panes` was dead → tmux panes orphaned on exit. | **Implemented** (backend-correct orphan kill before dir removal). |
| **8 leader controls** | large; add codec + drain logic | **Stale.** `create_mode_set_request` / `into_permission_rules` / `drain_control_messages` ModeSet+TeamPermissionUpdate apply-logic ALL exist and are wired in-process. Only missing: a **producer** (no leader caller), `team_file::set_member_mode`, the **cross-process** pump invoking `drain_control_messages`, and cross-process `team_allowed_paths` seeding. No TUI teams-roster surface exists yet. | Deferred to a dedicated PR (Phase 4). |
| **9 resume** | medium, pure wiring | **Reframed.** `reconnect.rs` is implemented with genuinely zero callers, but a working identity-driven team-context path (`resolve_team_snapshot`) already exists — `reconnect.rs` + `AppState.team_context` are a redundant orphan. | **Deleted (Phase 5).** Removed `coordinator/reconnect.rs` (3 fns + tests + `pub mod`), `AppState.team_context` field, and the now-unused `coco_state::TeamContext` re-export. The identity dynamic-context tier (`get_dynamic_team_context`, used by the 3-tier identity resolution) is load-bearing and intentionally kept; only its unused `set_dynamic_team_context` setter lingers (deleting it alone would be a half-measure — tracked, not part of this orphan). |
| **10 sandbox** | medium, two-way stub | **Blocked.** The sandbox approval bridge is fully dead in production (`check_network` / `*_async` / `set_approval_bridge` zero callers; nothing emits `SandboxApprovalRequired`). Even single-process sandbox network approval doesn't work; the worker side would be dead code. | **Deferred** — sandbox-bootstrap, not agentteam. |
| **11 coordinator mode** | medium, product decision | **Mostly wired.** `engine_prompt:80` gate fires; env-launch parity works for the live session today. Only **resume survival** is missing — the `saveMode` snapshot half (`MetadataEntry::Mode`) is absent; the `reconcile_on_resume` read path is fully wired. | **Implemented (Phase 2)** — `SessionManager::save_mode` + exit-checkpoint snapshot. |
| **12 worker badge** | low | Accurate; additive `Option` fields, ~6 files. coco-rs can do better than TS (real per-teammate color vs TS hardcoded `cyan`). | **Implemented (Phase 3)** — `coco_types::WorkerBadge` threaded request→event→state→title. |

### What Phase 2 (gap 11) + Phase 3 (gap 12) landed

**Phase 2 — coordinator-mode resume survival:**
- `SessionManager::save_mode(id, "coordinator"|"normal")` appends a `Mode`
  metadata entry (`app/session/src/lib.rs`). The reader / `reconcile_on_resume`
  path was already fully wired — only the writer was missing.
- `tui_runner` exit checkpoint snapshots `is_coordinator_mode(features)` into
  the transcript (gated on `Feature::AgentTeams` to keep non-team transcripts
  clean). A resumed coordinator session now re-enters coordinator mode.
- Env-launch (`COCO_COORDINATOR_MODE=1`) remains the TS-faithful entry trigger;
  no CLI flag added (TS has none either).

**Phase 3 — worker badge at the permission seam:**
- New `coco_types::WorkerBadge { name, color: AgentColorName }`.
- Threaded `Option<WorkerBadge>` through `ToolPermissionRequest` →
  `TuiOnlyEvent::ApprovalRequired` → `PermissionPromptState`. The cross-process
  producer (`leader_permission.rs`) sets it with the worker's real assigned
  palette color (improves on TS's hardcoded `cyan`); in-process requests leave
  it `None` (TS parity).
- The prompt renderer suffixes the title with `· @name` (TS
  `PermissionRequestTitle.tsx:32`). The text surface is monochrome, so the
  typed color is carried for styled / SDK consumers.
- 4 new tests (`save_mode` round-trip, title-with/without-badge); 20 existing
  permission test literals updated for the additive field. `PanePromptState`
  gained `#[allow(clippy::large_enum_variant)]` (single-instance UI state).

### What gap 6 + gap 7-pane implementation landed (this pass)

- `mailbox::create_shutdown_approved_message` now carries the approver's
  own `pane_id` / `backend_type` (empty pane id collapses to `None`).
- `AgentHandle` gained `request_shutdown` (leader→teammate),
  `respond_to_shutdown` (teammate→leader, enriches with self pane coords
  from `team.json`), and `teardown_teammate` (leader-side: `kill_pane` via
  the pane backend + `rollback_member` + `unassign_teammate_tasks`). All
  three implemented on `SwarmAgentHandle`; default-`Err` elsewhere.
- `SendMessageTool` intercepts structured `shutdown_request` /
  `shutdown_response` and routes them through the handle (with TS-parity
  validation: no broadcast request; response must target `team-lead`).
- `leader_inbox_poller` gained a `ShutdownApproved` arm → `teardown_teammate`.
  This unifies in-process + cross-process teardown through the one poller.
- `TaskListHandle::unassign_teammate_tasks` added (disk impl overrides;
  default no-op).
- `cleanup_session_teams` now kills orphaned **tmux** panes before removing
  dirs (iTerm2 orphan-on-crash teardown documented as a follow-up; the
  graceful path uses the registry-routed backend via `teardown_teammate`).
- Teammate prompt addendum now explains the shutdown-response protocol.
- 14 unit tests added (protocol round-trip, SendMessage branches + validation,
  prompt addendum, unassign). The `poll_once` `ShutdownApproved` arm +
  `teardown_teammate` are **E2E-unvalidated** (like gap 1 / gap 4b) — covered
  by the planned two-process tmux test.

---

> **(Original, superseded by the re-verification above.) Status (2026-06-04):
> 7 / 12 gaps fixed** — gap 1 (cross-process teammate
> inbox→turn pump — THE keystone), gap 2 (clap launch break), gap 3
> (plan-approval codec), gap 4a (in-process pending-message loop), gap 4b
> (teammate→leader regular messages + idle notifications), gap 5 (leader
> team-awareness reminder), gap 7 (team cleanup on exit).
> **Remaining:** gap 6 (shutdown end-to-end — the pump delivers ShutdownRequest
> as a turn; pane teardown is still owed), gap 8 (leader controls producer —
> the pump skips ModeSet/TeamPermissionUpdate, leaving them unread for this
> wiring), gap 9 (resume restoration), gap 10 (sandbox sync), gap 11
> (coordinator-mode reachability), gap 12 (worker badge). Cross-process can now
> boot AND be driven AND report back: a pane teammate consumes its mailbox and
> runs real turns (gap 1), and the leader surfaces a teammate's regular
> messages + idle notifications to its model via the command queue (gap 4b).

## Progress & TODO (live tracker)

**Done (7 / 12)** — commit refs on `feat/core`:

- [x] **gap 2** — clap launch break (drop identity flags, rely on env) · `9d82f0b5`
- [x] **gap 3** — plan-approval codec (`#[serde(default)]` timestamp) · `9d82f0b5`
- [x] **gap 4a** — shared `InMemoryPendingMessageStore` wired into engine + `SwarmAdapter` (leader→teammate) · `47a92992`
- [x] **gap 5** — leader team-awareness `team_context` reminder · `6ebf107d`
- [x] **gap 7** — team cleanup on exit (`cleanup_session_teams`) · `6ebf107d`
- [x] **gap 1** — cross-process teammate inbox→turn pump (THE keystone): `scan_next_prompt` + `teammate_inbox_pump` + turn-id handshake · `6028ba72`
- [x] **gap 4b** — teammate→leader regular messages + idle notifications via `CommandQueue` (`QueueOrigin::Coordinator`) · `1fe24787`

**Remaining (5 / 12)** — prioritized; size + dependency noted:

- [ ] **gap 9 — resume restoration** · _medium, pure wiring, low risk · RECOMMENDED NEXT._
  `reconnect.rs` (`compute_initial_team_context` / `initialize_from_session` /
  `extract_team_metadata`) is implemented + tested with **zero callers**. Call
  `compute_initial_team_context` on fresh teammate boot and the resume helpers
  on `--resume`, writing into team context. Same source family as gap 5.
- [ ] **gap 6 — shutdown end-to-end** · _large._ Pump already delivers
  `ShutdownRequest` as a turn; still owed: leader-side `ShutdownApproved`
  consumer → `kill_pane` (via `BackendRegistry`) + `remove_member_by_agent_id`
  + `unassign_teammate_tasks`; worker-side structured-approval producer; wire
  `executor.terminate`/`send_shutdown_request` into `TaskStopTool`'s teammate
  path; teammate prompt shutdown addendum.
- [ ] **gap 8 — leader controls producer** · _large._ In-process consumer wired,
  no producer. Pump currently leaves ModeSet/TeamPermissionUpdate **unread**.
  Add a leader action (`create_mode_set_request` + `write_to_mailbox` +
  `team_file::set_member_mode`); cross-process teammate poller applies
  `ModeSetRequest`/`TeamPermissionUpdate` via `drain_control_messages` logic;
  stop passing empty `team_allowed_paths`.
- [ ] **gap 10 — sandbox network-permission sync** · _medium._ Mirror
  `MailboxPermissionBridge` in the sandbox proxy deny path; extend
  `leader_inbox_poller` with a `SandboxPermissionRequest` arm → existing
  `SandboxApprovalRequired` TUI event. Types exist + tested, zero callers.
- [ ] **gap 11 — coordinator-mode reachability** · _medium, includes a product
  decision._ A CLI flag / `TeamCreate` side-effect / settings toggle that sets
  `CocoCoordinatorMode` (or persists `SessionMode::Coordinator`) so
  `engine_prompt.rs:90` fires and the wired `reconcile_on_resume` becomes live.
- [ ] **gap 12 — worker badge at the permission UI seam** · _low._ Add optional
  `worker_badge {name,color}` to `ToolPermissionRequest` → `ApprovalRequired` →
  TUI confirm renderer; in-process currently hardcodes `agent_id = session_id`.

**Validation debt — L0 + L1 LANDED (2026-06); L2/L3 remain.**

- [x] **L0 — `COCO_TEAMS_DIR` hermetic infra.** `EnvKey::CocoTeamsDir` +
  `team_file::teams_base_dir()` override (the single base-dir resolution
  point, so all mailbox/team-file paths isolate at once).
- [x] **L1 — hermetic pump integration tests.** `teammate_inbox_pump.test.rs`
  now drives `drain_control_tick` / `scan_tick` against a REAL on-disk mailbox
  (tempdir via `COCO_TEAMS_DIR`): mode-set→`SetPermissionMode`+marked-read,
  plain-message framing, shutdown-over-peer priority. The pump's file IPC is
  now integration-proven, not just in-memory-mocked. (Serialized via an async
  `ENV_LOCK` + nextest per-process isolation.)
- [x] **L2 — real-binary PTY + wiremock (PASSING).**
  `app/cli/tests/teammate_pty_e2e.rs` (`#[ignore]`): spawns the REAL `coco`
  binary in a PTY with `COCO_AGENT_*` identity + a temp `COCO_CONFIG_DIR`
  (`providers.json` → wiremock SSE model) + a seeded `COCO_TEAMS_DIR` mailbox;
  asserts the model received the framed prompt. **Verified passing** — the
  cross-process teammate genuinely boots → pump consumes mailbox → runs a turn.
- [x] **L3 — real tmux pane primitive (PASSING).**
  `coordinator/src/pane/tmux.test.rs::tmux_create_pane_spawns_real_pane`
  (`#[ignore]`, tmux-gated): `TmuxBackend::create_teammate_pane` creates a real
  pane on the PID-scoped swarm socket. **Verified against tmux 3.4.**
- [x] **External-mode socket bug — FIXED + regression-tested.**
  `TmuxBackend` external mode (`is_native=false`) created panes on the PID
  swarm socket but ran `kill_pane`/`send_command`/`set_pane_*`/`hide`/`show`/
  `rebalance` on the default socket — so external-session teammate panes
  couldn't receive commands or be torn down (a real gap-6/gap-1 bug). Fix:
  collapsed the socket choice into `TmuxBackend::socket()` + a single
  `TmuxBackend::run()` entry point; every op now targets the backend's own
  server (native `$TMUX` / external PID socket). The L3 test
  (`tmux_pane_lifecycle_create_and_kill`) was upgraded to create→kill and
  assert the pane is gone — fails pre-fix, passes post-fix (tmux 3.4).

- [ ] (original framing) **gaps 1 / 4b / 6 / 8-cross are unit-covered but
  E2E-unvalidated**
  (like #257). The components have deterministic unit tests
  (`scan_next_prompt` priority, `inject_and_wait` handshake,
  `control_message_to_command`, protocol round-trips, shutdown constructors,
  roster filtering/cycle), but the cross-process boot→consume→turn→report
  loop has no end-to-end test. **Why it's a separate effort (2026-06
  finding):** a real two-process test needs (a) a **mock-model harness** the
  spawned teammate process points at — without a model the teammate can't run
  a turn to assert on; (b) **tmux** (gate via `is_tmux_available`); and (c)
  even a lighter *hermetic* integration test of the pump's mailbox path is
  blocked because the mailbox/team-file API resolves its base dir from
  `dirs::home_dir()` directly (not injectable) — so it would either pollute
  the real `~/.claude/teams` or require an env-mutation hack (discouraged) or
  a base-dir-injection refactor across all mailbox callers. Recommended
  sequencing: first make `team_file::teams_base_dir` injectable (or add a
  `COCO_TEAMS_DIR` override consumed by `mailbox`/`team_file`), then write a
  hermetic pump integration test (no process spawn), then the full
  two-process tmux test on top.


Adversarial completeness audit of the coco-rs agent-teams subsystem
(in-process + cross-process teammates) against the TS reference
(`agents/claude-code-kim/src`). 10 capability areas × (independent
investigation + adversarial verification) + synthesis — 21 agents.
Verdicts below reflect the **verified** state (verifier corrections applied
over investigator claims).

## Overall verdict

**In-process teammates: borderline-usable, NOT a complete feature.**
The spawn/run loop is the one genuinely production-wired pillar
(`AgentTool → SwarmAgentHandle → roster_store → disk + InProcessBackend →
run_in_process_teammate`): real multi-turn LLM work, task claiming, mailbox
idle/shutdown/peer handling, compaction, idle-report to leader (verified,
regression-tested). Create/delete + one-team guard + active-member delete
guard work. Permission sync works (teammate Ask-deny inherits the leader's
`TuiPermissionBridge`). Per-turn Escape interrupt is wired. **But** the
leader-awareness data plane is dead (`AppState.team_context` has zero
writers; team-context / teammate-mailbox / pending-message reminders are
None-fed → the leader's *model* never perceives it is leading a team and
**never sees a teammate's regular message** — teammate→leader content is
silently dropped). Plan-mode approval is **broken** (a serde codec mismatch
— the consumer requires a `timestamp` every writer omits → an actively
approving leader blocks the teammate forever). Leader→teammate
mode/permission push has a wired consumer but no producer. Resume never
restores team context. Net: a single teammate doing work + permissions +
interrupt is usable; two-way team awareness, plan approval, leader controls,
and resume are not.

**Cross-process (tmux/iTerm2) teammates: NOT usable.** The producer half
(leader → pane) is wired (backend detect/register, pane create, command
build, OS launch, initial-prompt write, leader-side permission forwarding).
But the consumer half on the spawned child was missing end-to-end: there was
**no teammate-side inbox→turn pump** (the only one ran exclusively inside
the in-process runner), so a pane teammate booted and **sat idle forever**.
*(Resolved — gap 1: `app/cli::teammate_inbox_pump` now drives turns from the
child's mailbox. The rest of this paragraph reflects the pre-fix snapshot.)*
Compounding it, `build_teammate_command` emits identity CLI flags the clap
`Cli` struct does not define (no catch-all) → the spawned `coco` child
**fails argument parsing on launch** (identity already rides `COCO_*` env,
making the flags redundant AND launch-breaking). Lifecycle leaks: no
graceful-shutdown hook → panes + team dirs orphan on SIGINT; `commit_member`
writes `is_active=true` for pane members with no reset path → the disk-backed
delete guard can **permanently block deleting** any team that spawned a pane
member. Only the regular tool-permission round trip works cross-process.

## Completeness matrix

| Capability area | in-process | cross-process |
|---|---|---|
| Team lifecycle (create/delete/cleanup/reconnect) | partial | partial |
| In-process teammate spawn + runner loop | **wired** | na |
| Cross-process / pane spawn (tmux/iTerm2) | na | **stub** |
| Leader↔teammate messaging (mailbox/SendMessage/idle/msg→turn) | content→leader **wired** (gap 4b) | msg→turn **wired** (g1), content→leader **wired** (g4b) |
| Worker→leader permission sync (bridge/mailbox/sandbox) | partial | partial |
| Plan-mode approval (request → approve → exit) | partial (**broken**) | stub |
| Leader controls (set mode / push permission rules) | partial | **missing** |
| Teammate shutdown (request → approve → pane kill + cleanup) | partial | stub |
| Leader awareness (team_context / reminder / TUI / coordinator prompt) | **stub** | partial |
| Coordinator mode + 3-tier identity | partial | partial |

`wired` = production-reachable & functional; `partial` = some wired some
not; `stub` = present but None/Err/no-op / zero production callers; `missing`
= absent; `na` = not applicable.

## Prioritized gaps (12) — fix is mostly *wiring existing logic*

### Critical
1. **Cross-process teammate has no inbox→turn pump** (cross). — **FIXED.**
   A launched pane/tmux child now reads its own file mailbox and drives turns.
   *Implemented:* lifted the priority scan into the shared
   `runner_loop::scan_next_prompt` (shutdown>team-lead>peer>unclaimed-task;
   `wait_for_next_prompt_or_shutdown` now loops over it, in-process behavior
   unchanged). New `app/cli::teammate_inbox_pump` spawns when
   `resolve_teammate_identity()` is `Some` + `Feature::AgentTeams`: it ticks
   the scan, frames each result via `format_as_teammate_message`, and injects
   it as `UserCommand::SubmitInput` — **not** the command queue, which only
   drains mid-turn and cannot *start* one. Serialization is the crux: a
   `SubmitInput` `drain_active_turn(Wait)` CANCELS any in-flight turn, so the
   pump blocks on a **turn-id-correlated** completion handshake
   (`PumpDoneGuard` fires the turn's `user_message_id`; the pump waits for its
   own id, ignoring foreign human/slash turns) before the next scan.
   Always-framing keeps content off the `SubmitInput` empty/slash `continue`
   early-returns (which would skip turn-spawn and wedge the handshake). A
   dedicated `pump_cancel` fired after `app.run()` returns lets the pump drop
   its `command_tx` clone so the driver can shut down (else the process hangs
   on exit). Mis-injection guard: `scan_next_prompt` filters on
   `!is_structured_protocol_message`, so a stray response/notification in the
   teammate's own inbox can never be injected as a model prompt.
   *Deferred (own gaps, documented in the pump module):* live ModeSet /
   TeamPermissionUpdate application (gap 8 — left unread), pane teardown on
   ShutdownRequest (gap 6 — delivered as a turn so the teammate wraps up),
   teammate→leader idle/result reporting. **E2E-unvalidated** like #257 —
   needs a two-process tmux test.
2. **Spawned coco child fails clap parsing** (cross). `build_teammate_command`
   emits `--agent-id/--agent-name/--team-name/--parent-session-id/
   --agent-color/--plan-mode-required`, none defined on `Cli`, no catch-all.
   Identity already rides `COCO_*` env.
   *Fix:* drop the identity flags from `coordinator/src/spawn.rs`
   `build_teammate_command` (+ `build_inherited_cli_flags`); rely on the
   already-exported `COCO_AGENT_*` env (identity.rs tier-3). Update
   `spawn.test.rs` which locks in the broken shape.
3. **Plan-mode approval response is unparseable in-process** (in-proc).
   `coordinator::mailbox::ProtocolMessage::PlanApprovalResponse.timestamp` is
   a REQUIRED serde field, but every leader-side writer (TUI human approve at
   `tui_runner.rs:1448` + model SendMessage) omits it → `wait_for_plan_approval`
   never matches → an approving leader blocks the teammate forever.
   *Fix:* add `#[serde(default)]` to `protocol.rs` `PlanApprovalResponse.timestamp`
   (or emit a timestamp from writers); preferably consolidate on
   `coco_tool_runtime::plan_approval` as the single codec, with a round-trip
   test. Cross-process additionally needs `is_teammate/plan_mode_required`
   bridged from `coco_coordinator::identity` into `QueryEngineConfig`.
4. **Leader never surfaces a teammate's regular message / idle notification
   to its model** (both). — **FIXED** (4a + 4b). 4a wired the shared
   `Arc<InMemoryPendingMessageStore>` into BOTH `engine.with_pending_messages`
   and the `SwarmAdapter` reminder source (the leader→teammate direction).
   4b extended `leader_inbox_poller::poll_once`: a plain-text teammate message
   is enqueued onto the leader's `CommandQueue` with `QueueOrigin::Coordinator`
   (framed via `format_teammate_messages`, drained into the leader's next
   turn), and a new `IdleNotification` arm surfaces "teammate X is now idle /
   completed task Y" the same way; both mark-read after enqueue. The poller no
   longer early-returns when no approval UI is registered — only the
   `PermissionRequest` arm needs it; everything else is queue-routed. This is
   the unified teammate→leader content path for in-process AND cross-process
   (both write to the team-lead mailbox). Note: `agent_pending_messages` is the
   leader→teammate reminder and (per TS `attachments.ts:1088`) returns empty
   for the main thread, so the leader's own inbound goes through the queue, not
   that reminder.

### High
5. **Leader-awareness data plane dead in-process** (both). `AppState.team_context`
   never written; team_context/teammate_mailbox reminders None-fed → the
   leader's model has zero awareness it runs a team.
   *Fix:* `TeamCreateTool::execute` returns an `AppStatePatch` setting
   `team_context` via the ready-made `reconnect::compute_initial_team_context`
   (zero callers today). Back `SwarmAdapter::team_context` off the roster
   (`active_team_name` + members → `TeamContextSnapshot`) and
   `teammate_mailbox` off the mailbox handle; install `SwarmAdapter` into
   `ReminderSources`. Prefer the roster as the single live source.
6. **Shutdown unwired end-to-end** (both). No leader-side `ShutdownApproved`
   consumer (poller drops it), no worker-side structured-approval producer,
   and the leader's shutdown-REQUEST initiator (`executor.terminate`) has zero
   callers. Membership/tasks/panes leak.
   *Fix:* wire `executor.terminate`/`send_shutdown_request` into `TaskStopTool`'s
   teammate path; add a `SendMessageTool` shutdown-approval branch (read self
   pane_id/backend_type from team file → real `ShutdownApproved`; fix
   `create_shutdown_approved_message`'s hardcoded `None`); extend
   `leader_inbox_poller` with a `ShutdownApproved` arm → `kill_pane` (via
   `BackendRegistry`) + `remove_member_by_agent_id` + `unassign_teammate_tasks`
   (all implemented) + mark task completed. Port shutdown guidance into the
   teammate prompt addendum.
7. **No graceful / session-end cleanup of orphaned teams** (both).
   `cleanup_session_teams` + `get_session_cleanup_teams` +
   `kill_orphaned_teammate_panes` are dead (registry written, never read) → on
   SIGINT/crash, pane processes + team dirs leak (gh-32730 class).
   *Fix:* register a shutdown hook in app/cli (near the tui_runner
   graceful-shutdown block) iterating `get_session_cleanup_teams()` →
   `cleanup_session_teams(session_id)` → `cleanup_team_directories` +
   backend-aware `kill_orphaned_teammate_panes` (route through the pane
   `BackendRegistry` keyed on `member.backend_type`, not hardcoded tmux).
8. **Leader cannot set teammate mode / push permission rules** (both).
   In-process consumer fully wired; NO producer; `team_allowed_paths` always
   empty (`team_tools` passes `Vec::new()`). Cross-process also lacks a
   teammate-side control poller.
   *Fix:* add a leader action (TUI roster control or slash command) calling
   `create_mode_set_request` + `write_to_mailbox` AND a new
   `team_file::set_member_mode` (port from teamHelpers.ts) for write-back.
   Wire `TeamCreate.allowed_paths` from real intent. Cross-process: the gap-1
   teammate poller also applies `ModeSetRequest`/`TeamPermissionUpdate` via
   `drain_control_messages` logic.

### Medium / Low
9. **Resume never restores team context** (medium). `reconnect.rs`
   (`compute_initial_team_context`/`initialize_from_session`/
   `extract_team_metadata`) is implemented + tested but has zero callers.
   *Fix:* call `compute_initial_team_context` on fresh teammate boot, and
   `initialize_from_session`/`extract_team_metadata` on resume, writing into
   `AppState.team_context`. Pure wiring.
10. **Sandbox network-permission sync is a full stub both directions**
    (medium). `ProtocolMessage::SandboxPermissionRequest/Response` have zero
    callers; leader poller skips them; `worker_sandbox_permissions` never
    populated.
    *Fix:* mirror `MailboxPermissionBridge` in the sandbox proxy network-deny
    path (request via mailbox + block on own inbox); extend
    `leader_inbox_poller` with a `SandboxPermissionRequest` arm → existing
    `SandboxApprovalRequired` TUI event + reply. Types exist + tested.
11. **Coordinator mode unreachable in production** (medium).
    `is_coordinator_mode` gates on `CocoCoordinatorMode` env nothing sets;
    `matchSessionMode` resume-flip has no producer (only a circular Mode
    re-append). Leader can never enter coordinator mode; cross-process workers
    render no `<task-notification>` read-back.
    *Fix:* a CLI flag / TeamCreate side-effect / settings toggle that sets
    `CocoCoordinatorMode` (or persists `SessionMode::Coordinator` at save from
    the live gate) so `engine_prompt.rs:90` fires and the wired
    `reconcile_on_resume` becomes functional. Mirror the in-process
    `<task-notification>` render in the pane terminate path. Wire
    `coordinator_user_context` into prompt assembly.
12. **Worker identity (badge) dropped at the permission UI seam** (low).
    `ApprovalRequired` carries no `agent_id`/badge; in-process hardcodes
    `agent_id = session_id`.
    *Fix:* add optional `worker_badge {name,color}` to `ToolPermissionRequest`
    → `ApprovalRequired` → TUI confirm renderer. Cross-process
    `leader_permission.rs` already has `agent_id = worker_name@team`; for
    in-process, thread the teammate identity into the permission request.

## Reuse map (close gaps by wiring existing logic, not writing new)

- `runner_loop.rs:981 wait_for_next_prompt_or_shutdown` — exact mailbox
  priority logic the cross-process teammate poller needs; lift to a shared helper.
- `coordinator/src/mailbox/{io,protocol}.rs` — `read_mailbox`/
  `read_unread_messages`/`mark_message_as_read_by_index`/`parse_protocol_message`/
  `format_teammate_messages`: ready file-IPC primitives for the missing
  teammate consumer, the leader regular-message branch, and the reminder sources.
- `coordinator/src/reconnect.rs` `compute_initial_team_context`/
  `initialize_from_session`/`extract_team_metadata` — implemented + tested,
  zero callers; close BOTH the resume gap and the TeamCreate team_context write.
- `coco_system_reminder` `QueueOrigin::Coordinator` + `wrap_command_text` —
  framing for delivering a teammate message as a queued turn.
- `coco_tool_runtime::InMemoryPendingMessageStore` +
  `SwarmAdapter::with_pending_messages` + `engine_builder::with_pending_messages`
  — full pendingMessages pipeline exists; only the two-site shared-Arc wiring
  in session_runtime is missing.
- `core/system-reminder/.../generators/team.rs` `TeamContextGenerator`/
  `TeammateMailboxGenerator` — render correctly; only the `GeneratorContext`
  inputs need populating from the roster + mailbox.
- `roster_store.rs active_team_name` + member list — live cross-process source;
  build `TeamContextSnapshot` from it (avoids the unreachable TUI-only AppState
  field; `agent_handle.rs` already chose roster as authoritative).
- `tasks/src/task_list.rs unassign_teammate_tasks` +
  `team_file.rs remove_member_by_agent_id` — implemented; wire into a leader
  shutdown handler.
- `coordinator/src/pane/{tmux,iterm2}.rs kill_pane` + the pane `BackendRegistry`
  — functional backend-typed pane kills; reuse for shutdown + backend-aware
  orphan cleanup.
- `runner_loop_mailbox_permission.rs MailboxPermissionBridge` — template for
  the missing sandbox worker side (swap message types).
- `app/cli/src/leader_inbox_poller.rs poll_once` scaffold — extend its match
  (regular msg, IdleNotification, ShutdownApproved, SandboxPermissionRequest)
  rather than writing new pollers; clone its structure for the teammate-side
  control poller.
- `coordinator/src/identity.rs run_with_teammate_context` /
  `create_teammate_context` — wrap `run_in_process_teammate` to populate tier-1
  identity.
- `runner_loop.rs:1085 drain_control_messages` +
  `WireTeamPermissionUpdate::into_permission_rules` — the cross-process control
  poller reuses this apply logic.
- `coco_subagent` `coordinator_user_context` / `render_task_notification` /
  `session_mode_switch_action` — byte-faithful pure logic ready to consume.
- `app/cli/src/tui_permission_bridge.rs TuiPermissionBridge` — reuse as the
  sandbox prompt resolver + missing leader desktop notification.

## Phased sequencing

**Phase 0 — make in-process a complete feature** (highest leverage, smallest
surface, all wiring):
1. Fix the plan-approval codec mismatch (gap 3) — `#[serde(default)]` on
   `PlanApprovalResponse.timestamp`.
2. Close teammate→leader content loop (gap 4) — shared pending store +
   leader_inbox_poller regular-message + IdleNotification branches.
3. Populate leader awareness (gap 5) — TeamCreate writes team_context;
   SwarmAdapter sources from roster + mailbox.
4. Wire shutdown for in-process (gaps 6, 7) — terminate→TaskStop, approval
   branch, leader consumer, graceful cleanup hook.
5. Add a producer for leader controls (gap 8) + `set_member_mode` write-back;
   stop hardcoding empty `team_allowed_paths`.
6. Wire resume restoration (gap 9).

**Phase 1 — make cross-process launch-and-drive** (depends on Phase 0 shared
helpers):
7. Fix the launch break (gap 2) — drop identity CLI flags, rely on env.
8. Add the teammate-side inbox→turn pump (gaps 1, 8-cross, 3-cross) — reuse
   `wait_for_next_prompt_or_shutdown` + mailbox + `drain_control_messages`.
9. Bridge `is_teammate`/`plan_mode_required` into top-level `QueryEngineConfig`.
10. Fix the cross-process delete-guard hazard — reset `is_active` for pane
    members on stop/exit.
11. Wire cross-process lifecycle cleanup — backend-aware
    `kill_orphaned_teammate_panes` + `cleanup_session_teams` in the shutdown hook;
    render `<task-notification>` in the pane terminate path.

**Phase 2 — polish / parity** (non-blocking): sandbox permission sync (gap 10),
worker-badge identity (gap 12), leader desktop notification, coordinator-mode
reachability (gap 11), and deleting the dead parallel impls
(`PermissionSyncBridge`, file-based pending dir, `get_teammate_executor`) that
mask the live paths.

## Methodology

Workflow `agentteams-completeness-audit` (run `wf_69bf04a7-cbe`, 2026-06):
10 capability areas, each investigated independently (TS + coco-rs read,
in-process vs cross-process state, stub/caller verification) then
adversarially verified (skeptic refutes both over-claimed-wired and
under-claimed-stub). The verifier corrected several investigator claims —
notably proving the plan-approval `timestamp` codec mismatch and the clap
launch break, and refuting a wrong "Escape interrupt is dead" claim.
