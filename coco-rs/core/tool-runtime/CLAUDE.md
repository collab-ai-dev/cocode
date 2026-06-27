# coco-tool-runtime

Tool trait, concurrent executor, tool registry, callback handles. Defines the interface; `coco-tools` provides implementations.

## Key Types

- **Trait + context**: `Tool`, `ToolUseContext`, `DescriptionOptions`, `InterruptBehavior`, `ProgressSender`/`ProgressReceiver`/`ToolProgress`, `PromptOptions`, `SearchReadInfo`, `McpToolInfo`
- **Executor**: `ToolExecutor` (batch via `execute_with`) + `StreamingHandle` (streaming via `feed_plan`/`commit_flush`), `PendingToolCall`. Plan/outcome DTOs (`call_plan`): `ToolCallPlan`, `PreparedToolCall`, `RunOneRuntime`, `UnstampedToolCallOutcome`/`ToolCallOutcome`, `ToolSideEffects`, `ToolMessagePath`, `ToolCallErrorKind`.
- **Registry**: `ToolRegistry`
- **Errors**: `ToolError`, `SyntheticToolError`, `ToolUseEvent`, `classify_tool_error`, `format_tool_error`
- **Validation**: `ValidationResult`, `ValidatedInput` — proof-carrying input newtype; the only constructor (`ValidatedInput::validate`) fuses freeform coercion (`coerce_raw_string_input`) + runtime schema validation. `PendingToolCall.input` and `call_plan::PreparedToolCall.parsed_input` require it, so a freeform tool's raw-string wire input (apply_patch) can never reach permission checks or `serde_json::from_value::<T::Input>` uncoerced. History/wire carriers (`ToolCallPart.input`) intentionally keep the raw shape for provider round-trips.
- **Callback handles** (decouple tool → subsystem circular deps; every handle has a `NoOp*` impl for tests):
  - `AgentHandle`/`AgentHandleRef` + `AgentSpawnRequest`/`AgentSpawnResponse`/`AgentSpawnStatus` — subagent spawning
  - `AgentQueryEngine`/`AgentQueryEngineRef` + `AgentQueryConfig`/`AgentQueryResult` — side-agent queries
  - `HookHandle`/`HookHandleRef` + `HookPermission`/`PreToolUseOutcome`/`PostToolUseOutcome`
  - `McpHandle`/`McpHandleRef` + `McpToolAnnotations`/`McpToolSchema`
  - `TaskHandle`/`TaskHandleRef` + `BackgroundShellRequest`/`BackgroundTaskInfo`/`BackgroundTaskStatus`/`StallInfo`/`TaskOutputDelta` — running background tasks (shell/agent)
  - `TaskListHandle`/`TaskListHandleRef` — persistent V2 plan-item store (`TaskCreate`/`Update`/`Get`/`List`/`Stop`/`Output`). DTOs live in `coco-types` (`TaskRecord`, `TaskRecordUpdate`, `TaskListStatus`, `TaskClaimOutcome`, `ExpandedView`); `coco-tool-runtime` re-exports them. `InMemoryTaskListHandle` for tests; `NoOpTaskListHandle` for sessions without a store.
  - `TodoListHandle`/`TodoListHandleRef` + `TodoRecord` (re-export) — per-agent V1 TodoWrite checklist. `InMemoryTodoListHandle` is the default.
  - `check_verification_nudge(&[&str])` — shared pure helper used by both V1 `TodoWrite` and V2 `TaskUpdate` (`/verif/i` gate, ≥3 items).
  - `MailboxHandle`/`MailboxHandleRef` + `InboxMessage`/`MailboxEnvelope`
  - `ScheduleStore`/`ScheduleStoreRef` — cron store
  - `SideQuery`/`SideQueryHandle` + `SideQueryRequest`/`SideQueryResponse` + `side_query_to_text_callback`
  - `ToolPermissionBridge`/`ToolPermissionBridgeRef` + `ToolPermissionRequest`/`ToolPermissionDecision`/`ToolPermissionResolution`
  - `CanUseToolHandle`/`CanUseToolHandleRef` + `CanUseToolDecision` (`Allow{updated_input}` / `Deny{message}` / `Ask`) + `DecisionReason` + `CanUseToolCallContext` + `NoOpCanUseToolHandle` + `deny_all_handle(reason)` — per-fork tool-execution gate dispatched at `execution::execute_tool_call` step 3.5 BEFORE the tool's built-in `check_permissions`. The `Allow{updated_input}` variant is the path-rewrite hook speculation overlay needs.
  - `PlanApprovalMessage`/`PlanApprovalRequest`/`PlanApprovalResponse`
- **Stall detection**: `STALL_CHECK_INTERVAL_MS`, `STALL_TAIL_BYTES`, `STALL_THRESHOLD_MS`, `format_stall_notification`, `format_task_notification`, `matches_interactive_prompt`

## Architecture

