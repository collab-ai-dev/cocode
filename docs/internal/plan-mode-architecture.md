# Plan Mode Architecture

This document is the plan-mode lifecycle owner for `coco-rs`. The TS source is
the behavioral spec. TS file paths are relative to the TS project's `src/`
directory. Rust mirrors the lifecycle with shared state, typed tool results,
and explicit app-state patches.

## TS Source Map

| Concern | TS source | Rust owner |
|---------|-----------|------------|
| Enter tool | `tools/EnterPlanModeTool/EnterPlanModeTool.ts` | `core/tools/src/tools/plan_mode.rs` |
| Exit tool | `tools/ExitPlanModeTool/ExitPlanModeV2Tool.ts` | `core/tools/src/tools/plan_mode.rs` |
| Slash `/plan` | `commands/plan/plan.tsx` | `app/cli/src/tui_runner.rs` |
| Mode transition side effects | `utils/permissions/permissionSetup.ts` | `core/permissions/src/mode_transition.rs` |
| Plan file utilities | `utils/plans.ts` | `core/context/src/plan_mode.rs` |
| ExitPlanMode observable input normalization | `utils/api.ts:normalizeToolInput` | `app/query/src/tool_input_normalizer.rs` |
| Full/sparse/reentry/exit reminders | `utils/attachments.ts`, `utils/messages.ts` | `core/system-reminder`, `app/query/src/engine_turn_reminders.rs`, `core/context/src/plan_mode.rs` |
| Plan-role model swap | `utils/model/model.ts:getRuntimeMainLoopModel` | `app/query/src/engine.rs` |

`/ultraplan` is intentionally not mirrored. Root guidance says coco-rs skips
TS Ultraplan because it depends on CCR backend behavior not shipped here.

## State Model

The durable session state is `coco_types::ToolAppState`. Plan mode uses these
fields:

| Field | Meaning |
|-------|---------|
| `permission_mode` | Live mode source of truth for TUI, SDK, bridge, reminders, and tool contexts. |
| `pre_plan_mode` | Mode to restore when `ExitPlanMode` succeeds. |
| `plan_mode_entry_ms` | Unix-ms baseline for optional `ExitPlanMode` stale-plan advisory. |
| `has_exited_plan_mode` | One-shot reentry latch. Preserved across Plan reentry until the reentry reminder emits. |
| `needs_plan_mode_exit_attachment` | One-shot exit banner latch. Cleared after emit or when Plan is re-entered. |
| `pending_clear_message_history` | Set when the user approves `ExitPlanMode` with clear-context. |
| `pending_plan_verification` | Legacy/default-off path set by `ExitPlanMode` only when `settings.plan_mode.verify_execution` is explicitly enabled; drives follow-up verify reminders until `VerifyPlanExecution` clears it. |

All external mode switchers must call
`coco_permissions::apply_permission_mode_transition_to_app_state`. This mirrors
TS `transitionPermissionMode()` for app-state-shaped side effects:

1. Non-Plan to Plan stashes `pre_plan_mode`, clears stale exit-banner state,
   and stamps `plan_mode_entry_ms`.
2. Plan to non-Plan clears `pre_plan_mode`, sets `has_exited_plan_mode`, and
   schedules `needs_plan_mode_exit_attachment`.
3. If the plan had Auto classifier state active and the target is not Auto,
   the same transition clears `stripped_dangerous_rules` and schedules the
   auto-mode exit banner.
4. Auto to non-Auto clears `stripped_dangerous_rules` through the existing
   auto-boundary helper.

`ExitPlanModeTool` keeps a specialized patch because it must restore
`pre_plan_mode`, handle optional clear-context, keep or clear stripped rules
when restoring Auto, and set plan-verification state in one atomic update.

## Enter Paths

Plan mode can be entered through the model tool, `/plan`, TUI/SDK mode controls,
or bridge controls. All paths converge on the same app-state transition helper.

Model-driven entry:

1. `EnterPlanModeTool::execute` rejects agent contexts, matching TS.
2. It reads live `app_state.permission_mode` with `ctx.permission_context.mode`
   as fallback.
3. It returns a `ToolResult` with an app-state patch from
   `build_enter_plan_mode_patch`.
4. The streaming executor applies the patch after tool execution.
5. The next engine iteration reads live app state and emits plan reminders.

External entry:

1. `/plan`, TUI Shift+Tab, SDK `control/setPermissionMode`, and bridge
   `SetPermissionMode` update the session-visible mode.
2. The same helper stashes the prior mode and stamps entry time.
3. The next `QueryEngine` turn sees Plan through `ToolAppState`, not a frozen
   config snapshot.

