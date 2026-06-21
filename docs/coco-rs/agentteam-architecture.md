# Agent / Team / Swarm — Unified Architecture

This document supersedes the conflicting `subagent-refactor-plan.md` and
`agent-loop-refactor-plan.md` for the agentteam subsystem and tracks the
multi-PR migration started on the `feat/agentteam` branch.

The TS source is the behavior reference. TS file paths are relative to the TS
project's `src/` directory. Every module in this design has a TS-source pointer;
coco-rs mirrors TS semantics unless explicitly noted (Anthropic-only features —
CCR remote, GrowthBook gates, `feature('ULTRAPLAN')` — are skipped per root
`CLAUDE.md`).

## Layered home for each concern

```
┌──────────────────────────────────────────────────────────────────────┐
│  App:        cli, tui, session, query, state                        │
│  Root:       coordinator (NEW, deferred), commands, skills, hooks,  │
│              tasks, memory, plugins, keybindings                     │
│  Core:       subagent (catalog + prompt + filter + fork +           │
│              transcript + coordinator_mode), tool-runtime, tools,   │
│              permissions, messages, context, system-reminder         │
│  Exec:       shell, sandbox, …                                       │
└──────────────────────────────────────────────────────────────────────┘
```

| Concern | Crate today | Crate eventually | TS source |
|---|---|---|---|
| Definition catalog (built-ins, source precedence, snapshot) | `core/subagent` | unchanged | `tools/AgentTool/loadAgentsDir.ts`, `builtInAgents.ts` |
| AgentTool prompt rendering | `core/subagent::prompt` | unchanged | `tools/AgentTool/prompt.ts` |
| Tool filter planning + permission parsing | `core/subagent::filter` | unchanged | `tools/AgentTool/agentToolUtils.ts`, `permissionSetup.ts` |
| Frontmatter parsing + validation | `core/subagent::{frontmatter,validation}` | unchanged | `tools/AgentTool/loadAgentsDir.ts` |
| Fork-subagent rules + XML | `core/subagent::fork` | unchanged | `tools/AgentTool/forkSubagent.ts` |
| Resume transcript filtering | `core/subagent::transcript` | unchanged | `tools/AgentTool/resumeAgent.ts` |
| Coordinator-mode prompt + worker pool + `<task-notification>` XML | `core/subagent::coordinator_mode` | unchanged | `coordinator/coordinatorMode.ts` |
| `AgentTool` / `TeamCreate` / `TeamDelete` / `SendMessage` / `Skill` schemas | `core/tools/src/tools/agent.rs` | unchanged | `tools/{AgentTool,TeamCreateTool,TeamDeleteTool,SendMessageTool,SkillTool}/` |
| `AgentSpawnRequest` + `SpawnMode` DTO | `core/tool-runtime::agent_handle` | unchanged | `tools/AgentTool/AgentTool.tsx` (input shape) |
| `/agents` slash command | `commands/src/handlers/agents.rs` | unchanged | `commands/agents/` |
| Spawn lifecycle (runner, runner-loop, mailbox, file-IO, identity, discovery, reconnect, in-process backend) | `app/state/swarm_*` (21 modules, ~9 k LoC) | **new `root/coordinator` crate (deferred PR)** | `tasks/{Local,Remote,InProcessTeammate}AgentTask`, `utils/swarm/`, `utils/teammateMailbox.ts` |
| Terminal backends (tmux, iTerm2, panes, layout, it2 setup) | `app/state/swarm_backend*` + friends | sub-modules of `root/coordinator` (`pane::*`) | `utils/swarm/backends/{TmuxBackend,ITermBackend,PaneBackendExecutor}.ts`, `it2Setup.ts` |
| Team memory sync | `memory::team_sync` (scaffold) | wired to coordinator handoff (PR #5 follow-up) | `services/teamMemorySync/`, `tools/AgentTool/agentMemorySnapshot.ts` |

## Feature gating

`Feature::AgentTeams` is the single capability gate (`common/types/src/features.rs`,
key `agent_teams`). PR #5 promoted it to `Stage::Experimental` so it
appears in the `/experimental` menu.

| Sub-mode | Gate (env) | Composition |
|---|---|---|
| Coordinator mode | `COCO_COORDINATOR_MODE` | `Feature::AgentTeams` AND env truthy → `coco_subagent::is_coordinator_mode(&features)` |
| Fork subagent path | `COCO_FORK_SUBAGENT` | `Feature::AgentTeams` AND env truthy AND **not** coordinator AND **not** non-interactive → `coco_subagent::is_fork_subagent_active(&features, is_non_interactive)` |

Both env keys live on `coco_config::EnvKey` (`CocoCoordinatorMode`,
`CocoForkSubagent`). Per root `CLAUDE.md` no crate calls `std::env::var`
ad-hoc — always go through `coco_config::env::is_env_truthy(EnvKey::…)`.

## Three task kinds

`coco_types::TaskType` already enumerates them:

| Variant | TS source | Lifecycle |
|---|---|---|
| `LocalAgent` | `tasks/LocalAgentTask/` | Background fan-out subagent in same process. `AgentTool(run_in_background=true)` or `background: true` definition. |
| `RemoteAgent` | `tasks/RemoteAgentTask/` | CCR/Teleport remote session. **Skipped in coco-rs** — `AgentTool` returns explicit unsupported error. |
| `InProcessTeammate` | `tasks/InProcessTeammateTask/` | Long-lived teammate (named identity, mailbox, team-aware). |

The three are typed at the foundation. Wiring `AgentTool` to register
the right variant on spawn is part of the deferred PR #3 (the runner
sits in `app/state/swarm_*`).

## Spawn dispatch (target shape)

