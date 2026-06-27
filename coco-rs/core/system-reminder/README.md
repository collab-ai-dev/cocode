# coco-system-reminder

Per-turn `<system-reminder>` injection. Owns the 42 generators below (40 model-visible, 2 silent/display-only). File/context attachments, UI-only attachments, hook bookkeeping, slash-command metadata, and direct tool-result `<system-reminder>` strings live outside this crate — covered in the scan sections after the catalog.

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
 reminder key. A generator only gates on `GeneratorContext`, renders
 text, and returns `Option<SystemReminder>`.
4. **Orchestration** (`orchestrator.rs`, `throttle.rs`) applies config, tier,
 throttle, full/sparse cadence, and timeout policy. Generators run in
 parallel, while injection order is: user-input,
 all-thread, then main-thread.
5. **Injection** (`inject.rs`, `xml.rs`) converts `SystemReminder` values into
 `coco_types::Message` entries and routes silent reminders to the display-only
 sink so they never reach the model.

## coco-system-reminder catalog (42 generators)

Columns:
- **ID** — `coco-system-reminder` attachment/settings key. Most IDs map directly to `AttachmentKind` variants; a few are coco-rs synthetic grouping keys that cover multiple concrete types (`file`, `mcp_resource`, `agent_mention`, `selected_lines_in_ide`, `opened_file_in_ide`, or `queued_command`). See the Attachment coverage index for the type mapping.
- **Tier** — `Core` (all agents) / `Main` (main agent only) / `User` (only when user submitted input this turn)
- **What it does** — one-line purpose
- **Trigger** — gate chain
- **Settings** — `settings.json` → `system_reminder.attachments.<key>`; **bold** = default value
- **Source function** — trigger function reference
- **Text template** — text-template function reference

### Plan / Auto mode (5)

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `plan_mode` | Core | Injects the multi-phase plan-mode workflow instructions while the agent is in plan mode | `is_plan_mode == true`; 5-human-turn throttle, Full content every 5th emission, Sparse otherwise | `plan_mode` (**true**) | `getPlanModeAttachments` | `case 'plan_mode':` |
| `plan_mode_exit` | Core | One-shot "you have exited plan mode" banner after the agent leaves plan mode | Set by `ExitPlanMode` tool success or by the engine on an unannounced Plan→non-Plan transition; cleared post-emit | `plan_mode_exit` (**true**) | `getPlanModeExitAttachment` | `case 'plan_mode_exit':` |
| `plan_mode_reentry` | Core | One-shot "re-entering plan mode" banner when returning to plan with a prior plan file present | First plan turn after prior exit in this session, plan file exists, not a sub-agent | `plan_mode_reentry` (**true**) | `getPlanModeAttachments` (`:1186`, `plan_mode_reentry` branch) | `case 'plan_mode_reentry':` |
| `auto_mode` | Core | Injects autonomous-execution guidelines while auto mode is active | `is_auto_mode == true` (Auto permission mode OR Plan+classifier active); 5-human-turn throttle, Full every 5th | `auto_mode` (**true**) | `getAutoModeAttachments` | `case 'auto_mode':` |
| `auto_mode_exit` | Core | One-shot "you have exited auto mode" banner | Exit flag set on Auto→non-Auto transition; suppressed if still in auto mode | `auto_mode_exit` (**true**) | `getAutoModeExitAttachment` | `case 'auto_mode_exit':` |

### Todo / Task / Verify (3)

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `todo_reminder` | Core | Nudges the agent to use `TodoWrite` when it has been silent on task tracking | `TodoWrite` tool present AND `Brief` tool absent AND `turns_since_last_todo_write ≥ 10` AND `turns_since_last_todo_reminder ≥ 10`; V2-enabled sessions route to `task_reminder` instead | `todo_reminder` (**true**) | `getTodoReminderAttachments` | `case 'todo_reminder':` |
| `task_reminder` | Core | V2 equivalent of `todo_reminder` — nudges toward `TaskCreate`/`TaskUpdate` | `is_task_v2_enabled` AND `USER_TYPE != ant` AND `TaskUpdate` tool present AND `Brief` tool absent AND 10-turn silence gates | `task_reminder` (**true**) | `getTaskReminderAttachments` | `case 'task_reminder':` |
| `verify_plan_reminder` | Main | Prompts the agent to call `VerifyPlanExecution` after an `ExitPlanMode` | Pending-verification state exists, has not started/completed, `VerifyPlanExecution` is visible, AND every 10 human turns after plan exit env | `verify_plan_reminder` (**true**, no-op unless the tool is visible) | `getVerifyPlanReminderAttachment` | `case 'verify_plan_reminder':` |

