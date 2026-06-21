# coco-system-reminder

Per-turn `<system-reminder>` injection. **TS-first**: ports Claude Code's `src/utils/attachments.ts` + `src/utils/messages.ts`. If the Rust port and TS disagree on cadence, text, or trigger, follow TS.

Scope note: this crate owns the 42 `coco-system-reminder` generators below (40 model-visible, 2 silent/display-only). Claude Code's TS `Attachment` union is broader: file/context attachments, UI-only attachments, hook bookkeeping, slash-command metadata, and direct tool-result `<system-reminder>` strings live outside this crate. Those are covered in the scan sections after the catalog.

**TS source files**:
- `utils/attachments.ts` — `getAttachments()` orchestrator + per-reminder trigger functions
- `utils/messages.ts` — `normalizeAttachmentForAPI()` text templates (one `case` per reminder)
- `query.ts` — mid-turn consumption of memory / skill prefetch and `max_turns_reached`
- `utils/task/framework.ts` — task status attachment source; `services/compact/compact.ts` re-injects task status after compaction
- `utils/hooks.ts`, `services/tools/toolHooks.ts`, `query/stopHooks.ts` — hook attachment emitters
- `services/compact/compact.ts` — compaction-emitted attachments
- `buddy/prompt.ts` — companion-related attachments
- `utils/processUserInput/processSlashCommand.tsx`, `screens/REPL.tsx`, `components/permissions/rules/PermissionRuleList.tsx` — slash-command `metaMessages` sinks / producers
- `tools/FileReadTool/FileReadTool.ts`, `tools/TodoWriteTool/TodoWriteTool.ts`, `tools/TaskUpdateTool/TaskUpdateTool.ts`, `tools/WebSearchTool/WebSearchTool.ts` — tool-result reminders outside `Attachment`
- `Tool.ts`, `utils/forkedAgent.ts`, `tools/AgentTool/loadAgentsDir.ts`, `tools/AgentTool/runAgent.ts`, `entrypoints/sdk/coreSchemas.ts` — `criticalSystemReminder_EXPERIMENTAL` config/schema/pass-through references
- `query/tokenBudget.ts`, `query.ts` — token-budget continuation nudge outside `Attachment`
- `memdir/memoryAge.ts`, `utils/api.ts`, `utils/sideQuestion.ts`, `cli/print.ts`, `services/tokenEstimation.ts` — direct `<system-reminder>` text producers and attachment rendering consumers

Line numbers below are valid for the TS snapshot at time of writing — grep by function/case name if the file has drifted.

## Rust architecture

The crate is intentionally split into five stages. Keep new reminder work inside
the stage that owns that responsibility:

1. **Source materialization** (`sources/`) fans out to hooks, LSP, tasks,
   skills, MCP, swarm, IDE, and memory sources under per-source timeouts.
   Missing or timed-out sources degrade to empty snapshots.
2. **Turn input assembly** (`turn_runner.rs`, `context_builder.rs`,
   `turn_counting.rs`) converts engine state and history into scalar fields on
   `GeneratorContext`. Generators do not scan message history or call sibling
   subsystems directly.
3. **Pure generation** (`generators/`) owns one `AttachmentGenerator` per
   reminder key. A generator only gates on `GeneratorContext`, renders TS-parity
   text, and returns `Option<SystemReminder>`.
4. **Orchestration** (`orchestrator.rs`, `throttle.rs`) applies config, tier,
   throttle, full/sparse cadence, and timeout policy. Generators run in
   parallel, while injection order follows the TS batch order: user-input,
   all-thread, then main-thread.
5. **Injection** (`inject.rs`, `xml.rs`) converts `SystemReminder` values into
   `coco_types::Message` entries and routes silent reminders to the display-only
   sink so they never reach the model.

## coco-system-reminder catalog (42 generators)

Columns:
- **ID** — `coco-system-reminder` attachment/settings key. Most IDs match TS `Attachment.type`; a few are coco-rs synthetic grouping keys where TS emits more concrete attachment types (`file`, `mcp_resource`, `agent_mention`, `selected_lines_in_ide`, `opened_file_in_ide`, or `queued_command`). The TS columns name the exact upstream mapping.
- **Tier** — `Core` (all agents) / `Main` (main agent only) / `User` (only when user submitted input this turn)
- **What it does** — one-line purpose
- **Trigger** — gate chain
- **Settings** — `settings.json` → `system_reminder.attachments.<key>`; **bold** = default value
- **TS `attachments.ts`** — trigger function + line
- **TS `messages.ts`** — text-template `case` + line

### Plan / Auto mode (5)

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `plan_mode` | Core | Injects the multi-phase plan-mode workflow instructions while the agent is in plan mode | `is_plan_mode == true`; 5-human-turn throttle, Full content every 5th emission, Sparse otherwise | `plan_mode` (**true**) | `getPlanModeAttachments` (`:1186`) | `case 'plan_mode':` (`:3826`) |
| `plan_mode_exit` | Core | One-shot "you have exited plan mode" banner after the agent leaves plan mode | Set by `ExitPlanMode` tool success or by the engine on an unannounced Plan→non-Plan transition; cleared post-emit | `plan_mode_exit` (**true**) | `getPlanModeExitAttachment` (`:1248`) | `case 'plan_mode_exit':` (`:3848`) |
| `plan_mode_reentry` | Core | One-shot "re-entering plan mode" banner when returning to plan with a prior plan file present | First plan turn after prior exit in this session, plan file exists, not a sub-agent | `plan_mode_reentry` (**true**) | `getPlanModeAttachments` (`:1186`, `plan_mode_reentry` branch) | `case 'plan_mode_reentry':` (`:3829`) |
| `auto_mode` | Core | Injects autonomous-execution guidelines while auto mode is active | `is_auto_mode == true` (Auto permission mode OR Plan+classifier active); 5-human-turn throttle, Full every 5th | `auto_mode` (**true**) | `getAutoModeAttachments` (`:1335`) | `case 'auto_mode':` (`:3860`) |
| `auto_mode_exit` | Core | One-shot "you have exited auto mode" banner | Exit flag set on Auto→non-Auto transition; suppressed if still in auto mode | `auto_mode_exit` (**true**) | `getAutoModeExitAttachment` (`:1380`) | `case 'auto_mode_exit':` (`:3863`) |

### Todo / Task / Verify (3)

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `todo_reminder` | Core | Nudges the agent to use `TodoWrite` when it has been silent on task tracking | `TodoWrite` tool present AND `Brief` tool absent AND `turns_since_last_todo_write ≥ 10` AND `turns_since_last_todo_reminder ≥ 10`; V2-enabled sessions route to `task_reminder` instead | `todo_reminder` (**true**) | `getTodoReminderAttachments` (`:3266`) | `case 'todo_reminder':` (`:3663`) |
| `task_reminder` | Core | V2 equivalent of `todo_reminder` — nudges toward `TaskCreate`/`TaskUpdate` | `is_task_v2_enabled` AND `USER_TYPE != ant` AND `TaskUpdate` tool present AND `Brief` tool absent AND 10-turn silence gates | `task_reminder` (**true**) | `getTaskReminderAttachments` (`:3375`) | `case 'task_reminder':` (`:3680`) |
| `verify_plan_reminder` | Main | Prompts the agent to call `VerifyPlanExecution` after an `ExitPlanMode` | Pending-verification state exists, has not started/completed, `VerifyPlanExecution` is visible, AND every 10 human turns after plan exit; TS also gates on `CLAUDE_CODE_VERIFY_PLAN` env | `verify_plan_reminder` (**true**, no-op unless the tool is visible) | `getVerifyPlanReminderAttachment` | `case 'verify_plan_reminder':` (`:4240`) |

### Critical / Compaction / Date (3)

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `critical_system_reminder` | Core | Injects a user-supplied critical instruction on every turn | `config.critical_instruction.is_some()` | `critical_system_reminder` (**true**) | `getCriticalSystemReminderAttachment` (`:1587`) | `case 'critical_system_reminder':` (`:3872`) |
| `compaction_reminder` | Core | Reassures the agent that auto-compaction will preserve context on large windows | Auto-compact enabled AND context window ≥ 1 M AND used tokens ≥ 25% of effective window; TS gates on `feature('COMPACTION_REMINDERS')` | `compaction_reminder` (**true**) | `getCompactionReminderAttachment` | `case 'compaction_reminder':` (`:4139`) |
| `date_change` | Core | Notifies the agent when the local date rolls over (e.g. coding past midnight) | Local ISO date differs from the per-session latched date; first observation seeds without emit | `date_change` (**true**) | `getDateChangeAttachments` (`:1415`) | `case 'date_change':` (`:4162`) |