```rust
// coco_tool_runtime::AgentSpawnRequest
pub struct AgentSpawnRequest {
    // user input
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub team_name: Option<String>,
    // …

    // parent inheritance
    pub features: Option<Arc<Features>>,
    pub tool_overrides: Option<Arc<ToolOverrides>>,
    pub parent_tool_filter: Option<ToolFilter>,

    // child construction mode (PR #5 scaffolding)
    pub spawn_mode: SpawnMode,
}

pub enum SpawnMode {
    Fresh,                                              // default
    Fork {
        rendered_system_prompt: Vec<u8>,                // byte-faithful
        parent_messages: Vec<serde_json::Value>,        // deep clone
        inherit_tool_pool: bool,                        // cache-identical
    },
}
```

The runner consumes `SpawnMode::Fork` by:
1. Skipping `AgentDefinition.initial_prompt` rendering — use the inherited bytes verbatim.
2. Rewriting `tool_result` blocks in `parent_messages` to `coco_subagent::FORK_PLACEHOLDER` (pure-logic path: `coco_subagent::build_fork_context`).
3. Rejecting recursion via `coco_subagent::is_in_fork_child`.
4. Bubbling permission prompts to the parent terminal.
5. Forcing `run_in_background = true` (TS `forceAsync`).

## What landed on `feat/agentteam`

### PR #1 — `core/subagent` becomes the canonical catalog (merged)

- Migrated `app/cli` agent-discovery callsites to `coco_subagent::AgentDefinitionStore`.
- `with_agent_dirs(Vec<PathBuf>)` → `with_agent_search_paths(AgentSearchPaths)` on `CliInitializeBootstrap`.
- Moved `agent_fork.rs` → `core/subagent/src/fork.rs` and `agent_resume.rs` → `core/subagent/src/transcript.rs` (byte-faithful TS-mirror preserved).
- Deleted 5 legacy `core/tools/src/tools/agent_*.rs` modules + tests (~2,373 LoC of dead/duplicate loaders).
- Added `/agents` slash command (`commands/src/handlers/agents.rs`) with `list` / `show <name>` / `paths` / `validate` / `reload` subcommands consuming the catalog.

### PR #4 — coordinator-mode pure logic (merged)

- `core/subagent/src/coordinator_mode.rs`: `is_coordinator_mode`, `is_fork_subagent_active`, `worker_tool_pool` (sorted, dedup, internal-tools excluded), `coordinator_user_context` (TS key `workerToolsContext`), `coordinator_system_prompt` (byte-faithful template), `render_task_notification`, `session_mode_switch_action` (pure decision — caller owns env mutation).
- Added `EnvKey::CocoCoordinatorMode` and `EnvKey::CocoForkSubagent` to `coco_config`.
- Migrated `core/subagent::fork::is_fork_enabled` off `std::env::var` to `coco_config::env::is_env_truthy(EnvKey::CocoForkSubagent)` per project rule.

### PR #5 — feature promotion + spawn DTO (merged)

- `Feature::AgentTeams` → `Stage::Experimental` (`/experimental` menu).
- Added `SpawnMode::{Fresh, Fork{…}}` to `core/tool-runtime::agent_handle` and re-exported from crate root.
- `AgentSpawnRequest::spawn_mode` field (default `Fresh`) — stable contract for the future runner wiring.
- `TaskType` already enumerates the three TS task kinds — no change needed at the type level.

### PR #3 — `coco-coordinator` extraction (merged)

All 21 swarm modules previously hosted in `app/state` are now in the
new `coco-coordinator` crate at `coordinator/`. The crate sits at the
root layer (next to `tasks`, `memory`, `commands`) and owns:

```
coordinator/src/
├── lib.rs
├── constants.rs              ← was app/state/swarm_constants.rs
├── identity.rs               ← was app/state/swarm_identity.rs
├── discovery.rs              ← was app/state/swarm_discovery.rs
├── prompt.rs                 ← was app/state/swarm_prompt.rs
├── reconnect.rs              ← was app/state/swarm_reconnect.rs
├── teammate.rs               ← was app/state/swarm_teammate.rs
├── config.rs                 ← was app/state/swarm_config.rs
├── worktree.rs               ← was app/state/agent_worktree.rs
├── types.rs                  ← was app/state/swarm.rs
├── team_file.rs              ← was app/state/swarm_file_io.rs
├── mailbox/mod.rs            ← was app/state/swarm_mailbox.rs (1007 LoC, internal split deferred)
├── task.rs                   ← was app/state/swarm_task.rs
├── spawn.rs                  ← was app/state/swarm_spawn_utils.rs
├── runner.rs                 ← was app/state/swarm_runner.rs
├── runner_loop.rs            ← was app/state/swarm_runner_loop.rs
├── agent_handle.rs           ← was app/state/swarm_agent_handle.rs (impl AgentHandle)
├── inprocess_backend.rs      ← extracted from app/state/swarm_backend.rs (`InProcessBackend` impl)
└── pane/
    ├── mod.rs                ← was app/state/swarm_backend.rs (trait + registry + detection)
    ├── tmux.rs               ← was app/state/swarm_backend_tmux.rs
    ├── iterm2.rs             ← was app/state/swarm_backend_iterm2.rs
    ├── pane_executor.rs      ← was app/state/swarm_backend_pane.rs
    ├── layout.rs             ← was app/state/swarm_layout.rs
    └── it2_setup.rs          ← was app/state/swarm_it2_setup.rs
```

**Layering invariant**: `coco-coordinator` does NOT depend on `coco-state`
(would create a cycle). The shared data shapes that both layers read or
write — `TeamContext`, `TeammateEntry`, `StandaloneAgentContext`,
`SubAgentState`, `SubAgentStatus`, `IdleReason`, `TaskEntry`, the mailbox
protocol enums (`TeammateProtocolMessage`, `TeammateProtocolContent`) —
were lifted to `coco_types::agent_ipc` so neither side needs the other
just for the types.