### Critical / Compaction / Date (3)

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `critical_system_reminder` | Core | Injects a user-supplied critical instruction on every turn | `config.critical_instruction.is_some()` | `critical_system_reminder` (**true**) | `getCriticalSystemReminderAttachment` | `case 'critical_system_reminder':` |
| `compaction_reminder` | Core | Reassures the agent that auto-compaction will preserve context on large windows | Auto-compact enabled AND context window ≥ 1 M AND used tokens ≥ 25% of effective window | `compaction_reminder` (**true**) | `getCompactionReminderAttachment` | `case 'compaction_reminder':` |
| `date_change` | Core | Notifies the agent when the local date rolls over (e.g. coding past midnight) | Local ISO date differs from the per-session latched date; first observation seeds without emit | `date_change` (**true**) | `getDateChangeAttachments` | `case 'date_change':` |

### Engine-local reminders (5)

State comes from the engine / config / user input — no cross-crate dependency.

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `ultrathink_effort` | Core | Asks the agent to apply high reasoning effort when the user typed the `ultrathink` keyword | User prompt contains `ultrathink` (word-boundary, case-insensitive) | `ultrathink_effort` (**false**) | `getUltrathinkEffortAttachment` | `case 'ultrathink_effort':` |
| `token_usage` | Main | Reports `used/total; remaining` tokens every turn | Effective context window > 0 env | `token_usage` (**false**) | `getTokenUsageAttachment` | `case 'token_usage':` |
| `budget_usd` | Main | Reports `$used/$total; $remaining` when a USD budget is configured | `max_budget_usd.is_some()` | `budget_usd` (**true**) | `getMaxBudgetUsdAttachment` | `case 'budget_usd':` |
| `output_token_usage` | Main | Reports per-turn and session output-token counts against a turn budget | Turn-output-token budget set > 0 | `output_token_usage` (**false**) | `getOutputTokenUsageAttachment` | `case 'output_token_usage':` |
| `companion_intro` | Core | Introduces the configured companion character once per session | Companion name + species configured AND not previously announced | `companion_intro` (**false**) | | `case 'companion_intro':` |

### History-diff deltas (3)

Engine persists the previously-announced set on shared state and diffs each turn.

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `deferred_tools_delta` | Core | Announces tool availability changes so the agent knows what's new or gone | Current tool set differs from last-announced tool set | `deferred_tools_delta` (**true**) | `getDeferredToolsDeltaAttachment` | `case 'deferred_tools_delta':` |
| `agent_listing_delta` | Core | Lists agent types available for the `Agent` tool; flips header + adds concurrency note on first emission | Current agent-type set differs from last-announced | `agent_listing_delta` (**true**) | `getAgentListingDeltaAttachment` | `case 'agent_listing_delta':` |
| `mcp_instructions_delta` | Core | Surfaces added / removed MCP-server instructions mid-session | Per-server instructions differ from last-announced map | `mcp_instructions_delta` (**true**) | `getMcpInstructionsDeltaAttachment` | `case 'mcp_instructions_delta':` |

### Hook reminders (5)

All share one drain of pending hook events per turn.

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `hook_success` | Main | Surfaces successful hook stdout from `SessionStart` / `UserPromptSubmit` hooks | Success event of matching hookEvent AND non-empty content | `hook_success` (**true**) | emitted by sync hook executor (not in `getAttachments`) | `case 'hook_success':` |
| `hook_blocking_error` | Main | Reports why a hook blocked the turn (command + error text) | Blocking-error event from any hook | `hook_blocking_error` (**true**) | emitted by sync hook executor | `case 'hook_blocking_error':` |
| `hook_additional_context` | Main | Injects extra context lines a hook returned | Event with non-empty additional-context content | `hook_additional_context` (**true**) | emitted by sync hook executor | `case 'hook_additional_context':` |
| `hook_stopped_continuation` | Main | Reports when a hook halted a continuation | Stopped-continuation event | `hook_stopped_continuation` (**true**) | emitted by sync hook executor | `case 'hook_stopped_continuation':` |
| `async_hook_response` | Main | Multi-message surface for a completed async hook (systemMessage + additionalContext) | Completed async-hook response, drained on read (marks delivered) | `async_hook_response` (**true**) | `getAsyncHookResponseAttachments` | `case 'async_hook_response':` |