### Engine-local reminders (5)

State comes from the engine / config / user input — no cross-crate dependency.

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `ultrathink_effort` | Core | Asks the agent to apply high reasoning effort when the user typed the `ultrathink` keyword | User prompt contains `ultrathink` (word-boundary, case-insensitive); TS gates on `feature('ULTRATHINK')` + GrowthBook | `ultrathink_effort` (**false**) | `getUltrathinkEffortAttachment` (`:1446`) | `case 'ultrathink_effort':` (`:4170`) |
| `token_usage` | Main | Reports `used/total; remaining` tokens every turn | Effective context window > 0; TS gates on `CLAUDE_CODE_ENABLE_TOKEN_USAGE_ATTACHMENT` env | `token_usage` (**false**) | `getTokenUsageAttachment` (`:3807`) | `case 'token_usage':` (`:4058`) |
| `budget_usd` | Main | Reports `$used/$total; $remaining` when a USD budget is configured | `max_budget_usd.is_some()` | `budget_usd` (**true**) | `getMaxBudgetUsdAttachment` (`:3846`) | `case 'budget_usd':` (`:4067`) |
| `output_token_usage` | Main | Reports per-turn and session output-token counts against a turn budget | Turn-output-token budget set > 0; TS gates on `feature('TOKEN_BUDGET')` | `output_token_usage` (**false**) | `getOutputTokenUsageAttachment` (`:3828`) | `case 'output_token_usage':` (`:4076`) |
| `companion_intro` | Core | Introduces the configured companion character once per session | Companion name + species configured AND not previously announced; TS gates on `feature('BUDDY')` | `companion_intro` (**false**) | `buddy/prompt.ts:getCompanionIntroAttachment` | `case 'companion_intro':` (`:4232`) |

### History-diff deltas (3)

Engine persists the previously-announced set on shared state and diffs each turn.

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `deferred_tools_delta` | Core | Announces tool availability changes so the agent knows what's new or gone | Current tool set differs from last-announced tool set | `deferred_tools_delta` (**true**) | `getDeferredToolsDeltaAttachment` (`:1455`) | `case 'deferred_tools_delta':` (`:4178`) |
| `agent_listing_delta` | Core | Lists agent types available for the `Agent` tool; flips header + adds concurrency note on first emission | Current agent-type set differs from last-announced | `agent_listing_delta` (**true**) | `getAgentListingDeltaAttachment` (`:1490`) | `case 'agent_listing_delta':` (`:4194`) |
| `mcp_instructions_delta` | Core | Surfaces added / removed MCP-server instructions mid-session | Per-server instructions differ from last-announced map | `mcp_instructions_delta` (**true**) | `getMcpInstructionsDeltaAttachment` (`:1559`) | `case 'mcp_instructions_delta':` (`:4216`) |

### Hook reminders (5)

All share one drain of pending hook events per turn.

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `hook_success` | Main | Surfaces successful hook stdout from `SessionStart` / `UserPromptSubmit` hooks | Success event of matching hookEvent AND non-empty content | `hook_success` (**true**) | emitted by sync hook executor (not in `getAttachments`) | `case 'hook_success':` (`:4099`) |
| `hook_blocking_error` | Main | Reports why a hook blocked the turn (command + error text) | Blocking-error event from any hook | `hook_blocking_error` (**true**) | emitted by sync hook executor | `case 'hook_blocking_error':` (`:4090`) |
| `hook_additional_context` | Main | Injects extra context lines a hook returned | Event with non-empty additional-context content | `hook_additional_context` (**true**) | emitted by sync hook executor | `case 'hook_additional_context':` (`:4117`) |
| `hook_stopped_continuation` | Main | Reports when a hook halted a continuation | Stopped-continuation event | `hook_stopped_continuation` (**true**) | emitted by sync hook executor | `case 'hook_stopped_continuation':` (`:4130`) |
| `async_hook_response` | Main | Multi-message surface for a completed async hook (systemMessage + additionalContext) | Completed async-hook response, drained on read (marks delivered) | `async_hook_response` (**true**) | `getAsyncHookResponseAttachments` (`:3464`) | `case 'async_hook_response':` (`:4026`) |

### Diagnostics / tasks / skills / misc (6)

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `diagnostics` | Main | Injects new LSP/IDE diagnostics wrapped in `<new-diagnostics>…</new-diagnostics>` | New-since-last-snapshot diagnostic entries available | `diagnostics` (**true**) | `getDiagnosticAttachments` (`:2854`) + `getLSPDiagnosticAttachments` (`:2883`) | `case 'diagnostics':` (`:3812`) |
| `output_style` | Main | Reminds the agent to follow the active output-style guidelines | Active output-style name set | `output_style` (**true**) | `getOutputStyleAttachment` (`:1597`) | `case 'output_style':` (`:3797`) |
| `queued_command` | Core | Replays mid-turn queued items, each wrapped via `wrapCommandText` per typed [`QueueOrigin`] (coordinator / task-notification / channel / human) | Queue has at least one non-empty entry | `queued_command` (**true**) | `getQueuedCommandAttachments` (`:1046`) | `case 'queued_command':` (`:3739`) + `wrapCommandText` (`:5496`) |
| `task_status` | Main | Warns against duplicate background-task spawns; reports running/completed/killed tasks | Inline main-thread task snapshot from `getUnifiedTaskAttachments()` when `generateTaskAttachments()` returns task deltas; post-compaction async-agent snapshot is also re-injected | `task_status` (**true**) | `getUnifiedTaskAttachments` (`:3439`) / `compact.ts:createAsyncAgentAttachmentsIfNeeded` (`:1569`) | `case 'task_status':` (`:3954`) |
| `skill_listing` | Core | Lists available skills for the `Skill` tool | Active skill set non-empty (1% context-window budget in TS) | `skill_listing` (**true**) | `getSkillListingAttachments` (`:2661`) | `case 'skill_listing':` (`:3728`) |
| `invoked_skills` | Main | Re-surfaces the content of skills invoked in this session so guidelines persist after compaction | Session has invoked skills with cached content | `invoked_skills` (**true**) | `compact.ts:getInvokedSkillsForAgent` (`:1497`) | `case 'invoked_skills':` (`:3644`) |

### Swarm (3) — `agentSwarms` feature-gated upstream

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `teammate_mailbox` | Core | Delivers unread messages from other teammates (pre-formatted bundle) | Unread messages in this teammate's mailbox; skipped for `session_memory` fork (avoids silently stealing leader DMs) | `teammate_mailbox` (**true**) | `getTeammateMailboxAttachments` (`:3532`) | TS handles before switch (`formatTeammateMessages()`) — no `normalizeAttachmentForAPI` case |
| `team_context` | Core | One-shot first-turn team identity + member list for teammates | First turn as a teammate; not team lead; team registered | `team_context` (**true**) | `getTeamContextAttachment` (`:3775`) | TS handles before switch — no `normalizeAttachmentForAPI` case |
| `agent_pending_messages` | Core | Emits one `<system-reminder>` per pending teammate message, each wrapped with the coordinator-origin framing from `wrapCommandText`. Wire-level `AttachmentKind` maps to `QueuedCommand` for transcript parity. | Pending-messages inbox non-empty | `agent_pending_messages` (**true**) | `getAgentPendingMessageAttachments` (`:1085`) | TS emits N `queued_command` attachments with coordinator origin (`case 'queued_command':` `:3739`) |

### User-input tier (3)