Idempotent Plan to Plan is a no-op for plan latches. It does not overwrite
`pre_plan_mode` with Plan and does not refresh `plan_mode_entry_ms`.

## Plan File

Plan files mirror TS `utils/plans.ts`:

1. Default directory: `<config_home>/plans`.
2. Custom directory: `settings.plans_directory`, resolved like TS
   `resolve(cwd, setting)` and accepted only when the canonical or lexical
   normalized path remains inside the project root.
3. Main plan path: `{slug}.md`.
4. Sub-agent plan path: `{slug}-agent-{agent_id}.md`.
5. Slug is generated lazily per session and stored in the plan slug cache.
6. Forked sessions copy the source plan to a new slug so the fork cannot clobber
   the original session plan.

During plan mode, permission evaluation auto-allows writes to the resolved
session plan file and sub-agent plan variant. Other edits remain blocked or ask
according to the current mode.

## Observable Exit Input

TS injects `plan` and `planFilePath` into `ExitPlanMode` tool input before
hooks and SDK consumers observe the tool call. Rust mirrors that at the query
boundary through `tool_input_normalizer`:

1. Non-streaming assistant snapshots are normalized before they are written to
   message history and before the tool-call list reaches the runner.
2. Streaming tool calls are normalized as soon as their input JSON is complete,
   before pre-tool hooks, permission checks, and execution.
3. `ToolUseQueued` emits the normalized input, so protocol/SDK observers see
   the plan content.
4. If no plan exists on disk, input is left unchanged.
5. `ExitPlanModeTool` treats a byte-identical injected disk snapshot as
   observable context, not as a user-edited plan.
6. Normalization happens exactly once per tool call — at the engine boundary
   that builds the assistant-message `ToolCallPart`. The downstream runner
   (`tool_runner.rs`) consumes that already-normalized input; it does not
   re-read the plan file.

Mirroring TS's `normalizeToolInput` / `normalizeToolInputForAPI` pair, the
injected fields are **stripped back out before the assistant message is
re-sent to the model**: `coco_messages::normalize::normalize_messages_for_api`
removes `plan` / `planFilePath` from `ExitPlanMode` tool calls (the wire schema
is an empty object). The injected fields therefore live only in the persisted
transcript and in hook / SDK observation — never on the API wire. The shared
field-name constants (`EXIT_PLAN_MODE_INJECTED_PLAN_FIELD`,
`EXIT_PLAN_MODE_INJECTED_PLAN_FILE_PATH_FIELD`) are owned by
`coco-messages` so the inject and strip sites cannot drift.