### Diagnostics / tasks / skills / misc (6)

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `diagnostics` | Main | Injects new LSP/IDE diagnostics wrapped in `<new-diagnostics>…</new-diagnostics>` | New-since-last-snapshot diagnostic entries available | `diagnostics` (**true**) | `getDiagnosticAttachments` + `getLSPDiagnosticAttachments` | `case 'diagnostics':` |
| `output_style` | Main | Reminds the agent to follow the active output-style guidelines | Active output-style name set | `output_style` (**true**) | `getOutputStyleAttachment` | `case 'output_style':` |
| `queued_command` | Core | Replays mid-turn queued items, each wrapped via `wrapCommandText` per typed [`QueueOrigin`] (coordinator / task-notification / channel / human) | Queue has at least one non-empty entry | `queued_command` (**true**) | `getQueuedCommandAttachments` | `case 'queued_command':` + `wrapCommandText` |
| `task_status` | Main | Warns against duplicate background-task spawns; reports running/completed/killed tasks | Inline main-thread task snapshot from `getUnifiedTaskAttachments()` when `generateTaskAttachments()` returns task deltas; post-compaction async-agent snapshot is also re-injected | `task_status` (**true**) | `getUnifiedTaskAttachments` / | `case 'task_status':` |
| `skill_listing` | Core | Lists available skills for the `Skill` tool | Active skill set non-empty (1% context-window budget) | `skill_listing` (**true**) | `getSkillListingAttachments` | `case 'skill_listing':` |
| `invoked_skills` | Main | Re-surfaces the content of skills invoked in this session so guidelines persist after compaction | Session has invoked skills with cached content | `invoked_skills` (**true**) | | `case 'invoked_skills':` |

### Swarm (3) — `agentSwarms` feature-gated upstream

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `teammate_mailbox` | Core | Delivers unread messages from other teammates (pre-formatted bundle) | Unread messages in this teammate's mailbox; skipped for `session_memory` fork (avoids silently stealing leader DMs) | `teammate_mailbox` (**true**) | `getTeammateMailboxAttachments` | |
| `team_context` | Core | One-shot first-turn team identity + member list for teammates | First turn as a teammate; not team lead; team registered | `team_context` (**true**) | `getTeamContextAttachment` | |
| `agent_pending_messages` | Core | Emits one `<system-reminder>` per pending teammate message, each wrapped with the coordinator-origin framing from `wrapCommandText`. Wire-level `AttachmentKind` maps to `QueuedCommand` for transcript parity. | Pending-messages inbox non-empty | `agent_pending_messages` (**true**) | `getAgentPendingMessageAttachments` | Emits `queued_command` attachments with coordinator origin |

### User-input tier (3)

All gated on the user submitting input this turn. UUID-dedup ensures one fire per human turn across multi-iteration tool loops.

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `at_mentioned_files` | User | Announces `@path` files the user mentioned in their prompt | User prompt contains parseable `@file` tokens | `at_mentioned_files` (**true**) | `processAtMentionedFiles` | 
| `mcp_resources` | User | Announces `@server:uri` MCP resource references the user mentioned | User prompt contains `@server:uri` tokens matching a registered server | `mcp_resources` (**true**) | `processMcpResourceAttachments` | |
| `agent_mentions` | User | Hints the agent to invoke an `@agent-type` the user referenced | User prompt contains `@agent-type` mentions | `agent_mentions` (**true**) | `processAgentMentions` | |

### Main-thread IDE (2)

These are main-agent-only and can fire even when no new user prompt arrived.

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `ide_selection` | Main | Surfaces the user's IDE text selection with a 2000-char truncation cap | IDE bridge reports a non-empty selection for a known filename | `ide_selection` (**true**) | `getSelectedLinesFromIDE` | |
| `ide_opened_file` | Main | Notes which file the user just opened in the IDE | IDE bridge reports a non-empty opened filename | `ide_opened_file` (**true**) | `getOpenedFileFromIDE` | |