All gated on the user submitting input this turn. UUID-dedup ensures one fire per human turn across multi-iteration tool loops.

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `at_mentioned_files` | User | Announces `@path` files the user mentioned in their prompt | User prompt contains parseable `@file` tokens | `at_mentioned_files` (**true**) | `processAtMentionedFiles` | TS emits concrete `file` / `directory` / `pdf_reference` / `already_read_file` attachments; coco-rs consolidates path notice. Render cases: `file` (`:3545`), `directory` (`:3525`), `pdf_reference` (`:3600`), `already_read_file` (`:4252`, returns `[]`) |
| `mcp_resources` | User | Announces `@server:uri` MCP resource references the user mentioned | User prompt contains `@server:uri` tokens matching a registered server | `mcp_resources` (**true**) | `processMcpResourceAttachments` | TS concrete type `mcp_resource` (`case 'mcp_resource':` `:3877`) |
| `agent_mentions` | User | Hints the agent to invoke an `@agent-type` the user referenced | User prompt contains `@agent-type` mentions | `agent_mentions` (**true**) | `processAgentMentions` | TS concrete type `agent_mention` (`case 'agent_mention':` `:3946`) |

### Main-thread IDE (2)

TS places these in `mainThreadAttachments`, not `userInputAttachments`; they are main-agent-only and can fire even when no new user prompt arrived.

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `ide_selection` | Main | Surfaces the user's IDE text selection with a 2000-char truncation cap | IDE bridge reports a non-empty selection for a known filename | `ide_selection` (**true**) | `getSelectedLinesFromIDE` | TS concrete type `selected_lines_in_ide` (`case 'selected_lines_in_ide':` `:3613`) |
| `ide_opened_file` | Main | Notes which file the user just opened in the IDE | IDE bridge reports a non-empty opened filename | `ide_opened_file` (**true**) | `getOpenedFileFromIDE` | TS concrete type `opened_file_in_ide` (`case 'opened_file_in_ide':` `:3628`) |

### Memory (2)

| ID | Tier | What it does | Trigger | Settings | TS `attachments.ts` | TS `messages.ts` |
|---|---|---|---|---|---|---|
| `nested_memory` | Core | Injects nested `CLAUDE.md` / memory-file contents found via `@`-mention traversal | User `@`-mentioned paths triggered nested-memory traversal with hits | `nested_memory` (**true**) | `getNestedMemoryAttachments` (`:2167`) | `case 'nested_memory':` (`:3700`) |
| `relevant_memories` | Core | Surfaces semantically-ranked memory files for the user's prompt (async prefetched) | Prefetch returned ranked memory entries; each uses its stored header for prompt-cache stability | `relevant_memories` (**true**) | `getRelevantMemoryAttachments` (`:2196`) + `query.ts:startRelevantMemoryPrefetch` | `case 'relevant_memories':` (`:3708`) |

### Emitted elsewhere in TS — **not** this crate

TS attachment types that cross the reminder boundary but are emitted by different subsystems:

| ID | Tier | What it does | Trigger (TS) | TS location |
|---|---|---|---|---|
| `plan_file_reference` | Core | Re-injects the plan-file contents post-compaction so the plan survives the context bust | Compaction runs AND plan file exists | `services/compact/compact.ts:createPlanAttachmentIfNeeded` (`:1470`) · `messages.ts` `case 'plan_file_reference':` (`:3636`) |
| `compact_file_reference` | Core | Re-injects recently-read file references after compaction without inlining large content | Compaction runs AND read-file state contains paths not preserved in the compacted tail | `services/compact/compact.ts:createPostCompactFileAttachments` (`:1399`) · `messages.ts` `case 'compact_file_reference':` (`:3592`) |
| `edited_text_file` | Core | Warns that a previously-read file changed on disk and includes a diff snippet | A cached read-file path has a newer mtime and a text diff | `utils/attachments.ts:getChangedFiles` (`:2073`) · `messages.ts` `case 'edited_text_file':` (`:3538`); coco-rs currently owns this in `core/context::changed_files` + `app/cli::changed_file_to_message`, not this crate |
| `file` / `directory` / `pdf_reference` | User | Full user-input file context for `@path` mentions | `processAtMentionedFiles()` resolves a file, directory, or large PDF reference | `utils/attachments.ts:processAtMentionedFiles` (`:1900`) · `messages.ts` cases `file` (`:3545`), `directory` (`:3525`), `pdf_reference` (`:3600`) |

## Full TS Attachment coverage index

This table is the completeness guard for the TS `Attachment` union in
`utils/attachments.ts:440-731`. It includes every union member in the scanned TS
snapshot, even when the type is UI-only or not implemented by
`coco-system-reminder`.

