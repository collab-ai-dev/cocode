# coco-rs Agent Loop Refactor Plan

This document is a development guide for refactoring the coco-rs agent loop so
it aligns with the TypeScript implementation.

The TypeScript project is the behavioral specification. The Rust
implementation should not copy TS structure line by line. Instead, it should
preserve TS semantics using Rust-native architecture: explicit traits, typed
data transfer objects, clear crate boundaries, focused modules, and deterministic
message/result ordering.

## Summary

The current Rust `QueryEngine` owns too many responsibilities:

```text
LLM turn loop
  + model streaming
  + eager tool dispatch
  + tool permission checks
  + hook orchestration
  + tool executor setup
  + app-state patch handling
  + Agent tool routing
  + Skill tool routing
  + model recovery
  + compaction
```

The desired architecture splits these responsibilities into focused services:

```text
QueryEngine
  owns the high-level agent turn loop only

ToolCallRunner
  owns every tool invocation lifecycle

ToolContextFactory
  owns accurate ToolUseContext construction

HookAdapter
  bridges app/query to coco-hooks through coco_tool::HookHandle

AgentRuntime
  backs AgentTool with real subagent execution

SkillRuntime
  backs SkillTool with inline and forked skill behavior

ModelRuntime
  owns model call policy, fallback, streaming retry, and max-token recovery
```

This is a larger refactor, but it should be delivered in small phases. The most
important first step is not the module split itself. The first step is enforcing
the safety invariant that every assistant tool call produces exactly one matching
API-visible tool result.

## Why This Refactor Is Needed

The Rust implementation already has many of the correct types and partial
components:

- `Tool` trait has validation, permission, prompt, execution, and result APIs
  (`core/tool/src/traits.rs:189/364/383/416`).
- `ToolUseContext` already models 65+ TS context fields (`core/tool/src/context.rs:76-345`).
- `StreamingToolExecutor` can batch concurrent-safe tools (`core/tool/src/executor.rs:308-337`).
- `HookHandle` exists to avoid a dependency cycle between `coco-tool` and
  `coco-hooks`. `NoOpHookHandle` is the only impl today (`hook_handle.rs:218-251`).
- `AgentHandle` exists to avoid a dependency cycle between `coco-tools` and
  `coco-state`. `SwarmAgentHandle` implements it but several methods are stubs.
- `coco_tool::AgentQueryEngine` already exists as a trait for child query
  execution (`core/tool/src/agent_query.rs`). The refactor should reuse it,
  not introduce a new trait.
- `SkillManager` and skill prompt expansion helpers exist (`skills/src/lib.rs:140`,
  `core/tools/src/tools/skill_advanced.rs`).

The problem is wiring. Several runtime paths implement overlapping but
incomplete tool semantics:

- The eager streaming path in `engine.rs:1539` calls `tool.execute` directly,
  bypassing validation, hooks, and permission resolution. This is the most
  dangerous bug. It is **not** gated by `streaming_tool_execution` — it fires
  whenever a streamed tool input completes during streaming, and produces
  tool results without permission checks.
- The non-streaming path in `engine.rs:1899/2067/2153/2197` runs permission
  and hooks inline.
- `core/tool/src/executor.rs:394-418/570-655` has its own hook-handling code
  that calls `ctx.hook_handle`. Today `ctx.hook_handle` is always `None`
  (engine never installs a real one), so this code is dormant. Installing a
  real `HookHandle` will activate this path and create a second hook owner
  unless executor's hook calls are deleted in the same change.
- `core/tool/src/execution.rs` contains the canonical validation +
  Bash internal-field stripping helper, but neither engine path uses it.
- `ToolResult::new_messages` is appended only on the compaction path
  (`engine.rs:2588-2596`); the normal tool path drops it.
- `AgentTool` and `SkillTool` route through `ToolUseContext.agent`, but
  `engine.rs:2881` hardcodes `Arc::new(NoOpAgentHandle)`. There is no
  factory hook that lets a real handle be installed.
- `SwarmAgentHandle::resolve_skill` returns
  `Err("Skill resolution for '{name}' should be handled by the query engine")`
  (`swarm_agent_handle.rs:612-620`). Skills are not "routed through agent
  handle" — they are **broken end-to-end** today. The fix is to delete
  `resolve_skill` from `AgentHandle` and add a `SkillHandle` trait.
- `InProcessAgentRunner::spawn_agent` (`swarm_runner.rs:296`) only registers
  state; the executor loop is started elsewhere via `set_result_channel`.
  The split between registration and execution is implicit and easy to
  misuse. Rename to `register_agent` and add explicit `start_agent`.
- `create_tool_context` (`engine.rs:2801-2805`) hardcodes
  `thinking_level`, `is_non_interactive`, `max_budget_usd`,
  `custom_system_prompt`, `append_system_prompt` to defaults regardless of
  config.
- `fallback_model` is defined in `QueryEngineConfig` (`config.rs:62`) but
  never read in `engine.rs`.

This creates behavioral drift from TS:

- Streaming-dispatched tools execute without permission or hook checks.
- Validation can be skipped.
- Hook-blocked tools can produce no `tool_result`.
- Unknown tools can produce no `tool_result`.
- Post hooks can receive `null` instead of the effective input.
- Agent and Skill tools are exposed to the model but are not backed by a real
  runtime.
- `fallback_model`, `is_non_interactive`, `max_budget_usd`, `custom_system_prompt`,
  `append_system_prompt` are present in config but not honored by the runtime loop.

The refactor goal is to make the agent loop boring and reliable:

```text
QueryEngine calls the model.
ToolCallRunner executes all tool calls.
Every tool call returns messages.
QueryEngine appends those messages and continues.
```

## TS Source Mapping

The following TS files are the primary behavior references. Paths are relative
to the TS project's `src/` directory.

| TS file | Behavior to preserve | Rust target |
|---------|----------------------|-------------|
| `query.ts` | Main recursive agent loop: build prompt, call model, stream tokens, collect tool calls, execute tools, append results, compact, continue or stop. | `coco-rs/app/query/src/engine.rs`, new `model_runtime.rs`, new `tool_runner.rs` |
| `services/tools/toolExecution.ts` | Single tool lifecycle: tool lookup, input validation, hooks, permission, execution, post hooks, failure hooks, `newMessages`, continuation stop. | new `coco-rs/app/query/src/tool_runner.rs`; some reusable logic may live in `coco-rs/core/tool/src/execution.rs` |
| `services/tools/toolOrchestration.ts` | Partition safe vs unsafe tools, run concurrent-safe tools together, apply context modifiers after batches. | `coco-rs/core/tool/src/executor.rs`, new `ToolCallRunner` scheduling integration |
| `services/tools/StreamingToolExecutor.ts` | Streaming tool scheduling: add tool calls as they finish streaming, drain completed results, preserve same lifecycle as non-streaming tools. | `coco-rs/core/tool/src/executor.rs`, new streaming scheduling wrapper in `tool_runner.rs` |
| `services/tools/toolHooks.ts` | PreToolUse, PostToolUse, PostToolUseFailure behavior and hook output interpretation. | `coco-rs/hooks/src/orchestration.rs`, new `app/query/src/hook_adapter.rs` |
| `tools/AgentTool/AgentTool.tsx` | AgentTool input semantics: sync/background agents, teammate spawn, fork guard, worktree/remote isolation, progress, result shape. | `coco-rs/core/tools/src/tools/agent.rs`, new `AgentRuntime` behind `AgentHandle` |
| `tools/AgentTool/runAgent.ts` | Real subagent runtime: child model, child system prompt, child tools, permission inheritance, MCP, hooks, transcript, result aggregation. | `coco-rs/app/query/src/agent_adapter.rs`, `coco-rs/app/state/src/swarm_agent_handle.rs`, new `agent_runtime.rs` or state runtime |
| `tools/AgentTool/agentToolUtils.ts` | Agent tool filtering, allowed agent types, final AgentTool result shape, background lifecycle helpers. | `coco-rs/core/tools/src/tools/agent_advanced.rs`, `agent_spawn.rs`, AgentRuntime |
| `tools/AgentTool/loadAgentsDir.ts` | Agent definition loading and frontmatter parsing. | `coco-rs/core/tools/src/tools/agent_spawn.rs`, `agent_advanced.rs` |
| `tools/AgentTool/builtInAgents.ts` | Built-in agent definitions and default behavior. | `coco-rs/core/tools/src/tools/agent_spawn.rs` |
| `tools/SkillTool/SkillTool.ts` | SkillTool permission, inline expansion, forked skills, remote skills, `newMessages`, context modifiers. | `coco-rs/core/tools/src/tools/agent.rs`, new `SkillRuntime`, `coco-rs/skills/src/lib.rs`, `skill_advanced.rs` |
| `tools/SkillTool/prompt.ts` | Dynamic SkillTool prompt listing and skill visibility rules. | `coco-rs/skills/src/lib.rs`, `Tool::prompt`, provider-agnostic tool catalog builder |
| `query/tokenBudget.ts` | Token-budget continuation rules. | `coco-rs/app/query/src/budget.rs`, `engine.rs` |
| `utils/attachments.ts` | Attachment injection around turns and tool-generated context. | `coco-rs/core/context`, `coco-rs/core/system-reminder`, `app/query` attachment inbox |
| `utils/messages.ts` | Message creation and API normalization, including tool results. | `coco-rs/core/messages`, `coco-rs/common/types` |
| `utils/permissions/*` | Permission rule evaluation, auto-mode, denial tracking, user approval flow. | `coco-rs/core/permissions`, `app/query` permission controller |
| `utils/hooks/*` | Hook config, execution, aggregation, stop behavior, additional context. | `coco-rs/hooks`, new `HookAdapter` |
| `utils/swarm/*` | In-process teammates, mailbox, team spawn, task state. | `coco-rs/app/state/src/swarm_*` |

## TS Behavioral Contract

This section is the implementation checklist extracted from the TS project. The
Rust code does not need to mirror the TS call graph, but it must preserve these
observable behaviors.

### `query.ts`

Rust parity target:

1. Build the prompt from current history, system prompt, reminders, attachments,
   queued commands, and compacted history.
2. Build a provider-agnostic tool catalog from currently available tools.
3. Create a `ToolUseContext` equivalent with current model, permission context,
   app state, task state, file state, MCP, agents, skills, and hooks.
4. Call the model and stream assistant text, thinking, and tool input deltas.
5. If configured, allow streaming tool scheduling. This must use the same
   lifecycle as non-streaming execution.
6. Commit the assistant message containing text/thinking/tool calls.
7. Execute tool calls and collect model-visible tool result messages.
8. Add tool-generated messages and hook-generated context.
9. Generate tool summaries when enabled.
10. Drain queued commands and teammate inbox messages.
11. Decide whether to continue due to tool calls, token budget, max-output
    recovery, fallback, reactive compaction, or normal completion.

Important TS details:

- `fallbackModel` changes the active model and updates
  `toolUseContext.options.mainLoopModel`.
- `config.gates.streamingToolExecution` gates streaming tool scheduling.
- Tool execution returns messages that are normalized for the next API request.
- Hook continuation stops are represented as attachment messages and affect the
  next loop decision.

### `services/tools/toolExecution.ts`

Rust parity target for a single tool call:

1. Resolve tool by name.
2. If the tool is missing, return a synthetic error tool result.
3. Parse and validate model input before permission resolution.
4. Run tool-specific validation before permission resolution.
5. Strip model-controlled internal fields before the tool implementation sees
   the input.
6. Run `PreToolUse` hooks.
7. Apply hook input rewrite.
8. Apply hook permission behavior.
9. Resolve normal tool permission if hooks did not decide.
10. Execute the tool.
11. Run `PostToolUse` on execution success.
12. Run `PostToolUseFailure` **only** on execution-stage exceptions
    (`toolExecution.ts:1696`, inside the `catch (error)` block at line 1589).
    Unknown tool, validation failure, PreToolUse stop, and permission
    denial **do not** trigger `PostToolUseFailure` — they yield an error
    result and return early (`toolExecution.ts:396/848/995`). JSON parse
    failure is **pre-commit** (the call is dropped before the assistant
    message is committed) and so does not appear in this list at all —
    see "JSON Parse Failures Are Pre-Commit, Not Pre-Batch" in the
    `ToolCallRunner` section.
13. Return the tool result message, hook messages, `newMessages`, and any
    context modifier.

Rust should validate hook-updated input before execution even if TS currently
relies on hook trust. That is a Rust-side safety improvement and does not change
the model-visible TS contract.

### `services/tools/toolOrchestration.ts`

Rust parity target:

- Partition tool calls into consecutive concurrent-safe batches and individual
  unsafe calls.
- Run concurrent-safe tools together.
- Run unsafe tools serially.
- Preserve API-visible result order.
- Queue context modifiers from concurrent batches and apply them in submission
  order after the batch.
- Apply serial context modifiers before the next serial tool starts.

Rust equivalent:

- `ToolResult::app_state_patch` is the typed version of TS
  `contextModifier`.
- `StreamingToolExecutor` should apply patches after execution, not tools
  mutating shared state directly.
- Concurrent tools should not mutate shared app state inline.

### `services/tools/StreamingToolExecutor.ts`

Rust parity target:

- Streaming execution is a scheduler, not a separate semantic path.
- `addTool` should handle unknown tools by producing a completed error result.
- `executeTool` delegates to the same tool lifecycle as non-streaming tools.
- Completed results are drained while the model is still streaming.
- Remaining results are collected after the stream ends.
- Streaming fallback discards unfinished work safely.

Rust implication:

The current eager path that directly calls `tool.execute` should be removed or
rewired so it calls `ToolCallRunner`.

### `tools/AgentTool/AgentTool.tsx` And `tools/AgentTool/runAgent.ts`

Rust parity target:

- AgentTool is a control-flow tool. It launches a child agent, not a plain JSON
  helper.
- Agent definitions can be built-in, user, project, or plugin-provided.
- Child tools are resolved from the agent definition and parent constraints.
- Permission mode inheritance follows TS rules.
- Sync agents return a final result to the parent tool call.
- Background agents return launch metadata and store output for later retrieval.
- Teammates join a team and communicate through mailbox/task state.
- Fork agents clone or wrap parent context according to fork rules.
- Worktree isolation changes child cwd and cleanup behavior.

Rust implication:

`AgentHandle` is the correct dependency-inversion point, but it must be backed by
a real runtime in normal sessions. `NoOpAgentHandle` is only valid for tests or
explicitly unsupported contexts.

### `tools/SkillTool/SkillTool.ts` And `tools/SkillTool/prompt.ts`

Rust parity target:

- SkillTool lists only model-invocable skills.
- SkillTool resolves aliases.
- Disabled skills are rejected.
- Model-disabled skills are hidden from the model.
- Inline skills expand prompt text into new conversation messages.
- Fork skills launch an agent.
- Skill `newMessages` are tagged with the parent tool use id.
- Skill context modifiers affect subsequent tool context.

Rust implication:

Skill execution should not be implemented as `AgentHandle::resolve_skill`.
Skills need a dedicated runtime or handle backed by `SkillManager`.

## TS Concepts To Rust Concepts

| TS concept | Rust concept | Notes |
|------------|--------------|-------|
| `query.ts` loop | `QueryEngine` | Rust should keep this as the turn coordinator only. |
| `runToolUse` | `ToolCallRunner::run_one` | One authoritative lifecycle for every tool call. |
| `runTools` | `ToolCallRunner::run_many` + `StreamingToolExecutor` | Runner owns semantics; executor owns scheduling. |
| `StreamingToolExecutor` | `StreamingToolExecutor` plus runner integration | Must not bypass validation/hooks/permission. |
| `ToolUseContext` | `coco_tool::ToolUseContext` | Built by `ToolContextFactory` from live state. |
| `contextModifier` | `ToolResult::app_state_patch` plus `new_messages` | Patches mutate app state; messages mutate conversation history. |
| `newMessages` | `ToolResult::new_messages` | Must be appended and normalized into the next API request. |
| `canUseTool` | `PermissionController` | Includes hook override, tool permission, auto-mode, bridge. |
| `runPreToolUseHooks` | `QueryHookHandle::run_pre_tool_use` | Bridge from `coco-tool` to `coco-hooks`. |
| `runPostToolUseHooks` | `QueryHookHandle::run_post_tool_use` | Must receive effective input and output. |
| `runPostToolUseFailureHooks` | `QueryHookHandle::run_post_tool_use_failure` | Requires structured hook input in `coco-hooks`. |
| `runAgent` | `AgentRuntime` / `AgentQueryEngine` | Child QueryEngine execution with child context. |
| `SkillTool.call` | `SkillRuntime` | Inline returns `new_messages`; fork calls AgentRuntime. |
| `fallbackModel` | `ModelRuntime` | Requires a model/client factory or runtime abstraction. |

## Current Rust Hotspots

These Rust files contain the behavior that must be consolidated:

| Rust file | Current role | Refactor direction |
|-----------|--------------|--------------------|
| `coco-rs/app/query/src/engine.rs` | Main loop plus many tool lifecycle details. | Keep only turn orchestration; move tool lifecycle to `tool_runner.rs`. |
| `coco-rs/app/query/src/config.rs` | Query configuration. | Keep, but ensure all fields are honored or removed. |
| `coco-rs/app/query/src/agent_adapter.rs` | Adapts QueryEngine to AgentQueryEngine. | Expand to preserve messages, tool counts, allowed tools, context, and child services. |
| `coco-rs/core/tool/src/traits.rs` | Canonical `Tool` trait and prompt/validation APIs. | Keep; use the existing APIs consistently. |
| `coco-rs/core/tool/src/context.rs` | Canonical `ToolUseContext`. | Keep; build accurate instances through `ToolContextFactory`. |
| `coco-rs/core/tool/src/executor.rs` | Batching and concurrent execution. | Keep as scheduler/executor; remove duplicated lifecycle decisions from higher layers. |
| `coco-rs/core/tool/src/execution.rs` | Partial single-call lifecycle. | Either promote into the new runner or reduce to reusable sanitization/helpers. |
| `coco-rs/core/tool/src/hook_handle.rs` | Hook callback trait for dependency inversion. | Keep; implement it in app/query. |
| `coco-rs/core/tool/src/agent_handle.rs` | Agent callback trait for dependency inversion. | Keep; install real handles in QueryEngine. |
| `coco-rs/core/tools/src/tools/agent.rs` | Agent, Skill, SendMessage, Team tools. | Keep tool schemas and IO, but route Agent/Skill through real runtimes. |
| `coco-rs/core/tools/src/tools/skill_advanced.rs` | Existing skill expansion helpers. | Reuse in SkillRuntime. |
| `coco-rs/core/tools/src/tools/agent_spawn.rs` | Existing agent definition loading. | Reuse in AgentRuntime. |
| `coco-rs/core/tools/src/tools/agent_advanced.rs` | Agent filtering and prompt helpers. | Reuse in AgentRuntime and dynamic prompts. |
| `coco-rs/app/state/src/swarm_agent_handle.rs` | AgentHandle implementation. `spawn_subagent` works but `resolve_skill` is a stub returning Err. | Wire `spawn_subagent` to real `AgentQueryEngine`; remove `resolve_skill` (skills move to `SkillHandle`). |
| `coco-rs/app/state/src/swarm_runner.rs` | `spawn_agent` only registers state; execution is started later via `set_result_channel`. | Rename to `register_agent` + add explicit `start_agent` so the registration/execution split is named. |
| `coco-rs/app/state/src/swarm_runner_loop.rs` | In-process teammate loop. Defines its own `AgentExecutionEngine` trait, separate from `coco_tool::AgentQueryEngine`. | The two traits serve different purposes (teammate loop vs side queries). Keep both, document the split, do not merge. |
| `coco-rs/hooks/src/orchestration.rs` | Hook execution for many hook events. `AggregatedHookResult` already has all 18 fields needed. `PostToolUseFailureInput` exists in `inputs.rs`; only the `execute_post_tool_use_failure` wrapper helper is missing. | Add the wrapper helper; use through HookAdapter. |
| `coco-rs/skills/src/lib.rs` | Skill definitions, manager, prompt listing. | Use as SkillRuntime source of truth. |
| `coco-rs/app/query/src/single_turn.rs` | Single-turn helper for compaction summaries / classifier / memory extraction. | Out of scope. Not affected by this refactor; do not fold into ModelRuntime. |
| `coco-rs/app/query/src/emit.rs` | `CoreEvent` emission helpers. | Reused by ToolCallRunner; do not duplicate. |
| `coco-rs/app/query/src/plan_mode_reminder.rs` | Plan-mode reminder cadence. | Coordinates with prompt build, not tool runner. Document the touchpoint in `engine.rs`. |

## Required Invariants

These invariants must hold after the refactor.

### I1. Every Tool Call Gets One Tool Result

Every assistant tool call in the API-visible assistant message must be followed
by exactly one API-visible tool result with the same `tool_call_id`.

This includes:

- unknown tool
- schema or tool-specific validation failure
- PreToolUse hook block
- permission denial
- permission approval rejection
- approval bridge failure
- execution error
- cancellation
- join failure

