# Plan Audit: Comprehensive Gap Analysis

Exhaustive comparison of all plan docs against actual TS source + cocode-rs source.

## Freshness Update (May 14, 2026)

This file is a historical audit log. For current fix ordering, use
[current-gap-fix-plan.md](current-gap-fix-plan.md). A May 14 cleanup pass
checked the older open rows against live `coco-rs/` code and marked several
previous gaps as resolved or partial:

- `PermissionChecker` now has production preflight callers in Read, Write, and
  Edit.
- `RequiresAction` and `permission_denials` are wired through `app/query` and
  SDK session results.
- Skills/commands/plugins rows from Round 11 are no longer fully current:
  CommandRegistry seam, bundled-skill extraction, skill feature gates,
  `/rewind`, `/init`, `/commit-push-pr`, dependency resolver, plugin security,
  builtin plugin registry, and MCPB loader all have current implementations or
  partial implementations.

## Round 13: Reminder Signal Wiring + Sandbox Wrapper Stripping (May 7, 2026)

Follow-up audit pass after [Round 12](#round-12-system-reminder--sandbox-alignment-may-7-2026): caught
two architectural gaps the first pass missed. Reminder generators added in
Round 12 emit only when their `GeneratorContext` field is populated; the
field surface was added on the builder but the engine-side
`TurnReminderInput` never carried them. Sandbox `excluded_commands` matching
covered env-var prefixes and basename normalisation but not the safe
wrapper commands (`timeout`, `time`, `nice`, `nohup`) the TS
`shouldUseSandbox` fixed-point loop strips.

### Gaps Resolved in Round 13

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **Reminder signal wiring** (`max_turns_reached`, `current_session_memory`, `command_permissions`, `dynamic_skill`, `skill_discovery`, `structured_output`, `teammate_shutdown_batch`, `context_efficiency`) | system-reminder | All 8 fields added to `TurnReminderInput`, threaded through `run_turn_reminders` builder, and exercised by two new turn-runner tests. `engine_turn_reminders.rs` populates `max_turns_reached_signal` from `QueryEngineConfig::max_turns` (TS `query.ts:1508` parity: `turnCount + 1 > maxTurns`); `context_efficiency_signal` stays `false` (TS gates on `feature('HISTORY_SNIP')` which root CLAUDE.md explicitly does NOT port). The remaining six are TS event-time emissions (slash command, tool execution, swarm) — engine threads `None` with a comment so future subsystem ports populate the field with no engine change. | **RESOLVED (wiring)** / **DEFERRED (six event-time sources)** |
| **`excluded_commands` wrapper stripping** | sandbox config | `build_command_variants` BFS now also peels safe wrappers via `strip_safe_wrapper`. Handles `timeout` (with GNU flag prefixes + duration token validation), `time`, `nice` (`-n N` and `-N`), and `nohup` — including the optional `--` separator (`nohup -- cmd`). The strip is intentionally conservative: only enabled wrappers + a parsed-duration check for `timeout` so `timeout bazel build` (missing duration) leaves `bazel build` un-promoted. Three new unit tests cover the chained `timeout 300 FOO=bar /usr/bin/bazel run` case from the TS comment. TS parity: `bashPermissions.ts:524` (`stripSafeWrappers`) + `shouldUseSandbox.ts:82-101` (BFS fixed-point). | **RESOLVED** |

### Items Documented (deferred — needs cross-crate work, not adapter-local)

| Item | Note |
|------|------|
| Six event-time reminder emitters | `current_session_memory` (TS no creator yet), `command_permissions` (TS `processSlashCommand.tsx:909` from slash-command flow), `dynamic_skill` (TS `attachments.ts:2589` from skill loader), `skill_discovery` (TS no creator yet), `structured_output` (TS `services/tools/toolExecution.ts:1276` from tool execution), `teammate_shutdown_batch` (TS `collapseTeammateShutdowns.ts:43` from swarm coordinator). The reminder generator-side wiring is in place (`SystemReminder::silent_text` + builder + context); the upstream owners (`coco-skills`, `coco-permissions`, `services/tools`, swarm) need to populate the field on `TurnReminderInput`. No engine change needed for the consumer — only producer wiring. |
| Managed-policy-only sandbox filters (`allow_managed_domains_only` / `allow_managed_read_paths_only`) | TS `sandbox-adapter.ts:152-164,343-347` filters network domains + read paths to policy-source rules only when these flags are set. coco-rs has the per-source settings infrastructure (`SettingSource::Policy`, `load_policy_settings`) but `AdapterInputs` carries flat permission rule lists with no source distinction, so the gate would require a multi-source rule plumbing refactor. Deferred until enterprise-policy work lands. |
| Sandbox hot-reload subscriber | `RuntimeReloader` (`coco-config-reload`) publishes new `Arc<RuntimeConfig>` snapshots; `SandboxState::update_config` accepts hot-reloaded config. The wiring in between (subscribe to `RuntimePublisher`, re-run `adapter::build_runtime_config`, call `update_config`) is not installed. `tui_runner.rs:61-64` explicitly notes subscriber wiring is deferred until the QueryEngine integration lands. Same gap applies to the sandbox state. Editing `~/.coco/settings.json` mid-session has no effect on the sandbox config. |

Cleanup note (May 14): the old carried-over `PermissionChecker` row is
resolved. `core/tools/src/tools/sandbox_preflight.rs` is called from Read,
Write, and Edit before filesystem syscalls.

---

## Round 12: System-Reminder + Sandbox Alignment (May 7, 2026)

End-to-end audit of `core/system-reminder` and `exec/sandbox` against
TS source (`utils/attachments.ts`, `messages.ts`,
`utils/sandbox/sandbox-adapter.ts`, `entrypoints/sandboxTypes.ts`,
`tools/BashTool/shouldUseSandbox.ts`) plus codex-rs reference (linux-sandbox + sandboxing).

### Gaps Resolved in Round 12

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **VerifyPlanReminder turn-0 cadence** | system-reminder | Gate changed from `n <= 0 \|\| n % 10 != 0` to `n < 0 \|\| n % 10 != 0`; matches TS `attachments.ts:3919-3922` (the only skip is `turnCount % 10 !== 0`, so turn 0 fires). Test renamed `skips_when_turn_count_zero` → `fires_on_turn_count_zero`. | **RESOLVED** |
| **Per-batch reminder timeout** | system-reminder | `orchestrator::generate_all` now wraps `join_all` in a single `tokio::time::timeout(batch_timeout, ...)` matching TS's single shared `AbortController`. Per-generator hard-cap kept at 2× as a safety net. | **RESOLVED** |
| **8 audit-add reminder variants** | system-reminder | Added `MaxTurnsReached` / `CurrentSessionMemory` / `CommandPermissions` / `DynamicSkill` / `SkillDiscovery` / `StructuredOutput` / `TeammateShutdownBatch` / `ContextEfficiency` as `Coverage::SilentReminder` generators in `generators/audit_add.rs`. All emit through `SystemReminder::silent_text` (TS-parity: `is_api_visible=false`, body lands in UI/transcript only). | **RESOLVED** |
| **`fail_if_unavailable` hard-error** | sandbox bootstrap | Field added to `SandboxSettings` + `PartialSandboxSettings` + `SandboxConfig`. New env var `COCO_SANDBOX_FAIL_IF_UNAVAILABLE`. `build_sandbox_state` now returns `Result<Option<…>>` and propagates a fatal error when the gate is set. TS parity: `entrypoints/sandboxTypes.ts:95`. | **RESOLVED** |
| **`sandbox_unavailable_reason` UX banner** | sandbox bootstrap | `current_platform_supported()` exposed via `lib.rs`. `build_sandbox_state` calls `sandbox_unavailable_reason(...)` after gate failure and prints a stderr banner so the user understands why sandboxing is degraded. TS parity: `getSandboxUnavailableReason()` (#34044). | **RESOLVED** |
| **`scrub_bare_repo_files` post-command** | shell executor | `ShellExecutor` now stores `original_cwd`. Both `execute()` and `execute_with_progress()` call `scrub_bare_repo_after_command(...)` on every exit path (success / cancel / timeout / IO error). Closes [anthropics/claude-code#29316](https://github.com/anthropics/claude-code/issues/29316). TS parity: `cleanupAfterCommand()` calling `scrubBareGitRepoFiles()`. | **RESOLVED** |
| **Deny-read glob expansion** | sandbox | New `glob_expansion` module using `globset` + `walkdir`, bounded by `glob_scan_max_depth` (default 3). Adapter classifies `filesystem.deny_read` entries with metacharacters into `denied_read_globs`. macOS Seatbelt + Linux bwrap both expand at wrap time. Matches codex-rs `glob_scan_max_depth`. | **RESOLVED** |
| **CLAUDE.md generator-count drift** | docs | `core/system-reminder/CLAUDE.md` updated to enumerate all 8 audit-add variants and reflect the new total (50 registered generators, 60 `AttachmentKind` variants total). | **RESOLVED** |

### Items Documented (deferred — wiring exists but no consumer)

| Item | Note |
|------|------|
| Windows inner-stage sandbox | `platform/windows.rs` serializes config to base64 and builds the arg0-dispatch command, but the inner Win32 token-restriction stage is stubbed (matches codex-rs/windows-sandbox-rs). Defer per scope. |

Cleanup note (May 14): `PermissionChecker` runtime wiring is no longer a
deferred Round 12 item. Read, Write, and Edit now call
`sandbox_preflight::preflight_path`.

---

## Round 11: Skills / Commands / Plugins Parity Deep-Review (May 2, 2026)

Verified TS-vs-Rust audit across `coco-rs/skills`, `coco-rs/commands`,
`coco-rs/plugins` against TS `skills`, `commands`, `plugins`, and
`utils/{plugins,skills}`. TS file paths are relative to the TS project's `src/`
directory. **Full plan**:
[parity-skills-commands-plugins.md](parity-skills-commands-plugins.md) — every
gap below has TS file:line citations, type definitions, behavior, and UI specs.

### Summary

| Area | TS LOC | Rust LOC | Parity | Status |
|---|---|---|---|---|
| Skills | ~1.6K | historical | partial | extraction, inventory, and feature gates now landed; watcher/path activation and MCP-sourced skills remain |
| Commands | ~10K (96 cmds) | historical | partial | P1 handlers largely landed; Prompt file parts and command tail remain |
| Plugins | ~22K | historical | partial | dependency resolver/security/MCPB/builtins now exist; live refresh/install parity remains |

### P1 Gaps (Round A — unblocks user-visible flows)

| Gap | TS source | Rust state | Plan section |
|---|---|---|---|
| **CommandRegistry seam** (skills+plugins→commands) | `commands.ts` | **RESOLVED**: `build_command_registry` bridges skills/plugins; collision-order audit remains | §0 |
| **Bundled-skill `files` extraction + nonce dir** | `skills/bundledSkills.ts:53-220` | **RESOLVED**: `skills/src/extraction.rs` + `SkillDefinition.files/skill_root` | §1.1 |
| **Lazy `getPromptForCommand(args, ctx)`** | `skills/bundled/*.ts` | **PARTIAL**: `prompt_render.rs` handles invocation-time args/shell; Prompt file parts remain | §1.2 |
| **Bundled inventory drift** (3 extras, 4 missing, 3 ungated) | `skills/bundled/index.ts:24-78` | **RESOLVED**: Rust-only extras removed and gated skills added | §1.3 |
| **Skill `is_enabled` feature gate** | `types/command.ts` (per-command callback) | **RESOLVED**: `SkillDefinition.gated_by` + `SkillManager::visible(features)` | §1.4 |
| **`/rewind` + fileHistory + message selector** | `commands/rewind/rewind.ts:1-13` + `utils/fileHistory.ts` (~1110 LOC) | **RESOLVED**: handler, file history, TUI flow, SDK rewindFiles exist | §2.1 |
| **`/compact` full flow** (microcompact + session-memory + reactive) | `commands/compact/compact.ts:1-287` | **MOSTLY RESOLVED**: sentinel dispatches to `QueryEngine::run_manual_compact`; display/reactive parity still verify | §2.2 |
| **`/init` 8-phase prompt** | `commands/init.ts:1-256` | **RESOLVED**: prompt handler + prompt files | §2.3 |
| **`/memory` editor dialog** | `commands/memory/memory.tsx:1-89` | **PARTIAL**: dialog entries exist; editor-open flow/string parity still verify | §2.4 |
| **Prompt-type command execution path** | `commands.ts processSlashCommand` | **PARTIAL**: text prompt path wired; `Prompt::File` parts remain | §2.5 |
| **Plugin three-layer refresh** (Layer 2 reconcile + Layer 3 active) | `utils/plugins/{refresh,reconciler}.ts:1-216+265` | **PARTIAL**: `plugins/src/refresh.rs` exists; live SessionRuntime refresh remains | §3.1 |
| **Plugin dependency resolver** (DFS + cycle + scope demote + cross-mkt) | `utils/plugins/dependencyResolver.ts:1-305` | **RESOLVED**: `plugins/src/dependency.rs` full pure-function port | §3.2 |
| **MCPB (.mcpb / .dxt) bundles** | `utils/plugins/mcpbHandler.ts:968` + `zipCache.ts:406` | **PARTIAL**: loader/cache/config status exist; schema/UI/install parity remain | §3.3 |
| **Plugin security validation** (path traversal, impersonation, policy) | `utils/plugins/{validatePlugin,pluginPolicy}.ts:903+20` | **RESOLVED**: `plugins/src/security.rs` + schema validation | §3.4 |
| **Builtin plugin registry** (`{name}@builtin`) | `plugins/builtinPlugins.ts:1-159` | **PARTIAL**: registry exists; Layer-3 ordering and TUI section still verify | §3.5 |

### P2 Gaps (Round B — security + correctness)

| Gap | TS source | Plan section |
|---|---|---|
| Skill watcher: stability threshold, `.git/` ignore, ConfigChange hooks, `--add-dir` | `utils/skills/skillChangeDetector.ts:1-311` | §1.6 |
| Skill `paths` glob conditional activation | `skills/loadSkillsDir.ts:159-178` | §1.5 |
| MCP-sourced skills (write-once builder registry) | `skills/mcpSkillBuilders.ts` | §1.7 |
| `/commit-push-pr` orchestrator | `commands/commit-push-pr.ts:158` | **RESOLVED** (§2.6) |
| `createMovedToPluginCommand` migration helper | `commands/createMovedToPluginCommand.ts:22-65` | §2.7 |
| Headless install (auto-approve + zip-cache + 30s timeout) | `utils/plugins/headlessPluginInstall.ts:174` | §3.6 |
| Hot reload (notify watcher + needsRefresh notification) | `utils/plugins/loadPluginHooks.ts:287` | §3.7 |
| `installed_plugins.json` V1→V2 migration | `utils/plugins/installedPluginsManager.ts:1268` | §3.8 |
| Versioned cache paths `<name>/<version>/` | `utils/plugins/pluginVersioning.ts:157` | §3.8 |
| Official marketplace auto-install | `utils/plugins/officialMarketplaceStartupCheck.ts:439` | §3.8 |
| Contribution conflict warnings | `utils/plugins/loadPluginCommands.ts:946` | §3.8 |
| Plugin error taxonomy (20+ variants) | `types/plugin.ts` | **RESOLVED** (`plugins/src/errors.rs`) |

### P3 Gaps (Round C — parity tail)

- ~40 stub commands across config UI, session UX, diagnostics (`theme`, `color`, `branch`, `tag`, `share`, `env`, `bug-report`, `debug-tool-call`, `ant-trace`, `voice`, `heap-dump`, `ctx-viz`, `mock-limits`, …) — see §2.8.
- Marketplace search / hint recommendation system (`utils/plugins/{marketplaceManager,hintRecommendation}.ts:2643+164`).

### Cross-cutting Adjustments

- **`Command.source` enum**: **RESOLVED**. TS variants (`bundled`, `mcp`,
  `commands_DEPRECATED`, etc.) are represented in `coco-types::CommandSource`.
- **`CommandResult` enum**: **RESOLVED for core shapes**. Rust has compact,
  prompt, open-dialog, skip, and text/injected-prompt paths. Runner support
  for `Prompt::File` parts remains active.
- **Manifest format**: keep `PLUGIN.toml` for Rust-native plugins, but make
  `plugin.json` strict-validate. Current Rust accepts `plugin.json`, but
  unknown-field behavior still needs parity verification.
- **Telemetry**: emit TS `tengu_skill_file_changed`, `plugin_install_*` event names through `coco-otel` for dashboard parity.

---

## Round 9: Phase 1 Event Emission Wiring (April 12, 2026)

Phase 1 of `event-system-design.md` implemented: all Phase 0 type definitions
are now actively emitted by QueryEngine during session execution.

### Gaps Resolved in Round 9

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **SessionStarted emission** | 20_SDK / 15_State | Added `SessionBootstrap` struct in coco-query; QueryEngine emits `CoreEvent::Protocol(SessionStarted(...))` at session start with full init context (cwd, model, permission_mode, tools, version, optional slash_commands/agents/skills/mcp_servers/plugins). Matches TS `buildSystemInitMessage()`. | **RESOLVED** |
| **SessionStateChanged Running/Idle** | 20_SDK | QueryEngine emits `Running` at session entry and `Idle` at session exit (all 3 exit paths via split `run_internal_with_messages` → `run_session_loop`). TS: `notifySessionStateChanged()` in `print.ts`. May 14 cleanup: `RequiresAction` is also now emitted from permission prompt flow. | **RESOLVED** |
| **SessionResult emission** | 20_SDK | QueryEngine emits full `SessionResult(Box<SessionResultParams>)` at session exit with `duration_ms`, `duration_api_ms`, `total_cost_usd`, `usage`, `model_usage` (per-model from `CostTracker.per_model`), `is_error`, `stop_reason`, `result`/`errors`. Matches TS `SDKResultMessage` shape. | **RESOLVED** |
| **Hook lifecycle event wiring** | 11_Hooks / 20_SDK | Extended `orchestration::execute_pre_tool_use` and `execute_post_tool_use` to accept `event_tx: Option<&Sender<HookExecutionEvent>>`. QueryEngine spawns a detached forwarder task that translates `HookExecutionEvent::Started/Progress/Response` into `CoreEvent::Protocol(HookStarted/HookProgress/HookResponse)`. Matches TS `SDKHookStartedMessage/ProgressMessage/ResponseMessage`. | **RESOLVED** |
| **Phase 1 test coverage** | 20_SDK | 5 new `engine.test.rs` tests verifying: bootstrap field passthrough, state transition ordering, SessionResult metadata, result-after-idle emission ordering. | **RESOLVED** |

### Phase 1 Deferred to Phase 2

| Item | Reason |
|------|--------|
| `Task` lifecycle events (task_started/progress/notification) | Requires modifying core `TaskHandle` trait and adding event sink to `TaskManager`; blocked on subagent infrastructure not yet landed. Types are defined in coco-types per TS `SDKTaskStartedMessage/ProgressMessage/NotificationMessage`, just not yet emitted. |
| Streaming hook stdout/stderr | Current `HookExecutionEvent::Progress` emits keep-alive with empty strings; true streaming requires restructuring `execute_hook()` from `wait_with_output()` to incremental `AsyncRead`. Deferred to Phase 3. |

Cleanup note (May 14): `SessionStateChanged::RequiresAction` and
`permission_denials` accumulation are no longer deferred. They are implemented
in `app/query/src/permission_controller.rs` and carried through query / SDK
session results.

### Phase 1 Verification

- 17 coco-query engine tests (+5 new Phase 1 lifecycle tests)
- 55 coco-query lib tests total (15 StreamAccumulator + 17 engine + others)
- 27 coco-query e2e scenarios
- 61 coco-types lib tests
- 117 coco-tui lib tests
- Full `cargo check` workspace passes

---

## Round 8: Phase 0 Event System Implementation (April 12, 2026)

Phase 0 of `event-system-design.md` fully implemented. The event system now
matches the design's 3-layer `CoreEvent` architecture with `StreamAccumulator`
for SDK output.

### Gaps Resolved in Round 8

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **QueryEvent interim type** | 20_SDK / 21_Steering | Deleted `QueryEvent` (13 variants); QueryEngine now emits `CoreEvent` directly. TUI consumes via `handle_core_event()`. Deleted `map_query_event()` mapping layer. | **RESOLVED** |
| **CoreEvent envelope** | 20_SDK | Defined `CoreEvent { Protocol / Stream / Tui }` in coco-types. Consumers pattern-match on the 3 layers. | **RESOLVED** |
| **ServerNotification (52 variants)** | 20_SDK | Moved from coco-tui (17 variants) to coco-types, expanded to 52 (43 base + 9 TS gaps) plus `HookExecuted` from cocode-rs base. Every variant has explicit `#[serde(rename = "wire/method")]`. | **RESOLVED** |
| **AgentStreamEvent (7 variants)** | 20_SDK | Distinct from `coco_types::StreamEvent` (inference layer). Carries TextDelta/ThinkingDelta/ToolUseQueued/Started/Completed/McpToolCallBegin/End with turn-scoped item IDs. | **RESOLVED** |
| **ThreadItem tool mapping** | 20_SDK | `ThreadItemDetails` with 9 variants: CommandExecution (Bash), FileChange (Edit/Write), WebSearch, McpToolCall (mcp__*), Subagent (Agent/Task), ToolCall (others), AgentMessage, Reasoning, Error. | **RESOLVED** |
| **StreamAccumulator state machine** | 20_SDK | Full implementation in `coco-query/src/stream_accumulator.rs` per design §6.1. Handles text↔thinking transitions, tool lifecycle, MCP items. 15 unit tests. | **RESOLVED** |
| **TuiOnlyEvent (20 variants)** | 02_UI | Full list from design §4.1: ApprovalRequired, QuestionAsked, ElicitationRequested, SandboxApprovalRequired, PluginDataReady, OutputStylesReady, RewindCheckpointsReady, DiffStatsReady, CompactionCircuitBreakerOpen, MicroCompactionApplied, SessionMemoryCompactApplied, SpeculativeRolledBack, SessionMemoryExtraction{Started,Completed,Failed}, CronJobDisabled, CronJobsMissed, ToolCallDelta, ToolProgress, ToolExecutionAborted + RewindCompleted (coco-rs extension). Defined in coco-types to avoid cyclic dep. | **RESOLVED** |
| **Tool name in ToolUseCompleted** | 20_SDK | Added `name: String` field to `AgentStreamEvent::ToolUseCompleted` so consumers can reconstruct display without maintaining their own call_id→name map. | **RESOLVED** |

### Phase 0 Verification

- 50 coco-types unit tests (including 13 event tests)
- 50 coco-query unit tests (including 15 StreamAccumulator tests)
- 117 coco-tui unit tests (including snapshot tests)
- 27 e2e scenario tests in coco-query — all passing after refactor

### Known Post-Phase-0 Deviations from Design

| # | Deviation | Rationale |
|---|-----------|-----------|
| 1 | `TuiOnlyEvent` owned by coco-types, not coco-tui | `CoreEvent::Tui(TuiOnlyEvent)` is part of the envelope; moving it to coco-tui creates cyclic deps. Design §1.7 updated in place. |
| 2 | ~~TUI keeps internal `TuiNotification` (17 variants) as state-update type~~ | **RESOLVED** (April 2026 deep review): `TuiNotification` scheduled for deletion. 75% trivial pass-throughs; scaling to 57 variants defeats abstraction. TUI will match `CoreEvent` three layers directly with exhaustive `#[deny(non_exhaustive_omitted_patterns)]`. See `event-system-design.md` §1.7-1.8 and plan WS-2. |
| 3 | `BudgetDecision::Nudge` mapped to `ServerNotification::Error { category: "budget" }` | No direct design equivalent; Error with category field is acceptable. |
| 4 | Inference-layer `coco_types::StreamEvent` coexists with new `AgentStreamEvent` | Two abstraction layers (raw LLM stream vs. agent-loop-processed). Documented in crate-coco-types.md. |

---

## Round 7: Documentation Gap Closure (April 6, 2026)

Comprehensive gap closure across all plan docs. Focused on unmapped TS files, P0 critical gaps, P1 high-priority expansions, and P2 secondary expansions.

### Gaps Resolved in Round 7

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **Unmapped services/api/ files** | ts-to-rust-mapping | Added file-level detail for 20 services/api/ files, 11 services/compact/ files, 9 utils/telemetry/ files | **RESOLVED** |
| **5 open type issues** | Types | MessageRole→MessageKind, OAuthTokens→ApiOAuthTokens/McpOAuthTokens, TaskStateBase unified to coco-types, BuiltinPluginDefinition+SkillDefinition.hooks confirmed Value pattern | **RESOLVED** |
| **Plan mode state machine** | 12_Plan | Added full state machine, permission context, storage path, recovery (3-source), circuit breaker, teammate approval to crate-coco-tools.md | **RESOLVED** |
| **IDE/bridge architecture** | 22_IDE | Created crate-coco-bridge.md: SpawnMode, BridgeState, WorkSecret, permission relay, JWT auth, trusted device, transport layers (v1/v2), session runner | **RESOLVED** |
| **OTel L2 span hierarchy** | 17_Telemetry | Expanded to 6 span types with parent-child: interaction→llm_request, tool (blocked_on_user + execution), hook | **RESOLVED** |
| **OTel L3 event catalog** | 17_Telemetry | Corrected from "~53" to 665 actual unique events across 15 categories | **RESOLVED** |
| **OTel L6 controls** | 17_Telemetry | Elevated from deferred to P1: sampling, killswitch, PII safety, Datadog allowlist | **RESOLVED** |
| **Policy limits** | 03_LLM | Added to crate-coco-inference.md: 1h polling, ETag cache, role-based eligibility, fail-open | **RESOLVED** |
| **Bootstrap config** | 03_LLM | Expanded: 5s timeout, disk caching, model options merge, OAuth auth | **RESOLVED** |
| **Auth helpers** | 24_Auth | Added API key helper (5min TTL, SWR), bare mode (--bare hermetic auth) | **RESOLVED** |
| **QueryGuard 3-state** | 21_Steering | Fixed from binary to 3-state (idle→dispatching→running) with generation counter | **RESOLVED** |
| **CommandQueue priority** | 21_Steering | Added 3-level priority detail (now>next>later) with FIFO within level | **RESOLVED** |
| **Hook mutations** | 11_Hooks | Added PreToolUse→updatedInput, PostToolUse→updatedMCPToolOutput, function hooks, asyncRewake | **RESOLVED** |
| **Hook scope fix** | 11_Hooks | Fixed remaining wrong scope in hooks doc to Session>Local>Project>User>Plugin>Builtin | **RESOLVED** |
| **Compact warning + prompt** | 07_Compact | Added CompactWarningState and compact prompt generation | **RESOLVED** |
| **MCP elicitation expanded** | 06_MCP | Added waiting state, completion notification, channel permission relay with request IDs | **RESOLVED** |
| **Plugin MCPB format** | 25_Plugin | Added McpbLoadResult, McpbLoadStatus, cache metadata, config schema | **RESOLVED** |
| **Task TodoV2 + deps** | 13_Task | Added TodoV2 types, blocks/blockedBy dependency graph, resolution logic | **RESOLVED** |
| **Skill dynamic discovery** | 10_Skill | Added SkillChangeDetector state machine, watched paths, debouncing constants | **RESOLVED** |
| **Coordinator CLI flags** | 30_AgentTeams | Added build_inherited_cli_flags, match_session_mode | **RESOLVED** |
| **Shell sed parser + CWD** | 29_ShellParser | Added SedEditInfo, CwdTracker | **RESOLVED** |
| **CLI transport constants** | 01_CLI | Added retry/timeout constants, selection priority | **RESOLVED** |
| **LSP crate doc** | 27_LSP | Created crate-coco-lsp.md: 6 server states, diagnostic registry (LRU 500), 9 operations, crash recovery, plugin discovery, cocode-rs base reference | **RESOLVED** |
| **SDK DirectConnect** | 20_SDK | Expanded crate-coco-app.md: DirectConnectSessionState (5 states), ServerConfig, SessionIndexEntry, NDJSON codec, cocode-rs event design reference | **RESOLVED** |
| **FastMode expansion** | 34_FastMode | Added overage rejection, per-session opt-in, 1m merge, FastModeDisplayState | **RESOLVED** |
| **Adaptive thinking** | 19_ThinkLevel | Added model_supports_adaptive_thinking, resolve_applied_effort, numeric effort (0-255 ANT scale) | **RESOLVED** |
| **Prompt cache 16 dimensions** | 23_PromptCache | Expanded CachePromptState to 16 hash dimensions, TTL latching, per-tool hashing, token billing details | **RESOLVED** |
| **Background agent progress** | 26_BgAgents | Added AgentProgress, ToolActivity, symlink output, delta reads (8MB max) | **RESOLVED** |
| **Shell read-only flags** | 29_ShellParser | Expanded FlagArgType to 6 types, security patterns (combined flags, pickaxe, xargs getopt) | **RESOLVED** |
| **MCP XAA auth** | 24_Auth | Added XAA token exchange (RFC 8693 + 7523), OIDC discovery, XaaConfig/XaaResult types | **RESOLVED** |

### Updated Coverage Summary (Post-Round 7)

| Rating | Count | Areas |
|--------|-------|-------|
| Complete (100%) | 4 | 14_CodeIndexing, 32_Keybindings, 33_Remote, 35_Rewind |
| Good (75-99%) | 26 | 05_Tools (90%↑), 09_SlashCmd (88%), 18_Sandbox (80%), 31_AutoMemory (80%), 04_SysReminder (80%), 07_Compact (90%↑), 08_Subagent (80%), 11_Hooks (95%↑), 16_FileSystem (90%), 21_Steering (95%↑), 01_CLI (85%↑), 03_LLM (85%↑), 06_MCP (90%↑), 10_Skill (80%↑), 15_State (75%), 25_Plugin (80%↑), 30_AgentTeams (80%↑), 34_FastMode (85%↑), 13_Task (80%↑), 17_Telemetry (85%↑), 19_ThinkLevel (80%↑), 23_PromptCache (80%↑), 24_Auth (85%↑), 26_BgAgents (80%↑), 29_ShellParser (80%↑), 27_LSP (75%↑) |
| Partial (40-74%) | 1 | 20_SDK (60%↑) |
| New doc | 3 | 02_UI (95%↑) — crate-coco-tui.md (complete), 22_IDE (70%↑) — crate-coco-bridge.md, 27_LSP (75%↑) — crate-coco-lsp.md |
| Deferred (v3) | 1 | 28_BrowserControl (correctly deferred) |

### Remaining P0 Gaps

| # | Gap | Area | Status |
|---|-----|------|--------|
All P0 gaps are now **RESOLVED** (crate-coco-tui.md created in Round 7).

### Remaining Gaps (documented, implementation deferred)

| Priority | Gap | Area | Phase |
|----------|-----|------|-------|
| P1 | OTel span hierarchy implementation | 17_Telemetry | Phase 1 |
| P1 | OTel L3 665 event implementations | 17_Telemetry | Phase 3 |
| P1 | GrowthBook architectural decision for Rust | Cross | Phase 2 |
| P2 | SDK DirectConnect full session management | 20_SDK | Phase 5 |
| — | ~~crate-coco-tui.md~~ | 02_UI | **RESOLVED** — created with full widget catalog, event integration, TS component mapping |

### Round 10: System-Reminder + Compact Deep Review (April 29-30, 2026)

**Context**: deep review of `coco-system-reminder` and `services/compact` against TS external + follow-up "fix all items" pass + second deep-review pass that found three real bugs in the prior port. Almost all P0/P1 ports landed; remaining items are intentional non-ports or analytics/feature-gated.

**Round 10b deep-review fixes (post-port bugs caught and fixed):**

| Bug | Site | Fix |
|---|---|---|
| `sanitize_error_tool_result_content` gated on outer `tr.is_error` instead of wire-level `rp.is_error` — passes were no-ops when only the inner flag was set | `core/messages/src/normalize.rs` | Drop redundant outer guard; rely on `rp.is_error` (the wire-truth flag the Anthropic API rejects on). |
| `smoosh_system_reminder_into_tool_result` bailed on `is_error=true` tool_results, leaving the `text-after-tool_result` pattern intact — exactly the `\n\nHuman:` issue smoosh exists to prevent | `core/messages/src/normalize.rs::fold_text_into_last_tool_result` | Mirror TS `messages.ts:2545-2553`: SR text is text-only by definition, so smooshing into is_error tool_results is safe. |
| `merge_consecutive_same_role` merged Assistant+Assistant unconditionally — TS only merges chunks with matching `message.id`. Two distinct API responses landing back-to-back (e.g. retry-after-partial-stream on resume) get incorrectly stitched, producing mismatched thinking-block signatures the API rejects | `core/messages/src/normalize.rs` | Replace LlmMessage-level merge with Message-level `merge_consecutive_assistants_by_request_id` running BEFORE extraction (so `request_id` is still readable). User+User merge stays unconditional. Old `merge_consecutive_same_role` deleted. |
| Pipeline ordering: `sanitize_error` ran before `smoosh`, so smoosh-produced text in is_error tool_results escaped the text-only normalization | `core/messages/src/normalize.rs` | Reorder per TS: smoosh first, sanitize as final pass at LlmMessage level (`sanitize_error_tool_result_in_llm_messages`). |

**Regression tests** added in `core/messages/src/normalize.test.rs`:
- `merge_assistants_by_request_id_keeps_distinct_ids_separate`
- `merge_assistants_by_request_id_merges_matching_ids`
- `merge_assistants_with_no_request_id_stays_separate`
- `smoosh_folds_into_is_error_tool_result`
- `sanitize_strips_non_text_from_is_error_tool_result`

| Priority | Gap | TS source | Status | Notes |
|----------|-----|-----------|--------|-------|
| P0 | image-in-tool_result strip | `compact.ts:166-184` | **RESOLVED** | `compact.rs::strip_images_from_messages` now traverses `Message::ToolResult` content arrays. Test: `strip_images_walks_tool_result_content`. |
| P0 | `wrap_in_system_reminder` duplication | `messages.ts:3097` | **RESOLVED** | `coco-system-reminder::xml::wrap_system_reminder` delegates to `coco-messages::wrapping::wrap_in_system_reminder`. |
| P0 | `sanitizeErrorToolResultContent` | `messages.ts:1884` | **RESOLVED** | `core/messages/src/normalize.rs::sanitize_error_tool_result_content`; runs as Step 12 in the pipeline. |
| P0 | `smooshSystemReminderSiblings` | `messages.ts:1835-1873` | **RESOLVED** | `core/messages/src/normalize.rs::smoosh_system_reminder_into_tool_result` runs as Step 14, after same-role merge so SR-User next to Tool can fold. Bails on `is_error` and non-Text/Content output variants. |
| P1 | `filterOrphanedThinkingOnlyMessages` | `messages.ts:2311` | **RESOLVED** | Step 8; matches on `request_id` (TS `message.id` equivalent). |
| P1 | `filterTrailingThinkingFromLastAssistant` | `messages.ts:2322` | **RESOLVED** | Step 9; ordering invariant preserved (runs BEFORE whitespace filter). |
| P1 | `filterWhitespaceOnlyAssistantMessages` | `messages.ts:2324` | **RESOLVED** | Step 10; post-removal calls `merge_consecutive_user_messages` to reseal alternation. |
| P1 | `ensureNonEmptyAssistantContent` | `messages.ts:2325` | **RESOLVED** | Step 11; non-final assistants get `[No message content]` placeholder; final allowed empty for prefill. |
| P1 | `processSessionStartHooks('compact')` LLM path | `compact.ts:592` | **RESOLVED** | `engine_compaction.rs` invokes SessionStart after PostCompact and collects the hook events directly into `result.hook_results` (success output + additional contexts + blocking/stopped reminders) without also enqueueing them in the next-turn sync hook buffer. The aggregate output is also applied: `initialUserMessage` is inserted into the rewritten history and `watchPaths` is forwarded through a query→CLI side-effect sink. |
| P1 | `createPlanModeAttachmentIfNeeded` post-compact | `compact.ts:1542-1560` | **RESOLVED** | `services/compact/post_compact_plan_mode.rs::create_plan_mode_attachment_if_needed` renders the same Full-variant text as the system-reminder cadence. Wired in `engine_compaction.rs::try_full_compact` from a snapshot taken pre-compact (live `permission_mode == Plan` + `QueryEngineConfig.plan_mode_settings`). Plan instructions land on the FIRST post-compact turn, matching TS. |
| P1 | `createAsyncAgentAttachmentsIfNeeded` post-compact | `compact.ts:1568-1599` | **RESOLVED** | `services/compact/post_compact_async_agents.rs::create_async_agent_attachments` renders one `task_status` reminder per filtered async agent. Engine snapshot via `QueryEngine::snapshot_async_agents_for_post_compact` reads from optional `Arc<TaskManager>` (`with_running_tasks` builder hook), filters by TS rules: drop self-agent, drop pending, drop already-notified terminal. Empty when no `TaskManager` is installed (degrades to TS-feature-stripped behavior). |
| P2 | `recompactionInfo` populate | `compact.ts:317-323` | **RESOLVED** | `CompactRunOptions.recompaction_info: Option<RecompactionInfo>` plumbs through `compact_conversation`; `CompactResult.is_recompaction` reads from it. `QueryEngine::last_compact_state` (per-engine `Mutex<Option<LastCompactState>>`) tracks turn id + run id; `turn_counter` is incremented at the top of `finalize_turn_post_tools`. `try_full_compact` derives `RecompactionInfo` from the tracker each invocation and updates it post-success. |
| P0 | TS-style structured compact summary execution | `compact.ts` / `query.ts` compact fork path | **RESOLVED** | `compact_conversation` and `partial_compact_conversation` now pass `CompactSummaryAttempt` (`messages` + `context_messages` + `summary_request`) instead of a rendered `Fn(String)` prompt. `QueryEngine` prefers `ForkLabel::Compact` with deny-all tools and `skip_cache_write`, then falls back to a no-tools structured direct call. PostCompact hooks receive `CompactResult.raw_summary`, matching TS's raw `summary` payload. |
| P1 | Partial compact query-level restoration | `compact.ts:772-1106` + post-compact attachment helpers | **RESOLVED** | TUI rewind summarize now routes through `QueryEngine::run_partial_compact`, sharing PreCompact/PostCompact/SessionStart hooks, the summary executor, file/plan/skill/plan-mode/async-agent attachments, deferred-tool/agent/MCP delta injection, observer cleanup, and cache-break notification. `from`/Newest uses TS order `boundary → kept prefix → summary`; `up_to`/Oldest uses `boundary → summary → kept tail`. PTL retry for `from` truncates the full API context, not just the tail summary slice. |
| P1 | Auto compact failure breaker | `autoCompact.ts` failure guard | **RESOLVED** | `try_full_compact` returns `CompactOutcome`; `finalize_turn_post_tools` records auto-only consecutive failures and skips further auto LLM compaction after `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`. Manual compact is excluded; session-memory success resets the state. |
| P1 | Session-memory post-compact parity | `sessionMemoryCompact.ts:583-585` + plan attachment | **RESOLVED** | SM compact now injects current plan attachment and writes SessionStart hook output into `result.hook_results`; hook wire trigger remains TS-compatible `auto` for non-manual internal triggers. The SessionStart phase is emitted only after SM actually applies, so declined SM attempts fall through to LLM compact without a false hook phase. |
| P2 | `stripReinjectedAttachments` divergence | `compact.ts:211-223` | **NON-PORT (broader is correct)** | `survives_compaction()` filters more kinds than TS's narrow `skill_discovery`/`skill_listing` filter, but the intent is identical (drop regenerable, keep audit/UI-visible). Documented in source. |
| P3 | `relocateToolReferenceSiblings` | `messages.ts:2304` | **DEFERRED** | TS-gated on `tengu_toolref_defer_j8m`; coco-rs has no Tool Reference feature. Safe to defer until/unless the feature lands. |

---

## Round 6: Implementation Alignment (April 6, 2026)

Gaps identified in Round 5 addressed via code implementation in coco-rs.

### Gaps Resolved in Round 6

| Gap | Area | What Was Done | Status |
|-----|------|--------------|--------|
| **File backup/snapshot** | 16_Files | Implemented full FileHistoryState with content-addressed SHA-256 backups, make_snapshot/track_edit/rewind/get_diff_stats, session resume via hard-link migration. 12 tests. | **RESOLVED** |
| **QueryEngine expansion** | 21_Steering | Added CommandQueue (3-priority), QueryGuard (3-state machine), Inbox for teammate messages, ContinueReason enum for state transitions, expanded QueryEvent (13 types), interrupt handling. 12 new tests. | **RESOLVED** |
| **Attachment 3-batch parallel** | 04_SysReminder | Added generate_all_attachments_async() with per-batch 1000ms timeout, collect_batched_attachments() for sync path, subagent filtering. | **RESOLVED** |
| **Auto-mode classifier** | 29_Shell | Implemented two-stage XML classifier (Stage 1: 256 tokens, Stage 2: 4096 tokens), XML response parsing with JSON fallback, ambiguity detection for Stage 2 escalation. 5 new tests. | **RESOLVED** |
| **Denial tracking circuit breaker** | 21_Steering | Expanded DenialTracker with per-tool counts, circuit breaker (3 consecutive), suggestion messages, explicit reset_circuit_breaker(). 4 new tests. | **RESOLVED** |
| **Hook events → 27 types** | 11_Hooks | Expanded HookEventType to all 27 TS variants, added HookScope enum (Session>Local>Project>User>Plugin>Builtin), added status_message and async_rewake to SingleHookResult. | **RESOLVED** |
| **Compaction expansion** | 07_Compact | Added API microcompact (clear_tool_uses, clear_thinking), ReactiveCompactState with circuit breaker (3 failures), CompactObserver trait for post-compact hooks. | **RESOLVED** |
| **MCP elicitation** | 06_MCP | Added ElicitationRequest/Response/Field types, ElicitationFieldType enum, ChannelPermission types, McpResource for dynamic discovery. | **RESOLVED** |
| **Subagent enhancements** | 08_Subagent | Added AgentIsolation (None/Worktree/Remote), MemoryScope (User/Project/Local), expanded SpawnConfig with effort/use_exact_tools/isolation/memory_scope/mcp_servers/disallowed_tools/max_turns. | **RESOLVED** |
| **AppState expansion** | 15_State | Expanded from ~20 to 60+ fields: MCP clients/tools, plugins, notifications, speculation, bridge/IDE, remote, inbox, coordinator, ultraplan, elicitation, sandbox, bootstrap. | **RESOLVED** |
| **Hook scope hierarchy** | 11_Hooks | Fixed from "Skill>Plugin>Project>User>Global" to match TS: Session(highest)>Local>Project>User>Plugin>Builtin(lowest). | **RESOLVED** |

### Updated Coverage Summary (Post-Round 6)

| Rating | Count | Areas |
|--------|-------|-------|
| Complete (100%) | 4 | 14_CodeIndexing, 32_Keybindings, 33_Remote, 35_Rewind |
| Good (75-99%) | 10 | 05_Tools (85%), 09_SlashCmd (88%), 18_Sandbox (80%), 31_AutoMemory (80%), 04_SysReminder (80%↑), 07_Compact (85%↑), 08_Subagent (80%↑), 11_Hooks (85%↑), 16_FileSystem (90%↑), 21_Steering (85%↑) |
| Partial (40-74%) | 14 | 01_CLI (75%), 03_LLM (60%), 06_MCP (75%↑), 10_Skill (70%), 13_Task (55%), 15_State (75%↑), 17_Telemetry (60%), 19_ThinkLevel (60%), 23_PromptCache (60%), 24_Auth (55%), 25_Plugin (70%), 26_BgAgents (65%), 30_AgentTeams (70%↑), 34_FastMode (75%) |
| Weak (20-39%) | 4 | 12_PlanMode (30%), 20_SDK (40%), 27_LSP (20%), 29_ShellParser (55%↑) |
| Missing (<20%) | 2 | 02_UI (15%), 22_IDE (10%) |
| Deferred (v3) | 1 | 28_BrowserControl (correctly deferred) |

### Remaining P0 Gaps

| # | Gap | Area | Status |
|---|-----|------|--------|
| 1 | **No crate-coco-tui.md** | 02_UI | Deferred (per user: "fix all gaps except UI") |
| 5 | **Plan mode state machine** | 12_Plan | Not yet addressed |
| 6 | **IDE architecture** | 22_IDE | Not yet addressed |

---

## Round 5: 35-Area Deep Cross-Validation (April 3, 2026)

Full TS source code analysis vs coco-rs docs. Each area verified by dedicated agent reading actual TS files.
Multi-provider awareness checked. Shell-parser strategy updated to HYBRID (cocode-rs base + TS enhancements).

### Coverage Summary

| Rating | Count | Areas |
|--------|-------|-------|
| Complete (100%) | 4 | 14_CodeIndexing, 32_Keybindings, 33_Remote, 35_Rewind |
| Good (75-99%) | 4 | 05_Tools (85%), 09_SlashCmd (88%), 18_Sandbox (80%), 31_AutoMemory (80%) |
| Partial (40-74%) | 20 | 01_CLI (75%), 03_LLM (60%), 04_SysReminder (50%), 06_MCP (65%), 07_Compact (70%), 08_Subagent (65%), 10_Skill (70%), 11_Hooks (65%), 13_Task (55%), 15_State (55%), 16_FileSystem (75%), 17_Telemetry (60%), 19_ThinkLevel (60%), 21_Steering (65%), 23_PromptCache (60%), 24_Auth (55%), 25_Plugin (70%), 26_BgAgents (65%), 30_AgentTeams (60%), 34_FastMode (75%) |
| Weak (20-39%) | 4 | 12_PlanMode (30%), 20_SDK (40%), 27_LSP (20%), 29_ShellParser (35%) |
| Missing (<20%) | 2 | 02_UI (15%), 22_IDE (10%) |
| Deferred (v3) | 1 | 28_BrowserControl (correctly deferred) |

### P0 Critical Gaps — Block Implementation

| # | Gap | Area | What's Missing | Action |
|---|-----|------|---------------|--------|
| 1 | **No crate-coco-tui.md** | 02_UI | 346 component files unmapped; 30+ message renderers, dialog system, notification (5 terminal backends), output styles | Create crate-coco-tui.md: widget taxonomy, message components, dialog lifecycle, notification channels |
| 2 | **No crate-coco-lsp.md** | 27_LSP | Server lifecycle (6 states), diagnostic dedup (LRU 500), 9 LSP operations, crash recovery, plugin discovery | Create crate-coco-lsp.md: state machine, operations, diagnostics registry, error retry |
| 3 | **File backup/snapshot system** | 16_Files | Content-addressed backup (`~/.claude/file-history/`), 100-snapshot cap, message-level snapshotting, undo/rewind | Add FileHistoryState/Snapshot/Backup structs to crate-coco-context.md or crate-coco-tools.md |
| 4 | **Interrupt semantics** | 21_Steering | Reason codes (interrupt vs submit-interrupt), conditional message emission, vendor cleanup | Add interrupt handling spec to crate-coco-query.md |
| 5 | **Plan mode state machine** | 12_Plan | Plan file CRUD, storage path, recovery (3-source), interview phase, teammate approval, circuit breaker | Major expansion of plan mode in crate-coco-tools.md or new doc |
| 6 | **IDE architecture** | 22_IDE | MCP-based 17-IDE bridge, CCR daemon (3 spawn modes), DirectConnect server, permission bridge, diagnostics tracking | Create/rewrite IDE section in crate-coco-app.md |
| 7 | **Shell read-only validation** | 29_Shell | ~40 safe commands + 200 flags in COMMAND_ALLOWLIST. **Strategy: extend cocode-rs shell-parser with TS data** | Add read-only allowlist data to crate-coco-shell.md (already structurally present, needs flag detail) |

### P1 High-Priority Gaps — Affect Core Logic

| # | Gap | Area | Detail |
|---|-----|------|--------|
| 1 | Policy limits fetching | 03_LLM | Background polling (1h), ETag cache, role-based eligibility (OAuth + Team/Enterprise). Enterprise feature completely absent from Rust plan |
| 2 | Attachment 3-batch parallel | 04_SysReminder | Three parallel batches with 1000ms timeout, tier isolation (MainAgentOnly/AllThread/Always), generator streaming, 40+ attachment types |
| 3 | MCP channel servers + elicitation | 06_MCP | Permission relay via channels (KAIROS gate), form/URL elicitation, XAA token exchange (OIDC discovery) |
| 4 | API microcompact + reactive compact | 07_Compact | clear_tool_uses/clear_thinking API strategies, reactive compact (feature-gated prompt_too_long recovery), circuit breaker (3 failures) |
| 5 | Hook input/output mutation + 27 events | 11_Hooks | PreToolUse→updatedInput, PostToolUse→updatedMCPToolOutput. TS has **27 event types** (docs undercount): adds SessionEnd, InstructionsLoaded, ConfigChange, TeammateIdle, TaskCreated, TaskCompleted + WorktreeCreate/Remove, CwdChanged, FileChanged, Elicitation/Result, PostToolUseFailure, StopFailure, SubagentStart/Stop. Scope mismatch: TS is session>local>project>user (not skill>plugin>project>user). statusMessage field, asyncRewake flag (exit code 2), function hooks (session-scoped callbacks) |
| 6 | DirectConnect server | 20_SDK | HTTP+WS session lifecycle (starting→running→detached→stopping→stopped), NDJSON codec, session index persistence |
| 7 | Agent effort/handoff/cache/isolation | 08_Subagent | effort field, useExactTools (cache-identical prefixes), handoff classifier (auto-mode post-execution gate), model inheritance precedence (param>def>parent). Also: `isolation: 'remote'` (CCR, ant-only), `memory: 'user'\|'project'\|'local'` scope, `mcpServers` per-agent, `initialPrompt` field |
| 8 | Telemetry L6 controls | 17_Otel | Sampling (per-event config), killswitch (sink disable), GrowthBook integration, PII safety markers — all implemented in TS but marked "deferred" in docs |
| 9 | Auth apiKeyHelper + bare mode | 24_Auth | Command-based API key with 5min TTL cache, async prefetch; bare mode (--bare) hermetic auth; MCP OAuth XAA/OIDC |
| 10 | Generator vs loop + QueryGuard 3-state | 21_Steering | TS uses async generators (query loop, attachments, tool executor); docs assume classical loops. Affects SDK streaming. QueryGuard is 3-state (idle→dispatching→running) with generation counter — not binary. CommandQueue has 3-level priority (now>next>later). Memory/skill prefetch uses resource disposal pattern (load-bearing for performance) |
| 11 | Task management tools | 13_Task | 6 tools (Create/Update/Get/List/Stop/Output), TodoWrite, dependency graph (blocks/blockedBy), verification agent nudge |
| 12 | Multi-provider thinking conversion | 19_Think | Adaptive thinking (per-model), numeric effort (0-255 ANT scale), env overrides chain, per-model defaults (Opus 4.6→medium) |

### P2 Medium-Priority Gaps

| Gap | Area | Notes |
|-----|------|-------|
| Session persistence JSONL + cost restoration | 15_State | SessionManager file layout, resume picker, fork-session semantics |
| Skill effort/files/aliases + dynamic discovery | 10_Skill | Walks directory tree from edited files, conditional activation state machine |
| Prompt cache TTL latching + 12 hash dimensions | 23_Cache | GrowthBook allowlist, per-tool schema hashing |
| Plugin MCPB format + author validation + channels | 25_Plugin | Chrome extension bundles, homograph attack prevention |
| Agent teams idle notification + pane lifecycle | 30_Teams | planModeRequired, worktree ownership, hidden pane management |
| Background agent progress metadata + delta reads | 26_BgAgents | AgentProgress struct, streaming output, symlink-based output |
| Team memory dual-directory + KAIROS daily-log | 31_Memory | Private vs team dirs, nightly /dream skill, mutual exclusion |
| Compact circuit breaker (3 consecutive failures) | 07_Compact | Prevents 250K wasted API calls on irrecoverable errors |
| Destructive command warnings (18 patterns) | 29_Shell | Application-level, git/rm/db/k8s/terraform patterns (already in crate doc structurally) |
| Context overflow FLOOR_OUTPUT_TOKENS | 03_LLM | 3000-token floor check + thinking budget preservation on retry |
| Fast mode cooldown/overage + per-session opt-in | 34_Fast | handleFastModeOverageRejection, 1m merge behavior |
| FileEdit desanitization + quote handling | 16_Files | Reverses XML sanitization, contraction-aware curly quote normalization |
| Slash command 3-type union + auth gating | 09_Cmd | prompt/local/local-jsx execution models, availability[] filtering |
| 60+ deferred AppState fields | 15_State | Remote, notifications, attribution, plugins, MCP, speculation subsystems |
| GrowthBook equivalent decision | Cross | Pervasive in TS (15+ feature flags); needs architectural decision for Rust |
| Transport selection + retry constants | 01_CLI | getTransportForUrl() logic, POST_MAX_RETRIES=10, 500ms base delay, SSE 45s liveness, 600s give-up window |
| Daemon worker lifecycle | 01_CLI | `--daemon-worker=<kind>` arg parsing, worker registry, supervisor/worker protocol |
| Bootstrap config fetching | 03_LLM | `/api/claude_cli/bootstrap` endpoint, 5s timeout, disk caching, model options merge |
| NotebookEdit output clearing on replace | 16_Files | Resets execution_count to null, clears outputs array (stale execution data invalidation) |
| Sed edit parser | 29_Shell | 200+ LOC: sed in-place edit parsing and constraint validation |
| CWD tracking implementation | 29_Shell | Hidden temp file (pwd -P), deleted CWD recovery, NFC normalization, hook integration |
| Agent teams CLI flag propagation | 30_Teams | buildInheritedCliFlags(): permission mode, model override, plugin config, --teammate-mode flag, plan mode precedence |
| Agent teams session mode matching | 30_Teams | matchSessionMode() auto-flips CLAUDE_CODE_COORDINATOR_MODE env var on session resume |
| Prompt cache token billing | 23_Cache | cache_creation_input_tokens vs cache_read_input_tokens cost tier semantics |

### Factual Corrections (docs say X, TS actually does Y)

| Doc Location | Docs Say | TS Actually | Fix |
|-------------|---------|-------------|-----|
| crate-coco-hooks.md | "Scope: Skill > Plugin > Project > User > Global" | session > local > project > user > plugin/builtin (numeric priority); Global doesn't exist as hook source | Fix scope hierarchy |
| crate-coco-otel.md | "~53 application events" | 37 core Datadog events + 8 OAuth events + Chrome bridge events; docs undercount OAuth (2 methods vs 8 TS events) | Fix event count |
| crate-coco-otel.md | "L6 暂不实现 (sampling/killswitch)" | All L6 controls implemented in TS production: shouldSampleEvent(), sinkKilled(), checkMetricsEnabled() | Elevate L6 to P1 |
| ts-to-rust-mapping.md | memdir/ = 507 LOC | Actually ~1,736 LOC (memoryTypes 272 + memdir 508 + memoryAge 53 + findRelevantMemories 141 + memoryScan 95 + more) | Fix LOC |
| audit-gaps.md (Round 4) | "6 missing event types" in hooks | Actually ~16 missing event types from documented set to reach TS's 27 total | Fixed in Round 5 P1 #5 |

### Cross-Cutting Observations

1. **Generator pattern**: TS uses async generators for query loop, attachment injection, tool execution streaming. Docs model as classical loops. This architectural mismatch affects SDK streaming.
2. **Feature flags**: 15+ GrowthBook flags control tool loading, 12+ control behaviors. Labeled "v2/v3 deferred" in docs but ship in production Kairos builds.
3. **Multi-provider**: Auth routing (Anthropic/Bedrock/Vertex/Foundry) well-covered in crate-coco-inference.md. Thinking conversion per-provider documented. **Gap**: beta header matrix needs per-provider prompt cache behavior, model switching semantics.
4. **Shell-parser strategy updated**: HYBRID (cocode-rs `utils/shell-parser` base + TS security enhancements). No longer REWRITE.
5. **LOC undercounts**: memdir 507→1736, shell "6.2K cocode-rs" but TS is 23K, components 346 files.
6. **PII safety**: TS has type-enforced markers for telemetry. Not in docs.

---

## Historical Gaps (Rounds 1-3)

## Critical Gaps (must fix before implementation)

### 1. coco-messages: 100+ missing functions

`utils/messages.ts` exports **114 functions** — plan documents only 7.

**Missing categories:**
- 15 message creation helpers (createUserInterruptionMessage, createSyntheticUserCaveatMessage, etc.)
- 10 normalization functions (mergeUserMessages, mergeAssistantMessages, reorderAttachmentsForAPI)
- 8 tool result handlers (ensureToolResultPairing, filterUnresolvedToolUses, stripToolReferenceBlocks)
- 6 message lookup functions (buildMessageLookups, getSiblingToolUseIDs, getToolResultIDs)
- 5 streaming types (StreamingToolUse, StreamingThinking, handleMessageFromStream)
- 10 compact boundary functions (isCompactBoundaryMessage, findLastCompactBoundaryIndex)
- 20+ system message creators (createPermissionRetryMessage, createBridgeStatusMessage, etc.)
- 30+ utility functions (deriveShortMessageId, stripSignatureBlocks, wrapInSystemReminder, etc.)

**Action**: Don't enumerate all 114 functions in the plan. Instead, document the **categories** and the **core interface**. Implementation will translate function-by-function from TS.

### 2. Missing core concepts not in any plan doc

| Concept | TS source | What it is | Should be in |
|---------|-----------|------------|-------------|
| **Tool Result Budget (full Level 1 + Level 2)** | `utils/toolResultStorage.ts` (1040 LOC), `constants/toolLimits.ts`, `utils/mcpOutputStorage.ts` | Two-level pipeline: (1) per-tool persistence — `<persisted-output>` wrapper + 2KB preview + session-scoped `tool-results/` dir, invoked from `services/tools/toolExecution.ts:addToolResult`; (2) per-message aggregate budget (`MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000`) — `ContentReplacementState{seenIds,replacements}` + `enforceToolResultBudget`, invoked from `query.ts:379` before micro-compact. **Not** a single state machine. **Re-routed** from `coco-context` to `coco-tool-runtime` (storage + enforcement) + `coco-query` (wiring) + `coco-session` (transcript records) — see [`tool-result-budget-plan.md`](tool-result-budget-plan.md). Current Rust mirrors the TS storage owner, session-scoped paths, idempotent writes, preview replacement semantics, Level 2 prompt projection, transcript replacement records/resume reconstruction, MCP binary output persistence, session cleanup, and per-tool threshold defaults. Remaining work is ordinary E2E hardening around future MCP block types, not an open parity blocker. | `coco-tool-runtime` + `coco-query` + `coco-session` |
| `FileStateCache` | `utils/fileStateCache.ts` (1479 LOC) | LRU cache of file contents before tool execution | `coco-context` |
| `FileHistoryState` | `utils/fileHistory.ts` | Tracks file edits per turn for change detection/undo | `coco-messages` or `coco-context` |
| `processUserInput/` | `utils/processUserInput/` (4 files) | Pre-processes user input (images, slash commands, bash) | `coco-query` |
| `utils/tokens.ts` | Token extraction from messages/API responses | `coco-inference` |
| `utils/api.ts` (26K LOC) | Tool schema conversion, CacheScope, system prompt blocks | `coco-inference` |
| `utils/modelCost.ts` | Per-model pricing calculations | `coco-inference` |
| `utils/worktree.ts` (600 LOC) | Git worktree management | `coco-tools` |
| `utils/theme.ts` | Theme management | `coco-tui` |

### 3. coco-permissions: auto-mode/yolo classifier not documented

`utils/permissions/yoloClassifier.ts` (1495 LOC) — two-stage XML classifier system:
- Stage 1: FAST (64 tokens, nudged for quick block decision)
- Stage 2: THINKING (4096+ tokens, full chain-of-thought reasoning)
- Shared prompt prefix for cache hits between stages
- Integrates with:
  - `bashClassifier.ts` — semantic command safety (stub for external builds)
  - `classifierDecision.ts` (98 LOC) — classifier → PermissionDecision mapping
  - `classifierShared.ts` — safe-tool allowlist (read-only tools skip classifier)
  - `denialTracking.ts` (45 LOC) — 3 consecutive or 20 total denials → fallback to prompting
  - `autoModeState.ts` (39 LOC) — session state machine with GrowthBook circuit breaker
  - `dangerousPatterns.ts` — code-exec pattern stripping at auto-mode entry
  - `shellRuleMatching.ts` — wildcard/prefix/exact command matching
  - `PermissionContext.ts` (388 LOC) — decision lifecycle wrapper
  - 26 files total in permissions/ directory

**Action**: Document the **permission evaluation pipeline** as a flowchart, not list every function.

### 4. coco-tools: 7 missing tools

| Tool | Status | Notes |
|------|--------|-------|
| `MCPTool` | **FIXED** | MCP tool proxy (passthrough schema) |
| `McpAuthTool` | **FIXED** | MCP OAuth authentication |
| `PowerShellTool` (14 files) | **FIXED** | Windows-only, CLM security analysis |
| `REPLTool` | **FIXED** | REPL mode (wraps primitive tools) |
| `SleepTool` | **FIXED** | Wait/sleep tool (PROACTIVE/KAIROS gate) |
| `SyntheticOutputTool` | **FIXED** | SDK-only structured output |
| `ScheduleCronTool` path | **FIXED** | Added TS source paths for CronCreate/Delete/List |

### 5. coco-app/state: AppState has 60+ fields, plan has ~10

**Missing entire subsystems in AppState:**
- Bridge state (11 fields: replBridgeEnabled, replBridgeConnected, replBridgeSessionActive, etc.)
- Tungsten/tmux integration (5 fields: tungstenActiveSession, tungstenPanelVisible, etc.)
- WebBrowser/Bagel tool (3 fields: bagelActive, bagelUrl, bagelPanelVisible)
- Computer-use MCP state (computerUseMcpState)
- Coordinator mode (coordinatorTaskIndex, viewSelectionMode)
- KAIROS/assistant mode (kairosEnabled)
- Elicitation queue (elicitation.queue)
- Remote agent state (remoteAgentTaskSuggestions, remoteConnectionStatus)
- Thinking toggle (thinkingEnabled)
- Session hooks state (sessionHooks)
- Speculation/pipelining (speculation, speculationSessionTimeSavedMs)
- Fast mode & advisor (fastMode, advisorModel, effortValue)
- Inbox messages (inbox.messages)
- Notifications (notifications.current, notifications.queue)
- Worker sandbox permissions (workerSandboxPermissions)
- Ultraplan state (ultraplanLaunching, isUltraplanMode, etc.)
- Plugin system (plugins.enabled, plugins.disabled, plugins.errors, plugins.installationStatus)
- MCP system (mcp.clients, mcp.tools, mcp.commands, mcp.resources)
- Prompt suggestion (promptSuggestion)

### 6. ts-to-rust-mapping.md: 8 unmapped TS util files

| File | LOC | Belongs in |
|------|-----|-----------|
| `utils/processUserInput/` | 4 files | `coco-query` |
| `utils/fileHistory.ts` | 200+ | `coco-context` |
| `utils/tokens.ts` | 100+ | `coco-inference` |
| `utils/api.ts` | 26K | `coco-inference` |
| `utils/worktree.ts` | 600+ | `coco-tools` |
| `utils/modelCost.ts` | 200+ | `coco-inference` |
| `utils/theme.ts` | 200+ | `coco-tui` |
| `utils/config.ts` | 600+ | `coco-config` (GlobalConfig — already partially added) |

### 7. coco-inference: missing 5 major subsystems

| Subsystem | TS source | LOC | What it does |
|-----------|-----------|-----|-------------|
| `claude.ts` | `services/api/claude.ts` | 3,419 | Full query orchestration: streaming/non-streaming, beta headers, system prompt, fallback, media limits |
| `withRetry.ts` | `services/api/withRetry.ts` | 550 | Two-layer retry: exponential backoff + auth-aware retry + fast-mode aware + persistent mode |
| `filesApi.ts` | `services/api/filesApi.ts` | 748 | File upload/download: 500MB limit, retry, path security, concurrency pool |
| `dumpPrompts.ts` | `services/api/dumpPrompts.ts` | 227 | Non-blocking debug trace: fingerprint dedup, JSONL output, session-scoped |
| `utils/auth.ts` | `utils/auth.ts` | 2,002 | OAuth/API key: token refresh pipeline, distributed lock, 401 dedup, AWS/GCP auth |

### 8. coco-compact: 4 undocumented submodules

| Module | What it does |
|--------|-------------|
| `grouping.ts` (64 LOC) | Groups messages at API-round boundaries (by assistant.id, not human turns) |
| `postCompactCleanup.ts` (78 LOC) | Clears 10+ caches after compaction (classifier approvals, memory files, system prompt, etc.) |
| `apiMicrocompact.ts` (154 LOC) | API-level context management (clear_tool_uses, clear_thinking strategies) |
| `timeBasedMCConfig.ts` (44 LOC) | GrowthBook config: 60-min gap threshold, keep-recent=5 |

### 9. coco-shell: 4 undocumented modules

| Module | What it does |
|--------|-------------|
| `shouldUseSandbox.ts` (154 LOC) | Complex decision: sandbox enabled → policy → excluded commands (GrowthBook + user config) → feature flags |
| `destructiveCommandWarning.ts` (103 LOC) | ~20 regex patterns for destructive commands (git reset --hard, rm -rf, kubectl delete, terraform destroy, etc.) |
| `sedEditParser.ts` (200+ LOC) | Sed in-place edit parsing and constraint validation |
| `modeValidation.ts` (116 LOC) | Permission mode auto-allow: acceptEdits → mkdir/touch/rm/mv/cp/sed |

### 10. multi-provider-plan.md: missing provider-specific details

| Gap | What's missing |
|-----|---------------|
| Beta headers matrix | Which headers sent to which provider (10+ headers, 4+ providers) |
| Prompt caching by provider | Anthropic supports it with CacheScope; OpenAI doesn't |
| Streaming differences | Model-specific stream event handling |
| Message normalization across providers | How multi-turn messages adapt when switching providers |
| OpenAI Responses API | When to use Chat vs Responses, format differences |

### 11. coco-config: cocode-rs patterns not documented

| Pattern | What it is |
|---------|-----------|
| `ConfigSection` trait | Each config section implements `from_overrides()`, `from_env()`, `merge_json()`, `finalize()` |
| `ConfigResolver` | Resolves relative paths, merges model defaults with user overrides |
| `ConfigManager` with `RwLock` | Thread-safe config access with poison recovery |
| Provider/model JSON files | Separate `providers.json` and `models.json` config files in cocode-rs |

### 12. Hooks system: 15+ executor files not documented

`utils/hooks/` has 15+ files. Plan mentions 4 executor types (bash, prompt, http, agent). Missing:
- `fileChangedWatcher.ts` — file change hooks
- `hookEvents.ts` — event type definitions
- `hooksConfigManager.ts` — config management
- `postSamplingHooks.ts` — post-sampling hook pipeline
- `registerFrontmatterHooks.ts` — frontmatter hook registration
- `registerSkillHooks.ts` — skill-level hook registration
- `sessionHooks.ts` — session lifecycle hooks
- `skillImprovement.ts` — skill improvement hooks
- `ssrfGuard.ts` — SSRF protection for HTTP hooks

---

## Fix Status

| # | Gap | Status |
|---|-----|--------|
| 1 | coco-messages 114 functions | **FIXED**: Documented 15 categories with 114 function signatures in crate-coco-messages.md |
| 2 | Missing core concepts (ContentReplacementState, FileStateCache, etc.) | **FIXED**: Added 9 files to ts-to-rust-mapping.md |
| 3 | coco-permissions auto-mode/yolo | **FIXED**: Full classifier documentation added to crate-coco-permissions.md (two-stage XML, denial tracking, safe-tool allowlist, dangerous patterns, CLAUDE.md integration) |
| 4 | coco-tools 7 missing tools | **FIXED**: Added MCPTool, McpAuthTool, PowerShellTool, REPLTool, SleepTool, SyntheticOutputTool + ScheduleCronTool paths to crate-coco-tools.md |
| 5 | AppState 60+ fields | **P2**: Full state documented during coco-state implementation |
| 6 | ts-to-rust-mapping gaps | **FIXED**: Added 24 previously unmapped files |
| 7 | coco-inference auth + filesApi + retry + claude.ts | **FIXED**: Added auth system (OAuth, token refresh, distributed lock, 401 dedup, AWS/GCP), filesApi (upload/download pipeline, path security), retry engine (two-layer, fast-mode aware, persistent), query options, bootstrap, dump_prompts, HTTP utils to crate-coco-inference.md |
| 8 | coco-compact submodules | **FIXED**: Added grouping (API-round boundaries), postCompactCleanup (10+ cache clears), apiMicrocompact (context strategies), timeBasedMCConfig (GrowthBook config) to crate-coco-compact.md |
| 9 | coco-shell modules | **FIXED**: Added destructiveCommandWarning (20 patterns), shouldUseSandbox (decision logic), modeValidation (acceptEdits auto-allow), bash permission pipeline to crate-coco-shell.md |
| 10 | multi-provider beta headers | **FIXED**: Added 13-row beta header matrix to multi-provider-plan.md |
| 11 | coco-config patterns | **P3**: ConfigSection trait documented during implementation |
| 12 | Hooks 15+ executor files | **FIXED**: Added to ts-to-rust-mapping.md |
| 13 | String→Enum type safety (Round 5) | **FIXED**: ToolId, AgentTypeId, 8 new enums, 31 enum derive annotations, String→ToolId across 10+ docs |
| R5-1 | ToolName enum missing from crate-coco-types.md | **FIXED**: Added 41-variant enum with as_str(), FromStr |
| R5-2 | SubagentType enum missing from crate-coco-types.md | **FIXED**: Added 7-variant enum with as_str(), FromStr |
| R5-3 | ShellType vs ShellKind inconsistency | **FIXED**: Unified to ShellKind in coco-hooks + coco-config |
| R5-4 | EffortValue vs EffortLevel inconsistency | **FIXED**: Unified to EffortLevel in coco-types + coco-inference |
| R5-5 | BuiltinPluginDefinition layer violation (L1→L4) | **FIXED**: `BuiltinPluginDefinition.manifest` is `serde_json::Value`; plugin crate owns deserialization |
| R5-6 | SkillDefinition.hooks type mismatch | **FIXED**: `SkillDefinition.hooks` is `Option<serde_json::Value>` |
| R5-7 | MessageRole undefined in coco-messages | **FIXED / SUPERSEDED**: message filtering uses `MessageKind`; system-reminder has its own local `MessageRole` |
| R5-8 | ThinkingLevel name collision (config enum vs inference struct) | **FIXED**: Removed config enum, struct→coco-types, ModelInfo restored |
| R5-9 | TaskStateBase dual definition (types vs tasks) | **FIXED**: tasks uses `coco_types::TaskStateBase` as canonical |
| R5-10 | OAuthTokens collision (inference vs mcp) | **ACCEPTED / DOC CLEANUP**: distinct crate-local token structs; plan docs name them `ApiOAuthTokens` / `McpOAuthTokens` to avoid ownership ambiguity |

---

## Cross-Review Fixes (CLAUDE.md audit)

| Issue | What was wrong | Fix |
|-------|---------------|-----|
| 13 type inconsistencies | PermissionResult/PermissionDecision, check_permission/check_permissions, ApiProvider/ProviderApi, Option<i64>/i64 | **FIXED**: Canonical names in CLAUDE.md, updated crate docs |
| 8 redundancies | ModelInfo, ProviderApi defined in 3 places | **FIXED**: multi-provider-plan.md now defers to crate docs |
| Missing dependency sections | 7 crate docs had no Dependencies block | **FIXED**: Added to messages, compact, commands, shell, permissions, tools, modules, app |
| ToolResult.context_modifier | Referenced ToolUseContext from coco-types (circular) | **FIXED**: Removed from coco-types, handled by Tool::modify_context_after() |
| HooksSettings in coco-types | L1 type referencing L4 type | **FIXED**: Changed to `Option<Value>` in PromptCommandData |
| compact uses ToolUseContext | coco-compact shouldn't depend on coco-tool | **FIXED**: Changed to `&ApiClient` parameter |
| Missing TS mappings | MagicDocs, toolUseSummary, setup.ts | **FIXED**: Added to ts-to-rust-mapping.md |

---

## Cross-Review Round 2 (TS file-by-file + architecture deep dive)

### TS Mapping Gaps — FIXED

| Gap | Items | Fix |
|-----|-------|-----|
| Unmapped services/ files | 6 files (awaySummary, diagnosticTracking, internalLogging, mcpServerApproval, preventSleep, claudeAiLimitsHook) | **FIXED**: Added to ts-to-rust-mapping.md |
| Unmapped utils/ dirs | 4 dirs (filePersistence, dxt, deepLink, background) | **FIXED**: Added to ts-to-rust-mapping.md |
| Voice files not enumerated | services/voice*.ts catch-all → 3 specific files | **FIXED**: Enumerated voiceKeyterms, voiceStreamSTT, voice.ts |
| React hooks business logic | 16 hooks with substantial non-React logic | **FIXED**: Added "React Hooks with Business Logic" table to ts-to-rust-mapping.md |
| Stale counts | v1=55, total=75 | **FIXED**: Updated to v1=63, total=87 |

### Architecture Gaps — FIXED

| Gap | What was wrong | Fix |
|-----|---------------|-----|
| ToolUseContext under-specified | 15 fields documented, TS has 40+ | **FIXED**: Expanded to 40+ fields in crate-coco-tool.md with all callbacks, tracking sets, flags |
| Tool trait missing methods | 6 must-port methods not documented | **FIXED**: Added inputs_equivalent, prepare_permission_matcher, to_auto_classifier_input, get_path, backfill_observable_input, output_schema, modify_context_after + 8 v2 methods commented |
| StreamingToolExecutor behavior | "Tools execute after streaming" | **FIXED**: Documented that tools execute DURING API streaming. Added SyntheticToolError enum, context modifier stacking, progress handling |
| QueryEngine missing features | SDKPermissionDenial, orphanedPermission, snipReplay not documented | **FIXED**: Added 6 new fields to QueryEngine, 7 new fields to QueryEngineConfig, 4 new types |
| Message types incomplete | 5 variants, TS has 8+ | **FIXED**: Expanded to 8 variants + 14 system message sub-types + NormalizedMessage + StreamEvent + MessageOrigin |
| crate-coco-app.md missing TS source | No combined TS source header | **FIXED**: Added TS source line |

---

## Cross-Review Round 3 (Deep TS comparison — April 2026)

### Newly Fixed (this round)

| Gap | What was missing | Fix |
|-----|------------------|-----|
| coco-permissions: full classifier architecture | 74-line stub → TS has 1495-line 2-stage XML classifier | **FIXED**: Full documentation in crate-coco-permissions.md (two-stage, denial tracking, CLAUDE.md integration, safe-tool allowlist, dangerous pattern stripping, security invariants) |
| coco-messages: 114 functions | 7 documented | **FIXED**: 15 categories with signatures in crate-coco-messages.md |
| coco-inference: auth system | "65K LOC" placeholder | **FIXED**: Actual 2002 LOC with OAuth, token refresh, distributed lock, 401 dedup, AWS/GCP auth |
| coco-inference: claude.ts | Not documented | **FIXED**: 3419 LOC query orchestration (streaming/non-streaming, beta headers, fallback, media limits) |
| coco-inference: withRetry.ts | Skeleton only | **FIXED**: 550 LOC two-layer retry (auth-aware, fast-mode, persistent, context overflow) |
| coco-inference: filesApi.ts | Listed only | **FIXED**: 748 LOC upload/download pipeline, path security, concurrency |
| coco-inference: dumpPrompts.ts | Not documented | **FIXED**: 227 LOC non-blocking debug trace |
| coco-inference: bootstrap.ts | Not documented | **FIXED**: 141 LOC lazy-fetch org config |
| coco-compact: grouping.ts | Listed only | **FIXED**: API-round boundary grouping (by assistant.id) |
| coco-compact: postCompactCleanup.ts | Listed only | **FIXED**: 10+ cache clears, main-thread guard |
| coco-compact: apiMicrocompact.ts | Listed only | **FIXED**: API-level clear_tool_uses / clear_thinking strategies |
| coco-compact: timeBasedMCConfig.ts | Listed only | **FIXED**: GrowthBook config (60-min gap, keep-recent=5) |
| coco-shell: destructiveCommandWarning | Listed only | **FIXED**: 20 regex patterns documented |
| coco-shell: shouldUseSandbox | Listed only | **FIXED**: Decision logic (GrowthBook + user config + policy) |
| coco-shell: modeValidation | Listed only | **FIXED**: acceptEdits auto-allow commands |
| coco-shell: bash permissions pipeline | Not documented | **FIXED**: Full 7-step pipeline |

## Cross-Review Round 4 (35-Area TS-First Validation — April 2026)

### Major Corrections

| Issue | What was wrong | Fix |
|-------|---------------|-----|
| Steering ≠ GrowthBook | Area 21 "steering" wrongly identified as feature flags. Actual: mid-turn message queue & injection (user sends guidance while LLM working) | **FIXED**: Added steering section to crate-coco-query.md (QueuedCommand, CommandQueue, QueryGuard, mid-turn attachment injection, inbox) |
| Background execution = v1, not v2 | Both BashTool and AgentTool support `run_in_background`. Task framework is v1 core, coordinator is v2 orchestration on top | **FIXED**: Expanded coco-tasks in modules.md with TaskState union, isBackgrounded, auto-background, task output, notifications |
| Subagent = agent-as-task | Agents are not separate from tasks — they register as LocalAgentTaskState. No separate SubagentManager needed | **FIXED**: Added AgentTool architecture to crate-coco-tools.md (spawn routing, fork, worktree, tool filtering, lifecycle) |
| Prompt cache undocumented | 727 LOC cache break detection algorithm missing from inference doc | **FIXED**: Added CacheScope, CacheBreakDetector, 2-phase detection to crate-coco-inference.md |
| ts-utils-mapping B12 error | forkedAgent.ts etc. wrongly mapped to `memory/` | **FIXED**: Remapped to `coco-tools` (AgentTool submodule) |
| ThinkingConfig boolean | Rust doc had bool+budget, TS has 3-variant union (adaptive/enabled/disabled) | **FIXED**: Changed to enum in crate-coco-inference.md |
| Fast mode stub | Only enum documented, org-level behavior missing | **FIXED**: Added availability check, cooldown semantics, prefetch to crate-coco-config.md |
| Rewind 1-line | Only "Rewind to earlier turn" | **FIXED**: Added mechanism (message selector, file snapshots, restoration) to crate-coco-commands.md |
| FileHistory missing | Not in any doc | **FIXED**: Added FileHistoryState, encoding detection, FileStateCache to crate-coco-context.md |
| OAuth PKCE missing | Auth outline only | **FIXED**: Added 7-step PKCE flow, auth-code listener, crypto to crate-coco-inference.md |

### Stub Expansions

| Crate | What was expanded | Key additions |
|-------|------------------|---------------|
| coco-skills (modules.md) | 3-line load() → full multi-source discovery | Loading order (bundled→plugin→user→project→managed), dedup via realpath, conditional activation, memoization, bundled registry |
| coco-hooks (modules.md) | 6 event types → 15 | FileChanged, CwdChanged, PermissionDenied, PostToolUseFailure, PermissionRequest, Notification, Elicitation, ElicitationResult, WorktreeCreate + AsyncHookRegistry + HookScope priority |
| coco-memory (modules.md) | Basic CRUD → full feature | 4-type taxonomy scope rules, staleness detection (memoryAge), MEMORY.md truncation (200 lines/25KB), Sonnet-based recall selector, two-step save |
| coco-keybindings (modules.md) | 3 struct fields → full system | 18 KeybindingContext variants, 50+ KeybindingAction variants, chord support, platform defaults, reserved shortcuts (ctrl+c/d double-press), user binding merge |
| coco-tasks (modules.md) | Basic TaskManager → full background exec | TaskState 7-variant union, isBackgrounded flag, 3 entry points (explicit/auto/Ctrl+B), task output persistence (5GB cap), `<task-notification>` XML, PlanFileManager CRUD |

### False Positives Identified

| Area | Original assessment | Correction |
|------|-------------------|------------|
| 04 System Reminder | MISSING (CRITICAL) | N/A — Rust architectural improvement, not a TS module port |
| 14 Code Indexing | N/A | Confirmed: TS has only detection (22 tool types), no indexing |
| 21 Steering (GrowthBook) | MISSING (HIGH) | L6 intentionally deferred per mapping. Actual steering is message queue (fixed separately) |
| 26 Background Agents | PARTIAL (HIGH) | Merged into task system (#13). Background exec is v1 core |
| 27 LSP Integration | MISSING (CRITICAL) | cocode strategy (copy from cocode-rs). No TS-first doc needed |

### Remaining Deferred (implementation-time)

| Priority | Gap | Phase |
|----------|-----|-------|
| P2 | AppState: 60+ fields (remote, notifications, attribution, tungsten, speculation, plugins, MCP, inbox) | Phase 7 |
| P2 | ErrorExt::telemetry_msg() — 遥测脱敏方法 (TS 有 TelemetrySafeError，cocode-rs 无对应) | Phase 2 (cocode-error 扩展) |
| P3 | coco-config cocode-rs patterns (ConfigSection trait, ConfigResolver) | Phase 2 |
| P3 | 工具执行错误 errno 保留 — 确保 IO 错误在 OTel 中保留操作系统级 errno | Phase 4 |
| P1 | coco-otel L2: span 层级体系 — cocode-rs 仅 session_span，缺 interaction→tool→hook 嵌套 | Phase 1 |
| P1 | coco-otel L3: ~53 应用事件 — cocode-rs 仅 7 事件，缺 query/session/config/oauth/mcp 等 | Phase 3 |
| P2 | coco-otel L4: 8+ 业务 metrics — 缺 token.usage, cost.usage, lines_of_code, session.count 等 | Phase 3 |
| P2 | coco-otel L5: 自定义 exporter — 缺 BigQuery, 1P Event Logging, Perfetto, Beta tracing | Phase 3 |
| — | coco-otel L6: 运营控制 (event sampling, killswitch, metrics opt-out, GrowthBook) | **暂不实现** |

## Cross-Review Round 5 (String→Enum Audit + Cross-Verification — April 2026)

### String→Enum Type Safety Audit — Completed

Systematic review of all struct/enum definitions across crate docs. Every `String` field evaluated
for enum replacement. 67 String fields confirmed correct (dynamic values); all identity fields
converted to typed enums.

| Change | Scope | Details |
|--------|-------|---------|
| ToolId enum added | coco-types | `Builtin(ToolName) \| Mcp { server, tool } \| Custom(String)` — custom serde via Display/FromStr (flat string wire format) |
| AgentTypeId enum added | coco-types | `Builtin(SubagentType) \| Custom(String)` — same pattern as ToolId |
| HookEventType expanded | coco-types | 7 → 27 variants, `#[non_exhaustive]`, strum derives |
| 6 new enums added | coco-types | MessageKind, HookOutcome, CommandAvailability, CommandSource, UserType, Entrypoint |
| NormalizedMessage redesigned | coco-types | Replaced `role: String` with enum variants User/Assistant |
| tool_name→tool_id | 10+ crate docs | All identity fields across query, permissions, coordinator, remote, tool, hooks |
| agent_type→AgentTypeId | 4 crate docs | tools, tasks, coordinator, tool |
| Tool input enums | coco-tools | GrepOutputMode, ConfigAction, LspAction — all with full derives |
| Context enums | coco-context | Platform, ShellKind — replaced String fields in SystemContext |
| All 31 enums annotated | coco-types | Proper `#[derive]`, `#[serde(rename_all)]`, Copy where applicable, Default where applicable |
| CLAUDE.md updated | CLAUDE.md | Type Ownership, Canonical Names, Document Map — all reflect new enums |

### New Gaps Found (Cross-Verification)

| # | Gap | What's wrong | Severity | Fix |
|---|-----|-------------|----------|-----|
| R5-1 | ToolName enum missing | Referenced by `ToolId::Builtin(ToolName)` but enum not defined | **FIXED** | Added 41-variant enum with as_str(), FromStr, serde to crate-coco-types.md |
| R5-2 | SubagentType enum missing | Referenced by `AgentTypeId::Builtin(SubagentType)` but enum not defined | **FIXED** | Added 7-variant enum with as_str(), FromStr, serde to crate-coco-types.md |
| R5-3 | ShellType vs ShellKind inconsistency | coco-hooks and coco-config used `ShellType` (undefined); coco-context defines `ShellKind` | **FIXED** | Unified to `ShellKind` in coco-hooks.md and coco-config.md |
| R5-4 | EffortValue vs EffortLevel inconsistency | crate-coco-types.md and crate-coco-inference.md used `EffortValue` (undefined); 10+ other refs use `EffortLevel` | **FIXED** | Unified to `EffortLevel` in coco-types.md and coco-inference.md |
| R5-5 | BuiltinPluginDefinition layer violation | **FIXED**: `manifest` is `serde_json::Value` in `coco-types`; `coco-plugins` deserializes it at the boundary | **Architecture** | No open action |
| R5-6 | SkillDefinition.hooks type mismatch | **FIXED**: `SkillDefinition.hooks` is `Option<serde_json::Value>` | **Architecture** | No open action |
| R5-7 | MessageRole undefined | **SUPERSEDED**: message APIs use `MessageKind`; system-reminder has a local role enum for injected blocks | **Minor** | No open action |

### Type Collision Audit (comprehensive cross-doc review)

Systematically collected all ~238 struct/enum definitions across 27 crate docs.
Cross-referenced with TS source to identify redundancy and collisions.

| # | Collision | Files | Analysis | Resolution |
|---|-----------|-------|----------|------------|
| R5-8 | ThinkingLevel name collision | coco-config (was enum None/Low/Med/High) vs coco-inference (struct {effort, budget, interleaved}) | Config enum had NO TS equivalent — TS uses capability checks. The struct IS needed for multi-provider (cocode-rs proven design). | **FIXED**: Removed config enum. ThinkingLevel struct moved to coco-types as canonical shared type. ModelInfo restored with `default_thinking_level: Option<ThinkingLevel>` and `supported_thinking_levels`. Rationale: multi-provider needs richer thinking abstraction than TS's simple ThinkingConfig. |
| R5-9 | TaskStateBase dual definition | **FIXED**: `coco-tasks` uses `coco_types::TaskStateBase` | Canonical owner is `coco-types`. | No open action |
| R5-10 | OAuthTokens collision | **ACCEPTED**: API and MCP token shapes are intentionally separate crate-local structs; docs use `ApiOAuthTokens` / `McpOAuthTokens` names where owner clarity matters | No cross-crate type collision in Rust paths. | No open action |

**Unified to ThinkingLevel only** (EffortLevel + ThinkingConfig eliminated):
- TS has EffortLevel (4 levels) + ThinkingConfig (3 variants) as separate types
- cocode-rs has only ThinkingLevel struct — proven design, no EffortLevel/ThinkingConfig
- ThinkingLevel is a strict superset: can express everything both TS types can, plus budget+interleaved
- ReasoningEffort (6 levels) is the effort dimension WITHIN ThinkingLevel (not a standalone type)
- Flow: user settings → ThinkingLevel → ModelInfo resolution → per-provider API params

### Stale Entry Cleanup

Round 4 "Remaining Deferred" items reviewed — all still valid:
- P2 AppState, P2 telemetry_msg, P3 config patterns, P3 errno, P1/P2 coco-otel L2-L5: no changes

## Cross-Review Round 6 (opencode Multi-Provider Comparison — April 2026)

Deep comparison of coco-rs multi-provider design against opencode (TS, 40+ providers) and cocode-rs (Rust, 6 providers).

### Design Decisions

| # | Decision | Analysis | Outcome |
|---|----------|----------|---------|
| R6-1 | ThinkingLevel.options (HashMap) | cocode-rs used typed fields per provider param (include_thoughts, reasoning_summary, interleaved) → poor extensibility (3 crate changes per new param). opencode uses variant dicts (data-driven, zero code changes for new params). | **ADOPTED**: ThinkingLevel keeps only 2 typed fields (effort, budget_tokens — truly universal). All provider-specific thinking params move to `options: HashMap<String, Value>`. thinking_convert retains `&ModelInfo` for budget validation/clamping, does typed conversion for effort/budget, then merges options as passthrough. |
| R6-2 | default_thinking_level type | Was `Option<ThinkingLevel>` (full struct with params). Causes duplication — same params defined in both default and supported_thinking_levels. | **FIXED**: Changed to `Option<ReasoningEffort>` — just a ref to an entry in supported_thinking_levels. Single source of truth. |
| R6-3 | ModelInfo.slug naming | "slug" is a web URL term. vercel-ai uses `model_id()`, opencode uses `id`, all LLM APIs use `model`. | **RENAMED**: `model_id` in coco-rs plan docs. Aligns with vercel-ai and industry convention. |
| R6-4 | sdk_namespace for Bedrock/Vertex | Proposed adding ProviderInfo.sdk_namespace to route ProviderOptions to different namespaces for same ProviderApi. | **REJECTED**: vercel-ai provider impls handle Bedrock/Vertex parameter differences internally. Adding sdk_namespace would leak provider impl details to config layer. |
| R6-5 | opencode-style variant concept | opencode has `model.variants: Record<string, Record<string, any>>` — named presets user can select. | **NOT ADOPTED as separate concept**: ThinkingLevel.options + supported_thinking_levels achieves the same: user selects effort name, system resolves full param set from supported list. More type-safe. |
| R6-6 | ReasoningSummary enum | Was a typed enum (None/Auto/Concise/Detailed) on ModelInfo. | **REMOVED**: Now a string value in ThinkingLevel.options (e.g., `"reasoningSummary": "auto"`). No longer needs a standalone type. |
| R6-7 | ModelInfo.options vs ThinkingLevel.options | cocode-rs has ModelInfo.options for all extensions. Need clear separation. | **CLARIFIED**: ModelInfo.options = non-thinking per-model params (store:false). ThinkingLevel.options = thinking-related per-effort-level params (reasoningSummary, interleaved). Different merge points in RequestBuilder (Step 4 vs Step 3b). |
| R6-8 | ModelSpec.provider type | Was `ProviderApi` enum. Doesn't support sub-provider routing (bedrock, vertex). | **CHANGED**: `provider: String` (free-form) + `api: ProviderApi` (for dispatch). String supports "bedrock"/"vertex" without enum expansion. Aligned with cocode-rs actual impl. |
| R6-9 | Plan docs missing cocode-rs fields | ModelInfo, ProviderInfo, InferenceContext, RequestBuilder pipeline were incomplete vs cocode-rs actual code. | **FIXED**: All plan docs updated to reflect cocode-rs actual fields (top_k, timeout_secs, shell_type, max_tool_output_chars, ProviderModel, interceptors, request_options_merge module). |
| R6-10 | Capability enum alignment | coco-rs had different variant names than cocode-rs (ToolUse vs ToolCalling, Thinking vs ExtendedThinking). | **FIXED**: Aligned with cocode-rs Capability enum (TextGeneration, ToolCalling, ExtendedThinking, ReasoningSummaries, ParallelToolCalls, etc.). |

### Files Modified

- `crate-coco-types.md`: ThinkingLevel (options field, removed interleaved/max_output_tokens), ModelSpec (provider: String + api), Capability aligned, ApplyPatchToolType aligned
- `crate-coco-config.md`: ModelInfo full alignment (all cocode-rs fields), default_thinking_level as ReasoningEffort, ProviderInfo with models/interceptors, ProviderModel
- `crate-coco-inference.md`: thinking_convert simplified (no ModelInfo param), RequestBuilder full pipeline, request_options_merge module, InferenceContext full fields
- `multi-provider-plan.md`: config examples updated, design decisions table, removed redundant type definitions
- `CLAUDE.md`: type ownership, canonical names updated

## Cross-Review Round 7 (Prompt-Cache Implementation — May 2026)

Implementation of `prompt-cache-design.md` (2456 lines). Primary references: TS `services/prompt-caching/`, `betas.ts`, `should1hCacheTTL`. See round-3 findings (R3-F1..F8) embedded in the design doc.

### Implementation summary

| Layer | Modules introduced | Owner |
|---|---|---|
| `coco-types` | `cache.rs` (`PromptCacheMode`, `CacheTtl`, `CacheScope`, `PromptCacheConfig`, `BetaCapability`, `AccountKind`); 5 new `Capability` variants (`PromptCache`, `Context1m`, `InterleavedThinking`, `ContextManagement`, `TokenEfficientTools`) | provider-neutral |
| `coco-config` | `prompt_cache_settings.rs` — `PromptCacheRuntimeConfig`, `AnthropicRuntimeKnobs`, `AccountConfig`; 5 new `EnvKey` variants (`COCO_PROMPT_CACHE_ALLOWLIST`, `COCO_ANTHROPIC_*`); 3 new `RuntimeConfig` sections | settings layering |
| `coco-config` registry | builtin Claude models declare `PromptCache` + topology-relevant capabilities | per-model truth |
| `services/inference` | `cache_convert.rs` (pass-through emission of `cacheStrategy` / `requestedBetas` / `agenticQuery` / `querySource`); `build_call_options` returns `(call, merged_extra)` so detector hashes the merged map directly (Finding 5); `fingerprint.rs` extended with `runtime_state_digest` so settings-reload invalidation works (design §19.3 attack γ) | provider-agnostic |
| `vercel-ai-anthropic` | `cache_policy.rs` (`OnceLock` eligibility + allowlist latches, R3-F3); `beta_resolver.rs` (single source of truth for which betas a request emits, F7-deterministic sorted set); `beta_capabilities.rs` (typed enum → kebab-case header string, two-hop translation); `cache_placement.rs` (auto-marker on last user content block, design §10.3); `INTERNAL_ANTHROPIC_OPTION_KEYS` deny-list strips internal signals from raw map (Finding 2) | adapter-only policy |
| `vercel-ai-anthropic` `prepare_tools` | memory tool branch gated on shared `should_emit_context_management` predicate (R3-F2) — three sites now agree (body insert + memory tool + beta header) | shared predicate |

### Design decisions

| # | Decision | Outcome |
|---|----------|---------|
| R7-1 | Where does Anthropic-specific policy live? | **Adapter (`vercel-ai-anthropic`)**, not `services/inference`. Mirrors `services/inference/CLAUDE.md` rule "auth/OAuth/prompt-cache/rate-limit live in vercel-ai-anthropic". Inference layer only emits a typed pass-through map. |
| R7-2 | How do session-stable fields cross the L0 boundary? | Adapter-local mirror types (`AdapterAccountKind`, `AdapterCacheMode`, `AdapterCacheTtl`, `AdapterCacheScope`, `AdapterBetaCapability`) with **identical wire JSON** to `coco_types::*`. Translation happens in `services/inference::model_factory::build_anthropic` via `anthropic_caps_from(Option<&Vec<Capability>>)`. Round-trip stability checked by `wire_round_trip` tests. |
| R7-3 | When to invalidate the cached `Arc<dyn LanguageModelV4>` after settings reload? | Extended `ProviderClientFingerprint` with `runtime_state_digest` (SHA-256 over `account` + `prompt_cache` + `anthropic_knobs`). Mismatch at turn-boundary check rebuilds the client so flips of `account.kind`, `prompt_cache.allowlist`, or any `anthropic_knobs.*` propagate without process restart. |
| R7-4 | How to prevent internal-only signals from leaking into the wire body? | `INTERNAL_ANTHROPIC_OPTION_KEYS` (`cacheStrategy`, `requestedBetas`, `agenticQuery`, `querySource`) stripped from raw map before shallow-merge (design §10.1.5 / Finding 2). E2E test asserts none appear in `body.*`. |
| R7-5 | How to keep `betas` deterministic for snapshot tests + cache-break detector? | `BTreeSet<String>` in `ResolvedBetas`; final `anthropic-beta` header is `sort_unstable + join(',')` so wire output is byte-stable across runs (Finding 7). |
| R7-6 | Who owns the 1h-TTL eligibility latch? | Adapter `CachePolicy::eligible_1h: OnceLock<bool>`. Computed once on first call from `(account_kind, in_overage)`. Mid-session billing flip never silently upgrades — flipping requires settings reload that bumps the fingerprint and rebuilds the model. |
| R7-7 | Memory tool / context-management body / beta header — three sites, one rule | All three gate on `beta_resolver::should_emit_context_management(&AnthropicConfig)`. Fixes R3-F2: a half-emitted state (e.g. memory tool present without beta header) is now structurally impossible. Memory tool dropped + warning if predicate fails. |
| R7-8 | Bedrock auth / `ProviderTopology::Bedrock` | **Deferred** (design Non-Goal §2). Single-variant `ProviderTopology::FirstParty` keeps `matches!(topology, FirstParty)` predicates correct; future Bedrock PR adds variant + auth + bedrock_1h_env in one shot to keep half-states unrepresentable. |
| R7-9 | Ant-only gates (`cli-internal-2026-02-09`, `summarize-connector-text-*`, ant-context-management) | **Not ported** (§3.5). These are internal to Anthropic infra and would be no-ops for public users. |

### Files changed

- `coco-rs/common/types/src/{cache.rs,cache.test.rs,provider.rs,lib.rs}`
- `coco-rs/common/config/src/{prompt_cache_settings.rs,prompt_cache_settings.test.rs,settings/mod.rs,env.rs,runtime.rs,lib.rs,model/registry.rs,model/registry.test.rs}`
- `coco-rs/services/inference/src/{cache_convert.rs,cache_convert.test.rs,build_call_options.rs,build_call_options.test.rs,client.rs,client.test.rs,fingerprint.rs,model_factory.rs,lib.rs}`
- `coco-rs/vercel-ai/anthropic/src/{anthropic_config.rs,anthropic_provider.rs,beta_capabilities.rs,beta_capabilities.test.rs,beta_resolver.rs,beta_resolver.test.rs,cache_placement.rs,cache_placement.test.rs,cache_policy.rs,cache_policy.test.rs,lib.rs}`
- `coco-rs/vercel-ai/anthropic/src/messages/{anthropic_messages_options.rs,anthropic_messages_language_model.rs,anthropic_messages_language_model.test.rs,prepare_tools.rs,prepare_tools.test.rs}`

### Round 7 follow-up — Anthropic knobs migrated to per-provider `provider_options` (May 2026)

Round-7 had landed `AnthropicRuntimeKnobs` as a workspace-level `RuntimeConfig` section. That solved the immediate need but did not scale: the same shape would have to repeat for every future provider's behavior knobs, and `coco-config` (provider-neutral) would accumulate provider-specific schema. Migrated to the design that does scale.

| # | Decision | Outcome |
|---|----------|---------|
| R7-10 | Where do Anthropic-specific behavior knobs live? | **Per-provider-instance opaque map.** `ProviderConfig.provider_options: BTreeMap<String, Value>` carries the knobs; `vercel-ai-anthropic::parse_provider_options` parses them via an adapter-owned typed struct (`AnthropicProviderOptionsConfig`, `deny_unknown_fields`). `coco-config` is now provider-neutral end-to-end. |
| R7-11 | Settings.json shape changed? | Yes — `settings.anthropic_knobs.*` is replaced by `settings.providers.<name>.provider_options.{experimental_betas,disable_interleaved_thinking,show_thinking_summaries,non_interactive}`. Defaults match TS `betas.ts` parity (only `experimental_betas` defaults `true`). |
| R7-12 | Env var support for the four knobs? | **Dropped.** No `COCO_ANTHROPIC_*` env vars. Settings.json is canonical. The 4 `EnvKey::CocoAnthropic*` variants are removed. (No installed-base concern — Round 7 was the first ship.) |
| R7-13 | What replaces the `anthropic_knobs` slice in `runtime_state_digest`? | Per-provider `provider_options` map. `digest_runtime_state(&account, &prompt_cache, &provider_cfg.provider_options)` hashes a tagged sequence of `(key, canonical-JSON value)` pairs. Per-provider scoping means a knob flip on one Anthropic instance doesn't churn an unrelated instance's client. |
| R7-14 | Why an opaque `BTreeMap<String, Value>` at the config layer instead of a typed slot per provider? | Extensibility. Adding a future provider knob (OpenAI, Google, …) only requires the owning `vercel-ai-<provider>` crate to define its own `parse_provider_options` — no `coco-config` schema change. Mirrors `client_options.headers` semantics: opaque to the transport, typed at the consumer. |
| R7-15 | Merge semantics across settings layers? | Key-by-key shallow merge — overlay wins per key; `Value::Null` removes a key. Identical to `client_options.headers`. |

Removed (no longer in coco-config): `AnthropicRuntimeKnobs`, `PartialAnthropicKnobsSettings`, `Settings.anthropic_knobs`, `RuntimeConfig.anthropic_knobs`, `EnvKey::CocoAnthropicExperimentalBetas`, `EnvKey::CocoAnthropicDisableInterleavedThinking`, `EnvKey::CocoAnthropicShowThinkingSummaries`, `EnvKey::CocoAnthropicNonInteractive`. Re-exports in `coco-config::lib` dropped.

Added (in `vercel-ai-anthropic`): `provider_options.rs` with `AnthropicProviderOptionsConfig`, `parse_provider_options`, `ProviderOptionsError`. Added (in `coco-config`): `PartialProviderConfig.provider_options: Option<BTreeMap<String, Value>>` + `ProviderConfig.provider_options: BTreeMap<String, Value>` with key-by-key merge.