| TS attachment type | TS definition | TS emit location(s) | API rendering | README / Rust status |
|---|---:|---|---|---|
| `file` | `utils/attachments.ts:296` | `processAtMentionedFiles` (`utils/attachments.ts:1900`) -> `generateFileAttachment` (`utils/attachments.ts:3159`, `utils/attachments.ts:3181`); legacy resume transform in `utils/conversationRecovery.ts:93` | `utils/messages.ts:3545` | Outside this crate; represented by `at_mentioned_files` grouping. |
| `compact_file_reference` | `utils/attachments.ts:308` | `services/compact/compact.ts:createPostCompactFileAttachments`; `generateFileAttachment` compact branch (`utils/attachments.ts:3136`) | `utils/messages.ts:3592` | Outside this crate; post-compact file reference. |
| `pdf_reference` | `utils/attachments.ts:315` | `tryGetPDFReference` (`utils/attachments.ts:3007`) | `utils/messages.ts:3600` | Outside this crate; represented by `at_mentioned_files` grouping. |
| `already_read_file` | `utils/attachments.ts:324` | `generateFileAttachment` (`utils/attachments.ts:3100`) | `utils/messages.ts:4252` -> `[]` | Ported as silent/display-only. |
| `agent_mention` | `utils/attachments.ts:336` | `processAgentMentions` (`utils/attachments.ts:1985`) | `utils/messages.ts:3946` | Ported as `agent_mentions`. |
| `async_hook_response` | `utils/attachments.ts:341` | `getAsyncHookResponseAttachments` (`utils/attachments.ts:3491`) | `utils/messages.ts:4026` | Ported. |
| `hook_blocking_error` | `utils/attachments.ts:355` | `utils/hooks.ts:714`; `services/tools/toolHooks.ts:108`, `services/tools/toolHooks.ts:260` | `utils/messages.ts:4090` | Ported. |
| `hook_stopped_continuation` | `utils/attachments.ts:364` | `query/stopHooks.ts:274`, `query/stopHooks.ts:389`, `query/stopHooks.ts:431`; `services/tools/toolExecution.ts:1575`; `services/tools/toolHooks.ts:121` | `utils/messages.ts:4130` | Ported. |
| `hook_additional_context` | `utils/attachments.ts:372` | `utils/processUserInput/processUserInput.ts:233`; `utils/sessionStart.ts:165`, `utils/sessionStart.ts:222`; `tools/AgentTool/runAgent.ts:548`; `services/tools/toolHooks.ts:136`, `services/tools/toolHooks.ts:273`, `services/tools/toolHooks.ts:571` | `utils/messages.ts:4117` | Ported. |
| `hook_permission_decision` | `utils/attachments.ts:382` | `services/tools/toolExecution.ts:987` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `hook_system_message` | `utils/attachments.ts:389` | `utils/hooks.ts:2773` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `hook_cancelled` | `utils/attachments.ts:397` | `services/tools/toolHooks.ts:81`, `services/tools/toolHooks.ts:236`, `services/tools/toolHooks.ts:594`; `utils/hooks.ts:2323`, `utils/hooks.ts:2486` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `hook_error_during_execution` | `utils/attachments.ts:406` | `services/tools/toolHooks.ts:179`, `services/tools/toolHooks.ts:307`, `services/tools/toolHooks.ts:634`; `utils/hooks.ts:2169`, `utils/hooks.ts:2208`, `utils/hooks.ts:4825` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `hook_success` | `utils/attachments.ts:416` | `utils/hooks.ts:721`, `utils/hooks.ts:2581`, `utils/hooks.ts:2630`; `utils/hooks/execPromptHook.ts:176`; `utils/hooks/execAgentHook.ts:297` | `utils/messages.ts:4099` (only `SessionStart` / `UserPromptSubmit`) | Ported with TS event filter. |
| `hook_non_blocking_error` | `utils/attachments.ts:429` | `utils/hooks.ts:2349`, `utils/hooks.ts:2380`, `utils/hooks.ts:2517`, `utils/hooks.ts:2684`, `utils/hooks.ts:2716`; `utils/hooks/execPromptHook.ts:122`, `utils/hooks/execPromptHook.ts:142`, `utils/hooks/execPromptHook.ts:201`; `utils/hooks/execAgentHook.ts:329` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `edited_text_file` | `utils/attachments.ts:452` | `getChangedFiles` (`utils/attachments.ts:2118`) | `utils/messages.ts:3538` | Outside this crate; coco-rs handles changed-file context elsewhere. |
| `edited_image_file` | `utils/attachments.ts:457` | `getChangedFiles` (`utils/attachments.ts:2129`) | `utils/messages.ts:4252` -> `[]` | Ported as silent/display-only. |
| `directory` | `utils/attachments.ts:462` | `processAtMentionedFiles` (`utils/attachments.ts:1934`); legacy resume transform in `utils/conversationRecovery.ts:104` | `utils/messages.ts:3525` | Outside this crate; represented by `at_mentioned_files` grouping. |
| `selected_lines_in_ide` | `utils/attachments.ts:469` | `getSelectedLinesFromIDE` (`utils/attachments.ts:1635`) | `utils/messages.ts:3613` | Ported as `ide_selection`. |
| `opened_file_in_ide` | `utils/attachments.ts:479` | `getOpenedFileFromIDE` (`utils/attachments.ts:1888`) | `utils/messages.ts:3628` | Ported as `ide_opened_file`. |
| `todo_reminder` | `utils/attachments.ts:483` | `getTodoReminderAttachments` (`utils/attachments.ts:3309`) | `utils/messages.ts:3663`; text at `utils/messages.ts:3668` | Ported. |
| `task_reminder` | `utils/attachments.ts:488` | `getTaskReminderAttachments` (`utils/attachments.ts:3424`) | `utils/messages.ts:3680`; text at `utils/messages.ts:3688` | Ported. |
| `nested_memory` | `utils/attachments.ts:493` | `memoryFilesToAttachments` (`utils/attachments.ts:1727`) | `utils/messages.ts:3700` | Ported. |
| `relevant_memories` | `utils/attachments.ts:500` | `startRelevantMemoryPrefetch` / `getRelevantMemoryAttachments` (`utils/attachments.ts:2241`); consumed in `query.ts:1609` | `utils/messages.ts:3708` | Ported. |
| `dynamic_skill` | `utils/attachments.ts:525` | `getDynamicSkillAttachments` (`utils/attachments.ts:2589`) | `utils/messages.ts:3723` -> `[]` | Silent / UI-only; skills load separately. |
| `skill_listing` | `utils/attachments.ts:532` | `getSkillListingAttachments` (`utils/attachments.ts:2745`) | `utils/messages.ts:3728` | Ported. |
| `skill_discovery` | `utils/attachments.ts:538` | `getTurnZeroSkillDiscovery` via `utils/attachments.ts:805`; inter-turn prefetch from `query.ts:331` | `utils/messages.ts:3503` feature-gated pre-switch block | TS feature-gated; not ported until matching skill search exists. |
| `queued_command` | `utils/attachments.ts:544` | `getQueuedCommandAttachments` (`utils/attachments.ts:1073`); `getAgentPendingMessageAttachments` (`utils/attachments.ts:1096`) | `utils/messages.ts:3739` | Ported; `agent_pending_messages` maps to this TS type. |
| `output_style` | `utils/attachments.ts:556` | `getOutputStyleAttachment` (`utils/attachments.ts:1608`) | `utils/messages.ts:3797` | Ported. |
| `diagnostics` | `utils/attachments.ts:560` | `getDiagnosticAttachments` (`utils/attachments.ts:2872`); `getLSPDiagnosticAttachments` (`utils/attachments.ts:2908`) | `utils/messages.ts:3812` | Ported. |
| `plan_mode` | `utils/attachments.ts:565` | `getPlanModeAttachments` (`utils/attachments.ts:1234`); post-compact `services/compact/compact.ts:1554` | `utils/messages.ts:3826` | Ported. |
| `plan_mode_reentry` | `utils/attachments.ts:572` | `getPlanModeAttachments` (`utils/attachments.ts:1217`) | `utils/messages.ts:3829` | Ported. |
| `plan_mode_exit` | `utils/attachments.ts:576` | `getPlanModeExitAttachment` (`utils/attachments.ts:1272`) | `utils/messages.ts:3848` | Ported. |
| `auto_mode` | `utils/attachments.ts:581` | `getAutoModeAttachments` (`utils/attachments.ts:1373`) | `utils/messages.ts:3860` | Ported. |
| `auto_mode_exit` | `utils/attachments.ts:585` | `getAutoModeExitAttachment` (`utils/attachments.ts:1399`) | `utils/messages.ts:3863` | Ported. |
| `critical_system_reminder` | `utils/attachments.ts:588` | `getCriticalSystemReminderAttachment` (`utils/attachments.ts:1594`) | `utils/messages.ts:3872` | Ported. |
| `plan_file_reference` | `utils/attachments.ts:592` | `services/compact/compact.ts:1482` | `utils/messages.ts:3636` | Outside this crate; post-compact plan reference. |
| `mcp_resource` | `utils/attachments.ts:597` | `processMcpResourceAttachments` (`utils/attachments.ts:2039`) | `utils/messages.ts:3877` | Ported as `mcp_resources`. |
| `command_permissions` | `utils/attachments.ts:605` | `utils/processUserInput/processSlashCommand.tsx:909` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `task_status` | `utils/attachments.ts:611` | `utils/task/framework.ts:32` source type; `getUnifiedTaskAttachments` (`utils/attachments.ts:3454`); post-compact `services/compact/compact.ts:1586` | `utils/messages.ts:3954` | Ported. |
| `token_usage` | `utils/attachments.ts:621` | `getTokenUsageAttachment` (`utils/attachments.ts:3820`) | `utils/messages.ts:4058` | Ported, default off. |
| `budget_usd` | `utils/attachments.ts:627` | `getMaxBudgetUsdAttachment` (`utils/attachments.ts:3856`) | `utils/messages.ts:4067` | Ported. |
| `output_token_usage` | `utils/attachments.ts:633` | `getOutputTokenUsageAttachment` (`utils/attachments.ts:3836`) | `utils/messages.ts:4076` | Ported, default off. |
| `structured_output` | `utils/attachments.ts:639` | `services/tools/toolExecution.ts:1276` | `utils/messages.ts:4252` -> `[]` | Silent / UI-only. |
| `teammate_mailbox` | `utils/attachments.ts:720` | `getTeammateMailboxAttachments` (`utils/attachments.ts:3680`) | `utils/messages.ts:3457` pre-switch block | Ported. |
| `team_context` | `utils/attachments.ts:731` | `getTeamContextAttachment` (`utils/attachments.ts:3797`) | `utils/messages.ts:3467` pre-switch block | Ported. |
| `invoked_skills` | `utils/attachments.ts:646` | `services/compact/compact.ts:1531` | `utils/messages.ts:3644` | Ported. |
| `verify_plan_reminder` | `utils/attachments.ts:654` | `getVerifyPlanReminderAttachment` (`utils/attachments.ts:3928`) | `utils/messages.ts:4240` | Ported; emits only when `VerifyPlanExecution` is visible. |
| `max_turns_reached` | `utils/attachments.ts:657` | `query.ts:1510`, `query.ts:1707` | No `normalizeAttachmentForAPI` case | TS runtime/UI bookkeeping; not a model reminder in this snapshot. |
| `current_session_memory` | `utils/attachments.ts:662` | Type only in scanned TS snapshot | No `normalizeAttachmentForAPI` case | TS runtime/UI bookkeeping; no emitter found. |
| `teammate_shutdown_batch` | `utils/attachments.ts:668` | `utils/collapseTeammateShutdowns.ts:43` | No `normalizeAttachmentForAPI` case | UI/transcript collapse marker. |
| `compaction_reminder` | `utils/attachments.ts:672` | `getCompactionReminderAttachment` (`utils/attachments.ts:3954`) | `utils/messages.ts:4139` | Ported. |
| `context_efficiency` | `utils/attachments.ts:675` | `getContextEfficiencyAttachment` (`utils/attachments.ts:3982`) | `utils/messages.ts:4148` | TS `HISTORY_SNIP` feature-gated; not ported until snip runtime exists. |
| `date_change` | `utils/attachments.ts:678` | `getDateChangeAttachments` (`utils/attachments.ts:1443`) | `utils/messages.ts:4162` | Ported. |
| `ultrathink_effort` | `utils/attachments.ts:682` | `getUltrathinkEffortAttachment` (`utils/attachments.ts:1451`) | `utils/messages.ts:4170` | Ported, default off. |
| `deferred_tools_delta` | `utils/attachments.ts:686` | `getDeferredToolsDeltaAttachment` (`utils/attachments.ts:1474`); also re-announced by compact | `utils/messages.ts:4178` | Ported. |
| `agent_listing_delta` | `utils/attachments.ts:692` | `getAgentListingDeltaAttachment` (`utils/attachments.ts:1548`); also re-announced by compact | `utils/messages.ts:4194` | Ported. |
| `mcp_instructions_delta` | `utils/attachments.ts:702` | `getMcpInstructionsDeltaAttachment` (`utils/attachments.ts:1584`); also re-announced by compact | `utils/messages.ts:4216` | Ported. |
| `companion_intro` | `utils/attachments.ts:708` | `buddy/prompt.ts:31`; gated from `utils/attachments.ts:866` | `utils/messages.ts:4232` | Ported, default off. |
| `bagel_console` | `utils/attachments.ts:713` | Type only in scanned TS snapshot | No API-text case in `utils/messages.ts` | UI/runtime placeholder; no model reminder found. |