JSON parse failure is **not** in this list. It is a pre-commit failure:
the streaming/non-streaming accumulators drop a tool_use whose input
fails to parse before the assistant message is committed (see
`engine.rs:1592-1606` and the "JSON Parse Failures Are Pre-Commit, Not
Pre-Batch" subsection of `ToolCallRunner`). I1 only governs
**committed** tool_use entries.
- streaming fallback discard

The model must never see an assistant tool call without a matching result.

### I2. Streaming Does Not Change Tool Semantics

Streaming execution may change when a tool starts. It must not change:

- validation order
- hook behavior
- permission behavior
- input rewrite behavior
- post-hook behavior
- app-state patch behavior
- `new_messages` behavior
- error result mapping

TS uses `StreamingToolExecutor` as a scheduler that still calls `runToolUse`.
Rust should follow the same principle.

### I3. Tool Lifecycle Order Matches TS

The effective lifecycle is:

```text
resolve tool
  -> consume already-parsed input (JSON parse happens upstream in
     the assistant-message accumulator; parse failure is a pre-commit
     drop and never reaches the runner — see "JSON Parse Failures
     Are Pre-Commit, Not Pre-Batch")
  -> validate raw model input against tool schema
  -> run tool-specific validate_input
  -> strip model-controlled internal fields (defense in depth, AFTER validation)
  -> run PreToolUse hooks
  -> apply hook input rewrite
  -> re-validate hook-updated input (Rust-side tightening; TS skips)
  -> apply hook permission override if present
  -> run normal permission check if no hook override
  -> run auto-mode classifier for Ask when active
  -> run permission bridge for unresolved Ask
  -> execute tool
  -> on success:  run PostToolUse hooks
  -> on execution exception only:  run PostToolUseFailure hooks
        (NOT for unknown-tool / validation-failed / hook-blocked /
         permission-denied paths — those yield error result and return)
  -> build UnstampedToolCallOutcome carrying app_state_patch
  -> executor stamps completion_seq at surface time
        (runner does NOT apply the patch itself)
  -> orchestrator/executor applies patch BEFORE next serial tool's
        ToolUseContext is built (matches TS toolOrchestration.ts:140)
  -> emit messages per I5 (the buckets, not a single sequence)
```

**Patch application timing — must match TS, not the earlier draft.** The
contextModifier in TS is attached to the yielded value AFTER PostToolUse
runs (`toolExecution.ts:1467`), and the orchestrator applies it AFTER
receiving the yield (`toolOrchestration.ts:140`). Current Rust
`executor.rs:482-506` already does post-hook → patch in the right order;
the earlier I3 draft that said "execute → patch → post-hook" was wrong.
The patch is owned by the executor/orchestrator, applied between serial
tools so the next tool's context observes the mutation.

**Sanitization order — must match TS, not the current Rust code.** TS validates
the model input (`toolExecution.ts:614`), then strips `_simulatedSedEdit` as
defense in depth (`toolExecution.ts:756`). Current Rust does the opposite
(`core/tool/src/execution.rs:105` strips before `:140` validates). The
refactor must flip this so validation comes first; stripping is only a
safeguard against schema drift, not a precondition for validation. Update
all places in this plan that say "sanitize before validate" — this is the
single canonical order.

If the final order differs for a specific reason, that reason must be documented
in code and covered by tests.

The Rust-side tightening recommended here is re-validating hook-updated input
before permission and execution. TS hooks can rewrite input; Rust should not let
a hook rewrite bypass the tool's typed validation boundary.

### I4. Hook Input Is The Effective Input

If a PreToolUse hook rewrites input, then the updated input is the input for:

- permission checks
- auto-mode classifier
- permission bridge approval request
- tool execution
- PostToolUse
- PostToolUseFailure
- audit records

The current behavior of passing `serde_json::Value::Null` to post hooks must be
removed.

### I5. Message Buckets Are First-Class

A single tool call produces messages in **6 distinct buckets**. The runner
must emit them in TS-compatible order. This is more nuanced than "tool
result → new_messages → hook context"; that earlier framing was wrong.

Buckets and their TS sources:

| Bucket | TS source | Order role |
|---|---|---|
| **Pre-hook messages** | `runPreToolUseHooks` yielded `message` events at `toolExecution.ts:815` | Pushed to `resultingMessages` BEFORE permission/execution |
| **Tool result (non-MCP)** | `addToolResult` at `toolExecution.ts:1478`, fires BEFORE the post-hook loop runs | Emitted right after pre-hook messages |
| **Post-hook messages — non-MCP path (inline)** | Pushed inline as collected at `toolExecution.ts:1515` | For non-MCP: emitted BETWEEN tool_result and newMessages |
| **Tool result (MCP)** | `addToolResult(toolOutput)` at `toolExecution.ts:1541`, fires AFTER the post-hook loop completes (so `updatedMCPToolOutput` can rewrite the output) | For MCP: emitted AFTER pre-hook + hook collection, BEFORE newMessages |
| **`result.newMessages`** | Pushed at `toolExecution.ts:1566` | Non-MCP: emitted AFTER post-hook inline messages. MCP: emitted right after tool_result. |
| **Post-hook messages — MCP path (deferred)** | Collected into `hookResults` at `toolExecution.ts:1499`, flushed at `toolExecution.ts:1585` | For MCP: emitted AT THE END (after newMessages AND after prevent_continuation) |
| **PostToolUseFailure error path** | `runPostToolUseFailureHooks` (separate generator) at `toolExecution.ts:1696` | Only fires on **execution-stage exceptions**, not on unknown-tool / validation / pre-hook-stop / permission-denied paths |
| **prevent_continuation attachment** | Pushed at `toolExecution.ts:1572` (between newMessages at 1566 and the deferred MCP hookResults flush at 1585) | Appended after newMessages; for MCP it sits BEFORE the deferred post-hook flush |

**Critical distinction — MCP and non-MCP have DIFFERENT emission orders.**
The earlier draft that said "emission order is the same; only collection
timing differs" was wrong. The actual TS branching is at the post-hook
loop (`toolExecution.ts:1498–1530`):

- **non-MCP branch (line 1515):** post-hook messages pushed INLINE as the
  loop iterates. Result was already emitted at line 1478.
- **MCP branch (line 1499):** post-hook results pushed into a
  `hookResults` array, NOT emitted inline. After the loop, result emits
  at 1541, then newMessages at 1566, then the deferred `hookResults` flush
  at 1585.

Net effect (full success-path order, including prevent_continuation):
- non-MCP: `tool_result → post-hook msgs → newMessages → prevent`
- MCP:     `tool_result → newMessages → prevent → post-hook msgs`

Note the MCP-specific subtlety: `prevent_continuation` is pushed at TS line
1572 (success block, after newMessages), but the deferred MCP `hookResults`
are flushed at line 1585 — so for MCP, `prevent_continuation` sits BEFORE
the deferred post-hook messages, not at the very end.

Constraints:

- The runner must branch on `tool.is_mcp()` and emit the correct sequence
  per branch. `is_mcp` is **not** informational — it changes message order.
- MCP differs from non-MCP in two ways:
  1. PostToolUse hooks may rewrite the tool *output* before result emit.
  2. Post-hook messages are deferred to AFTER newMessages, not BETWEEN
     result and newMessages.
- `new_messages` (TS `result.newMessages`) is **separate** from
  `additional_contexts` (hook-produced). Both exist; do not conflate them.
- The success path uses PostToolUse; the failure path uses PostToolUseFailure.
  These are mutually exclusive — one tool call enters exactly one of them
  (and many tool calls enter neither).
- `prevent_continuation` surfaces as a synthetic attachment ONLY on the
  success path. It is appended AFTER `new_messages`. In MCP success it
  sits BEFORE the deferred post-hook messages (TS line 1572 vs 1585);
  in non-MCP success it is the last entry. **On the failure path it is
  not emitted at all** — TS jumps from `tool.execute()` exception
  straight into the catch block at `toolExecution.ts:1589`, returning
  `[error tool_result, ...hookMessages]` at :1715, bypassing the
  success-block prevent append at :1572.

This is required for:

- SkillTool inline expansion
- hook-added context
- MCP tool result rewrite ordering
- future tools that need to append structured context
- transcript correctness

### I6. Context Must Be Accurate

`ToolUseContext` must not be mostly hardcoded defaults. Fields in
`QueryEngineConfig` and live app state must be reflected accurately.

At minimum:

- `is_non_interactive`
- `max_budget_usd`
- `custom_system_prompt`
- `append_system_prompt`
- `main_loop_model`
- `messages`
- `agent`
- `hook_handle`
- `permission_bridge`
- `app_state`
- `file_read_state`
- `file_history`
- `config_home`
- `plans_dir`
- `task_list`
- `todo_list`
- `query_depth`
- `agent_id`
- `preserve_tool_use_results`

### I7. Agent And Skill Are Runtime Features

Agent and Skill tools are not simple JSON-producing tools.

`AgentTool` must run a real subagent or background/teammate task.
`SkillTool` must expand inline skills into conversation messages or fork a real
agent for forked skills.

Exposing these tools with `NoOpAgentHandle` in normal sessions is incorrect.

### I8. One Owner For Each Lifecycle Decision

There must be one owner for each decision:

- `QueryEngine` owns loop continuation.
- `ModelRuntime` owns model call/retry/fallback behavior.
- `ToolCallRunner` owns tool-call semantics, **including all hook calls**.
- `StreamingToolExecutor` owns scheduling, batching, and `app_state_patch`
  application only. It does **not** call hooks. Today's hook code at
  `executor.rs:394-418/570-655` must be removed in the same change that
  installs `QueryHookHandle`.
- `PermissionController` (in `app/query`) owns permission resolution
  *orchestration*. The auto-mode classifier LLM call lives in
  `coco-permissions`. PermissionController calls into it; it does not
  reimplement classifier logic.
- `HookAdapter` owns conversion between `coco-hooks` and `coco-tool`.
- `AgentRuntime` owns child agent execution. It uses the existing
  `coco_tool::AgentQueryEngine` trait — does not introduce a new one.
- `SkillRuntime` owns skill resolution and expansion via a new
  `SkillHandle` trait. `AgentHandle::resolve_skill` is **deleted**.

If two modules both decide the same thing, one of them is wrong.

### I9. App-State Mutations Are Queued Effects

Tools must not mutate shared app state inline. Mutations should flow through:

```text
ToolResult::app_state_patch
  -> executor applies patch under one write lock
  -> executor emits TaskPanelChanged when needed
  -> next ToolUseContext snapshot observes the updated state
```

This is the Rust equivalent of TS context modifiers.

The current implementation at `executor.rs:701-749` already does this
correctly: single write lock, ordered patch application, `TaskPanelChanged`
emission, `FnOnce` stripping for `Sync`-safety. **Keep it as is.**

Patch lifecycle on error: a tool that returns `Err` discards its patch (the
patch is never applied). Tests must cover this — silent patch leakage on
errors would cause invisible state drift.

### I10. Cancellation And Progress Are Part Of The Contract

Tool execution must be cancellation-aware and progress-capable:

- every running tool gets a cancellation token
- sibling abort for shell failures remains supported
- progress events are forwarded through `ToolUseContext.progress_tx`
- per-call lifecycle invariant is deterministic (one `Queued` → exactly
  one `Completed`; `Started` is conditional on `Runnable`); global
  start/completion order follows runtime scheduling and may interleave
  for concurrent batches (see I12 / I14)
- cancellation produces a model-visible error result for committed tool calls

Cancellation hierarchy (must be specified, not inferred):

```text
session cancellation token (root)
  -> turn cancellation token (child of session)
    -> per-tool cancellation token (child of turn, created by executor)
      -> sibling abort: shell tool failure cancels other concurrent siblings
```

`ToolCallRunner` forwards the turn token via `ctx.cancellation`. The
executor creates the per-tool tokens. Sibling abort lives in the executor
because it observes batch results. The runner does not duplicate any of
these.

### I11. Child Agents Must Not Inherit Accidental Parent State

Subagents should inherit only the state TS intentionally passes:

- model or model override
- permission mode according to inheritance rules
- allowed tools according to agent definition and parent constraints
- selected MCP servers
- file/read/task state where TS shares it
- fork context messages only for fork mode
- session id where required for transcript/plan-file grouping

They must not accidentally reuse parent mutable context, pending tool ids, or
permission prompts without explicit wiring.

### I12. Result Ordering: History In Completion Order; State Mutation In Model Order

This aligns with the TS **non-streaming concurrent path**, which is
the authoritative completion-order reference: `runToolsConcurrently`
feeds tool generators into `all()` at `utils/generators.ts:31`, which
yields via `Promise.race` in the actual order futures resolve, and
the caller appends to history in that completion order. Concurrent
context modifiers apply **after** the batch in model order
(`toolOrchestration.ts:54-62`). Rust enforces the same split.

Note on TS streaming: `StreamingToolExecutor.getCompletedResults()`
(`StreamingToolExecutor.ts:412-440`) iterates `this.tools` in model
order and only yields tools whose `status === 'completed'`, skipping
still-executing concurrent-safe tools; `Promise.race` at
`StreamingToolExecutor.ts:481` merely wakes the async generator when
any tool finishes. The resulting yield sequence for a concurrent-safe
batch is *emergent* completion order (slow earlier tools are skipped
until they finish), not a direct "yield on completion" — so this file
is NOT the TS parity reference. The Rust streaming executor tightens
this into an explicit completion-stream contract. Use
`FuturesUnordered` over in-process futures plus the existing
max-concurrency semaphore by default; do not use `JoinSet` /
`tokio::spawn` unless the runner is first made `Arc` + `'static`.
This gives real `completion_seq` ordering rather than emergent
ordering without imposing unnecessary `'static` bounds on borrowed
runner state.

Two orthogonal ordering axes — never collapse them into one key:

1. **Model-visible message history (`tool_result` and `new_messages`):**
   - **Concurrent-safe batch:** completion order. As each `run_one`
     future resolves, its `ToolCallOutcome` is appended to history. A
     slow earlier tool does not block a faster later tool — that is
     the whole point of concurrent scheduling.
   - **Serial unsafe tool:** execution order, which equals model order
     by construction (one tool runs at a time).
   - **Streaming (Phase 9):** when a tool finishes before the
     assistant message commits, defer the history-append callback.
     After commit, flush queued outcomes in their **real** completion
     sequence (the order observed during streaming), not a re-derived
     model order.

2. **Shared state mutation (`app_state_patch` / context modifiers):**
   - **Concurrent-safe batch:** collect each completed outcome's patch
     keyed by `model_index`. After the batch's last future completes,
     apply patches in `model_index` order under one write lock. TS
     parity: `toolOrchestration.ts:54-62` iterates the original
     `blocks` array (model order) and runs each queued modifier. Two
     concurrent tools therefore never observe each other's mutations,
     and the post-batch state is deterministic regardless of which
     tool finished first.
   - **Serial unsafe tool:** apply the patch immediately on completion,
     before the next tool's `ToolUseContext` is built. TS parity:
     `toolOrchestration.ts:130-141`.

3. **Stream/protocol events (per-event order, not a single ordering):**
   - `ToolUseQueued` — model order, emitted at assistant-message
     commit time for every committed call (Runnable + EarlyOutcome).
     Used for `app_state` / context modifier application. JSON-parse
     drops never reach this event.
   - `ToolUseStarted` — execution-start order, emitted **only for
     Runnable plans**; concurrent batch starts may interleave.
     EarlyOutcome calls produce no Started event.
   - `ToolUseCompleted` — completion order, emitted for every
     committed call. For `Runnable` it fires when `run_one` resolves;
     for `EarlyOutcome` it fires the moment the executor reaches that
     plan's barrier block in partition order (not globally before all
     Runnables), carrying the synthetic error outcome. SDK consumers
     see tools "ticking off" as they finish.

   See I14 for the per-call invariant (one Queued → exactly one
   Completed; Started is conditional). The three events have three
   different orderings and must not be conflated.

Two distinct ordering keys must travel with each call so the axes do
not drift back together:

- `model_index: usize` — the tool_use position within the assistant
  message. Used for: emitting `ToolUseQueued`, looking up per-call
  hook/permission state, applying `app_state_patch` after a
  concurrent-safe batch, telemetry correlation.
- `completion_seq: usize` — assigned monotonically by the executor as
  each outcome becomes available. Used for: appending the
  `ToolCallOutcome` to message history. For `Runnable` plans the seq
  is stamped when the `run_one` future resolves; for `EarlyOutcome`
  plans the seq is stamped when the executor processes that plan's
  block (i.e. when the barrier is reached in partition order, NOT
  globally before all Runnable plans). A schema-invalid EarlyOutcome
  sitting between two safe batches therefore gets a `completion_seq`
  that falls between the surrounding batches' completion seqs, in the
  partition traversal order.

A single `slot_index` field that conflates the two keys is a bug.
Whatever value drives history append is `completion_seq`; whatever
value drives `ToolUseContext` setup or patch application is
`model_index`.

Algorithm:

```text
prepare_batch assigns each PreparedToolCall a model_index (= position
  in the assistant tool_use list)
executor partitions plans into concurrent-safe batches and serial
  unsafe tools (TS toolOrchestration.ts:91-115 partitionToolCalls).
  Schema-invalid plans (EarlyOutcome::SchemaFailed) are NOT
  concurrency-safe; they form a single-tool barrier that breaks the
  preceding and following safe batches.
for each concurrent-safe batch the executor uses a completion stream
  (`FuturesUnordered` plus the existing max-concurrency semaphore) — NOT
  `for handle in handles { handle.await }`, which is submission order
  and blocks the whole batch on the slowest tool.
as each run_one future completes within the batch:
  executor stamps completion_seq (next monotonic value within the turn)
  emits ToolUseCompleted (in completion order)
  hands the outcome to the runner's history-append callback
    (history grows in completion order)
  queues the patch keyed by model_index (do NOT apply yet)
once the batch's last future resolves:
  iterate queued patches in model_index order; apply each under one
  write lock (TS parity)
for serial unsafe tools:
  await the single run_one future
  emit ToolUseCompleted with stamped completion_seq
  append outcome to history
  apply patch immediately, then build the next tool's context
streaming (Phase 9) starts run_one as soon as input arrives, but
  defers the history-append callback until the assistant message
  commits; on commit, flush queued completed outcomes in their
  real completion_seq order
```

Why this matches the TS non-streaming concurrent split and defines
Rust streaming explicitly: TS's observable non-streaming contract (via
`utils/generators.ts:31` `all()` + `Promise.race`) is that history
append order is completion order, while state mutation order is model
order for safe batches and execution order for serial. TS streaming
(`StreamingToolExecutor.getCompletedResults()` at
`services/tools/StreamingToolExecutor.ts:412-440`) emergently matches
this via model-order iteration + skip-if-executing, but is not the
reference — Rust instead makes the completion-stream contract
explicit. Conflating these axes
was the previous draft's mistake. With the two ordering keys
travelling separately, each axis is independently deterministic and
testable.

Document the split in code comments referencing this section. Without
this distinction written down, two implementers will diverge.

### I13. Fallback Switches Reset Provider Cache State

When `ModelRuntime` switches to `fallback_model`, prompt-cache breakpoints
are provider-specific. The runtime must:

- reset `CacheBreakDetector` state
- clear any pending cache pointers
- reset `UsageAccumulator` cache-hit/miss counters where they would be
  attributed to the wrong provider
- invalidate any provider-specific stream state that might be reused

This is a correctness requirement, not an optimization.

### I14. SDK NDJSON Stream Order Is A Public Contract

SDK consumers depend on the order of `ToolUseQueued`, `ToolUseStarted`,
`ToolUseCompleted` events. Phase 1 changes when these fire (eager dispatch
deletion shifts `ToolUseStarted` to after assistant-message commit). The
new contract:

```text
TurnStarted
  ToolUseQueued    (per committed call, in model order — emitted for
                    EarlyOutcome calls too: unknown-tool, schema
                    failure. JSON-parse-failure entries are pre-commit
                    drops and emit no Queued event — see "JSON Parse
                    Failures Are Pre-Commit, Not Pre-Batch" under
                    `ToolCallRunner`.)
  ToolUseStarted   (per Runnable call only, in execution start order;
                    concurrent batch starts may interleave but each
                    Runnable call appears exactly once. EarlyOutcome
                    calls have no Started event.)
  ToolUseCompleted (per committed call, in completion order — for
                    Runnable plans when `run_one` resolves; for
                    EarlyOutcome plans when the executor reaches that
                    plan's barrier block in partition order (NOT
                    globally before all Runnables), carrying the
                    synthetic error outcome. Invariant: one Queued
                    → exactly one Completed.)
TurnCompleted
```

Per-call invariant: every `ToolUseQueued` is followed by exactly one
`ToolUseCompleted` within the same turn. `ToolUseStarted` is conditional
(only Runnable plans).

Document the contract change in the PR description for downstream
consumers.

## Target Architecture

### High-Level Flow

```text
User input
  -> QueryEngine::run_session_loop
  -> preprocess input, reminders, attachments
  -> ModelRuntime::stream_or_generate
  -> collect assistant text, reasoning, tool calls
  -> append assistant message
  -> ToolCallRunner::run_tool_calls
  -> append returned messages
  -> finalize turn
  -> continue or stop
```

`QueryEngine` should own the loop, not the details of each tool.

### Target Module Layout

```text
coco-rs/app/query/src/
  engine.rs
    High-level loop only.

  tool_runner.rs
    ToolCallRunner, ToolInvocation, ToolRunOutput.
    Owns tool lifecycle and result-message construction.

  tool_context.rs
    ToolContextFactory.
    Builds ToolUseContext snapshots from config, history, app state, and injected handles.

  permission_controller.rs
    PermissionController.
    Owns check_permissions, hook overrides, auto-mode classifier, permission bridge.

  hook_adapter.rs
    QueryHookHandle.
    Implements coco_tool::HookHandle by calling coco_hooks::orchestration.

  model_runtime.rs
    ModelRuntime.
    Owns stream/generate, fallback model, max-output recovery, streaming retry.

  agent_adapter.rs
    QueryEngineAdapter.
    Runs child QueryEngine instances and returns real AgentQueryResult data.

  skill_runtime.rs
    Optional if SkillRuntime lives in app/query.
    Resolves skills and returns inline/fork execution plans.
```

Some teams may prefer putting `AgentRuntime` and `SkillRuntime` under
`app/state` or a new `app/agent` crate-level module. The important constraint is
dependency direction:

```text
coco-tool defines traits
coco-tools calls traits
app/query or app/state implements traits
```

`core/tools` must not depend on `app/query` or `app/state`.

### Ownership Matrix