### Memory (2)

| ID | Tier | What it does | Trigger | Settings | Source function | Text template |
|---|---|---|---|---|---|---|
| `nested_memory` | Core | Injects nested `CLAUDE.md` / memory-file contents found via `@`-mention traversal | User `@`-mentioned paths triggered nested-memory traversal with hits | `nested_memory` (**true**) | `getNestedMemoryAttachments` | `case 'nested_memory':` |
| `relevant_memories` | Core | Surfaces semantically-ranked memory files for the user's prompt (async prefetched) | Prefetch returned ranked memory entries; each uses its stored header for prompt-cache stability | `relevant_memories` (**true**) | `getRelevantMemoryAttachments` + | `case 'relevant_memories':` |

### Emitted elsewhere in coco-rs — not in this crate

Attachment types that cross the reminder boundary but are emitted by different coco-rs subsystems:

| ID | Tier | What it does | Trigger | Location |
|---|---|---|---|---|
| `plan_file_reference` | Core | Re-injects the plan-file contents post-compaction so the plan survives the context bust | Compaction runs AND plan file exists | `case 'plan_file_reference':` |
| `compact_file_reference` | Core | Re-injects recently-read file references after compaction without inlining large content | Compaction runs AND read-file state contains paths not preserved in the compacted tail | `case 'compact_file_reference':` |
| `edited_text_file` | Core | Warns that a previously-read file changed on disk and includes a diff snippet | A cached read-file path has a newer mtime and a text diff | `case 'edited_text_file':`; coco-rs currently owns this in `core/context::changed_files` + `app/cli::changed_file_to_message`, not this crate |
| `file` / `directory` / `pdf_reference` | User | Full user-input file context for `@path` mentions | `processAtMentionedFiles()` resolves a file, directory, or large PDF reference | cases `file`, `directory`, `pdf_reference` |

## Attachment coverage index

Completeness reference for all `AttachmentKind` variants, including UI-only and not-yet-implemented types.