## Intentionally skipped (TS-first rule)

| TS id | Why |
|---|---|
| `context_efficiency` (HISTORY_SNIP) | TS snip-compact nudge behind `feature('HISTORY_SNIP')`; port only if coco-rs ships the matching snip runtime/tool. |
| `skill_discovery` | Ported as the only audit-add model-visible reminder. Controlled by `system_reminder.attachments.skill_discovery`, not a `Feature`, because coco-rs uses config for skills. |
| `security_guidelines` | Zero matches in TS — cocode-rs Phase-2 invention, not TS-sourced. |
| `hook_cancelled` / `hook_error_during_execution` / `hook_non_blocking_error` / `hook_permission_decision` / `hook_system_message` / `structured_output` / `dynamic_skill` / `bagel_console` / `command_permissions` | TS `return []` in `normalizeAttachmentForAPI` — API-hidden typed events, produce zero API text. |
| `max_turns_reached` / `current_session_memory` / `teammate_shutdown_batch` | TS runtime/UI bookkeeping. They are in the TS `Attachment` union, but `normalizeAttachmentForAPI()` has no API-text case for them in this snapshot. |
| `image` | Content block subtype, not a reminder attachment. |

## Direct / non-attachment model-visible reminders

These are not `Attachment.type` mappings, but the broad TS scan found them
putting reminder-like text into model-visible user/tool-result/prompt content.

| TS location | Kind | Trigger | Notes |
|---|---|---|---|
| `utils/messages.ts:3097`, `utils/messages.ts:3101` | Direct XML wrapper helper | Called by attachment rendering and other subsystems | `wrapInSystemReminder()` / `wrapMessagesInSystemReminder()` are the canonical text wrappers. |
| `utils/messages.ts:1797`, `utils/messages.ts:2276` | Attachment post-pass wrapper | Feature gate `tengu_chair_sermon` | Ensures attachment-origin text is XML-wrapped even when a `normalizeAttachmentForAPI()` branch forgot to wrap. |
| `commands/brief.ts:114` | Direct XML meta message | `/brief` toggled and Kairos is inactive | Adds a one-shot meta message telling the model to use or stop using the Brief tool. |
| `utils/processUserInput/processSlashCommand.tsx:576`, `screens/REPL.tsx:3240` | Plain meta-message sink | Slash/local JSX commands return `metaMessages` | These messages are `isMeta: true`; they may be already wrapped by the producer, but the sink itself does not add XML. |
| `components/permissions/rules/PermissionRuleList.tsx:805` | Plain meta-message producer | User retries previously denied permission rules | Emits "Permission granted..." as a hidden model-visible message via the slash-command meta-message sink. |
| `utils/sideQuestion.ts:61` | Direct XML prompt prefix | Side-question fork is spawned | Wraps side-question constraints and the user question in a direct system-reminder prefix. |
| `tools/FileReadTool/FileReadTool.ts:706`, `tools/FileReadTool/FileReadTool.ts:707`, `tools/FileReadTool/FileReadTool.ts:730` | Direct XML tool-result reminder | Empty file, offset past EOF, or cyber-risk mitigation enabled | Tool-result content includes direct `<system-reminder>` warnings. |
| `memdir/memoryAge.ts:52` | Direct XML helper | Stale memory file is read by a caller that does not add its own wrapper | `memoryFreshnessNote()` returns a direct wrapped note. |
| `utils/api.ts:463` | Direct XML meta message | Extra API context is provided | Prepends a hidden context-guidance user message. |
| `utils/hooks.ts:238` | Direct XML queued notification | Async Stop hook exits with blocking code `2` | Enqueues a task notification whose value is already wrapped with `wrapInSystemReminder()`. |
| `cli/print.ts:379` | Direct XML prompt | Non-interactive team shutdown is required | Sends `SHUTDOWN_TEAM_PROMPT` before final response. |
| `utils/messages.ts:567` | Plain synthetic caveat | Local bash / slash command transcript caveat is inserted | Tells the model not to answer local-command-generated messages unless explicitly asked. |
| `query.ts:1226` | Plain meta-message recovery nudge | Max-output-token recovery path continues the turn | Tells the model to resume directly after output token truncation. |
| `query/tokenBudget.ts:70`, `query.ts:1326` | Plain meta-message nudge | `TOKEN_BUDGET` continuation decision says continue | Adds `decision.nudgeMessage` as hidden model-visible user content; this is not an `Attachment`. |
| `services/tools/toolExecution.ts:1096` | Plain meta-message hook retry nudge | `PermissionDenied` hook reports the command is now approved | Tells the model it may retry the denied command. |
| `query/stopHooks.ts:260`, `query/stopHooks.ts:378`, `query/stopHooks.ts:420` | Plain meta-message hook blocking notice | Stop / TaskCompleted / TeammateIdle hooks return blocking errors | Emits hidden hook-blocking messages in addition to structured hook attachments. |
| `commands/ultraplan.tsx:219` | Plain queued meta notice | User stops an active ultraplan session | Tells the model not to answer the stop notification and to wait for the next message. |
| `utils/conversationRecovery.ts:213` | Plain meta-message continuation | Recovered conversation detected an interrupted turn | Inserts "Continue from where you left off." |
| `services/api/claude.ts:1339` | Plain API prelude context | Deferred tools are enabled but the ToolSearch message form is not used | Prepends `<available-deferred-tools>...</available-deferred-tools>` as hidden model-visible context. |
| `services/compact/compact.ts:286` | Plain retry marker | Prompt-too-long retry drops earlier message groups | Prepends `[earlier conversation truncated for compaction retry]` to preserve API role alternation and context. |
| `services/mcp/channelNotification.ts:106`, `services/mcp/useManageMCPConnections.ts:525`, `cli/print.ts:4751`, `cli/print.ts:4827` | Plain queued channel message | MCP channel notifications arrive | Wraps channel content in `<channel-message ...>` and enqueues it as hidden model-visible prompt input. |
| `cli/print.ts:1848`, `screens/REPL.tsx:4092` | Plain proactive tick | Proactive/Kairos tick fires | Enqueues `<tick>...</tick>` as hidden model-visible prompt input. |
| `hooks/useScheduledTasks.ts:73`, `cli/print.ts:2714` | Plain scheduled-task prompt | Local scheduled task / cron fires | Enqueues the task prompt as hidden model-visible input. |
| `utils/teleport.tsx:68` | Plain meta-message state notice | Session is resumed from another machine | Tells the model application state may have changed and gives updated cwd. |
| `tools/TodoWriteTool/TodoWriteTool.ts:106`, `tools/TodoWriteTool/TodoWriteTool.ts:112` | Plain tool-result nudge | V1 todo list closes 3+ tasks with no verification step | Appends a verification-agent note to the tool result. |
| `tools/TaskUpdateTool/TaskUpdateTool.ts:386`, `tools/TaskUpdateTool/TaskUpdateTool.ts:397` | Plain tool-result nudge | V2 task completion / teammate task completion | Appends verification-agent note, and for swarm teammates a "call TaskList now" note. |
| `tools/WebSearchTool/WebSearchTool.ts:427` | Plain tool-result reminder | WebSearch returns sources | Appends mandatory-source-citation reminder to the tool result. |
| `services/compact/prompt.ts:270` | Prompt-template reminder | Compaction summarizer prompt | Static reminder to avoid tool calls during summarization. |
| `services/SessionMemory/prompts.ts:73`, `services/SessionMemory/prompts.ts:164`, `services/SessionMemory/prompts.ts:235` | Prompt-template reminders | Session-memory extraction / update prompts | Static section-preservation reminders for the session-memory model call. |
| `commands/security-review.ts:136` | Prompt-template reminder | `/security-review` command prompt | Static final reminder inside that command's generated prompt. |
| `commands/init.ts:215` | Prompt-template reminder | `/init` command asks the model to produce setup recap | Static instruction to remind the user generated files are only a starting point. |
| `utils/permissions/yoloClassifier.ts:561` | Classifier prompt suffix reminder | Yolo/auto-mode permission classifier stage-1 prompt | Static reminder that explicit user confirmation is required to override blocks. |
| `skills/bundled/scheduleRemoteAgents.ts:283`, `skills/bundled/scheduleRemoteAgents.ts:320` | Skill-generated reminder text | Scheduling remote agents | Skill prompt tells the agent to remind users about remote-agent environment limits and, conditionally, GitHub access setup. |
| `tools/ScheduleCronTool/prompt.ts:78`, `tools/ScheduleCronTool/prompt.ts:87`, `tools/ScheduleCronTool/prompt.ts:93`, `tools/ScheduleCronTool/prompt.ts:95`, `tools/ScheduleCronTool/prompt.ts:108`, `tools/ScheduleCronTool/prompt.ts:110`, `tools/ScheduleCronTool/CronCreateTool.ts:36` | Tool prompt examples / guidance | Schedule/Cron tool prompt | Static prompt text uses "remind me" examples and nudges minute selection for scheduled reminders; not a system-reminder producer. |