| Concern | Owner | Inputs | Outputs |
|---------|-------|--------|---------|
| Prompt construction | `QueryEngine` with context helpers | history, system prompt config, reminders, attachments | `Vec<LlmMessage>` |
| Tool catalog construction | `QueryEngine` helper or `ToolCatalogBuilder` | tool registry, `PromptOptions` | provider-agnostic tool catalog keyed by `ToolId` |
| Provider wire preparation | `app/query::ModelRuntime` / `app/query::tool_wire` adapter | tool catalog, active provider/model policy | `LanguageModelV4Tool` wire definitions + reverse map |
| Model call and retry | `ModelRuntime` | prompt, tool catalog, token config | assistant content, typed tool calls, usage, stop reason |
| Tool-call lifecycle | `ToolCallRunner` | tool invocations, history snapshot, context factory | messages, denials, continuation stop |
| Tool scheduling | `StreamingToolExecutor` | validated runnable tool jobs | ordered tool execution results |
| Permission | `PermissionController` | tool, effective input, context, hook override | allow/deny with audit metadata |
| Hook execution | `QueryHookHandle` | hook registry, orchestration context, tool input/output | hook outcomes |
| Context creation | `ToolContextFactory` | config, app state, history, handles | `ToolUseContext` |
| Agent execution | `AgentRuntime` | `AgentSpawnRequest`, parent context | `AgentSpawnResponse` |
| Skill execution | `SkillRuntime` | skill name, args, parent context | inline messages or fork result |
| App-state mutation | `StreamingToolExecutor` | `ToolResult::app_state_patch` | updated app state, task panel event |
| Compaction | `QueryEngine` + `coco-compact` | history, usage, context window | compacted history |

### Dependency Direction

The desired dependency direction is:

```text
common/types
  -> core/tool traits
  -> core/tools concrete tools
  -> app/query runtimes and adapters
  -> app/cli wiring
```

Important dependency rules:

- `coco-tool` may define `AgentHandle`, `SkillHandle`, `HookHandle`, and
  `ModelRuntime` traits or DTOs, but it should not depend on `coco-hooks`,
  `coco-state`, `coco-skills`, or provider-specific crates.
- `coco-tools` may call handles from `ToolUseContext`, but it should not know
  whether an agent is in-process, tmux, worktree, or remote.
- `app/query` may depend on hooks, permissions, messages, compact, inference,
  and tool traits because it is the orchestration layer.
- `app/state` may implement agent runtime details and swarm state, but should
  not duplicate query-loop semantics.
- CLI and SDK runners should wire concrete handles. They should not rebuild
  alternate query-loop behavior.

## Component Design

### QueryEngine After Refactor

`QueryEngine` keeps:

- session bootstrap
- history ownership
- command queue and inbox draining
- reminder and attachment injection
- model turn loop
- assistant message construction
- call into `ToolCallRunner`
- compaction decisions
- final `QueryResult`

`QueryEngine` stops owning:

- direct tool lookup
- direct validation
- direct permission logic
- direct hook execution
- direct app-state patch application
- direct eager tool execution
- direct AgentTool and SkillTool runtime behavior
- direct fallback model switching details

The intended `engine.rs` flow:

```rust
let prompt = self.build_prompt(&history).await?;
let tool_catalog = self.build_tool_catalog().await?;
let model_output = self.model_runtime.run_turn(prompt, tool_catalog).await?;

history.push(model_output.assistant_message);

if model_output.tool_calls.is_empty() {
    return self.finish_or_continue_without_tools(...).await;
}

let tool_output = self.tool_runner
    .run_tool_calls(model_output.tool_calls, &history, ToolRunOptions { ... })
    .await;

history.extend(tool_output.messages);
permission_denials.extend(tool_output.permission_denials);

if tool_output.prevent_continuation.is_some() {
    stop_or_emit_blocking_reason();
}
```

### ToolCallRunner

`ToolCallRunner` is the central piece of this refactor.

Suggested data types:

```rust
pub struct ToolInvocation {
    pub tool_use_id: String,
    pub target: ToolInvocationTarget,
    pub input: serde_json::Value,  // already-parsed model input; see "JSON Parse Failures" below
}

pub enum ToolInvocationTarget {
    /// Provider wire name was translated through the per-turn reverse map.
    Known(ToolId),
    /// Provider returned a tool name not present in the prepared map.
    /// Keep the raw wire name only for the synthetic UnknownTool message.
    Unknown { raw_wire_name: String },
}

pub struct ToolRunOptions {
    pub user_message_id: Option<String>,
    pub turn_id: String,
    pub scheduling: ToolScheduling, // Streaming | NonStreaming, not a bool
}

pub enum ToolScheduling {
    NonStreaming,
    Streaming,
}

pub struct ToolRunOutput {
    pub messages: Vec<coco_types::Message>,
    pub permission_denials: Vec<coco_types::PermissionDenialInfo>,
    pub prevent_continuation: Option<String>,
    pub tool_use_count: i64,
}
```

Ownership split:

- `prepare_batch` is the SOLE owner of: resolving `ToolId` to
  `Arc<dyn Tool>` through `ToolRegistry`,
  schema validation, `model_index` assignment (the tool_use position
  within the assistant message per I12 — NOT a history-append slot;
  that is `completion_seq`, stamped later by the executor). Any
  failure here produces a `ToolCallPlan::EarlyOutcome` that the
  executor passes through unchanged. `run_one` never sees an
  unresolved tool. Raw provider strings have already been translated by
  `ModelRuntime`; the only string that can reach `prepare_batch` is the
  raw wire name inside `ToolInvocationTarget::Unknown`, used solely for
  the synthetic error message.
- `run_one` is the SOLE owner of the per-tool semantic lifecycle
  starting from a fully-prepared, runnable job.

**JSON Parse Failures Are Pre-Commit, Not Pre-Batch.** By the time the
runner sees a `ToolInvocation`, JSON parsing has already happened
upstream: vercel-ai's `ToolCallPart.input` is `JSONValue`
(`coco-rs/vercel-ai/provider/src/content.rs:131`), and the streaming
accumulator parses raw bytes into `serde_json::Value` at
`engine.rs:1592-1606` before constructing `ToolCallPart` (a parse error
there `continue`s the loop and the call is never committed). The same
holds for the non-streaming path. So:

- A model-emitted tool call that fails JSON parsing is **dropped before
  commit** — it never appears in history, the assistant message, or the
  lifecycle event stream. No `ToolUseQueued` / `Started` / `Completed`
  are emitted because there is no committed call to attach them to.
- `prepare_batch` therefore does not need an `InvalidJson` `EarlyOutcome`
  variant. The `EarlyOutcome` set is `UnknownTool`, `SchemaFailed`,
  and any other **pre-execution** failure that is decided after the call
  has been committed by the assistant message but before per-tool
  serial work (hooks / permission / execute) runs. (Truly pre-commit
  failures like JSON parse errors never reach `prepare_batch` at all —
  they are dropped in the accumulator.)
- The unrecoverable-parse case is not silent: the streaming accumulator
  must `warn!` (already does, `engine.rs:1598-1604`) and the assistant
  message commit must record the dropped tool_use_id for telemetry. We
  do not fabricate a synthetic tool result — TS does not, and adding one
  would require a Queued/Completed pair for a call no normalization step
  ever sees.

If a future requirement forces parse failures to become model-visible
errors (e.g., to keep the assistant message contract `tool_use_count ==
tool_result_count`), the right shape is to widen the runner input to
something like `enum RawToolInput { Parsed(Value), Raw(String) }` and
move parsing into `prepare_batch`. We do **not** do that today.

Responsibilities (per `run_one`, executed serially in fresh context — see
Scheduling Contract for what may be batched up front):

- Consume an already-resolved `PreparedToolCall` (tool, parsed_input,
  model_index). No name lookup, no JSON parse, no **initial** schema
  check here — those are `prepare_batch`'s responsibility. `run_one`
  DOES re-run the cached `ToolSchemaValidator` against hook-rewritten
  input (see the per-tool serial step list below).
- Build `ToolUseContext` for this call (sees prior tools' app_state
  mutations).
- Run `tool.validate_input(&Value, &ToolUseContext)` — context-dependent;
  cannot be moved to batched preparation.
- Strip model-controlled internal fields after validation (defense in depth).
- Run `PreToolUse` through `HookHandle`.
- Apply hook input rewrite; re-validate hook-updated input.
- Resolve permission through `PermissionController`.
- Invoke `tool.execute()` directly within `run_one`. Per the Scheduling
  Contract, the executor calls `run_one` as a callback (TS pattern at
  `StreamingToolExecutor.ts:320` where `executeTool()` calls `runToolUse()`
  inline). The runner does NOT submit calls to a queue; it IS the work
  the executor schedules.
