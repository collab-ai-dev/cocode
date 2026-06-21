# coco-system-reminder

Per-turn dynamic `<system-reminder>` injection. Owns the entire reminder subsystem: types, generators, throttle, orchestration, and message injection.


## Key Types

- `AttachmentType` — generator variants mapped to discriminators or coco-rs
  synthetic grouping keys. Grouped by port phase:
  - **Phase A/B/C (11)**: plan-mode trio (`plan_mode` / `plan_mode_exit` / `plan_mode_reentry`), auto-mode pair (`auto_mode` / `auto_mode_exit`), todo/task pair (`todo_reminder` / `task_reminder`), `critical_system_reminder`, `compaction_reminder`, `date_change`, `verify_plan_reminder`.
  - **Phase 1 engine-local (5)**: `ultrathink_effort` / `token_usage` / `budget_usd` / `output_token_usage` / `companion_intro`.
  - **Phase 2 history-diff (3)**: `deferred_tools_delta` / `agent_listing_delta` / `mcp_instructions_delta`.
  - **Phase 3 cross-crate (14)**: `hook_success` / `hook_blocking_error` / `hook_additional_context` / `hook_stopped_continuation` / `async_hook_response` / `diagnostics` / `output_style` / `queued_command` / `task_status` / `skill_listing` / `invoked_skills` / `teammate_mailbox` / `team_context` / `agent_pending_messages`.
  - **Phase 4 user-input (3, UserPrompt tier)**: `at_mentioned_files` / `mcp_resources` / `agent_mentions`.
  - **Main-thread IDE (2)**: `ide_selection` / `ide_opened_file`.
  - **Silent native (2)**: `already_read_file` / `edited_image_file`.
  - **Audit note (May 2026)**: only `skill_discovery` is a model-visible
    generator. API-hidden events (`command_permissions`, `dynamic_skill`,
    `structured_output`, `max_turns_reached`, `teammate_shutdown_batch`) are
    emitted by their owning crates through typed `AttachmentMessage` history
    events. `current_session_memory` is consumed through the memory/compact
    provider path, not a reminder generator.
  - `AttachmentType::all()` returns the full catalog.
- `ReminderTier` — `Core` (all agents / all-thread batch), `MainAgentOnly` (main-thread batch), `UserPrompt` (only when user input is present). Maps to the three attachment batches in `getAttachments`.
- `XmlTag` — `SystemReminder` = `<system-reminder>`, `None` = raw.
- `SystemReminder` — unified generator output: `{ attachment_type, output: ReminderOutput, is_meta, is_silent }`.
- `ReminderOutput::{ Text | Messages | ModelAttachment | SilentAttachment }` — model-visible shapes plus silent/display-only metadata.
- `AttachmentGenerator` trait (`async_trait`) — one impl per reminder type. 4-hook lifecycle: `is_enabled`, `tier`, `throttle_config_for_context`, `generate`.
- `GeneratorContext<'a>` — per-turn state: permission-mode flags, tool list, turn-since-* counters, todos/plan_tasks, context-window metrics, date-change + verify-plan signals, full-content flags pre-computed by the orchestrator.
- `ThrottleManager` / `ThrottleConfig` — central rate limiter keyed by `AttachmentType`. Fields: `min_turns_between` = `TURNS_BETWEEN_*`, `full_content_every_n` = `FULL_REMINDER_EVERY_N_*`. Presets: `plan_mode` / `auto_mode` / `todo_reminder` / `verify_plan_reminder` / `none`.
- `SystemReminderOrchestrator` — parallel execution with per-generator timeout. Default registration order: user-input batch, all-thread batch, then main-thread batch.
- `TurnReminderInput` + `run_turn_reminders()` — one-call engine entry point; packages every per-turn input as named struct fields.
- `InjectedMessage` / `InjectedBlock` — post-orchestration conversion; `inject_reminders` writes `coco_types::Message::Attachment` with `is_meta=true` + `origin=SystemInjected`.
- `SystemReminderConfig` / `AttachmentSettings` — **live in `coco-config`** (re-exported here). Wired via `Settings.system_reminder` so every reminder can be toggled from `settings.json`.