Broad scan hits that are not model-context reminder producers are intentionally
not mapped above: UI tips / upgrade nudges (`services/tips/tipRegistry.ts`,
`hooks/useReplBridge.tsx`, `hooks/useTypeahead.tsx:1323`,
`components/messages/SystemTextMessage.tsx:552`,
`components/messages/SystemTextMessage.tsx:553`,
`components/messages/SystemTextMessage.tsx:799`,
`components/messages/SystemTextMessage.tsx:800`,
`components/messages/SystemTextMessage.tsx:801`), Grove notice
reminder-frequency UI (`services/api/grove.ts:309`,
`services/api/grove.ts:310`, `services/api/grove.ts:315`), prompt comments,
telemetry extraction
(`utils/telemetry/betaSessionTracing.ts`), transcript/UI stripping
(`components/messageActions.tsx`, `utils/transcriptSearch.ts`), and comments
that do not add model-visible text.
Full `isMeta: true` scan also finds model-visible payload transport that is not
a reminder/control notice: image metadata and image/PDF payloads
(`utils/processUserInput/processUserInput.ts:600`,
`tools/FileReadTool/FileReadTool.ts:887`, `tools/FileReadTool/FileReadTool.ts:942`,
`tools/FileReadTool/FileReadTool.ts:1013`), skill invocation
content (`utils/processUserInput/processSlashCommand.tsx:861`,
`utils/processUserInput/processSlashCommand.tsx:907`,
`tools/SkillTool/SkillTool.ts:1104`, `tools/AgentTool/runAgent.ts:641`), and
generic attachment/tool-use rendering helpers (`utils/messages.ts:4300`,
`utils/messages.ts:4331`, `utils/messages.ts:5383`,
`utils/messages.ts:5395`). Those are model-visible context carriers, but not reminder
implementations.
Full attachment-type-string scan also finds non-attachment homonyms:
commit-attribution file stats (`utils/commitAttribution.ts:682`),
autocomplete/typeahead suggestions (`hooks/unifiedSuggestions.ts:13`,
`hooks/unifiedSuggestions.ts:22`, `hooks/unifiedSuggestions.ts:128`,
`hooks/unifiedSuggestions.ts:140`, `hooks/useTypeahead.tsx:46`,
`utils/suggestions/directoryCompletion.ts:12`,
`utils/suggestions/directoryCompletion.ts:18`,
`utils/suggestions/directoryCompletion.ts:104`,
`utils/suggestions/directoryCompletion.ts:138`), UI display props
(`components/DiagnosticsDisplay.tsx:11`,
`components/messages/AttachmentMessage.tsx:359`), and transcript/system-only
metadata (`utils/plans.ts:386`). These use overlapping string literals but are
not `Attachment` reminder implementations.

## Prompt references to reminder mechanisms

These TS locations do not emit reminder messages by themselves, but they are
model-visible prompt text that tells the model how to interpret reminder
mechanisms.

| TS location | Prompt surface | Reference |
|---|---|---|
| `constants/prompts.ts:132`, `constants/prompts.ts:190`, `constants/prompts.ts:338`, `constants/prompts.ts:475` | Global system prompt / environment details | Explains that tool results and user messages may include `<system-reminder>` or other system tags, and that skill-discovery reminders may be surfaced each turn. |
| `tools/AgentTool/prompt.ts:197` | Agent tool prompt | Says available agent types may be listed in `<system-reminder>` messages. |
| `tools/SkillTool/prompt.ts:189` | Skill tool prompt | Says available skills are listed in system-reminder messages. |
| `tools/ToolSearchTool/prompt.ts:40`, `tools/ToolSearchTool/prompt.ts:41` | ToolSearch prompt | Says deferred tools appear via `<system-reminder>` or `<available-deferred-tools>` messages depending on feature state. |
| `tools/FileReadTool/prompt.ts:48` | FileRead tool prompt | Mentions the empty-file system-reminder warning returned in place of file contents. |
| `Tool.ts:275`, `utils/forkedAgent.ts:296`, `utils/forkedAgent.ts:458`, `utils/forkedAgent.ts:459`, `tools/AgentTool/loadAgentsDir.ts:121`, `tools/AgentTool/runAgent.ts:711`, `tools/AgentTool/runAgent.ts:712`, `tools/AgentTool/built-in/verificationAgent.ts:150`, `entrypoints/sdk/coreSchemas.ts:1134`, `entrypoints/sdk/coreSchemas.ts:1137` | Critical reminder configuration/schema/pass-through | Defines and threads `criticalSystemReminder_EXPERIMENTAL`; actual attachment emission remains `utils/attachments.ts:1587`-`1594`. |
| `commands/ultraplan.tsx:36`, `commands/ultraplan.tsx:61` | Code comment for remote CCR prompt assembly | Documents that an inlined `prompt.txt` asset is expected to be wrapped in `<system-reminder>`; the scanned checkout contains the import but not the text asset itself. |

## Execution semantics

This section covers reminder execution mechanics, not new reminder types.