| Attachment type | Emit location(s) | Status |
|---|---|---|
| `file` | `processAtMentionedFiles` () -> `generateFileAttachment` (, ) | Outside this crate; represented by `at_mentioned_files` grouping. |
| `compact_file_reference` | ; `generateFileAttachment` compact branch () | Outside this crate; post-compact file reference. |
| `pdf_reference` | `tryGetPDFReference` () | Outside this crate; represented by `at_mentioned_files` grouping. |
| `already_read_file` | `generateFileAttachment` () | Ported as silent/display-only. |
| `agent_mention` | `processAgentMentions` () | Ported as `agent_mentions`. |
| `async_hook_response` | `getAsyncHookResponseAttachments` () | Ported. |
| `hook_blocking_error` | ; , | Ported. |
| `hook_stopped_continuation` | , , ; ; | Ported. |
| `hook_additional_context` | ; , ; ; , , | Ported. |
| `hook_permission_decision` | | Silent / UI-only. |
| `hook_system_message` | | Silent / UI-only. |
| `hook_cancelled` | — | Silent / UI-only. |
| `hook_error_during_execution` | — | Silent / UI-only. |
| `hook_success` | sync hook executor | Ported (SessionStart / UserPromptSubmit events only). |
| `hook_non_blocking_error` | — | Silent / UI-only. |
| `edited_text_file` | `getChangedFiles` () | Outside this crate; coco-rs handles changed-file context elsewhere. |
| `edited_image_file` | `getChangedFiles` () | Ported as silent/display-only. |
| `directory` | `processAtMentionedFiles` () | Outside this crate; represented by `at_mentioned_files` grouping. |
| `selected_lines_in_ide` | `getSelectedLinesFromIDE` () | Ported as `ide_selection`. |
| `opened_file_in_ide` | `getOpenedFileFromIDE` () | Ported as `ide_opened_file`. |
| `todo_reminder` | `getTodoReminderAttachments` () | Ported. |
| `task_reminder` | `getTaskReminderAttachments` () | Ported. |
| `nested_memory` | `memoryFilesToAttachments` () | Ported. |
| `relevant_memories` | `startRelevantMemoryPrefetch` / `getRelevantMemoryAttachments` (); consumed in | Ported. |
| `dynamic_skill` | `getDynamicSkillAttachments` () | Silent / UI-only; skills load separately. |
| `skill_listing` | `getSkillListingAttachments` () | Ported. |
| `skill_discovery` | `getTurnZeroSkillDiscovery` via ; inter-turn prefetch from | Feature-gated; not ported until matching skill search exists. |
| `queued_command` | `getQueuedCommandAttachments` (); `getAgentPendingMessageAttachments` () | Ported; `agent_pending_messages` maps to this type. |
| `output_style` | `getOutputStyleAttachment` () | Ported. |
| `diagnostics` | `getDiagnosticAttachments` (); `getLSPDiagnosticAttachments` () | Ported. |
| `plan_mode` | `getPlanModeAttachments` (); post-compact | Ported. |
| `plan_mode_reentry` | `getPlanModeAttachments` () | Ported. |
| `plan_mode_exit` | `getPlanModeExitAttachment` () | Ported. |
| `auto_mode` | `getAutoModeAttachments` () | Ported. |
| `auto_mode_exit` | `getAutoModeExitAttachment` () | Ported. |
| `critical_system_reminder` | `getCriticalSystemReminderAttachment` () | Ported. |
| `plan_file_reference` | | Outside this crate; post-compact plan reference. |
| `mcp_resource` | `processMcpResourceAttachments` () | Ported as `mcp_resources`. |
| `command_permissions` | slash-command meta sink | Silent / UI-only. |
| `task_status` | source type; `getUnifiedTaskAttachments` (); post-compact | Ported. |
| `token_usage` | `getTokenUsageAttachment` () | Ported, default off. |
| `budget_usd` | `getMaxBudgetUsdAttachment` () | Ported. |
| `output_token_usage` | `getOutputTokenUsageAttachment` () | Ported, default off. |
| `structured_output` | | Silent / UI-only. |
| `teammate_mailbox` | `getTeammateMailboxAttachments` () | Ported. |
| `team_context` | `getTeamContextAttachment` () | Ported. |
| `invoked_skills` | | Ported. |
| `verify_plan_reminder` | `getVerifyPlanReminderAttachment` () | Ported; emits only when `VerifyPlanExecution` is visible. |
| `max_turns_reached` | , | Runtime/UI bookkeeping; not a model reminder. |
| `current_session_memory` | Type only (no emitter) | Runtime/UI bookkeeping; no emitter. |
| `teammate_shutdown_batch` | | UI/transcript collapse marker. |
| `compaction_reminder` | `getCompactionReminderAttachment` () | Ported. |
| `context_efficiency` | `getContextEfficiencyAttachment` () | `HISTORY_SNIP` feature-gated; not ported until snip runtime exists. |
| `date_change` | `getDateChangeAttachments` () | Ported. |
| `ultrathink_effort` | `getUltrathinkEffortAttachment` () | Ported, default off. |
| `deferred_tools_delta` | `getDeferredToolsDeltaAttachment` (); also re-announced by compact | Ported. |
| `agent_listing_delta` | `getAgentListingDeltaAttachment` (); also re-announced by compact | Ported. |
| `mcp_instructions_delta` | `getMcpInstructionsDeltaAttachment` (); also re-announced by compact | Ported. |
| `companion_intro` | ; gated from | Ported, default off. |
| `bagel_console` | Type only (no emitter) | UI/runtime placeholder; no model reminder found. |

## Intentionally not ported

| ID | Why |
|---|---|
| `context_efficiency` (HISTORY_SNIP) | History snip nudge — port only if coco-rs ships the matching snip runtime/tool. |
| `skill_discovery` | Ported as a model-visible reminder. Controlled by `system_reminder.attachments.skill_discovery`. |
| `security_guidelines` | Not implemented; no model-visible reminder needed. |
| `hook_cancelled` / `hook_error_during_execution` / `hook_non_blocking_error` / `hook_permission_decision` / `hook_system_message` / `structured_output` / `dynamic_skill` / `bagel_console` / `command_permissions` | `normalizeAttachmentForAPI` returns `[]` — API-hidden typed events, produce zero API text. |
| `max_turns_reached` / `current_session_memory` / `teammate_shutdown_batch` | Runtime/UI bookkeeping only; `normalizeAttachmentForAPI()` has no API-text case for these types. |
| `image` | Content block subtype, not a reminder attachment. |