## Module Layout

```
src/
├── error.rs          SystemReminderError + ErrorExt (codes 13_xxx)
├── types.rs          AttachmentType, ReminderTier, XmlTag, SystemReminder, ReminderOutput
├── xml.rs            wrap_with_tag / extract_system_reminder
├── throttle.rs       ThrottleConfig (+ presets) + ThrottleManager
├── generator.rs      AttachmentGenerator trait + GeneratorContext(Builder)
├── orchestrator.rs   SystemReminderOrchestrator (parallel + timeout)
├── inject.rs         InjectedMessage -> coco_types::Message
├── context_builder.rs app_state → GeneratorContext mapping helpers
├── turn_counting.rs  count_assistant_turns_since_tool/any_tool, count_human_turns
├── turn_runner.rs    TurnReminderInput + run_turn_reminders (engine entry)
├── generators/       plan/auto modes, todo/task reminders, hook events,
│                     deltas, memory, team/swarm, user input, IDE,
│                     token/budget, silent markers
└── lib.rs            module declarations + re-exports (SystemReminderConfig from coco-config)
```

## Key Invariants

- **Logic is canonical**: cadence, text content, and trigger conditions match the original claude-code behavior. Cocode-rs provides the crate shape only.
- **Human-turn UUID throttle**: plan-mode cadence counts non-meta user messages, not LLM iterations (TS `getPlanModeAttachmentTurnCount`). The engine tracks `last_human_turn_uuid_seen` on `ToolAppState` and advances the throttle counter only on a new UUID; `PlanModeReminder::turn_start_side_effects_only` writes it.
- **is_meta=true on all reminders**: hidden from UI transcripts, sent to the API wrapped in `<system-reminder>`.
- **Per-generator timeout**: `SystemReminderConfig::timeout_ms` (default 1000ms). Timed-out generators produce zero reminders; the turn continues.
- **Typed `ToolName` throughout**: no hand-written tool-name strings in gates or cadence helpers. `TASK_MANAGEMENT_TOOLS` + `count_assistant_turns_since_tool(ToolName::X)` thread typed references through so a `ToolName` rename propagates automatically.
- **Auto-mode gate**: `is_auto_mode == (mode == Auto) || (mode == Plan && is_auto_classifier_active)`. The engine reads `AutoModeState::is_active()` from `core/permissions` and threads it into `TurnReminderInput`.
- **Cross-run cadence**: each `run_session_loop` invocation constructs a fresh orchestrator. Engine seeds `ThrottleManager::seed_state` from `app_state.plan_mode_attachment_count` + `turns_since_last_attachment` so cadence survives. Post-emit bookkeeping mirrors the throttle back onto `app_state`.

## What this crate does NOT own

- **File / image / PDF / memory attachments** — those stay in `core/context::Attachment` (user-input-side, token-budgeted + deduped). Phase 4's `at_mentioned_files` generator emits a *reminder* (listing display paths); the **file content** still flows through `core/context`.
- **Static system prompt assembly** — that's `core/context::build_system_prompt`. This crate handles *dynamic per-turn* injection only.
- **Compaction summarization** — `services/compact` owns that. `plan_file_reference` is emitted **by** the compaction pipeline (not this crate) so it survives the context-bust.
- **Cross-crate data sources for Phase 3/4 snapshots** — each owning crate (`services/lsp`, `hooks`, `tasks`, `skills`, `app/state/swarm`, `app/query::CommandQueue`, `bridge`) populates a typed `*Snapshot` / `*Info` struct on `GeneratorContext`. The generators in `src/generators/` only *render* — they never call into a sibling crate.
