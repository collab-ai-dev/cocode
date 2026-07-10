# coco-tool-runtime

Tool trait, concurrent executor, tool registry, callback handles. Defines the interface; `coco-tools` provides implementations.

## Key Types

- **Trait + context**: `Tool`, `ToolUseContext`, `DescriptionOptions`, `InterruptBehavior`, `ProgressSender`/`ProgressReceiver`/`ToolProgress`, `PromptOptions`, `SearchReadInfo`, `McpToolInfo`
- **Executor**: `ToolExecutor` (batch via `execute_with`) + `StreamingHandle` (streaming via `feed_plan`/`commit_flush`), `PendingToolCall`. Plan/outcome DTOs (`call_plan`): `ToolCallPlan`, `PreparedToolCall`, `RunOneRuntime`, `UnstampedToolCallOutcome`/`ToolCallOutcome`, `ToolSideEffects`, `ToolMessagePath`, `ToolCallErrorKind`.
- **Registry**: `ToolRegistry`
- **Errors**: `ToolError`, `SyntheticToolError`, `ToolUseEvent`, `classify_tool_error`, `format_tool_error`
- **Validation**: `ValidationResult`, `ValidatedInput`, `Tool::validation_error_steer` — proof-carrying input newtype; the only constructor (`ValidatedInput::validate`) fuses object coercion (`coerce_input`), freeform coercion (`coerce_raw_string_input`), and runtime schema validation. `PendingToolCall.input` and `call_plan::PreparedToolCall.parsed_input` require it, so a tool's repaired input shape reaches permission checks / `serde_json::from_value::<T::Input>`, while history/wire carriers (`ToolCallPart.input`) intentionally keep the raw shape for provider round-trips. `validation_error_steer` lets a tool append TS-style targeted guidance to schema failures without changing validation semantics.
- **Callback handles** (decouple tool → subsystem circular deps; every handle has a `NoOp*` impl for tests):
  - `AgentHandle`/`AgentHandleRef` + `AgentSpawnRequest`/`AgentSpawnResponse`/`AgentSpawnStatus` — subagent spawning
  - `AgentQueryEngine`/`AgentQueryEngineRef` + `AgentQueryConfig`/`AgentQueryResult` — side-agent queries
  - `HookHandle`/`HookHandleRef` + `HookPermission`/`PreToolUseOutcome`/`PostToolUseOutcome`
  - `McpHandle`/`McpHandleRef` + `McpToolAnnotations`/`McpToolSchema`
  - `TaskHandle`/`TaskHandleRef` + `ShellTaskRequest`/`BackgroundTaskInfo`/`BackgroundTaskStatus`/`StallInfo`/`TaskOutputDelta` — running background tasks (shell/agent)
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

## Tool Result Budget (Level 1/2) + Recoverable Offload

Module DAG (strict, no cycles): `coco_types::persisted_output` (tags + the two
string predicates) ← `tool_result_storage` (write mechanics) ←
`tool_result_offload` (policy: window + budgets + Level 2). Design:
[`docs/coco-rs/tool-result-offload-v2-design.md`](../../../docs/coco-rs/tool-result-offload-v2-design.md).

- **`tool_result_storage`** — write mechanics only:
  - `ToolOutputStore::write_artifact(key, content)` — `ToolUse` keys use
    `create_new` (first write wins); `Named` keys (caller-computed,
    content-addressed) publish atomically (tmp + rename). Validates `Named`
    names (`[A-Za-z0-9._-]`, ≤100 bytes, no leading dot); owns no URL semantics.
  - `ToolOutputStore::persist_binary` — MIME-extension binary spill
    (`extension_for_mime_type` is the single MIME table, shared with WebFetch's
    store-less fallback).
  - `ResultSizeBound::{Bytes(i64), Unbounded}` — per-tool Level-1 declaration.
    **Declarations are authoritative — there is no hidden global clamp.** Trait
    default `Bytes(50_000)`; WebFetch declares `Bytes(102_000)` so preapproved
    docs pages pass verbatim; Glob's declared 100K is real.
- **`tool_result_offload`** — policy:
  - `WindowedView::compute` — pure 75%/25% head+tail window (zero I/O, zero
    alloc), snapped to line boundaries. Line accounting is CONSERVATIVE on both
    sides: a partial head line is re-read via `omitted_start_line`; a partial
    tail line is INCLUDED in `omitted_end_line` (and `tail_start_line` equals
    it) — the reported omitted range may overlap what is shown but never
    leaves an unreported gap.
  - `offload_windowed(store: Option<&ToolOutputStore>, key, content, budget)` —
    hard-wraps at 400 bytes (Read-navigability), windows, persists the complete
    wrapped text, renders head + omission marker + `<persisted-output>` footer.
    The tag wraps ONLY the trailing footer (never append text after it — that
    breaks `is_pointer_bearing` and lets micro-compact destroy the pointer). A
    missing store or failed write (warn-logged) degrades to a pointerless
    window — NEVER a tool error.
  - `InlineBudget` (i64 newtype): `try_new` for config (reject `<=0`),
    `from_request` for model params (clamp), `.capped_to(threshold)` binds it
    under a tool's declared threshold so a windowed render never re-persists.
  - `scaled_per_message_bytes(window_tokens)` — the window-scaled Level-2 cap.
  - `apply_tool_result_budget` + `ContentReplacementState { per_message_bytes }`
    — Level 2 offloads the largest fresh candidates until the group fits.
    Pointer-bearing (windowed) results COUNT toward the trigger but are never
    re-offloaded (a re-render under the same `ToolUse` id would point footer
    numbers at the wrong artifact bytes).
- **Level 1** — the query tool-outcome builder routes every over-threshold text
  result through `offload_windowed` with `ArtifactKey::ToolUse`. Window budget =
  `REFERENCE_BUDGET` (4K) by default, or the tool's `inline_window_budget()`
  (Bash keeps a larger window so tail errors survive), always capped by the
  threshold. Non-MCP PostToolUse hooks receive a string-capped (≤50K/value)
  view of the data envelope; MCP keeps the full envelope (hooks may rewrite it).

## Deferred refactors (pure code-quality)

Tracked here for future contributors — none of these changes behavior;
they're all structural cleanups identified in the May 2026 audit.

### `AgentSpawnRequest` grouping

`AgentSpawnRequest` is intentionally nested by concern:

```rust
pub struct AgentSpawnRequest {
    pub input: AgentSpawnInput,
    pub execution: AgentSpawnExecution,
    pub permissions: AgentSpawnPermissions,
    pub inheritance: AgentSpawnInheritance,
    pub routing: AgentSpawnRouting,
    pub telemetry: AgentSpawnTelemetry,
}
```

Portable wire fields live in those nested structs. Runtime-only handles and
process-local state stay behind `#[serde(skip)]`: definitions/output schema in
`input`, `SpawnMode` in `execution`, can-use-tool callbacks in `permissions`,
parent feature/tool/filter inheritance in `inheritance`, and abort signals in
`routing`. New spawn fields should be added to the smallest matching group,
not to the request root.

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