## Execution semantics

This section covers reminder execution mechanics, not new reminder types.

| Concern | Implementation details |
|---|---|
| Entry point | `QueryEngine` creates one session-scoped `SystemReminderOrchestrator` (`app/query/src/engine.rs:699`, `app/query/src/engine.rs:701`), builds `TurnReminderInput`, calls `run_turn_reminders()`, then appends the results with `inject_reminders()` (`app/query/src/engine.rs:1213`, `app/query/src/engine.rs:1214`). |
| Disabled / simple mode | The Rust crate has a master `system_reminder.enabled` switch. When false, `SystemReminderOrchestrator::generate_all()` returns no reminders (`core/system-reminder/src/orchestrator.rs:227`-`230`). It does not have the env-var special case that preserves queued commands. |
| Batch ordering | Rust parses the latest user input before source materialization, passes mentioned paths into `MemorySource`, materializes cross-crate sources, then registers generators in the same flatten order. `join_all` preserves that applicable-generator order (`core/system-reminder/src/orchestrator.rs`), and `default_registry_order_matches_ts_attachment_batches` locks it. |
| Concurrency | Rust has two parallel stages: `ReminderSources::materialize()` fans out source calls with `tokio::join!` (`core/system-reminder/src/sources/mod.rs:281`-`311`), then `SystemReminderOrchestrator::generate_all()` runs applicable generators concurrently with `future::join_all()` (`core/system-reminder/src/orchestrator.rs:266`-`272`). |
| Timeout | Rust uses hard `tokio::time::timeout` wrappers. Sources use `SystemReminderConfig.timeout_ms` as `per_source_timeout` (`app/query/src/engine.rs:1020`-`1038`) and `ReminderSources::gate()` returns defaults on timeout (`core/system-reminder/src/sources/mod.rs:55`-`68`, `core/system-reminder/src/sources/mod.rs:322`-`343`). Generators are also wrapped individually (`core/system-reminder/src/orchestrator.rs:336`-`356`). Default is 1000 ms (`common/config/src/system_reminder.rs:56`). |
| Error handling | Source timeouts return default values. Generator errors and timeouts are logged and become `None` (`core/system-reminder/src/orchestrator.rs:345`-`356`). A failed generator does not block other generators. |
| Throttle / full-content state | Rust pre-computes full/sparse decisions before running generators (`core/system-reminder/src/orchestrator.rs:233`-`243`), filters by config/tier/throttle (`core/system-reminder/src/orchestrator.rs:246`-`252`, `core/system-reminder/src/orchestrator.rs:288`-`319`), and only marks throttle state after a generator actually returns a reminder (`core/system-reminder/src/orchestrator.rs:274`-`277`). |
| Multiple reminders and API-message merging | Rust injects each simple text reminder as a separate `Message::Attachment` (`core/system-reminder/src/inject.rs:149`-`167`). The prompt normalizer extracts attachment messages and merges consecutive same-role `LlmMessage::User` entries (`core/messages/src/normalize.rs:85`-`94`, `core/messages/src/normalize.rs:107`-`125`). So multiple reminder messages can collapse into one API user message, but the history still stores separate reminder messages. |
| Tool-result sibling folding | The current Rust normalizer only does role-level merge (`core/messages/src/normalize.rs:123`-`156`). It does not implement `tengu_chair_sermon` tool-result sibling folding. |
| Post-emit bookkeeping | Rust does explicit post-emit bookkeeping in `QueryEngine`: clear one-shot plan/auto flags and update last-announced tool/agent/MCP baselines only for fired reminder types (`app/query/src/engine.rs:1198`-`1208`). |


## Cadence constants

| Preset | `min_turns_between` | `full_content_every_n` |
|---|---|---|
| plan-mode | 5 | 5 |
| auto-mode | 5 | 5 |
| todo / task reminder | 10 | — |
| verify-plan | 10 | — |
| one-shots (exit banners / critical / compaction / date-change / delta reminders) | 0 | — |

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
 "ultrathink_effort": false, // opt-in
 "token_usage": false, // opt-in
 "budget_usd": true,
 "output_token_usage": false, // opt-in
 "companion_intro": false, // opt-in
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

Feature-gated reminders default **off** (not active unless explicitly enabled in settings).

