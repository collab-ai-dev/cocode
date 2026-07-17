# coco-coordinator

Spawn lifecycle for the agent-team subsystem. Owns the runner, runner
loop, mailbox IPC, team files, terminal pane backends (tmux / iTerm2 /
in-process), agent identity / discovery, and the
[`coco_tool_runtime::AgentHandle`] implementation the tool layer
invokes.

## Layer

L5 (root). Sits next to `commands`, `tasks`, `memory`. Shared data shapes
(mailbox protocol, sub-agent state snapshots, team / teammate /
standalone-agent context, task entry — incl. `InProcessTeammateTaskState`,
which lives in `coco_types`) live in `coco_types::agent_ipc` so the
coordinator, host, and surfaces share values without importing one another.

## Module map

| Module | Purpose |
|---|---|
| `runner` / `runner_loop` | Outer lifecycle + per-iteration scheduling. `InProcessAgentRunner`, `PermissionBridge`, `InProcessRunnerConfig`, `AgentExecutionEngine` trait. |
| `runner_loop_mailbox_permission` / `_notify` / `_wait` | Split from `runner_loop`: cross-process worker permission over mailbox IPC (pane-mode teammates can't share the leader's `ToolPermissionBridge`); notification / task helpers writing to the team-lead inbox; plan-approval mailbox waiter. |
| `agent_handle/` | `SwarmAgentHandle: AgentHandle` — the bridge AgentTool dispatches to. Split: `mod.rs` (struct + setters + trait impl + teammate dispatch), `spawn.rs` (sync + background subagent dispatch), `handoff.rs` (post-spawn classifier + AgentSummary), `resume.rs` (background-spawn resume), `teammate_engine.rs` (`AgentQueryEngine` → `AgentExecutionEngine` bridge). |
| `inprocess_backend` | `InProcessBackend: TeammateExecutor` — wraps `InProcessAgentRunner` for the registry. Lives outside `pane/` because it composes the runner. |
| `mailbox/{mod,io,lock,protocol}` | File-based teammate inboxes (`<config_home>/teams/{team}/inboxes/{agent}.json`) with `fs2` advisory locking: `io.rs` (path / JSON r/w), `lock.rs` (30-retry exponential backoff), `protocol.rs` (envelope codec). |
| `team_file` | `<config_home>/teams/{team}/config.json` r/w + lock helpers. `COCO_TEAMS_DIR` overrides the base dir (test isolation). |
| `roster_store` | Coordinator-owned roster lifecycle — the single write path for team membership and active/idle transitions (`team_file` stays raw file IO for discovery/tests). |
| `session_team` | Implicit session-team bootstrap: a leader session idempotently owns one team, `session-<sessionId[:8]>`, created at CLI startup — not by a model tool call. |
| `identity` | 3-tier teammate identity resolution: thread-local context → dynamic context → env vars. |
| `discovery` | Team / teammate enumeration from the teams dir. |
| `prompt` | Teammate system-prompt addendum builder. |
| `teammate` | Model fallback, init hooks, mode snapshot, leader permission bridge, spawn helpers. |
| `config` | `TeammateMode` (Auto / Tmux / Iterm2 / InProcess) + per-team config. |
| `worktree` | `AgentWorktreeManager` for `isolation: "worktree"` subagents. |
| `spawn` | CLI flag building + env var inheritance for spawned teammates. |
| `constants` | Tmux session names, env-var keys, `TEAM_LEAD_NAME`. Re-exports `coco_types::AgentColorName` for path stability. |
| `types` | `BackendType`, `TeammateIdentity`, `TeamManager`, `TeamFile`, `TeamMember`, `HandoffDecision`, `AgentSpawnResult`, plus the SwarmPermission* + related types. |
| `error` | Tier-3 typed error: `ErrorExt` + `StatusCode` classification. |
| `pane/` | `PaneBackend` / `TeammateExecutor` traits, `BackendRegistry`, detection helpers (`is_inside_tmux`, `is_in_iterm2`, …); concrete `tmux` / `iterm2` / `pane_executor` / `layout` / `it2_setup` backends. `layout::assign_teammate_color(name@team)` is the per-teammate color cache (stable within a session). |

## Key invariants

- **One-way layering**: coordinator behavior depends on core contracts and
  shared `coco-types`; agent-host assembly installs its handles into each
  session runtime. Query and TUI consume typed events/snapshots rather than a
  global application-state crate.
- **`AgentColorName` lives in `coco_types`** (canonical, also used by
  `core/subagent`). `crate::constants::AgentColorName` is a re-export
  alias kept for path stability inside the crate.
- **`SpawnMode::Fork` end-to-end**: `AgentTool::execute` builds it from
  `ctx.messages` + `ctx.rendered_system_prompt` (gated on
  `coco_subagent::is_fork_subagent_active` and the recursion guard
  `is_in_fork_child`). `SwarmAgentHandle::spawn_subagent` consumes it via
  `coco_subagent::build_fork_context` + `preserve_tool_use_results = true`.
- **Coordinator-mode tool pool**: `SwarmAgentHandle::spawn_subagent`
  applies `coco_subagent::worker_tool_pool(simple_mode)` to subagent
  `allowed_tools` when `coco_subagent::is_coordinator_mode(&features)`.
- **Coordinator `<task-notification>` XML**: `runner_loop`'s cleanup
  path renders `coco_subagent::render_task_notification(...)` and pushes
  it to the leader's mailbox on worker terminate (when coordinator mode
  is active).

## Conventions

- Modules import siblings via `use crate::<module>` — no `as swarm_*`
  alias artifacts.
- Pure-logic helpers belong in `coco-subagent` (catalog, prompt rendering,
  filter, fork context, transcript filter, coordinator-mode templates).
  This crate is the orchestration layer — tokio, fs2, file IO, env vars,
  process spawning.

## Open follow-ups (tracked in code as `TODO(...)`)

- **`coco_memory::team_sync` snapshot bodies** (`TODO(PR3-step9)` markers
  there). Coordinator's spawn / terminate path is the consumer when the
  IO lands.
- **Coordinator-mode system-prompt swap at session bootstrap** — pure
  helper `coco_subagent::coordinator_system_prompt(simple_mode)` is
  ready; the wiring lives in `app/query` / `app/cli` (outside coordinator
  scope).