| Concern | TS behavior | coco-rs behavior / parity note |
|---|---|---|
| Entry point | `getAttachments()` is the batch producer (`utils/attachments.ts:743`). `getAttachmentMessages()` then yields one `AttachmentMessage` per returned attachment (`utils/attachments.ts:2937`, `utils/attachments.ts:2968`). | `QueryEngine` creates one session-scoped `SystemReminderOrchestrator` (`app/query/src/engine.rs:699`, `app/query/src/engine.rs:701`), builds `TurnReminderInput`, calls `run_turn_reminders()`, then appends the results with `inject_reminders()` (`app/query/src/engine.rs:1213`, `app/query/src/engine.rs:1214`). |
| Disabled / simple mode | If `CLAUDE_CODE_DISABLE_ATTACHMENTS` or `CLAUDE_CODE_SIMPLE` is set, TS skips normal attachments but still returns `queued_command` attachments so drained queued commands are not lost (`utils/attachments.ts:752`-`760`). | The Rust crate has a master `system_reminder.enabled` switch. When false, `SystemReminderOrchestrator::generate_all()` returns no reminders (`core/system-reminder/src/orchestrator.rs:227`-`230`). It does not have the TS env-var special case that preserves queued commands. |
| Batch ordering | TS runs user-input attachments first (`utils/attachments.ts:817`-`819`) because `@file` processing seeds nested-memory triggers. It then builds `allThreadAttachments` (`utils/attachments.ts:821`-`941`) and `mainThreadAttachments` (`utils/attachments.ts:943`-`987`). Final output order is user-input results, then all-thread results, then main-thread results (`utils/attachments.ts:998`-`1002`). | Rust parses the latest user input before source materialization, passes mentioned paths into `MemorySource`, materializes cross-crate sources, then registers generators in the same TS flatten order. `join_all` preserves that applicable-generator order (`core/system-reminder/src/orchestrator.rs`), and `default_registry_order_matches_ts_attachment_batches` locks it. |
| Concurrency | TS runs all user-input attachment promises together with `Promise.all()` (`utils/attachments.ts:819`). After that, all-thread and main-thread batches run in parallel via nested `Promise.all()` (`utils/attachments.ts:989`-`993`). | Rust has two parallel stages: `ReminderSources::materialize()` fans out source calls with `tokio::join!` (`core/system-reminder/src/sources/mod.rs:281`-`311`), then `SystemReminderOrchestrator::generate_all()` runs applicable generators concurrently with `future::join_all()` (`core/system-reminder/src/orchestrator.rs:266`-`272`). |
| Timeout | TS creates an `AbortController`, schedules `abort()` after 1000 ms, and puts it into the attachment context (`utils/attachments.ts:766`-`768`). This is cooperative cancellation: only subcalls that observe the signal stop early. `clearTimeout()` runs after the main batches settle (`utils/attachments.ts:996`). | Rust uses hard `tokio::time::timeout` wrappers. Sources use `SystemReminderConfig.timeout_ms` as `per_source_timeout` (`app/query/src/engine.rs:1020`-`1038`) and `ReminderSources::gate()` returns defaults on timeout (`core/system-reminder/src/sources/mod.rs:55`-`68`, `core/system-reminder/src/sources/mod.rs:322`-`343`). Generators are also wrapped individually (`core/system-reminder/src/orchestrator.rs:336`-`356`). Default is 1000 ms (`common/config/src/system_reminder.rs:56`). |
| Error handling | Each TS producer is wrapped in `maybe()`: success may emit sampled telemetry; any thrown error is logged and becomes `[]` (`utils/attachments.ts:1005`-`1040`). One failed attachment producer does not poison the turn. | Source timeouts return default values. Generator errors and timeouts are logged and become `None` (`core/system-reminder/src/orchestrator.rs:345`-`356`). A failed generator does not block other generators. |
| Throttle / full-content state | TS scans prior attachment messages to decide cadence and whether a reminder is full or sparse, e.g. plan/auto modes and todo/task reminders. The exact producer rows above cite the individual scanners. | Rust pre-computes full/sparse decisions before running generators (`core/system-reminder/src/orchestrator.rs:233`-`243`), filters by config/tier/throttle (`core/system-reminder/src/orchestrator.rs:246`-`252`, `core/system-reminder/src/orchestrator.rs:288`-`319`), and only marks throttle state after a generator actually returns a reminder (`core/system-reminder/src/orchestrator.rs:274`-`277`). |
| Multiple reminders and API-message merging | TS does not merge attachments during generation: every attachment is yielded separately (`utils/attachments.ts:2967`, `utils/attachments.ts:2968`). During `normalizeMessagesForAPI()`, an attachment is rendered through `normalizeAttachmentForAPI()` (`utils/messages.ts:2269`-`2277`); if the previous API message is also user-role, TS folds the rendered attachment message(s) into that user message (`utils/messages.ts:2279`-`2285`) via `mergeUserMessagesAndToolResults()` (`utils/messages.ts:2372`-`2386`). Consecutive user messages may also merge later (`utils/messages.ts:4907`-`4918`). | Rust injects each simple text reminder as a separate `Message::Attachment` (`core/system-reminder/src/inject.rs:149`-`167`). The prompt normalizer extracts attachment messages and merges consecutive same-role `LlmMessage::User` entries (`core/messages/src/normalize.rs:85`-`94`, `core/messages/src/normalize.rs:107`-`125`). So multiple reminder messages can collapse into one API user message, but the history still stores separate reminder messages. |
| Tool-result sibling folding | TS has a gated post-pass: `ensureSystemReminderWrap()` wraps any unwrapped attachment-origin text (`utils/messages.ts:1797`-`1810`), and `smooshSystemReminderSiblings()` can fold `<system-reminder>` text siblings into the last `tool_result` block (`utils/messages.ts:1820`-`1858`, `utils/messages.ts:5366`-`5374`). | The current Rust normalizer only does role-level merge (`core/messages/src/normalize.rs:123`-`156`). It does not implement TS's `tengu_chair_sermon` tool-result sibling folding. |
| Post-emit bookkeeping | TS state changes are mostly embedded in producer implementations, e.g. drain-on-read hook queues or mailbox read marking. | Rust does explicit post-emit bookkeeping in `QueryEngine`: clear one-shot plan/auto flags and update last-announced tool/agent/MCP baselines only for fired reminder types (`app/query/src/engine.rs:1198`-`1208`). |

## Reverse-scan mechanism coverage

These are mechanism-level references found by exact scans for
`createAttachmentMessage`, `getAttachmentMessages`, `normalizeAttachmentForAPI`,
`wrapInSystemReminder`, `wrapMessagesInSystemReminder`, and literal
`<system-reminder>`. They are not new reminder types; they validate producer
flow, rendering, token estimation, and non-model consumers.