One intentional asymmetry: TS `normalizeToolInput` also calls
`persistFileSnapshotIfRemote()` so CCR remote sessions survive pod recycling.
coco-rs does not write file snapshots, but resume recovery still *reads* them
(see [Resume Recovery](#resume-recovery)) so a TS-authored transcript can be
recovered by coco-rs.

## Exit Paths

Normal `ExitPlanMode` flow:

1. Validate mode is Plan, unless the caller is a teammate path that has its own
   plan-approval semantics.
2. Read the plan from disk. If permission approval supplied an edited `plan`,
   that input wins and is written back to disk.
3. For required teammate approval, write a `plan_approval_request` mailbox
   message and return `awaitingLeaderApproval`.
4. For normal local exit, restore `pre_plan_mode` or Default.
5. Set `has_exited_plan_mode`, `needs_plan_mode_exit_attachment`,
   `pending_plan_verification`, and optional clear-context state.
6. Restore or keep stripped dangerous rules according to the target mode.

External Plan to non-Plan transitions do not run the approval flow. They still
set the same exit and reentry latches so reminders stay consistent with TS
mode switching.

## Reentry And Changed Plan Detection

TS does not do semantic plan comparison in code. It sets a one-shot
`hasExitedPlanMode` flag and, when the user re-enters Plan while a plan file
exists, injects a reentry reminder. The reminder instructs the model to:

1. Read the existing plan file.
2. Compare it with the current user request.
3. Overwrite the plan for a different task.
4. Modify and clean up the plan for the same continuing task.
5. Edit the plan before calling `ExitPlanMode`.

Rust mirrors that exactly:

1. Leaving Plan sets `has_exited_plan_mode = true`.
2. Re-entering Plan keeps that flag and only clears stale exit-banner state.
3. `PlanModeReentryGenerator` fires only when mode is Plan, the reentry latch is
   set, a plan file exists, and the caller is not a sub-agent.
4. Post-emit bookkeeping clears `has_exited_plan_mode` after the reentry
   reminder actually fires.

The "changed to another plan" check is therefore prompt-side and model-visible,
not a Rust semantic diff.

## Reminder Chain

The Rust reminder chain mirrors TS attachments but uses typed generators:

1. `engine_turn_reminders` snapshots `ToolAppState`, plan path, plan existence,
   current tools, tasks, todos, and compaction data.
2. `core/system-reminder` generators decide whether to emit:
   `plan_mode`, `plan_mode_reentry`, `plan_mode_exit`, `auto_mode_exit`, and
   related reminders.
3. `core/context/src/plan_mode.rs` renders the exact model-facing prose.
4. Post-emit bookkeeping clears one-shot flags only for reminders that fired.

Full/sparse cadence is persisted on `ToolAppState`, so it survives multiple
`QueryEngine` instances. Tool-result rounds inside the same human turn do not
advance the plan reminder cadence.

## Plan-Role Model Swap

Plan mode can use a configured `ModelRole::Plan` client. The engine must read
live app state before each LLM call:

1. If live mode is Plan and the latest assistant context is below the configured
   fallback token threshold, use the Plan-role client.
2. Otherwise use the active Main/fallback model runtime client.

This matters for model-driven `EnterPlanMode` within a single engine run:
the next LLM iteration must switch to the Plan-role client even though
`QueryEngineConfig.permission_mode` was Default at construction.

## Resume Recovery

Plan resume follows TS `copyPlanForResume()` priority:

1. Reuse the slug from transcript metadata.
2. If the plan file already exists, do nothing.
3. Recover from the most recent `file_snapshot` system message with key `plan`.
4. Fall back to message history:
   - TS assistant shape: `{ type: "assistant", message: { content: [...] } }`
   - Rust assistant shape: `{ role: "assistant", content: [...] }`
   - user `planContent`
   - `plan_file_reference` attachment
5. Write recovered content to `{slug}.md`.

File snapshots have global priority over tool inputs, matching TS. This matters
for remote sessions where plan files can be lost while transcripts survive.

coco-rs reads `file_snapshot` entries but never writes them — TS's
`persistFileSnapshotIfRemote()` is CCR-remote-specific and not mirrored. The
read path is kept so a TS-authored transcript resumed under coco-rs still
recovers its plan; a coco-rs-authored transcript recovers from the
`ExitPlanMode` tool input or the `plan_file_reference` attachment instead.

## Verification

The optional verify path is Rust-owned but follows TS intent:

1. Plan entry records `plan_mode_entry_ms`.
2. `ExitPlanMode` can compare plan file mtime against that entry timestamp when
   the legacy `settings.plan_mode.verify_execution` setting is explicitly
   enabled.
3. Outcomes are soft signals: edited, not edited, missing, or skipped when no
   timestamp exists.

The check does not block plan approval. It only gives the model and future
reminder chain a durable signal. This legacy path is deprecated and inactive by
default.

`VerifyPlanExecutionTool` is the lightweight mirror of TS's conditional
`VerifyPlanExecution` tool reference. It is not default-registered, and the
flow only runs when legacy verification is explicitly enabled and the tool is
explicitly registered. **It performs no verification itself** —
TS's (unavailable) tool spins up a background verification agent
(`state/AppStateStore.ts` carries `verificationStarted` /
`verificationCompleted` sub-flags for that flow); coco-rs deliberately ships
the simpler shape. The model is expected to inspect the plan, implementation,
and verification commands first; calling the tool only records the checkpoint
and clears `pending_plan_verification` so the `verify_plan_reminder` stops
firing.

## Test Coverage

The lifecycle is covered at the owner closest to each behavior:

| Behavior | Test owner |
|----------|------------|
| Shared mode transition latches | `core/permissions/src/mode_transition.test.rs` |
| Enter/exit tool state and verification | `core/tools/src/tools/plan_mode.test.rs` |
| VerifyPlanExecution checkpoint | `core/tools/src/tools/verify_plan_execution.test.rs` |
| Resume recovery priority and TS transcript shapes | `core/context/src/plan_mode.test.rs` |
| ExitPlanMode observable plan injection | `app/query/src/tool_input_normalizer.test.rs`, `app/query/src/engine.test.rs` |
| SDK and bridge external mode switching | `app/cli/src/sdk_server/*test.rs` |
| Full engine reminder cadence, reentry, and live model swap | `app/query/tests/integration_plan_lifecycle.rs` |

Before committing plan-mode changes, run focused crate tests first, then the
workspace final gate once:

```bash
cd coco-rs
just fmt
just test-crate coco-permissions
just test-crate coco-context
just test-crate coco-tools
just test-crate coco-cli
just test-crate coco-query
```
