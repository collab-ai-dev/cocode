# coco-tasks

Three distinct kinds of task state — deliberately separated because
their lifecycles differ:

| Module | Purpose |
|--------|---------|
| [`running`](src/running.rs) | Running background tasks (shell / agent / workflow). `TaskManager` emits `CoreEvent::Protocol(TaskStarted/Progress/Completed)` via an optional `with_event_sink(tx)` channel. |
| [`task_list`](src/task_list.rs) | Durable plan items stored on disk per task-list-id with `fs2` file locking + high-water-mark. Shared across a team. |
| [`todos`](src/todos.rs) | Ephemeral per-agent TodoWrite (V1) checklist. In-memory only — not persisted to disk. |

V1 (`TodoWrite`) and V2 (`Task*` tools) are gated by `is_todo_v2_enabled()` at the CLI layer — never both at once. Running-task state is orthogonal and always on.

## Key Types

### running
- `TaskManager` — `Arc<RwLock<HashMap<id, TaskStateBase>>>` + outputs map; optional `mpsc::Sender<CoreEvent>` sink for SDK NDJSON parity. `create` / `get` / `update_status` / `stop` / `set_output` / `get_output` / `list` / `remove_completed`.
- `TaskOutput` — `{stdout, stderr, exit_code}`.
- Event-emission coverage: `TaskStarted` on `create`, `TaskProgress` on non-terminal transitions, `TaskCompleted` on terminal (with `TaskCompletionStatus` mapping: Completed→Completed, Failed→Failed, Killed|Cancelled→Stopped).

### task_list
- `Task` — id, subject, description, active_form, owner, status, blocks, blockedBy, metadata.
- `TaskStatus` — 3 variants: `Pending`, `InProgress`, `Completed` (not the 6-variant `coco_types::TaskStatus` which is for running tasks).
- `TaskUpdate` — partial update struct; `metadata_merge` supports null-deletion.
- `TaskListStore` — disk-backed store. API: `create_task`, `get_task`, `list_tasks`, `update_task`, `delete_task`, `block_task`, `claim_task` (with optional agent-busy check), `unassign_teammate_tasks`, `should_nudge_verification_after_update`.
- `ClaimResult` — `Success` / `TaskNotFound` / `AlreadyClaimed` / `AlreadyResolved` / `Blocked` / `AgentBusy`.
- `resolve_task_list_id(teammate_team, leader_team, session_id)` — 5-level precedence.
- `TaskHookSink` trait — app layer implements this to fire `HookEventType::TaskCreated` / `::TaskCompleted`; avoids depending on `coco-hooks` from this crate.

### todos
- `TodoItem` — `{content, status, activeForm}`. **No id field** (positional identity).
- `TodoStore` — per-agent `HashMap<String, Vec<TodoItem>>` keyed by `agent_id.unwrap_or(session_id)`.
- `should_nudge_verification(&[&str])` — shared verification-nudge helper used by both V1 `TodoWrite` and V2 `TaskUpdate`.

### handle_impls
- `impl TaskListHandle for TaskListStore` — bridges the crate to `coco_tool_runtime::TaskListHandleRef`.
- `impl TodoListHandle for TodoStore` — bridges to `coco_tool_runtime::TodoListHandleRef`.

## Disk Layout (task_list)

```
{config_home}/tasks/{sanitize(list_id)}/
├── .lock                # fs2 file-lock sentinel
├── .highwatermark       # max task id ever assigned; prevents reuse
├── 1.json
├── 2.json
└── ...
```

Locking: list-level lock (`.lock`) for create / reset / agent-busy claim; per-task lock (`{id}.json`) for updates / claims. 30-retry backoff (5–100ms) gives ~2.6s budget on a 10-way race.