**`app/state` after extraction**: down from 21 swarm modules to zero.
Just `lib.rs` (471 LoC of `AppState` field tree) and `lib.test.rs`.

**Migrated consumers**: `app/cli/src/{session_runtime,tui_runner}.rs`
import `coco_coordinator::mailbox::SwarmMailboxHandle` directly. The
historical `coco_state::swarm_*` re-exports were dropped in step 10 (no
remaining external consumers). `app/state/Cargo.toml` no longer has the
`coco-coordinator` dep.

### PR #3 step 7 — `SpawnMode::Fork` dispatch end-to-end (merged)

- `AgentTool::execute` (`core/tools/src/tools/agent.rs`) reads
  `ctx.messages` and `ctx.rendered_system_prompt`, runs the
  `is_fork_subagent_active(features, is_non_interactive)` gate, runs the
  `is_in_fork_child` recursion guard, and builds
  `SpawnMode::Fork { rendered_system_prompt, parent_messages, inherit_tool_pool }`.
  Forces `run_in_background = true` for fork mode (TS `forceAsync`).
- `SwarmAgentHandle::spawn_subagent` (`coordinator/src/agent_handle.rs`)
  consumes the variant: threads the parent's pre-rendered system prompt
  bytes verbatim into `AgentQueryConfig.system_prompt`, calls
  `coco_subagent::build_fork_context` to rewrite `tool_result` blocks
  with `FORK_PLACEHOLDER`, sets `preserve_tool_use_results = true`, and
  populates `fork_context_messages`.

### PR #3 step 8 — coordinator-mode worker pool + task-notification (merged)

- `SwarmAgentHandle::spawn_subagent` applies
  `coco_subagent::worker_tool_pool(simple_mode)` to subagent
  `allowed_tools` when the leader is in coordinator mode (gated by
  `coco_subagent::is_coordinator_mode(&features)` + `COCO_SIMPLE`).
- `runner_loop::run_in_process_teammate` cleanup path renders a
  `<task-notification>` XML envelope via `coco_subagent::render_task_notification`
  on worker terminate (when coordinator mode is active) and pushes it to
  the leader's mailbox as a teammate text message. TS `coordinatorMode.ts:130-152`.
- **Still open** for step 8: the system-prompt swap at session bootstrap
  (the leader uses `coordinator_system_prompt(simple_mode)` instead of
  the default). Lives in `app/query` / `app/cli` (outside coordinator
  scope). Pure helper is ready in `coco_subagent::coordinator_system_prompt`.

### PR #3 step 9 — `agentMemorySnapshot` API contract (merged)

`coco_memory::team_sync` exposes the four-function API the coordinator's
spawn / terminate path is expected to call:

- `snapshot_dir_for_agent(project_root, agent_type) -> PathBuf`
- `check_agent_memory_snapshot(...) -> SnapshotAction { None | Initialize | PromptUpdate }`
- `initialize_from_snapshot(...) -> Result<()>`
- `persist_snapshot_from_local(...) -> Result<()>`

Function bodies are **stubs** with `TODO(PR3-step9)` markers — porting
the TS `agentMemorySnapshot.ts` file IO (snapshot.json + .snapshot-synced.json
metadata, copySnapshotToLocal, updateSnapshotFromLocal) is a follow-up
PR. The contract is concrete so the coordinator's spawn path can wire
the calls today and the IO can fill in incrementally.

## PR #11 — TS-alignment behavior fixes (merged)

After the audit identified ~35% behavior gap between the extracted
crate and TS reference, this batch closed the highest-leverage P0/P1
items without faking the deeper architecture work:

### Coordinator-mode runtime is now wired

- `app/query/src/engine_prompt.rs` consults
  `coco_subagent::is_coordinator_mode(&features)` and swaps to
  `coordinator_system_prompt(simple_mode)` when active. The leader
  actually becomes a coordinator when `COCO_COORDINATOR_MODE=1` instead
  of silently using the regular prompt.
- `coco-query` now depends on `coco-subagent` (foundation crate, no
  cycle).

### Task-notification XML round-trips end-to-end

- Send-side already wired in step 8.
- New: `coco_subagent::parse_task_notification(text)` +
  `looks_like_task_notification(text)` pure-logic helpers. Returns
  owned `ParsedTaskNotification`. Receive-side consumers in
  `app/query` / `coordinator` can now structurally extract task-id /
  status / summary / result / usage from inbound mailbox messages
  instead of relying on the model's pattern-match.

### Runner-loop critical fixes

- **Shutdown enforcement**: `runner_loop` tracks `handling_shutdown`;
  after the model's reply to a `ShutdownRequest` finalises, the loop
  exits cleanly so cleanup (idle notification + pane teardown +
  coordinator task-notification) runs. Previously the flag was set but
  never honoured.
- **Plan-approval await**: new `wait_for_plan_approval(identity,
  cancelled, request_id)` helper polls the teammate's inbox for a
  `PlanApprovalResponse` matching the request id and respects
  cancellation. Caller integration (block before first
  implementation turn when `plan_mode_required`) is the next step.
- **Compaction comment honesty**: the sliding-window truncation is
  flagged `TODO(coordinator-compaction)` with the correct rationale
  for the deferral (cycle through `coco-compact` requires the engine
  bridge to own triggering).

### Color persistence

- `agent_handle::spawn_teammate` now calls
  `crate::pane::layout::assign_teammate_color(&"name@team")` instead
  of hashing the name. Colors stay stable across spawns within a
  session (TS parity with `teammateLayoutManager.ts:assignTeammateColor`).