- Stream lifecycle events are owned by the executor (see "Lifecycle
  Event Ownership" below). The runner does NOT emit `ToolUseQueued`,
  `ToolUseStarted`, or `ToolUseCompleted`; it only builds the semantic
  outcome.
- Convert `ToolResult<Value>` or `ToolError` into `ToolMessageBuckets` with
  MCP-aware ordering per I5.
- Respect hook `prevent_continuation`.
- Build an `UnstampedToolCallOutcome` whose `effects.app_state_patch`
  carries the patch. The runner does NOT stamp `completion_seq` and
  does NOT apply the patch. The executor calls
  `stamp_and_extract_effects(next_seq)` at surface time, splits the
  unstamped body into a patch-free `ToolCallOutcome` (handed to
  `on_outcome` immediately for history append) and a `ToolSideEffects`
  the executor applies at the right moment (serial: before the next
  tool's context build; concurrent batch: end-of-batch under one write
  lock, in `model_index` order). Patch ownership never moves through
  `on_outcome`. Errors drop the patch per I9 — the `FnOnce` destructor
  releases captured state without invoking.

Important: `ToolCallRunner` should produce messages. `QueryEngine` should not
rebuild tool result messages itself.

#### ToolCallRunner Internal Structure

The runner is sequential code with `tracing` spans per stage — not a runtime
state machine. A `ToolCallStage` enum was considered and **rejected**: it
adds ceremony without enabling resumption, partial recovery, or anything
beyond what `#[tracing::instrument(name = "stage_name")]` already provides.

If a future need for runtime stage tracking emerges (e.g. external pause/
resume, audit log replay), introduce the enum then with concrete users.

Suggested prepared-call type — preparation can fail (unknown tool,
schema validation failure). Those failures must still produce one tool
result per I1, so preparation returns a `ToolCallPlan` that either
carries a runnable job or a pre-built early outcome. JSON parse failure
is **not** an `EarlyOutcome` source — it is a pre-commit failure handled
by the assistant-message accumulator (see "JSON Parse Failures Are
Pre-Commit, Not Pre-Batch" below).

**Crate location (layer rule, not a style preference)**: the scheduler
DTOs — `ToolCallPlan`, `PreparedToolCall`, `UnstampedToolCallOutcome`,
`ToolCallOutcome`, `RunOneRuntime`, `ToolMessagePath`,
`ToolCallErrorKind` — live in **`coco-tool`** (the interface crate),
not in `coco-query`. Runner-local message assembly helpers such as
`ToolMessageBuckets` and `ToolMessageOrder` live in
`app/query/src/tool_message.rs` because the executor never inspects
them; only the flattened `ordered_messages` and `ToolMessagePath`
cross the scheduler boundary. `coco-query` already depends on `coco-tool`
(`coco-rs/app/query/Cargo.toml:14`), but `coco-tool` MUST NOT depend
on `coco-query` (layer rule in root `CLAUDE.md` — L3 cannot depend on
L5; also `coco-rs/docs/coco-rs/CLAUDE.md` "Circular Dependency
Prevention"). Placing the DTOs in `coco-query` would force
`StreamingToolExecutor::execute_with` (which lives in `coco-tool`) to
reference `coco-query` types, inverting the dependency. The only other
acceptable placement would be full generic parameters on
`execute_with` (opaque `Job` / `Outcome` traits, with stamping done via
a `stamp` callback) — but that adds ceremony with no benefit because
the executor must already understand `ToolCallPlan::EarlyOutcome` to
treat schema-invalid plans as barriers, and barrier semantics require
peeking at the concrete enum variant. Concrete DTOs in `coco-tool` is
the path taken. `coco-query` provides the `ToolCallRunner`
implementation that produces `ToolCallPlan` values and consumes
`ToolCallOutcome` values; the runner lives in `app/query/src/tool_runner.rs`,
but the types it traffics in are `coco-tool`'s.

```rust
/// What `prepare_batch` returns per assistant tool_use entry.
pub enum ToolCallPlan {
    /// Tool resolved, schema validated. Ready for run_one.
    Runnable(PreparedToolCall),
    /// Preparation failed (unknown tool / schema failure).
    /// Outcome carries a synthetic error result but is UNSTAMPED —
    /// `completion_seq` is assigned by the executor when the
    /// partitioner reaches this plan's barrier block, at which point
    /// the unstamped outcome becomes a `ToolCallOutcome` and is
    /// handed to `on_outcome`. `prepare_batch` does not know the
    /// completion order yet, so it cannot construct `ToolCallOutcome`
    /// directly.
    EarlyOutcome(UnstampedToolCallOutcome),
}

/// Effect-free, context-free preparation only. Stores resolved tool,
/// original invocation (with a `Known(ToolId)` target), and
/// schema-validated input. Does NOT store
/// ToolUseContext, hook results, permission decisions, or
/// `tool.validate_input()` results — those depend on `&ToolUseContext`
/// and must be computed serially during execution (see "Scheduling
/// Contract" for why batched semantic prep is unsafe for serial tools).
pub struct PreparedToolCall {
    pub invocation: ToolInvocation,
    pub tool: Arc<dyn Tool>,
    pub parsed_input: serde_json::Value,  // already parsed upstream (engine.rs:1592-1606);
                                          // post-SCHEMA-validation here
                                          // (NOT tool.validate_input — that runs in run_one)
    pub model_index: usize,               // tool_use position in the assistant message
                                          // (per I12). Drives ToolUseContext lookup,
                                          // app_state_patch ordering, telemetry. NOT
                                          // history append order — that uses
                                          // completion_seq, stamped by the executor.
}
```

Hook context, permission audit, and the live `ToolUseContext` are
constructed **per-tool** at execution time, not at preparation time, so
they observe app_state mutations from prior serial tools.

The executor receives a per-tool runtime that owns the scheduler-only
state — cancellation token, sibling-abort signal, progress channel,
and `model_index` (the tool_use position per I12, used for patch
ordering and telemetry — NOT a history-append slot; a single
`slot_index` that conflates model order and completion order is a bug
per I12). The runner gets it through the callback so its semantic
lifecycle remains single-source:

```rust
/// Scheduler-owned per-tool runtime. The executor builds one of these
/// per call and hands it to the runner via the callback.
pub struct RunOneRuntime {
    pub cancellation: CancellationToken,   // child of turn token (per I10)
    pub sibling_abort: SiblingAbortSignal,  // shell-failure broadcast
    pub progress_tx: ProgressSender,       // forwarded into ToolUseContext
    pub model_index: usize,                // matches PreparedToolCall.model_index;
                                           // completion_seq is NOT here — it is
                                           // stamped by the executor on the returned
                                           // outcome, not visible to run_one
}
```

Suggested outcome types — the runner builds all I5 buckets explicitly,
then flattens them exactly once while it still holds the resolved
`Arc<dyn Tool>`. The history-facing outcome carries only the already
ordered message block. This keeps the MCP/non-MCP ordering decision
inside the runner and avoids leaking bucket-flattening responsibility
back into `QueryEngine` or the executor.

The outcome is split in two so that "completion_seq is assigned by the
executor" is enforced by the type system, not by convention:

```rust
/// What `prepare_batch` and `run_one` produce. Carries every
/// field of the final outcome EXCEPT `completion_seq`, which only the
/// executor knows how to assign (it is the monotonic completion-order
/// sequence within the turn). Constructable without knowing the
/// turn-wide completion order.
pub struct UnstampedToolCallOutcome {
    pub tool_use_id: String,
    pub tool_id: ToolId,
    pub model_index: usize,                // tool_use position per I12
    /// Pre-flattened, TS-ordered message stream — already resolved via
    /// `ToolMessageBuckets::flatten(ToolMessageOrder::for_tool(&*tool))`
    /// while the runner still holds `Arc<dyn Tool>`. QueryEngine
    /// appends this verbatim; it MUST NOT re-resolve the tool, re-sort,
    /// or re-run the flatten template. If a future caller needs
    /// per-bucket telemetry, add runner-local telemetry before
    /// flattening; do not carry owned buckets across the scheduler
    /// boundary just to re-derive message order later.
    pub ordered_messages: Vec<Message>,
    /// Lifecycle path retained for telemetry/tests. The full buckets are
    /// runner-local only; they are consumed by `flatten` and never cross
    /// the scheduler boundary.
    pub message_path: ToolMessagePath,
    pub error_kind: Option<ToolCallErrorKind>,
    pub permission_denial: Option<PermissionDenialInfo>,
    pub prevent_continuation: Option<String>,
    /// Side-effects that must cross the scheduler boundary.
    /// Separated from history-facing data because `AppStatePatch` is
    /// `Box<dyn FnOnce(&mut ToolAppState) + Send + Sync>` (see
    /// `common/types/src/app_state.rs:277`) — a single owned value
    /// that cannot simultaneously ride with the outcome into
    /// `on_outcome` (history) AND stay in the executor for later
    /// `apply` (state mutation). `stamp_and_extract_effects` splits
    /// them: `ToolSideEffects` stays inside the executor; the
    /// history-facing `ToolCallOutcome` is patch-free by construction.
    pub effects: ToolSideEffects,
}

/// Scheduler-facing side-effects moved out of the outcome body at
/// surface time. Discarded (effects dropped, never applied) on error
/// per I9 — drop runs the `FnOnce` destructor without invoking it.
pub struct ToolSideEffects {
    pub app_state_patch: Option<AppStatePatch>,
    // Future effects (pending cache invalidations, telemetry
    // side-channels, etc.) live here — they do NOT leak into the
    // history-facing outcome.
}

impl UnstampedToolCallOutcome {
    /// Surface-time operation owned by the executor. Splits the
    /// outcome into:
    ///   1. A history-facing `ToolCallOutcome` (patch-free; safe to
    ///      hand to `on_outcome` the moment the future resolves).
    ///   2. `ToolSideEffects` the executor keeps until the right
    ///      application moment (serial: before the next tool's
    ///      `ToolUseContext`; concurrent-safe batch: under one write
    ///      lock at end-of-batch, iterated in `model_index` order).
    ///
    /// Visibility is `pub(crate)` within `coco-tool` so only the
    /// executor (same crate) can call it. `ToolCallRunner` in
    /// `coco-query` returns the unstamped body and never sees this
    /// method.
    pub(crate) fn stamp_and_extract_effects(
        self,
        completion_seq: usize,
    ) -> (ToolCallOutcome, ToolSideEffects) {
        let UnstampedToolCallOutcome {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            effects,
        } = self;
        let outcome = ToolCallOutcome {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            completion_seq,
        };
        (outcome, effects)
    }
}

/// The final outcome delivered to `on_outcome`. Fields are **private**;
/// constructor lives only in `stamp_and_extract_effects` (also
/// `pub(crate)`). External crates cannot fabricate or mutate a
/// `ToolCallOutcome`, which makes "executor-only stamping" a
/// type-system guarantee — not a documentation convention. Readers
/// use the explicit accessors below.
///
/// This is patch-free by construction: any `AppStatePatch` that was
/// in the unstamped body lives in `ToolSideEffects`, held by the
/// executor until the correct apply moment.
pub struct ToolCallOutcome {
    tool_use_id: String,
    tool_id: ToolId,
    model_index: usize,
    /// Pre-flattened, TS-ordered messages — the runner ran
    /// `ToolMessageBuckets::flatten(ToolMessageOrder::for_tool(&*tool))`
    /// while it still held `Arc<dyn Tool>`, so QueryEngine appends
    /// this verbatim. `ToolMessagePath` is captured alongside for
    /// telemetry/test assertions, but buckets are not retained here.
    ordered_messages: Vec<Message>,
    /// Retained for structured telemetry and tests that need to know
    /// which lifecycle path produced the ordered messages. Do not use
    /// this to re-derive message order.
    message_path: ToolMessagePath,
    error_kind: Option<ToolCallErrorKind>,
    permission_denial: Option<PermissionDenialInfo>,
    prevent_continuation: Option<String>,
    completion_seq: usize,
}

impl ToolCallOutcome {
    pub fn tool_use_id(&self) -> &str { &self.tool_use_id }
    pub fn tool_id(&self) -> &ToolId { &self.tool_id }
    pub fn model_index(&self) -> usize { self.model_index }
    pub fn completion_seq(&self) -> usize { self.completion_seq }
    /// History append: consumers iterate this directly.
    pub fn ordered_messages(&self) -> &[Message] { &self.ordered_messages }
    /// Structured view for telemetry / tests only; never re-flatten.
    pub fn message_path(&self) -> ToolMessagePath { self.message_path }
    pub fn error_kind(&self) -> Option<&ToolCallErrorKind> { self.error_kind.as_ref() }
    pub fn permission_denial(&self) -> Option<&PermissionDenialInfo> {
        self.permission_denial.as_ref()
    }
    pub fn prevent_continuation(&self) -> Option<&str> {
        self.prevent_continuation.as_deref()
    }

    /// Destructure into owned parts (history-append consumes
    /// `ordered_messages`).
    /// Does NOT expose any `AppStatePatch` — patches live in
    /// `ToolSideEffects`, not here.
    pub fn into_parts(self) -> ToolCallOutcomeParts { /* ... */ }
}

/// `ToolCallOutcome` doesn't implement `Deref` — Rust style reserves
/// `Deref` for smart-pointer / borrow-wrapper types (`Arc`, `Ref`,
/// `String`), not plain data wrappers, because `Deref` muddles method
/// resolution. Read access goes through the accessor methods above.

/// Which lifecycle path produced this bucket. The flatten algorithm
/// branches on this — MCP deferred-post-hook ordering ONLY applies to
/// the success path; failure and early-return paths use a single
/// canonical order regardless of `is_mcp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMessagePath {
    /// `tool.execute()` ran to completion. Post-hook is `PostToolUse`.
    /// MCP success diverges from non-MCP success per I5.
    Success,
    /// `tool.execute()` threw. Post-hook is `PostToolUseFailure`
    /// (TS `toolExecution.ts:1696` inside the catch block at :1589).
    /// TS returns `[error tool_result, ...failure hook messages]` at
    /// :1715–:1750 — MCP deferred logic does not apply here.
    Failure,
    /// Unknown tool / schema / pre-hook stop / permission denial.
    /// (JSON parse failure is NOT here — it's a pre-commit drop in
    /// the accumulator and never produces a ToolMessageBuckets.)
    /// No post-hook ran; only pre_hook (if any emitted) + synthetic
    /// error tool_result (per I3 step 12).
    EarlyReturn,
}

/// Runner-local helper in `app/query/src/tool_message.rs`.
/// It is consumed before the scheduler boundary; do not store this
/// in `ToolCallOutcome`.
pub struct ToolMessageBuckets {
    pub pre_hook: Vec<Message>,
    pub tool_result: Message,              // exactly one
    pub new_messages: Vec<Message>,        // tool's result.new_messages (Success only)
    pub post_hook: Vec<Message>,           // PostToolUse (Success) OR PostToolUseFailure (Failure); empty on EarlyReturn
    pub prevent_continuation_attachment: Option<Message>,
    pub path: ToolMessagePath,
}

/// Runner-local helper in `app/query/src/tool_message.rs`.
/// Selects the success-path flatten template. Non-MCP emits post_hook
/// inline between tool_result and new_messages; MCP defers post_hook to
/// after new_messages + prevent (TS toolExecution.ts:1499 vs :1585). The
/// Failure / EarlyReturn paths use a single canonical order regardless
/// of tool kind, so this enum is consulted only when `path == Success`.
///
/// Use a typed enum (not `bool`) so call sites cannot accidentally
/// invert true/false; matches the doc's "Prefer Typed Results Over
/// Booleans" rule.
pub enum ToolMessageOrder {
    NonMcp,
    Mcp,
}

impl ToolMessageOrder {
    /// Resolve from the running tool itself, so call sites use the
    /// same source of truth that the runner branches on (`tool.is_mcp()`,
    /// per "Constraints" above and TS `isMcp` property). Avoids drift
    /// between `ToolId::Mcp` discriminant and actual tool metadata
    /// (e.g. a custom MCP wrapper exposed under a non-MCP `ToolId`,
    /// or vice versa).
    pub fn for_tool(tool: &dyn Tool) -> Self {
        if tool.is_mcp() { Self::Mcp } else { Self::NonMcp }
    }
}

impl ToolMessageBuckets {
    /// Flatten in TS-correct order. `path` discriminates which
    /// emission template applies; `order` only affects Success.
    ///
    /// Success, NonMcp:  pre_hook, tool_result, post_hook, new_messages, prevent
    /// Success, Mcp:     pre_hook, tool_result, new_messages, prevent, post_hook
    /// Failure (any):    pre_hook, tool_result, post_hook_failure
    ///                   (TS catch block at toolExecution.ts:1715–1737 returns
    ///                    [error tool_result, ...hookMessages]; the success-block
    ///                    prevent append at :1572 is BYPASSED on exception, and
    ///                    the MCP deferred logic also does not apply.
    ///                    `prevent_continuation_attachment` MUST be None here —
    ///                    enforce in the runner.)
    /// EarlyReturn:      pre_hook, tool_result
    ///                   (no post_hook ran; prevent_continuation requires a
    ///                    successful pre-hook → execute path)
    pub fn flatten(self, order: ToolMessageOrder) -> Vec<Message> { ... }
}
```

The flat `result_message + new_messages` shape from earlier drafts cannot
represent the I5 buckets correctly. The runner internally constructs
`ToolMessageBuckets`, resolves `ToolMessageOrder::for_tool(&*tool)` while
it still holds `Arc<dyn Tool>`, flattens once, and returns the resulting
`ordered_messages: Vec<Message>` in `UnstampedToolCallOutcome`. The engine
appends the block verbatim. Ownership is one-way: only the runner ever
touches buckets + order; QueryEngine never re-resolves the tool or
re-runs flatten. (Earlier drafts left ordering resolution implicit on
the engine side; this is the fix.)

`ToolCallRunner::run_one` should have a narrow shape:

```rust
async fn run_one(
    &self,
    job: PreparedToolCall,
    runtime: RunOneRuntime,
) -> UnstampedToolCallOutcome
```

Note the return type: the runner does NOT stamp `completion_seq` and
does NOT split out side effects — the executor does both, the moment
the `run_one` future resolves, via
`UnstampedToolCallOutcome::stamp_and_extract_effects(next_seq)`. The
same call consumes `EarlyOutcome`'s unstamped body when the
partitioner reaches its barrier block. This keeps the two-axis split
compile-enforced (`model_index` travels with the call from
`prepare_batch` onward; `completion_seq` is type-injected at surface
time) AND keeps patch ownership on the executor's side of the boundary
— the history-facing `ToolCallOutcome` is patch-free by construction.

It should not return `Result<UnstampedToolCallOutcome, _>` for normal tool-call errors.
Unknown tools, invalid input, hook blocks, denials, execution failures, and
cancellations are successful runner outcomes that contain model-visible error
tool results. Reserve `Result::Err` for infrastructure failures that prevent the
runner from building any valid message. Even then, the caller should convert the
failure into a synthetic tool result for every committed tool call.

The runner should include a small `ToolCallMessageBuilder` helper that owns:

- success result message construction
- synthetic error result message construction
- hook additional-context message construction
- parent tool-use id tagging for `new_messages`
- stable result ordering

This avoids reintroducing scattered message creation in `engine.rs`,
`executor.rs`, or tool implementations.

#### Message Commit Model

Per I5, each tool call produces messages in 6 buckets. Within a single tool
call, the bucket order depends on whether the tool is MCP:

**Non-MCP tool, success path** (post-hook messages emit INLINE between
result and newMessages):

```text
[pre-hook messages]                          (emitted as collected)
tool_result for tool_use_id X                (emitted at toolExecution.ts:1478)
[post-hook messages + additional_contexts]   (emitted INLINE at :1515)
[tool's result.new_messages]                 (emitted at :1566)
[prevent_continuation attachment, if any]
```

**MCP tool, success path** (post-hook collection runs first to allow output
rewrite; hook messages are DEFERRED to after newMessages and after
prevent_continuation):

```text
[pre-hook messages]
                                             (post-hook collection at :1499,
                                              may rewrite tool output via
                                              updatedMCPToolOutput;
                                              hook MESSAGES held in hookResults,
                                              not emitted yet)
tool_result for tool_use_id X                (emitted at :1541, possibly-rewritten)
[tool's result.new_messages]                 (emitted at :1566)
[prevent_continuation attachment, if any]    (emitted at :1572)
[post-hook messages + additional_contexts]   (deferred hookResults flushed at :1585)
```

**The two branches diverge** at the post-hook loop. `ToolMessageBuckets::flatten`
takes a typed `ToolMessageOrder` (not `bool`) and dispatches:

```rust
match order {
    ToolMessageOrder::NonMcp => [pre_hook, tool_result, post_hook, new_messages, prevent].flatten(),
    ToolMessageOrder::Mcp    => [pre_hook, tool_result, new_messages, prevent, post_hook].flatten(),
}
```

Call sites resolve via `ToolMessageOrder::for_tool(&*tool)` (which
delegates to `tool.is_mcp()` — the same predicate the runner branches
on per "Constraints" above) rather than passing a raw boolean. This
keeps the MCP/non-MCP architectural branch tied to one source of truth
(the `Tool` trait's own `is_mcp()` method, mirroring TS's `isMcp`
property) and prevents accidental inversion or drift from `ToolId`.

**Failure path (execution-stage exception only):**

```text
[pre-hook messages]
tool_result for tool_use_id X             (carries the error)
[post-hook-failure messages + additional_contexts]   (inline; no MCP defer)
```

The success-block `prevent_continuation` append at TS line 1572 is
BYPASSED when execution throws — control transfers to catch (line 1589)
which returns `[error tool_result, ...hookMessages]` (line 1715–1737)
without consulting `shouldPreventContinuation`. The runner must therefore
leave `prevent_continuation_attachment` as `None` whenever
`path == Failure`.

**Early-return paths** (unknown tool / schema failure / validation failure /
PreToolUse stop / permission denied) — see I3 step 12 — do NOT run any
post-hook (success or failure). JSON parse failure is **not** in this
list: it is a pre-commit drop (no committed tool_use, no synthetic
result, no events) — see "JSON Parse Failures Are Pre-Commit, Not
Pre-Batch" under `ToolCallRunner`:

```text
[pre-hook messages, if any were emitted before the early return]
synthetic tool_result for tool_use_id X   (carries the error)
```

**Across multiple tool calls in one assistant message** (per I12 history
ordering — completion order for safe batches; execution order for
serial; EarlyOutcome plans append when the executor reaches their
block in partition order — they act as barriers that split the
surrounding safe batches, not a global prefix before all Runnables):

```text
assistant message with tool_use entries [A, B, C]
<bucket sequence for whichever Runnable finishes first>
<bucket sequence for the next Runnable to finish>
<bucket sequence for the last Runnable to finish>
```

(For a fully serial unsafe batch, the order above happens to equal
model order [A, B, C]; for a concurrent-safe batch where B finishes
first, the order is [B, A, C] or whatever the actual completion
sequence was.)

Rules:

- The assistant message must be committed before any tool result for that
  assistant message.
- Within a single tool call, follow the bucket order above based on
  `tool.is_mcp()` and success vs failure.
- Across tool calls in a concurrent-safe batch, history is in
  completion order (I12). The executor uses `FuturesUnordered` with
  its max-concurrency semaphore, not submission-order awaits.
- Across tool calls in a serial unsafe batch, history is in execution
  order, which equals model order by construction.
- `app_state_patch` / context modifiers from a concurrent-safe batch
  apply post-batch in **model_index** order under one write lock
  (TS `toolOrchestration.ts:54-62` parity). This is independent of
  history append order — two safe tools may surface in history out of
  model order, but their state mutations apply in model order.
- A tool call that was never committed into an assistant message should not
  emit a tool result. This matters during streaming retry/fallback.
- A committed tool call that was cancelled or discarded must emit a synthetic
  error result.
- Streaming (Phase 9) flushes queued completed outcomes after assistant-
  message commit in the **real `completion_seq` order observed during
  streaming**, not a re-derived model order.

The runner should return messages already ordered. `QueryEngine` should append
them as a block and should not sort or rebuild them.

#### Scheduling Contract

The boundary between `ToolCallRunner` and `StreamingToolExecutor` must be
unambiguous. TS chose: `StreamingToolExecutor.executeTool()` calls
`runToolUse()` directly (`StreamingToolExecutor.ts:320`). Both safe and
unsafe paths share one lifecycle, the executor only schedules.

**coco-rs follows the TS approach: executor invokes runner via callback.**
The executor never builds `ToolMessageBuckets` or `ToolCallOutcome`
itself; it dispatches each prepared call back into `runner.run_one()`.

```text
ToolCallRunner
  - Owns the entire semantic lifecycle: validation, hooks, permission,
    execution, post-hook, message-bucket building.
  - Owns UnstampedToolCallOutcome construction (only the runner can
    produce it). The runner does NOT assign `completion_seq` — it
    returns the unstamped body; the executor stamps at surface time.
  - Hands a callback (Fn(PreparedToolCall) -> Future<Unstamped>) to the
    executor along with the prepared jobs.

StreamingToolExecutor
  - Owns scheduling decisions only:
      * partition into concurrent-safe batches vs serial unsafe calls
      * for serial jobs: invoke runner callback per job; call
        `stamp_and_extract_effects(next_seq)` on the returned
        unstamped outcome; apply the extracted
        `effects.app_state_patch` to ToolAppState BEFORE the next job
        (matches TS toolOrchestration.ts:140 modifier-before-next-tool);
        hand the patch-free `ToolCallOutcome` to the history-append
        callback.
      * for concurrent batches: invoke runner callback through a
        `FuturesUnordered` completion stream while preserving the
        existing max-concurrency semaphore. As each
        future resolves, call `stamp_and_extract_effects(next_seq)`,
        emit `ToolUseCompleted`, and hand the patch-free outcome to
        the history-append callback immediately, so a slow earlier
        tool does not block a fast later tool. Meanwhile queue the
        extracted `effects.app_state_patch` keyed by `model_index`.
        After the batch's last future resolves, apply queued patches
        in **model_index order** under one write lock (matches TS
        toolOrchestration.ts:54-62 — `for (const block of blocks)`
        iterates in original order). Patch ownership is exclusive to
        the executor; `on_outcome` never sees it.
      * sibling abort on shell failure
      * per-tool cancellation tokens
  - Surfaces each ToolCallOutcome to the runner's history-append
    callback in completion order (per I12). For serial unsafe tools
    this naturally equals model order; for concurrent-safe batches it
    is the actual finish order; for EarlyOutcome plans the outcome is
    surfaced when the executor reaches that plan's block in partition
    order (the EarlyOutcome acts as a single-tool barrier per the
    partitioner's rule below — it splits the surrounding safe batches
    and is emitted between them, not globally before all Runnables).
    `completion_seq` is stamped at that moment.
  - Does NOT inspect or build ToolMessageBuckets, run hooks, or call
    validate_input.
```

Suggested API (streaming — no batch-return `Vec`, so history append
cannot be deferred to end-of-batch):

```rust
impl StreamingToolExecutor {
    /// Drives a batch of plans and hands each completed outcome to
    /// `on_outcome` as soon as it is available (completion order for
    /// concurrent-safe blocks; partition/execution order for serial
    /// and EarlyOutcome blocks). The method returns when every plan
    /// has been surfaced exactly once. `on_outcome` is responsible
    /// for history append; the executor never accumulates outcomes
    /// into a batch-result vector.
    pub async fn execute_with<F, Fut, H>(
        &self,
        plans: Vec<ToolCallPlan>,
        run_one: F,
        on_outcome: H,
    ) -> Result<(), ExecuteError>
    where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Sync + Send,
        Fut: Future<Output = UnstampedToolCallOutcome> + Send,
        H: FnMut(ToolCallOutcome) + Send;
    // At surface time:
    //   * Runnable: when the `run_one` future resolves
    //   * EarlyOutcome: when partition traversal reaches the barrier
    // the executor calls
    //   let (outcome, effects) =
    //       unstamped.stamp_and_extract_effects(next_seq);
    // and then:
    //   - hands `outcome` (patch-free) to `on_outcome` immediately
    //     so history grows in completion order;
    //   - keeps `effects.app_state_patch` in a per-call holder keyed
    //     by `model_index`. For concurrent-safe batches the executor
    //     applies queued patches in `model_index` order under one
    //     write lock at end-of-batch; for serial unsafe tools the
    //     patch is applied before building the next tool's
    //     `ToolUseContext`. On error (per I9) the patch is dropped
    //     without being called — `Drop` runs the FnOnce destructor,
    //     which releases captured state without side effects.
    //
    // `run_one` cannot pre-stamp because `stamp_and_extract_effects`
    // is `pub(crate)` in coco-tool. `on_outcome` cannot receive an
    // unstamped outcome because the `ToolCallOutcome` fields are
    // private and its constructor is only reachable through
    // `stamp_and_extract_effects`. `on_outcome` also cannot observe
    // patches: `ToolCallOutcome` is patch-free by construction.
}

// Caller:
let plans = runner.prepare_batch(invocations).await;
executor
    .execute_with(
        plans,
        |job, runtime| runner.run_one(job, runtime),
        |outcome| runner.append_to_history(outcome),
    )
    .await?;
```

Equivalent caller-facing shapes that satisfy the same contract. These
change how outcomes are surfaced to the caller, not the internal
scheduling primitive; the default internal implementation remains
`FuturesUnordered` over borrowed futures plus the max-concurrency
semaphore.

- `execute_with(...) -> impl Stream<Item = ToolCallOutcome>` — caller
  consumes the stream with `while let Some(o) = s.next().await`.
- `execute_with(..., tx: mpsc::Sender<ToolCallOutcome>)` — executor
  sends each outcome into `tx` the instant it is ready.

What MUST NOT happen: `-> Vec<ToolCallOutcome>`. Collecting the whole
batch into a vector before returning silently re-introduces "append
after batch" semantics and breaks I12.

For each plan the executor:
1. `EarlyOutcome(o)` — when the partitioner reaches this plan's block
   (EarlyOutcome is a single-tool barrier, so it sits between
   surrounding safe batches, never inside one), the executor stamps
   `completion_seq` on `o` and hands it to `on_outcome` immediately.
   No scheduling, no cancellation token, no `run_one` callback.
   `ToolUseStarted` is NOT emitted (no execution began), but
   `ToolUseCompleted` IS emitted with the synthetic error outcome so
   per-call invariant
   (one Queued → exactly one Completed) holds.
2. `Runnable(job)` — build a fresh `RunOneRuntime` (per-tool cancellation
   token as a child of the turn token, sibling-abort handle, progress
   channel, `model_index`), emit `ToolUseStarted`, invoke
   `run_one(job, runtime)` through a `FuturesUnordered` completion stream
   so outcomes surface in completion
   order; call `stamp_and_extract_effects(next_seq)` on the returned
   unstamped outcome (splits off the patch into `ToolSideEffects`,
   keyed by `model_index`, held by the executor); emit
   `ToolUseCompleted` and hand the patch-free `ToolCallOutcome` to
   `on_outcome` immediately.

The executor is purely a scheduler that applies patches between jobs
and calls `stamp_and_extract_effects` at surface time. The runner owns
every byte of the outcome body (the `UnstampedToolCallOutcome`); only
the executor produces the final patch-free `ToolCallOutcome` and holds
`ToolSideEffects` until the correct apply moment.

**Lifecycle Event Ownership** (single source per event — no double-emit):

| Event | Owner | When |
|---|---|---|
| `ToolUseQueued` | `QueryEngine` | Emitted for **every** committed tool_use entry in model order, before plans are handed to the executor. Including unknown-tool / schema-failure entries (those are committed but become `EarlyOutcome` plans). JSON-parse-failure entries are pre-commit drops and emit no Queued event — there is no committed tool_use to attach one to. |
| `ToolUseStarted` | `StreamingToolExecutor` | Only for `Runnable` plans, just before invoking the `run_one` callback. NOT emitted for `EarlyOutcome` (no execution began). Order is "start order" — for serial batches this equals model order; for concurrent batches it is non-deterministic. |
| `ToolUseCompleted` | `StreamingToolExecutor` | Emitted for **every** plan: after `run_one` returns for `Runnable`, and when the executor reaches that `EarlyOutcome` barrier block in partition order (carrying the synthetic error outcome). This preserves the per-call invariant: one `Queued` → exactly one `Completed`. Order is completion order per I12 — EarlyOutcome's Completed falls between the surrounding safe batches' Completed events, not before all of them. |

The runner does NOT emit any of these. This avoids the double-emit
risk where Phase 1's `StreamingToolExecutor::with_event_sink` and a
runner emit-call would both fire the same event. `with_event_sink`
(Phase 1 task) is the executor's emission channel for `ToolUseStarted`
and `ToolUseCompleted` only — `ToolUseQueued` is emitted by `QueryEngine`
through its existing `emit::*` helpers.

If a helper in `core/tool/src/execution.rs` remains, it is a pure helper
used by the runner, not a second lifecycle owner.

**What batched preparation may do up front (effect-free, context-free only):**

- Resolve `ToolInvocationTarget::Known(ToolId)` → `Arc<dyn Tool>`;
  convert `ToolInvocationTarget::Unknown` into an UnknownTool
  `EarlyOutcome`.
- Consume already-parsed `serde_json::Value` from `ToolCallPart.input`
  (the streaming/non-streaming accumulators upstream parse JSON and
  drop bad entries pre-commit at `engine.rs:1592-1606` — `prepare_batch`
  never sees raw JSON strings).
- Run schema validation. **The schema is full JSON-Schema, not
  properties-only.** TS validates against the full Zod schema
  (`tool.inputSchema.safeParse()` at `toolExecution.ts:614`), which
  includes `required`, `additionalProperties`, nested object shapes,
  enums, array constraints, oneOf/anyOf, format, and so on. The Rust
  validator MUST cover the same surface — anything less is silently
  weaker than TS and will accept inputs TS rejects.

  **Rust landing point**: introduce a new `core/tool/src/schema.rs`
  with two helpers:
  - `effective_tool_schema(tool: &dyn Tool) -> serde_json::Value` —
    returns the **complete JSON-Schema object the model actually saw**
    (a full `{type, properties, required, additionalProperties, ...}`
    document, not a `properties` sub-map). Mirror selection logic from
    `coco-rs/app/query/src/engine.rs:2752-2755`: prefer
    `tool.input_json_schema()` (the explicit override hook at
    `core/tool/src/traits.rs:163`) when it returns `Some`, since that
    is already a full schema. Otherwise use the migrated
    `Tool::input_schema()` full-schema document (see `ToolInputSchema`
    migration note below — after Phase 4, both branches return a
    complete JSON-Schema object; this is a branch selection, not a
    legacy fallback). **Do not**
    `serde_json::to_value(tool.input_schema())` directly if
    `Tool::input_schema()` ever returned a `{ properties: ... }`
    sub-map — that would produce a wrapped
    `{ "type": "object", "properties": { "properties": ... } }` and
    silently mis-validate. Phase 4 eliminates this risk by requiring
    `Tool::input_schema()` to return a full JSON-Schema document (the
    `ToolInputSchema` migration below is a **Phase 4 prerequisite**, not
    an ongoing compatibility shim). `effective_tool_schema()` therefore
    has exactly one code path — it returns whatever full schema
    `input_json_schema()` / `input_schema()` provides, and there is no
    wrap-properties branch to maintain. After this refactor, the
    provider-agnostic tool catalog builder MUST also call
    `effective_tool_schema()` so model-visible schema, `prepare_batch`
    validator input, and post-PreToolUse re-validation all derive from
    the **same** full-schema document.
  - `ToolSchemaValidator` — caches a compiled `jsonschema::Validator`
    per `ToolId` keyed on the effective schema. The cached validator is
    invoked **twice** per call: once by `prepare_batch` against the
    parsed model input, and again by `run_one` after a `PreToolUse` hook
    returns `updated_input` (per I3's Rust-side tightening — see the
    serial-step list below). "Cached" here means the **compiled
    validator** is built once per turn per `ToolId`, not that validation
    itself runs only once. The runner does NOT call `services/inference`
    directly (would invert the L2 → L3 dependency).

  **`ToolInputSchema` migration (Phase 4 prerequisite, lands before
  any other Phase 4 work)**: today
  `coco-rs/common/types/src/tool.rs:273-277` defines
  `ToolInputSchema { properties: HashMap<String, Value> }` —
  properties-only with type implicitly `"object"` and no
  `required` / `additionalProperties` / nested-shape carriage. This
  is **not** TS-parity-capable. Phase 4 widens `ToolInputSchema` to
  carry the full JSON-Schema document (or replaces it with
  `serde_json::Value` typed as a full schema), and in the same PR
  updates every built-in tool to emit a full schema (via the migrated
  `Tool::input_schema()` default or the `input_json_schema()`
  override where derive cannot express it — the same role TS's
  per-tool Zod definitions play). Any tool still returning a
  properties-only sub-map is fixed directly in that PR; there is no
  fallback path, no transitional shim, and no "weaker than TS"
  escape hatch carried forward. `effective_tool_schema()` after this
  migration has **one** code path returning a full JSON-Schema
  document, and Phase 4 acceptance tests (see Phase 4 section below)
  enforce that invariant mechanically.

  **Canonical tool identity (MCP and built-in).** Schema repair alone
  is not enough: today the four axes of "what is this tool" drift
  across the codebase. The MCP registry uses qualified string names
  (`mcp__<server>__<tool>`) at `coco-rs/core/tool/src/registry.rs:36`;
  `McpTool::name()` at `coco-rs/core/tools/src/tools/mcp_tools.rs:297`
  returns the *raw* remote tool name; and the API-request builder at
  `coco-rs/app/query/src/engine.rs:2757` uses `tool.name()` as the
  model-visible wire name. The refactor separates three concepts that
  must never share one "canonical string":

  - **Tool identity**: `ToolId`, used for registry lookup, permissions,
    hooks, audit, events, telemetry correlation, and tool execution.
  - **Human pattern string**: derived from `ToolId` for settings,
    permission rules, hook matchers, and CLAUDE.md-style text.
  - **Provider wire name**: request-local string accepted by a specific
    model provider. This may be sanitized, shortened, or rejected, and
    two `ToolId`s can collide after provider-specific normalization.

  `coco-tool` owns only the first category plus intrinsic metadata:

  ```rust
  /// Single source of truth for a tool's intrinsic properties.
  /// Provider-agnostic and model-agnostic: this struct lives in
  /// `coco-tool` (L3) and must not reference any provider, wire
  /// format, or LLM model type. Tools are defined once and bound
  /// to any provider at call time; they never depend on the
  /// provider layer below them.
  pub struct ToolDefinitionEntry {
      /// Canonical `ToolId` — `Builtin(ToolName) | Mcp { server, tool } | Custom`.
      /// Absolute identity. All internal lookups (registry,
      /// permissions, hooks, audit, event routing) key on this,
      /// never on strings.
      pub tool_id: ToolId,
      /// Short stable label for OTel / logs. For MCP tools, the
      /// raw remote `tool` name (strip the `mcp__<server>__`
      /// prefix); for built-ins, the `ToolName` variant name.
      /// Telemetry dashboards aggregate by bare tool across
      /// servers. Not a wire name and not an identity — purely a
      /// display label for observability.
      pub telemetry_name: String,
      /// Full JSON-Schema document returned by
      /// `effective_tool_schema(tool)`. Built once at registration,
      /// compiled into the cached validator described above.
      pub schema: serde_json::Value,
  }
  ```

  **No wire name in `ToolDefinitionEntry`.** An earlier draft carried a
  `model_name` / `canonical_name` string here so the registry and model
  request builder could share it. That was wrong: it conflated registry
  lookup, human-readable permission-rule keys, and provider wire format
  into a single string, and pulled the L3 tool layer toward
  provider-aware design. The corrected design:

  - **Registry key: `ToolId`.** `ToolRegistry` becomes
    `HashMap<ToolId, RegisteredTool>`, where `RegisteredTool` holds
    `Arc<dyn Tool>` plus `ToolDefinitionEntry`. `core/tool/src/registry.rs:36`'s
    current string-keyed execution lookup is replaced. String aliases
    may remain only for human search/discovery UI; they must resolve to
    `ToolId` before execution and are not an execution map.
  - **Permission rule / CLAUDE.md / hook matcher key:** a
    `ToolId::as_pattern_string(&self) -> String` helper in
    `coco-types` returns the user-facing pattern (`"Read"`,
    `"Bash"`, `"mcp__server__tool"`) derived deterministically from
    the `ToolId` variant. This is the only place strings are
    derived from `ToolId`, and it flows **one-way** (ToolId →
    pattern string). Reverse parsing (`pattern → ToolId`) is a
    separate concern owned by `coco-permissions` when loading
    `settings.json`.
  - **Wire name: provider-adapter problem, not the tool's.** The
    standalone `vercel-ai-*` crates remain provider SDK crates and do
    not depend on `coco-types` or `ToolId`. They continue to receive
    and emit string tool names through `LanguageModelV4Tool` /
    `ToolCallPart`. A coco-owned adapter layer in `app/query`
    (`ModelRuntime` or `app/query/src/tool_wire.rs`) owns the
    `ToolId <-> wire String` map for the active provider/model:

    ```
    // Lives in app/query, where depending on coco-tool, coco-types,
    // and vercel-ai-provider does not invert the crate graph. It does
    // NOT live in coco-tool, coco-inference, or vercel-ai-* provider crates.
    pub struct ToolWirePolicy {
        provider: ProviderApi,
        model_id: String,
    }

    impl ToolWirePolicy {
        pub fn prepare_tools(
            &self,
            catalog: &[ToolDefinitionEntry],
        ) -> Result<PreparedToolSet, NameError> { ... }
    }

    pub struct PreparedToolSet {
        pub wire_tools: Vec<LanguageModelV4Tool>,
        pub to_wire: HashMap<ToolId, String>,
        pub from_wire: HashMap<String, ToolId>,
    }

    impl PreparedToolSet {
        pub fn tool_id_for_wire(&self, wire: &str) -> Option<&ToolId> {
            self.from_wire.get(wire)
        }
    }
    ```

    `ToolWirePolicy` is provider/model specific and stateless enough to
    live on `ModelRuntime`; `PreparedToolSet` is request-local state.
    Reverse lookup must happen through `PreparedToolSet`, not by
    reparsing a wire string globally, because provider normalization can
    be lossy. The adapter rewrites outbound tool definitions to provider
    wire strings, calls `ApiClient` with `wire_tools`, and translates
    every returned or streamed tool name through `PreparedToolSet` before
    `QueryEngine` / `ToolCallRunner` sees the call. Collision detection
    (two `ToolId`s that collapse to the same wire string under Gemini
    sanitization) is a `prepare_tools` error in this adapter layer, not
    a registry concern and not a `coco-tool` concern.

  After this refactor, `Tool::name()` / `McpTool::name()` may still
  exist as a display/native-name API because the trait requires it
  today, but it is no longer used for identity, registry lookup,
  permission matching, hook matching, or provider wire naming.
  `engine.rs` hands provider-agnostic `ToolId` + schema + prompt data
  to `ModelRuntime`; `ModelRuntime` prepares provider wire tools and
  returns typed model output whose tool calls already carry `ToolId`.
  Lower-level `vercel-ai-provider` remains string-based; the typed
  boundary is the coco adapter, not the standalone provider crates.
  OTel spans read `telemetry_name` directly from the registry entry.

  Phase 4 acceptance tests:
  1. `ToolRegistry` has no `String`-keyed execution lookup. The runner
     can resolve executable tools only by `ToolId`; any remaining
     string alias/search map is named as UI/discovery-only and is not
     reachable from the execution path.
  2. `ToolId::as_pattern_string` round-trips with
     `coco-permissions`'s pattern parser for every built-in and at
     least one MCP case.
  3. Coco-owned `ToolWirePolicy::prepare_tools` produces a
     `PreparedToolSet` where each `to_wire` entry round-trips through
     `PreparedToolSet::tool_id_for_wire` for every tool in a
     representative registry (built-ins + MCP with hyphenated names)
     across Anthropic / OpenAI / Google policy implementations. The
     test crate must not add a `coco-types` dependency to any
     `vercel-ai-*` provider crate.
  4. Gemini collision detection: two `ToolId`s whose wire names
     collapse to the same string under Gemini sanitization MUST
     fail `prepare_tools` with a clear error, not silently merge.

  **Schema-invalid plans are batching barriers.** TS partition
  (`toolOrchestration.ts:91-115`) calls `inputSchema.safeParse(input)`
  inside `partitionToolCalls`; on failure, `isConcurrencySafe = false`,
  so the schema-invalid tool gets its own block and breaks any
  surrounding safe batch. Rust mirrors this: a `ToolCallPlan::EarlyOutcome`
  produced by `SchemaFailed` (or `UnknownTool`, or any other pre-execution
  decision) MUST be treated by the executor's partitioner as
  not-concurrency-safe, ending the preceding safe batch and starting a
  new one after the EarlyOutcome. This preserves the TS guarantee that
  a malformed tool call cannot ride alongside safe tools in a concurrent
  batch — it always lands as its own serial breakpoint.
- Assign I12 `model_index` values (= position in the assistant tool_use list).

**What must run per-tool serially (after each prior tool's app_state_patch
has been applied):**

- Build `ToolUseContext` via `ToolContextFactory` (sees fresh app state).
- Run `tool.validate_input(&Value, &ToolUseContext)` — its signature takes
  `&ToolUseContext` for a reason (`core/tool/src/traits.rs:360` doc:
  "context needed for stateful validation like read-before-write
  enforcement"). `BashTool` reads `ctx.tool_config`; `ExitPlanModeV2Tool`
  reads `ctx.is_teammate` and `ctx.permission_context.mode`. Running
  this in batched prep with a stale or shared context would be wrong.
- Strip model-controlled internal fields (defense in depth, AFTER validation).
- Run `PreToolUse` hooks (may rewrite input based on current state).
- **Apply hook `updated_input`** if the hook returned one (per I3 line
  ~406). Replace the working input with the hook's rewritten value.
- **Re-validate the rewritten input** against the same effective schema
  (use `ToolSchemaValidator` from prepare_batch's cache — keyed on
  `ToolId`, no recompile cost). A hook that returns malformed input
  produces a synthetic validation error here, NOT silently downstream.
- **Re-run `tool.validate_input(&rewritten, &ctx)`** — context-dependent
  invariants (read-before-write, plan-mode gates, etc.) must hold for
  whatever the hook rewrote into.
- Run permission checks (auto-mode classifier observes recent history)
  using the **rewritten validated input**.
- Execute the tool with the **rewritten validated input**.
- Run `PostToolUse` / `PostToolUseFailure` per I3.
- Build `UnstampedToolCallOutcome` carrying the patch; executor stamps
  `completion_seq` at surface time.
- Executor/orchestrator applies the patch BEFORE next serial tool's
  context build.

If a future optimization wants to pre-run `validate_input` for some
tools, add an explicit marker API (e.g. `Tool::validation_is_context_free()
-> bool` defaulting to `false`). Do not gate this on doc convention.

This matches TS `toolOrchestration.ts:130–141`: serial mode applies the
context modifier of tool N before tool N+1 starts. If we batched hooks/
permission/context up front, a serial tool that depends on a prior tool's
mutation would observe stale state.

For concurrent batches (all tools `is_concurrency_safe`), the runner can
batch the per-tool steps in parallel. Concurrency-safe does NOT mean
read-only: `TaskCreateTool` and `TaskUpdateTool` are both
`is_concurrency_safe() == true` yet emit `app_state_patch`
(`coco-rs/core/tools/src/tools/task_tools.rs:256/293/498/750`). The
contract is narrower: concurrent tools must NOT mutate `ctx.app_state`
inline during execution; they may only emit patches via
`UnstampedToolCallOutcome.effects.app_state_patch`. The executor
extracts each patch via `stamp_and_extract_effects`, queues it keyed
by `model_index`, and applies queued patches in `model_index` order
under one write lock AFTER the batch completes
(`toolOrchestration.ts:54–62` parity). This preserves determinism — two
concurrent tools cannot observe each other's mutations.

For streaming execution, the runner can prepare each call as soon as
enough input has streamed, but it must defer API-visible message emission
until the assistant message is committed.

#### Error Mapping

Use a closed enum for synthetic tool errors so tests can assert exact behavior:

```rust
pub enum ToolCallErrorKind {
    UnknownTool,
    // NOTE: no `InvalidJson` variant — JSON parse failure is a
    // pre-commit drop in the assistant-message accumulator
    // (engine.rs:1592-1606) and never produces a synthetic tool_result.
    // See "JSON Parse Failures Are Pre-Commit, Not Pre-Batch".
    SchemaFailed,            // initial schema check in prepare_batch, OR
                             // re-validation after PreToolUse rewrite
    ValidationFailed,        // tool.validate_input() (incl. post-hook re-run)
    HookBlocked,             // PreToolUse stop
    PermissionDenied,
    PermissionBridgeFailed,
    ExecutionFailed,         // exception thrown by tool.execute()
    PreExecutionCancelled,   // cancelled/abort BEFORE tool.execute() started
                             // (prepare / schema / validation / hook /
                             // permission stages). TS parity:
                             // toolExecution.ts:413 — pre-execution abort
                             // synthesizes a tool result WITHOUT firing
                             // PostToolUseFailure hooks.
    ExecutionCancelled,      // cancelled AFTER tool.execute() started.
                             // TS parity: toolExecution.ts:1696 — runs
                             // PostToolUseFailure hooks.
    JoinFailed,              // tokio join error; only reachable after
                             // execute() started (the join handle exists
                             // only once the spawn is in flight), so
                             // treated as execution-stage.
    StreamingDiscarded,
}

impl ToolCallErrorKind {
    /// Whether this error path runs PostToolUseFailure hooks.
    /// Per I3 step 12 + TS toolExecution.ts:1696 (execution-stage fail
    /// runs failure hooks) vs toolExecution.ts:413 (pre-execution
    /// abort does NOT). The enum itself encodes the lifecycle stage,
    /// so this match is exhaustive and no `execution_started: bool`
    /// side-channel is needed.
    pub fn runs_post_tool_use_failure(self) -> bool {
        match self {
            // Execution actually started; failure hook fires.
            Self::ExecutionFailed
            | Self::ExecutionCancelled
            | Self::JoinFailed => true,

            // Early-return / pre-execution paths; no failure hook.
            Self::UnknownTool
            | Self::SchemaFailed
            | Self::ValidationFailed
            | Self::HookBlocked
            | Self::PermissionDenied
            | Self::PermissionBridgeFailed
            | Self::PreExecutionCancelled
            | Self::StreamingDiscarded => false,
        }
    }
}

/// Runner construction rule: the runner decides between
/// `PreExecutionCancelled` and `ExecutionCancelled` based on a local
/// `execution_started: bool` flag that is set to `true` only in the
/// narrow window immediately before `tool.execute()` is awaited and
/// cleared once its future resolves. Cancellation observed while the
/// flag is `false` maps to `PreExecutionCancelled`; observed while
/// `true` (or via a `JoinError`) maps to `ExecutionCancelled` /
/// `JoinFailed`. This flag lives inside `run_one` — it never leaks
/// into `UnstampedToolCallOutcome` or the executor, because the enum
/// already carries the decision. Tests must cover cancel-before-hook,
/// cancel-between-hook-and-execute, and cancel-mid-execute.
```

Each variant maps to:

- stable user-facing text
- stable audit metadata where relevant
- one model-visible tool result
- optional protocol/stream event
- whether `PostToolUseFailure` runs (per the table above)

This keeps error text consistent across streaming and non-streaming paths,
and prevents the runner from over-firing failure hooks on early-return paths.

**Scope of "TS parity" for error formats.** Two parity dimensions are
distinct and must be specified separately:

1. **Semantic parity (REQUIRED).** Same accept/reject decisions, same
   `ToolCallErrorKind` for equivalent failure causes, same lifecycle
   (post-hook fires? failure-hook fires? prevent-continuation suppressed?).
   Tested via behavioral tests, not text snapshots. This is mandatory
   for Phase 4 acceptance.

2. **Textual parity (OPT-IN, deferred).** TS schema failures emit
   `<tool_use_error>InputValidationError: ${formatZodValidationError(...)}</tool_use_error>`
   (`toolExecution.ts:617, 670`) using Zod's specific error message
   format. Rust uses `jsonschema`, whose default error text differs
   from Zod's. Bit-for-bit textual parity would require:
   - a dedicated `format_jsonschema_error_as_zod` formatter that
     translates `jsonschema` `ValidationError` paths/keywords into
     Zod-style messages;
   - snapshot tests covering the cross product of (schema feature) ×
     (failure mode) — required missing, unknown field with
     `additionalProperties: false`, nested type mismatch, enum
     mismatch, oneOf mismatch, etc.;
   - the same schema-not-sent hint logic at `toolExecution.ts:619-630`
     (needs `toolUseContext.options.tools` and `messages` access — a
     non-trivial dependency to surface to the validator).

   Phase 4 ships **semantic parity only**. Textual parity is a
   subsequent opt-in PR with its own snapshot suite. The error message
   carried in the synthetic tool_result must be informative and stable
   across runs but is not required to byte-match TS.

### PermissionController

`PermissionController` should isolate permission policy from execution.

Inputs:

- `Arc<dyn Tool>`
- effective input
- `ToolUseContext`
- optional hook permission override
- current history for auto-mode classifier

Outputs:

```rust
pub enum PermissionResolution {
    Allow { input: serde_json::Value },
    Deny {
        message: String,
        audit: PermissionDenialInfo,
        source: PermissionDenySource,
    },
}

pub enum PermissionDenySource {
    PreToolUse,
    ToolCheck,
    AutoModeClassifier,
    ApprovalBridge,
    MissingBridge,
}
```

Behavior:

1. If PreToolUse returned a Deny override, deny immediately.
2. If PreToolUse returned an Allow override, allow without normal prompt.
3. If PreToolUse returned an Ask override, force approval flow.
4. Otherwise call `tool.check_permissions`.
5. If decision is Ask and auto-mode is active, run classifier.
6. If still Ask, call permission bridge if present.
7. If bridge rejects or fails, deny with a tool result.
8. If no bridge is present, preserve existing default only if intentional and
   documented. Prefer explicit behavior for headless/non-interactive sessions.

**PermissionDenied hook parity (TS).** TS fires PermissionDenied hooks
only for auto-mode classifier denials when the classifier feature is
enabled (`services/tools/toolExecution.ts:1073`). Rust should match
that by default: `PermissionController` records a
`PermissionDenySource`, and the runner calls
`HookHandle::run_permission_denied(PermissionDeniedInput { ... })`
only when `source == PermissionDenySource::AutoModeClassifier` and the
matching feature/config gate is active. PreToolUse Deny, ordinary
tool-check denial, bridge rejection, and missing bridge still produce
one synthetic error tool result, but they do **not** fire
PermissionDenied hooks unless a later PR explicitly documents that as a
Rust-side behavior expansion.

Rust already defines `PermissionDeniedInput` at
`coco-rs/hooks/src/inputs.rs:189`; the missing pieces are the
structured orchestration wrapper and a matching
`HookHandle::run_permission_denied` method (add to
`core/tool/src/hook_handle.rs` alongside the existing
`run_post_tool_use_failure`). Phase 3 HookAdapter extends
`QueryHookHandle` with this method; Phase 4 PermissionController
invokes it for classifier denials. Acceptance tests cover both the
positive classifier-deny path and a non-classifier deny path that must
not fire the hook.

This keeps TS permission behavior centralized instead of scattered across the
engine and executor.

### HookAdapter

`core/tool/src/hook_handle.rs` already defines the right abstraction. The missing
piece is an app/query implementation.

Add:

```text
coco-rs/app/query/src/hook_adapter.rs
```

`QueryHookHandle` should hold:

- `Arc<HookRegistry>`
- `OrchestrationContext`
- optional hook execution event sender

It should implement `coco_tool::HookHandle`:

- `run_pre_tool_use`
- `run_post_tool_use`
- `run_post_tool_use_failure`

`coco-hooks` currently has structured functions for PreToolUse and PostToolUse.
It needs a structured PostToolUseFailure helper that builds
`PostToolUseFailureInput` instead of calling `HookRegistry::execute_hooks`
directly.

Mapping from `AggregatedHookResult`:

| `AggregatedHookResult` field | Tool handle field |
|------------------------------|-------------------|
| `updated_input` | `PreToolUseOutcome::updated_input` |
| `permission_behavior` Allow/Ask/Deny | `PreToolUseOutcome::permission_override` |
| `blocking_error` | `blocking_reason` |
| `hook_permission_decision_reason` | `permission_reason` |
| `additional_contexts` | `additional_contexts` |
| `system_message` | `system_message` |
| `suppress_output` | `suppress_output` |
| `updated_mcp_tool_output` | `PostToolUseOutcome::updated_mcp_tool_output` (MCP-only; see below) |
| `prevent_continuation` | `PostToolUseOutcome::prevent_continuation` |
| `stop_reason` | `PostToolUseOutcome::stop_reason` |

**MCP-only output rewrite (TS parity).** TS applies
`updatedMCPToolOutput` only when `isMcpTool(tool)` — see
`services/tools/toolHooks.ts:145` (`if (result.updatedMCPToolOutput &&
isMcpTool(tool))`). The Rust field is therefore **named
`updated_mcp_tool_output`** (not a generic `updated_output`) and the
runner's output-substitution step must be gated on
`tool.is_mcp()`; for non-MCP tools the field is ignored even if a hook
sets it. This prevents the "any PostToolUse hook silently rewrites
built-in tool output" divergence. Phase 4's acceptance tests cover
both cases (see Unit Tests below).

### ToolContextFactory

Current `create_tool_context` hardcodes several fields. Move this logic into a
factory so it can be tested without running the full query loop.

Suggested type:

```rust
pub struct ToolContextFactory {
    config: QueryEngineConfig,
    tools: Arc<ToolRegistry>,
    cancel: CancellationToken,
    app_state: Option<Arc<RwLock<ToolAppState>>>,
    agent: AgentHandleRef,
    hook_handle: Option<HookHandleRef>,
    mailbox: MailboxHandleRef,
    task_list: TaskListHandleRef,
    todo_list: TodoListHandleRef,
    permission_bridge: Option<ToolPermissionBridgeRef>,
    file_read_state: Option<Arc<RwLock<FileReadState>>>,
    file_history: Option<Arc<RwLock<FileHistoryState>>>,
}
```

Factory method:

```rust
pub async fn build(
    &self,
    history: &[Message],
    opts: ToolContextOptions,
) -> ToolUseContext
```

`ToolContextOptions` should include:

- `tool_use_id`
- `user_message_id`
- `progress_tx`
- `query_depth`
- `agent_id`
- `agent_type`
- `cwd_override`
- `preserve_tool_use_results`

Fields that must be fixed:

- `is_non_interactive` from `QueryEngineConfig`.
- `max_budget_usd` from `QueryEngineConfig`.
- `custom_system_prompt` from `QueryEngineConfig::system_prompt`.
- `append_system_prompt` from `QueryEngineConfig::append_system_prompt`.
- `messages` from current history.
- `agent` from injected `AgentHandleRef`.
- `hook_handle` from `QueryHookHandle`.
- `progress_tx` from active runner.
- `query_depth` from parent runtime.
- `main_loop_model` from current active model, not stale config after fallback.

### Streaming Tool Execution

Streaming execution should be a scheduling mode inside `ToolCallRunner`.

Do not keep an eager path that calls `tool.execute` directly.

Required behavior:

- If `streaming_tool_execution` is false, never start tools during model
  streaming.
- If hooks are configured and streaming hooks are not safe to run mid-stream,
  disable eager start or route through the same full lifecycle with a safe
  ordering guarantee.
- If a stream fails and the model turn is retried non-streaming, discard any
  in-flight streamed tool work and return synthetic errors only if those tool
  calls have already become part of the committed assistant message.
- **Streaming ordering — single public contract (reconciles I12).**
  The two axes from I12 both apply and do not conflict once you pick
  the right one per consumer:
  - **History append order (model-visible `tool_result` messages):**
    real `completion_seq` observed during streaming. This is the SDK-
    visible contract. A concurrent-safe batch surfaces results in the
    order tools actually finished; a slow earlier tool does not block
    a faster later tool. Serial unsafe tools naturally collapse to
    model order (one tool at a time).
  - **`app_state_patch` application:** always **model_index** order,
    applied post-batch under one write lock. Never tied to
    `completion_seq`.
  - **TS comparison (not parity):** TS
    `StreamingToolExecutor.getCompletedResults()`
    (`StreamingToolExecutor.ts:412-440`) iterates `this.tools` in
    model order and yields completed tools while breaking on a
    still-running unsafe tool. That yields an *emergent* completion
    sequence constrained by model-order iteration; it is not a
    separate contract. Rust tightens this into an explicit
    `completion_seq` stream (via `FuturesUnordered` plus the executor's
    max-concurrency semaphore) and flushes queued outcomes after
    assistant-message commit in that stamped order. Earlier "API order" wording in this
    section was imprecise; the authoritative statement is above.

Recommended initial implementation:

1. Phase 1 disables direct eager execution unless it can use `ToolCallRunner`.
2. Phase 2 implements non-streaming `ToolCallRunner`.
3. Phase 3 adds streaming scheduling by feeding completed streamed tool calls
   into the same lifecycle.

This avoids preserving the current bypass path.

### AgentRuntime

`AgentTool` should remain in `core/tools`, but it should call a real
`AgentHandle`.

`QueryEngine` needs:

```rust
pub fn with_agent_handle(mut self, handle: AgentHandleRef) -> Self
```

Then `ToolContextFactory` installs that handle.

Suggested runtime split:

```text
AgentDefinitionStore
  loads built-in, user, project, and plugin agent definitions

AgentRuntime
  resolves request mode and builds child runtime configuration

AgentQueryEngine
  executes a child query and returns result data

AgentStateStore
  records background/teammate state, task output, cancellation, and transcript ids
```

Suggested data types:

```rust
pub struct AgentRuntimeConfig {
    pub parent_agent_id: Option<String>,
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    pub config_home: PathBuf,
    pub permission_mode: PermissionMode,
    pub allowed_tools: Option<Vec<String>>,
    pub selected_mcp_servers: Vec<String>,
    pub query_depth: i32,
}

pub struct ResolvedAgentInvocation {
    pub mode: AgentInvocationMode,
    pub definition: AgentDefinition,
    pub prompt: String,
    pub child_config: QueryEngineConfig,
    pub inherited_context: AgentInheritedContext,
}

pub enum AgentInvocationMode {
    Sync,
    Background,
    Teammate,
    Fork,
    Worktree,
    Remote,
}
```

The runtime should separate three decisions that are currently easy to mix:

- definition resolution: which agent was requested and what it is allowed to do
- execution mode: sync, background, teammate, fork, worktree, or remote
- query execution: how the child `QueryEngine` actually runs

Runtime behavior:

#### Sync Subagent

TS reference:

- `tools/AgentTool/AgentTool.tsx`
- `tools/AgentTool/runAgent.ts`

Rust behavior:

1. Resolve requested agent definition.
2. Resolve child model.
3. Resolve child permission mode using TS inheritance semantics.
4. Resolve child tool set.
5. Build child system prompt.
6. Build child `QueryEngineConfig`.
7. Run child `QueryEngine` through `AgentQueryEngine::execute_query`.
8. Return final text, tool use count, token usage, duration, and agent id.

Do not call `InProcessAgentRunner::spawn_agent` and then wait for a missing
`result_rx`.

Child query construction details:

- `agent_id` must be unique and stable for transcript/task references.
- `query_depth` should increment and enforce a recursion limit.
- child history should contain only the instruction and intentional inherited
  context, not parent pending tool results.
- child tool registry should be filtered before building the tool catalog.
- child MCP discovery should include only allowed MCP servers.
- child permission bridge should identify the child agent in approval prompts.
- child hooks should run with the child context but preserve parent session
  identity where TS does.
- child cancellation should be linked to the parent cancellation token.

Returned `AgentQueryResult` should include:

- final assistant text
- final messages or a transcript reference, depending on existing API shape
- tool use count
- token usage
- duration
- stop reason
- agent id
- any task/output reference needed by SDK/TUI consumers

#### Background Subagent

Behavior:

1. Create task/subagent state.
2. Spawn a Tokio task that runs the child query.
3. Store result or error in state/output file equivalent.
4. Return `AsyncLaunched` with `agent_id` and optional output path.

Background execution details:

- The parent tool result should be returned immediately after launch.
- The background task must own its cancellation token.
- Task state must transition through running, completed, failed, or cancelled.
- Final output should be retrievable by existing task/agent output tools.
- Progress should be emitted through `CoreEvent` and visible to TUI/SDK.
- Parent process shutdown should either cancel or persist enough state to
  explain that the task was interrupted.

#### Teammate Spawn

Behavior:

1. Create/register teammate.
2. Start `run_in_process_teammate`.
3. Wire mailbox, task state, cancellation, and permission bridge.
4. Return `TeammateSpawned`.

`InProcessAgentRunner::spawn_agent` should either:

- actually start execution, or
- be renamed to `register_agent` and paired with an explicit `start_agent`.

The current naming hides the fact that it only registers state.

Teammate-specific details:

- A teammate should have a mailbox handle.
- SendMessage should route through mailbox state, not ad hoc shared memory.
- Teammate task state should be visible in the same state tree as normal tasks.
- Permission prompts should identify the teammate and requested tool.
- Teammate lifecycle events should use the same event stream as other agents.

#### Fork Subagent

Existing `agent_fork.rs` helpers should be wired when fork mode is enabled:

- prevent recursive fork
- clone parent context messages
- wrap child instruction
- return the fork placeholder or real background task result according to TS

Fork support should be implemented after sync/background subagents are correct.

#### Worktree And Remote Modes

TS includes worktree and remote agent behavior. Rust should represent these
modes in the public runtime model even if they are implemented later.

First implementation target:

- parse and preserve the requested mode
- reject unsupported modes with one model-visible tool result
- avoid silently falling back to sync mode

Later implementation target:

- worktree mode creates or selects an isolated cwd
- worktree cleanup follows TS cleanup rules
- remote mode uses the remote agent transport when available
- remote failures are reported through the same AgentTool result shape

#### Agent Prompt And Tool Filtering

Agent prompt generation should reuse existing helpers rather than duplicating
string logic:

- `agent_spawn.rs` for definition loading
- `agent_advanced.rs` for allowed tools and prompt descriptions
- `coco-context` helpers for environment and project context

Filtering rules should be applied before the tool catalog is generated. A
child agent should never see a forbidden tool in the model prompt and then rely
on runtime denial as the only enforcement.

### SkillRuntime

Skills should not be resolved through `AgentHandle`. Add a dedicated runtime or
handle.

Trait (uses snafu enums per project policy, not `String` errors):

```rust
#[async_trait::async_trait]
pub trait SkillHandle: Send + Sync {
    async fn invoke_skill(
        &self,
        name: &str,
        args: &str,
        ctx: &ToolUseContext,
    ) -> Result<SkillInvocationResult, SkillInvocationError>;
}

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum SkillInvocationError {
    #[snafu(display("skill not found: {name}"))]
    NotFound { name: String },
    #[snafu(display("skill is disabled: {name}"))]
    Disabled { name: String },
    #[snafu(display("skill is hidden from model: {name}"))]
    HiddenFromModel { name: String },
    #[snafu(display("argument expansion failed: {source}"))]
    Expansion { source: ExpansionError },
    #[snafu(display("forked agent failed: {source}"))]
    Forked { source: AgentRuntimeError },
    #[snafu(display("remote skill mode is not supported"))]
    RemoteUnsupported,
}
```

Suggested invocation model:

```rust
pub enum SkillInvocationKind {
    Inline,
    Fork,
    Remote,
}

pub struct ResolvedSkillInvocation {
    pub skill: SkillDefinition,
    pub kind: SkillInvocationKind,
    pub expanded_prompt: String,
    pub allowed_tools: Option<Vec<String>>,
    pub model: Option<ModelRole>,         // ModelRole, not raw String
    pub source: SkillSource,
}
```

`SkillInvocationResult` — typed payload (no `serde_json::Value` for internally
produced+consumed data, per `coco-rs/CLAUDE.md`):

```rust
pub enum SkillInvocationResult {
    Inline {
        output: SkillInlineOutput,        // typed, not Value
        new_messages: Vec<Message>,
        allowed_tools: Option<Vec<String>>,
        model: Option<ModelRole>,
    },
    Forked {
        agent_id: AgentId,
        output: AgentQueryResult,         // reuse coco_tool::AgentQueryResult
    },
}

pub struct SkillInlineOutput {
    pub status: SkillInlineStatus,        // exhaustive enum, matches TS 'inline'/'completed'
    pub summary: String,
}
```

`AgentHandle::resolve_skill` is **deleted** in this phase (it currently
returns `Err("Skill resolution should be handled by the query engine")` and
is a known broken stub).

Resolution order should match TS behavior:

1. Normalize name.
2. Resolve alias.
3. Resolve source priority using existing `SkillManager` behavior.
4. Reject disabled skills.
5. Reject model-hidden or `disable_model_invocation` skills when called from
   the model-facing SkillTool.
6. Decide inline, fork, or remote invocation.
7. Expand arguments into the skill prompt.
8. Return a typed invocation plan to the tool.

Inline behavior:

1. Normalize skill name.
2. Resolve alias through `SkillManager`.
3. Reject disabled skills.
4. Reject `disable_model_invocation` for model-called SkillTool.
5. Expand arguments using `skill_advanced::expand_skill_prompt`.
6. Build new user/context messages tagged with the parent tool use id.
7. Return a normal tool result plus `new_messages`.

Forked behavior:

1. Resolve skill.
2. Expand prompt.
3. Resolve skill agent/model/tool restrictions.
4. Call `AgentRuntime`.
5. Return forked result.

Remote behavior:

Remote skills should be represented in the type system but can remain a later
phase if the current Rust runtime does not yet support them. The first
implementation should reject unsupported remote invocation with a clear
model-visible tool result rather than pretending it is inline.

Skill prompt behavior:

**Design choice — diverges from TS.** TS lists available skills via system
reminders, not in the SkillTool description (`SkillTool/prompt.ts:189`:
"Available skills are listed in system-reminder messages"). The TS tool
description itself is mostly static.

coco-rs intentionally injects the skill list into `SkillTool::prompt`
because:

- coco-rs already runs system reminders via `coco-system-reminder`, but
  the reminder cadence is throttled and may miss turns where skills change.
- Putting the list in the tool definition guarantees the model sees the
  current set every turn.
- This is consistent with how coco-rs handles dynamic agent listings.

If TS parity is preferred later, this can be flipped without API breakage.

Implementation:

- The tool catalog builder becomes async.
- Calls `tool.prompt(&PromptOptions)` for dynamic tools.
- `PromptOptions` (`core/tool/src/traits.rs:56–68`) needs a new field:
  `pub skill_names: Vec<String>` (sorted for determinism). Add this in
  Phase 5.
- `SkillTool::prompt` uses `coco_skills::generate_skill_tool_prompt`
  with the injected skill list.

Skill prompt filtering rules:

- built-in, project, user, and plugin skills can all contribute if enabled
- aliases should be shown only when TS shows them, otherwise only resolve them
- hidden skills should not appear
- model-disabled skills should not appear
- skills requiring unavailable tools should either be hidden or described with
  the restricted tool set, matching TS behavior
- prompt text should be stable for tests by sorting skills deterministically

Skill message rules:

- Inline skills should return one normal SkillTool result message plus
  `new_messages`.
- The expanded skill prompt should be tagged with the parent tool use id.
- The next model request should see the expanded prompt after the SkillTool
  result.
- Forked skills should return the child agent result through the same tool
  result pipeline as AgentTool.
- Any skill resolution failure should be one synthetic SkillTool result, not an
  engine-level error.

**Message schema requirement.** Tagging messages with a parent tool use id
requires a schema change. Today `UserMessage` (`common/types/src/message.rs:123`)
has no `parent_tool_use_id` field; only `ProgressMessage` carries
`tool_use_id` (different semantic — it's the tool *being progressed*, not
the *parent* of the message). Add to Phase 7:

```rust
pub struct UserMessage {
    // ... existing fields ...
    /// If this message was generated by a tool call (e.g. inline skill
    /// expansion), the id of that tool use. Used for transcript grouping
    /// and to mark the message as transient until its parent tool resolves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
}
```

This matches TS `SkillTool.ts:728` `tagMessagesWithToolUseID`. Without
this field, the parent-tagging requirement in I5 / SkillRuntime cannot
be satisfied.

### ModelRuntime

The model layer needs a dedicated runtime because fallback model and streaming
retry cannot be implemented cleanly when `ApiClient` wraps exactly one model.

Responsibilities:

- Build `QueryParams`.
- Prepare provider wire tool definitions from the provider-agnostic
  `ToolId` catalog, including per-turn forward/reverse maps.
- Call streaming or non-streaming API.
- Translate every returned/streamed provider tool name back to `ToolId`
  before `QueryEngine` sees it.
- Track active model id.
- Switch to fallback model when provider indicates fallback.
- Retry non-streaming after streaming failure when safe.
- Handle prompt-too-long reactive compaction signal.
- Handle max-output-token escalation.
- Preserve token usage and request timing.

**Architecture: concrete struct, no trait.** This supersedes the earlier
draft that proposed a `trait ModelRuntime`. Per the project's
"async fn / async_trait" guidance and the "no trait without users" rule,
introduce a concrete `ModelRuntime` struct in `app/query`:

```rust
pub struct ModelRuntime {
    primary: Arc<ApiClient>,
    fallback: Option<Arc<ApiClient>>,
    active: ModelSlot,                 // Primary | Fallback
    tool_wire: ToolWirePolicy,          // coco-owned, provider-aware adapter
    cache_state: CacheBreakDetector,
}

enum ModelSlot { Primary, Fallback }

impl ModelRuntime {
    pub async fn run_turn(&mut self, params: ModelTurnParams)
        -> Result<ModelTurnOutput, ModelTurnError>
    { ... }

    pub fn current_model_name(&self) -> &str { ... }

    pub fn switch_to_fallback(&mut self) {
        self.active = ModelSlot::Fallback;
        self.cache_state.reset();      // I13
    }
}
```

For tests, use a `#[cfg(test)]` constructor that injects mock
`Arc<ApiClient>` instances. Do not introduce a `trait ModelRuntime`
preemptively.

`ModelTurnParams` takes the provider-agnostic tool catalog, not
`LanguageModelV4Tool` directly. `ModelRuntime` lowers that catalog to
provider wire names for the active client, stores the reverse map for
the turn, and returns `ModelTurnOutput` with tool calls already resolved
to `ToolId`. `ApiClient` / `vercel-ai-provider` may remain string-based
internally; that string boundary is hidden below `ModelRuntime`.

## Detailed Implementation Phases

Phase order is intentional. Earlier phases close correctness gaps; later
phases enable new behavior. Each phase is independently shippable.

### Phase 1: Delete Eager Bypass

Goal: close the most dangerous correctness gap (tools executing without
permission/hook checks during streaming).

The eager dispatch path at `engine.rs:1539` calls `tool.execute` directly.
Today it fires whenever a streamed tool input completes, **regardless** of
the `streaming_tool_execution` config flag. Gating it by config does not
fix the underlying bug — it would just hide it on the default path.

**Delete `try_eager_dispatch` entirely.** Streaming continues to stream
tokens (text, thinking, tool input deltas), but tool *execution* is
deferred until the assistant message is committed. This matches TS
semantics: in TS, `StreamingToolExecutor.executeTool()` calls the same
`runToolUse()` lifecycle. The Rust eager path was never TS parity.

Streaming tool *scheduling* (starting safe tools while later ones still
stream) returns in Phase 9 as a feature on top of `ToolCallRunner`, not
as a bypass.

Tasks:

- Delete `try_eager_dispatch` and its callers in `engine.rs`.
- Delete the duplicated permission/execute calls at `engine.rs:1513/1539`.
- Add `helpers::make_tool_error_message` (already exists; ensure all error
  paths use it).
- Use it for unknown tools (currently silently skipped in some paths).
- Use it for PreToolUse hook blocks.
- Use it for permission denials and approval rejections.
- **Lifecycle event ownership stays in `QueryEngine` until Phase 4.**
  `StreamingToolExecutor::with_event_sink`
  (`coco-rs/core/tool/src/executor.rs:181`) today emits only
  `TaskPanelChanged` patches (sends at `executor.rs:497` and `:733`);
  it does NOT yet emit `ToolUseQueued` / `ToolUseStarted` /
  `ToolUseCompleted`. This phase MUST NOT half-move those lifecycle
  events into the executor — doing so without the Phase 4 runner
  boundary in place leads to duplicate emission or dropped events
  during the transition. Concrete rule: Phase 1 leaves
  `ToolUseStarted` / `ToolUseCompleted` emission exactly where it is
  today (in `QueryEngine`'s post-dispatch path). Phase 4 — the same
  PR that introduces `ToolCallRunner` and removes the eager bypass's
  peer — extends `with_event_sink` to emit the full lifecycle triplet
  from the executor, and deletes the QueryEngine-side emission in the
  same commit. Either move lifecycle events in one step or not at
  all; no interleaved state.
- Add invariant tests: every committed assistant tool call produces exactly
  one tool result message with the matching `tool_use_id`.

Files:

- `coco-rs/app/query/src/engine.rs`
- `coco-rs/app/query/src/helpers.rs`
- `coco-rs/core/tool/src/executor.rs`
- `coco-rs/app/query/src/engine.test.rs`

Acceptance criteria:

- Unknown tool returns an error tool result.
- PreToolUse block returns an error tool result.
- Permission denial returns an error tool result.
- Eager dispatch path is gone (grep for `try_eager_dispatch` returns no
  hits in `engine.rs`).
- No assistant tool call is committed without a matching tool result.
- Streaming text/thinking deltas still flow.

### Phase 2: ToolContextFactory + Fix Hardcoded Fields

Goal: pure refactor — make `ToolUseContext` accurate. No behavior change
beyond fields the runtime should always have honored.

Tasks:

- Add `app/query/src/tool_context.rs` with `ToolContextFactory`.
- Move `create_tool_context` (`engine.rs:2786-2925`) into the factory.
- Fix the 5 hardcoded fields (`engine.rs:2801-2805`):
  `thinking_level`, `is_non_interactive`, `max_budget_usd`,
  `custom_system_prompt`, `append_system_prompt`.
- Snapshot `messages` from current history.
- Use injected `AgentHandleRef` (still `NoOpAgentHandle` until Phase 6 — but
  through the factory hook so Phase 6 can swap).
- Tests that the factory honors every config field listed in I6.
- **Wire `auto_compact_enabled` into the auto-compaction trigger.**
  Today `coco-rs/app/query/src/engine.rs:2415` and `:2434` call
  `coco_compact::should_auto_compact(tokens, window, max_out)`
  without consulting `self.config.auto_compact_enabled` — the flag
  currently only affects `calculate_token_warning_state`'s reminder
  state (`coco-rs/services/compact/src/auto_trigger.rs:76`), not the
  actual trigger. TS parity: `isAutoCompactEnabled()` short-circuits
  the trigger, not just reminders. Fix in this phase:
  1. Gate the two `should_auto_compact` sites in `engine.rs` on
     `self.config.auto_compact_enabled` (`&&` check before the call,
     since `should_auto_compact`'s existing three-argument signature
     is token-based and should not grow a bool parameter — the gate
     is a caller concern).
  2. Add a regression test in `engine.test.rs`:
     `auto_compact_disabled_skips_trigger_even_above_threshold`
     — build a history whose `estimate_tokens` exceeds
     `auto_compact_threshold`, set `auto_compact_enabled: false`,
     run `process_turn`, assert no `ContextCompacted` event and no
     mutation to `history.messages.len()`.
  3. A companion test with `auto_compact_enabled: true` asserts the
     trigger DOES fire under the same token conditions, so the gate
     is proven tight.

Files:

- `coco-rs/app/query/src/tool_context.rs`
- `coco-rs/app/query/src/tool_context.test.rs`
- `coco-rs/app/query/src/engine.rs`

Acceptance criteria:

- Every field in I6 is read from config or live state, not hardcoded.
- Factory is constructible in tests without a full `QueryEngine`.
- `auto_compact_enabled: false` blocks auto-compaction at the trigger,
  not just at the reminder layer (see two tests above).

### Phase 3: HookAdapter + Delete Executor Hook Calls

Goal: install a real `HookHandle` and remove the dormant duplicate hook
owner in `executor.rs`. **These two changes must ship together** — they
guard each other against double-firing hooks.

Tasks:

- Add `app/query/src/hook_adapter.rs` implementing `coco_tool::HookHandle`
  by calling into `coco_hooks::orchestration`.
- `QueryHookHandle` holds `Arc<HookRegistry>`, `OrchestrationContext`,
  optional event sender.
- Add `coco_hooks::orchestration::execute_post_tool_use_failure` wrapper
  (uses existing `PostToolUseFailureInput` from `inputs.rs:52` and existing
  `HookSpecificOutput::PostToolUseFailure` from `orchestration.rs:148`).
- Add `coco_hooks::orchestration::execute_permission_denied` wrapper
  using existing `PermissionDeniedInput`; expose it through
  `HookHandle::run_permission_denied` and `QueryHookHandle`.
- Install `QueryHookHandle` into `ToolUseContext` via `ToolContextFactory`.
- **Delete** hook calls in `executor.rs:394-418` and `570-655`. Executor
  no longer touches `ctx.hook_handle`.
- Update executor tests to mock-call the runner instead of expecting hook
  calls.

Files:

- `coco-rs/app/query/src/hook_adapter.rs`
- `coco-rs/app/query/src/hook_adapter.test.rs`
- `coco-rs/hooks/src/orchestration.rs`
- `coco-rs/hooks/src/inputs.rs`
- `coco-rs/core/tool/src/hook_handle.rs`
- `coco-rs/core/tool/src/executor.rs`
- `coco-rs/core/tool/src/executor.test.rs`

Acceptance criteria:

- Hook adapter tests cover PreToolUse, PostToolUse,
  PostToolUseFailure, and PermissionDenied.
- `grep "hook_handle" core/tool/src/executor.rs` returns no production
  references (test setup may still construct one).
- `AggregatedHookResult` field mapping table (in HookAdapter section
  below) is implemented and tested.

### Phase 4: ToolCallRunner

Goal: one non-streaming tool lifecycle. **Split into 5 sub-PRs** to keep
review surface manageable.

Sub-PR 4a: Tool identity + full-schema catalog
- Migrate `ToolRegistry` execution lookup from `String` to `ToolId`.
- Add `ToolDefinitionEntry` / `RegisteredTool` with intrinsic metadata
  only: `tool_id`, `telemetry_name`, and full JSON Schema.
- Add `ToolId::as_pattern_string()` in `coco-types`; keep reverse
  parsing in `coco-permissions` config loading.
- Add `core/tool/src/schema.rs` with `effective_tool_schema()` and
  cached `ToolSchemaValidator`.
- Add a coco-owned tool catalog / wire adapter in `app/query`
  (`tool_catalog.rs` + `tool_wire.rs`, or equivalent). This adapter
  prepares `LanguageModelV4Tool` for the active provider and records a
  per-turn `wire -> ToolId` reverse map. It must not live in any
  standalone `vercel-ai-*` provider crate.
- Current `engine.rs` may use this adapter directly until Phase 8 moves
  ownership under `ModelRuntime`.

Sub-PR 4b: `ToolCallMessageBuilder`
- Extract scattered message-building helpers into one module.
- No behavior change. Pure refactor.

Sub-PR 4c: Fix `core/tool/src/execution.rs` order + promote
- Today `execution.rs` (288 LOC) strips internal fields BEFORE validation
  (line 105 strip, line 140 validate). This is the wrong order per I3.
- **First** flip the order so `validate_input` runs before
  `strip_internal_bash_fields`. Rename the existing entry-point or split
  into two functions: `validate_input_then_strip(...)` returning the
  stripped+validated input. Do **not** keep the misleading name
  `sanitize_and_validate`.
- **Then** make both engine paths call into the corrected helper.
- Engine batch path now uses one validation source with the right order.

Sub-PR 4d: `ToolCallRunner::run_one` + executor callback API
- New file `app/query/src/tool_runner.rs`.
- `prepare_batch` is sole owner of tool resolution, schema validation
  (via the new `core/tool/src/schema.rs` validator), `model_index`
  assignment (tool_use position per I12 — NOT history-append slot;
  `completion_seq` is stamped later by the executor), and
  `ToolCallPlan::EarlyOutcome` synthesis on resolution / schema
  failure. JSON parsing is **not** here — it has already happened
  upstream in the assistant-message accumulator (see "JSON Parse
  Failures Are Pre-Commit, Not Pre-Batch" under `ToolCallRunner`).
- `run_one` consumes a fully-prepared `PreparedToolCall` and orchestrates
  per I3: build ToolUseContext → tool.validate_input (with fresh context)
  → strip internal fields → pre-hook → re-validate hook-rewritten input
  → permission (via PermissionController) → execute → on-success
  PostToolUse / on-execution-exception PostToolUseFailure → build
  ToolMessageBuckets per I5 → build UnstampedToolCallOutcome with
  `effects.app_state_patch` populated → return. The executor calls
  `stamp_and_extract_effects(next_seq)` when the future resolves,
  splits the unstamped body into a patch-free `ToolCallOutcome` plus
  `ToolSideEffects`, and applies the patch at the correct moment
  (serial: pre-next-tool; concurrent: end-of-batch under one write
  lock, in `model_index` order). The runner does NOT do tool lookup
  or initial schema validation (those are `prepare_batch`'s job); it
  DOES re-run the cached schema validator against hook-rewritten
  input. It does NOT apply the patch.
- `StreamingToolExecutor` API changes to
  `execute_with(plans, run_one, on_outcome)` (see Scheduling Contract
  for the full signature: `run_one` returns `UnstampedToolCallOutcome`;
  executor stamps `completion_seq` at surface time and hands the
  stamped `ToolCallOutcome` to `on_outcome` one-by-one — no
  `Vec<ToolCallOutcome>` return, no end-of-batch accumulation). It
  schedules but never builds outcomes.
- Executor owns: partition, concurrency, sibling abort, per-tool
  cancellation tokens, app_state_patch application between serial tools.
- Engine batch path replaced call-site by call-site.

Sub-PR 4e: `ToolCallRunner::run_many`
- Replaces the entire engine.rs:1899-2200 block.
- Streams outcomes as they complete via `FuturesUnordered` plus the
  executor's max-concurrency semaphore (per I12) — no pre-allocated
  result-slot vector. The current
  submission-order pattern at `coco-rs/core/tool/src/executor.rs:682-699`
  (`for handle in handles { handle.await }`) blocks the whole batch on
  the slowest tool and must be removed in this sub-PR.
- For concurrent-safe batches, the executor calls
  `stamp_and_extract_effects(next_seq)` as each future resolves. The
  returned patch-free `ToolCallOutcome` is handed to the runner's
  history-append callback immediately (completion-order history). The
  extracted `ToolSideEffects.app_state_patch` is queued keyed by
  `model_index` for post-batch application in model_index order under
  one write lock (model-order state mutation — matches TS
  `toolOrchestration.ts:54-62`). Patch ownership never crosses into
  the history-facing outcome, so there is no move-conflict between
  "hand to on_outcome" and "apply later". I9 errors drop the patch
  (FnOnce destructor releases captured state without invoking).
- Engine.rs shrinks by ~300 LOC.

Files:

- `coco-rs/app/query/src/tool_runner.rs`
- `coco-rs/app/query/src/tool_message.rs`
- `coco-rs/app/query/src/tool_catalog.rs`
- `coco-rs/app/query/src/tool_wire.rs`
- `coco-rs/app/query/src/permission_controller.rs`
- `coco-rs/app/query/src/engine.rs`
- `coco-rs/common/types/src/tool.rs`
- `coco-rs/core/tool/src/registry.rs`
- `coco-rs/core/tool/src/schema.rs`
- `coco-rs/core/tool/src/execution.rs`
- `coco-rs/core/tool/src/executor.rs`

Acceptance criteria:

- Validation runs before permission.
- Registry execution lookup is keyed by `ToolId`, not string names.
- Provider wire names are prepared in the coco adapter layer and
  translated back to `ToolId` before tool execution.
- Hook-updated input is used for permission and execution.
- Post hooks receive the effective input (not `Value::Null`).
- `ToolResult::new_messages` are appended after each tool result.
- Result message order matches I12.
- Existing tool execution tests still pass.

### Phase 5: Dynamic Tool Prompts

Goal: make Agent and Skill prompts match available runtime state.

Independent of phases 6-9 — can land any time after Phase 2.

This is a deliberate Rust-side divergence from TS, which lists skills via
system reminders (`SkillTool/prompt.ts:189`). See "Skill prompt behavior"
section above for rationale.

Tasks:

- Add `skill_names: Vec<String>` field to `PromptOptions`
  (`core/tool/src/traits.rs:56`). Sorted for determinism.
- Make the tool catalog builder async.
- Build `PromptOptions` from current tools, agents, skills, permission context,
  and non-interactive mode.
- Call `tool.prompt(&PromptOptions)` instead of only `tool.description`.
- Implement dynamic `prompt` for `AgentTool`.
- Implement dynamic `prompt` for `SkillTool` using
  `coco_skills::generate_skill_tool_prompt`.

Files:

- `coco-rs/core/tool/src/traits.rs`            (PromptOptions field)
- `coco-rs/app/query/src/engine.rs`
- `coco-rs/core/tools/src/tools/agent.rs`
- `coco-rs/core/tools/src/tools/agent_spawn.rs`
- `coco-rs/skills/src/lib.rs`

Acceptance criteria:

- Tool definitions include current agent listing.
- Tool definitions include current skill listing.
- Disabled or model-hidden skills are not advertised to the model.

### Phase 6: AgentRuntime

Goal: make AgentTool actually run subagents.

Tasks:

- Add `QueryEngine::with_agent_handle`.
- Ensure TUI, SDK, and headless runners install a real handle.
- Rework `SwarmAgentHandle::spawn_subagent` to use the existing
  `coco_tool::AgentQueryEngine` trait. Do **not** introduce a new trait.
- Rename `InProcessAgentRunner::spawn_agent` to `register_agent`. Add
  explicit `start_agent`. Update callers.
- Rework background subagent handling to spawn and store output.
- Rework teammate spawn to actually start `run_in_process_teammate`.
- Document that `coco_tool::AgentQueryEngine` (side queries) and
  `swarm_runner_loop::AgentExecutionEngine` (in-process teammate loop)
  serve different purposes. Both stay.
- Fix `QueryEngineAdapter` to return final messages and tool counts.
- Propagate allowed tools, permission mode, session id, context messages, and
  bypass capability.

Files:

- `coco-rs/app/query/src/engine.rs`
- `coco-rs/app/query/src/agent_adapter.rs`
- `coco-rs/app/state/src/swarm_agent_handle.rs`
- `coco-rs/app/state/src/swarm_runner.rs`
- `coco-rs/app/state/src/swarm_runner_loop.rs`
- `coco-rs/app/cli/src/tui_runner.rs`
- `coco-rs/app/cli/src/sdk_server/sdk_runner.rs`

Acceptance criteria:

- ✅ `AgentTool` sync call runs a child query and returns real output.
- ✅ Background AgentTool returns `AsyncLaunched` and stores result —
  `coordinator/src/agent_handle/spawn.rs:1278` is the real `tokio::spawn` site;
  event drain (`:1121`) and summary timer (`:1157`) confirm execution.
- ✅ Teammate AgentTool starts the teammate loop.
- ✅ Child agent receives restricted tools — `ToolFilter::narrow_with(parent)`
  in `agent_adapter.rs:207` + per-context enforcement at
  `core/tool-runtime/src/registry.rs`.
- ✅ Child query returns final messages and tool counts.
- ✅ `InProcessAgentRunner` exposes `register_agent` + `start_agent`; old
  `spawn_agent` is removed.

Verified 2026-05-15 against actual sources.

### Phase 7: SkillRuntime

Goal: make SkillTool behavior match TS.

Tasks:

- Add `SkillHandle` trait in `coco-tool` (signature in SkillRuntime section
  above; uses `SkillInvocationError` snafu enum, not `Result<_, String>`).
- **Delete** `AgentHandle::resolve_skill` and `SwarmAgentHandle::resolve_skill`.
  The current implementation is a stub returning `Err`.
- Install `SkillHandle` into `ToolUseContext` via `ToolContextFactory`.
- Implement inline skill expansion with `new_messages` tagged with parent
  `tool_use_id`.
- Implement forked skill execution through `AgentRuntime`.
- Track invoked skills for reminders.
- Enforce disabled and model-invocation restrictions.
- Reuse existing `skill_advanced` expansion helpers.

Files:

- `coco-rs/common/types/src/message.rs`          (add `parent_tool_use_id` to `UserMessage`)
- `coco-rs/core/tool/src/context.rs`
- `coco-rs/core/tool/src/skill_handle.rs`        (new)
- `coco-rs/core/tool/src/agent_handle.rs`        (delete `resolve_skill`)
- `coco-rs/app/query/src/skill_adapter.rs`       (new — implements SkillHandle)
- `coco-rs/app/state/src/swarm_agent_handle.rs`  (delete `resolve_skill` impl)
- `coco-rs/core/tools/src/tools/agent.rs`        (SkillTool routes through SkillHandle)
- `coco-rs/core/tools/src/tools/skill_advanced.rs`
- `coco-rs/skills/src/lib.rs`

Acceptance criteria:

- Inline skill appends expanded prompt messages with parent tool-use id.
- Fork skill runs a child agent and returns through the same tool result
  pipeline as AgentTool.
- `new_messages` are visible in the next model request.
- Disabled skills fail with an error tool result (not a panic).
- Hidden/model-disabled skills are not advertised in SkillTool prompt.
- `grep "resolve_skill" coco-rs/` returns no hits.

### Phase 8: ModelRuntime + Cache Reset On Fallback

Goal: align fallback, streaming retry, and max-token recovery with TS.

Tasks:

- Introduce a concrete `ModelRuntime` struct holding primary
  `Arc<ApiClient>` + `Option<Arc<ApiClient>>` fallback. No new trait
  unless tests demand mocking — `cfg(test)` seams suffice.
- Move the Phase 4 tool-wire adapter under `ModelRuntime` ownership:
  each turn prepares provider wire tools, keeps the reverse map, and
  returns typed `ToolId` tool calls to `QueryEngine`.
- Honor `fallback_model`.
- Track active model after fallback.
- Ensure `ToolUseContext.main_loop_model` reflects active model.
- **Reset `CacheBreakDetector` state on provider switch (I13).** Cache
  pointers from provider A are nonsense to provider B. This is a
  correctness requirement.
- Gate max-output escalation according to TS behavior and user/env overrides.
- Implement non-streaming retry after streaming failure where appropriate.
- Keep token-budget continuation behind config.

Files:

- `coco-rs/app/query/src/model_runtime.rs`
- `coco-rs/app/query/src/engine.rs`
- `coco-rs/services/inference/src/client.rs`
- `coco-rs/app/cli/src/model_factory.rs`

Acceptance criteria:

- Fallback model is used when configured and triggered.
- Active model name updates after fallback.
- `CacheBreakDetector` is reset on switch — verified by test that asserts
  cache state is cleared.
- Max-token escalation does not override explicit user intent incorrectly.
- Streaming failure retry does not leave dangling tool results.

### Phase 9 (Optional): Streaming Tool Scheduling

Goal: re-add streaming tool scheduling as a feature on top of
`ToolCallRunner` — never as a bypass.

Only attempt after Phase 4 has stabilized in production.

Tasks:

- `ToolCallRunner::run_one` accepts an optional "start when ready" hook
  fed by the streaming accumulator.
- Defer all API-visible message emission until the assistant message is
  committed.
- If streaming retry/fallback occurs, discard any in-flight streamed tool
  work that was not committed.
- Synthetic error results only for tool calls that already became part of
  the committed assistant message.
- Gate behind `streaming_tool_execution` config; default off until proven.

Acceptance criteria:

- Streaming and non-streaming go through the same `ToolCallRunner`.
- No tool results emitted for uncommitted assistant messages.
- Disabling the gate restores Phase 4 behavior exactly.

## Test Strategy

Follow the repository rule: no inline `#[cfg(test)] mod tests`. Use companion
`*.test.rs` files with `#[path = "..."]`.

### Unit Tests

`app/query/src/tool_runner.test.rs`:

- unknown tool produces one error tool result
- invalid input produces one error tool result
- validation runs before permission
- pre-hook deny produces one error tool result
- pre-hook input rewrite reaches permission and execution
- permission denial records audit info and produces one result
- post-hook output rewrite changes tool result (MCP tool — `updated_mcp_tool_output` applies)
- post-hook output rewrite is IGNORED for non-MCP tools (TS parity: `toolHooks.ts:145` `isMcpTool` guard)
- auto-mode classifier permission denial fires `PermissionDenied` hook
  in addition to returning an error tool result (TS parity:
  `toolExecution.ts:1073`)
- non-classifier permission denial returns an error tool result without
  firing `PermissionDenied` hook
- cancellation before `tool.execute()` maps to `PreExecutionCancelled` and does NOT run `PostToolUseFailure`
- cancellation during `tool.execute()` maps to `ExecutionCancelled` and DOES run `PostToolUseFailure`
- post-hook failure receives effective input and error
- `new_messages` are appended after tool result
- prevent-continuation attachment stops the loop
- non-MCP success: pre_hook → tool_result → post_hook → new_messages →
  prevent-continuation (in that exact order in history)
- MCP success: pre_hook → tool_result → new_messages → prevent-continuation
  → post_hook (post-hook deferred to the end)
- execution failure (`ToolMessagePath::Failure`): pre_hook → tool_result
  → post_hook_failure, with `prevent_continuation_attachment == None`
  enforced (failure path never emits prevent, matching TS catch return
  at `toolExecution.ts:1715-1737`)
- EarlyOutcome plan emits `ToolUseQueued` and `ToolUseCompleted`
  (synthetic outcome) but never `ToolUseStarted`; per-call invariant
  one Queued → exactly one Completed holds
- schema validation uses `effective_tool_schema(tool)` and validates
  against the **full** JSON Schema returned by the tool (via
  `input_json_schema()` override or the migrated `Tool::input_schema()`
  default — both branches return a complete document, not a properties
  sub-map). Phase 4 acceptance requires full parity with TS Zod
  validation: `required`, `additionalProperties: false`, nested object
  shapes, enum values, array item type / length constraints,
  oneOf/anyOf, and format all reject inputs the same way TS does.
  **Hard acceptance gates (mechanical, not soft criteria):**
  - `effective_tool_schema()` has no wrap-properties branch (the
    function body must not construct `{"type": "object", "properties":
    <map>}` from a `HashMap<String, Value>` anywhere — grep-enforced
    in CI).
  - A parametrized test iterates every registered built-in tool and
    asserts `effective_tool_schema(tool)` for each tool:
    1. is a JSON object with root `"type": "object"` (rejects the
       properties-only shape, which has no root `"type"`);
    2. the set of root keys is NOT exactly `{"properties"}` — a
       legitimate full schema may be as small as `{type, properties}`,
       but a document whose only root key is `properties` is the
       legacy sub-map shape and fails this gate;
    3. **Narrow pathological-shape reject** — targets the specific
       double-wrap produced by `serde_json::to_value(ToolInputSchema)`
       against the old properties-only definition, nothing more. The
       gate checks: if the root has `properties` AND
       `properties.properties` exists AND
       `properties.properties` is a JSON object whose every value is
       itself a JSON object lacking `type` / `$ref` (i.e. looks like
       an old `HashMap<String, Value>` entry-set rather than a
       schema-valued property), fail. This is deliberately narrow.
       The gate does NOT reject:
       - boolean schemas (`true` / `false`);
       - schemas driven by `const` / `not` / `if` / `then` / `else` /
         `patternProperties` / `additionalProperties`-only objects;
       - any other legitimate JSON Schema constructs a tool (or an
         MCP / plugin server) might advertise.
       These non-double-wrap shapes go through and are covered by
       gate 4 syntactic compile + behavioral parity tests below.
    4. compiles successfully through `jsonschema::Validator::new` —
       sanity check that the document is at least syntactically
       well-formed; not the structural guard.

    A root object with all-optional primitive properties (no
    `required`, no `additionalProperties`, no nested objects) is
    ACCEPTED — it is a legal full JSON Schema, and many read-only
    inspector tools have exactly this shape.
  - Representative-coverage parity tests (separate from the per-tool
    gates above) exercise real tool inputs end-to-end against their
    `effective_tool_schema` to verify behavioral TS-Zod parity:
    `required` field missing → reject; unknown field with
    `additionalProperties: false` → reject; `enum` value mismatch →
    reject; `array` item-type / length violation → reject;
    `oneOf` / `anyOf` mismatch → reject; nested object type mismatch
    → reject; and representative accepted inputs → pass. These cover
    the actual TS parity surface, not just the structural shape.
  - MCP / plugin tools pass through whatever full schema the remote
    advertises. Gates (1), (2), and (4) apply — gate (3)'s narrow
    double-wrap check also applies, but only the specific
    `properties.properties` old-map shape is rejected, so legitimate
    MCP schemas using `patternProperties`, boolean schemas, `oneOf`,
    `const`, etc. pass through unchanged.
- `PreToolUse` hook returning schema-invalid `updated_input` produces a
  validation error **before** permission/execution (the cached
  `ToolSchemaValidator` runs a second time against the rewritten input);
  the tool is not executed, and one error tool result is emitted
- JSON parse failure upstream in the accumulator emits no
  `ToolUseQueued` / `ToolUseStarted` / `ToolUseCompleted` and no
  tool_result (the tool_use is dropped before the assistant message
  commits); a `warn!` is logged with `tool_use_id` and `tool_name`
- I12 history ordering — concurrent-safe batch: A is slow, B is fast →
  history append order is `[B, A]`, not `[A, B]`. The executor uses
  a completion stream; submission-order awaits are a regression.
- I12 state mutation ordering — same batch, both A and B emit
  `app_state_patch` → after the batch completes, patches apply in
  `model_index` order (`[A, B]`) regardless of completion order.
  Verify by asserting the post-batch app-state matches `apply(B, apply(A, before))`,
  not `apply(A, apply(B, before))`.
- I12 serial unsafe — A then B serial → history is `[A, B]` (execution
  order = model order); A's patch is applied before B's
  `ToolUseContext` is built (B observes A's mutation).
- I12 streaming (Phase 9) — tool finishes before assistant message
  commits → its append is queued; on commit, queued outcomes flush in
  real `completion_seq` order, not re-derived model order.
- Schema parity — `required` missing, unknown field with
  `additionalProperties: false`, nested object type mismatch, enum
  value mismatch, array length / item-type violation: each one
  produces a `SchemaFailed` `ToolCallErrorKind` exactly as TS would
  reject the equivalent Zod schema. Tests cover semantic parity
  (accept/reject), not textual parity (Zod-vs-jsonschema message
  differences are out of scope for Phase 4).
- Schema-invalid barrier — assistant message has tool_use entries
  `[safe_A, schema_invalid_B, safe_C]` → executor partition produces
  three blocks: `[safe_A]` (concurrent), `[schema_invalid_B]`
  (EarlyOutcome, not concurrent), `[safe_C]` (concurrent). `safe_A`
  and `safe_C` MUST NOT share a batch. The EarlyOutcome appends in
  partition order — the executor stamps its `completion_seq` when it
  reaches that barrier block, so it lands between `safe_A`'s and
  `safe_C`'s completion seqs regardless of wall-clock race between
  `safe_A` and `safe_C`'s `run_one` futures.

`app/query/src/tool_context.test.rs`:

- config fields propagate into `ToolUseContext`
- history is available in context
- app-state live permission mode is used
- agent and hook handles are installed
- query depth and agent id are preserved

`app/query/src/hook_adapter.test.rs`:

- PreToolUse aggregation maps to `PreToolUseOutcome`
- PostToolUse aggregation maps to `PostToolUseOutcome`
- PostToolUseFailure uses structured input
- updated input maps correctly
- `updated_mcp_tool_output` maps correctly and is ignored for non-MCP
  execution by the runner

`app/query/src/agent_adapter.test.rs`:

- final messages are returned
- tool use count is returned
- allowed tools are honored
- permission mode is propagated
- session id and agent id are propagated

`core/tools/src/tools/agent.test.rs`:

- AgentTool maps sync result shape
- AgentTool maps async launched result shape
- AgentTool maps teammate spawned result shape
- SkillTool inline uses skill runtime
- SkillTool fork uses agent runtime

`skills/src/lib.test.rs` or runtime tests:

- aliases resolve
- disabled skills reject
- model-disabled skills are hidden from prompt
- argument expansion matches TS expectations

### Integration Tests

Add query-engine tests that simulate model output with tool calls:

- one unknown tool call
- one valid tool call
- multiple concurrent-safe tool calls
- mixed safe and unsafe tool calls
- hook stop
- streaming disabled
- streaming enabled but fallback occurs
- AgentTool sync child call
- SkillTool inline call

### Commands

Run from `coco-rs/`:

```bash
just test-crate coco-tool
just test-crate coco-tools
just test-crate coco-query
just test-crate coco-state
just check
just clippy
```

If shared crates change, run:

```bash
just test
just pre-commit
```

## PR Breakdown

Do not ship this as one giant PR. PRs map 1:1 to phases except Phase 4
which splits into 4 sub-PRs.

### PR 1: Delete Eager Bypass

- Delete `try_eager_dispatch` from `engine.rs`.
- Unknown/hook-blocked tools produce error results via existing
  `helpers::make_tool_error_message`.
- Add invariant tests for "every committed tool call gets one result."

### PR 2: ToolContextFactory + Fix Hardcoded Fields

- Add `tool_context.rs` with `ToolContextFactory`.
- Fix the 5 hardcoded fields in `create_tool_context`.
- Pure refactor; no behavior change beyond fields that should always
  have been honored.

### PR 3: HookAdapter + Delete Executor Hook Calls (atomic)

- Add `QueryHookHandle` implementing `coco_tool::HookHandle`.
- Add `execute_post_tool_use_failure` wrapper in `coco-hooks`.
- Delete hook calls in `executor.rs:394-418/570-655` in the **same PR**
  to prevent double-firing once `QueryHookHandle` is installed.

### PR 4a: ToolCallMessageBuilder

- Pure extraction of message-building helpers. No behavior change.

### PR 4b: Fix execution.rs order, then promote

- Flip the strip-vs-validate order in `core/tool/src/execution.rs` so
  validation runs first (per I3). Rename helper to
  `validate_input_then_strip` (or split into two steps).
- Make both engine paths call into the corrected helper.
- Both engine paths use one validation source with TS-correct order.

### PR 4c: ToolCallRunner::run_one

- New `tool_runner.rs`.
- Engine batch path replaced call-site by call-site.
- Executor reduced to pure scheduler.

### PR 4d: ToolCallRunner::run_many

- Replaces engine.rs:1899-2200 block.
- Drives `StreamingToolExecutor::execute_with` with an `on_outcome`
  callback that appends each outcome to history the moment it arrives
  (completion order for concurrent-safe batches, execution order for
  serial, partition order for EarlyOutcome barriers). No
  pre-allocated result slots, no batch-end re-sort — history grows in
  real arrival order per I12. State-mutation patches (`app_state_patch`)
  are the only thing queued and replayed in `model_index` order, and
  only within a single concurrent-safe batch.

### PR 5: Dynamic Prompts

- Make tool definition building async.
- Use `Tool::prompt`.
- Add Agent/Skill dynamic prompts.

### PR 6: AgentRuntime

- Wire real AgentHandle through `QueryEngine::with_agent_handle`.
- Reuse existing `coco_tool::AgentQueryEngine`.
- Rename `spawn_agent` → `register_agent` + add `start_agent`.
- Fix sync/background/teammate subagent paths.

### PR 7: SkillRuntime

- Add `parent_tool_use_id: Option<String>` to `UserMessage` schema.
- Add `SkillHandle` trait with snafu-typed errors.
- Delete `AgentHandle::resolve_skill` everywhere.
- Implement inline and fork skills.
- Preserve `new_messages` with parent tool-use id tagging.

### PR 8: ModelRuntime + Cache Reset

- Add concrete `ModelRuntime` (no new trait yet).
- Honor `fallback_model`.
- Reset `CacheBreakDetector` on provider switch (I13).
- Align max-token recovery and streaming retry.

### PR 9 (Optional): Streaming Tool Scheduling

- Re-add streaming scheduling on top of `ToolCallRunner`.
- Gated by `streaming_tool_execution`; default off.

## Rust Design Guidelines

### Keep `engine.rs` Small

`engine.rs` is already large. New behavior should go into new modules unless it
is truly part of the high-level loop.

### Prefer Traits At Crate Boundaries

Use traits where dependency direction would otherwise invert:

- `HookHandle`: `core/tool` -> implemented by `app/query`
- `AgentHandle`: `core/tools` -> implemented by `app/state` or `app/query`
- `SkillHandle`: `core/tools` -> implemented by `app/query` or `skills` adapter

`ModelRuntime` is a **concrete struct** in `app/query`, not a trait. See
the ModelRuntime section above. Test seams use `#[cfg(test)]` constructors
that inject mock `Arc<ApiClient>`; no trait surface is introduced.

### Prefer Typed Results Over Booleans

Avoid APIs like:

```rust
run_tool(input, true, false, None)
```

Use enums or option structs:

```rust
ToolRunOptions {
    scheduling: ToolScheduling::Streaming,
    user_message_id,
    turn_id,
}
```

### Keep Scheduling Separate From Semantics

`StreamingToolExecutor` should decide when tools run. It should not decide what
a tool call means. The lifecycle must live in one place.

### Preserve The Two-Axis I12 Ordering

Three orderings, three different keys — do not collapse them:

- **Model-visible history (`tool_result` append order):** completion
  order for concurrent-safe batches (a slow earlier tool must not block
  a faster later tool); execution order for serial unsafe tools (which
  equals model order by construction); partition order for EarlyOutcome
  barriers (they split the surrounding safe batches).
- **Shared state mutation (`app_state_patch` / context modifiers):**
  model order, applied after the concurrent-safe batch under one write
  lock — this is the only place "deterministic model order" applies.
- **Tool-use commit into the assistant message:** model order
  (the assistant message itself is written once, in the order the model
  emitted the tool_use blocks).

"API-visible results are deterministic" is **not** an I12 goal for
concurrent-safe batches — history order for those batches is the real
completion order and will vary across runs when tool latencies vary.

### Make Error Results Explicit

Do not silently skip a tool call. Skipping creates invalid message history.
Every error should be model-visible unless it is a pure UI/protocol issue.

### Use Existing Helpers

Do not reimplement existing logic:

- use `skill_advanced::expand_skill_prompt`
- use `agent_spawn::load_agents_from_dirs`
- use `agent_advanced::resolve_agent_tools`
- use `helpers::make_tool_error_message`
- use `coco_messages` for message creation and normalization

### Avoid Holding Locks Across `.await`

`ToolContextFactory`, `AgentRuntime`, and `SkillRuntime` will touch shared state.
They should clone snapshots before async work whenever possible.

Preferred pattern:

```text
read lock
  -> copy the fields needed for this call
drop lock
  -> await model/tool/agent work
write lock briefly
  -> apply one patch or state transition
```

Avoid:

- holding `RwLock` guards while calling a model
- holding app-state locks while running tools
- holding skill-manager locks while launching agents
- calling hook or permission bridge code while a state lock is held

### Keep Data Transfer Types Narrow

Do not pass `QueryEngine` or `QueryEngineConfig` deep into every helper. Prefer
small DTOs:

- `ToolRunOptions`
- `ToolContextOptions`
- `ModelTurnParams`
- `AgentRuntimeConfig`
- `ResolvedSkillInvocation`

This makes tests simpler and prevents new modules from depending on unrelated
engine state.

### Use Exhaustive Enums

Use enums for control-flow concepts that TS represents as strings:

- tool scheduling mode
- agent invocation mode
- skill invocation kind
- permission override behavior
- synthetic tool error kind
- model turn stop reason

Avoid wildcard arms unless the enum is intentionally open to provider-specific
values. When an enum grows, the compiler should point to every place that needs
review.

### Keep Module Sizes Under Control

Do not make `engine.rs`, `agent.rs`, or `executor.rs` larger while adding this
work. If a file is already large, add focused modules:

- `tool_runner.rs`
- `tool_message.rs`
- `tool_context.rs`
- `permission_controller.rs`
- `hook_adapter.rs`
- `agent_runtime.rs`
- `skill_runtime.rs`
- `model_runtime.rs`

Tests should live in companion `*.test.rs` files and be referenced with
`#[path = "..."]`, following repository policy.

### Use Structured Errors At Boundaries

Inside app/query, prefer typed errors until the outermost boundary converts to
`anyhow::Result`. Use snafu enums per project policy (see `coco-rs/CLAUDE.md`
Error Handling section).

**Scope of the snafu rule:**

- **New traits introduced by this refactor** (`SkillHandle`, `ModelRuntime`
  helpers if any, `PermissionDenyReason`, `ToolCallErrorKind`,
  `ModelTurnError`): MUST use snafu enums. Never `Result<_, String>`.
- **Existing `coco-tool` traits with established error contracts**:
  - `AgentHandle` (`agent_handle.rs:127–187`) returns `Result<_, String>`
    on 9 methods. Migration to snafu is **out of scope** for this
    refactor. Track separately; the existing API is stable.
  - `AgentQueryEngine` (`agent_query.rs:122`) returns `anyhow::Result`.
    This matches the app-layer convention (see project CLAUDE.md error
    table). Keep as-is.
- **App/query internal types** (`ToolCallRunner`, `PermissionController`,
  `ModelRuntime` struct): use typed snafu errors internally; convert to
  `anyhow::Result` only at the outermost engine boundary.

The blanket "no `Result<_, String>` anywhere" rule was too broad; scope it
to new types only. Migrating existing traits is a separate cleanup.

```rust
#[derive(Debug, snafu::Snafu)]
pub enum ModelTurnError {
    #[snafu(display("prompt too long (retryable={retryable})"))]
    PromptTooLong { retryable: bool },
    #[snafu(display("provider error: {status_code:?}"))]
    Provider { status_code: Option<StatusCode> },
    #[snafu(display("stream interrupted"))]
    StreamInterrupted,
}
```

For model-visible tool failures, prefer `ToolCallErrorKind` plus a stable
message builder instead of free-form strings scattered across modules.

### Use `async fn` In Trait For Internal Types; `async_trait` Only For Trait Objects

Rust 2024 native `async fn` in trait is stable. Use it for internal types
that aren't dyn-dispatched. Reserve `#[async_trait::async_trait]` for trait
objects (`Arc<dyn Trait>` cross-crate boundaries):

- `Arc<dyn Tool>` — needs `async_trait` (already does).
- `Arc<dyn AgentHandle>`, `Arc<dyn HookHandle>`, `Arc<dyn SkillHandle>` —
  need `async_trait`.
- `ModelRuntime` if concrete (no trait) — native `async fn`. Add a
  `#[cfg(test)]` mock seam if tests need it; do not add a trait
  preemptively.
- `PermissionController`, `ToolContextFactory`, `ToolCallRunner` — concrete
  structs with `async fn` methods. No trait surface.

### Use `ToolId`, Not `String`, For Tool Identity

Providers return wire strings. Translate them through the per-turn
`PreparedToolSet` reverse map into `ToolId` at the `ModelRuntime`
boundary, before `QueryEngine` or `ToolCallRunner` sees the call. Per
`coco-rs/CLAUDE.md` Canonical Names, `ToolId` is the identity type;
`String` is for permission patterns (`ToolPattern`), provider wire
strings below `ModelRuntime`, and unconstrained user input only.

Same rule for `AgentTypeId`, `ModelRole`, `HookEventType`, etc. — never
introduce a raw-string field for a closed set.

### Add `tracing` Spans Per Stage

A runner that owns the lifecycle is exactly the right place for spans:

```rust
#[tracing::instrument(
    skip(self, runtime),
    fields(
        tool_use_id = %job.invocation.tool_use_id,
        tool = %job.invocation.tool_id,
        model_index = job.model_index,
    )
)]
async fn run_one(&self, job: PreparedToolCall, runtime: RunOneRuntime)
    -> UnstampedToolCallOutcome
{ ... }
```

Stages worth their own span: `validate_input`, `pre_hook`, `permission`,
`execute`, `post_hook`, `build_message`. coco-otel emits these as
structured events.

### Discard `app_state_patch` On Tool Error

A tool that returns `Err` discards its patch. Today
`executor.rs:744-748` already handles this for the no-app-state case;
extend the rule explicitly: error results never apply patches. Test must
cover this — silent patch leakage on errors would cause invisible state
drift.

### Make Unsupported Parity Explicit

If TS supports a behavior that Rust cannot support in the first slice, represent
it explicitly:

- parse the option
- keep it in the typed mode enum
- return a clear model-visible unsupported result
- add a TODO with the TS source file path and behavior
- add a test so it does not silently change later

This is especially important for remote agents, worktree isolation, remote
skills, and streaming tool execution during fallback.

## Non-Goals For The First Refactor

These items should not block the first safety-focused PRs:

- full remote agent execution parity
- full remote skill execution parity
- complete worktree lifecycle cleanup
- UI polish for new progress events
- provider-specific fallback heuristics beyond the existing config contract
- rewriting all tool implementations
- changing public tool schemas unless TS parity requires it
- large unrelated cleanup of `coco-state` or TUI code

The early PRs should make the existing loop safe and structurally clear. Deeper
AgentTool and SkillTool parity can land after the runner and context boundaries
are stable.

## Implementation Acceptance Checklist

Use this checklist during code review.

### Tool Runner Checklist

- Every committed tool call gets one result.
- Unknown tool, invalid input, validation failure, hook block, permission denial,
  cancellation, and execution error all produce synthetic results.
- Validation runs before permission.
- Hook-updated input is revalidated.
- Effective input reaches permission, execution, and post hooks.
- `new_messages` are appended after the result message.
- App-state patch application has one owner.
- Streaming and non-streaming paths call the same runner.

### Context Checklist

- `ToolUseContext` is built by one factory.
- Context fields are snapshots, not live locks where avoidable.
- Active fallback model is reflected in `main_loop_model`.
- Agent, hook, mailbox, MCP, task, todo, and permission handles are installed.
- Query depth and agent id are propagated.
- Non-interactive mode and budget fields are honored.

### Agent Checklist

- Normal sessions never install `NoOpAgentHandle`.
- Sync agents run a child query and return real output.
- Background agents start work and store final output.
- Teammates actually start their run loop.
- Child tool registry is filtered before prompt generation.
- Child permission prompts identify the child.
- Unsupported modes return model-visible unsupported results.

### Skill Checklist

- Skill resolution uses `SkillManager`.
- Aliases resolve.
- Disabled and model-hidden skills are rejected or hidden correctly.
- Inline skills produce `new_messages`.
- Fork skills call `AgentRuntime`.
- Remote unsupported behavior is explicit.
- Dynamic SkillTool prompt is deterministic and test-covered.

### Model Checklist

- Fallback model behavior is owned by `ModelRuntime`.
- Active model name is observable by tool context.
- Streaming retry does not leave orphan tool work.
- Prompt-too-long and max-output recovery are tested.
- Token usage and stop reason survive the runtime boundary.

## Migration Risks

### Risk: Changing Permission Semantics

Moving permission logic can accidentally change when prompts happen.

Mitigation:

- snapshot current behaviors in tests first
- add explicit tests for Ask, Deny, bridge rejection, and auto-mode classifier
- keep permission decisions typed and auditable

### Risk: Hook Ordering Drift

Hooks are subtle because they can rewrite inputs, block calls, modify outputs,
and stop continuation.

Mitigation:

- implement HookAdapter separately
- test all hook output fields
- compare behavior against TS flow

### Risk: Agent Runtime Scope Explosion

AgentTool includes sync agents, background agents, teammates, fork mode,
worktrees, progress, and output persistence.

Mitigation:

- implement sync local subagent first
- then background
- then teammate
- then fork/worktree

### Risk: Skill Runtime Hidden Dependencies

Skill expansion depends on skill source, arguments, plugin paths, user config,
and hidden/model-invocation flags.

Mitigation:

- reuse existing `coco-skills` and `skill_advanced` helpers
- add tests for each source and visibility flag
- keep remote skills as a later extension if not already supported

### Risk: Too Much In One PR

This refactor touches the core loop. Large unreviewable changes will be risky.

Mitigation:

- keep PRs phase-based
- preserve behavior with tests before deleting old paths
- remove old paths only after the new runner is covered

## Definition Of Done

The refactor is complete when:

- `QueryEngine` no longer manually implements tool lifecycle details.
- Every assistant tool call always produces exactly one matching tool result.
- Streaming and non-streaming execution use the same `ToolCallRunner`.
- The eager bypass at `engine.rs:1539` is deleted (grep confirms).
- Input validation always happens before permissions, using the shared
  `core/tool/src/execution.rs::validate_input_then_strip` helper (validation
  runs before defense-in-depth stripping per I3).
- PreToolUse input rewrites affect permission, execution, and post hooks.
- PostToolUse and PostToolUseFailure receive structured effective inputs.
- `ToolResult::new_messages` are appended and normalized correctly.
- `ToolUseContext` is accurate and test-covered (no hardcoded fields).
- `AgentTool` runs real sync/background/teammate agents through the
  existing `coco_tool::AgentQueryEngine` trait.
- `SkillTool` supports inline and forked skills via the new `SkillHandle`
  trait. `AgentHandle::resolve_skill` is deleted.
- Dynamic Agent/Skill prompts are generated from live state.
- `fallback_model`, `append_system_prompt`, `custom_system_prompt`,
  `is_non_interactive`, `max_budget_usd`, and `auto_compact_enabled` are
  honored.
- `CacheBreakDetector` resets on `fallback_model` switch (I13).
- Result message ordering follows I12: concurrent-safe batches append
  in completion order, serial unsafe tools in execution order,
  EarlyOutcome barriers in partition order; `app_state_patch`
  application is the only axis in model order (post-batch, under one
  write lock).
- Hook execution has exactly one owner (the runner). `executor.rs` does
  not call `ctx.hook_handle`.
- Cancellation hierarchy is documented and tested per I10.
- All **new** trait signatures introduced by this refactor (`SkillHandle`,
  internal app/query error types) use snafu enums, not `Result<_, String>`.
  Migration of existing `AgentHandle`/`AgentQueryEngine` is out of scope.
- All tool identity fields use `ToolId`, not raw `String`.
- `tracing` spans cover each lifecycle stage.
- Tests cover all invariants above.

### Out Of Scope For This Refactor

The following are intentionally not changed:

- `single_turn.rs` — unrelated single-turn helper for compaction/classifier.
- `emit.rs` — existing `CoreEvent` emission helpers are reused as-is.
- `executor.rs:701-749` `app_state_patch` application — already correct.
- `swarm_runner_loop::AgentExecutionEngine` — different purpose from
  `coco_tool::AgentQueryEngine`; both stay.
- `plan_mode_reminder.rs` — coordinates with prompt build, not the runner.
- Existing `coco_tool::AgentQueryEngine` trait — reused, not redefined.

## Short Version

This is a TS-aligned Rust refactor, not a TS line-by-line port.

The single most important architectural change is:

```text
Move all tool-call semantics out of QueryEngine and into one ToolCallRunner.
```

After that, AgentRuntime and SkillRuntime can be wired cleanly because they will
return normal tool results and `new_messages` through the same pipeline as every
other tool.
