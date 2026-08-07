# coco-system-reminder

Per-turn dynamic `<system-reminder>` injection. Owns the reminder subsystem: types, generators, orchestration, cross-crate source traits, and message injection.

## Key Types

- `AttachmentType` — one variant per reminder; full catalog is `AttachmentType::all()`.
  Conceptually grouped by phase: core plan/auto/todo/task reminders; engine-local
  (token/budget/companion/goal-context); **world-state deltas** (deferred tools /
  agent listing / MCP servers + instructions / model switch — see below);
  cross-crate snapshots (hooks, diagnostics,
  skills, team, queued commands, …); user-input tier (@-mentions, MCP resources,
  agent mentions); IDE; silent-native (`already_read_file` / `edited_image_file`).
- `ReminderTier` — `Core` (all agents) / `MainAgentOnly` / `UserPrompt` (only when user input is present).
- `SystemReminder` — `{ attachment_type, output, is_meta, is_silent }`;
  `ReminderOutput::{Text | Messages | SkillDiscovery | ModelAttachment | SilentAttachment}`.
  Consumers check both `reminder.is_silent || reminder.output.is_silent()`.
- `AttachmentGenerator` trait — one impl per reminder type under `generators/`.
  **3-hook lifecycle**: `is_enabled(config)`, `tier()` (defaults to
  `AttachmentType::tier()`), `generate(ctx)`. Errors bubble to the orchestrator,
  which logs and continues — one failure never poisons another's output.
- `GeneratorContext<'a>` (+ builder) — per-turn state precomputed by the engine:
  mode flags, tools, todos/plan tasks, context-window metrics, delta + cross-crate
  snapshots, and history-derived cadence counters.
- `SystemReminderOrchestrator` — session-owned generator registry
  (`with_default_generators()` wires built-ins in injection order: user-input,
  all-thread, main-thread batch). Gate chain: config `enabled` → generator
  `is_enabled` → tier; survivors run concurrently under a batch timeout (2x
  per-generator timeout as safety net).
- `TurnReminderInput` + `run_turn_reminders()` — one-call engine entry point.
- `ReminderSources` (`sources/`) — per-subsystem source traits (`HookEventsSource`,
  `DiagnosticsSource`, `TaskStatusSource`, `SkillsSource`, `McpSource`, `SwarmSource`,
  `IdeBridgeSource`, `MemorySource`), implemented by the owning crates — the
  reminder analog of `core/tool-runtime`'s handle pattern (one-way edge, no
  cycles). `materialize()` fans out via `tokio::join!` with per-source timeout,
  degrading errors/timeouts to defaults.
- `QueueOrigin` + `wrap_command_text` (`queue_origin`) — typed origin for mid-turn
  queued commands (`Coordinator` / `TaskNotification` / `Channel{server}` / `Human`
  / `Cron`); each origin gets its own framing prose.
- `InjectedMessage` / `inject_reminders` — converts orchestrator output into `coco_types::Message::Attachment`.
- `SystemReminderConfig` / `AttachmentSettings` — **live in `coco-config`** (re-exported here); every reminder toggleable via `Settings.system_reminder`.

## World state vs. cadence

Two different questions, two different mechanisms — do not merge them.

- *"A session fact the model should know until it changes"* → **world state**.
  Diffed in `app/query::world_state` against `coco_types::WorldStateSnapshot`,
  the per-agent-scope record of what this model has already been told. That
  snapshot is **persisted** into the session transcript
  (`coco_session::MetadataEntry::WorldStateSnapshot`) and restored at session
  build, so a resumed session does not re-announce an inventory the reloaded
  history already contains. Generators here (`deferred_tools_delta`,
  `agent_listing_delta`, `mcp_servers_delta`, `mcp_instructions_delta`,
  `model_switch`) only *render* the diff.
- *"A prompt we re-issue on a schedule"* → a cadenced generator, below.

The baseline advances only for sections whose reminder actually reached
history (`WorldStateDelta::adopt_fired`), so a generator that is disabled,
times out, or errors retries next turn instead of losing its announcement.
`model_switch` is the one exception and adopts unconditionally: the mismatch
it reports never resolves on its own, so a suppressed notice would otherwise
re-fire every turn for the rest of the session.

## Cadence lives in generators (no orchestrator throttle)

The former `ThrottleManager` / `ThrottleConfig` are gone — the orchestrator
holds **no** throttle state. Throttled generators (plan/auto steady-state,
todo/task/verify nudges, Full-vs-Sparse cycling) derive their gate inside
`generate()` from history-scan counters the engine precomputes onto
`GeneratorContext` (`plan_mode_turns_since_attachment`,
`turns_since_last_todo_write`, …) via the `turn_counting` helpers; `Ok(None)`
skips the turn. History-derived cadence survives session restarts with no
seeded state. Plan/auto cadence counts *human* turns (non-meta user messages),
not LLM iterations — `last_human_turn_uuid` on `GeneratorContext` is scanned
from history so multi-tool-round iterations within one human turn count once
(`turn_counting::human_turns_since_attachment_opt` and friends).

## Module Layout

`error` (codes 13_xxx) · `types` (AttachmentType/ReminderTier/SystemReminder/ReminderOutput) · `xml` (wrap/extract) · `generator` (trait + context/builder + snapshot structs) · `orchestrator` · `inject` · `context_builder` (app_state → ctx helpers) · `turn_counting` · `turn_runner` (engine entry) · `queue_origin` · `sources/` (traits + materialized + noop) · `generators/` (the generator impls; related reminders share a file, e.g. `hook_events.rs`).

## Key Invariants

- **is_meta=true on all reminders**: hidden from UI transcripts, sent to the API wrapped in `<system-reminder>` with `origin=SystemInjected`.
- **Timeout**: `SystemReminderConfig::timeout_ms` (default 1000ms). Timed-out generators produce zero reminders; the turn continues.
- **Typed `ToolName` throughout**: no hand-written tool-name strings in gates or cadence helpers — `TASK_MANAGEMENT_TOOLS` + `count_assistant_turns_since_tool(ToolName::X)` thread typed references so a rename propagates.
- **Generators only render**: cross-crate data arrives as typed `*Snapshot` / `*Info` structs on `GeneratorContext`, populated through the `sources/` traits — generators never call into sibling crates.

## What this crate does NOT own

- **File / image / PDF / memory attachments** — those stay in `core/context::Attachment` (user-input-side, token-budgeted + deduped). The `at_mentioned_files` generator emits a *reminder* (listing display paths); the **file content** still flows through `core/context`.
- **Static system prompt assembly** — that's `core/context::build_system_prompt`. This crate handles *dynamic per-turn* injection only.
- **Compaction summarization** — `services/compact` owns that. `plan_file_reference` is emitted **by** the compaction pipeline (not this crate) so it survives the context-bust.
- **Cross-crate data sources for the snapshots** — each owning crate (`services/lsp`, `hooks`, `tasks`, `skills`, `coordinator`, `app/query::CommandQueue`, `bridge`, `memory`) implements its `*Source` trait and populates `GeneratorContext`.