- `agent_handle::send_message` populates the outgoing
  `TeammateMessage.color` field from
  `crate::pane::layout::get_teammate_color(&from)` so peers can render
  the source consistently. Previously hardcoded `None`.

### AgentTool input schema parity

Six fields added to both `AgentSpawnRequest` (DTO in tool-runtime) and
the AgentTool input schema (core/tools): `effort`, `use_exact_tools`,
`mcp_servers`, `disallowed_tools`, `max_turns`, `initial_prompt`. TS
parity with `AgentTool.tsx:82-125`. Each field is optional with a
sensible default and skipped at the JSON boundary when unset.

### Handoff classifier (security)

New `coco_subagent::handoff` module — pure-logic prompt builders +
parser for the 2-stage LLM safety classifier from TS
`agentToolUtils.ts:classifyHandoffIfNeeded`:

- `should_classify(agent_type, total_tool_use_count)` — read-only
  agents and zero-tool turns skip
- `stage1_prompts(agent_type, transcript, count) -> (system, user)`
- `stage2_prompts(stage1_verdict, transcript) -> (system, user)`
- `parse_classifier_response(text) -> HandoffClassification`
- `render_block_message(verdict) -> Option<String>` — wraps `Blocked`
  in a `SECURITY: …` payload for `<tool_use_error>`
- `build_transcript_summary(messages)` — strips tool-result bodies so
  the classifier sees actions, not data, and the prompt stays bounded

The actual LLM call lives in the runtime layer
(`coco_tool_runtime::SideQueryHandle`); the coordinator's
`agent_handle.rs` after-spawn hook is the intended wiring site (still
to land — uses these pure helpers when it does).

### Team-memory secret guard (security)

New `coco_secret_redact::scan_secrets(input) -> Vec<SecretMatch>`
detection API alongside the existing `redact_secrets(input)` redaction
API. Reuses the same regex set (Anthropic / OpenAI / GitHub / Slack /
AWS / Bearer) but returns labelled match positions for the
**block-don't-redact** flow.

New `coco_memory::team_sync::assert_no_secrets_in_team_memory(content)`
returns `SecretGuardOutcome::{Safe, Blocked { rule_labels }}`. Mirrors
TS `services/teamMemorySync/teamMemSecretGuard.ts`. Caller in the
team-memory write path (still to wire — `memory::team_sync` callers in
the eventual sync state machine) gates on this and rejects writes
with the labelled reason. Matched bytes are intentionally NOT in the
reject reason so it can be safely logged.

## Still-open items (post Phase D + E + post-D cleanup + P0 + P1)

