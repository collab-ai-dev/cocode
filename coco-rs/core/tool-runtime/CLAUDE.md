# coco-tool-runtime

Tool trait, concurrent executor, tool registry, callback handles. Defines the interface; `coco-tools` provides implementations.

## Key Types

- **Trait + context**: `Tool`, `ToolUseContext`, `DescriptionOptions`, `InterruptBehavior`, `ProgressSender`/`ProgressReceiver`/`ToolProgress`, `PromptOptions`, `SearchReadInfo`, `McpToolInfo`
- **Executor**: `ToolExecutor` (batch via `execute_with`) + `StreamingHandle` (streaming via `feed_plan`/`commit_flush`), `PendingToolCall`. Plan/outcome DTOs (`call_plan`): `ToolCallPlan`, `PreparedToolCall`, `RunOneRuntime`, `UnstampedToolCallOutcome`/`ToolCallOutcome`, `ToolSideEffects`, `ToolMessagePath`, `ToolCallErrorKind`.
- **Registry**: `ToolRegistry`
- **Errors**: `ToolError`, `SyntheticToolError`, `ToolUseEvent`, `classify_tool_error`, `format_tool_error`
- **Validation**: `ValidationResult`, `ValidatedInput`, `Tool::validation_error_steer` — proof-carrying input newtype; the only constructor (`ValidatedInput::validate`) fuses object coercion (`coerce_input`), freeform coercion (`coerce_raw_string_input`), and runtime schema validation. `PendingToolCall.input` and `call_plan::PreparedToolCall.parsed_input` require it, so a tool's repaired input shape reaches permission checks / `serde_json::from_value::<T::Input>`, while history/wire carriers (`ToolCallPart.input`) intentionally keep the raw shape for provider round-trips. `validation_error_steer` lets a tool append targeted guidance to schema failures without changing validation semantics.

### Callback handles

Decouple tool → subsystem circular deps; each has a `NoOp*` test double and is injected via `ToolUseContext` at runtime.

| Handle | Purpose |
|---|---|
| `AgentHandle` | Subagent spawning (`AgentSpawnRequest`/`Response`/`Status`) |
| `AgentQueryEngine` | Side-agent queries (`AgentQueryConfig`/`AgentQueryResult`) |
| `HookHandle` | Pre/PostToolUse hook outcomes |
| `McpHandle` | MCP tool schema + annotations |
| `TaskHandle` | Running background tasks — shell/agent (`ShellTaskRequest`, `BackgroundTaskInfo`, `StallInfo`, …) |
| `TaskListHandle` | Durable V2 plan-item store; DTOs (`TaskRecord` etc.) live in `coco-types`, re-exported here; `InMemoryTaskListHandle` / `NoOpTaskListHandle` |
| `TodoListHandle` | Per-agent V1 TodoWrite checklist (`TodoRecord`); `InMemoryTodoListHandle` is the default |
| `MailboxHandle` | Teammate inbox (`InboxMessage`/`MailboxEnvelope`) |
| `ScheduleStore` | Cron store (`disk_backed_schedule_store` impl) |
| `SideQuery` | One-shot side LLM queries (`side_query_to_text_callback`) |
| `ToolPermissionBridge` | Interactive permission request/decision/resolution |
| `GoalHandle` | Tool-facing seam onto the session goal runtime; leaf dep on `coco-goals` domain types, never on `coco-goal-runtime` |
| `CanUseToolHandle` | Per-fork tool-execution gate (`Allow{updated_input}` / `Deny{message}` / `Ask`) in `can_use_tool.rs`; dispatched from `app/query::tool_call_preparer::resolve_can_use_tool_decision` (step 3.5) BEFORE the tool's built-in `check_permissions`; `Allow{updated_input}` is the speculation overlay's path-rewrite hook |

Also: `PlanApprovalMessage`/`Request`/`Response` DTOs, and `check_verification_nudge(&[&str])` — shared pure helper used by both V1 `TodoWrite` and V2 `TaskUpdate` (`/verif/i` gate, ≥3 items).

## Architecture

- **Safe tools** (read-only, idempotent) execute concurrently; **unsafe tools** queue and execute after streaming stop. `ToolExecutor` orchestrates this.
- All cross-subsystem interaction (tasks, agents, hooks, MCP, mailbox) goes through callback handle traits — `coco-tool-runtime` does NOT depend on `coco-tools`, `coco-tasks`, `coco-commands`, etc. Implementations are injected via `ToolUseContext` at runtime.
- `ToolUseContext` is the typed payload carried across tool invocations (see main CLAUDE.md "Typed Structs over JSON Values" for the `ToolAppState` migration story).

## Schema ownership

`ToolInputSchema` (`src/schema.rs`) is the **self-validating newtype**: it owns
the JSON Schema `Value` (model-facing, via `as_value()`) plus a compiled
`Arc<jsonschema::Validator>` (runtime, via `validate()`), built once at
construction by `from_input_type::<T>()` (derive path, auto-closed with
`additionalProperties:false`) or `from_value(json!({ … }))` (hand-built /
MCP-wire / `--json-schema`). Every tool declares `runtime_validation_schema()`
(no default ⇒ E0046 forces it). The model-facing wire shape has a single
source of truth: `tool_spec(&SchemaContext, &PromptOptions) -> ToolSpec`
(`Function`/`Freeform`); its default builds a `Function` from `prompt()` + the
runtime schema, and tools override it to hide hook-injected runtime-only
fields (Bash/Agent/ExitPlanMode) or to present a `Freeform` grammar tool
(apply_patch). There is no separate validator cache. See
`docs/internal/tool-schema-final-plan.md` (v4.3).

## Tool Result Budget (Level 1/2) + Recoverable Offload

Module DAG (strict, no cycles): `coco_types::persisted_output` (tags + the two
string predicates) ← `tool_result_storage` (write mechanics) ←
`tool_result_offload` (policy: window + budgets + Level 2). Design:
[`docs/internal/tool-result-offload-v2-design.md`](../../../docs/internal/tool-result-offload-v2-design.md).

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

## Conventions

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