- **Safe tools** (read-only, idempotent) execute concurrently; **unsafe tools** queue and execute after streaming stop. `ToolExecutor` orchestrates this.
- All cross-subsystem interaction (tasks, agents, hooks, MCP, mailbox) goes through callback handle traits — `coco-tool-runtime` does NOT depend on `coco-tools`, `coco-tasks`, `coco-commands`, etc. Implementations are injected via `ToolUseContext` at runtime.
- `ToolUseContext` is the typed payload carried across tool invocations (see main CLAUDE.md "Typed Structs over JSON Values" for the `ToolAppState` migration story).

## Tool Result Budget (Level 1 + state types for Level 2)

Owner of the `tool_result_storage` module (planned: `src/tool_result_storage/`).
Plan: [`docs/coco-rs/tool-result-budget-plan.md`](../../../docs/coco-rs/tool-result-budget-plan.md).

- **Level 1** — per-tool persistence helpers: `persist_to_disk` and
  `render_persisted_reference` live in `tool_result_storage.rs`; `coco-query`
  invokes them after `Tool::execute()` for singleton text results when
  `Tool::max_result_size_chars()` opts in. Known gaps: overwrite rather than
  `create_new`, no empty-content guard here, and Bash still has a tool-local
  `temp_dir()` persistence path for shell stdout.
- **Level 2** — aggregate budget state and decision logic:
  `ContentReplacementState` + `apply_tool_result_budget`. `coco-query` owns the
  message projection/wiring. Known gap: currently replaces selected IDs with
  `[Old tool result content cleared]`; the intended behavior is to persist
  selected fresh candidates and replay the exact `<persisted-output>` preview
  string from replacement state/transcript records.

`Tool::max_result_size_chars()` uses `i64::MAX` as the Rust sentinel for
"unbounded" opt-out.

## Deferred refactors (pure code-quality)

Tracked here for future contributors — none of these changes behavior;
they're all structural cleanups identified in the May 2026 audit.

### Split `AgentSpawnRequest` into 4 sub-structs

`AgentSpawnRequest` carries 27 fields covering four distinct concerns:
model-visible input (prompt / description / subagent_type / model /
run_in_background / isolation / cwd / effort / name / team_name / mode /
auto_background_ms), permission policy (mcp_servers / disallowed_tools /
max_turns / can_use_tool / require_can_use_tool / features /
tool_overrides / parent_tool_filter), routing (tool_use_id /
invoking_agent_id / session_id / fork_label), and spawn-mode discriminant
(spawn_mode / definition / constraints / fork_context_messages /
skip_transcript / use_exact_tools / initial_prompt / model_role /
enable_summarization / is_non_interactive).

Mechanical split:

```rust
pub struct AgentSpawnRequest {
    pub input: AgentSpawnInput,
    pub policy: AgentSpawnPolicy,
    pub routing: AgentSpawnRouting,
    pub spawn_mode: SpawnMode,
    pub definition: Option<Arc<AgentDefinition>>,
    pub constraints: Option<AgentSpawnConstraints>,
    pub fork_context_messages: Vec<Arc<Message>>,
}
```

Touches 5 callers (`AgentTool`, `memory::{extract, dream, session}`,
`coordinator::resume`). Pure refactor, no semantics change.

### `TaskExtras` enum on `TaskStateBase`

`TaskStateBase` currently carries 5 LocalAgent-specific Option fields
(`progress` / `retrieved` / `retain` / `evict_after` / `is_backgrounded`)
that default to None / false for shell / dream / teammate tasks. Pollutes
the type with always-None fields and the per-task-type match logic ends
up scattered across `running.rs` / `reminder_source.rs` / TUI panels.

```rust
pub enum TaskExtras {
    LocalAgent(LocalAgentExtras),
    LocalShell(LocalShellExtras),
    Dream,
    None,
}

pub struct TaskStateBase {
    // core fields only ...
    pub extras: TaskExtras,
}
```

Eliminates sparse LocalAgent sidecar shims entirely; per-task-type accessors
return concrete types via match. Touches 20+ files (every consumer of
`progress` / `retrieved` / `retain` / `evict_after` / `is_backgrounded`).

### Schema ownership (done — formerly a deferred refactor)

`ToolInputSchema` is the **self-validating newtype** in `src/schema.rs`: it
owns the JSON Schema `Value` (model-facing, via `as_value()`) plus a compiled
`Arc<jsonschema::Validator>` (runtime, via `validate()`), built once at
construction by `from_input_type::<T>()` (Bucket-A derive, auto-closed with
`additionalProperties:false`) or `from_value(json!({ … }))` (hand-built /
MCP-wire / `--json-schema`). The old `coco_types::ToolInputSchema
{ properties, required }` data struct and the `Tool::input_schema()` bridge are
**deleted** — every tool declares `runtime_validation_schema()` (no default ⇒
E0046 forces it). The model-facing wire shape is the single source of truth
`tool_spec(&SchemaContext, &PromptOptions) -> ToolSpec` (`Function`/`Freeform`);
its default builds a `Function` from `prompt()` + the runtime schema, and tools
override it to hide hook-injected runtime-only fields (Bash/Agent/ExitPlanMode)
or to present a `Freeform` grammar tool (apply_patch). There is no separate
validator cache. See `docs/coco-rs/tool-schema-final-plan.md` (v4.3).