| Surface | TS locations | Classification |
|---|---|---|
| Attachment drain / construction | `query.ts:1509`, `query.ts:1580`, `query.ts:1624`, `query.ts:1706`, `utils/attachments.ts:2937`, `utils/attachments.ts:2968`, `utils/attachments.ts:3201`, `utils/processUserInput/processUserInput.ts:504`, `utils/processUserInput/processSlashCommand.tsx:897` | Main turn, memory/skill prefetch, max-turn signal, and slash-command attachment drain. Type-specific mappings are in the coverage index above. |
| Hook / session attachment construction | `utils/sessionStart.ts:164`, `utils/sessionStart.ts:221`, `tools/AgentTool/runAgent.ts:547`, `utils/processUserInput/processUserInput.ts:232`, `utils/processUserInput/processSlashCommand.tsx:908`, `query/stopHooks.ts:273`, `query/stopHooks.ts:388`, `query/stopHooks.ts:430`, `utils/hooks.ts:713`, `utils/hooks.ts:720`, `utils/hooks.ts:2168`, `utils/hooks.ts:2207`, `utils/hooks.ts:2322`, `utils/hooks.ts:2348`, `utils/hooks.ts:2379`, `utils/hooks.ts:2485`, `utils/hooks.ts:2516`, `utils/hooks.ts:2580`, `utils/hooks.ts:2629`, `utils/hooks.ts:2683`, `utils/hooks.ts:2715`, `utils/hooks.ts:2772`, `utils/hooks.ts:4824`, `utils/hooks/execPromptHook.ts:121`, `utils/hooks/execPromptHook.ts:141`, `utils/hooks/execPromptHook.ts:175`, `utils/hooks/execPromptHook.ts:200`, `utils/hooks/execAgentHook.ts:296`, `utils/hooks/execAgentHook.ts:328`, `services/tools/toolHooks.ts:80`, `services/tools/toolHooks.ts:107`, `services/tools/toolHooks.ts:120`, `services/tools/toolHooks.ts:135`, `services/tools/toolHooks.ts:178`, `services/tools/toolHooks.ts:235`, `services/tools/toolHooks.ts:259`, `services/tools/toolHooks.ts:272`, `services/tools/toolHooks.ts:306`, `services/tools/toolHooks.ts:570`, `services/tools/toolHooks.ts:593`, `services/tools/toolHooks.ts:633`, `services/tools/toolExecution.ts:986`, `services/tools/toolExecution.ts:1275`, `services/tools/toolExecution.ts:1574` | Hook/session producers already split into rendered vs silent attachment types in the coverage index. |
| Post-compact attachment re-injection | `services/compact/compact.ts:573`, `services/compact/compact.ts:576`, `services/compact/compact.ts:584`, `services/compact/compact.ts:963`, `services/compact/compact.ts:966`, `services/compact/compact.ts:974`, `services/compact/compact.ts:1448`, `services/compact/compact.ts:1481`, `services/compact/compact.ts:1530`, `services/compact/compact.ts:1553`, `services/compact/compact.ts:1585` | Re-announces file/plan/skill/plan-mode/task-status/delta context after compaction; covered by the outside-crate and ported rows above. |
| Attachment rendering / token estimation | `utils/messages.ts:2270`, `utils/messages.ts:3453`, `services/tokenEstimation.ts:360` | `normalizeAttachmentForAPI()` call sites. Token estimation renders attachments for budgeting; it does not emit a new reminder. |
| XML wrapper helpers and wrapper call sites | `utils/messages.ts:1797`, `utils/messages.ts:1803`, `utils/messages.ts:1810`, `utils/messages.ts:2276`, `utils/messages.ts:3097`, `utils/messages.ts:3098`, `utils/messages.ts:3101`, `utils/messages.ts:3110`, `utils/messages.ts:3119`, `utils/messages.ts:3294`, `utils/messages.ts:3380`, `utils/messages.ts:3394`, `utils/messages.ts:3414`, `utils/messages.ts:3440`, `utils/messages.ts:3448`, `utils/messages.ts:3510`, `utils/messages.ts:3526`, `utils/messages.ts:3539`, `utils/messages.ts:3549`, `utils/messages.ts:3557`, `utils/messages.ts:3573`, `utils/messages.ts:3582`, `utils/messages.ts:3593`, `utils/messages.ts:3601`, `utils/messages.ts:3621`, `utils/messages.ts:3629`, `utils/messages.ts:3637`, `utils/messages.ts:3656`, `utils/messages.ts:3673`, `utils/messages.ts:3693`, `utils/messages.ts:3701`, `utils/messages.ts:3709`, `utils/messages.ts:3732`, `utils/messages.ts:3777`, `utils/messages.ts:3788`, `utils/messages.ts:3805`, `utils/messages.ts:3819`, `utils/messages.ts:3844`, `utils/messages.ts:3856`, `utils/messages.ts:3868`, `utils/messages.ts:3873`, `utils/messages.ts:3881`, `utils/messages.ts:3926`, `utils/messages.ts:3938`, `utils/messages.ts:3947`, `utils/messages.ts:3963`, `utils/messages.ts:3991`, `utils/messages.ts:4021`, `utils/messages.ts:4054`, `utils/messages.ts:4061`, `utils/messages.ts:4070`, `utils/messages.ts:4083`, `utils/messages.ts:4093`, `utils/messages.ts:4111`, `utils/messages.ts:4123`, `utils/messages.ts:4133`, `utils/messages.ts:4140`, `utils/messages.ts:4153`, `utils/messages.ts:4163`, `utils/messages.ts:4171`, `utils/messages.ts:4190`, `utils/messages.ts:4212`, `utils/messages.ts:4228`, `utils/messages.ts:4233`, `utils/messages.ts:4248` | Rendering implementation for the API cases listed in the coverage index; these are not separate producers. |
| Literal XML producer / consumer scan | `commands/brief.ts:108`, `commands/brief.ts:114`, `commands/brief.ts:118`, `cli/print.ts:379`, `cli/print.ts:389`, `memdir/memoryAge.ts:27`, `memdir/memoryAge.ts:45`, `memdir/memoryAge.ts:52`, `utils/api.ts:463`, `utils/api.ts:469`, `utils/sideQuestion.ts:61`, `utils/sideQuestion.ts:76`, `utils/hooks.ts:138`, `utils/messages.ts:1791`, `utils/messages.ts:1800`, `utils/messages.ts:1808`, `utils/messages.ts:1820`, `utils/messages.ts:1849`, `utils/messages.ts:2330`, `utils/messages.ts:2502`, `utils/messages.ts:3470`, `utils/messages.ts:3495`, `utils/queryHelpers.ts:432`, `utils/attachments.ts:271`, `utils/attachments.ts:2270`, `utils/transcriptSearch.ts:117`, `utils/transcriptSearch.ts:120`, `utils/transcriptSearch.ts:125`, `components/messageActions.tsx:402`, `components/VirtualMessageList.tsx:126`, `utils/telemetry/betaSessionTracing.ts:141`, `utils/telemetry/betaSessionTracing.ts:143`, `utils/telemetry/betaSessionTracing.ts:146`, `services/vcr.ts:302` | Direct XML producers are mapped above; import/comment/predicate/query/transcript/UI/telemetry/VCR entries strip, detect, or preserve reminder text but do not create reminders. |

## Cadence constants (TS-verbatim)

| Preset | `min_turns_between` | `full_content_every_n` | TS source |
|---|---|---|---|
| plan-mode | 5 | 5 | `PLAN_MODE_ATTACHMENT_CONFIG.{TURNS_BETWEEN_ATTACHMENTS, FULL_REMINDER_EVERY_N_ATTACHMENTS}` |
| auto-mode | 5 | 5 | `AUTO_MODE_ATTACHMENT_CONFIG` |
| todo / task reminder | 10 | — | `TODO_REMINDER_CONFIG.{TURNS_SINCE_WRITE, TURNS_BETWEEN_REMINDERS}` |
| verify-plan | 10 | — | `VERIFY_PLAN_REMINDER_CONFIG.TURNS_BETWEEN_REMINDERS` |
| one-shots (exit banners / critical / compaction / date-change / delta reminders) | 0 | — | — |

## Settings

All reminder toggles live under `settings.json` → `system_reminder.attachments`:

```jsonc
{
  "system_reminder": {
    "enabled": true,
    "timeout_ms": 1000,
    "critical_instruction": "Optional verbatim text injected every turn",
    "attachments": {
      "plan_mode": true,
      "plan_mode_exit": true,
      "plan_mode_reentry": true,
      "auto_mode": true,
      "auto_mode_exit": true,
      "todo_reminder": true,
      "task_reminder": true,
      "verify_plan_reminder": true,
      "critical_system_reminder": true,
      "compaction_reminder": true,
      "date_change": true,
      "ultrathink_effort": false,        // opt-in (TS feature('ULTRATHINK'))
      "token_usage": false,              // opt-in (TS env var)
      "budget_usd": true,
      "output_token_usage": false,       // opt-in (TS feature('TOKEN_BUDGET'))
      "companion_intro": false,          // opt-in (TS feature('BUDDY'))
      "deferred_tools_delta": true,
      "agent_listing_delta": true,
      "mcp_instructions_delta": true,
      "hook_success": true,
      "hook_blocking_error": true,
      "hook_additional_context": true,
      "hook_stopped_continuation": true,
      "async_hook_response": true,
      "diagnostics": true,
      "output_style": true,
      "queued_command": true,
      "task_status": true,
      "skill_listing": true,
      "invoked_skills": true,
      "teammate_mailbox": true,
      "team_context": true,
      "agent_pending_messages": true,
      "at_mentioned_files": true,
      "mcp_resources": true,
      "agent_mentions": true,
      "ide_selection": true,
      "ide_opened_file": true,
      "nested_memory": true,
      "relevant_memories": true
    }
  }
}
```

Feature-gated reminders default **off** to match TS external-build behavior (`feature('X')` → false in external builds).

## Global TS entry point

`getAttachments(input, toolUseContext, ideSelection, queuedCommands, messages?, querySource?)` at `utils/attachments.ts:743-1002` is the canonical per-turn orchestrator. It splits reminders into three batches (`userInputAttachments` / `allThreadAttachments` / `mainThreadAttachments`) which map to this crate's `User` / `Core` / `Main` tiers respectively.
