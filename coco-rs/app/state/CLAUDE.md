# coco-state

Central application state tree (`Arc<RwLock<AppState>>`). Zustand-like shared
state touching ~80 fields; plus the **swarm** subsystem (21 modules) that ports
coordinator / team / multi-pane orchestration.

## Key Types

| Type | Purpose |
|------|---------|
| `AppState` | Model, session, agent, token tracking, tasks, MCP, plugins, notifications, speculation, bridge, team/inbox, coordinator, elicitation, sandbox, onboarding, bootstrap_data |
| `McpClientState`, `PluginState`, `NotificationState`, `TaskEntry`, `InboxEntry` | Field substructures |
| `PendingWorkerRequest`, `PendingSandboxRequest`, `WorkerSandboxPermissions` | Leader-side queues for multi-agent permission arbitration |
| `TeamContext`, `StandaloneAgentContext` | Active team membership / agent identity |
| `ElicitationEntry` | MCP elicitation queue |
| `SubAgentState` | Per-subagent status map |

## Swarm subsystem extracted (PR #3)

All 21 swarm modules previously hosted here moved to the dedicated
`coco-coordinator` crate at `coordinator/` — see
`docs/coco-rs/agentteam-architecture.md`. `app/state` now holds only the
`AppState` field tree; spawn lifecycle, mailbox IPC, terminal pane
backends, runner, runner-loop, identity / discovery / reconnect, and the
`AgentHandle` implementation all live in `coco-coordinator`.

`coco-state` does **not** depend on `coco-coordinator` (one-way layering:
`coco-coordinator → coco-state` is the only allowed direction). Inter-crate
data shapes that both layers read or write (`TeamContext`, `TeammateEntry`,
`SubAgentState`, `SubAgentStatus`, `IdleReason`, `TaskEntry`, the mailbox
protocol enums) live in `coco_types::agent_ipc` so neither side has to
import the other for the types alone.

## Conventions

- Always use `Arc<RwLock<AppState>>` — never clone the state itself.
- Field access is serde-stable (snake_case); `#[serde(default)]` on all optional fields.
- `onChangeAppState` side-effects (persisting settings, CCR notify) are **not** ported here — handled by the owning crate on mutation.
