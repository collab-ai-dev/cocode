# coco-coordinator

Spawn lifecycle for the agent-team subsystem. Owns the runner, runner
loop, mailbox IPC, team file, terminal pane backends (tmux / iTerm2 /
in-process), agent identity / discovery / reconnect, and the
[`coco_tool_runtime::AgentHandle`] implementation the tool layer
invokes.

## Layer

L5 (root). Sits next to `commands`, `tasks`, `memory`. **Does NOT depend
on `coco-state`** — would create a cycle (the cleanup at the end of
PR #3 deliberately broke this). Shared data shapes (mailbox protocol,
sub-agent state snapshots, team / teammate / standalone-agent context,
task entry) live in `coco_types::agent_ipc` so neither side has to
import the other for the types alone.

## Module map

| Module | Purpose |
|---|---|
| `runner` / `runner_loop` | Outer lifecycle + per-iteration scheduling. `InProcessAgentRunner`, `PermissionBridge`, `InProcessRunnerConfig`, `AgentExecutionEngine` trait. |
| `agent_handle/` | `SwarmAgentHandle: AgentHandle` — the bridge that AgentTool dispatches to. Split: `mod.rs` (struct + setters + trait impl + teammate dispatch), `spawn.rs` (sync + background subagent dispatch), `handoff.rs` (post-spawn classifier + AgentSummary), `resume.rs` (background-spawn resume). |
| `inprocess_backend` | `InProcessBackend: TeammateExecutor` — wraps `InProcessAgentRunner` for the registry. Lives outside `pane/` because it composes the runner. |
| `mailbox/{mod,io,lock,protocol}.rs` | File-based teammate inboxes (`~/.coco/teams/{team}/inboxes/{agent}.json`) with `fs2` advisory locking + retry/jitter. Split into `io.rs` (path / JSON r/w), `lock.rs` (fs2 + 30-retry exponential backoff), `protocol.rs` (envelope codec). |
| `team_file` | `~/.coco/teams/{team}/config.json` r/w + lock helpers. |
| `task` | `InProcessTeammateTaskState` — UI mirror state for in-process teammates (capped at 50 messages). |
| `identity` | 3-tier teammate identity resolution: thread-local context → dynamic context → env vars. |
| `discovery` | Team / teammate enumeration via `team.json`. |
| `prompt` | Teammate system-prompt addendum builder. |
| `reconnect` | Restore team context from resumed sessions. |
| `teammate` | Model fallback, init hooks, mode snapshot, leader permission bridge, spawn helpers. |
| `config` | `TeammateMode` (Auto / Tmux / Iterm2 / InProcess) + per-team config. |
| `worktree` | `AgentWorktreeManager` for `isolation: "worktree"` subagents. |
| `spawn` | CLI flag building + env var inheritance for spawned teammates. |
| `constants` | Tmux session names, env-var keys, `TEAM_LEAD_NAME`. Re-exports `coco_types::AgentColorName` for path stability. |
| `types` | `BackendType`, `TeammateIdentity`, `TeamManager`, `TeamFile`, `TeamMember`, `HandoffDecision`, `AgentSpawnResult`, plus the SwarmPermission* + related types. |
| `pane/mod.rs` | `PaneBackend` trait, `TeammateExecutor` trait, `BackendRegistry`, detection helpers (`is_inside_tmux`, `is_in_iterm2`, `is_it2_cli_available`, `is_tmux_available`). |
| `pane/{tmux,iterm2,pane_executor,layout,it2_setup}` | Concrete backend impls + iTerm2 Python bootstrap. |

## Key invariants

- **One-way layering**: this crate does not depend on `coco-state`.
  AppState integration goes the other way — `app/state` (and consumers
  like `app/cli`, `app/query`, `app/tui`) imports `coco_coordinator::*`
  directly.
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
  alias artifacts (those were a mechanical-move leftover and were
  removed in the post-extraction cleanup).
- `coco_types::AgentColorName` is the single canonical color enum;
  `crate::constants::AgentColorName` re-exports it.
- Pure-logic helpers belong in `coco-subagent` (catalog, prompt rendering,
  filter, fork context, transcript filter, coordinator-mode templates).
  This crate is the orchestration layer — tokio, fs2, file IO, env vars,
  process spawning.

## Color caches

Two distinct caches live in `pane::layout`, both reset by
[`clear_teammate_colors`]:

- `assign_teammate_color(name@team)` — per-teammate, persists across
  spawns within a session so `lead@my-team` always renders in the same
  color. Used by `agent_handle::spawn_teammate` and `send_message` color
  routing.
- `assign_agent_type_color(AgentTypeId)` — per-agent-type, so all
  `Explore` spawns share one color regardless of how many copies are
  running. Populated by `agent_handle::spawn::spawn_subagent`.

## Open follow-ups (tracked in code as `TODO(...)`)

- **`coco_memory::team_sync` snapshot bodies** (`TODO(PR3-step9)` markers
  there). Coordinator's spawn / terminate path is the consumer when the
  IO lands.
- **Coordinator-mode system-prompt swap at session bootstrap** — pure
  helper `coco_subagent::coordinator_system_prompt(simple_mode)` is
  ready; the wiring lives in `app/query` / `app/cli` (outside coordinator
  scope).