| Item | Status | Notes |
|---|---|---|
| Semantic compaction in runner_loop | open | Sliding-window safety valve in place; full compaction from coordinator deferred |
| Permission-bridge poll loop on workers | **closed** | Workers inherit the leader's `ToolPermissionBridge` via `wire_engine` (SDK / TUI); `MailboxPermissionBridge` covers cross-process pane. The orphaned in-process mpsc circuit was deleted in the post-D cleanup pass after deep review surfaced it as dead code |
| Leader-side TUI permission overlay | **closed (P0)** | `TuiPermissionBridge` installed on `SessionRuntime` for TUI mode; emits `TuiOnlyEvent::ApprovalRequired` and resolves via `UserCommand::ApprovalResponse` |
| Plan-approval block-and-await caller | partial | `wait_for_plan_approval` helper exists; runner gates on `plan_mode_required` |
| Sandbox permission flow | blocked | `SandboxApprovalBridge` trait + `SdkSandboxApprovalBridge` impl exist on the producer side, but `PermissionChecker::new` has zero production callers — the sandbox layer isn't invoked from Bash / file tools today. A TUI UI consumer (paralleling the new `TuiPermissionBridge`) is straightforward to write but would never fire until the sandbox is wired into the tool execution path. That wiring is a sandbox-bootstrap concern, not an agentteam concern; tracked here for visibility only |
| `forkedAgent.ts` post-turn cache slot | **closed (D8 + D1/D2)** | `CacheSafeParams` slot + `ForkDispatcher` trait + production CLI impl; `/btw` and `promptSuggestion` consume it |
| Memory snapshot file IO bodies | **closed (E1)** | All snapshot IO + secret guard in `coco_memory::team_sync` |
| Team-memory sync watcher | **out of scope (C2)** | Server-coupled to Anthropic backend; cross-machine sync delegated to git/S3/etc |
| TUI components for coordinator/teammate | **closed (E4)** | `CoordinatorPanel`, `TeammateViewHeader`, `SubagentPanel`, `TeammateSpinner` are wired through `presentation::activity`, `surface::viewport`, and focused widgets |
| AgentSummary — one-shot at completion | **closed (E3)** | Via `SideQueryHandle`; populates `SubAgentState.last_message` |
| AgentSummary — periodic (every 30 s) | **closed (bg path)** | The live `TaskOutputDelta` streaming infrastructure unblocked this — the bg AgentTool path now spawns a 30s timer per spawn that reads `AgentTaskRegistry::read_output(task_id)`, hands the recent tail to `side_query` for summarization, and writes the result onto `SubAgentState.last_message` so the panel updates while the agent is running. Cancellation observes the same token as the engine driver. Sync (foreground) AgentTool spawns block the parent loop and don't need periodic summarization — the one-shot at-completion summary (E3) covers them |
| `<task-notification>` ingestion call site | **closed** | `runner_loop` emits via `render_task_notification` on worker terminate when coordinator mode is active |
| Handoff classifier orchestration call site | **closed** | `SwarmAgentHandle::classify_handoff_if_needed` runs after every subagent completion |
| AgentTool reads `AgentDefinition.background` | **closed (D5 + P1')** | OR'd into `run_in_background` along with `is_coordinator_mode` and gated by `COCO_BACKGROUND_TASKS_DISABLE`; in-process-teammate guard throws on conflict |
| AgentTool background task registration | **closed** | `coco_cli::task_runtime::TaskRuntime` implements both `coco_tool_runtime::TaskHandle` (read/control) and `AgentTaskRegistry` (registration). The same `Arc` flows into the engine via `wire_engine` and into `SwarmAgentHandle::set_task_registry`, so AgentTool's bg path registers spawns through the same `TaskManager` that `TaskGet` / `TaskOutput` / `TaskStop` read from. The bg dispatch returns the `task_id` as the response's `agent_id` so the model can address the spawn directly. `kill_task` flips a per-task `CancellationToken` that the spawn's `tokio::select!` observes |
| AgentTool background — live `TaskOutputDelta` streaming | **closed** | New `AgentQueryConfig.event_tx` field carries an optional `mpsc::Sender<CoreEvent>` from caller through to the adapter. The bg AgentTool path allocates a per-task channel, drains `Stream::TextDelta` events into the task's output buffer via `AgentTaskRegistry::append_output`, so `TaskOutput` returns mid-flight text immediately rather than waiting for completion. `mark_completed` no longer double-appends `response_text` since the deltas already streamed |
| AgentTool background — `--resume` persistence + model trigger | **closed (TS-aligned, model-driven)** | TS-faithful resume via per-agent JSONL transcript + sidecar metadata. New `AgentTranscriptStore` trait in `coco-tool-runtime` (registration side decoupled from `coco-session` to respect L5-app vs L5-root layering); `SessionAgentTranscriptStore` impl in `app/cli` wraps `coco_session::TranscriptStore`. Storage layout matches TS `<sessions_dir>/<session_id>/subagents/agent-<id>.{jsonl,meta.json}`. Bg path writes meta on register + transcript on completion. **Model-driven trigger** (TS `SendMessageTool.ts:822-872` parity): `SendMessageTool::execute` checks `ctx.task_handle.get_task_status(to)` first; when the target's status is terminal (`Completed` / `Failed` / `Killed`), it dispatches `ctx.agent.resume_agent(to, message, session_id)` instead of routing through the team mailbox. The model just keeps using `SendMessage` and resume is automatic on stopped agents. `resume_agent` reads transcript + meta, filters via `coco_subagent::filter_transcript`, and dispatches a new spawn with `SpawnMode::Fork` carrying the prior history as `parent_messages`. Earlier "engine state isn't recoverable" framing was wrong — TS doesn't recover the streaming connection either, only the conversation history, which IS recoverable |
| `/clear` clears post-turn slots | **closed (D4)** | `ToolAppState::default()` reset clears `prompt_suggestion`; cache slot is engine-local and rebuilds per turn |
| `runner_loop.rs` 800-LoC cap | **closed (P1)** | Split into `runner_loop_mailbox_permission` + `runner_loop_wait` + `runner_loop_notify`; main file at 761 LoC |
| `core/tools/src/tools/agent.rs` 800-LoC cap | **closed** | Old 1011-LoC monolith split into per-tool modules under `tools/agent/` (`agent_tool.rs`, `skill_tool.rs`, `send_message_tool.rs`, `team_tools.rs`); largest module 591 LoC |
| `coordinator/src/agent_handle_spawn.rs` 800-LoC cap | **closed** | Old 854-LoC file split into `agent_handle/{mod,spawn,handoff,resume}.rs`; largest module 585 LoC |
| Per-`AgentTypeId` color cache | **closed** | `pane::layout::assign_agent_type_color` mirrors TS `agentColorManager.ts`; populated by `spawn_subagent` so `SubagentPanel` renders all `Explore` spawns in the same color |
| Per-agent MCP server validation | **closed (fail-fast)** | `AgentTool::execute` rejects spawns whose declared `mcp_servers` aren't connected via `McpHandle::connected_servers`. Fail-fast (not a 30 s poll) since coco-rs settles MCP boot at session start; the model retries with corrected `mcp_servers` |
| `#[non_exhaustive]` on cross-crate enums | **closed** | Added to `HandoffClassification`, `SessionMode`, `SessionModeSwitch`, `TaskNotificationStatus` — were public crate-root re-exports without the attribute |
| Resume mistakenly used `SpawnMode::Fork` (rewrites tool_result to `FORK_PLACEHOLDER`) | **closed** | New `SpawnMode::Resume { parent_messages }` variant. `spawn.rs` keeps `tool_result` blocks verbatim under Resume so the resumed child can continue the conversation; system_prompt is left empty so the engine rebuilds from `definition`. Mirrors TS `resumeAgent.ts:resumeAgentBackground` for non-fork agent types |
| Dangling `SubAgentState` entry on validation failure | **closed** | `spawn_subagent` now commits the agent-state push only after worktree + execution-engine gates pass; earlier code left a Pending entry visible to `query_agent_status` / `SubagentPanel` whenever validation failed |
| `SendMessageTool` auto-resume with empty `session_id` | **closed** | Reject upfront with a clear "parent session id is unavailable" error instead of forwarding to `resume_agent` and surfacing a confusing inner "no metadata" failure |
| `render_block_message` dangling em-dash on empty reason | **closed** | Bare `"BLOCKED"` (no `:`) used to render `"SECURITY: subagent output withheld — "`; now collapses to `"unspecified safety concern"` so the payload is a self-contained sentence |
| Coordinator never sees worker's first-turn errors | **closed** | `runner_loop` query-error path was an early `return` that ran `on_teammate_stop` but skipped the coordinator-mode `<task-notification>` send. Refactored to stash the error in `run_error: Option<String>` and `break` to the unified cleanup, which now emits `TaskNotificationStatus::Failed` (with the error in `result`) when `is_coordinator_mode`, and `Completed` otherwise |
| Mailbox lock retries / backoff envelope drifted from TS | **closed** | Aligned `mailbox/lock.rs` to TS `proper-lockfile` config (`retries: 10, minTimeout: 5, maxTimeout: 100`): 10 retries, exponential 5 → 100 ms cap with `[0.5×, 1.5×)` jitter (TS achieves equivalent via `randomize: true`). Earlier 30 retries × 50 ms cap × jitter could block ~1.5 s under contention vs TS's ~1 s envelope |
| Pink color used wrong tmux palette index | **closed** | `agent_color_to_tmux(Pink)` returned `colour213`; TS `TmuxBackend.ts:67` uses `colour205`. Fixed plus updated the parity test |
| Pane-active-border-style not set on tmux 3.2+ | **closed** | `TmuxBackend::set_pane_border_color` only ran one `select-pane -P "pane-border-style=..."`. TS does a 3-step sequence: `select-pane -P bg=default,fg=...`, `set-option -p ... pane-border-style ...`, `set-option -p ... pane-active-border-style ...` so the colour stays applied whether the pane is active or inactive. Rust now mirrors the full sequence |
| Dead `pending_user_messages: Vec<String>` scaffold | **closed** | Local Vec was passed into `wait_for_next_prompt_or_shutdown` but never written to anywhere — TS reads from `task.pendingUserMessages` populated by transcript-view UI, which coco-rs doesn't yet have. Removed the parameter + branch; `wait_for_next_prompt_or_shutdown` doc-comment now points to the future port (per-`agent_id` `mpsc::UnboundedReceiver` registry on `InProcessAgentRunner`) so when the TUI lands the seam is obvious |

### Feature gate promotion

`coco_types::features::Stage` has three variants — `UnderDevelopment`,
`Experimental`, `Stable`. There is no Beta tier. Promotion options for
`Feature::AgentTeams`:

- **Stay `Experimental`** with refreshed copy. Recommended until the
  background task registration item closes (the only remaining
  user-visible correctness gap; sandbox UI is structurally unreachable
  today, so it doesn't block).
- **`Stable`**: requires background task registration + sandbox
  producer-side wiring + a soak window. High bar, deliberately so —
  Stable means it's hidden from `/experimental` and shipped on by
  default.

Earlier doc revisions said "Experimental → Beta"; that's invalid for
this enum and was corrected here.

## Architecture invariants (verified)

These hold at HEAD and are checked by `just check` + the tests:

- **`coco-coordinator` does not depend on `coco-cli`.** The
  registration seam between them is the `AgentTaskRegistry` trait
  in `coco-tool-runtime`, which both crates can import without a
  cycle. Verified by `grep coco-cli coco-rs/coordinator/Cargo.toml`
  returning empty.
- **No new upward dependencies introduced in `coco-tasks`.** The
  crate's dependency list at HEAD matches the original extraction
  commit (`53d12459e`); only two pure accessor methods
  (`set_tool_use_id`, `mark_notified`) were added to
  `TaskManager`. The crate is L4 (depends on coco-config /
  coco-system-reminder / coco-tool-runtime / coco-types) — never
  was a leaf, and that hasn't changed.
- **Companion `*.test.rs` pattern preserved on every new module.**
  `task_runtime.rs` + `.test.rs`, `tui_permission_bridge.rs` +
  `.test.rs`, `runner_loop_*.rs` (covered by `runner_loop.test.rs`).
- **No new wire enums.** `AgentTaskRegistry` is a trait; the new
  `AgentQueryConfig.event_tx` field is a `#[serde(skip)]` runtime-
  only handle, not a wire-tagged enum. The project rule on
  `#[non_exhaustive]` only applies to wire enums and isn't
  triggered by this work.
- **Module size cap (800 LoC, excl. tests) for files in this
  crate's scope.** `coordinator/runner_loop.rs` is 761 after the
  P1 split; `coordinator/agent_handle_spawn.rs` is 700 after the
  bg dispatch + periodic-summary additions. Pre-existing
  app-layer modules (`engine.rs` 1596, `tui_runner.rs` 1256,
  `main.rs` 1221, `session_runtime.rs` 869) remain over the cap;
  this work didn't move any of them across the threshold.

## Skipped follow-up candidates (analyzed, not implemented)

After the disk-spill + AgentSummary-gate work, three additional
candidates were evaluated. Two were rejected after analysis as
non-actionable; one is open infrastructure work outside agentteam
scope.

- **`initTaskOutputAsSymlink` parity (deliberate divergence).** TS
  symlinks the per-task `.output` file to the agent's per-spawn
  JSONL transcript so `TaskOutput` reads progress directly from
  the conversation log. coco-rs maintains them as **separate
  streams** instead: `<task_id>.output` carries raw text deltas
  for the model-facing `TaskOutput` tool, while
  `<sessions_dir>/<session_id>/subagents/agent-<id>.jsonl` carries
  full JSON message entries for `agent/resume`. Symlinking would
  force `TaskOutput` to return JSON-stringified entries instead of
  clean text — a UX regression for the model. The two-stream
  approach preserves `TaskOutput`'s text contract while still
  providing the JSONL transcript needed for resume. Resume now
  works (see "Background AgentTool resume" entry); the symlink is
  the only TS feature deliberately not adopted.

- **`getTaskOutput` tail read with omitted-bytes header
  (closed).** TS `read_output` returns the LAST 8 MiB of the file,
  not the FIRST, prepending `[N KB of earlier output omitted]\n`
  when content exceeds the cap. The model sees recent activity
  rather than cold-start text. coco-rs now mirrors via
  `DiskTaskOutput::read_tail(max_bytes)`; the `AgentTaskRegistry::
  read_output` impl in `TaskRuntime` calls into it. 3 tests cover
  under-cap / over-cap / empty paths.

- **Sandbox UI consumer (still blocked).**
  `PermissionChecker::new` has zero production callers; the
  sandbox layer isn't invoked from Bash / file tools today, so a
  TUI UI consumer (paralleling `TuiPermissionBridge`) would never
  fire. Building it would be dead code. This is sandbox-bootstrap
  work outside agentteam scope.

## Operational concerns (closed)

Three runtime concerns surfaced during deep review of the
TaskRuntime + periodic-summary work; all closed:

- **Timer-leak window on natural completion.** The 30 s periodic-
  summary timer races a tokio `select!` between the per-task
  `CancellationToken` and a sleeping ticker. Engine completion
  alone wouldn't fire the token — the timer would wait up to 30 s
  for the next `is_terminal` check before exiting. Fixed by
  having `TaskRuntime::mark_completed` and `mark_failed` BOTH
  cancel the token in addition to flipping `TaskStatus`. Tests:
  `mark_completed_cancels_per_task_token`,
  `mark_failed_cancels_per_task_token`.
- **Unbounded output buffer.** `Arc<Mutex<String>>` per task grew
  without retention. Capped at 8 MiB; head-truncation drops
  oldest bytes and prepends `OUTPUT_TRUNCATION_NOTICE` so
  consumers know they're reading a tail. Char-boundary aware so
  truncation never splits a UTF-8 codepoint. Tests:
  `append_output_truncates_when_over_cap`,
  `append_output_preserves_utf8_boundaries`.
- **Periodic-summary LLM cost at saturation.** With
  `MAX_IN_PROCESS_AGENTS = 16` × 30 s ticks, a fully-saturated
  coordinator burns up to 32 side-query calls / minute on
  summarization. No throttle today; matches TS cadence. Tracked
  but not gated — heaviest workloads can disable via
  `COCO_BACKGROUND_TASKS_DISABLE` (which collapses the bg path
  entirely, the only way to fully cut the cost).

## Deferred design decisions

### `lastCacheSafeParams` post-turn cache slot — implemented (D8-impl)

The scaffold is in place. Post-turn fork features (`/btw`,
`promptSuggestion`, `postTurnSummary`) don't exist in coco-rs yet —
when any of them ship the cache slot is ready to read.

**What landed**:

- `coco_types::CacheSafeParams` DTO with the cache-key-affecting
  fields (`rendered_system_prompt`, `model_id`,
  `fork_context_messages`). `coco_messages::Message` is excluded from
  the type itself; messages cross the layer as `Vec<serde_json::Value>`,
  same shape as `AgentQueryConfig.fork_context_messages`. Tools and
  thinking config are intentionally NOT in the DTO — they invalidate
  the cache regardless of the slot, and the live `ToolUseContext` is
  not serialisable.
- `QueryEngine.last_cache_safe_params: Arc<RwLock<Option<...>>>` slot,
  with `last_cache_safe_params() -> Option<CacheSafeParams>` accessor,
  `cache_safe_params_handle()` for observers, and
  `clear_cache_safe_params()` for `/clear` regen paths.
- Internal `save_post_turn_cache_params(&MessageHistory)` helper
  invoked from BOTH exit sites in `run_session_loop`: the
  tool-execution path (via `finalize_turn_post_tools`) and the
  text-only end-of-turn early return at `engine.rs:1407`. Empty
  history skips. Mirrors TS `handleStopHooks` calling
  `saveCacheSafeParams` after every successful turn.

**Limitations vs TS** (intentional, drop-in upgradable):

- Tools/thinking-config not stored. A post-turn fork that calls a
  different tool list will still cache-miss; that's true in TS too
  unless the caller explicitly threads the parent's `toolUseContext`.
  When a future fork caller wants tool parity it can read the engine's
  current `Arc<ToolRegistry>` directly.
- No `userContext` / `systemContext` (TS) — those represent
  attachment-injected user/system context which coco-rs threads
  through `coco_messages::Message` already.

**Revisit when**: a post-turn fork feature (`/btw`, etc.) lands. The
caller reads `engine.last_cache_safe_params().await` at fork time,
threads `fork_context_messages` into a fresh
`AgentQueryConfig`, and pins `model_id`. Cache parity follows.

### Sandbox interactive permission bridge — implemented (D7-impl)

`coco-sandbox` now ships an opt-in async approval bridge:

- `bridge::SandboxApprovalBridge` trait + `SandboxApprovalRequest` /
  `SandboxApprovalDecision` / `SandboxOperation` DTOs
  (`exec/sandbox/src/bridge.rs`).
- `PermissionChecker::with_approval_bridge` /
  `set_approval_bridge` / `has_approval_bridge` setters.
- Async variants `check_path_async` / `check_network_async`. When no
  bridge is installed they're identical to the sync versions; with a
  bridge installed they consult it on the deny path and rewrite
  `Err → Ok(())` on `Approved`. Static `check_path` / `check_network`
  keep their fail-closed semantics so existing callers don't change
  behavior.
- `NoOpSandboxApprovalBridge` always rejects — useful as the explicit
  default when "the seam is wired but no UI is hooked up yet" needs to
  be visible.

The trait deliberately lives in `coco-sandbox` rather than reusing
`coco_tool_runtime::ToolPermissionBridge`: the sandbox is a *physical*
enforcement layer, while `ToolPermissionBridge` is a *semantic*
permission surface. Adapters can fan a single approval UI to both —
that adapter belongs in `coco-coordinator` or the CLI, not in
`coco-sandbox`.

Approved operations log `decision = "approved_by_bridge"` on the same
`sandbox.permission_check` / `sandbox.network_check` tracing fields
existing emissions already use — auditable in the same telemetry
stream as immediate denies.

### `ModelRole::CoordinatorWorker` — not adding

**Question**: should coordinator-mode workers (spawned by AgentTool when
the leader's session has `COCO_COORDINATOR_MODE=1`) route through a
dedicated `ModelRole::CoordinatorWorker` so the user can map workers to
a cheaper / faster model than the generic `ModelRole::Subagent`?

**Decision**: **no, stay on `ModelRole::Subagent`** for now.

**Rationale**:

- TS doesn't differentiate. Coordinator workers in `coordinator/coordinatorMode.ts`
  inherit the regular subagent model selection chain (`AgentTool model
  param > AgentDefinition.model > 'inherit'`) — no parallel role
  hierarchy. Adding the variant would be a Rust-only superior path,
  not a parity feature.
- The `coco_subagent::worker_tool_pool(simple_mode)` filter already
  gives coordinators the right *tool* surface (excluded
  `INTERNAL_WORKER_TOOLS`); efficiency comes from the reduced tool
  count more than from a different model.
- Users who want a cheaper coordinator-worker model already have the
  knob: declare `model: haiku` (or `model_role: fast`) in the worker
  agent's `.md` frontmatter. T7 makes that path real — the AgentTool
  spawn boundary now reads `AgentDefinition.model_role` via
  `coco_subagent::resolve_subagent_selection`.
- Adding a new `ModelRole` variant ripples through `coco_config::ModelRoles`
  (resolution + fallback chain), `RuntimeConfig::resolve_model_roles`
  (Main-walk fallback), settings.json schema (`models.coordinator_worker`),
  TUI role-picker UI, and OTel telemetry attributes. That's a lot of
  surface for marginal gain.

**Revisit when**: a real workload shows that coordinator workers benefit
from a different model than other subagents in a way `model_role:` on
the per-agent definition can't express — e.g. mixed-fleet scenarios
where the same `AgentDefinition` should pick a different model in
coordinator vs non-coordinator contexts. Until then, the per-`.md`
declaration is the right mechanism.

## (Historical, kept for archive) Original PR #3 deferral

**Why originally deferred**: `app/state/swarm.rs` imports `AgentMessage`,
`SubAgentState`, `SubAgentStatus`, `AgentMessageContent` from
`app/state/lib.rs`. Cleanly extracting requires also splitting
AppState's intermixed types — a multi-session refactor.

**What needs to happen before PR #3**:

1. Decide where `AgentMessage` / `SubAgentState` / `SubAgentStatus` /
   `AgentMessageContent` live. Two options:
   - Move to `core/messages` (they describe message content) — preferred.
   - Move to `coco-types` (foundational data) — fine if they're small.
2. `swarm_mailbox.rs` (1007 LoC) breaks the project 800-LoC module
   rule. Split into `mailbox/io.rs` + `mailbox/lock.rs` as part of the
   move.
3. Remove the only two external consumers of `coco_state::swarm_mailbox::SwarmMailboxHandle`
   (`app/cli/src/{session_runtime,tui_runner}.rs`) by importing from
   `coco_coordinator::mailbox`.

**Module layout for the new crate**:

```
coordinator/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── runner.rs           ← swarm_runner.rs
    ├── runner_loop.rs      ← swarm_runner_loop.rs
    ├── mailbox/
    │   ├── io.rs           ← swarm_mailbox.rs (read/write/path)
    │   └── lock.rs         ← swarm_mailbox.rs (fs2 + retry/jitter)
    ├── team_file.rs        ← swarm_file_io.rs
    ├── discovery.rs        ← swarm_discovery.rs
    ├── identity.rs         ← swarm_identity.rs
    ├── agent_handle.rs     ← swarm_agent_handle.rs (impl AgentHandle)
    ├── task.rs             ← swarm_task.rs (InProcessTeammateTaskState)
    ├── reconnect.rs        ← swarm_reconnect.rs
    ├── spawn.rs            ← swarm_spawn_utils.rs + SpawnMode wiring
    ├── prompt.rs           ← swarm_prompt.rs
    ├── teammate.rs         ← swarm_teammate.rs
    ├── coordinator_mode_runtime.rs   ← env mutation + bootstrap
    └── pane/
        ├── mod.rs          ← swarm_backend.rs (trait + registry)
        ├── tmux.rs         ← swarm_backend_tmux.rs
        ├── iterm2.rs       ← swarm_backend_iterm2.rs
        ├── pane_executor.rs← swarm_backend_pane.rs
        ├── layout.rs       ← swarm_layout.rs
        └── it2_setup.rs    ← swarm_it2_setup.rs
```

## Cross-references

- TS taxonomy and IPC details: `docs/coco-rs/subagent-refactor-plan.md` (Phase 0–10), `agent-loop-refactor-plan.md` (Phases 1–9, invariants I1–I14). Both predate this doc; refer to them for the scaffolding rationale and TS line citations, but treat the *layering decisions* in this doc as authoritative.
- TS Anthropic-only features explicitly skipped in coco-rs: CCR `RemoteAgentTask`, GrowthBook gates (`tengu_*`), `feature('ULTRAPLAN')`, `feature('FORK_SUBAGENT')` (replaced by `COCO_FORK_SUBAGENT` env), `feature('COORDINATOR_MODE')` (replaced by `COCO_COORDINATOR_MODE` env).
- Project rules: root `CLAUDE.md` § "Multi-Provider Boundaries", "Type Safety", "Code Hygiene" (env vars must use `COCO_*` and live on `EnvKey`).
